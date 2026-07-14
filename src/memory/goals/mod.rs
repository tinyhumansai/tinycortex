//! `goals` — the agent's long-term goals when interacting with the user.
//!
//! One of the "specialized long-term memory surfaces" in the [top-level
//! layer diagram](crate::memory), alongside [`crate::memory::tool_memory`].
//! Unlike the chunk/tree/retrieval stack, this module owns its own tiny
//! flat-file persistence directly under the workspace root (see
//! [`crate::memory::config::MemoryConfig::workspace`]) rather than going
//! through the generic store — the whole document is small enough that a
//! dedicated markdown file is simpler than a store-backed row.
//!
//! A deliberately small, high-level memory surface: it maintains a compact
//! markdown file (`MEMORY_GOALS.md`, ~200–500 tokens) holding an editable,
//! ordered **list** of the user's durable goals. The list can be mutated two
//! ways:
//!
//! - **Explicitly** — via the [`store`] operations (`list` / `add` / `edit` /
//!   `delete`), which back both RPC handlers and agent tools in OpenHuman.
//! - **By reflection** — a turn-based pass ([`reflect()`]) that reads the current
//!   list plus a context nudge and applies add/edit/delete. On an empty list
//!   (first run) it populates an initial set; otherwise it makes minimal
//!   changes. The LLM step is abstracted behind [`reflect::GoalsGenerator`];
//!   TinyCortex never calls a real model.
//!
//! ## Invariants (ported from OpenHuman)
//!
//! - At most [`store::GOALS_MAX_ITEMS`] (8) items.
//! - Rendered file at most [`store::GOALS_FILE_MAX_CHARS`] (2000) chars.
//! - Each goal is a single line; empty / multi-line text is rejected.
//! - Cap trimming drops the oldest items first.
//! - A missing file loads as an empty document.
//! - Mutations serialise through a process-wide lock.
//! - The storage path must stay inside the workspace; symlink escapes are
//!   rejected.
//!
//! NOTE: "editable" above means the recognised `- [id] text` item lines
//! round-trip correctly. Free prose, extra headings, or sub-bullets a human
//! adds to the file are silently dropped by the next parse→mutate→save cycle
//! (see [`types::GoalsDoc::render`]) — treat the file as machine-owned for
//! anything beyond editing an existing item's text inline.
//!
//! The workspace root is read from [`crate::memory::config::MemoryConfig`].

pub mod reflect;
pub mod store;
pub mod types;

pub use reflect::{
    build_prompt, reflect, GoalMutation, GoalsGenerator, NoopGenerator, ReflectOutcome,
};
pub use store::{
    add, add_for, delete, delete_for, edit, edit_for, goals_path, list_for, load, save, GOALS_FILE,
    GOALS_FILE_MAX_CHARS, GOALS_MAX_ITEMS,
};
pub use types::{GoalItem, GoalsDoc};
