//! Chunk deletion by source / source-prefix / owner, with cascade cleanup of
//! the dependent score / entity-index / embedding side tables, the source
//! ingest gate, and any on-disk content files.
//!
//! Unlike OpenHuman, this slice does **not** cascade into summary trees (that
//! subsystem is not ported here); it deletes only the chunk-owned rows and
//! files.
//!
//! Raw archives are cascade-cleaned by reachability: a file and its raw-file
//! ingest gate are removed only after the transaction confirms no surviving
//! chunk references that path. A malformed surviving pointer row makes cleanup
//! conservative and preserves all candidate raw files.
//!
//! Selection and dependent-row cleanup are set-based in SQLite. Rust only
//! materialises the matched rows whose filesystem pointers and source-tree
//! scopes must be processed after the database transaction commits.

use anyhow::{Context, Result};
use rusqlite::params;
use std::collections::HashSet;

use super::connection::with_connection;
use super::content_root;
use super::raw_refs::RawRef;
use super::store_sources::RAW_FILE_GATE_KIND;
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
    delete_chunks_by_source_filter(config, source_kind, DeleteFilter::ExactSource(source_id))
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
        DeleteFilter::SourcePrefix(source_id_prefix),
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
    delete_chunks_by_source_filter(config, source_kind, DeleteFilter::Owner(owner))
}

#[derive(Clone, Copy)]
enum DeleteFilter<'a> {
    ExactSource(&'a str),
    SourcePrefix(&'a str),
    Owner(&'a str),
}

impl<'a> DeleteFilter<'a> {
    fn value(self) -> &'a str {
        match self {
            Self::ExactSource(value) | Self::SourcePrefix(value) | Self::Owner(value) => value,
        }
    }

    fn chunk_predicate(self) -> &'static str {
        match self {
            Self::ExactSource(_) => "source_id = ?2",
            Self::SourcePrefix(_) => "substr(source_id, 1, length(?2)) = ?2",
            Self::Owner(_) => "owner = ?2",
        }
    }
}

