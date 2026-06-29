//! Persistence for the scoring layer:
//!
//! - `mem_tree_score` — per-chunk score rationale (which signals fired, why
//!   dropped/kept).
//! - `mem_tree_entity_index` — inverted index `entity_id → node_id` so
//!   retrieval can resolve entity-scoped queries in O(lookup).
//!
//! The schema is declared centrally in `memory::chunks` (`schema.rs::SCHEMA`);
//! this file only owns the CRUD operations against those tables.
//!
//! ## Divergence from OpenHuman
//!
//! The `mem_tree_score` table in TinyCortex does not carry `llm_importance` /
//! `llm_importance_reason` columns, so those fields are admission-time signals
//! only: the persisted row records the resulting `total`. They are kept on
//! [`ScoreRow`] / [`ScoreSignals`] for API compatibility and read back as their
//! defaults (`0.0` / `None`). TinyCortex also has no identity registry, so the
//! `is_user` column is always written as `0`.

use std::collections::HashMap;

use anyhow::{Context, Result};
use rusqlite::{params, Connection, OptionalExtension, Transaction};
use serde::{Deserialize, Serialize};

use crate::memory::chunks::with_connection;
use crate::memory::config::MemoryConfig;
use crate::memory::score::extract::EntityKind;
use crate::memory::score::resolver::CanonicalEntity;
use crate::memory::score::signals::ScoreSignals;

/// Resolve `is_user` for one canonical entity.
///
/// TinyCortex has no Composio-style identity registry, so this is always
/// `false`. The column exists in the schema for forward compatibility with an
/// identity-matching layer; until one is wired in, no row is flagged as the
/// user's own.
fn entity_is_user(_entity: &CanonicalEntity) -> bool {
    false
}

/// Same as [`entity_is_user`] but for the summary-index path where only the
/// canonical id is in scope. Always `false` for the same reason.
fn canonical_id_is_user(_canonical_id: &str) -> bool {
    false
}

/// Serialized per-chunk score rationale. Mirrors the `mem_tree_score` row.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct ScoreRow {
    pub chunk_id: String,
    pub total: f32,
    pub signals: ScoreSignals,
    pub dropped: bool,
    pub reason: Option<String>,
    pub computed_at_ms: i64,
    /// One-line LLM-supplied explanation for the importance rating. Diagnostic
    /// only and **not persisted** (the schema has no column for it) — read back
    /// as `None`.
    #[serde(default)]
    pub llm_importance_reason: Option<String>,
}

/// Upsert one score rationale row, replacing any existing entry for `chunk_id`.
pub fn upsert_score(config: &MemoryConfig, row: &ScoreRow) -> Result<()> {
    with_connection(config, |conn| {
        upsert_score_on_connection(conn, row)?;
        Ok(())
    })
}

pub fn upsert_score_tx(tx: &Transaction<'_>, row: &ScoreRow) -> Result<()> {
    tx.execute(
        SCORE_UPSERT_SQL,
        params![
            row.chunk_id,
            row.total,
            row.signals.token_count,
            row.signals.unique_words,
            row.signals.metadata_weight,
            row.signals.source_weight,
            row.signals.interaction,
            row.signals.entity_density,
            i32::from(row.dropped),
            row.reason,
            row.computed_at_ms,
        ],
    )?;
    Ok(())
}

const SCORE_UPSERT_SQL: &str = "INSERT OR REPLACE INTO mem_tree_score (
    chunk_id, total,
    token_count_signal, unique_words_signal,
    metadata_weight, source_weight, interaction_weight, entity_density,
    dropped, reason, computed_at_ms
 ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11)";

fn upsert_score_on_connection(conn: &Connection, row: &ScoreRow) -> Result<()> {
    conn.execute(
        SCORE_UPSERT_SQL,
        params![
            row.chunk_id,
            row.total,
            row.signals.token_count,
            row.signals.unique_words,
            row.signals.metadata_weight,
            row.signals.source_weight,
            row.signals.interaction,
            row.signals.entity_density,
            i32::from(row.dropped),
            row.reason,
            row.computed_at_ms,
        ],
    )?;
    Ok(())
}

