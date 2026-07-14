//! Persona compiler (doc 06 §6.5 / §6.9): a **deterministic**, non-LLM step that
//! assembles the facet tree roots into the prompt-ready pack `persona/PERSONA.md`.
//!
//! Section order is fixed (§6.9): identity header, `## Directives` (T0,
//! budget-protected first), then Communication, Coding style, Stack, Workflow,
//! Environment, Anti-preferences. Each facet is clamped to a per-facet token
//! budget and the whole pack to `[5_000, 10_000]` tokens using the crate's token
//! estimator. Strength annotations ("distilled from N observations across M
//! projects") let downstream consumers judge confidence.

use std::collections::BTreeMap;
use std::path::PathBuf;

use anyhow::{Context, Result};

use super::types::PersonaFacet;
use crate::memory::config::MemoryConfig;
use crate::memory::tree::summarise::clamp_to_budget;

/// Default per-facet token budget for a compiled section.
pub const DEFAULT_PER_FACET_BUDGET: u32 = 1_200;
/// Default total pack floor / ceiling (§6.1).
pub const DEFAULT_TOTAL_MIN: u32 = 5_000;
pub const DEFAULT_TOTAL_MAX: u32 = 10_000;

/// Everything the deterministic compiler needs. Gathered by the pipeline from
/// the sealed facet trees, or reconstructed from disk by the `compile`
/// subcommand.
#[derive(Debug, Clone)]
pub struct PackInputs {
    /// Identity line (e.g. the owner's email or name).
    pub identity: String,
    /// Compiled root body per facet (front-matter already stripped).
    pub facet_bodies: BTreeMap<PersonaFacet, String>,
    /// Verbatim T0 directive rules for the Directives section (near-verbatim,
    /// not LLM-folded). Rendered ahead of any distilled directives body.
    pub directives: Vec<String>,
    /// Observation counts per facet (strength annotation).
    pub counts: BTreeMap<PersonaFacet, usize>,
    /// Distinct scope (project/repo) counts per facet (strength annotation).
    pub scopes: BTreeMap<PersonaFacet, usize>,
    /// Per-facet token budget.
    pub per_facet_budget: u32,
    /// Total pack token ceiling.
    pub total_budget_max: u32,
}

impl PackInputs {
    /// Build inputs with the default budgets.
    pub fn new(identity: impl Into<String>) -> Self {
        Self {
            identity: identity.into(),
            facet_bodies: BTreeMap::new(),
            directives: Vec::new(),
            counts: BTreeMap::new(),
            scopes: BTreeMap::new(),
            per_facet_budget: DEFAULT_PER_FACET_BUDGET,
            total_budget_max: DEFAULT_TOTAL_MAX,
        }
    }
}

/// Compile the pack markdown. Deterministic: identical inputs → identical bytes.
pub fn compile_pack(inputs: &PackInputs) -> String {
    let mut out = String::new();
    out.push_str(&format!("# Persona: {}\n\n", inputs.identity.trim()));
    out.push_str(&header_summary(inputs));
    out.push('\n');

    // Budget accounting: directives (ALL[0]) is emitted first and is therefore
    // protected — later facets are dropped before it if the ceiling is hit.
    let mut spent: u32 = estimate(&out);
    for facet in PersonaFacet::ALL {
        let body = match facet_body(inputs, facet) {
            Some(b) if !b.trim().is_empty() => b,
            _ => continue,
        };
        let remaining = inputs.total_budget_max.saturating_sub(spent);
        if remaining == 0 {
            break;
        }
        let facet_budget = inputs.per_facet_budget.min(remaining);
        let (clamped, _) = clamp_to_budget(body.trim(), facet_budget);
        if clamped.trim().is_empty() {
            continue;
        }

        let mut section = String::new();
        section.push_str(&format!("## {}\n", facet.heading()));
        if let Some(annotation) = strength_annotation(inputs, facet) {
            section.push_str(&format!("_{annotation}_\n\n"));
        } else {
            section.push('\n');
        }
        section.push_str(clamped.trim());
        section.push_str("\n\n");

        spent += estimate(&section);
        out.push_str(&section);
    }

    out.trim_end().to_string() + "\n"
}

