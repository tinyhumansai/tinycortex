//! Local vector store backed by SQLite.
//!
//! Provides a self-contained vector database for storing, searching, and
//! managing text embeddings. Uses SQLite for persistence and brute-force
//! cosine similarity for retrieval (fast enough for on-device workloads up
//! to ~100K vectors).
//!
//! The store only persists and searches packed `f32` vectors; all model calls
//! go through the injected [`EmbeddingBackend`]. Ported faithfully from
//! OpenHuman's `memory_store::vectors::store`.
//!
//! # Usage
//!
//! ```ignore
//! let backend = Arc::new(InertEmbedding::new(768));
//! let store = VectorStore::open(db_path, backend)?;
//!
//! store.insert("doc-1", "notes", "The quick brown fox", json!({})).await?;
//! let results = store.search("notes", "fast animal", 5).await?;
//! ```

use std::path::Path;
use std::sync::Arc;

use anyhow::Context;
use parking_lot::Mutex;
use rusqlite::Connection;

use super::embedding::EmbeddingBackend;

/// SQL to create the vector store schema.
const INIT_SQL: &str = "
    PRAGMA journal_mode = WAL;
    PRAGMA synchronous = NORMAL;

    CREATE TABLE IF NOT EXISTS vectors (
        id         TEXT    NOT NULL,
        namespace  TEXT    NOT NULL,
        text       TEXT    NOT NULL,
        embedding  BLOB    NOT NULL,
        metadata   TEXT    NOT NULL DEFAULT '{}',
        created_at REAL    NOT NULL,
        updated_at REAL    NOT NULL,
        PRIMARY KEY (namespace, id)
    );
    CREATE INDEX IF NOT EXISTS idx_vectors_ns ON vectors(namespace);

    CREATE TABLE IF NOT EXISTS store_meta (
        key        TEXT    PRIMARY KEY,
        value      TEXT    NOT NULL,
        updated_at REAL    NOT NULL
    );
";

/// A single search result from the vector store.
#[derive(Debug, Clone)]
pub struct SearchResult {
    /// The stored document ID.
    pub id: String,
    /// The namespace.
    pub namespace: String,
    /// The original text.
    pub text: String,
    /// Cosine similarity score (0.0 – 1.0).
    pub score: f64,
    /// Arbitrary JSON metadata attached at insert time.
    pub metadata: serde_json::Value,
}

/// SQLite-backed local vector store.
///
/// Thread-safe: the inner connection is behind a `parking_lot::Mutex` and the
/// struct is `Send + Sync`. Embedding calls are async and run through the
/// configured [`EmbeddingBackend`].
pub struct VectorStore {
    pub(crate) conn: Arc<Mutex<Connection>>,
    backend: Arc<dyn EmbeddingBackend>,
}

impl VectorStore {
    /// Opens (or creates) a vector store at the given SQLite database path.
    ///
    /// On first open the backend name and dimensions are persisted to a
    /// `store_meta` table. On subsequent opens the stored dimensions are
    /// compared against the runtime backend and an error is returned if they
    /// mismatch (prevents silent cosine-similarity corruption from
    /// mixed-dimension vectors) — see the private `check_or_store_meta` helper
    /// below for known gaps in that guard (an unreadable `store_meta` row is
    /// indistinguishable from first-open, and a corrupt stored-dims string
    /// silently disables the check rather than failing closed).
    pub fn open(db_path: &Path, backend: Arc<dyn EmbeddingBackend>) -> anyhow::Result<Self> {
        if let Some(parent) = db_path.parent() {
            std::fs::create_dir_all(parent)?;
        }

        let conn = Connection::open(db_path)?;
        conn.execute_batch(INIT_SQL)?;

        Self::check_or_store_meta(&conn, &*backend)?;

        Ok(Self {
            conn: Arc::new(Mutex::new(conn)),
            backend,
        })
    }

    /// Opens an in-memory vector store (useful for tests).
    pub fn open_in_memory(backend: Arc<dyn EmbeddingBackend>) -> anyhow::Result<Self> {
        let conn = Connection::open_in_memory()?;
        conn.execute_batch(INIT_SQL)?;
        Self::check_or_store_meta(&conn, &*backend)?;
        Ok(Self {
            conn: Arc::new(Mutex::new(conn)),
            backend,
        })
    }

