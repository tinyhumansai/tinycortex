//! Git-history ingestion for the persona pipeline (doc 06 §6.7), feature-gated
//! behind `git-diff`. Split out of `pipeline.rs` to keep that module within the
//! repo's file-size norm; the digest/fold machinery it drives lives on
//! [`Pipeline`] in the parent module.

use std::path::Path;

use anyhow::Result;

use super::{Budget, DigestGuards, Pending, Pipeline, RunMode, RunReport};
use crate::memory::persona::readers::git_history::{self, GitReadConfig};
use crate::memory::persona::reduce::{FacetAsks, ReduceState};
use crate::memory::persona::state;

impl Pipeline<'_> {
    /// Ingest each qualifying repo's author commits as digest sessions. A repo's
    /// HEAD watermark is attached to the LAST of its sessions, so it commits only
    /// once the whole repo has been folded (a budget-truncated repo re-scans next
    /// run). `state` is the module (type namespace); the value binding is the
    /// reduce accumulator.
    pub(super) async fn ingest_git(
        &self,
        mode: RunMode,
        asks: &FacetAsks,
        state: &mut ReduceState,
        budget: &mut Budget,
        guards: &mut DigestGuards,
        report: &mut RunReport,
    ) -> Result<()> {
        let git_cfg = GitReadConfig {
            author_emails: self.persona.author_emails.clone(),
            batch_size: self.persona.git.batch_size,
            max_commits: self.persona.git.max_commits,
            diff_sample_cap: self.persona.git.diff_sample_cap,
            diff_size_cap_bytes: self.persona.git.diff_size_cap_bytes,
            small_commit_max_files: self.persona.git.small_commit_max_files,
        };
        let author_hash = author_set_hash(&self.persona.author_emails);

        // Read all qualifying repos into a pending list (serial, cheap), then
        // digest concurrently + fold serially.
        let mut pending: Vec<Pending> = Vec::new();
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
            for session in &sessions {
                report.evidence_units += session.evidence.len();
            }
            if sessions.is_empty() {
                // No author commits — watermark immediately (nothing to digest).
                state::record_watermark(self.store, &key, &head).await?;
                continue;
            }
            let head_value = serde_json::Value::String(head);
            let last = sessions.len() - 1;
            for (i, session) in sessions.into_iter().enumerate() {
                let commit = (i == last).then(|| (key.clone(), head_value.clone()));
                pending.push(Pending { session, commit });
            }
        }
        self.digest_and_fold(pending, asks, state, budget, guards, report)
            .await
    }
}

/// Stable short hash of the author-email set, so changing it forces a re-scan.
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
fn git_head_sha(repo: &Path) -> Option<String> {
    let repo = git2::Repository::open(repo).ok()?;
    let head = repo.head().ok()?;
    head.target().map(|oid| oid.to_string())
}
