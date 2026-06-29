//! End-to-end: clean → compose → push the conversation into a tree as one
//! leaf, through the injected [`TreeLeafSink`] write contract so the archivist
//! stays unaware of tree internals.
//!
//! The archivist intentionally writes one leaf per archived conversation rather
//! than persisting another bespoke store. [`chunk_id_for_session`] hashes
//! `(session_id, composed_markdown)` so retries are deterministic for the same
//! conversation snapshot while distinct sessions or edits produce a fresh leaf
//! id.
//!
//! These archivist leaves are synthetic conversation snapshots, not chunk-store
//! rows. In OpenHuman they participate in the L0 buffer contract only;
//! downstream source-tree sealing still expects chunk-store-backed leaves when
//! rehydrating inputs, so multi-conversation summarisation of archivist-only
//! source trees needs a dedicated hydration path before these synthetic leaves
//! can seal upward.

use crate::memory::archivist::clip::clean_conversation;
use crate::memory::archivist::compose::compose_conversation_md;
use crate::memory::archivist::sink::{now, LeafMeta, TreeLeafSink};
use crate::memory::archivist::types::Turn;
use crate::memory::store::content::atomic::sha256_hex;

/// Rough bytes-per-token divisor used to estimate a leaf's token count.
const TOKEN_DIVISOR: usize = 4;

/// Outcome of archiving a conversation: the leaf id that was written plus any
/// summaries that sealed during the cascade. Mirrors OpenHuman's
/// `TreeWriteOutcome` (archivist leaves never trigger an immediate seal, so
/// `seal_pending` is always `false`).
#[derive(Clone, Debug, PartialEq)]
pub struct ArchiveOutcome {
    /// The deterministic id of the leaf that was appended.
    pub chunk_id: String,
    /// Ids of summaries that sealed as a result of the append (empty when the
    /// leaf merely buffers).
    pub new_summary_ids: Vec<String>,
    /// Whether a seal is pending. Always `false` for archivist leaves.
    pub seal_pending: bool,
}

/// Clean the conversation, compose it as markdown, and append a single leaf to
/// `sink`. Returns the [`ArchiveOutcome`] including any summary ids that sealed
/// during the cascade.
///
/// `tool`-role turns and per-turn `tool_calls_json` are dropped first (see
/// [`clean_conversation`]); the leaf timestamp is the last cleaned turn's
/// timestamp, or "now" for an empty conversation.
pub fn archive_to_tree(
    sink: &dyn TreeLeafSink,
    session_id: &str,
    turns: &[Turn],
) -> anyhow::Result<ArchiveOutcome> {
    let cleaned = clean_conversation(turns);
    let md = compose_conversation_md(&cleaned);
    let chunk_id = chunk_id_for_session(session_id, &md);
    let token_count = (md.len() / TOKEN_DIVISOR).max(1) as u32;
    let timestamp = cleaned.last().map(|t| t.timestamp).unwrap_or_else(now);

    let meta = LeafMeta {
        chunk_id: chunk_id.clone(),
        session_id: session_id.to_string(),
        token_count,
        timestamp,
    };

    let new_summary_ids = sink.append_leaf(&md, &meta)?;
    Ok(ArchiveOutcome {
        chunk_id,
        new_summary_ids,
        seal_pending: false,
    })
}

/// Derive the deterministic leaf id for a conversation snapshot:
/// `archivist:<sha256(session_id ‖ \0 ‖ markdown)[..32 hex chars]>`.
///
/// Stable for the same `(session_id, markdown)` pair so retries are
/// idempotent; distinct sessions or edited transcripts hash to a fresh id.
pub fn chunk_id_for_session(session_id: &str, md: &str) -> String {
    let mut bytes = Vec::with_capacity(session_id.len() + 1 + md.len());
    bytes.extend_from_slice(session_id.as_bytes());
    bytes.push(0);
    bytes.extend_from_slice(md.as_bytes());
    let hex = sha256_hex(&bytes);
    format!("archivist:{}", &hex[..32])
}

#[cfg(test)]
#[path = "tree_writer_tests.rs"]
mod tests;