    /// Returns a reference to the embedding backend.
    pub fn backend(&self) -> &dyn EmbeddingBackend {
        self.backend.as_ref()
    }

    /// Persist or validate the embedding configuration in `store_meta`.
    ///
    /// NOTE: two related gaps, both currently present:
    /// - A failed read of the `embed_dims` row (e.g. a transient I/O error)
    ///   collapses to `None` via `.ok()` and is treated identically to a
    ///   genuinely first-ever open, so it silently *overwrites*
    ///   `embed_provider`/`embed_dims` with the current runtime backend
    ///   instead of surfacing the read failure.
    /// - The stored dims string is parsed with `.parse().unwrap_or(0)`; a
    ///   corrupted value collapses to `0`, and the mismatch check is skipped
    ///   whenever either side is `0` — so a corrupt row disables the guard
    ///   entirely instead of failing closed.
    fn check_or_store_meta(
        conn: &Connection,
        backend: &dyn EmbeddingBackend,
    ) -> anyhow::Result<()> {
        let now = now_ts();
        let stored_dims: Option<String> = conn
            .query_row(
                "SELECT value FROM store_meta WHERE key = 'embed_dims'",
                [],
                |row| row.get(0),
            )
            .ok();

        match stored_dims {
            None => {
                // First open — persist metadata.
                let stmts: &[(&str, &str)] = &[
                    ("embed_provider", backend.name()),
                    ("embed_dims", &backend.dimensions().to_string()),
                ];
                for (key, value) in stmts {
                    conn.execute(
                        "INSERT OR REPLACE INTO store_meta (key, value, updated_at) VALUES (?1, ?2, ?3)",
                        rusqlite::params![key, value, now],
                    )?;
                }
            }
            Some(dims_str) => {
                let stored: usize = dims_str.parse().unwrap_or(0);
                let runtime = backend.dimensions();
                if stored != 0 && runtime != 0 && stored != runtime {
                    anyhow::bail!(
                        "vector store dimension mismatch: database was created with \
                         {stored}-dim embeddings but the current backend ({}) uses \
                         {runtime} dims. Delete the database or reconfigure the backend.",
                        backend.name()
                    );
                }
            }
        }

        Ok(())
    }

    // ── Write operations ─────────────────────────────────────

    /// Inserts or updates a text entry. The text is embedded automatically.
    ///
    /// If an entry with the same `(namespace, id)` already exists it is replaced.
    pub async fn insert(
        &self,
        id: &str,
        namespace: &str,
        text: &str,
        metadata: serde_json::Value,
    ) -> anyhow::Result<()> {
        let embedding = self.backend.embed_one(text).await?;
        self.insert_with_vector(id, namespace, text, &embedding, metadata)
    }

    /// Inserts with a pre-computed embedding vector (skips the embed call).
    ///
    /// NOTE: the vector's length is never checked against the store's
    /// configured dimensions (`store_meta.embed_dims`). A wrong-dimension
    /// vector is stored as-is; [`cosine_similarity`] then returns `0.0` for
    /// any query against it (length mismatch), so the row silently never
    /// surfaces in search results rather than erroring at insert time.
    pub fn insert_with_vector(
        &self,
        id: &str,
        namespace: &str,
        text: &str,
        embedding: &[f32],
        metadata: serde_json::Value,
    ) -> anyhow::Result<()> {
        let blob = vec_to_bytes(embedding);
        let meta_str = serde_json::to_string(&metadata)?;
        let now = now_ts();

        let conn = self.conn.lock();
        conn.execute(
            "INSERT OR REPLACE INTO vectors (id, namespace, text, embedding, metadata, created_at, updated_at)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)",
            rusqlite::params![id, namespace, text, blob, meta_str, now, now],
        )?;

        Ok(())
    }