/// Fetch one chunk's score rationale.
pub fn get_score(config: &MemoryConfig, chunk_id: &str) -> Result<Option<ScoreRow>> {
    with_connection(config, |conn| {
        conn.query_row(
            "SELECT chunk_id, total,
                    token_count_signal, unique_words_signal,
                    metadata_weight, source_weight, interaction_weight, entity_density,
                    dropped, reason, computed_at_ms
             FROM mem_tree_score WHERE chunk_id = ?1",
            params![chunk_id],
            |row| {
                Ok(ScoreRow {
                    chunk_id: row.get(0)?,
                    total: row.get(1)?,
                    signals: ScoreSignals {
                        token_count: row.get(2)?,
                        unique_words: row.get(3)?,
                        metadata_weight: row.get(4)?,
                        source_weight: row.get(5)?,
                        interaction: row.get(6)?,
                        entity_density: row.get(7)?,
                        // Not a persisted column — admission-time only.
                        llm_importance: 0.0,
                    },
                    dropped: row.get::<_, i32>(8)? != 0,
                    reason: row.get(9)?,
                    computed_at_ms: row.get(10)?,
                    // Not a persisted column — diagnostic only.
                    llm_importance_reason: None,
                })
            },
        )
        .optional()
        .map_err(anyhow::Error::from)
    })
}

/// Defensive cap for batched `IN (?,?,…)` reads. SQLite's
/// `SQLITE_MAX_VARIABLE_NUMBER` has been 32 766 since 3.32, so 500 leaves a
/// large safety margin against hosts with a lower compile-time cap.
const MAX_FETCH_BATCH: usize = 500;

/// Batched read of just the `total` field for many chunk ids.
///
/// Narrow on purpose. The returned map contains only `chunk_id`s that have a
/// score row; missing ids are silently absent, matching the per-row
/// [`get_score`] contract (callers then fall back to the documented `0.0`).
pub fn get_scores_batch(
    config: &MemoryConfig,
    chunk_ids: &[String],
) -> Result<HashMap<String, f32>> {
    if chunk_ids.is_empty() {
        return Ok(HashMap::new());
    }
    with_connection(config, |conn| {
        let mut out: HashMap<String, f32> = HashMap::with_capacity(chunk_ids.len());
        for window in chunk_ids.chunks(MAX_FETCH_BATCH) {
            let placeholders = (1..=window.len())
                .map(|i| format!("?{i}"))
                .collect::<Vec<_>>()
                .join(",");
            let sql = format!(
                "SELECT chunk_id, total FROM mem_tree_score
                  WHERE chunk_id IN ({placeholders})"
            );
            let mut stmt = conn.prepare(&sql).context("prepare get_scores_batch")?;
            let params: Vec<&dyn rusqlite::ToSql> =
                window.iter().map(|id| id as &dyn rusqlite::ToSql).collect();
            let rows = stmt
                .query_map(params.as_slice(), |row| {
                    Ok((row.get::<_, String>(0)?, row.get::<_, f32>(1)?))
                })
                .context("query get_scores_batch")?;
            for row in rows {
                let (chunk_id, total) = row.context("decode get_scores_batch row")?;
                out.insert(chunk_id, total);
            }
        }
        Ok(out)
    })
}

const ENTITY_INDEX_UPSERT_SQL: &str = "INSERT OR REPLACE INTO mem_tree_entity_index (
    entity_id, node_id, node_kind, entity_kind, surface,
    score, timestamp_ms, tree_id, is_user
 ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9)";

/// Index one (entity, chunk) association.
///
/// Idempotent on the composite primary key `(entity_id, node_id)` so
/// re-indexing the same association is a no-op update.
pub fn index_entity(
    config: &MemoryConfig,
    entity: &CanonicalEntity,
    node_id: &str,
    node_kind: &str,
    timestamp_ms: i64,
    tree_id: Option<&str>,
) -> Result<()> {
    let is_user = entity_is_user(entity);
    with_connection(config, |conn| {
        conn.execute(
            ENTITY_INDEX_UPSERT_SQL,
            params![
                entity.canonical_id,
                node_id,
                node_kind,
                entity.kind.as_str(),
                entity.surface,
                entity.score,
                timestamp_ms,
                tree_id,
                is_user as i32,
            ],
        )?;
        Ok(())
    })
}

