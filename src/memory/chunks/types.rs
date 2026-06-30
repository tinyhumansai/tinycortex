//! Core types for the memory chunk layer.
//!
//! This module defines the canonical [`Chunk`] representation produced by the
//! ingestion pipeline along with its provenance [`Metadata`] and back-pointer
//! [`SourceRef`].
//!
//! All chunk IDs are deterministic: `sha256(source_kind | "\0" | source_id |
//! "\0" | seq | "\0" | content)` truncated to 32 hex chars so re-ingest of the
//! same source material yields stable IDs and idempotent upserts.

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

/// Which kind of upstream source produced a chunk.
///
/// Used both as a metadata discriminator and as the routing key for the
/// canonicaliser dispatch in the ingest pipeline.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SourceKind {
    /// Chat transcript scoped by channel or group (Slack, Discord, Telegram, WhatsApp…).
    Chat,
    /// Email thread (Gmail and generic IMAP).
    Email,
    /// Standalone document (Notion page, Drive doc, meeting note, uploaded file…).
    Document,
}

impl SourceKind {
    /// Stable string representation for DB storage and RPC surfaces.
    pub fn as_str(self) -> &'static str {
        match self {
            SourceKind::Chat => "chat",
            SourceKind::Email => "email",
            SourceKind::Document => "document",
        }
    }

    /// Parse back from the on-wire / on-disk string form.
    pub fn parse(s: &str) -> Result<Self, String> {
        match s {
            "chat" => Ok(SourceKind::Chat),
            "email" => Ok(SourceKind::Email),
            "document" => Ok(SourceKind::Document),
            other => Err(format!("unknown source kind: {other}")),
        }
    }
}

/// Concrete upstream provider the content came from.
///
/// Each variant maps to exactly one [`SourceKind`] via [`Self::kind`]. Wire
/// form is snake_case (see [`Self::as_str`] / [`Self::parse`]) so it is stable
/// across DB rows, JSON-RPC payloads, and logs.
///
/// Marked `#[non_exhaustive]` so new providers can be added in later phases
/// without breaking downstream pattern matches.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
#[non_exhaustive]
pub enum DataSource {
    // ── Chat transcripts (grouped by channel/group) ────────────────────
    /// Discord channel/server messages. Feeds [`SourceKind::Chat`].
    Discord,
    /// Telegram chat/group messages. Feeds [`SourceKind::Chat`].
    Telegram,
    /// WhatsApp chat/group messages. Feeds [`SourceKind::Chat`].
    Whatsapp,

    // ── Agent conversations (stored as durable memory) ────────────────
    /// Agent conversation transcripts persisted as durable memory. Feeds [`SourceKind::Chat`].
    Conversation,

    // ── Email threads (grouped by thread) ──────────────────────────────
    /// Gmail thread. Feeds [`SourceKind::Email`].
    Gmail,
    /// Catch-all for non-Gmail providers (Outlook, FastMail, generic IMAP, …).
    OtherEmail,

    // ── Documents (no grouping) ────────────────────────────────────────
    /// Notion page. Feeds [`SourceKind::Document`].
    Notion,
    /// Meeting notes document. Feeds [`SourceKind::Document`].
    MeetingNotes,
    /// Google Drive document. Feeds [`SourceKind::Document`].
    DriveDocs,
}

impl DataSource {
    /// Which [`SourceKind`] this provider feeds into.
    pub fn kind(self) -> SourceKind {
        match self {
            Self::Discord | Self::Telegram | Self::Whatsapp | Self::Conversation => {
                SourceKind::Chat
            }
            Self::Gmail | Self::OtherEmail => SourceKind::Email,
            Self::Notion | Self::MeetingNotes | Self::DriveDocs => SourceKind::Document,
        }
    }

    /// Stable snake_case identifier for DB storage, RPC payloads, and logs.
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Discord => "discord",
            Self::Telegram => "telegram",
            Self::Whatsapp => "whatsapp",
            Self::Conversation => "conversation",
            Self::Gmail => "gmail",
            Self::OtherEmail => "other_email",
            Self::Notion => "notion",
            Self::MeetingNotes => "meeting_notes",
            Self::DriveDocs => "drive_docs",
        }
    }

    /// Parse back from the on-wire / on-disk string form.
    pub fn parse(s: &str) -> Result<Self, String> {
        match s {
            "discord" => Ok(Self::Discord),
            "telegram" => Ok(Self::Telegram),
            "whatsapp" => Ok(Self::Whatsapp),
            "conversation" => Ok(Self::Conversation),
            "gmail" => Ok(Self::Gmail),
            "other_email" => Ok(Self::OtherEmail),
            "notion" => Ok(Self::Notion),
            "meeting_notes" => Ok(Self::MeetingNotes),
            "drive_docs" => Ok(Self::DriveDocs),
            other => Err(format!("unknown data source: {other}")),
        }
    }

    /// Every known variant, in declaration order. Useful for tests, CLI
    /// completion, and enumerating supported providers in diagnostic output.
    pub fn all() -> &'static [DataSource] {
        &[
            Self::Discord,
            Self::Telegram,
            Self::Whatsapp,
            Self::Conversation,
            Self::Gmail,
            Self::OtherEmail,
            Self::Notion,
            Self::MeetingNotes,
            Self::DriveDocs,
        ]
    }
}