    /// Bulk-insert multiple entries. Each text is embedded automatically.
    pub async fn insert_batch(
        &self,
        namespace: &str,
        entries: &[(&str, &str, serde_json::Value)], // (id, text, metadata)
    ) -> anyhow::Result<()> {
        if entries.is_empty() {
            return Ok(());
        }

        let texts: Vec<&str> = entries.iter().map(|(_, text, _)| *text).collect();
        let embeddings = self.backend.embed(&texts).await?;

        if embeddings.len() != entries.len() {
            anyhow::bail!(
                "embedding count mismatch: got {} embeddings for {} entries",
                embeddings.len(),
                entries.len()
            );
        }

        let now = now_ts();
        let conn = self.conn.lock();
        let tx = conn.unchecked_transaction()?;

        for ((id, text, metadata), embedding) in entries.iter().zip(embeddings.iter()) {
            let blob = vec_to_bytes(embedding);
            let meta_str = serde_json::to_string(metadata)?;
            tx.execute(
                "INSERT OR REPLACE INTO vectors (id, namespace, text, embedding, metadata, created_at, updated_at)
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)",
                rusqlite::params![id, namespace, text, blob, meta_str, now, now],
            )?;
        }

        tx.commit()?;

        Ok(())
    }

    // ── Search ───────────────────────────────────────────────

    /// Searches for the `limit` most similar entries to `query` within a namespace.
    ///
    /// The query is embedded via the configured backend and compared against
    /// all stored vectors using cosine similarity.
    pub async fn search(
        &self,
        namespace: &str,
        query: &str,
        limit: usize,
    ) -> anyhow::Result<Vec<SearchResult>> {
        let query_vec = self.backend.embed_one(query).await?;
        self.search_by_vector(namespace, &query_vec, limit)
    }

    /// Searches using a pre-computed query vector.
    pub fn search_by_vector(
        &self,
        namespace: &str,
        query_vec: &[f32],
        limit: usize,
    ) -> anyhow::Result<Vec<SearchResult>> {
        if limit == 0 {
            return Ok(Vec::new());
        }

        let conn = self.conn.lock();
        let mut stmt = conn.prepare(
            "SELECT id, namespace, text, embedding, metadata FROM vectors WHERE namespace = ?1",
        )?;

        let rows: Vec<(String, String, String, Vec<u8>, String)> = stmt
            .query_map(rusqlite::params![namespace], |row| {
                Ok((
                    row.get::<_, String>(0)?,
                    row.get::<_, String>(1)?,
                    row.get::<_, String>(2)?,
                    row.get::<_, Vec<u8>>(3)?,
                    row.get::<_, String>(4)?,
                ))
            })?
            .collect::<rusqlite::Result<Vec<_>>>()?;

        // Score-only intermediate: keep metadata as the raw JSON string instead
        // of parsing it here. We scan every vector in the namespace but return
        // only `limit` rows, so parsing the metadata of all N candidates would
        // throw away all but `limit` parses. Defer the parse until after the
        // truncation below, where it runs `limit` times instead of N.
        struct ScoredRow {
            score: f64,
            id: String,
            namespace: String,
            text: String,
            meta_str: String,
        }

        let mut scored: Vec<ScoredRow> = rows
            .into_iter()
            .map(|(id, namespace, text, blob, meta_str)| {
                let stored_vec = bytes_to_vec(&blob);
                let score = cosine_similarity(query_vec, &stored_vec);
                ScoredRow {
                    score,
                    id,
                    namespace,
                    text,
                    meta_str,
                }
            })
            .collect();

        // Sort descending by score.
        scored.sort_by(|a, b| {
            b.score
                .partial_cmp(&a.score)
                .unwrap_or(std::cmp::Ordering::Equal)
        });
        scored.truncate(limit);

        // Parse metadata only for the rows that survived truncation. Invalid
        // JSON falls back to Null.
        let results: Vec<SearchResult> = scored
            .into_iter()
            .map(|row| {
                let metadata =
                    serde_json::from_str(&row.meta_str).unwrap_or(serde_json::Value::Null);
                SearchResult {
                    id: row.id,
                    namespace: row.namespace,
                    text: row.text,
                    score: row.score,
                    metadata,
                }
            })
            .collect();

        Ok(results)
    }

    // ── Delete / management ──────────────────────────────────

    /// Deletes a single entry by ID within a namespace.
    ///
    /// Returns `true` if a row was actually deleted.
    pub fn delete(&self, namespace: &str, id: &str) -> anyhow::Result<bool> {
        let conn = self.conn.lock();
        let affected = conn.execute(
            "DELETE FROM vectors WHERE namespace = ?1 AND id = ?2",
            rusqlite::params![namespace, id],
        )?;
        Ok(affected > 0)
    }

    /// Deletes all entries in a namespace.
    ///
    /// Returns the number of deleted rows.
    pub fn clear_namespace(&self, namespace: &str) -> anyhow::Result<usize> {
        let conn = self.conn.lock();
        let affected = conn.execute(
            "DELETE FROM vectors WHERE namespace = ?1",
            rusqlite::params![namespace],
        )?;
        Ok(affected)
    }

    /// Returns the number of entries in a namespace (or all if `None`).
    pub fn count(&self, namespace: Option<&str>) -> anyhow::Result<usize> {
        let conn = self.conn.lock();
        // SQLite COUNT(*) is an i64; rusqlite (>= 0.33) no longer implements
        // FromSql for usize, so read as i64 and convert. Overflow is impossible
        // in practice but handled rather than silently truncated.
        let count: i64 = match namespace {
            Some(ns) => conn.query_row(
                "SELECT COUNT(*) FROM vectors WHERE namespace = ?1",
                rusqlite::params![ns],
                |row| row.get(0),
            )?,
            None => conn.query_row("SELECT COUNT(*) FROM vectors", [], |row| row.get(0))?,
        };
        usize::try_from(count).context("vector count did not fit in usize")
    }

    /// Lists all distinct namespaces.
    pub fn list_namespaces(&self) -> anyhow::Result<Vec<String>> {
        let conn = self.conn.lock();
        let mut stmt = conn.prepare("SELECT DISTINCT namespace FROM vectors ORDER BY namespace")?;
        let namespaces: Vec<String> = stmt
            .query_map([], |row| row.get(0))?
            .collect::<rusqlite::Result<Vec<_>>>()?;
        Ok(namespaces)
    }
}

