//! Domain types for the agent's long-term goals list.
//!
//! Goals are a small, ordered list of durable objectives the agent holds when
//! interacting with the user. They are persisted as a compact markdown document
//! (`MEMORY_GOALS.md`) — see [`super::store`] — and surfaced over RPC + agent
//! tools. Each item carries a stable short id so edit/delete operations can
//! address a specific line without depending on ordering.
//!
//! This module is pure: it owns parse/render and the in-memory mutation logic
//! (`add` / `edit` / `delete`) plus their validation rules. The cap-enforcing
//! persistence layer lives in [`super::store`]; the reflection apply/dedupe
//! logic lives in [`super::reflect()`].

use serde::{Deserialize, Serialize};

use crate::memory::error::MemoryError;
use crate::memory::store::safety::{
    has_likely_email, has_likely_pii, has_likely_secret, sanitize_text,
};

fn has_goal_pii(text: &str) -> bool {
    has_likely_email(text) || has_likely_pii(text) || sanitize_text(text).report.pii_redactions > 0
}

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

    /// Validate that `text` is a non-empty, single-line goal body. A
    /// newline-bearing goal would inject extra `- [..]` list lines on reload,
    /// corrupting the stored shape — so it is rejected outright.
    fn validate_text(text: &str) -> Result<&str, MemoryError> {
        let text = text.trim();
        if text.is_empty() {
            return Err(MemoryError::Invalid(
                "goal text must not be empty".to_string(),
            ));
        }
        if text.contains('\n') || text.contains('\r') {
            return Err(MemoryError::Invalid(
                "goal text must be a single line".to_string(),
            ));
        }
        if has_likely_secret(text) || has_goal_pii(text) {
            return Err(MemoryError::Invalid(
                "goal text must not contain secrets or PII".to_string(),
            ));
        }
        Ok(text)
    }

    /// Append a new goal, returning the assigned id. Text is trimmed; empty or
    /// multi-line text is rejected.
    pub fn add(&mut self, text: &str) -> Result<String, MemoryError> {
        let text = Self::validate_text(text)?;
        let id = self.next_id();
        self.items.push(GoalItem::new(&id, text));
        Ok(id)
    }

    /// Replace the text of the goal with `id`. Returns an error if the id is
    /// unknown or the new text is empty/multi-line.
    pub fn edit(&mut self, id: &str, text: &str) -> Result<(), MemoryError> {
        let text = Self::validate_text(text)?;
        let item = self
            .items
            .iter_mut()
            .find(|i| i.id == id)
            .ok_or_else(|| MemoryError::NotFound(format!("no goal with id '{id}'")))?;
        item.text = text.to_string();
        Ok(())
    }

    /// Delete the goal with `id`. Returns an error if the id is unknown.
    pub fn delete(&mut self, id: &str) -> Result<(), MemoryError> {
        let before = self.items.len();
        self.items.retain(|i| i.id != id);
        if self.items.len() == before {
            return Err(MemoryError::NotFound(format!("no goal with id '{id}'")));
        }
        Ok(())
    }
}

#[cfg(test)]
#[path = "types_tests.rs"]
mod tests;
