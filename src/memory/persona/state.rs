//! Incremental-run state for the persona pipeline (doc 06 §6.7).
//!
//! Mirrors the `sync::state::SyncStateStore` pattern with a persona-local
//! [`PersonaStateStore`] trait (the `sync` module is gated behind the heavy
//! `sync`/reqwest feature, and persona stays dependency-light). Cursors are
//! deliberately coarse and cheap:
//!
//! - JSONL / transcript sources: `(mtime_ms, len)` per file — append-only in
//!   practice, so an unchanged `(mtime, len)` skips the file.
//! - Git: last-distilled HEAD sha per repo (+ the author-set hash, so changing
//!   the email list forces a re-scan).
//! - Instruction files: content sha per path — re-digest only on change.
//!
//! A concrete [`FileStateStore`] persists the map as one JSON file under the
//! workspace so cursors survive across process runs; tests use an in-memory
//! store. Evidence ids are content-addressed (§6.3), so overlapping cursors are
//! harmless — this state is a *fast-skip* optimisation, not a correctness gate.

use std::collections::HashMap;
use std::path::{Path, PathBuf};

use anyhow::{Context, Result};
use async_trait::async_trait;
use parking_lot::Mutex;
use serde::{Deserialize, Serialize};

/// Persona-scoped state namespace.
pub const NAMESPACE: &str = "persona-sync-state";

/// A minimal key/value state store, scoped by namespace. Mirrors
/// `sync::state::SyncStateStore` so the same persistence pattern is reused
/// without depending on the feature-gated `sync` module.
#[async_trait]
pub trait PersonaStateStore: Send + Sync {
    /// Fetch a stored value, if any.
    async fn get(&self, namespace: &str, key: &str) -> Result<Option<serde_json::Value>>;
    /// Store a value, overwriting any prior value.
    async fn set(&self, namespace: &str, key: &str, value: &serde_json::Value) -> Result<()>;
}

/// A file's fast-skip cursor.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct FileCursor {
    /// File modification time in milliseconds since the epoch.
    pub mtime_ms: i64,
    /// File length in bytes.
    pub len: u64,
}

impl FileCursor {
    /// Read the current cursor for `path`.
    pub fn of(path: &Path) -> Option<Self> {
        let meta = std::fs::metadata(path).ok()?;
        let mtime_ms = meta
            .modified()
            .ok()
            .and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok())
            .map(|d| d.as_millis() as i64)?;
        Some(Self {
            mtime_ms,
            len: meta.len(),
        })
    }
}

/// Cursor key for a transcript/instruction file of a given source kind.
pub fn file_key(source_kind: &str, path: &Path) -> String {
    format!("{source_kind}:{}", path.display())
}

/// Cursor key for a git repo.
pub fn git_key(repo: &Path) -> String {
    format!("git_history:{}", repo.display())
}

/// True when `path` is unchanged since its recorded cursor (skip it).
pub async fn file_unchanged(store: &dyn PersonaStateStore, key: &str, path: &Path) -> Result<bool> {
    let current = match FileCursor::of(path) {
        Some(c) => c,
        None => return Ok(false),
    };
    match store.get(NAMESPACE, key).await? {
        Some(v) => {
            let stored: FileCursor = match serde_json::from_value(v) {
                Ok(c) => c,
                Err(_) => return Ok(false),
            };
            Ok(stored == current)
        }
        None => Ok(false),
    }
}

/// Record `path`'s current cursor under `key`.
pub async fn record_file(store: &dyn PersonaStateStore, key: &str, path: &Path) -> Result<()> {
    if let Some(cursor) = FileCursor::of(path) {
        store
            .set(NAMESPACE, key, &serde_json::to_value(cursor)?)
            .await?;
    }
    Ok(())
}

/// True when the recorded string watermark under `key` equals `value` (skip).
pub async fn watermark_unchanged(
    store: &dyn PersonaStateStore,
    key: &str,
    value: &str,
) -> Result<bool> {
    match store.get(NAMESPACE, key).await? {
        Some(serde_json::Value::String(s)) => Ok(s == value),
        _ => Ok(false),
    }
}

/// Record a string watermark (git sha, content sha) under `key`.
pub async fn record_watermark(store: &dyn PersonaStateStore, key: &str, value: &str) -> Result<()> {
    store
        .set(
            NAMESPACE,
            key,
            &serde_json::Value::String(value.to_string()),
        )
        .await
}

/// JSON-file-backed [`PersonaStateStore`]. Persists the whole map atomically on
/// each `set` so cursors survive across runs.
pub struct FileStateStore {
    path: PathBuf,
    data: Mutex<HashMap<String, serde_json::Value>>,
}

impl FileStateStore {
    /// Open (or create) the state file under a workspace's persona directory.
    pub fn open_in_workspace(workspace: &Path) -> Result<Self> {
        let path = workspace.join("persona").join("sync-state.json");
        let data = if path.exists() {
            let bytes = std::fs::read(&path)
                .with_context(|| format!("read persona state {}", path.display()))?;
            serde_json::from_slice(&bytes).unwrap_or_default()
        } else {
            HashMap::new()
        };
        Ok(Self {
            path,
            data: Mutex::new(data),
        })
    }

    fn composite(namespace: &str, key: &str) -> String {
        format!("{namespace}:{key}")
    }

    fn persist(&self, snapshot: &HashMap<String, serde_json::Value>) -> Result<()> {
        if let Some(parent) = self.path.parent() {
            std::fs::create_dir_all(parent)
                .with_context(|| format!("create persona dir {}", parent.display()))?;
        }
        let bytes = serde_json::to_vec_pretty(snapshot)?;
        crate::memory::fsutil::atomic_write(&self.path, &bytes)
            .with_context(|| format!("write persona state {}", self.path.display()))?;
        Ok(())
    }
}

#[async_trait]
impl PersonaStateStore for FileStateStore {
    async fn get(&self, namespace: &str, key: &str) -> Result<Option<serde_json::Value>> {
        Ok(self
            .data
            .lock()
            .get(&Self::composite(namespace, key))
            .cloned())
    }

    async fn set(&self, namespace: &str, key: &str, value: &serde_json::Value) -> Result<()> {
        let snapshot = {
            let mut guard = self.data.lock();
            guard.insert(Self::composite(namespace, key), value.clone());
            guard.clone()
        };
        self.persist(&snapshot)
    }
}

#[cfg(test)]
#[path = "state_tests.rs"]
mod tests;
