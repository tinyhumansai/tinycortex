//! Key-value storage — `kv_global` + `kv_namespace` tables.
//!
//! A first-class peer of the vector store and entity index: a self-contained
//! SQLite-backed JSON KV with a global scope and per-namespace scopes. Ported
//! from OpenHuman's `memory_store::kv` (lifted off the `UnifiedMemory`
//! connection into a standalone [`KvStore`] with its own connection).
//!
//! Writes run through the [`safety`](crate::memory::store::safety) guard:
//! secret-like or PII-like keys/namespaces are rejected outright, and values
//! are sanitized before they land in the store.

use parking_lot::Mutex;
use rusqlite::{params, Connection, OptionalExtension};
use serde_json::{json, Value};
use std::path::Path;
use std::sync::Arc;

use crate::memory::store::safety;

fn parse_value_json(raw: &str, key: &str, context: &str) -> Result<Value, String> {
    serde_json::from_str(raw).map_err(|error| format!("{context} JSON for key '{key}': {error}"))
}
use crate::memory::types::MemoryKvRecord;

const SCHEMA_SQL: &str = "
    CREATE TABLE IF NOT EXISTS kv_global (
        key        TEXT    PRIMARY KEY,
        value_json TEXT    NOT NULL,
        updated_at REAL    NOT NULL
    );

    CREATE TABLE IF NOT EXISTS kv_namespace (
        namespace  TEXT    NOT NULL,
        key        TEXT    NOT NULL,
        value_json TEXT    NOT NULL,
        updated_at REAL    NOT NULL,
        PRIMARY KEY (namespace, key)
    );
    CREATE INDEX IF NOT EXISTS idx_kv_ns ON kv_namespace(namespace);
";

const OWNED_CONNECTION_PRAGMAS: &str = "
    PRAGMA journal_mode = WAL;
    PRAGMA synchronous = NORMAL;
    PRAGMA busy_timeout = 15000;
";

/// SQLite-backed global + namespace JSON key-value store.
///
/// Thread-safe: the connection is behind a `parking_lot::Mutex`.
pub struct KvStore {
    conn: Arc<Mutex<Connection>>,
}

impl KvStore {
    /// Open (or create) a KV store at `db_path`.
    pub fn open(db_path: &Path) -> anyhow::Result<Self> {
        if let Some(parent) = db_path.parent() {
            std::fs::create_dir_all(parent)?;
        }
        let conn = Connection::open(db_path)?;
        Self::init_owned_connection(&conn)?;
        Ok(Self {
            conn: Arc::new(Mutex::new(conn)),
        })
    }

    /// Open an in-memory KV store (useful for tests).
    pub fn open_in_memory() -> anyhow::Result<Self> {
        let conn = Connection::open_in_memory()?;
        Self::init_owned_connection(&conn)?;
        Ok(Self {
            conn: Arc::new(Mutex::new(conn)),
        })
    }

    /// Use a connection owned and configured by the embedding application.
    ///
    /// Only the KV schema is installed. Journal mode, synchronous mode, busy
    /// timeout, and transaction policy remain owned by the caller.
    pub fn from_shared_connection(conn: Arc<Mutex<Connection>>) -> anyhow::Result<Self> {
        conn.lock().execute_batch(SCHEMA_SQL)?;
        Ok(Self { conn })
    }

    fn init_owned_connection(conn: &Connection) -> anyhow::Result<()> {
        conn.execute_batch(OWNED_CONNECTION_PRAGMAS)?;
        conn.execute_batch(SCHEMA_SQL)?;
        Ok(())
    }

    /// Seconds since the Unix epoch, as a float (matches OpenHuman's `updated_at`).
    fn now_ts() -> f64 {
        use std::time::{SystemTime, UNIX_EPOCH};
        SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map(|d| d.as_secs_f64())
            .unwrap_or(0.0)
    }

    /// Normalise a namespace into a stable storage key: lowercase is *not*
    /// applied (callers may rely on case), but whitespace and path-hostile
    /// characters collapse to `_` so `"team alpha/#1"` and `"team_alpha/_1"`
    /// address the same bucket.
    fn sanitize_namespace(namespace: &str) -> String {
        let trimmed = namespace.trim();
        if trimmed.is_empty() {
            return crate::memory::types::GLOBAL_NAMESPACE.to_string();
        }
        trimmed
            .chars()
            .map(|c| match c {
                c if c.is_ascii_alphanumeric() || matches!(c, '-' | '_' | '/') => c,
                _ => '_',
            })
            .collect()
    }

