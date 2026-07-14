//! The configured-source registry.
//!
//! Sources are persisted as `[[memory_sources]]` entries in a TOML config file
//! (typically `config.toml`). In OpenHuman this lived on a large shared `Config`
//! struct loaded through an async RPC; TinyCortex does not own that global
//! config, so the registry here is a small self-contained reader/writer over a
//! single TOML file. Other top-level keys in the file are preserved across
//! writes — only the `memory_sources` array is rewritten.
//!
//! Every mutation follows the spec's atomic load-modify-validate-save cycle:
//! load the current file, apply the change in memory, validate, and persist.
//! Each on-disk write (`SourceRegistry::atomic_write`) is atomic (temp file +
//! rename), so a crash mid-write cannot leave a truncated `config.toml`.
//!
//! The complete load-modify-save cycle is guarded by a process-wide mutation
//! lock, so separate [`SourceRegistry`] handles cannot overwrite one another's
//! in-process updates. Atomic rename protects each individual disk write.

use std::path::{Path, PathBuf};
use std::sync::{LazyLock, Mutex};

use anyhow::{anyhow, bail, Context, Result};

use super::types::{MemorySourceEntry, MemorySourcePatch, SourceKind};

/// Serializes each registry load-modify-save transaction in this process.
///
/// A single lock deliberately covers every path: registry mutation is rare,
/// and correctness is more important than allowing unrelated config files to
/// race through their atomic renames. The on-disk rename remains the crash-
/// safety boundary; this mutex closes the in-process lost-update window.
static REGISTRY_MUTATION_LOCK: LazyLock<Mutex<()>> = LazyLock::new(|| Mutex::new(()));

fn mutation_guard() -> std::sync::MutexGuard<'static, ()> {
    REGISTRY_MUTATION_LOCK
        .lock()
        .expect("source registry mutation lock poisoned")
}

/// Conservative default sync caps for a Composio toolkit, keyed by toolkit slug.
///
/// Single source of truth for the cheap out-of-the-box sync volume. Applied to a
/// source entry when it is first registered. Never overwrites a user-customised
/// cap. Returns `(max_items, sync_depth_days)`.
pub fn memory_sync_defaults_for_toolkit(toolkit: &str) -> (Option<u32>, Option<u32>) {
    match toolkit {
        "gmail" => (Some(100), Some(30)),
        "slack" => (Some(50), Some(14)),
        "notion" => (Some(30), Some(30)),
        "linear" => (Some(50), Some(30)),
        "clickup" => (Some(50), Some(30)),
        "github" => (Some(50), Some(30)),
        // Generic fallback for any toolkit not listed above.
        _ => (Some(30), Some(14)),
    }
}

/// A registry of [`MemorySourceEntry`] values backed by a TOML config file.
///
/// Construct one with [`SourceRegistry::new`], pointing at the `config.toml`
/// path. The file need not exist yet — reads return an empty list and the first
/// write creates it (and any missing parent directories).
#[derive(Debug, Clone)]
pub struct SourceRegistry {
    path: PathBuf,
}

impl SourceRegistry {
    /// Create a registry persisted at `config_path`.
    pub fn new(config_path: impl Into<PathBuf>) -> Self {
        Self {
            path: config_path.into(),
        }
    }

    /// The config file path this registry reads and writes.
    pub fn path(&self) -> &Path {
        &self.path
    }

    /// Read the whole config file into a TOML table (empty if it doesn't exist).
    fn read_table(&self) -> Result<toml::Table> {
        if !self.path.exists() {
            return Ok(toml::Table::new());
        }
        let text = std::fs::read_to_string(&self.path)
            .with_context(|| format!("failed to read {}", self.path.display()))?;
        let table: toml::Table = toml::from_str(&text)
            .with_context(|| format!("failed to parse {}", self.path.display()))?;
        Ok(table)
    }

    /// List all configured sources.
    pub fn list(&self) -> Result<Vec<MemorySourceEntry>> {
        let table = self.read_table()?;
        match table.get("memory_sources") {
            Some(value) => value
                .clone()
                .try_into()
                .context("failed to decode [[memory_sources]]"),
            None => Ok(Vec::new()),
        }
    }

    /// List enabled sources of a given [`SourceKind`].
    pub fn list_enabled_by_kind(&self, kind: SourceKind) -> Result<Vec<MemorySourceEntry>> {
        Ok(self
            .list()?
            .into_iter()
            .filter(|s| s.kind == kind && s.enabled)
            .collect())
    }

