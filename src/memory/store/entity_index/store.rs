//! SQLite persistence for the entity occurrence index.
//!
//! Owns the `mem_tree_entity_index` table: an inverted index
//! `entity_id → node_id` so retrieval can resolve entity-scoped queries in
//! O(lookup). Ported from OpenHuman's `memory_tree::score::store` (the entity
//! CRUD only — the per-chunk score rationale table stays with the scorer).
//!
//! The `is_user` self-identity check is abstracted behind [`SelfIdentity`]: the
//! storage primitive must not depend on the host's identity registry, so it
//! takes an injectable resolver that defaults to [`NoSelfIdentity`] (always
//! `false`).

use std::path::Path;
use std::sync::Arc;

use parking_lot::Mutex;
use rusqlite::{params, Connection, Transaction};

use super::types::{CanonicalEntity, EntityHit, EntityKind};

/// Resolves whether a canonical entity refers to the local user / self.
///
/// Abstracts the host's identity registry away from storage. Hosts plug a real
/// implementation; tests and registry-less hosts use [`NoSelfIdentity`].
pub trait SelfIdentity: Send + Sync {
    /// Returns `true` when `(kind, surface)` matches a self-identity.
    fn is_self(&self, kind: EntityKind, surface: &str) -> bool;
}

/// A [`SelfIdentity`] that never matches — the default for registry-less hosts.
#[derive(Clone, Copy, Debug, Default)]
pub struct NoSelfIdentity;

impl SelfIdentity for NoSelfIdentity {
    fn is_self(&self, _kind: EntityKind, _surface: &str) -> bool {
        false
    }
}

/// Schema for the entity occurrence index.
///
/// Primary key `(entity_id, node_id)` makes re-indexing the same association a
/// no-op update (idempotent). Indexes back both query directions.
const INIT_SQL: &str = "
    PRAGMA journal_mode = WAL;
    PRAGMA synchronous = NORMAL;

    CREATE TABLE IF NOT EXISTS mem_tree_entity_index (
        entity_id    TEXT    NOT NULL,
        node_id      TEXT    NOT NULL,
        node_kind    TEXT    NOT NULL,
        entity_kind  TEXT    NOT NULL,
        surface      TEXT    NOT NULL,
        score        REAL    NOT NULL,
        timestamp_ms INTEGER NOT NULL,
        tree_id      TEXT,
        is_user      INTEGER NOT NULL DEFAULT 0,
        PRIMARY KEY (entity_id, node_id)
    );
    CREATE INDEX IF NOT EXISTS idx_entity_index_entity ON mem_tree_entity_index(entity_id);
    CREATE INDEX IF NOT EXISTS idx_entity_index_node ON mem_tree_entity_index(node_id);
";

const UPSERT_SQL: &str = "INSERT OR REPLACE INTO mem_tree_entity_index (
    entity_id, node_id, node_kind, entity_kind, surface,
    score, timestamp_ms, tree_id, is_user
 ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9)";

/// SQLite-backed entity occurrence index.
///
/// Thread-safe: the connection is behind a `parking_lot::Mutex`.
pub struct EntityIndex {
    conn: Arc<Mutex<Connection>>,
    identity: Arc<dyn SelfIdentity>,
}

impl EntityIndex {
    /// Open (or create) an index at `db_path` with the default no-op identity.
    pub fn open(db_path: &Path) -> anyhow::Result<Self> {
        Self::open_with_identity(db_path, Arc::new(NoSelfIdentity))
    }

    /// Open (or create) an index at `db_path` with a custom identity resolver.
    pub fn open_with_identity(
        db_path: &Path,
        identity: Arc<dyn SelfIdentity>,
    ) -> anyhow::Result<Self> {
        if let Some(parent) = db_path.parent() {
            std::fs::create_dir_all(parent)?;
        }
        let conn = Connection::open(db_path)?;
        conn.execute_batch(INIT_SQL)?;
        Ok(Self {
            conn: Arc::new(Mutex::new(conn)),
            identity,
        })
    }

    /// Open an in-memory index (useful for tests).
    pub fn open_in_memory() -> anyhow::Result<Self> {
        Self::open_in_memory_with_identity(Arc::new(NoSelfIdentity))
    }

    /// Open an in-memory index with a custom identity resolver.
    pub fn open_in_memory_with_identity(identity: Arc<dyn SelfIdentity>) -> anyhow::Result<Self> {
        let conn = Connection::open_in_memory()?;
        conn.execute_batch(INIT_SQL)?;
        Ok(Self {
            conn: Arc::new(Mutex::new(conn)),
            identity,
        })
    }

    /// Resolve `is_user` for a single-occurrence entity via the configured
    /// [`SelfIdentity`] resolver.
    fn is_user(&self, entity: &CanonicalEntity) -> bool {
        self.identity.is_self(entity.kind, &entity.surface)
    }

