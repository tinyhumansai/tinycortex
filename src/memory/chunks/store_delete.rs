//! Chunk deletion by source / source-prefix / owner, with cascade cleanup of
//! the dependent score / entity-index / embedding side tables, the source
//! ingest gate, and any on-disk content files.
//!
//! Unlike OpenHuman, this slice does **not** cascade into summary trees (that
//! subsystem is not ported here); it deletes only the chunk-owned rows and
//! files.
//!
//! ## Deletion completeness gaps (audit findings SC-7, SC-20)
//! - Only files at `content_path` are removed. A chunk's `raw_refs_json`
//!   pointers (the raw-archive mirror — the *only* copy of the body for
//!   email chunks, see [`super::raw_refs`]) are never parsed here, so their
//!   files are never deleted, and no reachability check runs to see whether
//!   another surviving chunk still references the same raw file. Deleting an
//!   email account's chunks leaves every message body on disk.
//! - Matching `RAW_FILE_GATE_KIND` rows in `mem_tree_ingested_sources` (the
//!   raw-archive ingest gate — see [`super::store::RAW_FILE_GATE_KIND`]) are
//!   never cleared alongside the deleted chunks.
//! - Every public entry point loads every chunk row for the source kind into
//!   memory and filters in Rust, then issues five `DELETE` statements per
//!   matched chunk — `O(N·M)` round-trips for `N` chunks touched across `M`
//!   dependent tables, rather than a single set-based `DELETE ... WHERE id IN
//!   (subquery)` per table.

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
///
/// Idempotent: deleting an already-empty/nonexistent source returns `Ok(0)`
/// rather than erroring. See the module doc for what this does *not* clean up
/// (raw-archive files, raw-file gate rows).
///
/// # Errors
/// See `delete_chunks_by_source_filter`.
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
///
/// # Errors
/// See `delete_chunks_by_source_filter`.
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
///
/// Unlike [`delete_chunks_by_source`] / [`delete_chunks_by_source_prefix`],
/// this never removes an ingest-gate row directly by owner match (the gate
/// table has no `owner` column) — a gate row is only removed here as a
/// side effect of its `source_id` becoming fully orphaned (every chunk under
/// that source id deleted), independent of which owner triggered that.
///
/// # Errors
/// See `delete_chunks_by_source_filter`.
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

/// Shared implementation behind [`delete_chunks_by_source`],
/// [`delete_chunks_by_source_prefix`], and [`delete_chunks_by_owner`].
///
/// Loads every chunk row for `source_kind`, keeps those where `matches_chunk`
/// (given `(source_id, owner)`) returns `true`, then — inside one
/// transaction — deletes each matched chunk's score/entity-index/embedding/
/// reembed-skip rows before the chunk row itself, and finally removes any
/// `mem_tree_ingested_sources` row whose `source_id` either satisfies
/// `matches_ingested_source` directly or became fully orphaned (zero
/// remaining chunks) as a result of this delete. On-disk `content_path`
/// files are collected during the transaction but removed only after it
/// commits, via [`remove_chunk_content_files`] (best-effort; a filesystem
/// failure there does not roll back or fail the DB-side delete, and does not
/// surface as an `Err` from this function).
///
/// # Errors
/// Returns `Err` if any `SELECT`/`DELETE` statement or the transaction commit
/// fails. On error, no chunk/index rows are removed — the whole batch rolls
/// back with the transaction. (Content-file removal never contributes to an
/// `Err` here; see above.)
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

        for source_id in &orphaned_deleted_sources {
            if let Some(tree) = crate::memory::tree::store::get_tree_by_scope_conn(
                &tx,
                crate::memory::tree::store::TreeKind::Source,
                source_id,
            )? {
                let cascade = crate::memory::tree::store::delete_tree_cascade_tx(&tx, &tree.id)?;
                content_paths.extend(cascade.content_paths);
            }
        }

        let deleted = chunks.len();
        tx.commit()?;
        Ok(deleted)
    })?;

    remove_chunk_content_files(config, &content_paths);
    Ok(deleted)
}

/// Remove stale gates and a source-scoped tree once no chunks remain.
pub fn delete_orphaned_source_tree(
    config: &MemoryConfig,
    source_kind: SourceKind,
    source_id: &str,
) -> Result<bool> {
    let mut content_paths = Vec::new();
    let cascaded = with_connection(config, |conn| {
        let tx = conn.unchecked_transaction()?;
        let remaining: i64 = tx.query_row(
            "SELECT COUNT(*) FROM mem_tree_chunks WHERE source_kind=?1 AND source_id=?2",
            params![source_kind.as_str(), source_id],
            |row| row.get(0),
        )?;
        if remaining > 0 {
            return Ok(false);
        }
        let versioned_prefix = format!("{source_id}@");
        let gate_ids = {
            let mut stmt =
                tx.prepare("SELECT source_id FROM mem_tree_ingested_sources WHERE source_kind=?1")?;
            let rows =
                stmt.query_map(params![source_kind.as_str()], |row| row.get::<_, String>(0))?;
            rows.filter_map(|row| match row {
                Ok(id) if id == source_id || id.starts_with(&versioned_prefix) => Some(Ok(id)),
                Ok(_) => None,
                Err(error) => Some(Err(error)),
            })
            .collect::<rusqlite::Result<Vec<_>>>()?
        };
        for gate_id in gate_ids {
            tx.execute(
                "DELETE FROM mem_tree_ingested_sources WHERE source_kind=?1 AND source_id=?2",
                params![source_kind.as_str(), gate_id],
            )?;
        }
        let cascaded = if let Some(tree) = crate::memory::tree::store::get_tree_by_scope_conn(
            &tx,
            crate::memory::tree::store::TreeKind::Source,
            source_id,
        )? {
            let deleted = crate::memory::tree::store::delete_tree_cascade_tx(&tx, &tree.id)?;
            content_paths.extend(deleted.content_paths);
            true
        } else {
            false
        };
        tx.commit()?;
        Ok(cascaded)
    })?;
    remove_chunk_content_files(config, &content_paths);
    Ok(cascaded)
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
