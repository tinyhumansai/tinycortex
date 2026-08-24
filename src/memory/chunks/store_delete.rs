//! Chunk deletion by source / source-prefix / owner / id, and the whole-tier
//! purge, with cascade cleanup of the dependent score / entity-index /
//! embedding side tables, the source ingest gate, and any on-disk content
//! files.
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
use rusqlite::{params, OptionalExtension, Transaction};
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

/// Delete one chunk by its id, with the same dependent-row, ingest-gate and
/// content-file cleanup the source-scoped deletes perform.
///
/// Returns `1` when the chunk existed and `0` when it did not — deleting an
/// unknown id is not an error, matching the idempotence of the other deletes.
///
/// The stored source kind is read first because the shared implementation
/// needs it to decide whether the source's ingest gate and source-scoped
/// summary tree have just been orphaned. It cannot go stale between the read
/// and the delete: a chunk id is derived from its source kind, so a row whose
/// kind changed is a row whose id changed.
///
/// # Errors
/// Returns `Err` if the kind lookup fails, if the stored kind is not one this
/// build knows (a hand-edited DB), or for the reasons
/// `delete_chunks_by_source_filter` documents.
pub fn delete_chunk_by_id(config: &MemoryConfig, chunk_id: &str) -> Result<usize> {
    let stored_kind = with_connection(config, |conn| {
        let stored = conn
            .query_row(
                "SELECT source_kind FROM mem_tree_chunks WHERE id = ?1",
                params![chunk_id],
                |row| row.get::<_, String>(0),
            )
            .optional()
            .context("Failed to read the source kind of the chunk to delete")?;
        Ok(stored)
    })?;
    let Some(stored_kind) = stored_kind else {
        return Ok(0);
    };
    let source_kind = SourceKind::parse(&stored_kind).map_err(|error| {
        anyhow::anyhow!("Chunk {chunk_id} has an unusable source kind: {error}")
    })?;
    delete_chunks_by_source_filter(config, source_kind, DeleteFilter::Chunk(chunk_id))
}

#[derive(Clone, Copy)]
enum DeleteFilter<'a> {
    ExactSource(&'a str),
    SourcePrefix(&'a str),
    Owner(&'a str),
    Chunk(&'a str),
}

impl<'a> DeleteFilter<'a> {
    fn value(self) -> &'a str {
        match self {
            Self::ExactSource(value)
            | Self::SourcePrefix(value)
            | Self::Owner(value)
            | Self::Chunk(value) => value,
        }
    }

    fn chunk_predicate(self) -> &'static str {
        match self {
            Self::ExactSource(_) => "source_id = ?2",
            Self::SourcePrefix(_) => "substr(source_id, 1, length(?2)) = ?2",
            Self::Owner(_) => "owner = ?2",
            Self::Chunk(_) => "id = ?2",
        }
    }

    /// The `source_kind = ?1 AND` prefix the source-scoped filters select on,
    /// and nothing for the by-id filter.
    ///
    /// `id` is the primary key, so a kind ANDed onto it can never narrow the
    /// match — it can only make the whole delete silently select nothing when
    /// the caller's kind and the stored one disagree. `?1` stays bound either
    /// way: SQLite sizes a statement's parameter list by the highest index it
    /// names, so an unused `?1` beside a used `?2` is legal, and all four
    /// filters keep the one binding site below.
    fn source_kind_clause(self) -> &'static str {
        match self {
            Self::ExactSource(_) | Self::SourcePrefix(_) | Self::Owner(_) => {
                "source_kind = ?1 AND "
            }
            Self::Chunk(_) => "",
        }
    }
}

/// Shared implementation behind [`delete_chunks_by_source`],
/// [`delete_chunks_by_source_prefix`], [`delete_chunks_by_owner`], and
/// [`delete_chunk_by_id`].
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
              WHERE {}{}",
            filter.source_kind_clause(),
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
            // Neither an owner nor a single chunk id names a source, so
            // neither may remove an ingest gate on its own — chunks of that
            // source owned by someone else, or simply other chunks, can still
            // be there. The orphan sweep above already takes the gate in the
            // case where they are not.
            DeleteFilter::Owner(_) | DeleteFilter::Chunk(_) => {}
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

/// Every table [`purge_all`] empties before `mem_tree_chunks`, in the order it
/// empties them.
///
/// The order is load-bearing. `PRAGMA foreign_keys` is ON for every connection
/// this module hands out, so a parent whose children are still present cannot
/// be deleted: `mem_tree_summaries` and `mem_tree_buffers` go before the
/// `mem_tree_trees` rows they reference. The four embedding/tombstone sidecars
/// are listed explicitly even though their foreign keys declare
/// `ON DELETE CASCADE` — the same belt-and-braces `purge_global_topic_trees`
/// uses, so this keeps emptying them if a future schema revision drops a
/// cascade.
const PURGE_TABLES_BEFORE_CHUNKS: &[&str] = &[
    "mem_tree_score",
    "mem_tree_entity_index",
    "mem_tree_entity_edges",
    "mem_tree_entity_hotness",
    "mem_tree_jobs",
    "mem_tree_buffers",
    "mem_tree_summary_embeddings",
    "mem_tree_summary_reembed_skipped",
    "mem_tree_summaries",
    "mem_tree_trees",
    "mem_tree_chunk_embeddings",
    "mem_tree_chunk_reembed_skipped",
];

