//! Filtered and paginated chunk listing.

use anyhow::{Context, Result};

use super::connection::with_connection;
use super::store::{row_to_chunk, CHUNK_STATUS_DROPPED, SELECT_COLUMNS};
use super::types::{Chunk, SourceKind};
use crate::memory::config::MemoryConfig;

const DEFAULT_LIST_LIMIT: usize = 100;
const MAX_LIST_LIMIT: usize = 10_000;

/// Optional filters and pagination for [`list_chunks`].
#[derive(Debug, Default, Clone)]
pub struct ListChunksQuery {
    /// Exact source-kind filter.
    pub source_kind: Option<SourceKind>,
    /// Exact logical source-id filter.
    pub source_id: Option<String>,
    /// Exact owner filter.
    pub owner: Option<String>,
    /// Inclusive lower source-time bound in epoch milliseconds.
    pub since_ms: Option<i64>,
    /// Inclusive upper source-time bound in epoch milliseconds.
    pub until_ms: Option<i64>,
    /// Maximum rows, clamped to the store safety cap.
    pub limit: Option<usize>,
    /// Ordered rows to skip for internal pagination.
    pub offset: Option<usize>,
    /// Allowed memory-source identifiers.
    pub source_scope: Option<std::collections::HashSet<String>>,
    /// Exclude lifecycle-dropped chunks.
    pub exclude_dropped: bool,
}

/// List chunks matching all supplied filters in deterministic newest-first
/// order. Source allowlisting is applied in SQL before the row limit.
///
/// # Errors
/// Returns an error when SQLite preparation, execution, or row decoding fails.
pub fn list_chunks(config: &MemoryConfig, query: &ListChunksQuery) -> Result<Vec<Chunk>> {
    with_connection(config, |conn| {
        let mut sql = format!("SELECT {SELECT_COLUMNS} FROM mem_tree_chunks WHERE 1=1");
        let mut bound: Vec<Box<dyn rusqlite::ToSql>> = Vec::new();
        if let Some(kind) = query.source_kind {
            sql.push_str(" AND source_kind = ?");
            bound.push(Box::new(kind.as_str().to_string()));
        }
        for (clause, value) in [
            (" AND source_id = ?", query.source_id.as_ref()),
            (" AND owner = ?", query.owner.as_ref()),
        ] {
            if let Some(value) = value {
                sql.push_str(clause);
                bound.push(Box::new(value.clone()));
            }
        }
        if let Some(value) = query.since_ms {
            sql.push_str(" AND timestamp_ms >= ?");
            bound.push(Box::new(value));
        }
        if let Some(value) = query.until_ms {
            sql.push_str(" AND timestamp_ms <= ?");
            bound.push(Box::new(value));
        }
        if query.exclude_dropped {
            sql.push_str(" AND lifecycle_status != ?");
            bound.push(Box::new(CHUNK_STATUS_DROPPED.to_string()));
        }
        append_source_scope(&mut sql, &mut bound, query.source_scope.as_ref());
        sql.push_str(" ORDER BY timestamp_ms DESC, seq_in_source ASC, id ASC LIMIT ? OFFSET ?");
        bound.push(Box::new(normalized_limit(query.limit)));
        bound.push(Box::new(
            i64::try_from(query.offset.unwrap_or(0)).unwrap_or(i64::MAX),
        ));
        let params = bound
            .iter()
            .map(|value| value.as_ref() as &dyn rusqlite::ToSql)
            .collect::<Vec<_>>();
        conn.prepare(&sql)?
            .query_map(params.as_slice(), row_to_chunk)?
            .collect::<rusqlite::Result<Vec<_>>>()
            .context("Failed to collect chunks")
    })
}

fn append_source_scope(
    sql: &mut String,
    bound: &mut Vec<Box<dyn rusqlite::ToSql>>,
    allowed: Option<&std::collections::HashSet<String>>,
) {
    let Some(allowed) = allowed else { return };
    sql.push_str(
        " AND (NOT EXISTS (SELECT 1 FROM json_each(mem_tree_chunks.tags_json)
          WHERE value = 'memory_sources')",
    );
    if !allowed.is_empty() {
        sql.push_str(" OR (");
        for (index, source_id) in allowed.iter().enumerate() {
            if index > 0 {
                sql.push_str(" OR ");
            }
            sql.push_str("source_id = ? OR substr(source_id, 1, length(?)) = ?");
            bound.push(Box::new(source_id.clone()));
            let prefix = format!("mem_src:{source_id}:");
            bound.push(Box::new(prefix.clone()));
            bound.push(Box::new(prefix));
        }
        sql.push(')');
    }
    sql.push(')');
}

fn normalized_limit(requested: Option<usize>) -> i64 {
    let limit = requested
        .unwrap_or(DEFAULT_LIST_LIMIT)
        .clamp(1, MAX_LIST_LIMIT);
    i64::try_from(limit).unwrap_or(MAX_LIST_LIMIT as i64)
}
