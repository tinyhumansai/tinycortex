//! Cold-start error classification, stale-file cleanup, and corrupt-DB
//! recovery for the chunk SQLite database.
//!
//! Three independent concerns live here:
//! - [`is_io_open_error`] / [`try_cleanup_stale_files`]: the retry path wired
//!   into [`super::connection::get_or_init_connection`] for the *cold-start*
//!   WAL/SHM bootstrap race (a fresh process opening a DB whose side-files
//!   are mid-write from another process).
//! - [`is_transient_cold_start`]: a second, overlapping classifier for the
//!   same error family, currently exercised only by tests (see NOTE on its
//!   doc comment).
//! - [`recover_corrupt_db`] / [`quick_check_ok`]: a `SQLITE_CORRUPT`
//!   quarantine-and-rebuild path that is **not** invoked from any production
//!   call site today — see the NOTE on [`recover_corrupt_db`] (audit finding
//!   SC-5). A real `SQLITE_CORRUPT` on the chunk DB currently wedges the
//!   store rather than recovering.
//!
//! [`try_cleanup_stale_files`] never unlinks the `-wal`: it first attempts a
//! `wal_checkpoint(TRUNCATE)` and, only if that fails, quarantines the `-wal`
//! by renaming it to a timestamped `.quarantine` sibling so committed data is
//! preserved for manual recovery (audit finding SC-4). Only the `-shm` is
//! deleted outright. It targets the specific cold-start bootstrap races, not
//! a general-purpose "fix a stuck DB" hammer.

use anyhow::{Context, Result};
use chrono::Utc;
use rusqlite::Connection;
use std::path::{Path, PathBuf};

use super::connection::{drop_cached_connection, get_or_init_connection};
use super::{db_path_for, SQLITE_BUSY_TIMEOUT};
use crate::memory::config::MemoryConfig;

// SQLite extended result codes that fire during cold-start WAL/SHM bootstrap
// races. Extended codes are `SQLITE_IOERR (10) | (sub << 8)`.
/// `CANTOPEN` — racing the lockfile/WAL creation done by another connection.
const SQLITE_CANTOPEN: i32 = 14;
/// `IOERR_TRUNCATE` — the WAL/db is being truncated during bootstrap.
const SQLITE_IOERR_TRUNCATE: i32 = 1546;
/// `IOERR_SHMOPEN` — opening a new `-shm` shared-memory segment failed.
const SQLITE_IOERR_SHMOPEN: i32 = 4618;
/// `IOERR_SHMSIZE` — the `-shm` file is being resized during bootstrap.
const SQLITE_IOERR_SHMSIZE: i32 = 4874;
/// `IOERR_SHMMAP` — mapping a page of the `-shm` wal-index failed.
const SQLITE_IOERR_SHMMAP: i32 = 5386;
/// `IOERR_IN_PAGE` — an mmap-page I/O fault, also seen under WAL cold-start.
const SQLITE_IOERR_IN_PAGE: i32 = 8714;

