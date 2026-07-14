//! Prompt rendering for tool-scoped memory rules.
//!
//! ## Why a dedicated prompt section
//!
//! Mid-session compression rewrites the rolling chat buffer but never the
//! system prompt — that prompt is frozen for the whole session by design
//! (so the inference backend's prefix cache stays warm). Anything that must
//! be **compression-resistant** therefore has to live in the system prompt.
//!
//! That is exactly where Critical and High priority [`ToolMemoryRule`]s
//! belong: a "never email Sarah" rule cannot be silently dropped when the
//! buffer fills up.
//!
//! ## What gets rendered
//!
//! [`ToolMemoryRulesSection`] takes ownership of a caller-supplied list of
//! rules (already filtered to the eager priorities by
//! [`ToolMemoryStore::rules_for_prompt`]) at construction time and renders
//! them once. The rendered bytes are stable for the lifetime of the
//! session, preserving the inference backend's prefix-cache hit.
//!
//! The pure [`render_tool_memory_rules`] helper is also exposed so callers
//! that pre-render the block (tests, dynamic prompt sources) can share the
//! same byte-stable logic.
//!
//! [`ToolMemoryRule`]: super::types::ToolMemoryRule
//! [`ToolMemoryStore::rules_for_prompt`]: super::store::ToolMemoryStore::rules_for_prompt

use super::types::{ToolMemoryPriority, ToolMemoryRule};

/// Heading injected when at least one rule is present.
pub const TOOL_MEMORY_HEADING: &str = "## Tool-scoped rules";

/// Prompt section that renders an at-construction snapshot of
/// [`ToolMemoryRule`]s into the system prompt.
///
/// Construct via [`Self::new`] with the rules the session builder
/// pre-fetched from [`ToolMemoryStore::rules_for_prompt`].
///
/// [`ToolMemoryStore::rules_for_prompt`]: super::store::ToolMemoryStore::rules_for_prompt
pub struct ToolMemoryRulesSection {
    rendered: String,
}

impl ToolMemoryRulesSection {
    /// Build a section from a pre-fetched rule snapshot.
    ///
    /// Rendering happens up-front so subsequent reads — which run once per
    /// system prompt assembly — are I/O-free and deterministic.
    pub fn new(rules: Vec<ToolMemoryRule>) -> Self {
        Self {
            rendered: render_tool_memory_rules(&rules),
        }
    }

    /// Construct an empty section. Useful as a placeholder for builders
    /// that always include the section in their chain.
    pub fn empty() -> Self {
        Self {
            rendered: String::new(),
        }
    }

    /// Returns true when the section will emit no output.
    pub fn is_empty(&self) -> bool {
        self.rendered.trim().is_empty()
    }

    /// The rendered system-prompt block. Stable for the lifetime of the
    /// section so the inference prefix cache stays warm.
    pub fn rendered(&self) -> &str {
        &self.rendered
    }
}

/// Pure rendering helper — public so callers that pre-render the block
/// (e.g. tests, dynamic prompt sources) can share the same logic.
///
/// Tool names and bodies are normalized to single-line prompt text. This keeps
/// stored newlines/backticks from forging headings or escaping code spans.
pub fn render_tool_memory_rules(rules: &[ToolMemoryRule]) -> String {
    if rules.is_empty() {
        return String::new();
    }

    // Stable order: group by normalized tool name, then Critical before High,
    // then by rule body and id. Callers may pass an
    // already-sorted list (the store does), but rendering must not depend
    // on that contract — the system prompt has to be byte-stable.
    let mut sorted: Vec<&ToolMemoryRule> = rules.iter().collect();
    sorted.sort_by(|a, b| {
        a.tool_name
            .cmp(&b.tool_name)
            .then_with(|| b.priority.cmp(&a.priority))
            .then_with(|| a.rule.cmp(&b.rule))
            .then_with(|| a.id.cmp(&b.id))
    });

    let mut out = String::new();
    out.push_str(TOOL_MEMORY_HEADING);
    out.push_str("\n\n");
    out.push_str(
        "These rules are pinned by the user or by the safety pipeline. Treat \
        every entry as a hard constraint when considering the matching tool — \
        do not override them silently. Lower-priority guidance lives in the \
        `tool-{name}` memory namespace and can be queried via `memory_recall` \
        if needed.\n\n",
    );

    let mut current_tool: Option<&str> = None;
    for rule in sorted {
        if current_tool != Some(rule.tool_name.as_str()) {
            if current_tool.is_some() {
                out.push('\n');
            }
            out.push_str("### `");
            out.push_str(&prompt_line(&rule.tool_name).replace('`', "'"));
            out.push_str("`\n");
            current_tool = Some(rule.tool_name.as_str());
        }
        out.push_str("- ");
        out.push_str(priority_marker(rule.priority));
        out.push(' ');
        out.push_str(&prompt_line(&rule.rule));
        out.push('\n');
    }

    out
}

fn prompt_line(value: &str) -> String {
    value.split_whitespace().collect::<Vec<_>>().join(" ")
}

fn priority_marker(priority: ToolMemoryPriority) -> &'static str {
    match priority {
        ToolMemoryPriority::Critical => "**[critical]**",
        ToolMemoryPriority::High => "**[high]**",
        ToolMemoryPriority::Normal => "**[normal]**",
    }
}

#[cfg(test)]
#[path = "render_tests.rs"]
mod tests;
