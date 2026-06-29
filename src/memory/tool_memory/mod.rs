//! Tool-scoped memory layer for durable learnings and high-priority rules.
//!
//! A first-class storage and retrieval surface for **actionable**
//! tool-specific guidance, distinct from per-tool effectiveness statistics
//! and from the generic `global` / `skill-*` namespaces. Ported from
//! OpenHuman's `memory_tools` module.
//!
//! ## Namespace convention
//!
//! Each tool gets its own namespace `tool-{tool_name}`. The prefix is
//! distinct from `global`, `skill-{id}`, and `tool_effectiveness` so
//! list/clear operations can reason about it without ambiguity. Build the
//! namespace string via [`tool_memory_namespace`] — never hard-code the
//! format.
//!
//! ## Persistence abstraction
//!
//! [`ToolMemoryStore`] is generic over any [`crate::memory::traits::Memory`]
//! backend (held as an `Arc<dyn Memory>`). This keeps the layer independent
//! of any particular store: production code can pass a SQLite/file backend,
//! while tests use the in-memory `MockMemory` helper. Rules are persisted as
//! JSON entries keyed by `rule/{id}`, so exact-key lookups stay cheap and
//! never block on an embedding model.
//!
//! ## Components
//!
//! - [`types`]  — [`ToolMemoryRule`], [`ToolMemoryPriority`],
//!   [`ToolMemorySource`], and the [`tool_memory_namespace`] helper.
//! - [`store`]  — [`ToolMemoryStore`]: the put / get / list / delete /
//!   prompt API built on top of an `Arc<dyn Memory>`.
//! - [`render`] — [`render_tool_memory_rules`] and
//!   [`ToolMemoryRulesSection`], which pin Critical / High rules into the
//!   system prompt so they survive mid-session compression.

pub mod render;
pub mod store;
pub mod types;

#[cfg(test)]
pub mod test_helpers;

pub use render::{render_tool_memory_rules, ToolMemoryRulesSection, TOOL_MEMORY_HEADING};
pub use store::{ToolMemoryStore, TOOL_MEMORY_PROMPT_CAP};
pub use types::{tool_memory_namespace, ToolMemoryPriority, ToolMemoryRule, ToolMemorySource};
