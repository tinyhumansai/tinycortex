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

pub fn get_buffer_conn(conn: &Connection, tree_id: &str, level: u32) -> Result<Buffer> {
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
pub fn upsert_buffer_tx(tx: &Transaction<'_>, buf: &Buffer) -> Result<()> {
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

/// Reset a buffer at `(tree_id, level)` to empty.
///
/// Seal transactions should use the snapshot-aware internal consumer instead.
pub fn clear_buffer_tx(tx: &Transaction<'_>, tree_id: &str, level: u32) -> Result<()> {
    upsert_buffer_tx(tx, &Buffer::empty(tree_id, level))
}

/// Consume exactly a previously-read buffer snapshot during a seal.
///
/// The current row must still begin with the snapshot ids. Items appended
/// while the summariser was running remain in their original order. If a
/// competing seal already changed the prefix, the transaction fails instead
/// of inserting a duplicate summary or deleting unrelated items.
pub(crate) fn consume_snapshot_tx(tx: &Transaction<'_>, snapshot: &Buffer) -> Result<()> {
    let mut current = get_buffer_conn(tx, &snapshot.tree_id, snapshot.level)?;
    if !current.item_ids.starts_with(&snapshot.item_ids) {
        anyhow::bail!(
            "buffer changed while sealing tree_id={} level={}",
            snapshot.tree_id,
            snapshot.level
        );
    }

    current.item_ids.drain(..snapshot.item_ids.len());
    current.token_sum = current.token_sum.saturating_sub(snapshot.token_sum).max(0);
    if current.item_ids.is_empty() {
        current.oldest_at = None;
        current.token_sum = 0;
    }
    upsert_buffer_tx(tx, &current)
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