/// A concrete pointer back to where a chunk originated — used for citation,
/// drill-down, and deduplication at re-ingest time.
///
/// Consumers should treat this as an opaque, source-specific reference. The
/// shape depends on [`SourceKind`]:
/// - **Chat**: `{platform}://{channel}/{message_id}` or `{permalink}`
/// - **Email**: message-id header (`<abc@example.com>`) or provider URL
/// - **Document**: file path, Notion page URL, Drive file id
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct SourceRef {
    /// Opaque provider-specific identifier for the exact source record.
    pub value: String,
}

impl SourceRef {
    /// Wrap an opaque provider-specific identifier as a [`SourceRef`].
    pub fn new(value: impl Into<String>) -> Self {
        Self {
            value: value.into(),
        }
    }
}

/// Provenance metadata captured per chunk at ingest time.
///
/// Captures at minimum: source type, source identifier, owner/account,
/// timestamps, and tags/labels when available.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct Metadata {
    /// Which upstream source kind produced this chunk.
    pub source_kind: SourceKind,
    /// Stable logical id for the ingestion group (channel id, thread id, doc id).
    ///
    /// Chat: channel/group id. Email: thread id. Document: doc id.
    pub source_id: String,
    /// Account or user the content belongs to. Empty string for anonymous / system sources.
    pub owner: String,
    /// Point-in-time timestamp for ordering within a source.
    ///
    /// For chats = message time; for emails = message sent time;
    /// for documents = last-modified or ingest time.
    #[serde(with = "chrono::serde::ts_milliseconds")]
    pub timestamp: DateTime<Utc>,
    /// Covering time range the chunk spans. For a single leaf it usually equals
    /// `(timestamp, timestamp)`; for later summary nodes it widens to cover all
    /// children.
    #[serde(with = "time_range_serde")]
    pub time_range: (DateTime<Utc>, DateTime<Utc>),
    /// Arbitrary labels / tags carried through from the source (e.g. Gmail labels,
    /// Slack reactions, Notion tags). Ingest does not interpret these.
    #[serde(default)]
    pub tags: Vec<String>,
    /// Opaque pointer back to the raw source record for drill-down / citation.
    pub source_ref: Option<SourceRef>,
    /// When set, overrides `source_id` for the chunk file path so multiple
    /// items share one directory. `source_id` remains the dedup key.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub path_scope: Option<String>,
}

impl Metadata {
    /// Convenience constructor used by canonicalisers: point timestamp,
    /// `time_range = (timestamp, timestamp)`.
    pub fn point_in_time(
        source_kind: SourceKind,
        source_id: impl Into<String>,
        owner: impl Into<String>,
        timestamp: DateTime<Utc>,
    ) -> Self {
        Self {
            source_kind,
            source_id: source_id.into(),
            owner: owner.into(),
            timestamp,
            time_range: (timestamp, timestamp),
            tags: Vec::new(),
            source_ref: None,
            path_scope: None,
        }
    }
}

/// A single ingested chunk — the atomic persistence unit.
///
/// In the design this is the leaf of a source tree. Later phases build summary
/// nodes on top of these leaves; here they live standalone.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct Chunk {
    /// Deterministic id derived from (source_kind, source_id, seq_in_source, content).
    pub id: String,
    /// Canonical Markdown content.
    pub content: String,
    /// Provenance metadata.
    pub metadata: Metadata,
    /// Token count (rough heuristic — 1 token ≈ 4 chars).
    pub token_count: u32,
    /// Sequence number of this chunk inside its logical source. Stable and
    /// starts at 0 for the first chunk of a source.
    pub seq_in_source: u32,
    /// When this chunk was persisted to the local store.
    #[serde(with = "chrono::serde::ts_milliseconds")]
    pub created_at: DateTime<Utc>,
    /// True when this chunk is a sub-split of a single logical unit (e.g. a
    /// chat message or email body that exceeded `max_tokens`). Each piece
    /// carries this flag so downstream scorers can lower its weight relative to
    /// whole-unit chunks.
    #[serde(default)]
    pub partial_message: bool,
}

/// A chunk staged for the MD-content write path: a [`Chunk`] whose full body
/// lives on disk at `content_path` (with `content_sha256` for integrity), while
/// the SQLite `content` column carries only a ≤500-char preview.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct StagedChunk {
    /// The chunk being persisted.
    pub chunk: Chunk,
    /// Forward-slash relative path (under the content root) where the full body lives.
    pub content_path: String,
    /// Hex SHA-256 of the on-disk body, recorded for integrity checks.
    pub content_sha256: String,
}

