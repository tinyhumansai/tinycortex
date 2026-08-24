//! Filtered chunk listing and the reads that must agree with it: the count of
//! what it matched, the detail view of the same page, and the per-source
//! rollup over the same scope.

use anyhow::{Context, Result};

use super::connection::with_connection;
use super::store::{row_to_chunk, CHUNK_STATUS_DROPPED, SELECT_COLUMNS};
use super::types::{Chunk, SourceKind};
use crate::memory::config::MemoryConfig;

const DEFAULT_LIST_LIMIT: usize = 100;
const MAX_LIST_LIMIT: usize = 10_000;

/// Optional filters and pagination for [`list_chunks`].
///
/// Every field is a filter and they compose with `AND`; the default matches
/// everything the store holds, bounded by the store's own row cap.
///
/// # An empty list means "unfiltered", not "match nothing"
///
/// The `Vec` fields are absent-by-default, exactly like the `Option` ones: an
/// empty `ids` does not restrict the id, it declines to. That is forced by
/// [`Default`] — a query that left every list empty would otherwise match no
/// rows at all — and it is the reading a caller deserialising a partial query
/// off the wire gets for free.
///
/// The cost is real and worth naming: a caller that *computed* a candidate set
/// and got nothing must not hand the empty set to this struct expecting an
/// empty page. It has to short-circuit and skip the query, because the query
/// will happily return everything.
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
    /// Exact chunk ids to keep.
    pub ids: Vec<String>,
    /// Allowed source kinds. Composes with — does not replace — `source_kind`;
    /// setting both keeps only rows satisfying each, which is empty whenever
    /// the scalar is not one of the listed kinds.
    pub source_kinds: Vec<SourceKind>,
    /// Allowed logical source ids. Composes with `source_id` on the same terms
    /// as `source_kinds` does with `source_kind`.
    pub source_ids: Vec<String>,
    /// Keep only chunks the entity index links to one of these canonical
    /// entity ids.
    pub entity_ids: Vec<String>,
    /// Keep only chunks the entity index links to an entity of one of these
    /// kinds.
    ///
    /// Independent of `entity_ids`: setting both asks for a chunk carrying
    /// *some* listed entity and *some* entity of a listed kind, not for one
    /// index row satisfying both. Each predicate therefore means exactly one
    /// thing whether or not the other is set, which is worth more than the
    /// narrower joint reading — a caller that wants the joint one already
    /// knows the kind of the ids it is asking about.
    pub entity_kinds: Vec<String>,
    /// Substring the stored `content` column must contain, matched literally
    /// — `%` and `_` in the value are text, not wildcards — and
    /// case-insensitively for ASCII, which is what SQLite's `LIKE` does.
    ///
    /// This searches the SQLite column, which for a staged chunk holds only a
    /// ≤500-character preview of a body that lives in the content vault. A
    /// match is therefore proof the text is in the chunk; a miss is not proof
    /// it is absent.
    pub content_contains: Option<String>,
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
        append_filters(&mut sql, &mut bound, query)?;
        append_page(&mut sql, &mut bound, query);
        let params = as_params(&bound);
        conn.prepare(&sql)?
            .query_map(params.as_slice(), row_to_chunk)?
            .collect::<rusqlite::Result<Vec<_>>>()
            .context("Failed to collect chunks")
    })
}

/// One chunk plus the per-chunk facts stored beside it in the same row.
///
/// # No body
///
/// The vault file named by `content_path` is deliberately not read. A list
/// view renders a page of these, and reading each body would turn one query
/// into one query plus a filesystem round trip per row, for text most callers
/// never show — [`Chunk::content`] already carries the stored preview. A
/// caller that needs a full body asks for that one chunk's body.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ChunkDetailRow {
    /// The chunk row, decoded exactly as [`list_chunks`] decodes it.
    pub chunk: Chunk,
    /// Vault path of the full body, when this chunk has one.
    ///
    /// An empty string reads back as `None`, matching
    /// [`get_chunk_content_path`](super::get_chunk_content_path): the column is
    /// NULL for an inline chunk and a path for a staged one, and an empty
    /// string is neither.
    pub content_path: Option<String>,
    /// Lifecycle state — `admitted`, `sealed`, `dropped`, …
    ///
    /// `Option` because the column arrived as an additive `ALTER TABLE`: a row
    /// written by a build that predates it, or by a writer that bypassed it,
    /// can still read back NULL, and one such row must not fail the page.
    pub lifecycle_status: Option<String>,
    /// Whether an embedding vector exists for this chunk in **any** signature.
    ///
    /// Not scoped to a model on purpose — this answers "has this been embedded
    /// at all", which is the question an inspection view asks. Whether a
    /// *particular* space holds it is
    /// [`get_chunk_embedding_for_signature`](super::get_chunk_embedding_for_signature).
    pub has_embedding: bool,
}

