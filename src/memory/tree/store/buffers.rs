//! Persistence for unsealed buffers (`mem_tree_buffers`).

use anyhow::{Context, Result};
use chrono::{DateTime, Utc};
use rusqlite::{params, Connection, OptionalExtension, Transaction};

use super::common::ms_to_utc;
use super::types::Buffer;
use crate::memory::chunks::with_connection;
use crate::memory::config::MemoryConfig;

/// Read the current buffer at `(tree_id, level)` or return an empty one.
pub fn get_buffer(config: &MemoryConfig, tree_id: &str, level: u32) -> Result<Buffer> {
    with_connection(config, |conn| get_buffer_conn(conn, tree_id, level))
}

pub(crate) fn get_buffer_conn(conn: &Connection, tree_id: &str, level: u32) -> Result<Buffer> {
    let mut stmt = conn.prepare(
        "SELECT tree_id, level, item_ids_json, token_sum, oldest_at_ms
           FROM mem_tree_buffers WHERE tree_id = ?1 AND level = ?2",
    )?;
    let row = stmt
        .query_row(params![tree_id, level], row_to_buffer)
        .optional()
        .context("Failed to query buffer")?;
    Ok(row.unwrap_or_else(|| Buffer::empty(tree_id, level)))
}

/// Upsert a buffer row.
pub(crate) fn upsert_buffer_tx(tx: &Transaction<'_>, buf: &Buffer) -> Result<()> {
    let now_ms = Utc::now().timestamp_millis();
    tx.execute(
        "INSERT INTO mem_tree_buffers (
            tree_id, level, item_ids_json, token_sum, oldest_at_ms, updated_at_ms
        ) VALUES (?1, ?2, ?3, ?4, ?5, ?6)
        ON CONFLICT(tree_id, level) DO UPDATE SET
            item_ids_json = excluded.item_ids_json,
            token_sum = excluded.token_sum,
            oldest_at_ms = excluded.oldest_at_ms,
            updated_at_ms = excluded.updated_at_ms",
        params![
            buf.tree_id,
            buf.level,
            serde_json::to_string(&buf.item_ids)?,
            buf.token_sum,
            buf.oldest_at.map(|t| t.timestamp_millis()),
            now_ms,
        ],
    )
    .with_context(|| {
        format!(
            "Failed to upsert buffer tree_id={} level={}",
            buf.tree_id, buf.level
        )
    })?;
    Ok(())
}

/// Reset a buffer at `(tree_id, level)` to empty (used at seal time).
///
/// # NOTE
/// This unconditionally wipes the whole buffer row rather than removing only
/// the ids that were actually sealed. The caller in
/// [`crate::memory::tree::bucket_seal::seal_one_level`] snapshots the buffer,
/// awaits an arbitrarily long summariser call with no lock held, and only then
/// runs this inside the seal transaction — if another `append_to_buffer` call
/// lands on the same `(tree_id, level)` in that window, its item is clobbered
/// here and is never summarised. Callers that need to seal only a known
/// snapshot should re-read the buffer here and remove the snapshotted ids by
/// set-difference instead of clearing unconditionally (tracked as `TR-1` in
/// `docs/spec/audit/03-tree-archivist-conversations.md`).
pub(crate) fn clear_buffer_tx(tx: &Transaction<'_>, tree_id: &str, level: u32) -> Result<()> {
    upsert_buffer_tx(tx, &Buffer::empty(tree_id, level))
}

/// List stale **L0** buffers ordered by `oldest_at_ms` ASC. Only L0 (raw-leaf)
/// buffers are returned — force-sealing an under-fanout upper buffer would
/// produce a degenerate single-child summary and collapse the tree into a chain.
pub fn list_stale_buffers(config: &MemoryConfig, older_than: DateTime<Utc>) -> Result<Vec<Buffer>> {
    with_connection(config, |conn| {
        let mut stmt = conn.prepare(
            "SELECT tree_id, level, item_ids_json, token_sum, oldest_at_ms
               FROM mem_tree_buffers
              WHERE oldest_at_ms IS NOT NULL
                AND oldest_at_ms <= ?1
                AND level = 0
              ORDER BY oldest_at_ms ASC",
        )?;
        let rows = stmt
            .query_map(params![older_than.timestamp_millis()], row_to_buffer)?
            .collect::<rusqlite::Result<Vec<_>>>()
            .context("Failed to collect stale buffers")?;
        Ok(rows)
    })
}

fn row_to_buffer(row: &rusqlite::Row<'_>) -> rusqlite::Result<Buffer> {
    let tree_id: String = row.get(0)?;
    let level: i64 = row.get(1)?;
    let item_ids_json: String = row.get(2)?;
    let token_sum: i64 = row.get(3)?;
    let oldest_ms: Option<i64> = row.get(4)?;

    let item_ids: Vec<String> = serde_json::from_str(&item_ids_json).map_err(|e| {
        rusqlite::Error::FromSqlConversionFailure(2, rusqlite::types::Type::Text, Box::new(e))
    })?;
    let oldest_at = oldest_ms.map(ms_to_utc).transpose()?;
    Ok(Buffer {
        tree_id,
        level: level.max(0) as u32,
        item_ids,
        token_sum,
        oldest_at,
    })
}