/// Deterministic chunk id.
///
/// `sha256(source_kind | "\0" | source_id | "\0" | seq | "\0" | content)`
/// hex-encoded, first 32 chars (128 bits of collision resistance).
///
/// Content is included so multiple ingest calls that share a `source_id` don't
/// collide on `seq=0,1,2,…`. Re-ingesting the same canonical content under the
/// same `(source_id, seq)` still produces the same id, so upserts stay
/// idempotent.
pub fn chunk_id(
    source_kind: SourceKind,
    source_id: &str,
    seq_in_source: u32,
    content: &str,
) -> String {
    let mut hasher = Sha256::new();
    hasher.update(source_kind.as_str().as_bytes());
    hasher.update([0u8]);
    hasher.update(source_id.as_bytes());
    hasher.update([0u8]);
    hasher.update(seq_in_source.to_be_bytes());
    hasher.update([0u8]);
    hasher.update(content.as_bytes());
    let digest = hasher.finalize();
    let hex = digest.iter().fold(String::with_capacity(64), |mut acc, b| {
        use std::fmt::Write;
        let _ = write!(acc, "{b:02x}");
        acc
    });
    hex[..32].to_string()
}

/// Approximate token count (GPT-family heuristic: 1 token ≈ 4 chars).
pub fn approx_token_count(text: &str) -> u32 {
    // saturating_add guards against absurdly long inputs
    let chars = text.chars().count() as u32;
    chars.saturating_add(3) / 4
}

/// Per-character weight in **quarter-token** units for
/// [`conservative_token_estimate`]. Deliberately pessimistic so the chunker and
/// the embed backstop never under-split: real SentencePiece/WordPiece output for
/// hash-, code-, and markdown-dense text approaches ~1 token/char — far above
/// the `chars/4` GPT heuristic in [`approx_token_count`].
fn char_token_quarters(ch: char) -> u32 {
    if ch.is_ascii_alphanumeric() {
        2 // 0.50 token/char — alphanumeric runs pack ~2-4 chars per token
    } else if ch.is_whitespace() {
        1 // 0.25 token/char — whitespace usually merges into adjacent pieces
    } else {
        4 // 1.00 token/char — ASCII punctuation/symbols AND all non-ASCII
          // (Hebrew/CJK/emoji), which tokenise ~1 piece per char or worse
    }
}

/// Conservative (over-estimating) token count, for embed-safety decisions only.
///
/// [`approx_token_count`] (`chars/4`) under-counts dense markdown/hash/code by
/// ~5×. This weights characters by class so the result is an upper-ish bound on
/// real tokeniser output. It does **not** replace `approx_token_count`, which
/// still drives summariser/seal token budgeting.
pub fn conservative_token_estimate(text: &str) -> u32 {
    let quarters: u64 = text
        .chars()
        .map(|c| u64::from(char_token_quarters(c)))
        .sum();
    let tokens = (quarters + 3) / 4; // ceil(quarters / 4)
    tokens.min(u64::from(u32::MAX)) as u32
}

/// Largest leading slice of `text` whose [`conservative_token_estimate`] is
/// ≤ `budget`, ending on a UTF-8 char boundary. Returns the whole string when
/// already within budget. Used as the embed-path backstop so an over-long body
/// can never be sent to the embedder above its input limit.
pub fn truncate_to_conservative_tokens(text: &str, budget: u32) -> &str {
    if conservative_token_estimate(text) <= budget {
        return text;
    }
    let cap = u64::from(budget).saturating_mul(4); // quarter-tokens
    let mut acc: u64 = 0;
    for (idx, ch) in text.char_indices() {
        let q = u64::from(char_token_quarters(ch));
        if acc + q > cap {
            return &text[..idx];
        }
        acc += q;
    }
    text
}

mod time_range_serde {
    use chrono::{DateTime, TimeZone, Utc};
    use serde::{Deserialize, Deserializer, Serialize, Serializer};

    #[derive(Serialize, Deserialize)]
    struct Wire {
        start_ms: i64,
        end_ms: i64,
    }

    pub fn serialize<S: Serializer>(
        value: &(DateTime<Utc>, DateTime<Utc>),
        serializer: S,
    ) -> Result<S::Ok, S::Error> {
        Wire {
            start_ms: value.0.timestamp_millis(),
            end_ms: value.1.timestamp_millis(),
        }
        .serialize(serializer)
    }

    pub fn deserialize<'de, D: Deserializer<'de>>(
        deserializer: D,
    ) -> Result<(DateTime<Utc>, DateTime<Utc>), D::Error> {
        let wire = Wire::deserialize(deserializer)?;
        let start = Utc
            .timestamp_millis_opt(wire.start_ms)
            .single()
            .ok_or_else(|| serde::de::Error::custom("invalid start_ms"))?;
        let end = Utc
            .timestamp_millis_opt(wire.end_ms)
            .single()
            .ok_or_else(|| serde::de::Error::custom("invalid end_ms"))?;
        Ok((start, end))
    }
}

#[cfg(test)]
#[path = "types_tests.rs"]
mod tests;