/// Batch index all entities extracted from a chunk.
pub fn index_entities(
    config: &MemoryConfig,
    entities: &[CanonicalEntity],
    node_id: &str,
    node_kind: &str,
    timestamp_ms: i64,
    tree_id: Option<&str>,
) -> Result<usize> {
    if entities.is_empty() {
        return Ok(0);
    }
    with_connection(config, |conn| {
        let tx = conn.unchecked_transaction()?;
        {
            let mut stmt = tx.prepare(ENTITY_INDEX_UPSERT_SQL)?;
            for e in entities {
                stmt.execute(params![
                    e.canonical_id,
                    node_id,
                    node_kind,
                    e.kind.as_str(),
                    e.surface,
                    e.score,
                    timestamp_ms,
                    tree_id,
                    entity_is_user(e) as i32,
                ])?;
            }
        }
        tx.commit()?;
        Ok(entities.len())
    })
}

/// Remove all entity-index rows for a given node. Used before re-indexing a
/// re-scored chunk so entities dropped from the new extraction don't leak
/// through (`INSERT OR REPLACE` never deletes).
pub fn clear_entity_index_for_node(config: &MemoryConfig, node_id: &str) -> Result<usize> {
    with_connection(config, |conn| {
        let n = conn.execute(
            "DELETE FROM mem_tree_entity_index WHERE node_id = ?1",
            params![node_id],
        )?;
        Ok(n)
    })
}

pub fn clear_entity_index_for_node_tx(tx: &Transaction<'_>, node_id: &str) -> Result<usize> {
    let n = tx.execute(
        "DELETE FROM mem_tree_entity_index WHERE node_id = ?1",
        params![node_id],
    )?;
    Ok(n)
}

/// Index summary-node entities by canonical id only. Summary-level entity
/// metadata is LLM-derived — the summariser emits a curated list of canonical
/// ids without per-occurrence span/surface data.
///
/// Writes the kind prefix (everything before the first `:`) into the
/// `entity_kind` column so [`lookup_entity`]'s `EntityKind::parse()` keeps
/// round-tripping on summary rows. `surface` stores the full canonical id as a
/// stable placeholder. The summary's score is reused for each of its entities.
pub fn index_summary_entity_ids_tx(
    tx: &Transaction<'_>,
    entity_ids: &[String],
    node_id: &str,
    score: f32,
    timestamp_ms: i64,
    tree_id: Option<&str>,
) -> Result<usize> {
    if entity_ids.is_empty() {
        return Ok(0);
    }
    let mut stmt = tx.prepare(ENTITY_INDEX_UPSERT_SQL)?;
    for canonical_id in entity_ids {
        // Canonical ids follow the "<kind>:<value>" convention. Without this
        // split, `entity_kind` would hold the full id and `lookup_entity`'s
        // `EntityKind::parse()` would fail at read time, poisoning any mixed
        // leaf/summary lookup.
        let entity_kind = match canonical_id.split_once(':') {
            Some((kind, _)) => kind,
            None => canonical_id.as_str(),
        };
        stmt.execute(params![
            canonical_id,
            node_id,
            "summary",
            entity_kind,
            canonical_id,
            score,
            timestamp_ms,
            tree_id,
            canonical_id_is_user(canonical_id) as i32,
        ])?;
    }
    Ok(entity_ids.len())
}

pub fn index_entities_tx(
    tx: &Transaction<'_>,
    entities: &[CanonicalEntity],
    node_id: &str,
    node_kind: &str,
    timestamp_ms: i64,
    tree_id: Option<&str>,
) -> Result<usize> {
    if entities.is_empty() {
        return Ok(0);
    }
    let mut stmt = tx.prepare(ENTITY_INDEX_UPSERT_SQL)?;
    for e in entities {
        stmt.execute(params![
            e.canonical_id,
            node_id,
            node_kind,
            e.kind.as_str(),
            e.surface,
            e.score,
            timestamp_ms,
            tree_id,
            entity_is_user(e) as i32,
        ])?;
    }
    Ok(entities.len())
}

