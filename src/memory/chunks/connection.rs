//! Cached SQLite connection management for the chunk DB.
//!
//! `with_connection()` previously opened a new SQLite connection and re-ran the
//! full schema init on every call. This module installs a process-level
//! `ConnectionCache` keyed by DB path: each entry holds one
//! `parking_lot::Mutex<Connection>` that is initialised once (schema +
//! migrations) and reused for all subsequent calls. A per-entry
//! [`CircuitBreaker`] stops retrying after [`CB_THRESHOLD`] consecutive init
//! failures for [`CB_COOLDOWN`] so a broken install does not busy-loop.

use anyhow::{Context, Result};
use parking_lot::Mutex as PMutex;
use rusqlite::Connection;
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicU32, Ordering};
#[cfg(test)]
use std::sync::Mutex;
use std::sync::{Arc, OnceLock};
use std::time::{Duration, Instant};

use super::migrations::{migrate_legacy_embeddings_to_sidecar, purge_global_topic_trees};
use super::recovery::{is_io_open_error, try_cleanup_stale_files};
use super::schema::SCHEMA;
use super::{db_path_for, SQLITE_BUSY_TIMEOUT};
use crate::memory::config::MemoryConfig;

// ── Schema-apply instrumentation (test-only) ─────────────────────────────────

#[cfg(test)]
static SCHEMA_APPLY_COUNTS: OnceLock<Mutex<HashMap<PathBuf, usize>>> = OnceLock::new();

fn record_schema_apply(_path: &Path) {
    #[cfg(test)]
    {
        let counts = SCHEMA_APPLY_COUNTS.get_or_init(|| Mutex::new(HashMap::new()));
        let mut guard = counts.lock().expect("schema apply count mutex poisoned");
        *guard.entry(_path.to_path_buf()).or_insert(0) += 1;
    }
}

#[cfg(test)]
#[doc(hidden)]
pub(crate) fn schema_apply_count_for_path_for_tests(path: &Path) -> usize {
    SCHEMA_APPLY_COUNTS
        .get()
        .and_then(|m| {
            m.lock()
                .ok()
                .map(|guard| guard.get(path).copied().unwrap_or(0))
        })
        .unwrap_or(0)
}

// ── Circuit breaker ──────────────────────────────────────────────────────────

/// How many consecutive init failures before the circuit breaker trips.
pub(crate) const CB_THRESHOLD: u32 = 3;
/// How long the circuit breaker holds the DB closed after tripping.
pub(crate) const CB_COOLDOWN: Duration = Duration::from_secs(30);

/// Per-path circuit breaker: after [`CB_THRESHOLD`] consecutive init failures
/// the breaker trips and `get_or_init_connection` returns an error immediately
/// until [`CB_COOLDOWN`] elapses. On the first success it resets to zero.
struct CircuitBreaker {
    consecutive_failures: AtomicU32,
    tripped: AtomicBool,
    last_trip: PMutex<Option<Instant>>,
}

impl CircuitBreaker {
    fn new() -> Self {
        Self {
            consecutive_failures: AtomicU32::new(0),
            tripped: AtomicBool::new(false),
            last_trip: PMutex::new(None),
        }
    }

    /// Records a successful init. Returns `true` if this call cleared a
    /// previously-tripped breaker (a transition back to healthy).
    fn record_success(&self) -> bool {
        self.consecutive_failures.store(0, Ordering::Relaxed);
        *self.last_trip.lock() = None;
        self.tripped.swap(false, Ordering::Relaxed)
    }

    /// Records one more failure. Returns `true` if this call just tripped the
    /// breaker (i.e. the threshold was crossed right now).
    fn record_failure(&self) -> bool {
        let prev = self.consecutive_failures.fetch_add(1, Ordering::Relaxed);
        let count = prev + 1;
        if count >= CB_THRESHOLD && !self.tripped.swap(true, Ordering::Relaxed) {
            *self.last_trip.lock() = Some(Instant::now());
            return true;
        }
        // Re-arm the cooldown on each subsequent failure while already tripped.
        if self.tripped.load(Ordering::Relaxed) {
            *self.last_trip.lock() = Some(Instant::now());
        }
        false
    }

    /// Returns `true` when the breaker is open AND the cooldown has not yet
    /// elapsed. Returns `false` (allowing a retry) once the cooldown passes.
    fn is_open(&self) -> bool {
        if !self.tripped.load(Ordering::Relaxed) {
            return false;
        }
        let guard = self.last_trip.lock();
        matches!(*guard, Some(t) if t.elapsed() < CB_COOLDOWN)
    }
}

// ── Connection cache ─────────────────────────────────────────────────────────

struct ConnectionCache {
    connections: PMutex<HashMap<PathBuf, Arc<PMutex<Connection>>>>,
    breakers: PMutex<HashMap<PathBuf, Arc<CircuitBreaker>>>,
    init_locks: PMutex<HashMap<PathBuf, Arc<PMutex<()>>>>,
}

static CONN_CACHE: OnceLock<ConnectionCache> = OnceLock::new();

