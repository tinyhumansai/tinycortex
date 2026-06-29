//! Drop tool-call payloads from a conversation.
//!
//! Pure transform — no IO, no allocation beyond the result vec.

use crate::memory::archivist::types::Turn;

/// Return a new conversation with every turn's `tool_calls_json` stripped to
/// `None`. Also drops `tool`-role turns entirely (their content is the tool
/// result, which is noisy and rarely useful out of context).
///
/// This is the heart of the archivist: provider-specific tool-call JSON and
/// raw tool-result dumps distort vector embeddings of the surrounding human
/// conversation, so both are removed before the conversation lands in the tree.
pub fn clean_conversation(turns: &[Turn]) -> Vec<Turn> {
    turns
        .iter()
        .filter(|t| t.role != "tool")
        .map(|t| Turn {
            role: t.role.clone(),
            content: t.content.clone(),
            tool_calls_json: None,
            timestamp: t.timestamp,
        })
        .collect()
}

#[cfg(test)]
#[path = "clip_tests.rs"]
mod tests;
