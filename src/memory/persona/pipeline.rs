//! Persona pipeline orchestration (doc 06 §6.5–§6.8): drives readers → digest →
//! facet-tree reduce → compile, with incremental cursors and run budgets.
//!
//! Two modes (§6.7): `Backfill` walks everything oldest-first so trees fold
//! chronologically; `Incremental` skips files/repos whose cursor is unchanged.
//! Both honour the run budget (max sessions / LLM calls); provider cost is
//! bounded by the provider's own per-run ceiling. Because evidence ids are
//! content-addressed, re-runs dedupe naturally — cursors are a fast-skip, not a
//! correctness gate.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use anyhow::Result;
use serde::Serialize;

use super::compile::{write_pack, PackInputs};
use super::config::PersonaConfig;
use super::distill::digest_session;
use super::readers::{claude_code, codex, instruction, RawSession};
use super::reduce::{fold_digest, fold_directives, seal_and_collect, FacetAsks, ReduceState};
use super::state::PersonaStateStore;
use super::state::{self, file_key, file_unchanged, record_file};
use super::types::PersonaFacet;
use crate::memory::config::MemoryConfig;
use crate::memory::score::extract::ChatProvider;
use crate::memory::tree::Summariser;

/// Run mode (§6.7).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RunMode {
    /// Walk everything, oldest-first.
    Backfill,
    /// Cursor-forward only: skip unchanged files/repos.
    Incremental,
}

impl RunMode {
    fn as_str(self) -> &'static str {
        match self {
            RunMode::Backfill => "backfill",
            RunMode::Incremental => "incremental",
        }
    }
}

/// What a run did — printed by the harness `status`/run output.
#[derive(Debug, Clone, Default, Serialize)]
pub struct RunReport {
    /// The mode that ran.
    pub mode: String,
    /// Transcript/instruction files discovered.
    pub files_seen: usize,
    /// Sessions/batches actually digested.
    pub sessions_processed: usize,
    /// Files skipped because their cursor was unchanged.
    pub sessions_skipped: usize,
    /// Instruction-file rules folded (verbatim T0).
    pub directives_folded: usize,
    /// Total evidence units extracted this run.
    pub evidence_units: usize,
    /// Digest calls that produced at least one observation.
    pub digests: usize,
    /// Observations distilled.
    pub observations: usize,
    /// Per-facet observation counts (facet wire-string → count).
    pub facet_counts: BTreeMap<String, usize>,
    /// True when a run budget stopped the run early (checkpointed).
    pub budget_hit: bool,
    /// Path of the compiled pack, if written.
    pub pack_path: Option<String>,
}

/// A run's budget accounting.
struct Budget {
    max_sessions: usize,
    max_calls: usize,
    sessions: usize,
    calls: usize,
}

impl Budget {
    fn from(cfg: &PersonaConfig) -> Self {
        Self {
            max_sessions: cfg.run_budget.max_sessions,
            max_calls: cfg.run_budget.max_llm_calls as usize,
            sessions: 0,
            calls: 0,
        }
    }
    /// True if another digest would exceed the budget (stop cleanly).
    fn exhausted(&self) -> bool {
        self.sessions >= self.max_sessions || self.calls >= self.max_calls
    }
    fn charge(&mut self) {
        self.sessions += 1;
        self.calls += 1;
    }
}

/// The pipeline binds the workspace, config, provider, summariser, and state
/// store; `run` executes one pass.
pub struct Pipeline<'a> {
    /// Memory workspace config (tree store, content root).
    pub config: &'a MemoryConfig,
    /// Persona config (roots, models, budgets, asks).
    pub persona: &'a PersonaConfig,
    /// Chat provider for the digest map step.
    pub provider: &'a dyn ChatProvider,
    /// Summariser for the facet-tree folds.
    pub summariser: &'a dyn Summariser,
    /// Incremental-run state store.
    pub store: &'a dyn PersonaStateStore,
}