/// Effective body for a facet. The Directives section is the verbatim T0 rules
/// (near-verbatim) followed by any distilled directives body; every other facet
/// is just its distilled tree body.
fn facet_body(inputs: &PackInputs, facet: PersonaFacet) -> Option<String> {
    if facet == PersonaFacet::Directives {
        let mut out = String::new();
        for rule in &inputs.directives {
            out.push_str("- ");
            out.push_str(rule.trim());
            out.push('\n');
        }
        if let Some(b) = inputs.facet_bodies.get(&facet) {
            if !b.trim().is_empty() {
                out.push_str(b.trim());
                out.push('\n');
            }
        }
        return (!out.trim().is_empty()).then(|| out.trim().to_string());
    }
    inputs
        .facet_bodies
        .get(&facet)
        .filter(|b| !b.trim().is_empty())
        .cloned()
}

/// One-line trait summary naming the facets that carry content.
fn header_summary(inputs: &PackInputs) -> String {
    let present: Vec<&str> = PersonaFacet::ALL
        .iter()
        .filter(|f| facet_body(inputs, **f).is_some())
        .map(|f| f.heading())
        .collect();
    if present.is_empty() {
        return "_No persona evidence distilled yet._\n".to_string();
    }
    format!(
        "> Mimic-grade persona pack distilled from this person's coding-agent history, \
instruction files, and git commits. Facets covered: {}.\n",
        present.join(", ")
    )
}

/// "distilled from N observations across M projects" for a facet, or `None`.
fn strength_annotation(inputs: &PackInputs, facet: PersonaFacet) -> Option<String> {
    let n = inputs.counts.get(&facet).copied().unwrap_or(0);
    if n == 0 {
        return None;
    }
    let m = inputs.scopes.get(&facet).copied().unwrap_or(0);
    if m > 1 {
        Some(format!("distilled from {n} observations across {m} projects"))
    } else {
        Some(format!("distilled from {n} observations"))
    }
}

/// Token estimate consistent with [`clamp_to_budget`].
fn estimate(text: &str) -> u32 {
    clamp_to_budget(text, u32::MAX).1
}

/// Directory holding persona artifacts (`<workspace>/persona`).
pub fn persona_dir(config: &MemoryConfig) -> PathBuf {
    config.workspace.join("persona")
}

/// Absolute path of the compiled pack.
pub fn pack_path(config: &MemoryConfig) -> PathBuf {
    persona_dir(config).join("PERSONA.md")
}

/// Path of the persisted verbatim-directives store (§6.9). One rule per line.
pub fn directives_path(config: &MemoryConfig) -> PathBuf {
    persona_dir(config).join("directives.md")
}

/// Persist the verbatim T0 directive rules so a later no-LLM `compile` can
/// reconstruct the Directives section.
pub fn write_directives(config: &MemoryConfig, directives: &[String]) -> Result<()> {
    let path = directives_path(config);
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)
            .with_context(|| format!("create persona dir {}", parent.display()))?;
    }
    crate::memory::fsutil::atomic_write(&path, directives.join("\n").as_bytes())
        .with_context(|| format!("write directives {}", path.display()))?;
    Ok(())
}

/// Read the persisted verbatim directives (empty if none).
pub fn read_directives(config: &MemoryConfig) -> Vec<String> {
    match std::fs::read_to_string(directives_path(config)) {
        Ok(s) => s.lines().filter(|l| !l.trim().is_empty()).map(str::to_string).collect(),
        Err(_) => Vec::new(),
    }
}

/// Compile the pack and write it to [`pack_path`], returning the path.
pub fn write_pack(config: &MemoryConfig, inputs: &PackInputs) -> Result<PathBuf> {
    let markdown = compile_pack(inputs);
    let path = pack_path(config);
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)
            .with_context(|| format!("create persona dir {}", parent.display()))?;
    }
    crate::memory::fsutil::atomic_write(&path, markdown.as_bytes())
        .with_context(|| format!("write persona pack {}", path.display()))?;
    Ok(path)
}

#[cfg(test)]
#[path = "compile_tests.rs"]
mod tests;
