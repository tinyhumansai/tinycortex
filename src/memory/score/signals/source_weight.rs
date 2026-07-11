//! Source-weight signal — per-provider base weight derived from the
//! `DataSource` when it can be inferred from a chunk's tags.
//!
//! Rationale:
//! - High-intentionality messaging (direct DMs, personal emails) scores higher
//! - Broadcast/channel content scores lower
//! - Documents authored by the user score higher than shared-but-unmodified drops
//!
//! Phase 2 takes a conservative approach: per-[`DataSource`] base weight.
//! Finer distinction (DM vs channel on Slack specifically) requires richer
//! ingest-time metadata and is deferred.

use crate::memory::chunks::{DataSource, Metadata, SourceKind};

const PROVIDER_PREFIX: &str = "provider:";

/// Best-effort map from `Metadata` to a [`DataSource`] — checks the `tags`
/// list for a stable `provider:<snake_case>` provider tag. If not present,
/// falls back to kind-based defaults.
///
/// The ingestion pipeline can (and should) add a provider tag on the
/// canonicalised output so this signal fires deterministically. Until that's
/// wired everywhere, we fall back to the kind-level default.
pub fn infer_data_source(meta: &Metadata) -> Option<DataSource> {
    for tag in &meta.tags {
        let Some(provider) = tag.strip_prefix(PROVIDER_PREFIX) else {
            continue;
        };
        if let Ok(ds) = DataSource::parse(provider) {
            return Some(ds);
        }
    }
    None
}

/// Score in `[0.0, 1.0]` for the chunk's originating provider.
pub fn score(meta: &Metadata) -> f32 {
    if let Some(ds) = infer_data_source(meta) {
        return weight_for(ds);
    }
    // Fallback: kind-level defaults consistent with per-provider averages.
    match meta.source_kind {
        SourceKind::Email => 0.75,
        SourceKind::Document => 0.7,
        SourceKind::Chat => 0.5,
    }
}

/// Per-[`DataSource`] base weight in `[0.0, 1.0]`, hand-tuned per the
/// rationale in the module docs (scoped/directed communication scores
/// higher than broadcast-style channels). Exhaustive match — adding a new
/// `DataSource` variant is a compile error here until a weight is assigned.
fn weight_for(ds: DataSource) -> f32 {
    match ds {
        // Personal email providers score high — typically small, directed audiences
        DataSource::Gmail => 0.8,
        DataSource::OtherEmail => 0.7,
        // Chat providers differ: WhatsApp is typically DM-heavy, Discord
        // can be broadcast-heavy, Telegram mixes both
        DataSource::Whatsapp => 0.75,
        DataSource::Telegram => 0.6,
        DataSource::Discord => 0.5,
        // Agent conversations — high signal, direct interaction with the user
        DataSource::Conversation => 0.9,
        // Documents: Notion = structured, Drive = mixed, Meeting notes = high value
        DataSource::Notion => 0.75,
        DataSource::DriveDocs => 0.6,
        DataSource::MeetingNotes => 0.85,
    }
}

#[cfg(test)]
#[path = "source_weight_tests.rs"]
mod tests;
