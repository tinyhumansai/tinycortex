//! Cached SQLite connection management for the chunk DB.
//!
//! `with_connection()` previously opened a new SQLite connection and re-ran the
//! full schema init on every call. This module installs a process-level
//! `ConnectionCache` keyed by DB path: each entry holds one
//! `parking_lot::Mutex<Connection>` that is initialised once (schema +
//! migrations) and reused for all subsequent calls. A per-entry
//! [`CircuitBreaker`] stops retrying after the configured failure threshold
//! for a cooldown period so a broken install does not busy-loop.

use anyhow::{Context, Result};
use parking_lot::Mutex as PMutex;
use rusqlite::Connection;
use std::collections::HashMap;
use std::path::{Path, PathBuf};
#[cfg(test)]
use std::sync::Mutex;
use std::sync::{Arc, OnceLock};

use super::connection_breaker::CircuitBreaker;
#[cfg(test)]
pub(crate) use super::connection_breaker::CB_THRESHOLD;

use super::migrations::{migrate_legacy_embeddings_to_sidecar, purge_global_topic_trees};
use super::recovery::{
    is_corrupt_error, is_io_open_error, quarantine_corrupt_files, try_cleanup_stale_files,
};
use super::schema::SCHEMA;
use super::{db_path_for, SQLITE_BUSY_TIMEOUT};
use crate::memory::config::MemoryConfig;

// ── Schema-apply instrumentation (test-only) ─────────────────────────────────

#[cfg(test)]
static SCHEMA_APPLY_COUNTS: OnceLock<Mutex<HashMap<PathBuf, usize>>> = OnceLock::new();

/// Test-only instrumentation: bump the per-path counter of successful
/// [`open_and_init`] calls. A no-op in non-test builds (the whole body is
/// `#[cfg(test)]`-gated), so this carries no cost or behavior in production.
/// Used by tests to assert the connection cache actually skips re-init on
/// subsequent `with_connection` calls for the same path.
fn record_schema_apply(_path: &Path) {
    #[cfg(test)]
    {
        let counts = SCHEMA_APPLY_COUNTS.get_or_init(|| Mutex::new(HashMap::new()));
        let mut guard = counts.lock().expect("schema apply count mutex poisoned");
        *guard.entry(_path.to_path_buf()).or_insert(0) += 1;
    }
}

/// Number of times [`open_and_init`] has run to completion for `path` in this
/// process, or `0` if it has never run. Test-only; panics are avoided (lock
/// poisoning degrades to `0` rather than propagating).
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

// ── Connection cache ─────────────────────────────────────────────────────────

/// Process-global, per-DB-path connection state.
///
/// Three independent maps, each guarded by its own `parking_lot::Mutex` (so
/// locking one never blocks the others): the live cached connection, the
/// circuit breaker tracking recent init failures, and a per-path init lock
/// used to serialise cold-start initialisation across concurrent callers.
/// Entries are never evicted except by [`invalidate_connection`],
/// [`drop_cached_connection`], or test-only path-local reset — one
/// entry accumulates per distinct workspace path for the life of the process.
struct ConnectionCache {
    /// One live, already-initialised `Connection` per DB path, wrapped so
    /// multiple `Arc` holders can share it and `with_connection` callers
    /// serialise access via the inner mutex.
    connections: PMutex<HashMap<PathBuf, Arc<PMutex<Connection>>>>,
    /// One [`CircuitBreaker`] per DB path, tracking consecutive init failures.
    breakers: PMutex<HashMap<PathBuf, Arc<CircuitBreaker>>>,
    /// One dedicated lock per DB path used only to serialise the cold-start
    /// path in [`get_or_init_connection`] — never held during normal
    /// `with_connection` use.
    init_locks: PMutex<HashMap<PathBuf, Arc<PMutex<()>>>>,
}

static CONN_CACHE: OnceLock<ConnectionCache> = OnceLock::new();

/// Access (lazily constructing on first call) the process-wide connection
/// cache singleton.
fn conn_cache() -> &'static ConnectionCache {
    CONN_CACHE.get_or_init(|| ConnectionCache {
        connections: PMutex::new(HashMap::new()),
        breakers: PMutex::new(HashMap::new()),
        init_locks: PMutex::new(HashMap::new()),
    })
}