/// The same page [`list_chunks`] returns, with the three per-chunk facts an
/// inspection view shows beside each row.
///
/// One `SELECT`, not four reads per row: `content_path`, `lifecycle_status`
/// and the embedding existence test are all answerable from the chunk row and
/// one correlated `EXISTS`, and a caller rendering a page of them out of
/// process would otherwise pay a round trip per fact per row.
///
/// Filters, order and page bounds come from the same builders [`list_chunks`]
/// uses, so switching a caller between the two views changes what it learns
/// about each row and nothing about which rows it gets.
///
/// # Errors
/// Returns an error when SQLite preparation, execution, or row decoding fails.
pub fn list_chunk_details(
    config: &MemoryConfig,
    query: &ListChunksQuery,
) -> Result<Vec<ChunkDetailRow>> {
    with_connection(config, |conn| {
        let mut sql = format!(
            "SELECT {SELECT_COLUMNS},
                    content_path,
                    lifecycle_status,
                    EXISTS (SELECT 1 FROM mem_tree_chunk_embeddings e
                             WHERE e.chunk_id = mem_tree_chunks.id)
               FROM mem_tree_chunks WHERE 1=1"
        );
        let mut bound: Vec<Box<dyn rusqlite::ToSql>> = Vec::new();
        append_filters(&mut sql, &mut bound, query)?;
        append_page(&mut sql, &mut bound, query);
        let params = as_params(&bound);
        conn.prepare(&sql)?
            .query_map(params.as_slice(), |row| {
                Ok(ChunkDetailRow {
                    chunk: row_to_chunk(row)?,
                    content_path: row
                        .get::<_, Option<String>>(DETAIL_CONTENT_PATH_COLUMN)?
                        .filter(|path| !path.is_empty()),
                    lifecycle_status: row.get(DETAIL_LIFECYCLE_STATUS_COLUMN)?,
                    has_embedding: row.get::<_, i64>(DETAIL_HAS_EMBEDDING_COLUMN)? != 0,
                })
            })?
            .collect::<rusqlite::Result<Vec<_>>>()
            .context("Failed to collect chunk details")
    })
}

// The three columns `list_chunk_details` selects past `SELECT_COLUMNS`, whose
// fourteen columns `row_to_chunk` consumes as `0..=13`. Named rather than
// spelled inline so adding a column to `SELECT_COLUMNS` moves them in one
// place instead of silently shifting what each `row.get` reads.
const DETAIL_CONTENT_PATH_COLUMN: usize = 14;
const DETAIL_LIFECYCLE_STATUS_COLUMN: usize = 15;
const DETAIL_HAS_EMBEDDING_COLUMN: usize = 16;

/// How many rows [`list_chunks`] would match for `query`, ignoring its `limit`
/// and `offset`.
///
/// The predicate is built by the same private filter builder the listing uses,
/// so the two cannot drift apart. That matters more than the saving: a total that
/// disagrees with the page beside it sends a caller paging towards rows that
/// are not there, which is worse than having no total at all.
///
/// [`count_chunks`](super::count_chunks) answers a different question — every
/// row in the table, no filters — and is not a substitute for this.
///
/// # Errors
/// Returns an error when SQLite preparation or execution fails.
pub fn count_chunks_matching(config: &MemoryConfig, query: &ListChunksQuery) -> Result<u64> {
    with_connection(config, |conn| {
        let mut sql = String::from("SELECT COUNT(*) FROM mem_tree_chunks WHERE 1=1");
        let mut bound: Vec<Box<dyn rusqlite::ToSql>> = Vec::new();
        append_filters(&mut sql, &mut bound, query)?;
        let params = as_params(&bound);
        let total: i64 = conn
            .query_row(&sql, params.as_slice(), |row| row.get(0))
            .context("Failed to count chunks")?;
        Ok(total.max(0) as u64)
    })
}

/// What the store holds under one `(source_kind, source_id)` pair.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SourceTotal {
    /// The source kind the chunks were ingested under.
    pub source_kind: SourceKind,
    /// The logical source id, verbatim as stored.
    pub source_id: String,
    /// Chunks currently held under this pair, at any lifecycle status.
    pub chunk_count: u64,
    /// The newest source time among them, in epoch milliseconds.
    pub last_timestamp_ms: i64,
}

