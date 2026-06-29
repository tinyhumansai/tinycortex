//! The tree-leaf append sink — the archivist's one outbound dependency.
//!
//! OpenHuman's `archive_to_tree` calls straight into
//! `memory_tree::tree::bucket_seal::append_leaf`, hard-wiring the archivist to
//! tree internals. In TinyCortex the `tree` module is ported concurrently, so
//! the archivist must not hard-depend on it. Instead it appends through the
//! small [`TreeLeafSink`] trait defined here, matching OpenHuman's `append
//! leaf` contract: hand the sink a markdown blob plus its leaf metadata and get
//! back the ids of any summaries that sealed during the cascade.
//!
//! A concrete tree-backed implementation lives in the `tree` module; the
//! [`RecordingSink`] in this module is a zero-IO implementation used by the
//! archivist's own tests to assert the cleaned markdown and single-leaf
//! behaviour without standing up a real tree.

use chrono::{DateTime, Utc};

/// Metadata that travels with a leaf when the archivist appends it. Mirrors the
/// meaningful fields of OpenHuman's `TreeLeafPayload` that the archivist
/// actually sets (the extractor-derived `entities` / `topics` / `score` are
/// always empty for cleaned conversations, so they are omitted here).
#[derive(Clone, Debug, PartialEq)]
pub struct LeafMeta {
    /// Deterministic leaf id, `sha256(session_id ‖ markdown)[..32]` with an
    /// `archivist:` prefix. See
    /// [`chunk_id_for_session`](crate::memory::archivist::chunk_id_for_session).
    pub chunk_id: String,
    /// Source session the conversation came from. Archives must cite their
    /// source session id (spec invariant).
    pub session_id: String,
    /// Heuristic token count for the leaf body.
    pub token_count: u32,
    /// Timestamp to stamp the leaf with — the last cleaned turn's timestamp,
    /// or "now" for an empty conversation.
    pub timestamp: DateTime<Utc>,
}

/// The archivist's outbound write contract.
///
/// A single conversation is appended as exactly one leaf. The returned vector
/// holds the ids of any summary nodes that sealed as a result of the append
/// (empty when the leaf merely buffers), matching OpenHuman's `append_leaf`
/// return shape.
pub trait TreeLeafSink {
    /// Append `markdown` as one leaf described by `meta`. Returns the ids of
    /// any summaries that sealed during the cascade.
    fn append_leaf(&self, markdown: &str, meta: &LeafMeta) -> anyhow::Result<Vec<String>>;
}

/// A captured `append_leaf` call: the exact markdown blob and its metadata.
#[derive(Clone, Debug, PartialEq)]
pub struct RecordedLeaf {
    /// The markdown blob handed to the sink.
    pub markdown: String,
    /// The leaf metadata handed to the sink.
    pub meta: LeafMeta,
}

/// In-memory [`TreeLeafSink`] that records every append for inspection.
///
/// Optionally returns a fixed set of "sealed" summary ids so callers can
/// exercise the cascade-id plumbing without a real tree.
#[derive(Default)]
pub struct RecordingSink {
    leaves: std::sync::Mutex<Vec<RecordedLeaf>>,
    seal_ids: Vec<String>,
}

impl RecordingSink {
    /// A sink that records appends and reports no sealed summaries.
    pub fn new() -> Self {
        Self::default()
    }

    /// A sink that records appends and reports `seal_ids` as sealed on every
    /// append.
    pub fn with_seal_ids(seal_ids: Vec<String>) -> Self {
        Self {
            leaves: std::sync::Mutex::new(Vec::new()),
            seal_ids,
        }
    }

    /// Snapshot of every recorded append, in call order.
    pub fn leaves(&self) -> Vec<RecordedLeaf> {
        self.leaves.lock().expect("recording sink poisoned").clone()
    }

    /// Number of appends recorded so far.
    pub fn len(&self) -> usize {
        self.leaves.lock().expect("recording sink poisoned").len()
    }

    /// Whether any append has been recorded.
    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }
}

impl TreeLeafSink for RecordingSink {
    fn append_leaf(&self, markdown: &str, meta: &LeafMeta) -> anyhow::Result<Vec<String>> {
        self.leaves
            .lock()
            .expect("recording sink poisoned")
            .push(RecordedLeaf {
                markdown: markdown.to_string(),
                meta: meta.clone(),
            });
        Ok(self.seal_ids.clone())
    }
}

/// Default timestamp used when a conversation has no turns.
pub(crate) fn now() -> DateTime<Utc> {
    Utc::now()
}
