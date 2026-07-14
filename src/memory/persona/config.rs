//! Declarative configuration for the persona pipeline (doc 06 §6.8).
//!
//! Fully serde-declarative like the rest of the crate: source roots (with
//! platform defaults for the launch vendors), export paths, author emails,
//! instruction-file roots, model ids, per-facet asks (defaulted, overridable),
//! per-facet + total token budgets, and run budgets. Kept in its own
//! feature-gated module rather than bolted onto the core `MemoryConfig` so the
//! dependency-light core never carries persona fields.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

use super::compile::{DEFAULT_PER_FACET_BUDGET, DEFAULT_TOTAL_MAX};
use super::reduce::FacetAsks;
use super::types::PersonaFacet;

/// Run-budget ceilings (§6.7): a run aborts cleanly once any is hit.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PersonaRunBudget {
    /// Max sessions/batches digested per run.
    pub max_sessions: usize,
    /// Max LLM chat calls per run (enforced by the provider too).
    pub max_llm_calls: u32,
    /// Max provider spend per run, USD.
    pub max_cost_usd: f64,
}

impl Default for PersonaRunBudget {
    fn default() -> Self {
        Self {
            max_sessions: 5_000,
            max_llm_calls: 5_000,
            max_cost_usd: 5.0,
        }
    }
}

/// Git reader tunables mirrored into config (serde-friendly).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PersonaGitConfig {
    /// Commit-message evidence units per digest batch.
    pub batch_size: usize,
    /// Max author commits scanned per repo.
    pub max_commits: usize,
    /// Max sampled diffs per repo.
    pub diff_sample_cap: usize,
    /// Max bytes of any single sampled diff kept.
    pub diff_size_cap_bytes: usize,
    /// A commit qualifies for diff sampling only if it changed at most this many
    /// files.
    pub small_commit_max_files: usize,
}

impl Default for PersonaGitConfig {
    fn default() -> Self {
        Self {
            batch_size: 100,
            max_commits: 2_000,
            diff_sample_cap: 20,
            diff_size_cap_bytes: 4_000,
            small_commit_max_files: 3,
        }
    }
}

/// Top-level persona configuration.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PersonaConfig {
    /// Identity line for the pack header (email / name).
    pub identity: String,
    /// Claude Code transcript root (`~/.claude/projects`).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub claude_code_root: Option<PathBuf>,
    /// Codex rollout root (`~/.codex/sessions`).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub codex_root: Option<PathBuf>,
    /// Roots walked for repo-scoped instruction files + git repos.
    #[serde(default)]
    pub project_roots: Vec<PathBuf>,
    /// Explicit global instruction files (`~/.claude/CLAUDE.md`, …).
    #[serde(default)]
    pub global_instruction_files: Vec<PathBuf>,
    /// Author emails used to filter git history (case-insensitive).
    #[serde(default)]
    pub author_emails: Vec<String>,
    /// Chat/digest model id.
    pub chat_model: String,
    /// Embedding model id.
    pub embed_model: String,
    /// Per-facet ask overrides (facet wire-string → ask). Missing facets use the
    /// built-in default ask.
    #[serde(default)]
    pub facet_asks: BTreeMap<String, String>,
    /// Per-facet compiled-section token budget.
    #[serde(default = "default_per_facet_budget")]
    pub per_facet_token_budget: u32,
    /// Total pack token ceiling.
    #[serde(default = "default_total_budget")]
    pub total_token_budget: u32,
    /// Run budgets.
    #[serde(default)]
    pub run_budget: PersonaRunBudget,
    /// Git reader tunables.
    #[serde(default)]
    pub git: PersonaGitConfig,
}

fn default_per_facet_budget() -> u32 {
    DEFAULT_PER_FACET_BUDGET
}
fn default_total_budget() -> u32 {
    DEFAULT_TOTAL_MAX
}

impl PersonaConfig {
    /// Construct with platform defaults derived from `home`. This is where the
    /// five-vendor source-root defaults live (config itself stays pure).
    pub fn with_home(home: &Path, identity: impl Into<String>) -> Self {
        Self {
            identity: identity.into(),
            claude_code_root: Some(home.join(".claude/projects")),
            codex_root: Some(home.join(".codex/sessions")),
            project_roots: vec![home.join("work")],
            global_instruction_files: vec![
                home.join(".claude/CLAUDE.md"),
                home.join(".codex/AGENTS.md"),
            ],
            author_emails: Vec::new(),
            // Model ids are plain strings the host may override — persona code
            // must not name any concrete provider, only the id it routes.
            chat_model: "deepseek/deepseek-v4-flash".to_string(),
            embed_model: "openai/text-embedding-3-small".to_string(),
            facet_asks: BTreeMap::new(),
            per_facet_token_budget: DEFAULT_PER_FACET_BUDGET,
            total_token_budget: DEFAULT_TOTAL_MAX,
            run_budget: PersonaRunBudget::default(),
            git: PersonaGitConfig::default(),
        }
    }

    /// Resolve per-facet asks (config overrides + built-in defaults).
    pub fn asks(&self) -> FacetAsks {
        let mut map = BTreeMap::new();
        for facet in PersonaFacet::ALL {
            if let Some(ask) = self.facet_asks.get(facet.as_str()) {
                map.insert(facet, ask.clone());
            }
        }
        FacetAsks(map)
    }
}

#[cfg(test)]
#[path = "config_tests.rs"]
mod tests;
