//! YAML front-matter + body composition for chunk and summary `.md` files.
//!
//! Each file written to disk has the form:
//! ```text
//! ---
//! source_kind: chat
//! source_id: slack:#eng
//! seq: 0
//! ...
//! tags:
//!   - person/Alice-Smith
//! ---
//! ## 2026-04-28T10:00:00Z — alice
//! Message body here.
//! ```
//!
//! **SHA-256 is computed over the body bytes only** (everything after the
//! second `---\n` delimiter), so tags can be rewritten atomically without
//! invalidating the content hash.

pub mod chunk;
pub mod summary;
pub mod yaml;

#[cfg(test)]
#[path = "compose_tests.rs"]
mod tests;

/// Bump when the on-disk artifact format changes incompatibly.
pub const MEMORY_ARTIFACT_FORMAT: u32 = 2;
/// Core crate version stamped into summary front-matter provenance. The
/// front-matter key (`openhuman_core_version`) is preserved as a wire string
/// from OpenHuman for vault compatibility.
pub const OPENHUMAN_CORE_VERSION: &str = env!("CARGO_PKG_VERSION");

pub use chunk::{compose_chunk_file, rewrite_tags};
pub use summary::{
    compose_summary_md, rewrite_summary_tags, scope_short_label, ComposedSummary,
    SummaryComposeInput,
};
pub use yaml::{scan_fm_field, source_tag, split_front_matter, with_source_tag, yaml_scalar};