fn conn_cache() -> &'static ConnectionCache {
    CONN_CACHE.get_or_init(|| ConnectionCache {
        connections: PMutex::new(HashMap::new()),
        breakers: PMutex::new(HashMap::new()),
        init_locks: PMutex::new(HashMap::new()),
    })
}

/// Run the full one-time DB initialisation (journal mode, schema, migrations)
/// against an already-open `Connection`.
fn init_db(conn: &Connection, config: &MemoryConfig) -> Result<()> {
    conn.busy_timeout(SQLITE_BUSY_TIMEOUT)
        .context("Failed to configure chunk DB busy timeout")?;
    // SQLite resets `foreign_keys` to off on every new connection — set it here
    // so fast-path callers reuse the cached conn with FKs already on.
    conn.execute_batch("PRAGMA foreign_keys = ON;")
        .context("Failed to enable chunk DB foreign_keys pragma")?;
    // The chunk DB runs the TRUNCATE rollback journal, so crash-safety requires
    // synchronous=FULL — NORMAL is only corruption-safe under WAL.
    conn.execute_batch("PRAGMA synchronous = FULL;")
        .context("Failed to set chunk DB synchronous=FULL")?;
    apply_schema(conn)?;
    migrate_legacy_embeddings_to_sidecar(conn, config)?;
    purge_global_topic_trees(conn, config)?;
    Ok(())
}

fn apply_schema(conn: &Connection) -> Result<()> {
    // The chunk DB uses the TRUNCATE rollback journal, NOT WAL. WAL's `-shm`
    // shared-memory index + `-wal` checkpoint machinery are the root of the
    // cold-start IOERR_SHMMAP / IOERR_TRUNCATE failures. Requesting TRUNCATE on
    // a DB a prior release left in WAL mode checkpoints the `-wal` back into the
    // main file and removes the side-files, migrating WAL databases in place.
    let journal_mode: String = conn
        .query_row("PRAGMA journal_mode=TRUNCATE", [], |row| row.get(0))
        .context("Failed to set chunk DB journal_mode=TRUNCATE")?;
    if !journal_mode.eq_ignore_ascii_case("truncate") {}
    conn.execute_batch(SCHEMA)
        .context("Failed to initialize chunk DB schema")?;
    // Additive, idempotent migrations.
    add_column_if_missing(conn, "mem_tree_chunks", "embedding", "BLOB")?;
    add_column_if_missing(conn, "mem_tree_score", "llm_importance", "REAL")?;
    add_column_if_missing(conn, "mem_tree_score", "llm_importance_reason", "TEXT")?;
    add_column_if_missing(conn, "mem_tree_chunks", "parent_summary_id", "TEXT")?;
    add_column_if_missing(conn, "mem_tree_summaries", "embedding", "BLOB")?;
    add_column_if_missing(
        conn,
        "mem_tree_chunks",
        "lifecycle_status",
        "TEXT NOT NULL DEFAULT 'admitted'",
    )?;
    conn.execute_batch(
        "CREATE INDEX IF NOT EXISTS idx_mem_tree_chunks_lifecycle \
         ON mem_tree_chunks(lifecycle_status);",
    )
    .context("Failed to create mem_tree_chunks lifecycle index")?;
    add_column_if_missing(conn, "mem_tree_chunks", "path_scope", "TEXT")?;
    add_column_if_missing(conn, "mem_tree_chunks", "content_path", "TEXT")?;
    add_column_if_missing(conn, "mem_tree_chunks", "content_sha256", "TEXT")?;
    add_column_if_missing(conn, "mem_tree_summaries", "content_path", "TEXT")?;
    add_column_if_missing(conn, "mem_tree_summaries", "content_sha256", "TEXT")?;
    add_column_if_missing(conn, "mem_tree_summaries", "doc_id", "TEXT")?;
    add_column_if_missing(conn, "mem_tree_summaries", "version_ms", "INTEGER")?;
    conn.execute_batch(
        "CREATE INDEX IF NOT EXISTS idx_mem_tree_summaries_doc_version \
         ON mem_tree_summaries(tree_id, doc_id, version_ms);",
    )
    .context("Failed to create mem_tree_summaries doc/version index")?;
    add_column_if_missing(conn, "mem_tree_chunks", "raw_refs_json", "TEXT")?;
    add_column_if_missing(
        conn,
        "mem_tree_entity_index",
        "is_user",
        "INTEGER NOT NULL DEFAULT 0",
    )?;
    add_column_if_missing(conn, "mem_tree_jobs", "failure_reason", "TEXT")?;
    add_column_if_missing(conn, "mem_tree_jobs", "failure_class", "TEXT")?;
    Ok(())
}

/// Idempotent `ALTER TABLE ADD COLUMN` — treats an existing column as success.
pub(super) fn add_column_if_missing(
    conn: &Connection,
    table: &str,
    name: &str,
    sql_type: &str,
) -> Result<()> {
    match conn.execute(
        &format!("ALTER TABLE {table} ADD COLUMN {name} {sql_type}"),
        [],
    ) {
        Ok(_) => Ok(()),
        Err(err) if err.to_string().contains("duplicate column name") => Ok(()),
        Err(err) => Err(err).with_context(|| format!("Failed to add column {table}.{name}")),
    }
}