    /// Get a single source by id, if present.
    pub fn get(&self, id: &str) -> Result<Option<MemorySourceEntry>> {
        Ok(self.list()?.into_iter().find(|s| s.id == id))
    }

    /// Persist the full source list, preserving any other top-level config
    /// keys.
    ///
    /// Writes are atomic: the new TOML is written to a same-directory temp file
    /// and then renamed over the config. This keeps a failed/crashed write from
    /// leaving a truncated `config.toml`, matching the OpenHuman source
    /// registry contract.
    ///
    /// Mutation callers hold [`REGISTRY_MUTATION_LOCK`] across their initial
    /// read and this preserving re-read, keeping the two snapshots ordered with
    /// respect to every other in-process writer.
    fn write_all(&self, entries: &[MemorySourceEntry]) -> Result<()> {
        let mut table = self.read_table()?;
        let value = toml::Value::try_from(entries).context("failed to encode memory_sources")?;
        table.insert("memory_sources".to_string(), value);
        let text = toml::to_string_pretty(&table).context("failed to serialize config")?;
        if let Some(parent) = self.path.parent() {
            if !parent.as_os_str().is_empty() {
                std::fs::create_dir_all(parent)
                    .with_context(|| format!("failed to create {}", parent.display()))?;
            }
        }
        self.atomic_write(text.as_bytes())?;
        Ok(())
    }

    fn atomic_write(&self, bytes: &[u8]) -> Result<()> {
        let parent = self
            .path
            .parent()
            .filter(|p| !p.as_os_str().is_empty())
            .unwrap_or_else(|| Path::new("."));
        let filename = self
            .path
            .file_name()
            .and_then(|n| n.to_str())
            .ok_or_else(|| anyhow!("config path has no file name: {}", self.path.display()))?;
        let tmp_path = parent.join(format!(
            ".{filename}.tmp-{}",
            uuid::Uuid::new_v4().as_simple()
        ));

        let write_result = (|| -> Result<()> {
            {
                let mut file = std::fs::File::create(&tmp_path)
                    .with_context(|| format!("failed to create {}", tmp_path.display()))?;
                use std::io::Write;
                file.write_all(bytes)
                    .with_context(|| format!("failed to write {}", tmp_path.display()))?;
                file.sync_all()
                    .with_context(|| format!("failed to sync {}", tmp_path.display()))?;
            }
            std::fs::rename(&tmp_path, &self.path).with_context(|| {
                format!(
                    "failed to atomically replace {} with {}",
                    self.path.display(),
                    tmp_path.display()
                )
            })?;
            Ok(())
        })();

        if write_result.is_err() {
            let _ = std::fs::remove_file(&tmp_path);
        }
        write_result
    }

    /// Validate and add a new source. Fails if the id already exists.
    pub fn add(&self, entry: MemorySourceEntry) -> Result<MemorySourceEntry> {
        let _guard = mutation_guard();
        entry.validate().map_err(|e| anyhow!(e))?;
        let mut sources = self.list()?;
        if sources.iter().any(|s| s.id == entry.id) {
            bail!("source with id '{}' already exists", entry.id);
        }
        sources.push(entry.clone());
        self.write_all(&sources)?;
        Ok(entry)
    }

    /// Apply a [`MemorySourcePatch`] to an existing source, then re-validate and
    /// save. Fails if no source has the given id.
    pub fn update(&self, id: &str, patch: MemorySourcePatch) -> Result<MemorySourceEntry> {
        let _guard = mutation_guard();
        let mut sources = self.list()?;
        let entry = sources
            .iter_mut()
            .find(|s| s.id == id)
            .ok_or_else(|| anyhow!("source '{id}' not found"))?;

        patch.validate_for_kind(entry.kind.clone())?;
        patch.apply_to(entry);
        entry.validate().map_err(|e| anyhow!(e))?;
        let updated = entry.clone();
        self.write_all(&sources)?;
        Ok(updated)
    }

    /// Remove a source by id. Returns `true` if an entry was removed.
    pub fn remove(&self, id: &str) -> Result<bool> {
        let _guard = mutation_guard();
        let mut sources = self.list()?;
        let before = sources.len();
        sources.retain(|s| s.id != id);
        let removed = sources.len() < before;
        if removed {
            self.write_all(&sources)?;
        }
        Ok(removed)
    }