/// Run the full one-time DB initialisation (busy timeout, pragmas, schema,
/// migrations) against a freshly-[`Connection::open`]ed connection.
///
/// Order matters: `busy_timeout` and `foreign_keys` must be set before any
/// query runs (SQLite resets `foreign_keys` on every new connection, so this
/// must happen per-open, not once globally); `synchronous = FULL` is required
/// because [`apply_schema`] forces the TRUNCATE rollback journal, under which
/// `NORMAL` synchronous is not crash-safe. Idempotent — safe to call again on
/// an already-migrated DB (every step is `IF NOT EXISTS` / duplicate-tolerant).
///
/// # Errors
/// Returns `Err` if any pragma fails to apply, if schema DDL fails, or if
/// either one-shot migration ([`migrate_legacy_embeddings_to_sidecar`],
/// [`purge_global_topic_trees`]) fails.
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

/// Force the TRUNCATE journal mode, apply [`SCHEMA`], and run every additive
/// `ALTER TABLE ADD COLUMN` / index migration accumulated since the initial
/// release.
///
/// All statements here are idempotent (`IF NOT EXISTS` DDL,
/// [`add_column_if_missing`] tolerates an existing column) so calling this
/// against an already-migrated DB is a safe no-op.
///
/// # Errors
/// Returns `Err` if the `journal_mode` pragma, the schema batch, or any
/// individual column/index migration fails.
fn apply_schema(conn: &Connection) -> Result<()> {
    // The chunk DB uses the TRUNCATE rollback journal, NOT WAL. WAL's `-shm`
    // shared-memory index + `-wal` checkpoint machinery are the root of the
    // cold-start IOERR_SHMMAP / IOERR_TRUNCATE failures. Requesting TRUNCATE on
    // a DB a prior release left in WAL mode checkpoints the `-wal` back into the
    // main file and removes the side-files, migrating WAL databases in place.
    let journal_mode: String = conn
        .query_row("PRAGMA journal_mode=TRUNCATE", [], |row| row.get(0))
        .context("Failed to set chunk DB journal_mode=TRUNCATE")?;
    anyhow::ensure!(
        journal_mode.eq_ignore_ascii_case("truncate"),
        "SQLite refused journal_mode=TRUNCATE (active mode: {journal_mode})"
    );
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
    // Ask/flavour instruction for flavoured trees (issue #68). Nullable; NULL for
    // source/topic/global trees.
    add_column_if_missing(conn, "mem_tree_trees", "ask", "TEXT")?;
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

/// Obtain (or lazily create) a cached connection for the workspace described
/// by `config`.
///
/// Fast path: an already-cached, healthy connection is returned without
/// taking any lock beyond the cache's own. Cold path: callers race to acquire
/// a per-DB-path init lock so only one of them actually opens + migrates the
/// database; the rest block briefly and then observe the now-cached
/// connection. On an I/O error consistent with stale WAL/SHM side-files from a
/// prior crash (see [`is_io_open_error`]), the stale files are cleaned up
/// ([`try_cleanup_stale_files`]) and open+init is retried exactly once before
/// giving up.
///
/// # Errors
/// Returns `Err` immediately, without attempting to open the DB, when the
/// per-path circuit breaker is open (see [`CircuitBreaker`]). Otherwise
/// returns `Err` if opening the SQLite file or running [`init_db`] fails
/// (after the single stale-file-cleanup retry) — each failure is recorded
/// against the breaker, and three consecutive failures trip it for the
/// configured cooldown.
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

    // A caller ahead of us may have failed and re-tripped the breaker while
    // we waited for the init lock.
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

    // Re-check the cache once we hold the init lock.
    {
        let guard = conn_cache().connections.lock();
        if let Some(conn) = guard.get(&db_path) {
            return Ok(Arc::clone(conn));
        }
    }

    // Attempt to open + init. On certain I/O errors we clean up stale WAL/SHM
    // side-files and retry once.
    let conn = open_and_init(&db_path, config)
        .or_else(|first_err| {
            if is_io_open_error(&first_err) {
                try_cleanup_stale_files(&db_path);
                open_and_init(&db_path, config)
            } else {
                Err(first_err)
            }
        })
        .or_else(|error| {
            if is_corrupt_error(&error) {
                quarantine_corrupt_files(config)?;
                open_and_init(&db_path, config)
            } else {
                Err(error)
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
            if breaker.record_success() {
                log::info!(
                    "[memory_tree:sqlite] connection circuit breaker recovered path={}",
                    db_path.display()
                );
            }
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
            // NOTE: `record_failure` returns whether this call just tripped
            // the breaker; currently discarded, so the trip event itself is
            // not logged, only observable later via `is_open`. See SC-9.
            breaker.record_failure();
            Err(err)
        }
    }
}

/// Ensure the DB directory exists, open the SQLite file, and run the full
/// schema init sequence ([`init_db`]).
///
/// # Errors
/// Returns `Err` if the parent directory cannot be created, the SQLite file
/// cannot be opened, or [`init_db`] fails. Does not clean up a partially
/// created directory or file on failure — the caller (or a subsequent call)
/// simply retries against the same path.
///
/// # Panics
/// Panics if `db_path` has no parent component. Not reachable in practice:
/// `db_path` is always produced by [`super::db_path_for`], which joins onto
/// `config.workspace`.
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
/// on the next `with_connection` call). Also clears the breaker, so a prior
/// tripped state does not carry over to the next open attempt.
///
/// Does not close the underlying SQLite connection explicitly; if no other
/// `Arc` clone is held elsewhere, dropping the last reference closes it via
/// `rusqlite`'s `Drop` impl. Currently unused in production code
/// (`#[allow(dead_code)]`) — reserved for callers that need to force a
/// reopen (e.g. after external file manipulation).
#[allow(dead_code)]
pub(crate) fn invalidate_connection(config: &MemoryConfig) {
    let db_path = db_path_for(config);
    conn_cache().connections.lock().remove(&db_path);
    conn_cache().breakers.lock().remove(&db_path);
}

/// Quarantine and rebuild a corrupt database while holding the same per-path
/// initialization lock used by cold opens.
pub(super) fn recover_corrupt_connection(config: &MemoryConfig) -> Result<bool> {
    let db_path = db_path_for(config);
    let init_lock = {
        let mut locks = conn_cache().init_locks.lock();
        locks
            .entry(db_path.clone())
            .or_insert_with(|| Arc::new(PMutex::new(())))
            .clone()
    };
    let _init_guard = init_lock.lock();

    // Remove the cache entry, then quiesce every caller that already cloned
    // its Arc before renaming the database. The guard remains held through
    // quarantine and rebuild, so no write can land in the quarantined file.
    let cached = conn_cache().connections.lock().remove(&db_path);
    let _cached_guard = cached.as_ref().map(|connection| connection.lock());
    conn_cache().breakers.lock().remove(&db_path);
    let quarantined = quarantine_corrupt_files(config)?;
    let conn = open_and_init(&db_path, config)
        .context("failed to rebuild chunk DB schema after quarantining corrupt DB")?;
    let connection = Arc::new(PMutex::new(conn));
    conn_cache()
        .connections
        .lock()
        .insert(db_path.clone(), connection);
    let breaker = {
        let mut breakers = conn_cache().breakers.lock();
        breakers
            .entry(db_path)
            .or_insert_with(|| Arc::new(CircuitBreaker::new()))
            .clone()
    };
    breaker.record_success();
    Ok(quarantined)
}

/// Remove only one workspace from the test connection cache.
///
/// Keeping cache-reset scope local prevents parallel tests with unrelated
/// tempdirs from invalidating one another's live `Arc`s.
#[cfg(test)]
pub(crate) fn clear_connection_cache_for(config: &MemoryConfig) {
    let path = db_path_for(config);
    conn_cache().connections.lock().remove(&path);
    conn_cache().init_locks.lock().remove(&path);
}

/// Open the chunk SQLite DB and run a closure against it.
///
/// The underlying connection is initialised once per workspace path and reused
/// from a process-level cache. Schema migrations run exactly once on the first
/// call for a given `config.workspace`.
///
/// `f` runs while holding the connection's `parking_lot::Mutex`: every
/// `with_connection` call for the same workspace path is serialised against
/// every other one, including calls made from different async tasks. This is
/// a synchronous, blocking mutex — calling this function directly from an
/// async context blocks the executor thread for the duration of `f`, and
/// (worst case, on a cold start or after a transient failure) for up to
/// `SQLITE_BUSY_TIMEOUT` while waiting on SQLite's own busy handler. Callers
/// on an async runtime should wrap this in `spawn_blocking` or equivalent.
///
/// # Errors
/// Returns `Err` if the connection cannot be obtained/initialised (circuit
/// breaker open, or open/init failure) or if `f` itself returns `Err`.
pub fn with_connection<T>(
    config: &MemoryConfig,
    f: impl FnOnce(&Connection) -> Result<T>,
) -> Result<T> {
    let conn_arc = get_or_init_connection(config)?;
    let guard = conn_arc.lock();
    f(&guard)
}

/// Return the initialized connection shared by chunk, tree, and auxiliary
/// stores for this workspace.
///
/// Embedding applications may pass this handle to shared-connection stores
/// such as `KvStore` and `EntityIndex`. Callers must not change connection
/// pragmas or hold the mutex across an await point.
pub fn shared_connection(config: &MemoryConfig) -> Result<Arc<PMutex<Connection>>> {
    get_or_init_connection(config)
}

#[cfg(test)]
#[path = "connection_tests.rs"]
mod tests;
