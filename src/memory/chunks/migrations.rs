//! One-shot SQLite migrations for the chunk DB.
//!
//! Each migration is version-gated via `PRAGMA user_version` so it runs exactly
//! once per vault. Called from [`super::connection`] during DB initialisation.
//!
//! ## Contract
//! Both migrations here follow the same shape: read `user_version`, bail out
//! if it is already at/past the migration's target version, otherwise do the
//! work inside one `unchecked_transaction`, commit, then bump
//! `user_version` in a *separate* statement after the commit. That last step
//! is not itself transactional with the migration body — a crash between
//! commit and the `pragma_update` re-runs the (idempotent, `DELETE`/copy-only)
//! migration body on next open, which is safe. The reverse is not safe: see
//! the NOTE on [`migrate_legacy_embeddings_to_sidecar`] (audit finding SC-10).
//!
//! Neither migration is exercised by an automated test in this module (audit
//! test-coverage gap) — both are asserted only via the module's behavior at
//! `connection::init_db` call time.

use anyhow::{Context, Result};
use rusqlite::Connection;

use super::embeddings::{
    active_embedding_dims, set_chunk_embedding_for_signature_tx,
    set_summary_embedding_for_signature_tx, tree_active_signature,
};
use super::{content_root, GLOBAL_TOPIC_PURGE_MIGRATION_VERSION, TREE_EMBEDDING_MIGRATION_VERSION};
use crate::memory::config::MemoryConfig;

/// One-shot migration: copy legacy per-chunk/summary `.embedding` blobs into the
/// normalised `mem_tree_chunk_embeddings` / `mem_tree_summary_embeddings` sidecar
/// tables.
///
/// Version-gated: `PRAGMA user_version < TREE_EMBEDDING_MIGRATION_VERSION`
/// triggers the copy; otherwise it is a no-op. Dim-mismatched rows are skipped
/// (left for a later re-embed backfill); the legacy column is preserved.
///
/// # NOTE — skipped rows are stranded (audit finding SC-10)
/// `PRAGMA user_version` is bumped to [`super::TREE_EMBEDDING_MIGRATION_VERSION`]
/// unconditionally once this function reaches the end, including the case
/// where every row for a table was skipped for dim mismatch. Because this
/// migration is version-gated and one-shot, a dim-mismatched legacy blob
/// skipped here is never retried by this migration on any later open — it
/// permanently keeps only its legacy `.embedding` column and never gets a
/// `mem_tree_chunk_embeddings` / `mem_tree_summary_embeddings` sidecar row
/// from this path. (A later re-embed backfill, if run, can still populate the
/// sidecar independently; this migration itself will not.)
///
/// # Errors
/// Returns `Err` if reading/bumping `user_version` fails, if preparing/
/// executing the per-table `SELECT` fails, or if writing a converted vector
/// into the sidecar table fails. The whole copy runs inside one transaction,
/// so any error here leaves `user_version` un-bumped and the sidecar tables
/// untouched — safe to retry on the next open.
pub(super) fn migrate_legacy_embeddings_to_sidecar(
    conn: &Connection,
    config: &MemoryConfig,
) -> Result<()> {
    let version: i64 = conn
        .query_row("PRAGMA user_version", [], |r| r.get(0))
        .context("read PRAGMA user_version for embedding migration")?;
    if version >= TREE_EMBEDDING_MIGRATION_VERSION {
        return Ok(());
    }

    let sig = tree_active_signature(config);
    let dims = active_embedding_dims(config);

    let tx = conn.unchecked_transaction()?;

    for (table, is_chunk) in [("mem_tree_chunks", true), ("mem_tree_summaries", false)] {
        let mut stmt = tx.prepare(&format!(
            "SELECT id, embedding FROM {table} WHERE embedding IS NOT NULL"
        ))?;
        let rows = stmt.query_map([], |r| {
            Ok((r.get::<_, String>(0)?, r.get::<_, Vec<u8>>(1)?))
        })?;
        for row in rows {
            let (id, blob) = row?;
            if !blob.len().is_multiple_of(4) {
                continue;
            }
            if blob.len() / 4 != dims {
                // Different embedding space — unrecoverable from the blob; leave
                // for a later re-embed backfill.
                continue;
            }
            let vec: Vec<f32> = blob
                .chunks_exact(4)
                .map(|c| f32::from_le_bytes([c[0], c[1], c[2], c[3]]))
                .collect();
            if is_chunk {
                set_chunk_embedding_for_signature_tx(&tx, &id, &sig, &vec)?;
            } else {
                set_summary_embedding_for_signature_tx(&tx, &id, &sig, &vec)?;
            }
        }
    }

    tx.commit()?;
    conn.pragma_update(None, "user_version", TREE_EMBEDDING_MIGRATION_VERSION)
        .context("set PRAGMA user_version after embedding migration")?;
    Ok(())
}

