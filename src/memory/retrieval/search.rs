//! `search_entities` — free-text `LIKE` search over the entity index.
//!
//! The entity index (`mem_tree_entity_index`) holds one row per
//! `(entity, node)` occurrence. This primitive exposes it as a fuzzy lookup:
//! "I'm not sure if `alice` is the canonical id — let me search". Rows are
//! grouped by canonical id so repeated mentions collapse into a single
//! [`EntityMatch`] with an aggregate count.
//!
//! Matching rules (ported from OpenHuman's `memory_tree::retrieval::search`):
//! - The query is lowercased before binding into the `LIKE` parameter.
//! - A row matches when `entity_id LIKE '%q%'` OR `surface LIKE '%q%'`.
//! - `kinds` narrows the match by `entity_kind IN (...)` when non-empty.
//! - Output is ordered by mention count DESC (then recency) so the strongest
//!   matches surface first.
//!
//! NOTE: `query`'s SQLite `LIKE` metacharacters (`%`, `_`) are NOT escaped
//! before being embedded in the `%…%` pattern — there is no `ESCAPE` clause.
//! A literal query of `"100%"` or `"_"` is interpreted as a wildcard, not a
//! literal string, and can match far more broadly than the caller intended
//! (up to "everything, capped at `limit`"). Escape `%`/`_`/the escape
//! character itself in `query` before calling if literal matching is
//! required.

use anyhow::{Context, Result};
use rusqlite::params_from_iter;

use crate::memory::chunks::with_connection;
use crate::memory::config::MemoryConfig;
use crate::memory::score::extract::EntityKind;

use super::types::EntityMatch;

/// Default result cap applied when the caller passes `limit = 0`.
const DEFAULT_LIMIT: usize = 5;
/// Hard ceiling on `limit` regardless of what the caller requests.
const MAX_LIMIT: usize = 100;

/// Search the entity index for canonical ids matching `query`.
///
/// Returns at most `limit` matches (default 5, clamped to 100). Each match is
/// aggregated across every row of the entity index so `mention_count` reflects
/// total occurrences regardless of which tree they came from. A blank /
/// whitespace-only query returns no matches (rather than dumping the whole
/// index via `LIKE '%%'`).
///
/// See the module-level NOTE: `query` is matched as a raw `LIKE` pattern with
/// no metacharacter escaping.
///
/// # Errors
///
/// Returns `Err` on a SQLite statement-prepare or row-collection failure
/// (including a corrupt `entity_kind` value in the index, see
/// `row_to_match`).
pub fn search_entities(
    config: &MemoryConfig,
    query: &str,
    kinds: Option<&[EntityKind]>,
    limit: usize,
) -> Result<Vec<EntityMatch>> {
    let limit = normalise_limit(limit);
    let query = query.trim();
    if query.is_empty() {
        return Ok(Vec::new());
    }

    let q_lower = query.to_lowercase();
    with_connection(config, |conn| {
        let pattern = format!("%{q_lower}%");
        let (sql, params) = build_sql_and_params(&pattern, kinds, limit);
        let mut stmt = conn
            .prepare(&sql)
            .context("search_entities: failed to prepare statement")?;
        let mapped = stmt
            .query_map(params_from_iter(params.iter()), row_to_match)?
            .collect::<rusqlite::Result<Vec<_>>>()
            .context("search_entities: failed to collect rows")?;
        Ok(mapped)
    })
}

/// Clamp `limit` into `[1, MAX_LIMIT]`, substituting [`DEFAULT_LIMIT`] for `0`.
fn normalise_limit(limit: usize) -> usize {
    if limit == 0 {
        DEFAULT_LIMIT
    } else {
        limit.min(MAX_LIMIT)
    }
}

/// Build the SQL string + bound parameters. Kept in its own function so the
/// generated statement's shape is unit-testable without a real DB.
fn build_sql_and_params(
    pattern: &str,
    kinds: Option<&[EntityKind]>,
    limit: usize,
) -> (String, Vec<rusqlite::types::Value>) {
    use rusqlite::types::Value;
    let mut sql = String::from(
        "SELECT
            entity_id,
            entity_kind,
            MAX(surface) AS surface_sample,
            COUNT(*) AS mention_count,
            MAX(timestamp_ms) AS last_seen_ms
         FROM mem_tree_entity_index
         WHERE (LOWER(entity_id) LIKE ?1 OR LOWER(surface) LIKE ?1)",
    );
    let mut params: Vec<Value> = vec![Value::Text(pattern.to_string())];

    if let Some(ks) = kinds {
        if !ks.is_empty() {
            let placeholders: Vec<String> = (0..ks.len()).map(|i| format!("?{}", i + 2)).collect();
            sql.push_str(&format!(
                " AND entity_kind IN ({})",
                placeholders.join(", ")
            ));
            for k in ks {
                params.push(Value::Text(k.as_str().to_string()));
            }
        }
    }

    sql.push_str(
        " GROUP BY entity_id, entity_kind
          ORDER BY mention_count DESC, last_seen_ms DESC
          LIMIT ?",
    );
    params.push(Value::Integer(limit as i64));

    (sql, params)
}

/// Map one grouped result row (see [`build_sql_and_params`]'s `SELECT`
/// list, by ordinal) into an [`EntityMatch`].
///
/// # Errors
///
/// Returns `Err` if `entity_kind` fails [`EntityKind::parse`] (an unrecognised
/// or corrupt wire string in `mem_tree_entity_index`) — surfaced as a
/// `rusqlite::Error::FromSqlConversionFailure` so the caller sees a single
/// query failure rather than a partial result set.
fn row_to_match(row: &rusqlite::Row<'_>) -> rusqlite::Result<EntityMatch> {
    let canonical_id: String = row.get(0)?;
    let kind_s: String = row.get(1)?;
    let surface: String = row.get(2)?;
    let mention_count: i64 = row.get(3)?;
    let last_seen_ms: i64 = row.get(4)?;

    let kind = EntityKind::parse(&kind_s).map_err(|e| {
        rusqlite::Error::FromSqlConversionFailure(1, rusqlite::types::Type::Text, e.into())
    })?;

    Ok(EntityMatch {
        canonical_id,
        kind,
        surface,
        mention_count: mention_count.max(0) as u64,
        last_seen_ms,
    })
}

#[cfg(test)]
#[path = "search_tests.rs"]
mod tests;