/// True if `err` (or anything in its cause chain) is one of the SQLite codes
/// that fire during cold-start WAL/SHM bootstrap races.
///
/// Checks both `err.root_cause()` and every error in `err`'s `source()` chain,
/// so it catches the code whether `anyhow` wrapped the `rusqlite::Error`
/// directly at the root or nested it under additional context.
///
/// NOTE: this duplicates the code list in [`is_io_open_error`] (used by the
/// live retry path in `connection.rs`) but is itself `#[allow(dead_code)]` —
/// it is currently only reachable from this crate's test modules, not from
/// any production call site.
#[allow(dead_code)]
pub(crate) fn is_transient_cold_start(err: &anyhow::Error) -> bool {
    fn is_transient_sqlite(e: &(dyn std::error::Error + 'static)) -> bool {
        if let Some(rusqlite::Error::SqliteFailure(ffi, _)) = e.downcast_ref::<rusqlite::Error>() {
            return matches!(
                ffi.extended_code,
                SQLITE_CANTOPEN
                    | SQLITE_IOERR_TRUNCATE
                    | SQLITE_IOERR_SHMOPEN
                    | SQLITE_IOERR_SHMSIZE
                    | SQLITE_IOERR_SHMMAP
                    | SQLITE_IOERR_IN_PAGE
            );
        }
        false
    }
    if is_transient_sqlite(err.root_cause()) {
        return true;
    }
    let mut src: Option<&(dyn std::error::Error + 'static)> = Some(err.as_ref());
    while let Some(cur) = src {
        if is_transient_sqlite(cur) {
            return true;
        }
        src = cur.source();
    }
    false
}

/// Whether `err` looks like one of the I/O error codes that warrant a
/// stale-file cleanup + single retry before giving up.
///
/// Checks only the immediate `downcast_ref::<rusqlite::Error>()` on `err`
/// (not the full cause chain the way [`is_transient_cold_start`] does),
/// falling back to a case-insensitive substring match against the formatted
/// error (`{err:#}`) for error shapes that don't downcast cleanly — this
/// catches messages surfaced through `anyhow::Context` wrapping that loses
/// the original `rusqlite::Error` type.
pub(crate) fn is_io_open_error(err: &anyhow::Error) -> bool {
    if let Some(rusqlite::Error::SqliteFailure(f, _)) = err.downcast_ref::<rusqlite::Error>() {
        return matches!(
            f.extended_code,
            SQLITE_CANTOPEN
                | SQLITE_IOERR_TRUNCATE
                | SQLITE_IOERR_SHMOPEN
                | SQLITE_IOERR_SHMSIZE
                | SQLITE_IOERR_SHMMAP
                | SQLITE_IOERR_IN_PAGE
        ) || f.code == rusqlite::ErrorCode::CannotOpen;
    }
    let msg = format!("{err:#}").to_ascii_lowercase();
    msg.contains("disk i/o error")
        || msg.contains("unable to open database file")
        || msg.contains("xshmmap")
        || msg.contains("truncate file")
}

/// Clean up WAL/SHM side-files that can block a clean DB open after a crash,
/// **without ever discarding committed data**.
///
/// SQLite writes committed-but-uncheckpointed transactions into the `-wal`
/// side-file; for a legacy DB still in WAL mode (see `connection::apply_schema`,
/// which migrates such DBs to TRUNCATE on a *successful* open) that data lives
/// *only* in the `-wal` until a checkpoint folds it back into the main file.
/// Unconditionally unlinking the `-wal` here would silently drop those
/// committed transactions. This routine therefore:
///
/// 1. Never unlinks the `-wal`. It first attempts a `wal_checkpoint(TRUNCATE)`
///    so any committed frames are folded into the main DB.
/// 2. Only if that checkpoint attempt fails does it *quarantine* the `-wal` —
///    renaming it to a timestamped `.quarantine-<ts>` sibling that is preserved
///    for manual recovery rather than deleted.
/// 3. Always removes the `-shm` where present: it is a pure, rebuildable
///    shared-memory index, and deleting it clears the stale wal-index state that
///    drives the cold-start `IOERR_SHM*` failures (SQLite rebuilds it on open).
///
/// Returns `true` if anything was checkpointed, quarantined, or removed.
pub(crate) fn try_cleanup_stale_files(db_path: &Path) -> bool {
    let mut cleaned = false;
    let wal = with_name_suffix(db_path, "-wal");
    let shm = with_name_suffix(db_path, "-shm");

    if wal.exists() {
        if try_checkpoint(db_path) {
            // Committed frames (if any) are now folded into the main DB; a
            // successful TRUNCATE checkpoint also drops the `-wal` on close.
            cleaned = true;
        } else if quarantine_file(&wal) {
            // The checkpoint failed — preserve the WAL's committed frames by
            // renaming rather than destroying them, but still unblock the open.
            cleaned = true;
        }
    }

    // The `-shm` never holds durable data, so deleting it is always safe.
    if shm.exists() && std::fs::remove_file(&shm).is_ok() {
        cleaned = true;
    }

    cleaned
}

/// Open `db_path` on a short-lived connection and run
/// `PRAGMA wal_checkpoint(TRUNCATE)` to fold committed WAL frames back into the
/// main database. Returns `true` only when the checkpoint completed (the `busy`
/// flag is `0`); any open/query failure yields `false` so the caller quarantines
/// the `-wal` instead of trusting a partial or missed checkpoint.
fn try_checkpoint(db_path: &Path) -> bool {
    let conn = match Connection::open(db_path) {
        Ok(conn) => conn,
        Err(_) => return false,
    };
    let _ = conn.busy_timeout(SQLITE_BUSY_TIMEOUT);
    // `wal_checkpoint` returns `(busy, log_frames, checkpointed_frames)`; a
    // `busy` of `0` means every committed frame was checkpointed.
    conn.query_row("PRAGMA wal_checkpoint(TRUNCATE)", [], |row| {
        row.get::<_, i64>(0)
    })
    .map(|busy| busy == 0)
    .unwrap_or(false)
}

/// Quarantine `path` by renaming it to a sibling `<name>.quarantine-<ts>` file,
/// preserving its contents for manual recovery. Returns `true` on success.
fn quarantine_file(path: &Path) -> bool {
    if !path.exists() {
        return false;
    }
    let ts = Utc::now().format("%Y%m%dT%H%M%SZ").to_string();
    let dst = with_name_suffix(path, &format!(".quarantine-{ts}"));
    std::fs::rename(path, &dst).is_ok()
}

/// Append `suffix` to the *file name* of `path` (so `chunks.db` + `-wal`
/// = `chunks.db-wal`). SQLite names its side-files this way.
fn with_name_suffix(path: &Path, suffix: &str) -> PathBuf {
    let mut p = path.to_path_buf();
    let name = p
        .file_name()
        .unwrap_or_default()
        .to_string_lossy()
        .into_owned();
    p.set_file_name(format!("{name}{suffix}"));
    p
}

/// Run `PRAGMA quick_check(1)` against `db_path` on a fresh, short-lived
/// connection. `Ok(true)` when the structural scan reports `"ok"`.
///
/// `quick_check` is a fast structural scan (not the exhaustive
/// `PRAGMA integrity_check`) — sufficient to confirm the file is no longer
/// corrupt, but it does not verify every b-tree page or cross-reference.
/// Opens a brand-new, uncached `Connection` (deliberately bypassing the
/// process connection cache) so this can safely run before/independent of
/// whatever state that cache is in.
///
/// # Errors
/// Returns `Err` if the file cannot be opened or the pragma query fails
/// (e.g. the file is missing, unreadable, or so corrupt the pragma itself
/// errors rather than reporting a structural problem).
fn quick_check_ok(db_path: &Path) -> Result<bool> {
    let conn = Connection::open(db_path)
        .with_context(|| format!("open for quick_check: {}", db_path.display()))?;
    let _ = conn.busy_timeout(SQLITE_BUSY_TIMEOUT);
    let result: String = conn
        .query_row("PRAGMA quick_check(1)", [], |row| row.get(0))
        .context("running PRAGMA quick_check")?;
    Ok(result.eq_ignore_ascii_case("ok"))
}

/// Recover from a `SQLITE_CORRUPT` (malformed image) on the chunk DB.
///
/// Quarantines the damaged file (and its WAL/SHM side-files) to a timestamped
/// `.corrupt-<ts>` copy — preserved, not deleted — then rebuilds an empty
/// schema so the store resumes instead of wedging indefinitely.
///
/// Returns `Ok(true)` when a quarantine + rebuild happened, `Ok(false)` when a
/// fresh `PRAGMA quick_check` now passes (the earlier failure was transient),
/// and `Err` when the quarantine rename or the schema rebuild failed.
///
/// # NOTE — not wired into any production error path (audit finding SC-5)
/// `#[allow(dead_code)]` here is not incidental: nothing in `connection.rs`
/// currently calls this on a `SQLITE_CORRUPT` failure, so a real corruption
/// event wedges `get_or_init_connection` (via the circuit breaker) rather
/// than triggering this recovery. Wiring it in requires care beyond adding a
/// call site: the two-step "drop cached connection, then rename" sequence
/// below does not hold any lock across the gap, so a concurrent
/// `with_connection` call for the same path can reopen and re-cache the
/// about-to-be-quarantined file between step 1 and step 3 — its writes then
/// land in the file this function is about to rename out from under it, and
/// step 4's fresh `get_or_init_connection` call returns that same stale
/// cached `Arc` instead of a connection to the newly rebuilt schema. A safe
/// wiring needs the per-path init lock held across the whole
/// quarantine-and-rebuild sequence.
///
/// # Errors
/// Returns `Err` if quarantining any of the main/`-wal`/`-shm` files fails
/// (e.g. permissions, or a lingering file handle keeping the rename from
/// succeeding on platforms with mandatory file locking), or if rebuilding the
/// schema via [`get_or_init_connection`] fails.
#[allow(dead_code)]
pub(crate) fn recover_corrupt_db(config: &MemoryConfig) -> Result<bool> {
    let db_path = db_path_for(config);

    // 1. Drop any cached (corrupt) connection + breaker so the OS file handle
    //    is closed before we rename.
    drop_cached_connection(config);

    // 2. Re-confirm corruption against the on-disk file. If `quick_check` now
    //    passes, the image is actually healthy — don't destroy good data.
    if db_path.exists() && matches!(quick_check_ok(&db_path), Ok(true)) {
        return Ok(false);
    }

    // 3. Quarantine the main DB + WAL/SHM side-files to `<name>.corrupt-<ts>`.
    let ts = Utc::now().format("%Y%m%dT%H%M%SZ").to_string();
    for suffix in &["", "-wal", "-shm"] {
        let src = with_name_suffix(&db_path, suffix);
        if !src.exists() {
            continue;
        }
        let dst = with_name_suffix(&src, &format!(".corrupt-{ts}"));
        std::fs::rename(&src, &dst).with_context(|| {
            format!(
                "failed to quarantine corrupt chunk DB file {} -> {}",
                src.display(),
                dst.display()
            )
        })?;
    }

    // 4. Rebuild an empty schema by forcing a fresh open.
    get_or_init_connection(config)
        .context("failed to rebuild chunk DB schema after quarantining corrupt DB")?;

    Ok(true)
}

#[cfg(test)]
#[path = "recovery_tests.rs"]
mod tests;
