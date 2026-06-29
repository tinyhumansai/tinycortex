//! Disk-backed entity store.
//!
//! Read/write of entity markdown files under
//! `<content_root>/entities/<kind>/<canonical_id>.md`. Upserts rewrite only the
//! YAML front matter and preserve any user-edited notes body, so the vault can
//! be hand-edited (in Obsidian, an editor, or by another tool) without losing
//! those edits on the next programmatic write.

use std::fs;
use std::path::PathBuf;

use anyhow::{Context, Result};
use chrono::Utc;

use crate::memory::config::MemoryConfig;
use crate::memory::entities::canonical::slugify_id;
use crate::memory::entities::frontmatter::{compose, extract_notes, parse};
use crate::memory::entities::types::{Entity, EntityKind};

/// Directory under the content root that holds entity files.
const ENTITIES_DIR: &str = "entities";

/// Resolve the content root from the workspace.
///
/// Mirrors OpenHuman's `memory_tree` content layout: markdown content lives
/// under `<workspace>/memory_tree/content`. The entity registry is rooted at
/// `<content_root>/entities`.
fn content_root(config: &MemoryConfig) -> PathBuf {
    config.workspace.join("memory_tree").join("content")
}

/// Directory holding every file of one kind: `<content_root>/entities/<kind>`.
fn kind_dir(config: &MemoryConfig, kind: EntityKind) -> PathBuf {
    content_root(config).join(ENTITIES_DIR).join(kind.as_str())
}

/// Full path to one entity file.
fn entity_path(config: &MemoryConfig, kind: EntityKind, canonical_id: &str) -> PathBuf {
    kind_dir(config, kind).join(format!("{}.md", slugify_id(canonical_id)))
}

/// Upsert an entity.
///
/// Preserves any user-edited notes body that already exists on disk; only the
/// YAML front matter is rewritten. Returns the stored entity with `updated_at`
/// refreshed to the write time.
pub fn put_entity(config: &MemoryConfig, mut entity: Entity) -> Result<Entity> {
    let dir = kind_dir(config, entity.kind);
    fs::create_dir_all(&dir).with_context(|| format!("failed to mkdir -p {}", dir.display()))?;
    let path = entity_path(config, entity.kind, &entity.id);

    // Preserve any free-form notes the user typed into the file.
    let existing_notes = match fs::read_to_string(&path) {
        Ok(text) => extract_notes(&text),
        Err(_) => String::new(),
    };

    entity.updated_at = Utc::now();
    let bytes = compose(&entity, &existing_notes).into_bytes();
    fs::write(&path, &bytes)
        .with_context(|| format!("failed to write entity {}", path.display()))?;
    Ok(entity)
}

/// Read an entity by canonical id. Returns `Ok(None)` when the file is absent.
pub fn get_entity(
    config: &MemoryConfig,
    kind: EntityKind,
    canonical_id: &str,
) -> Result<Option<Entity>> {
    let path = entity_path(config, kind, canonical_id);
    if !path.exists() {
        return Ok(None);
    }
    let text =
        fs::read_to_string(&path).with_context(|| format!("failed to read {}", path.display()))?;
    Ok(parse(&text))
}

/// List every stored entity of a given kind.
///
/// Order is filesystem-dependent — callers that need a sort impose their own.
pub fn list_entities(config: &MemoryConfig, kind: EntityKind) -> Result<Vec<Entity>> {
    let dir = kind_dir(config, kind);
    if !dir.exists() {
        return Ok(Vec::new());
    }
    let mut out = Vec::new();
    for entry in
        fs::read_dir(&dir).with_context(|| format!("failed to read_dir {}", dir.display()))?
    {
        let entry = entry?;
        let name = entry.file_name();
        let s = name.to_string_lossy();
        if !s.ends_with(".md") {
            continue;
        }
        let text = fs::read_to_string(entry.path())
            .with_context(|| format!("failed to read {}", entry.path().display()))?;
        if let Some(e) = parse(&text) {
            out.push(e);
        }
    }
    Ok(out)
}

/// Find an entity of `kind` whose `aliases`, `emails`, `handles[*].value`, or
/// `display_name` matches `needle` (case-insensitive).
///
/// Returns the first match in walk order, or `None`. A linear scan — for a
/// single-user workspace with thousands (not millions) of entities this is
/// fine and avoids maintaining a separate index.
pub fn lookup_alias(
    config: &MemoryConfig,
    kind: EntityKind,
    needle: &str,
) -> Result<Option<Entity>> {
    let lower = needle.to_lowercase();
    for e in list_entities(config, kind)? {
        if e.aliases.iter().any(|a| a.to_lowercase() == lower) {
            return Ok(Some(e));
        }
        if e.emails.iter().any(|m| m.to_lowercase() == lower) {
            return Ok(Some(e));
        }
        if e.handles.iter().any(|h| h.value.to_lowercase() == lower) {
            return Ok(Some(e));
        }
        if e.display_name
            .as_deref()
            .map(|n| n.to_lowercase() == lower)
            .unwrap_or(false)
        {
            return Ok(Some(e));
        }
    }
    Ok(None)
}

#[cfg(test)]
#[path = "store_tests.rs"]
mod tests;