impl Pipeline<'_> {
    /// Execute one pass in `mode`, writing `persona/PERSONA.md`.
    pub async fn run(&self, mode: RunMode) -> Result<RunReport> {
        let asks = self.persona.asks();
        let mut state = ReduceState::default();
        // Seed verbatim directives from the persisted store so an incremental
        // run (which cursor-skips unchanged instruction files) still emits the
        // Directives section. fold_directives dedups on re-read.
        state.directives = super::compile::read_directives(self.config);
        let mut budget = Budget::from(self.persona);
        let mut report = RunReport {
            mode: mode.as_str().to_string(),
            ..Default::default()
        };

        // 1. Instruction files (no LLM) — highest-confidence T0 directives.
        self.ingest_instructions(mode, &mut state, &mut report)
            .await?;

        // 2. Transcripts (Claude Code + Codex) — the digest map step.
        self.ingest_transcripts(mode, &asks, &mut state, &mut budget, &mut report)
            .await?;

        // 3. Git history (feature-gated).
        #[cfg(feature = "git-diff")]
        self.ingest_git(mode, &asks, &mut state, &mut budget, &mut report)
            .await?;

        report.budget_hit = budget.exhausted();

        // 4. Seal facet trees + compile the pack.
        let bodies = seal_and_collect(self.config, &asks, self.summariser).await?;
        let pack_path = self.compile_and_write(bodies, &state)?;
        report.pack_path = Some(pack_path.display().to_string());
        for (facet, n) in &state.counts {
            report.facet_counts.insert(facet.as_str().to_string(), *n);
        }
        Ok(report)
    }

    /// Re-assemble the pack from the current facet-tree roots without any LLM
    /// calls (the `compile` subcommand).
    pub fn compile_only(&self) -> Result<PathBuf> {
        use super::reduce::strip_frontmatter;
        use crate::memory::tree::flavoured::compile_flavoured_root;
        use crate::memory::tree::TreeFactory;

        let asks = self.persona.asks();
        let mut bodies = BTreeMap::new();
        let mut state = ReduceState::default();
        // Reconstruct verbatim directives from the persisted store.
        state.directives = super::compile::read_directives(self.config);
        for facet in PersonaFacet::ALL {
            let factory = TreeFactory::flavoured(facet.tree_scope(), asks.ask(facet));
            let tree = factory.get_or_create(self.config)?;
            let markdown = compile_flavoured_root(self.config, &tree.id)?;
            let body = strip_frontmatter(&markdown);
            if !body.trim().is_empty() {
                bodies.insert(facet, body);
                // Use leaves-folded as a rough observation count proxy.
                *state.counts.entry(facet).or_default() += tree.root_id.is_some() as usize;
            }
        }
        self.compile_and_write(bodies, &state)
    }

    /// Build [`PackInputs`] and write the pack.
    fn compile_and_write(
        &self,
        bodies: BTreeMap<PersonaFacet, String>,
        state: &ReduceState,
    ) -> Result<PathBuf> {
        // Persist the verbatim directives so a later `compile` can rebuild the
        // Directives section without re-reading instruction files.
        if !state.directives.is_empty() {
            super::compile::write_directives(self.config, &state.directives)?;
        }
        let mut inputs = PackInputs::new(self.persona.identity.clone());
        inputs.facet_bodies = bodies;
        inputs.directives = state.directives.clone();
        inputs.counts = state.counts.clone();
        inputs.scopes = state.scopes.iter().map(|(k, v)| (*k, v.len())).collect();
        inputs.per_facet_budget = self.persona.per_facet_token_budget;
        inputs.total_budget_max = self.persona.total_token_budget;
        write_pack(self.config, &inputs)
    }

    async fn ingest_instructions(
        &self,
        mode: RunMode,
        state: &mut ReduceState,
        report: &mut RunReport,
    ) -> Result<()> {
        let files = instruction::discover(
            &self.persona.project_roots,
            &self.persona.global_instruction_files,
        );
        for file in files {
            report.files_seen += 1;
            let key = file_key("instruction_file", &file.path);
            let bytes = match std::fs::read(&file.path) {
                Ok(b) => b,
                Err(_) => continue,
            };
            let sha = instruction::content_sha(&bytes);
            if mode == RunMode::Incremental
                && state::watermark_unchanged(self.store, &key, &sha).await?
            {
                report.sessions_skipped += 1;
                continue;
            }
            let session = match instruction::read_file(&file) {
                Ok(s) => s,
                Err(_) => continue,
            };
            report.evidence_units += session.evidence.len();
            report.directives_folded += session.evidence.len();
            fold_directives(&session.evidence, state);
            state::record_watermark(self.store, &key, &sha).await?;
        }
        Ok(())
    }

    async fn ingest_transcripts(
        &self,
        mode: RunMode,
        asks: &FacetAsks,
        state: &mut ReduceState,
        budget: &mut Budget,
        report: &mut RunReport,
    ) -> Result<()> {
        let mut files: Vec<(PathBuf, &'static str)> = Vec::new();
        if let Some(root) = &self.persona.claude_code_root {
            for p in claude_code::discover(root) {
                files.push((p, "claude_code"));
            }
        }
        if let Some(root) = &self.persona.codex_root {
            for p in codex::discover(root) {
                files.push((p, "codex"));
            }
        }
        // Oldest-first for chronological folding.
        files.sort_by_key(|(p, _)| file_mtime_ms(p));

        for (path, kind) in files {
            report.files_seen += 1;
            let key = file_key(kind, &path);
            if mode == RunMode::Incremental && file_unchanged(self.store, &key, &path).await? {
                report.sessions_skipped += 1;
                continue;
            }
            let session: RawSession = match read_transcript(kind, &path) {
                Ok(s) => s,
                Err(_) => continue,
            };
            record_file(self.store, &key, &path).await?;
            if session.is_empty() {
                continue;
            }
            report.evidence_units += session.evidence.len();
            if budget.exhausted() {
                report.budget_hit = true;
                break;
            }
            budget.charge();
            let digest = digest_session(self.provider, &session).await;
            report.sessions_processed += 1;
            if digest.is_empty() {
                continue;
            }
            report.digests += 1;
            report.observations += digest.observations.len();
            fold_digest(self.config, &digest, asks, self.summariser, state).await?;
        }
        Ok(())
    }

    #[cfg(feature = "git-diff")]
    async fn ingest_git(
        &self,
        mode: RunMode,
        asks: &FacetAsks,
        state: &mut ReduceState,
        budget: &mut Budget,
        report: &mut RunReport,
    ) -> Result<()> {
        use super::readers::git_history::{self, GitReadConfig};

        let git_cfg = GitReadConfig {
            author_emails: self.persona.author_emails.clone(),
            batch_size: self.persona.git.batch_size,
            max_commits: self.persona.git.max_commits,
            diff_sample_cap: self.persona.git.diff_sample_cap,
            diff_size_cap_bytes: self.persona.git.diff_size_cap_bytes,
            small_commit_max_files: self.persona.git.small_commit_max_files,
        };
        let author_hash = author_set_hash(&self.persona.author_emails);

        for repo in git_history::discover(&self.persona.project_roots) {
            report.files_seen += 1;
            let head = match git_head_sha(&repo) {
                Some(h) => format!("{h}:{author_hash}"),
                None => continue,
            };
            let key = state::git_key(&repo);
            if mode == RunMode::Incremental
                && state::watermark_unchanged(self.store, &key, &head).await?
            {
                report.sessions_skipped += 1;
                continue;
            }
            let sessions = match git_history::read_repo(&repo, &git_cfg) {
                Ok(s) => s,
                Err(_) => continue,
            };
            for session in sessions {
                report.evidence_units += session.evidence.len();
                if budget.exhausted() {
                    report.budget_hit = true;
                    break;
                }
                budget.charge();
                let digest = digest_session(self.provider, &session).await;
                report.sessions_processed += 1;
                if digest.is_empty() {
                    continue;
                }
                report.digests += 1;
                report.observations += digest.observations.len();
                fold_digest(self.config, &digest, asks, self.summariser, state).await?;
            }
            state::record_watermark(self.store, &key, &head).await?;
        }
        Ok(())
    }
}