// ── Vector math utilities ────────────────────────────────────

/// Serializes a float vector to little-endian bytes for SQLite BLOB storage.
pub fn vec_to_bytes(v: &[f32]) -> Vec<u8> {
    let mut bytes = Vec::with_capacity(v.len() * 4);
    for &f in v {
        bytes.extend_from_slice(&f.to_le_bytes());
    }
    bytes
}

/// Deserializes little-endian bytes back to a float vector.
///
/// Uses `chunks_exact(4)`, so any trailing 1-3 bytes that don't form a full
/// `f32` are silently dropped rather than erroring — a truncated or corrupt
/// blob yields a shorter vector instead of a decode failure. (Contrast with
/// `crate::memory::chunks::embedding_from_blob`, the chunk-store's own blob
/// decoder, which errors instead of truncating — the two decoders currently
/// have different contracts for malformed input.)
pub fn bytes_to_vec(bytes: &[u8]) -> Vec<f32> {
    bytes
        .chunks_exact(4)
        .map(|chunk| {
            let arr: [u8; 4] = chunk.try_into().unwrap_or([0; 4]);
            f32::from_le_bytes(arr)
        })
        .collect()
}

/// Computes cosine similarity between two vectors. Returns 0.0 for
/// mismatched lengths, empty vectors, or zero-magnitude vectors.
///
/// The raw cosine value (mathematically in `[-1.0, 1.0]`) is clamped to
/// `[0.0, 1.0]` before being returned, so two vectors pointing in opposite
/// directions score identically to two unrelated (orthogonal) vectors — both
/// report `0.0`. Callers that need to distinguish "opposite" from "unrelated"
/// cannot do so from this score alone.
pub fn cosine_similarity(a: &[f32], b: &[f32]) -> f64 {
    if a.len() != b.len() || a.is_empty() {
        return 0.0;
    }
    let mut dot = 0.0_f64;
    let mut norm_a = 0.0_f64;
    let mut norm_b = 0.0_f64;
    for (x, y) in a.iter().zip(b.iter()) {
        let x = f64::from(*x);
        let y = f64::from(*y);
        dot += x * y;
        norm_a += x * x;
        norm_b += y * y;
    }
    let denom = norm_a.sqrt() * norm_b.sqrt();
    if denom <= f64::EPSILON {
        return 0.0;
    }
    (dot / denom).clamp(0.0, 1.0)
}

fn now_ts() -> f64 {
    use std::time::{SystemTime, UNIX_EPOCH};
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs_f64())
        .unwrap_or(0.0)
}

#[cfg(test)]
#[path = "store_tests.rs"]
mod tests;