/// Empty the chunk tier: every chunk, every row keyed off one, the summary
/// trees built from them, the ingest gates that would otherwise refuse to
/// re-ingest anything, and the on-disk bodies they pointed at.
///
/// Returns the total number of rows removed, summed across every table this
/// purge empties — deliberately NOT the chunk-row count that
/// [`delete_chunks_by_source`] and its siblings return.
///
/// The unit differs from its siblings on purpose, and the reason is the caller
/// rather than this module: the only consumer is a whole-store wipe that has
/// always reported a cross-table sum, so returning chunk rows here would shrink
/// a number a user already reads without anything having changed about what was
/// forgotten. A scoped delete answers "how many chunks did this reach", which is
/// the caller's own unit; a purge answers "how much did emptying the store
/// remove", which is every row it touched.
///
/// The consequence is that the number moves when this purge learns about a new
/// table. That is accepted: a table added here is a table that was previously
/// left behind, so the count rising is the fix being visible rather than a
/// meaning change.
///
/// # What it deliberately does not clear
///
/// `mcp_writes`, the audit trail of tool-driven writes. It records that a
/// write happened, not what memory holds, and discarding an audit log is a
/// decision for whoever wants it discarded rather than a side effect of
/// emptying the store.
///
/// Summary trees are cleared here, unlike everywhere else in this module: a
/// tree built out of chunks that no longer exist is not memory this store can
/// explain, and the module's rule about not cascading into the summary tier is
/// about *which chunks* a scoped delete may reach beyond, not about leaving a
/// whole-store purge half done.
///
/// # Errors
/// Returns `Err` if any `DELETE`, the path collection, or the commit fails, in
/// which case the whole purge rolls back and the store is exactly as it was.
/// Content-file removal runs after the commit and is best-effort: a filesystem
/// failure there leaves orphaned files but neither fails nor undoes the
/// database side.
pub fn purge_all(config: &MemoryConfig) -> Result<usize> {
    let mut content_paths = Vec::new();
    let deleted = with_connection(config, |conn| {
        let tx = conn.unchecked_transaction()?;
        collect_purge_content_paths(&tx, &mut content_paths)?;
        let mut total = 0usize;
        for table in PURGE_TABLES_BEFORE_CHUNKS {
            total += tx.execute(&format!("DELETE FROM {table}"), [])?;
        }
        total += tx.execute("DELETE FROM mem_tree_chunks", [])?;
        total += tx.execute("DELETE FROM mem_tree_ingested_sources", [])?;
        tx.commit()?;
        Ok(total)
    })?;
    remove_chunk_content_files(config, &content_paths);
    Ok(deleted)
}

/// Collect every on-disk path [`purge_all`] is about to orphan: chunk and
/// summary bodies in the content vault, plus the raw archive files chunks
/// point at.
///
/// Gathered inside the transaction and removed only after it commits, the same
/// order `delete_chunks_by_source_filter` uses, so a purge that rolls back
/// never leaves a deleted file behind a surviving row. Unlike that function
/// there is no reachability question to answer first — nothing survives a
/// purge, so every path found here is unreferenced by definition.
///
/// A `raw_refs_json` value that does not parse is skipped rather than failing
/// the purge. Its file is then orphaned on disk, which is exactly what its
/// unreadable pointer row already made it.
fn collect_purge_content_paths(tx: &Transaction<'_>, out: &mut Vec<String>) -> Result<()> {
    for sql in [
        "SELECT content_path FROM mem_tree_chunks
          WHERE content_path IS NOT NULL AND content_path != ''",
        "SELECT content_path FROM mem_tree_summaries
          WHERE content_path IS NOT NULL AND content_path != ''",
    ] {
        let mut stmt = tx.prepare(sql)?;
        let rows = stmt.query_map([], |row| row.get::<_, String>(0))?;
        for row in rows {
            out.push(row.context("Failed to read a content path to purge")?);
        }
    }
    let mut stmt = tx.prepare(
        "SELECT raw_refs_json FROM mem_tree_chunks
          WHERE raw_refs_json IS NOT NULL AND raw_refs_json != ''",
    )?;
    let rows = stmt.query_map([], |row| row.get::<_, String>(0))?;
    for row in rows {
        let json = row.context("Failed to read a raw-ref pointer to purge")?;
        if let Ok(refs) = serde_json::from_str::<Vec<RawRef>>(&json) {
            out.extend(refs.into_iter().map(|raw_ref| raw_ref.path));
        }
    }
    Ok(())
}

/// Best-effort removal of on-disk chunk content files, with strict sandboxing:
/// a `content_path` that escapes the content root (via `..`, an absolute path,
/// or a symlink pointing outside) is refused rather than followed.
pub(super) fn remove_chunk_content_files(config: &MemoryConfig, content_paths: &[String]) {
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

/// Remove staged chunk files that have no committed chunk or summary pointer.
///
/// Used after a document-ingest gate race: the losing writer may have staged
/// bodies before discovering the committed winner. Paths referenced by the
/// winner are preserved.
pub(crate) fn remove_unreferenced_content_files(
    config: &MemoryConfig,
    content_paths: &[String],
) -> Result<()> {
    let mut unique = content_paths
        .iter()
        .filter(|path| !path.is_empty())
        .cloned()
        .collect::<HashSet<_>>();
    if unique.is_empty() {
        return Ok(());
    }
    with_connection(config, |connection| {
        unique.retain(|path| {
            connection
                .query_row(
                    "SELECT NOT EXISTS (
                         SELECT 1 FROM mem_tree_chunks WHERE content_path = ?1
                         UNION ALL
                         SELECT 1 FROM mem_tree_summaries WHERE content_path = ?1
                     )",
                    [path],
                    |row| row.get::<_, bool>(0),
                )
                .unwrap_or(false)
        });
        Ok(())
    })?;
    remove_chunk_content_files(config, &unique.into_iter().collect::<Vec<_>>());
    Ok(())
}