/// Dispatch to the right transcript reader by source-kind tag.
fn read_transcript(kind: &str, path: &Path) -> Result<RawSession> {
    match kind {
        "claude_code" => claude_code::read_session(path),
        "codex" => codex::read_session(path),
        other => anyhow::bail!("unknown transcript kind {other}"),
    }
}

/// File mtime in millis for oldest-first ordering (0 when unknown).
fn file_mtime_ms(path: &Path) -> i64 {
    state::FileCursor::of(path).map(|c| c.mtime_ms).unwrap_or(0)
}

/// Stable short hash of the author-email set, so changing it forces a re-scan.
#[cfg(feature = "git-diff")]
fn author_set_hash(emails: &[String]) -> String {
    use sha2::{Digest, Sha256};
    let mut sorted: Vec<String> = emails.iter().map(|e| e.to_lowercase()).collect();
    sorted.sort();
    let mut h = Sha256::new();
    h.update(sorted.join(",").as_bytes());
    h.finalize()
        .iter()
        .take(4)
        .map(|b| format!("{b:02x}"))
        .collect()
}

/// Current HEAD sha of a repo, or `None` for an empty/broken repo.
#[cfg(feature = "git-diff")]
fn git_head_sha(repo: &Path) -> Option<String> {
    let repo = git2::Repository::open(repo).ok()?;
    let head = repo.head().ok()?;
    head.target().map(|oid| oid.to_string())
}

#[cfg(test)]
#[path = "pipeline_tests.rs"]
mod tests;
