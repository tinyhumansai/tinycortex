//! Canonicalisers — normalise source-specific payloads into canonical
//! Markdown with provenance metadata.
//!
//! Each source kind has its own adapter. They all return the same shape:
//! a [`CanonicalisedSource`] containing the markdown blob plus a seed
//! [`Metadata`](crate::memory::chunks::Metadata) that the chunker will clone
//! onto each produced chunk.
//!
//! Adapters do not interpret content semantically — they only normalise
//! shape and capture provenance. Scoring / entity extraction / summarisation
//! happen downstream in later phases.

pub mod chat;
pub mod document;
pub mod email;
pub mod email_clean;

use chrono::{DateTime, Utc};
use serde::{Deserialize, Deserializer, Serialize};

use crate::memory::chunks::{Metadata, SourceRef};

/// Deserialise a `DateTime<Utc>` from either:
/// - a JSON integer = epoch **milliseconds** (legacy callers — back-compat),
/// - a JSON string = RFC 3339 / ISO-8601 (e.g. `"2026-05-17T19:30:00Z"`), or
///   a decimal string containing epoch milliseconds.
///
/// On an unparseable string a serde error is returned (no silent default).
/// Shared across chat, email, and document canonicalisers.
///
/// NOTE: known gap (audit finding QI-15 in
/// `docs/spec/audit/04-queue-ingest.md`) — both the `Millis` branch and the
/// decimal-string fallback accept **any** parseable `i64` as epoch
/// milliseconds with no range check. A caller that accidentally sends
/// epoch-**seconds** (~10 orders of magnitude smaller) has that value silently
/// accepted and interpreted as epoch-milliseconds, producing a timestamp near
/// the Unix epoch (1970) instead of a serde error. The resulting timestamp
/// feeds `timestamp` / `time_range` on the [`Metadata`] each canonicaliser
/// seeds, which in turn drive tree ordering and flush-staleness math — a
/// poisoned timestamp here can misorder chunks or make a fresh chunk look
/// arbitrarily stale. Callers
/// should range-check values below roughly `1e11` (below year ~1973 in
/// milliseconds) before they reach this deserializer if they cannot guarantee
/// the unit.
pub(crate) fn deserialize_flexible_timestamp<'de, D>(
    deserializer: D,
) -> Result<DateTime<Utc>, D::Error>
where
    D: Deserializer<'de>,
{
    #[derive(Deserialize)]
    #[serde(untagged)]
    enum RawTs {
        Millis(i64),
        Text(String),
    }

    let raw = RawTs::deserialize(deserializer)?;
    match raw {
        RawTs::Millis(ms) => chrono::TimeZone::timestamp_millis_opt(&Utc, ms)
            .single()
            .ok_or_else(|| serde::de::Error::custom(format!("invalid epoch-ms: {ms}"))),
        RawTs::Text(s) => {
            if let Ok(dt) = DateTime::parse_from_rfc3339(&s) {
                return Ok(dt.with_timezone(&Utc));
            }
            if let Ok(ms) = s.parse::<i64>() {
                return chrono::TimeZone::timestamp_millis_opt(&Utc, ms)
                    .single()
                    .ok_or_else(|| {
                        serde::de::Error::custom(format!("invalid epoch-ms string: {s}"))
                    });
            }
            Err(serde::de::Error::custom(format!(
                "cannot parse '{s}' as RFC 3339 or epoch-ms"
            )))
        }
    }
}

/// Output of a canonicaliser — one per logical source record
/// (a chat batch, an email, a document).
#[derive(Clone, Debug)]
pub struct CanonicalisedSource {
    /// Canonical Markdown blob produced by the adapter.
    pub markdown: String,
    /// Provenance the chunker will clone onto each emitted chunk.
    pub metadata: Metadata,
}

/// Shared input shape: a payload + a minimal provenance hint.
///
/// Every adapter accepts this generic envelope; the concrete payload type
/// is adapter-specific (see sibling modules for the per-kind inputs).
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct CanonicaliseRequest<P> {
    /// Logical source id (channel for chat, thread for email, doc id).
    pub source_id: String,
    /// Owner / user account.
    #[serde(default)]
    pub owner: String,
    /// Source-specific payload.
    pub payload: P,
    /// Optional tags carried through.
    #[serde(default)]
    pub tags: Vec<String>,
}

/// Trim provider-specific source references and drop blank pointers.
pub fn normalize_source_ref(source_ref: Option<String>) -> Option<SourceRef> {
    source_ref.and_then(|value| {
        let trimmed = value.trim();
        if trimmed.is_empty() {
            None
        } else {
            Some(SourceRef::new(trimmed.to_string()))
        }
    })
}