/// Shared implementation behind [`delete_chunks_by_source`],
/// [`delete_chunks_by_source_prefix`], and [`delete_chunks_by_owner`].
///
/// Selects matching rows into a temporary SQLite table, then deletes each
/// dependent table with one set-based statement before removing the chunk
/// rows. Ingest gates selected directly or made fully orphaned are removed in
/// the same transaction. On-disk `content_path` and unreferenced
/// `raw_refs_json` files are collected during the transaction but removed only after it
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
    filter: DeleteFilter<'_>,
) -> Result<usize> {
    let mut content_paths = Vec::new();
    let deleted = with_connection(config, |conn| {
        let tx = conn.unchecked_transaction()?;

        tx.execute_batch(
            "CREATE TEMP TABLE IF NOT EXISTS mem_tree_delete_selection (
                id TEXT PRIMARY KEY,
                source_id TEXT NOT NULL,
                path_scope TEXT,
                content_path TEXT,
                raw_refs_json TEXT
             );
             DELETE FROM mem_tree_delete_selection;",
        )?;
        let selection_sql = format!(
            "INSERT INTO mem_tree_delete_selection
                (id, source_id, path_scope, content_path, raw_refs_json)
             SELECT id, source_id, path_scope, content_path, raw_refs_json
               FROM mem_tree_chunks
              WHERE source_kind = ?1 AND {}",
            filter.chunk_predicate()
        );
        tx.execute(
            &selection_sql,
            params![source_kind.as_str(), filter.value()],
        )?;

        let chunks = {
            let mut stmt = tx.prepare(
                "SELECT id, source_id, content_path, path_scope, raw_refs_json
                   FROM mem_tree_delete_selection",
            )?;
            let rows = stmt.query_map([], |row| {
                Ok((
                    row.get::<_, String>(0)?,
                    row.get::<_, String>(1)?,
                    row.get::<_, Option<String>>(2)?,
                    row.get::<_, Option<String>>(3)?,
                    row.get::<_, Option<String>>(4)?,
                ))
            })?;
            rows.collect::<rusqlite::Result<Vec<_>>>()
                .context("Failed to collect chunks by source")?
        };

        let deleted_tree_scopes: HashSet<String> = chunks
            .iter()
            .map(|(_, source_id, _, path_scope, _)| {
                path_scope.clone().unwrap_or_else(|| source_id.clone())
            })
            .collect();

        let raw_path_candidates: HashSet<String> = chunks
            .iter()
            .filter_map(|(_, _, _, _, json)| json.as_deref())
            .filter_map(|json| serde_json::from_str::<Vec<RawRef>>(json).ok())
            .flatten()
            .map(|raw_ref| raw_ref.path)
            .collect();

        for (_chunk_id, _source_id, content_path, _path_scope, _raw_refs_json) in &chunks {
            if let Some(path) = content_path.as_ref().filter(|path| !path.is_empty()) {
                content_paths.push(path.clone());
            }
        }
        tx.execute(
            "DELETE FROM mem_tree_score
              WHERE chunk_id IN (SELECT id FROM mem_tree_delete_selection)",
            [],
        )?;
        tx.execute(
            "DELETE FROM mem_tree_entity_index
              WHERE node_id IN (SELECT id FROM mem_tree_delete_selection)",
            [],
        )?;
        tx.execute(
            "DELETE FROM mem_tree_chunk_embeddings
              WHERE chunk_id IN (SELECT id FROM mem_tree_delete_selection)",
            [],
        )?;
        tx.execute(
            "DELETE FROM mem_tree_chunk_reembed_skipped
              WHERE chunk_id IN (SELECT id FROM mem_tree_delete_selection)",
            [],
        )?;
        tx.execute(
            "DELETE FROM mem_tree_chunks
              WHERE id IN (SELECT id FROM mem_tree_delete_selection)",
            [],
        )?;

        // Clean raw files only when every surviving pointer row is readable.
        // Otherwise an unknown reference could make deleting a candidate file
        // destructive, so fail closed and leave the archive untouched.
        let mut surviving_raw_paths = HashSet::new();
        let mut has_corrupt_surviving_refs = false;
        {
            let mut stmt = tx.prepare(
                "SELECT raw_refs_json FROM mem_tree_chunks
                  WHERE raw_refs_json IS NOT NULL AND raw_refs_json != ''",
            )?;
            let rows = stmt.query_map([], |row| row.get::<_, String>(0))?;
            for row in rows {
                match serde_json::from_str::<Vec<RawRef>>(&row?) {
                    Ok(refs) => surviving_raw_paths.extend(refs.into_iter().map(|r| r.path)),
                    Err(_) => has_corrupt_surviving_refs = true,
                }
            }
        }
        if !has_corrupt_surviving_refs {
            for path in raw_path_candidates.difference(&surviving_raw_paths) {
                tx.execute(
                    "DELETE FROM mem_tree_ingested_sources
                      WHERE source_kind = ?1 AND source_id = ?2",
                    params![RAW_FILE_GATE_KIND, path],
                )?;
                content_paths.push(path.clone());
            }
        }

        // A fully-orphaned source (no chunks left) has its ingest gate removed.
        tx.execute(
            "DELETE FROM mem_tree_ingested_sources AS gate
              WHERE gate.source_kind = ?1
                AND gate.source_id IN (
                    SELECT DISTINCT source_id FROM mem_tree_delete_selection
                )
                AND NOT EXISTS (
                    SELECT 1 FROM mem_tree_chunks AS chunk
                     WHERE chunk.source_kind = gate.source_kind
                       AND chunk.source_id = gate.source_id
                )",
            params![source_kind.as_str()],
        )?;
        match filter {
            DeleteFilter::ExactSource(source_id) => {
                tx.execute(
                    "DELETE FROM mem_tree_ingested_sources
                      WHERE source_kind = ?1 AND source_id = ?2",
                    params![source_kind.as_str(), source_id],
                )?;
            }
            DeleteFilter::SourcePrefix(prefix) => {
                tx.execute(
                    "DELETE FROM mem_tree_ingested_sources
                      WHERE source_kind = ?1
                        AND substr(source_id, 1, length(?2)) = ?2",
                    params![source_kind.as_str(), prefix],
                )?;
            }
            DeleteFilter::Owner(_) => {}
        }

        for scope in &deleted_tree_scopes {
            let remaining: i64 = tx.query_row(
                "SELECT COUNT(*) FROM mem_tree_chunks
                  WHERE source_kind = ?1 AND COALESCE(path_scope, source_id) = ?2",
                params![source_kind.as_str(), scope],
                |row| row.get(0),
            )?;
            if remaining != 0 {
                continue;
            }
            if let Some(tree) = crate::memory::tree::store::get_tree_by_scope_conn(
                &tx,
                crate::memory::tree::store::TreeKind::Source,
                scope,
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
