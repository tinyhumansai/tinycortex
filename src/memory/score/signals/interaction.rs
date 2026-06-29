//! Interaction-weight signal — boosts chunks the user actively engaged with.
//!
//! Direct engagement is one of the strongest retention signals — "a message
//! you replied to" is almost always worth remembering, even if its content
//! looks noisy by other signals.
//!
//! Phase 2 infers engagement from a small set of reserved **tags**:
//! - `reply` — the user replied to this message/thread
//! - `sent` — the user authored this content
//! - `mention` — the user was @-mentioned
//! - `dm` — this arrived in a direct-message channel
//!
//! Ingest adapters can attach these tags during canonicalisation when the
//! upstream source supports the distinction. Absent tags → neutral score.

use crate::memory::chunks::Metadata;

/// Tag set when the user replied to this message/thread.
pub const TAG_REPLY: &str = "reply";
/// Tag set when the user authored this content.
pub const TAG_SENT: &str = "sent";
/// Tag set when the user was @-mentioned.
pub const TAG_MENTION: &str = "mention";
/// Tag set when the message arrived in a direct-message channel.
pub const TAG_DM: &str = "dm";

/// Score in `[0.0, 1.0]` based on engagement tags present on the chunk.
///
/// Multiple tags stack (capped at 1.0):
/// - `sent` → +0.6 (author)
/// - `reply` → +0.5 (active dialogue)
/// - `dm` → +0.3 (scoped audience)
/// - `mention` → +0.2 (addressed)
///
/// Absent any of these → 0.5 (neutral — don't drop the chunk on this signal
/// alone since most content lacks explicit engagement tags).
pub fn score(meta: &Metadata) -> f32 {
    let mut any_tag = false;
    let mut total: f32 = 0.0;
    for t in &meta.tags {
        match t.as_str() {
            TAG_SENT => {
                total += 0.6;
                any_tag = true;
            }
            TAG_REPLY => {
                total += 0.5;
                any_tag = true;
            }
            TAG_DM => {
                total += 0.3;
                any_tag = true;
            }
            TAG_MENTION => {
                total += 0.2;
                any_tag = true;
            }
            _ => {}
        }
    }
    if !any_tag {
        return 0.5;
    }
    total.clamp(0.0, 1.0)
}

#[cfg(test)]
#[path = "interaction_tests.rs"]
mod tests;