/// Every source the store holds chunks for, most-recently-active first.
///
/// The rollup a "what is in my memory" view needs, answered by one grouped
/// query instead of a listing the caller counts itself: the counts here are
/// over *all* matching rows, where a listing would only ever see one page of
/// them.
///
/// `scope` is the same memory-source allowlist [`ListChunksQuery::source_scope`]
/// carries and is applied by the same builder, so a scoped caller's totals
/// cover exactly the chunks its scoped listing can reach. `limit` shares the
/// listing's clamp — it bounds returned *groups*, not chunks.
///
/// Ordering breaks ties down to the group key so the result is stable across
/// calls; without that a caller polling this would watch equally-recent
/// sources swap places for no reason.
///
/// # Errors
/// Returns an error when SQLite preparation or execution fails, or when a
/// stored `source_kind` is not one this build knows (a hand-edited DB — the
/// same condition that already fails [`list_chunks`]).
pub fn source_totals(
    config: &MemoryConfig,
    limit: Option<usize>,
    scope: Option<&std::collections::HashSet<String>>,
) -> Result<Vec<SourceTotal>> {
    with_connection(config, |conn| {
        let mut sql = String::from(
            "SELECT source_kind, source_id, COUNT(*) AS chunk_count,
                    MAX(timestamp_ms) AS last_timestamp_ms
               FROM mem_tree_chunks WHERE 1=1",
        );
        let mut bound: Vec<Box<dyn rusqlite::ToSql>> = Vec::new();
        append_source_scope(&mut sql, &mut bound, scope);
        sql.push_str(
            " GROUP BY source_kind, source_id
              ORDER BY last_timestamp_ms DESC, chunk_count DESC,
                       source_kind ASC, source_id ASC
              LIMIT ?",
        );
        bound.push(Box::new(normalized_limit(limit)));
        let params = as_params(&bound);
        conn.prepare(&sql)?
            .query_map(params.as_slice(), |row| {
                let stored_kind: String = row.get(0)?;
                let source_kind = SourceKind::parse(&stored_kind).map_err(|error| {
                    rusqlite::Error::FromSqlConversionFailure(
                        0,
                        rusqlite::types::Type::Text,
                        error.into(),
                    )
                })?;
                let chunk_count: i64 = row.get(2)?;
                Ok(SourceTotal {
                    source_kind,
                    source_id: row.get(1)?,
                    chunk_count: chunk_count.max(0) as u64,
                    last_timestamp_ms: row.get(3)?,
                })
            })?
            .collect::<rusqlite::Result<Vec<_>>>()
            .context("Failed to collect source totals")
    })
}

/// Append every `WHERE` term the query asks for, binding in the same order.
///
/// Shared by [`list_chunks`], [`list_chunk_details`] and
/// [`count_chunks_matching`]: everything that decides *which* rows match lives
/// here, and nothing that decides how many of them are returned does. Ordering
/// and the page bounds stay in [`append_page`] because the count deliberately
/// ignores them — `limit` is a property of the page, not of the result set.
///
/// # Errors
/// Returns an error when a list predicate's values cannot be encoded as the
/// JSON array they are bound as.
fn append_filters(
    sql: &mut String,
    bound: &mut Vec<Box<dyn rusqlite::ToSql>>,
    query: &ListChunksQuery,
) -> Result<()> {
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
    append_in_list(sql, bound, "id", &query.ids)?;
    let source_kinds = query
        .source_kinds
        .iter()
        .map(|kind| kind.as_str().to_string())
        .collect::<Vec<_>>();
    append_in_list(sql, bound, "source_kind", &source_kinds)?;
    append_in_list(sql, bound, "source_id", &query.source_ids)?;
    append_entity_exists(sql, bound, "entity_id", &query.entity_ids)?;
    append_entity_exists(sql, bound, "entity_kind", &query.entity_kinds)?;
    if let Some(value) = query.content_contains.as_deref() {
        sql.push_str(" AND content LIKE ? ESCAPE '\\'");
        bound.push(Box::new(like_contains_pattern(value)));
    }
    if query.exclude_dropped {
        sql.push_str(" AND lifecycle_status != ?");
        bound.push(Box::new(CHUNK_STATUS_DROPPED.to_string()));
    }
    append_source_scope(sql, bound, query.source_scope.as_ref());
    Ok(())
}

/// Append the deterministic order and the page bounds.
///
/// Separate from [`append_filters`] so [`count_chunks_matching`] cannot pick
/// them up, and shared by the two listings so the same query cannot present
/// the same rows in two different orders depending on which view asked.
fn append_page(
    sql: &mut String,
    bound: &mut Vec<Box<dyn rusqlite::ToSql>>,
    query: &ListChunksQuery,
) {
    sql.push_str(" ORDER BY timestamp_ms DESC, seq_in_source ASC, id ASC LIMIT ? OFFSET ?");
    bound.push(Box::new(normalized_limit(query.limit)));
    bound.push(Box::new(
        i64::try_from(query.offset.unwrap_or(0)).unwrap_or(i64::MAX),
    ));
}

