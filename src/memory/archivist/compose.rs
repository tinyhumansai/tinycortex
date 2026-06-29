//! Compose a cleaned conversation into a single markdown blob.
//!
//! The output is the body of one tree leaf — newline-separated `## role`
//! sections with the turn content underneath. Plain markdown; no YAML
//! front-matter (the tree leaf already carries timestamps + provenance).

use crate::memory::archivist::types::Turn;

/// Render the cleaned turns as one markdown blob: `## <role>\n<content>\n` per
/// turn, with a single blank line separating consecutive turns.
///
/// An empty slice yields an empty string.
pub fn compose_conversation_md(turns: &[Turn]) -> String {
    let mut out = String::new();
    for (idx, turn) in turns.iter().enumerate() {
        if idx > 0 {
            out.push('\n');
        }
        out.push_str("## ");
        out.push_str(&turn.role);
        out.push('\n');
        out.push_str(&turn.content);
        if !turn.content.ends_with('\n') {
            out.push('\n');
        }
    }
    out
}

#[cfg(test)]
#[path = "compose_tests.rs"]
mod tests;