/// Obtain (or lazily create) a cached connection for the workspace described by
/// `config`. Returns `Err` immediately when the circuit breaker is open.
pub(crate) fn get_or_init_connection(config: &MemoryConfig) -> Result<Arc<PMutex<Connection>>> {
    let db_path = db_path_for(config);

    // Fast path: reject immediately if the breaker is open.
    {
        let breakers = conn_cache().breakers.lock();
        if let Some(breaker) = breakers.get(&db_path) {
            if breaker.is_open() {
                anyhow::bail!(
                    "[chunks] circuit breaker open for {}: too many consecutive init failures",
                    db_path.display()
                );
            }
        }
    }
    // Fast path: return cached connection if already initialised.
    {
        let guard = conn_cache().connections.lock();
        if let Some(conn) = guard.get(&db_path) {
            return Ok(Arc::clone(conn));
        }
    }

    // Slow path: serialise init per-path so concurrent workers don't all race
    // into `open_and_init` on a cold DB.
    let init_lock = {
        let mut guard = conn_cache().init_locks.lock();
        guard
            .entry(db_path.clone())
            .or_insert_with(|| Arc::new(PMutex::new(())))
            .clone()
    };
    let _init_guard = init_lock.lock();

    // Re-check the cache once we hold the init lock.
    {
        let guard = conn_cache().connections.lock();
        if let Some(conn) = guard.get(&db_path) {
            return Ok(Arc::clone(conn));
        }
    }

    // Attempt to open + init. On certain I/O errors we clean up stale WAL/SHM
    // side-files and retry once.
    let conn = open_and_init(&db_path, config).or_else(|first_err| {
        if is_io_open_error(&first_err) {
            try_cleanup_stale_files(&db_path);
            open_and_init(&db_path, config)
        } else {
            Err(first_err)
        }
    });

    match conn {
        Ok(conn) => {
            let arc_conn = Arc::new(PMutex::new(conn));
            conn_cache()
                .connections
                .lock()
                .insert(db_path.clone(), Arc::clone(&arc_conn));
            let breaker = {
                let mut guard = conn_cache().breakers.lock();
                guard
                    .entry(db_path.clone())
                    .or_insert_with(|| Arc::new(CircuitBreaker::new()))
                    .clone()
            };
            if breaker.record_success() {}
            Ok(arc_conn)
        }
        Err(err) => {
            let breaker = {
                let mut guard = conn_cache().breakers.lock();
                guard
                    .entry(db_path.clone())
                    .or_insert_with(|| Arc::new(CircuitBreaker::new()))
                    .clone()
            };
            if breaker.record_failure() {}
            Err(err)
        }
    }
}

/// Ensure the DB directory exists, open the SQLite file, and run the full
/// schema init sequence.
fn open_and_init(db_path: &Path, config: &MemoryConfig) -> Result<Connection> {
    let dir = db_path.parent().expect("db_path always has a parent");
    std::fs::create_dir_all(dir)
        .with_context(|| format!("Failed to create chunk DB dir: {}", dir.display()))?;
    let conn = Connection::open(db_path)
        .with_context(|| format!("Failed to open chunk DB: {}", db_path.display()))?;
    init_db(&conn, config)
        .with_context(|| format!("Failed to init chunk DB schema: {}", db_path.display()))?;
    record_schema_apply(db_path);
    Ok(conn)
}

/// Remove the cached connection for `config`'s workspace (forces a fresh open
/// on the next `with_connection` call). Also clears the breaker.
#[allow(dead_code)]
pub(crate) fn invalidate_connection(config: &MemoryConfig) {
    let db_path = db_path_for(config);
    conn_cache().connections.lock().remove(&db_path);
    conn_cache().breakers.lock().remove(&db_path);
}

/// Drop the cached connection + breaker for `config` (used by corrupt-DB
/// recovery before quarantining the on-disk file).
pub(super) fn drop_cached_connection(config: &MemoryConfig) {
    let db_path = db_path_for(config);
    conn_cache().connections.lock().remove(&db_path);
    conn_cache().breakers.lock().remove(&db_path);
}

/// Clear the entire connection cache. For test isolation only.
#[cfg(test)]
pub(crate) fn clear_connection_cache() {
    conn_cache().connections.lock().clear();
    conn_cache().breakers.lock().clear();
    conn_cache().init_locks.lock().clear();
}

/// Open the chunk SQLite DB and run a closure against it.
///
/// The underlying connection is initialised once per workspace path and reused
/// from a process-level cache. Schema migrations run exactly once on the first
/// call for a given `config.workspace`.
pub fn with_connection<T>(
    config: &MemoryConfig,
    f: impl FnOnce(&Connection) -> Result<T>,
) -> Result<T> {
    let conn_arc = get_or_init_connection(config)?;
    let guard = conn_arc.lock();
    f(&guard)
}

#[cfg(test)]
#[path = "connection_tests.rs"]
mod tests;
