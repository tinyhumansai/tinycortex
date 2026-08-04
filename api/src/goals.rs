//! Domain types for the agent's long-term goals list.
//!
//! Goals are a small, ordered list of durable objectives the agent holds when
//! interacting with the user. They are persisted as a compact markdown document
//! (`MEMORY_GOALS.md`) by the engine crate's `memory::goals::store` and
//! surfaced over RPC + agent tools. Each item carries a stable short id so
//! edit/delete operations can address a specific line without depending on
//! ordering.
//!
//! This module is **pure data**: it owns the shape, parse, and render only.
//! The validating mutation surface (`add` / `edit` / `delete`) lives next to
//! the `regex`-backed PII/secret predicates it calls, in the engine crate's
//! `memory::goals::store::GoalsDocMutations` trait, so the value types stay
//! free of the safety machinery and of `regex`. The cap-enforcing persistence
//! layer and the reflection apply/dedupe logic live in the engine crate too.

use serde::{Deserialize, Serialize};

/// Markdown header rendered at the top of `MEMORY_GOALS.md`.
pub(crate) const HEADER: &str = "# Long-term Goals";

/// A single long-term goal item.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct GoalItem {
    /// Stable short id (e.g. `g1`). Used as the dedupe/address key for
    /// `edit`/`delete`. Rendered inline in the markdown as `- [g1] …`.
    pub id: String,
    /// The goal text — one concise sentence.
    pub text: String,
}

impl GoalItem {
    /// Construct a goal item from an id + text, trimming surrounding
    /// whitespace from the text.
    pub fn new(id: impl Into<String>, text: impl Into<String>) -> Self {
        Self {
            id: id.into(),
            text: text.into().trim().to_string(),
        }
    }
}

/// The full goals document — an ordered list of [`GoalItem`]s.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct GoalsDoc {
    /// Ordered goal items. Order is meaningful for rendering and cap trimming
    /// (oldest = front).
    pub items: Vec<GoalItem>,
}

impl GoalsDoc {
    /// Parse a `MEMORY_GOALS.md` body into a [`GoalsDoc`].
    ///
    /// Recognised item lines look like `- [g1] do the thing`. Lines that don't
    /// match (the header, blank lines, free prose) are ignored so a
    /// hand-edited file degrades gracefully rather than erroring.
    pub fn parse(body: &str) -> Self {
        let mut items = Vec::new();
        for line in body.lines() {
            let trimmed = line.trim();
            // Strip the leading list marker, if present.
            let rest = match trimmed.strip_prefix("- ") {
                Some(r) => r.trim(),
                None => continue,
            };
            // Expect `[id] text`.
            let Some(after_open) = rest.strip_prefix('[') else {
                continue;
            };
            let Some(close_idx) = after_open.find(']') else {
                continue;
            };
            let id = after_open[..close_idx].trim();
            let text = after_open[close_idx + 1..].trim();
            if id.is_empty() || text.is_empty() {
                continue;
            }
            items.push(GoalItem::new(id, text));
        }
        Self { items }
    }

    /// Render the document back to markdown suitable for `MEMORY_GOALS.md`.
    ///
    /// NOTE: this emits only the header and the recognised `- [id] text`
    /// item lines — any free prose, sub-bullets, or other hand-added content
    /// a user wrote into the file is not represented in [`GoalsDoc`] and is
    /// therefore dropped on the next `parse` → mutate → `render` round-trip
    /// (e.g. via `add`/`edit`/`delete`/reflection). Treat this file as
    /// machine-owned rather than freely hand-editable.
    pub fn render(&self) -> String {
        let mut out = String::from(HEADER);
        out.push_str("\n\n");
        for item in &self.items {
            out.push_str(&format!("- [{}] {}\n", item.id, item.text));
        }
        out
    }

    /// Whether the list currently has no items. Used to drive the
    /// "first run / initial population" reflection behaviour.
    pub fn is_empty(&self) -> bool {
        self.items.is_empty()
    }

    /// Number of goal items currently held.
    pub fn len(&self) -> usize {
        self.items.len()
    }

    /// Allocate the next free `g<N>` id not already used in the list.
    pub fn next_id(&self) -> String {
        let mut n = self.items.len() + 1;
        loop {
            let candidate = format!("g{n}");
            if !self.items.iter().any(|i| i.id == candidate) {
                return candidate;
            }
            n += 1;
        }
    }

    /// Whether the list already holds `id`.
    pub fn contains_id(&self, id: &str) -> bool {
        self.items.iter().any(|i| i.id == id)
    }
}

#[cfg(test)]
#[path = "goals_tests.rs"]
mod tests;
