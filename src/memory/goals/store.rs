//! Persistence for the long-term goals list.
//!
//! The goals list lives in a single compact markdown file, `MEMORY_GOALS.md`,
//! in the memory workspace root ([`MemoryConfig::workspace`]). The file is
//! intentionally tiny — capped at ~500 tokens — so it stays cheap to read and
//! easy for a human to edit.
//!
//! All mutations go through `load → mutate → save`, which re-enforces the size
//! and item-count caps on every write. Trimming drops the *oldest* items first
//! (front of the list); it never silently corrupts the file. Every mutation is
//! serialised through a process-wide [`parking_lot::Mutex`] so concurrent
//! callers (user edits via RPC/tools and background reflection) can't clobber
//! each other's load→save sequences.
//!
//! Path validation rejects symlink escapes: a `MEMORY_GOALS.md` symlinked to a
//! target outside the workspace is refused so a hostile link can't read or
//! write outside the sandbox boundary.

use std::path::{Path, PathBuf};
use std::sync::OnceLock;

use parking_lot::Mutex;

use crate::memory::config::MemoryConfig;
use crate::memory::error::{MemoryEngineResult, MemoryError};

use super::types::GoalsDoc;

/// File name of the goals document inside the workspace.
pub const GOALS_FILE: &str = "MEMORY_GOALS.md";

/// Hard ceiling on the rendered file size. ~2000 chars ≈ ~500 tokens, which
/// keeps the document inside the "200–500 token" budget from the spec.
pub const GOALS_FILE_MAX_CHARS: usize = 2000;

/// Maximum number of goal items. A long-term goals list should be short and
/// focused; beyond this we drop the oldest entries.
pub const GOALS_MAX_ITEMS: usize = 8;

/// Serialises `load → mutate → save` sequences across all callers.
fn goals_mutation_lock() -> &'static Mutex<()> {
    static LOCK: OnceLock<Mutex<()>> = OnceLock::new();
    LOCK.get_or_init(|| Mutex::new(()))
}

/// Absolute path to `MEMORY_GOALS.md` within `workspace`.
pub fn goals_path(workspace: &Path) -> PathBuf {
    workspace.join(GOALS_FILE)
}

/// Verify that the resolved goals path stays inside `workspace`, defending
/// against symlink-based escapes. Returns the validated (non-canonical) path
/// to write/read.
fn validate_within_workspace(workspace: &Path) -> MemoryEngineResult<PathBuf> {
    let path = goals_path(workspace);

    // Canonicalize the parent (the workspace) — the file itself may not exist
    // yet on first write. If the workspace dir doesn't resolve, fall back to
    // the literal path so a fresh tempdir still works.
    let workspace_canon = workspace
        .canonicalize()
        .unwrap_or_else(|_| workspace.to_path_buf());
    let parent = path.parent().unwrap_or(workspace);
    let parent_canon = parent
        .canonicalize()
        .unwrap_or_else(|_| parent.to_path_buf());
    if !parent_canon.starts_with(&workspace_canon) {
        return Err(MemoryError::PathEscape(format!(
            "goals path resolves outside workspace: {path:?}"
        )));
    }

    // If the file already exists as a symlink, ensure its target also stays
    // inside the workspace — a symlinked MEMORY_GOALS.md could otherwise
    // read/write outside the boundary even with a valid parent dir.
    if let Ok(meta) = std::fs::symlink_metadata(&path) {
        if meta.file_type().is_symlink() {
            let resolved = path.canonicalize().map_err(|e| {
                MemoryError::PathEscape(format!("failed to resolve goals symlink {path:?}: {e}"))
            })?;
            if !resolved.starts_with(&workspace_canon) {
                return Err(MemoryError::PathEscape(format!(
                    "goals symlink resolves outside workspace: {resolved:?}"
                )));
            }
        }
    }
    Ok(path)
}