/// Append `AND <column> IN (…)` for a non-empty list, binding the whole list
/// as a single JSON array parameter.
///
/// One parameter however long the list, which is what makes this usable from a
/// filter builder at all. [`get_chunks_batch`](super::get_chunks_batch) binds
/// one placeholder per id and windows the ids to stay under SQLite's
/// bound-parameter ceiling; it can window because it issues one statement per
/// window and merges the results into a map. A filtered listing cannot — the
/// order, `LIMIT` and `OFFSET` are properties of the whole result set, so
/// splitting the statement splits the page and the total stops matching it.
/// Binding through `json_each` removes the ceiling instead of managing it, and
/// is already what the source-scope gate below relies on.
///
/// # Errors
/// Returns an error when `values` cannot be encoded as a JSON array.
fn append_in_list(
    sql: &mut String,
    bound: &mut Vec<Box<dyn rusqlite::ToSql>>,
    column: &str,
    values: &[String],
) -> Result<()> {
    let Some(json) = json_list(values)? else {
        return Ok(());
    };
    sql.push_str(&format!(
        " AND {column} IN (SELECT value FROM json_each(?))"
    ));
    bound.push(Box::new(json));
    Ok(())
}

/// Append `AND EXISTS (…)` over the entity index for a non-empty list.
///
/// `EXISTS`, not a join: a chunk carrying three of the listed entities has
/// three index rows, and a join would return it three times. That would be a
/// duplicate page, and — worse — a total from [`count_chunks_matching`] three
/// times the number of chunks that actually match, which is exactly the
/// page/total disagreement the shared builder exists to prevent. `EXISTS`
/// cannot multiply rows, so neither read needs a `SELECT COUNT(*) FROM (…)`
/// wrapper or a `DISTINCT` to undo the damage.
///
/// # Errors
/// Returns an error when `values` cannot be encoded as a JSON array.
fn append_entity_exists(
    sql: &mut String,
    bound: &mut Vec<Box<dyn rusqlite::ToSql>>,
    column: &str,
    values: &[String],
) -> Result<()> {
    let Some(json) = json_list(values)? else {
        return Ok(());
    };
    sql.push_str(&format!(
        " AND EXISTS (SELECT 1 FROM mem_tree_entity_index ei
              WHERE ei.node_id = mem_tree_chunks.id
                AND ei.{column} IN (SELECT value FROM json_each(?)))"
    ));
    bound.push(Box::new(json));
    Ok(())
}

/// Encode a filter list as the JSON array `json_each` reads, or `None` when
/// the list is empty and the predicate is therefore not asked for.
///
/// # Errors
/// Returns an error when the values cannot be encoded.
fn json_list(values: &[String]) -> Result<Option<String>> {
    if values.is_empty() {
        return Ok(None);
    }
    serde_json::to_string(values)
        .map(Some)
        .context("Failed to encode chunk filter list")
}

/// Build the `LIKE` operand for "contains `value`", escaping the wildcards so
/// the caller's text is matched literally.
///
/// `%`, `_` and the escape character itself are escaped with `\`, paired with
/// `ESCAPE '\'` at the call site. Without this a source id or body fragment
/// containing `_` silently matches any character in that position — the same
/// literalness `delete_chunks_by_source_prefix` gets by filtering in Rust
/// rather than with `LIKE`.
fn like_contains_pattern(value: &str) -> String {
    let mut pattern = String::with_capacity(value.len() + 2);
    pattern.push('%');
    for character in value.chars() {
        if matches!(character, '\\' | '%' | '_') {
            pattern.push('\\');
        }
        pattern.push(character);
    }
    pattern.push('%');
    pattern
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

/// Borrow the boxed bindings as the slice `rusqlite` takes.
fn as_params(bound: &[Box<dyn rusqlite::ToSql>]) -> Vec<&dyn rusqlite::ToSql> {
    bound
        .iter()
        .map(|value| value.as_ref() as &dyn rusqlite::ToSql)
        .collect()
}

fn normalized_limit(requested: Option<usize>) -> i64 {
    let limit = requested
        .unwrap_or(DEFAULT_LIST_LIMIT)
        .clamp(1, MAX_LIST_LIMIT);
    i64::try_from(limit).unwrap_or(MAX_LIST_LIMIT as i64)
}
