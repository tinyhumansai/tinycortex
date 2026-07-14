//! Source readers (doc 06 §6.4): stream a source's on-disk history and emit
//! redacted [`PersonaEvidence`], one [`RawSession`] per logical session / file /
//! commit batch.
//!
//! The shared principle is **extract the person, discard the machine**: only
//! user decisions, corrections, and prompts are kept; tool output, reasoning,
//! file dumps, and vendor scaffolding are dropped. Each reader records the raw
//! bytes it walked and the bytes it kept so the ≥95% reduction contract is
//! measurable (see [`RawSession::reduction_ratio`]).
//!
//! Readers stream line-at-a-time and never materialise a whole session file.

use chrono::{DateTime, Utc};

use super::types::{EvidenceSource, PersonaEvidence};

pub mod claude_code;
pub mod codex;
pub mod instruction;

mod jsonl;

/// A logical session's worth of extracted, redacted evidence plus the byte
/// accounting that proves the extraction discarded the machine.
#[derive(Debug, Clone)]
pub struct RawSession {
    /// Provenance shared by every unit in this session.
    pub source: EvidenceSource,
    /// The extracted, redacted evidence (user turns, corrections, habits).
    pub evidence: Vec<PersonaEvidence>,
    /// Earliest event timestamp seen, if any.
    pub started_at: Option<DateTime<Utc>>,
    /// Latest event timestamp seen, if any.
    pub ended_at: Option<DateTime<Utc>>,
    /// Total bytes read from the source (the whole file / rows scanned).
    pub raw_bytes: u64,
    /// Bytes of extracted excerpt text kept (post-redaction).
    pub kept_bytes: u64,
}

impl RawSession {
    /// Start an empty session for `source`.
    pub fn new(source: EvidenceSource) -> Self {
        Self {
            source,
            evidence: Vec::new(),
            started_at: None,
            ended_at: None,
            raw_bytes: 0,
            kept_bytes: 0,
        }
    }

    /// Record a kept unit of evidence and fold its timestamp into the window.
    pub fn push(&mut self, ev: PersonaEvidence) {
        let ts = ev.timestamp;
        self.started_at = Some(self.started_at.map_or(ts, |cur| cur.min(ts)));
        self.ended_at = Some(self.ended_at.map_or(ts, |cur| cur.max(ts)));
        self.kept_bytes += ev.excerpt().len() as u64;
        self.evidence.push(ev);
    }

    /// Fraction of raw bytes discarded before the LLM sees anything. `0.0` when
    /// nothing was read.
    pub fn reduction_ratio(&self) -> f64 {
        if self.raw_bytes == 0 {
            return 0.0;
        }
        1.0 - (self.kept_bytes as f64 / self.raw_bytes as f64)
    }

    /// True when the session yielded no evidence (skip it in the pipeline).
    pub fn is_empty(&self) -> bool {
        self.evidence.is_empty()
    }
}

/// Heuristic correction/interrupt detector for tier assignment (T1 — the single
/// highest inference signal short of an explicit rule). A user turn that opens
/// with a redirection ("no, do X", "actually", "stop", "don't", "revert") is
/// the person stopping an agent and steering it.
pub fn looks_like_correction(text: &str) -> bool {
    let t = text.trim_start().to_lowercase();
    const OPENERS: [&str; 14] = [
        "no,", "no ", "nope", "actually", "wait", "stop", "don't", "dont", "instead",
        "revert", "undo", "that's wrong", "thats wrong", "not like that",
    ];
    OPENERS.iter().any(|p| t.starts_with(p))
        || t.contains("not what i")
        || t.contains("do it again")
        || t.contains("that's not")
}

/// True when a user turn is a slash-command / custom-command invocation (a
/// habit trace, T2) — leading `/` or `$` token, e.g. `/code-review`,
/// `$ship-and-babysit`.
pub fn looks_like_command(text: &str) -> bool {
    let t = text.trim_start();
    (t.starts_with('/') || t.starts_with('$'))
        && t.len() > 1
        && t.chars().nth(1).is_some_and(|c| c.is_ascii_alphanumeric())
}