    /// Remove every composio source bound to `connection_id`. Returns the count
    /// removed. Mirrors [`SourceRegistry::upsert_composio_source`], which keys
    /// composio sources on `connection_id` rather than the `src_*` id.
    pub fn remove_composio_source_by_connection_id(&self, connection_id: &str) -> Result<usize> {
        let _guard = mutation_guard();
        let mut sources = self.list()?;
        let before = sources.len();
        sources.retain(|s| {
            !(s.kind == SourceKind::Composio && s.connection_id.as_deref() == Some(connection_id))
        });
        let removed = before - sources.len();
        if removed > 0 {
            self.write_all(&sources)?;
        }
        Ok(removed)
    }

    /// Upsert a composio source keyed on `connection_id`.
    ///
    /// If a source with the same `connection_id` exists, its label is updated;
    /// otherwise a new entry is inserted with conservative per-toolkit caps. The
    /// update path never clobbers user-customised caps.
    pub fn upsert_composio_source(
        &self,
        toolkit: &str,
        connection_id: &str,
        label: &str,
    ) -> Result<MemorySourceEntry> {
        let _guard = mutation_guard();
        let mut sources = self.list()?;
        let (entry, _was_insert) =
            upsert_composio_entry_in_place(&mut sources, toolkit, connection_id, label);
        self.write_all(&sources)?;
        Ok(entry)
    }

    /// Batch-upsert Composio sources with one load and one atomic save.
    pub fn upsert_composio_sources_batch(&self, targets: &[ComposioUpsertTarget]) -> Result<u32> {
        if targets.is_empty() {
            return Ok(0);
        }
        let _guard = mutation_guard();
        let mut sources = self.list()?;
        for (toolkit, connection_id, label) in targets {
            upsert_composio_entry_in_place(&mut sources, toolkit, connection_id, label);
        }
        self.write_all(&sources)?;
        Ok(targets.len().min(u32::MAX as usize) as u32)
    }

    /// Enable every source and clear all per-source caps ("All In" mode).
    pub fn apply_all_in(&self) -> Result<Vec<MemorySourceEntry>> {
        let _guard = mutation_guard();
        let mut sources = self.list()?;
        for source in &mut sources {
            source.enabled = true;
            source.max_items = None;
            source.since_days = None;
            source.sync_depth_days = None;
            source.max_commits = None;
            source.max_issues = None;
            source.max_prs = None;
            source.max_tokens_per_sync = None;
            source.max_cost_per_sync_usd = None;
        }
        self.write_all(&sources)?;
        Ok(sources)
    }
}

pub type ComposioUpsertTarget = (String, String, String);

/// Apply a single composio upsert to an in-memory source list.
///
/// Pure (no I/O) so the registry path and unit tests share one find-or-push
/// predicate. Returns the resulting entry and whether it was a fresh insert.
pub(crate) fn upsert_composio_entry_in_place(
    sources: &mut Vec<MemorySourceEntry>,
    toolkit: &str,
    connection_id: &str,
    label: &str,
) -> (MemorySourceEntry, bool) {
    if let Some(existing) = sources.iter_mut().find(|s| {
        s.kind == SourceKind::Composio && s.connection_id.as_deref() == Some(connection_id)
    }) {
        existing.label = label.to_string();
        return (existing.clone(), false);
    }

    let (default_max_items, default_sync_depth_days) = memory_sync_defaults_for_toolkit(toolkit);
    let entry = MemorySourceEntry {
        id: format!("src_{}", uuid::Uuid::new_v4().as_simple()),
        kind: SourceKind::Composio,
        label: label.to_string(),
        enabled: true,
        toolkit: Some(toolkit.to_string()),
        connection_id: Some(connection_id.to_string()),
        path: None,
        glob: None,
        url: None,
        branch: None,
        paths: Vec::new(),
        max_commits: None,
        max_issues: None,
        max_prs: None,
        query: None,
        since_days: None,
        max_items: default_max_items,
        selector: None,
        max_tokens_per_sync: None,
        max_cost_per_sync_usd: None,
        sync_depth_days: default_sync_depth_days,
    };
    sources.push(entry.clone());
    (entry, true)
}

#[cfg(test)]
#[path = "registry_tests.rs"]
mod tests;