    /// Insert or update a global key-value pair.
    ///
    /// Returns `Err` when the key looks like a secret or personal identifier.
    pub fn set_global(&self, key: &str, value: &Value) -> Result<(), String> {
        if safety::has_likely_secret(key) {
            return Err("kv key cannot contain secrets".to_string());
        }
        if safety::has_likely_email(key) || safety::has_likely_pii(key) {
            return Err("kv key cannot contain personal identifiers".to_string());
        }

        let sanitized = safety::sanitize_json(value);
        let conn = self.conn.lock();
        conn.execute(
            "INSERT INTO kv_global (key, value_json, updated_at)
             VALUES (?1, ?2, ?3)
             ON CONFLICT(key) DO UPDATE SET value_json = excluded.value_json, updated_at = excluded.updated_at",
            params![key, sanitized.value.to_string(), Self::now_ts()],
        )
        .map_err(|e| format!("set_global: {e}"))?;
        Ok(())
    }

    /// Read a global key, returning `None` if absent.
    ///
    /// A present row with corrupt JSON returns an error rather than masquerading
    /// as a missing key.
    pub fn get_global(&self, key: &str) -> Result<Option<Value>, String> {
        let conn = self.conn.lock();
        let value: Option<String> = conn
            .query_row(
                "SELECT value_json FROM kv_global WHERE key = ?1",
                params![key],
                |row| row.get(0),
            )
            .optional()
            .map_err(|e| format!("get_global: {e}"))?;
        value
            .map(|raw| serde_json::from_str(&raw).map_err(|e| format!("get_global JSON: {e}")))
            .transpose()
    }

    /// Insert or update a namespace-scoped key-value pair.
    pub fn set_namespace(&self, namespace: &str, key: &str, value: &Value) -> Result<(), String> {
        if safety::has_likely_secret(namespace) || safety::has_likely_secret(key) {
            return Err("kv namespace/key cannot contain secrets".to_string());
        }
        if safety::has_likely_email(namespace)
            || safety::has_likely_email(key)
            || safety::has_likely_pii(namespace)
            || safety::has_likely_pii(key)
        {
            return Err("kv namespace/key cannot contain personal identifiers".to_string());
        }

        let sanitized = safety::sanitize_json(value);
        let conn = self.conn.lock();
        conn.execute(
            "INSERT INTO kv_namespace (namespace, key, value_json, updated_at)
             VALUES (?1, ?2, ?3, ?4)
             ON CONFLICT(namespace, key) DO UPDATE SET value_json = excluded.value_json, updated_at = excluded.updated_at",
            params![
                Self::sanitize_namespace(namespace),
                key,
                sanitized.value.to_string(),
                Self::now_ts()
            ],
        )
        .map_err(|e| format!("set_namespace: {e}"))?;
        Ok(())
    }

    /// Read a namespace-scoped key, returning `None` if absent.
    ///
    /// A corrupt stored value returns an error.
    pub fn get_namespace(&self, namespace: &str, key: &str) -> Result<Option<Value>, String> {
        let conn = self.conn.lock();
        let value: Option<String> = conn
            .query_row(
                "SELECT value_json FROM kv_namespace WHERE namespace = ?1 AND key = ?2",
                params![Self::sanitize_namespace(namespace), key],
                |row| row.get(0),
            )
            .optional()
            .map_err(|e| format!("get_namespace: {e}"))?;
        value
            .map(|raw| serde_json::from_str(&raw).map_err(|e| format!("get_namespace JSON: {e}")))
            .transpose()
    }

    /// Delete a global key. Returns `true` if a row was removed.
    pub fn delete_global(&self, key: &str) -> Result<bool, String> {
        let conn = self.conn.lock();
        let changed = conn
            .execute("DELETE FROM kv_global WHERE key = ?1", params![key])
            .map_err(|e| format!("delete_global: {e}"))?;
        Ok(changed > 0)
    }

    /// Delete a namespace-scoped key. Returns `true` if a row was removed.
    pub fn delete_namespace(&self, namespace: &str, key: &str) -> Result<bool, String> {
        let conn = self.conn.lock();
        let changed = conn
            .execute(
                "DELETE FROM kv_namespace WHERE namespace = ?1 AND key = ?2",
                params![Self::sanitize_namespace(namespace), key],
            )
            .map_err(|e| format!("delete_namespace: {e}"))?;
        Ok(changed > 0)
    }