/// Load the goals document from disk. Returns an empty document when the file
/// does not exist yet (first run).
pub fn load(workspace: &Path) -> MemoryEngineResult<GoalsDoc> {
    let path = validate_within_workspace(workspace)?;
    match std::fs::read_to_string(&path) {
        Ok(body) => Ok(GoalsDoc::parse(&body)),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(GoalsDoc::default()),
        Err(e) => Err(MemoryError::Io(e)),
    }
}

/// Enforce the item-count and byte-size caps on `doc`, dropping the oldest
/// items as needed. Returns the list of dropped item ids.
fn enforce_caps(doc: &mut GoalsDoc) -> Vec<String> {
    let mut dropped = Vec::new();
    // 1. Item-count cap.
    while doc.items.len() > GOALS_MAX_ITEMS {
        let removed = doc.items.remove(0);
        dropped.push(removed.id);
    }
    // 2. Byte-size cap — keep removing the oldest until the rendered file fits.
    //    Always leave at least one item if any remain so the file isn't
    //    pointlessly emptied by a single oversized entry.
    while doc.render().len() > GOALS_FILE_MAX_CHARS && doc.items.len() > 1 {
        let removed = doc.items.remove(0);
        dropped.push(removed.id);
    }
    dropped
}

/// Persist `doc` to disk, enforcing caps first. The `doc` is mutated in place
/// to reflect any cap trimming so the caller's view matches disk.
pub fn save(workspace: &Path, doc: &mut GoalsDoc) -> MemoryEngineResult<()> {
    let path = validate_within_workspace(workspace)?;
    let _dropped = enforce_caps(doc);

    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    std::fs::write(&path, doc.render())?;
    Ok(())
}

/// Add a goal, persist, and return `(new_id, updated_doc)`.
pub fn add(workspace: &Path, text: &str) -> MemoryEngineResult<(String, GoalsDoc)> {
    let _guard = goals_mutation_lock().lock();
    let mut doc = load(workspace)?;
    let id = doc.add(text)?;
    save(workspace, &mut doc)?;
    Ok((id, doc))
}

/// Edit a goal's text, persist, and return the updated document.
pub fn edit(workspace: &Path, id: &str, text: &str) -> MemoryEngineResult<GoalsDoc> {
    let _guard = goals_mutation_lock().lock();
    let mut doc = load(workspace)?;
    doc.edit(id, text)?;
    save(workspace, &mut doc)?;
    Ok(doc)
}

/// Delete a goal, persist, and return the updated document.
pub fn delete(workspace: &Path, id: &str) -> MemoryEngineResult<GoalsDoc> {
    let _guard = goals_mutation_lock().lock();
    let mut doc = load(workspace)?;
    doc.delete(id)?;
    save(workspace, &mut doc)?;
    Ok(doc)
}

// ── `MemoryConfig`-rooted convenience wrappers ───────────────────────────────
//
// These mirror the path-based primitives above but read the workspace root
// from `MemoryConfig`, which is how hosts drive the engine.

/// List the current goals for the engine rooted at `config`.
pub fn list_for(config: &MemoryConfig) -> MemoryEngineResult<GoalsDoc> {
    load(&config.workspace)
}

/// Add a goal for the engine rooted at `config`.
pub fn add_for(config: &MemoryConfig, text: &str) -> MemoryEngineResult<(String, GoalsDoc)> {
    add(&config.workspace, text)
}

/// Edit a goal for the engine rooted at `config`.
pub fn edit_for(config: &MemoryConfig, id: &str, text: &str) -> MemoryEngineResult<GoalsDoc> {
    edit(&config.workspace, id, text)
}

/// Delete a goal for the engine rooted at `config`.
pub fn delete_for(config: &MemoryConfig, id: &str) -> MemoryEngineResult<GoalsDoc> {
    delete(&config.workspace, id)
}

#[cfg(test)]
#[path = "store_tests.rs"]
mod tests;
