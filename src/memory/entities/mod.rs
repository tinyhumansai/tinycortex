//! Markdown-backed registry of people and other named things in the user's
//! world.
//!
//! Entity records live as markdown files in the content store so the vault is
//! the source of truth and arbitrary tools (an editor, `grep`, vector search)
//! can introspect or edit them without going through a separate database.
//!
//! ## On disk
//!
//! ```text
//! <content_root>/entities/<kind>/<canonical_id>.md
//! ```
//!
//! Each file is YAML front matter followed by a free-form notes body:
//!
//! ```markdown
//! ---
//! id: person:alice
//! kind: person
//! display_name: Alice Cooper
//! aliases:
//!   - Ali
//! emails:
//!   - alice@example.com
//! handles:
//!   - kind: slack
//!     value: U12345
//! created_at: 2026-05-23T22:00:00+00:00
//! updated_at: 2026-05-23T22:00:00+00:00
//! ---
//!
//! Free-form notes the user can edit. Preserved across upserts.
//! ```
//!
//! `kind` matches the entity-kind taxonomy the tree scorer emits, so the
//! canonical-id format the scorer produces round-trips through here unchanged.
//!
//! ## API
//!
//! - [`put_entity`]    — upsert by canonical id; preserves the notes body.
//! - [`get_entity`]    — read by canonical id.
//! - [`list_entities`] — walk a kind directory.
//! - [`lookup_alias`]  — find a canonical id by alias / email / handle value /
//!   display name (case-insensitive linear scan).
//! - [`canonical_id_for`] — derive a stable `<kind>:<value>` canonical id from
//!   a surface form (lowercases emails, strips `@`/`#` prefixes).
//!
//! ## Layout
//!
//! - [`types`]: [`Entity`], [`EntityKind`], [`EntityHandle`].
//! - [`canonical`]: canonical-id derivation and filename slugging.
//! - [`frontmatter`]: hand-rolled YAML front-matter reader/writer (no
//!   `serde_yaml` dependency; on-disk format identical to OpenHuman).
//! - [`store`]: disk-backed read/write and the notes-preserving upsert.
//!
//! ## Layer rules
//!
//! Borrows nothing from storage internals beyond the content-root path
//! (resolved from [`MemoryConfig::workspace`]). No SQLite, no async, no upward
//! dependencies on orchestration or tools.
//!
//! [`MemoryConfig::workspace`]: crate::memory::config::MemoryConfig::workspace

pub mod canonical;
pub mod frontmatter;
pub mod store;
pub mod types;

pub use canonical::canonical_id_for;
pub use store::{get_entity, list_entities, lookup_alias, put_entity};
pub use types::{Entity, EntityHandle, EntityKind};
