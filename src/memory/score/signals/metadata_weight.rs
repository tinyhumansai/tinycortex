//! Metadata-weight signal — base weight from the source kind's grouping.
//!
//! The idea: a 1:1 email thread is inherently higher-signal than a broadcast
//! Slack channel, regardless of content. This signal captures the "shape"
//! of the interaction: how scoped is the audience?
//!
//! Phase 2 keeps this simple: one weight per `SourceKind`. Per-grouping
//! context (e.g., channel size, thread participant count) is a future
//! refinement when we actually have that metadata at ingest.

use crate::memory::chunks::{Metadata, SourceKind};

/// Base weight for each source kind.
///
/// Email threads are typically scoped (1:1 or small groups, directed).
/// Documents are single-author outputs — high intentionality per chunk.
/// Chats vary widely; base weight is lower because the channel could be
/// a 200-person broadcast or a tight DM — the interaction signal disambiguates.
pub fn score(meta: &Metadata) -> f32 {
    match meta.source_kind {
        SourceKind::Email => 0.8,
        SourceKind::Document => 0.9,
        SourceKind::Chat => 0.5,
    }
}

#[cfg(test)]
#[path = "metadata_weight_tests.rs"]
mod tests;
