//! Chunk deletion by source / source-prefix / owner, with cascade cleanup of
//! the dependent score / entity-index / embedding side tables, the source
//! ingest gate, and any on-disk content files.
//!
//! Unlike OpenHuman, this slice does **not** cascade into summary trees (that
//! subsystem is not ported here); it deletes only the chunk-owned rows and
//! files.

use anyhow::{Context, Result};
use rusqlite::params;
use std::collections::HashSet;

use super::connection::with_connection;
use super::content_root;
use super::types::SourceKind;
use crate::memory::config::MemoryConfig;

/// Delete all chunk rows for one exact `(source_kind, source_id)` and clear
/// dependent source-local indexes + the ingest gate. Returns the number of
/// chunk rows removed.
pub fn delete_chunks_by_source(
    config: &MemoryConfig,
    source_kind: SourceKind,
    source_id: &str,
) -> Result<usize> {
    delete_chunks_by_source_filter(
        config,
        source_kind,
        |candidate, _owner| candidate == source_id,
        |candidate| candidate == source_id,
    )
}

/// Delete all chunk rows whose source id starts with `source_id_prefix`.
///
/// Rust-side prefix filter (not SQL `LIKE`) so provider ids containing `_` /
/// `%` are treated literally.
pub fn delete_chunks_by_source_prefix(
    config: &MemoryConfig,
    source_kind: SourceKind,
    source_id_prefix: &str,
) -> Result<usize> {
    delete_chunks_by_source_filter(
        config,
        source_kind,
        |candidate, _owner| candidate.starts_with(source_id_prefix),
        |candidate| candidate.starts_with(source_id_prefix),
    )
}

/// Delete all chunk rows for one exact `(source_kind, owner)` while preserving
/// source ingest gates that still have chunks owned by another connection.
pub fn delete_chunks_by_owner(
    config: &MemoryConfig,
    source_kind: SourceKind,
    owner: &str,
) -> Result<usize> {
    delete_chunks_by_source_filter(
        config,
        source_kind,
        |_source_id, candidate_owner| candidate_owner == owner,
        |_source_id| false,
    )
}

fn delete_chunks_by_source_filter(
    config: &MemoryConfig,
    source_kind: SourceKind,
    matches_chunk: impl Fn(&str, &str) -> bool,
    matches_ingested_source: impl Fn(&str) -> bool,
) -> Result<usize> {
    let mut content_paths = Vec::new();
    let deleted = with_connection(config, |conn| {
        let tx = conn.unchecked_transaction()?;

        let chunks = {
            let mut stmt = tx.prepare(
                "SELECT id, source_id, owner, content_path
                   FROM mem_tree_chunks
                  WHERE source_kind = ?1",
            )?;
            let rows = stmt.query_map(params![source_kind.as_str()], |row| {
                Ok((
                    row.get::<_, String>(0)?,
                    row.get::<_, String>(1)?,
                    row.get::<_, String>(2)?,
                    row.get::<_, Option<String>>(3)?,
                ))
            })?;
            rows.filter_map(|row| match row {
                Ok((id, source_id, owner, content_path)) if matches_chunk(&source_id, &owner) => {
                    Some(Ok((id, source_id, content_path)))
                }
                Ok(_) => None,
                Err(error) => Some(Err(error)),
            })
            .collect::<rusqlite::Result<Vec<_>>>()
            .context("Failed to collect chunks by source")?
        };

        let deleted_source_ids: HashSet<String> = chunks
            .iter()
            .map(|(_, source_id, _)| source_id.clone())
            .collect();

        for (chunk_id, _source_id, content_path) in &chunks {
            tx.execute(
                "DELETE FROM mem_tree_score WHERE chunk_id = ?1",
                params![chunk_id],
            )?;
            tx.execute(
                "DELETE FROM mem_tree_entity_index WHERE node_id = ?1",
                params![chunk_id],
            )?;
            tx.execute(
                "DELETE FROM mem_tree_chunk_embeddings WHERE chunk_id = ?1",
                params![chunk_id],
            )?;
            tx.execute(
                "DELETE FROM mem_tree_chunk_reembed_skipped WHERE chunk_id = ?1",
                params![chunk_id],
            )?;
            tx.execute(
                "DELETE FROM mem_tree_chunks WHERE id = ?1",
                params![chunk_id],
            )?;
            if let Some(path) = content_path.as_ref().filter(|path| !path.is_empty()) {
                content_paths.push(path.clone());
            }
        }

        // A fully-orphaned source (no chunks left) has its ingest gate removed.
        let mut orphaned_deleted_sources = HashSet::new();
        for source_id in &deleted_source_ids {
            let remaining: i64 = tx.query_row(
                "SELECT COUNT(*)
                   FROM mem_tree_chunks
                  WHERE source_kind = ?1 AND source_id = ?2",
                params![source_kind.as_str(), source_id],
                |row| row.get(0),
            )?;
            if remaining == 0 {
                orphaned_deleted_sources.insert(source_id.clone());
            }
        }

        let ingested_sources = {
            let mut stmt = tx.prepare(
                "SELECT source_id
                   FROM mem_tree_ingested_sources
                  WHERE source_kind = ?1",
            )?;
            let rows =
                stmt.query_map(params![source_kind.as_str()], |row| row.get::<_, String>(0))?;
            rows.filter_map(|row| match row {
                Ok(source_id)
                    if matches_ingested_source(&source_id)
                        || orphaned_deleted_sources.contains(&source_id) =>
                {
                    Some(Ok(source_id))
                }
                Ok(_) => None,
                Err(error) => Some(Err(error)),
            })
            .collect::<rusqlite::Result<Vec<_>>>()
            .context("Failed to collect ingested sources")?
        };

        for source_id in &ingested_sources {
            tx.execute(
                "DELETE FROM mem_tree_ingested_sources
                  WHERE source_kind = ?1 AND source_id = ?2",
                params![source_kind.as_str(), source_id],
            )?;
        }

        let deleted = chunks.len();
        tx.commit()?;
        Ok(deleted)
    })?;

    remove_chunk_content_files(config, &content_paths);
    Ok(deleted)
}

/// Best-effort removal of on-disk chunk content files, with strict sandboxing:
/// a `content_path` that escapes the content root (via `..`, an absolute path,
/// or a symlink pointing outside) is refused rather than followed.
fn remove_chunk_content_files(config: &MemoryConfig, content_paths: &[String]) {
    use std::path::{Component, Path};

    let root = content_root(config);
    let canonical_root = match std::fs::canonicalize(&root) {
        Ok(path) => path,
        Err(_) => return,
    };

    for rel in content_paths {
        let rel_path = Path::new(rel);
        let has_escape_component = rel_path.components().any(|component| {
            matches!(
                component,
                Component::ParentDir | Component::RootDir | Component::Prefix(_)
            )
        });
        if has_escape_component {
            continue;
        }

        let path = root.join(rel_path);
        // Resolve symlinks so a link pointing outside the content root is
        // detected; but remove the link entry itself (not its target).
        let resolved_path = match std::fs::canonicalize(&path) {
            Ok(path) => path,
            Err(_) => continue,
        };
        if !resolved_path.starts_with(&canonical_root) {
            continue;
        }

        let _ = std::fs::remove_file(&path);
    }
}