/// Result row from [`lookup_entity`].
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct EntityHit {
    pub entity_id: String,
    pub node_id: String,
    pub node_kind: String,
    pub entity_kind: EntityKind,
    pub surface: String,
    pub score: f32,
    pub timestamp_ms: i64,
    pub tree_id: Option<String>,
    /// True when the canonical id matched an identity registry at index time.
    /// Always `false` in TinyCortex (no identity registry yet).
    #[serde(default)]
    pub is_user: bool,
}

/// Find all nodes indexed against `entity_id`, newest first.
pub fn lookup_entity(
    config: &MemoryConfig,
    entity_id: &str,
    limit: Option<usize>,
) -> Result<Vec<EntityHit>> {
    // Clamp to i64::MAX before casting so callers can't wrap a large usize into
    // a negative LIMIT and bypass it.
    let limit = limit.unwrap_or(100).min(i64::MAX as usize) as i64;
    with_connection(config, |conn| {
        let mut stmt = conn.prepare(
            "SELECT entity_id, node_id, node_kind, entity_kind, surface,
                    score, timestamp_ms, tree_id, is_user
             FROM mem_tree_entity_index
             WHERE entity_id = ?1
             ORDER BY timestamp_ms DESC
             LIMIT ?2",
        )?;
        let rows = stmt
            .query_map(params![entity_id, limit], |row| {
                let kind_s: String = row.get(3)?;
                let entity_kind = EntityKind::parse(&kind_s).map_err(|e| {
                    rusqlite::Error::FromSqlConversionFailure(
                        3,
                        rusqlite::types::Type::Text,
                        e.into(),
                    )
                })?;
                let is_user_int: i32 = row.get(8)?;
                Ok(EntityHit {
                    entity_id: row.get(0)?,
                    node_id: row.get(1)?,
                    node_kind: row.get(2)?,
                    entity_kind,
                    surface: row.get(4)?,
                    score: row.get(5)?,
                    timestamp_ms: row.get(6)?,
                    tree_id: row.get(7)?,
                    is_user: is_user_int != 0,
                })
            })?
            .collect::<rusqlite::Result<Vec<_>>>()?;
        Ok(rows)
    })
}

/// All distinct canonical entity ids associated with `node_id`, ordered by
/// score (desc) then recency. Used by topic-routing to pick which topic trees a
/// node should fan into.
pub fn list_entity_ids_for_node(config: &MemoryConfig, node_id: &str) -> Result<Vec<String>> {
    with_connection(config, |conn| {
        let mut stmt = conn.prepare(
            "SELECT DISTINCT entity_id
               FROM mem_tree_entity_index
              WHERE node_id = ?1
              ORDER BY score DESC, timestamp_ms DESC, entity_id ASC",
        )?;
        let rows = stmt
            .query_map(params![node_id], |row| row.get::<_, String>(0))?
            .collect::<rusqlite::Result<Vec<_>>>()?;
        Ok(rows)
    })
}

/// Count rows in the entity index (for tests / diagnostics).
pub fn count_entity_index(config: &MemoryConfig) -> Result<u64> {
    with_connection(config, |conn| {
        let n: i64 = conn.query_row("SELECT COUNT(*) FROM mem_tree_entity_index", [], |r| {
            r.get(0)
        })?;
        Ok(n.max(0) as u64)
    })
}

/// Count score rows (for tests / diagnostics).
pub fn count_scores(config: &MemoryConfig) -> Result<u64> {
    with_connection(config, |conn| {
        let n: i64 = conn.query_row("SELECT COUNT(*) FROM mem_tree_score", [], |r| r.get(0))?;
        Ok(n.max(0) as u64)
    })
}

#[cfg(test)]
#[path = "store_tests.rs"]
mod tests;