    /// Index one `(entity, node)` association.
    ///
    /// Idempotent on the composite primary key `(entity_id, node_id)`.
    pub fn index_entity(
        &self,
        entity: &CanonicalEntity,
        node_id: &str,
        node_kind: &str,
        timestamp_ms: i64,
        tree_id: Option<&str>,
    ) -> anyhow::Result<()> {
        let is_user = self.is_user(entity);
        let conn = self.conn.lock();
        conn.execute(
            UPSERT_SQL,
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
    }

    /// Batch-index all entities extracted from a node, in one transaction.
    pub fn index_entities(
        &self,
        entities: &[CanonicalEntity],
        node_id: &str,
        node_kind: &str,
        timestamp_ms: i64,
        tree_id: Option<&str>,
    ) -> anyhow::Result<usize> {
        if entities.is_empty() {
            return Ok(0);
        }
        let conn = self.conn.lock();
        let tx = conn.unchecked_transaction()?;
        {
            let mut stmt = tx.prepare(UPSERT_SQL)?;
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
                    self.identity.is_self(e.kind, &e.surface) as i32,
                ])?;
            }
        }
        tx.commit()?;
        Ok(entities.len())
    }

    /// Index summary-node entities by canonical id only.
    ///
    /// Summary-level entity metadata is LLM-derived: the summariser emits a
    /// curated list of canonical ids without per-occurrence span/surface data.
    /// The `"<kind>"` prefix (before the first `:`) is written into
    /// `entity_kind` so [`lookup_entity`](Self::lookup_entity) keeps round-tripping
    /// through [`EntityKind::parse`] on mixed leaf/summary rows; the full
    /// canonical id is stored as a stable `surface` placeholder.
    pub fn index_summary_entity_ids(
        &self,
        entity_ids: &[String],
        node_id: &str,
        score: f32,
        timestamp_ms: i64,
        tree_id: Option<&str>,
    ) -> anyhow::Result<usize> {
        if entity_ids.is_empty() {
            return Ok(0);
        }
        let conn = self.conn.lock();
        let tx = conn.unchecked_transaction()?;
        {
            let mut stmt = tx.prepare(UPSERT_SQL)?;
            for canonical_id in entity_ids {
                let entity_kind = match canonical_id.split_once(':') {
                    Some((kind, _)) => kind,
                    None => canonical_id.as_str(),
                };
                let is_user = canonical_id_is_user(self.identity.as_ref(), canonical_id);
                stmt.execute(params![
                    canonical_id,
                    node_id,
                    "summary",
                    entity_kind,
                    canonical_id,
                    score,
                    timestamp_ms,
                    tree_id,
                    is_user as i32,
                ])?;
            }
        }
        tx.commit()?;
        Ok(entity_ids.len())
    }

    /// Remove all index rows for a node. Used before re-indexing a re-scored
    /// node so entities dropped from the new extraction don't leak (an
    /// `INSERT OR REPLACE` never deletes).
    pub fn clear_entity_index_for_node(&self, node_id: &str) -> anyhow::Result<usize> {
        let conn = self.conn.lock();
        let n = conn.execute(
            "DELETE FROM mem_tree_entity_index WHERE node_id = ?1",
            params![node_id],
        )?;
        Ok(n)
    }

    /// Find all nodes indexed against `entity_id`, newest first.
    pub fn lookup_entity(
        &self,
        entity_id: &str,
        limit: Option<usize>,
    ) -> anyhow::Result<Vec<EntityHit>> {
        // Clamp to i64::MAX before casting so callers can't wrap a large usize
        // into a negative LIMIT and bypass it.
        let limit = limit.unwrap_or(100).min(i64::MAX as usize) as i64;
        let conn = self.conn.lock();
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
    }

    /// All distinct canonical entity ids associated with `node_id`, ordered by
    /// score (desc) then recency. Used by topic-routing to pick which topic
    /// trees a node should fan into.
    pub fn list_entity_ids_for_node(&self, node_id: &str) -> anyhow::Result<Vec<String>> {
        let conn = self.conn.lock();
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
    }

    /// Count rows in the entity index (for tests / diagnostics).
    pub fn count_entity_index(&self) -> anyhow::Result<u64> {
        let conn = self.conn.lock();
        let n: i64 = conn.query_row("SELECT COUNT(*) FROM mem_tree_entity_index", [], |r| {
            r.get(0)
        })?;
        Ok(n.max(0) as u64)
    }

    /// Run a closure with the raw transaction, for callers that need to fold
    /// entity indexing into a larger atomic write. The closure receives a
    /// [`Transaction`] and may call [`index_entities_tx`].
    pub fn with_transaction<T>(
        &self,
        f: impl FnOnce(&Transaction<'_>) -> anyhow::Result<T>,
    ) -> anyhow::Result<T> {
        let conn = self.conn.lock();
        let tx = conn.unchecked_transaction()?;
        let out = f(&tx)?;
        tx.commit()?;
        Ok(out)
    }
}

/// Resolve `is_user` for a summary canonical id (`"<kind>:<value>"`), returning
/// `false` for malformed ids or non-matchable kinds.
fn canonical_id_is_user(identity: &dyn SelfIdentity, canonical_id: &str) -> bool {
    let Some((kind_str, value)) = canonical_id.split_once(':') else {
        return false;
    };
    let Ok(kind) = EntityKind::parse(kind_str) else {
        return false;
    };
    identity.is_self(kind, value)
}

/// Transaction-scoped batch index, for folding into a larger atomic write via
/// [`EntityIndex::with_transaction`]. `is_user` defaults to `false` here since
/// the identity resolver is not in scope on a bare transaction.
///
/// NOTE: because the upsert is keyed on `(entity_id, node_id)` and always
/// writes `is_user = 0`, re-indexing a node through this path clobbers a row
/// that a prior [`EntityIndex::index_entity`]/[`EntityIndex::index_entities`]
/// call had correctly marked `is_user = 1` — there is no read-before-write to
/// preserve the existing flag. Callers that need `is_user` preserved across
/// re-indexing should not mix this entry point with the identity-aware ones
/// for the same node.
pub fn index_entities_tx(
    tx: &Transaction<'_>,
    entities: &[CanonicalEntity],
    node_id: &str,
    node_kind: &str,
    timestamp_ms: i64,
    tree_id: Option<&str>,
) -> anyhow::Result<usize> {
    if entities.is_empty() {
        return Ok(0);
    }
    let mut stmt = tx.prepare(UPSERT_SQL)?;
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
            0_i32,
        ])?;
    }
    Ok(entities.len())
}

#[cfg(test)]
#[path = "store_tests.rs"]
mod tests;