/// One-shot purge of the removed global + topic trees.
///
/// The global (time-axis) and topic (subject-axis) trees were deleted in favour
/// of source trees. This removes their now-orphaned DB rows and on-disk summary
/// folders so old vaults clean themselves up on next open. Version-gated via
/// `PRAGMA user_version`; a no-op on workspaces that never had those trees.
///
/// Deletion order matters: child rows (summary embeddings, reembed-skip
/// markers, entity-index rows keyed by tree/summary) are removed before their
/// parent `mem_tree_summaries` / `mem_tree_trees` rows, so this works whether
/// or not `ON DELETE CASCADE` is declared for a given foreign key.
///
/// The on-disk `wiki/summaries/global*` / `topic-*` folder cleanup after the
/// DB transaction is best-effort: a filesystem error there is swallowed
/// (`let _ =`) rather than propagated, so a failed directory removal does not
/// block the `user_version` bump — those folders may survive as harmless
/// orphans if cleanup fails, but the DB-side purge still completes.
///
/// # Errors
/// Returns `Err` if any `DELETE` statement or the transaction commit fails, or
/// if bumping `user_version` afterward fails. Does not return `Err` for
/// filesystem cleanup failures (see above).
pub(super) fn purge_global_topic_trees(conn: &Connection, config: &MemoryConfig) -> Result<()> {
    let version: i64 = conn
        .query_row("PRAGMA user_version", [], |r| r.get(0))
        .context("read PRAGMA user_version for global/topic purge")?;
    if version >= GLOBAL_TOPIC_PURGE_MIGRATION_VERSION {
        return Ok(());
    }

    let tx = conn.unchecked_transaction()?;
    tx.execute(
        "DELETE FROM mem_tree_summary_embeddings WHERE summary_id IN \
         (SELECT id FROM mem_tree_summaries WHERE tree_kind IN ('global','topic'))",
        [],
    )?;
    tx.execute(
        "DELETE FROM mem_tree_summary_reembed_skipped WHERE summary_id IN \
         (SELECT id FROM mem_tree_summaries WHERE tree_kind IN ('global','topic'))",
        [],
    )?;
    tx.execute(
        "DELETE FROM mem_tree_entity_index WHERE tree_id IN \
         (SELECT id FROM mem_tree_trees WHERE kind IN ('global','topic'))",
        [],
    )?;
    tx.execute(
        "DELETE FROM mem_tree_summaries WHERE tree_kind IN ('global','topic')",
        [],
    )?;
    tx.execute(
        "DELETE FROM mem_tree_buffers WHERE tree_id IN \
         (SELECT id FROM mem_tree_trees WHERE kind IN ('global','topic'))",
        [],
    )?;
    tx.execute(
        "DELETE FROM mem_tree_trees WHERE kind IN ('global','topic')",
        [],
    )?;
    tx.execute(
        "DELETE FROM mem_tree_jobs WHERE kind IN ('topic_route','digest_daily')",
        [],
    )?;
    tx.commit()?;

    // On-disk: drop `wiki/summaries/global*` and `topic-*` summary folders.
    // Best-effort — a filesystem error must not abort the version bump.
    let summaries_root = content_root(config).join("wiki").join("summaries");
    if let Ok(entries) = std::fs::read_dir(&summaries_root) {
        for entry in entries.flatten() {
            let name = entry.file_name();
            let name = name.to_string_lossy();
            if name.starts_with("global") || name.starts_with("topic-") {
                let _ = std::fs::remove_dir_all(entry.path());
            }
        }
    }

    conn.pragma_update(None, "user_version", GLOBAL_TOPIC_PURGE_MIGRATION_VERSION)
        .context("set PRAGMA user_version after global/topic purge")?;
    Ok(())
}