    /// List all keys in a namespace, most recently updated first, as a JSON
    /// array of `{key, value, updatedAt}` objects.
    ///
    /// A corrupt stored value fails the listing so corruption is observable.
    pub fn list_namespace(&self, namespace: &str) -> Result<Vec<Value>, String> {
        let conn = self.conn.lock();
        let mut stmt = conn
            .prepare(
                "SELECT key, value_json, updated_at FROM kv_namespace
                 WHERE namespace = ?1 ORDER BY updated_at DESC",
            )
            .map_err(|e| format!("list_namespace prepare: {e}"))?;
        let mut rows = stmt
            .query(params![Self::sanitize_namespace(namespace)])
            .map_err(|e| format!("list_namespace query: {e}"))?;
        let mut out = Vec::new();
        while let Some(row) = rows
            .next()
            .map_err(|e| format!("list_namespace row: {e}"))?
        {
            let value_raw: String = row.get(1).map_err(|e| e.to_string())?;
            let key = row.get::<_, String>(0).map_err(|e| e.to_string())?;
            let value = parse_value_json(&value_raw, &key, "list_namespace")?;
            out.push(json!({
                "key": key,
                "value": value,
                "updatedAt": row.get::<_, f64>(2).map_err(|e| e.to_string())?,
            }));
        }
        Ok(out)
    }

    /// All KV records visible to `namespace`: the namespace's own records plus
    /// the global records, newest first.
    pub fn records_for_scope(&self, namespace: &str) -> Result<Vec<MemoryKvRecord>, String> {
        let mut records = self.records_namespace(namespace)?;
        records.extend(self.records_global()?);
        records.sort_by(|a, b| {
            b.updated_at
                .partial_cmp(&a.updated_at)
                .unwrap_or(std::cmp::Ordering::Equal)
        });
        Ok(records)
    }

    /// All records in a namespace as typed [`MemoryKvRecord`]s, newest first.
    ///
    /// An unparsable stored value returns an error.
    pub fn records_namespace(&self, namespace: &str) -> Result<Vec<MemoryKvRecord>, String> {
        let ns = Self::sanitize_namespace(namespace);
        let conn = self.conn.lock();
        let mut stmt = conn
            .prepare(
                "SELECT key, value_json, updated_at FROM kv_namespace
                 WHERE namespace = ?1 ORDER BY updated_at DESC",
            )
            .map_err(|e| format!("prepare records_namespace: {e}"))?;
        let mut rows = stmt
            .query(params![ns])
            .map_err(|e| format!("query records_namespace: {e}"))?;
        let mut out = Vec::new();
        while let Some(row) = rows
            .next()
            .map_err(|e| format!("row records_namespace: {e}"))?
        {
            let value_raw: String = row.get(1).map_err(|e| e.to_string())?;
            let key: String = row.get(0).map_err(|e| e.to_string())?;
            let value = parse_value_json(&value_raw, &key, "records_namespace")?;
            out.push(MemoryKvRecord {
                namespace: Some(ns.clone()),
                key,
                value,
                updated_at: row.get(2).map_err(|e| e.to_string())?,
            });
        }
        Ok(out)
    }

    /// All global records as typed [`MemoryKvRecord`]s, newest first.
    ///
    /// An unparsable stored value returns an error.
    pub fn records_global(&self) -> Result<Vec<MemoryKvRecord>, String> {
        let conn = self.conn.lock();
        let mut stmt = conn
            .prepare("SELECT key, value_json, updated_at FROM kv_global ORDER BY updated_at DESC")
            .map_err(|e| format!("prepare records_global: {e}"))?;
        let mut rows = stmt
            .query([])
            .map_err(|e| format!("query records_global: {e}"))?;
        let mut out = Vec::new();
        while let Some(row) = rows
            .next()
            .map_err(|e| format!("row records_global: {e}"))?
        {
            let value_raw: String = row.get(1).map_err(|e| e.to_string())?;
            let key: String = row.get(0).map_err(|e| e.to_string())?;
            let value = parse_value_json(&value_raw, &key, "records_global")?;
            out.push(MemoryKvRecord {
                namespace: None,
                key,
                value,
                updated_at: row.get(2).map_err(|e| e.to_string())?,
            });
        }
        Ok(out)
    }
}

#[cfg(test)]
#[path = "kv_tests.rs"]
mod tests;
