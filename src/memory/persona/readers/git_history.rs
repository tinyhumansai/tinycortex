//! Git commit-history reader (doc 06 §6.4, Family C — requires `git-diff`).
//!
//! Over configured repo roots, author-filtered by a configured email set, this
//! reader emits two evidence streams:
//!
//! - **Message style (T2)** — subject/body/stats, batched (~100 commits per
//!   unit) so the digest infers conventions (Conventional Commits, tense,
//!   subject length, body habits), cadence, and granularity.
//! - **Code style (T3)** — a bounded sample of small commit diffs (small
//!   commits first, hard size/count caps) for naming/comment/test signals.
//!   Explicitly T3: merged code includes agent-written code, so it can only
//!   corroborate.

use std::path::{Path, PathBuf};

use anyhow::{Context, Result};
use chrono::{DateTime, Utc};
use git2::{DiffOptions, Repository, Sort};
use walkdir::WalkDir;

use super::super::types::{EvidenceSource, EvidenceTier, PersonaEvidence, PersonaSourceKind};
use super::RawSession;

/// Tunables for the git reader.
#[derive(Debug, Clone)]
pub struct GitReadConfig {
    /// Author emails to include (case-insensitive). Empty = accept all authors.
    pub author_emails: Vec<String>,
    /// Commit-message evidence units per digest batch (~100).
    pub batch_size: usize,
    /// Max author commits scanned per repo (newest bounded).
    pub max_commits: usize,
    /// Max sampled diffs per repo (T3).
    pub diff_sample_cap: usize,
    /// Max bytes of any single sampled diff kept.
    pub diff_size_cap_bytes: usize,
    /// A commit qualifies for diff sampling only if it changed at most this many
    /// files (small commits first).
    pub small_commit_max_files: usize,
}

impl Default for GitReadConfig {
    fn default() -> Self {
        Self {
            author_emails: Vec::new(),
            batch_size: 100,
            max_commits: 2000,
            diff_sample_cap: 20,
            diff_size_cap_bytes: 4000,
            small_commit_max_files: 3,
        }
    }
}

/// Discover git repositories under `roots` (directories containing a `.git`
/// entry), bounded in depth. Nested repos are each returned once.
pub fn discover(roots: &[PathBuf]) -> Vec<PathBuf> {
    let mut repos: Vec<PathBuf> = Vec::new();
    for root in roots {
        for entry in WalkDir::new(root)
            .max_depth(4)
            .into_iter()
            .filter_map(|e| e.ok())
            .filter(|e| e.file_type().is_dir())
        {
            if entry.path().join(".git").exists() {
                repos.push(entry.path().to_path_buf());
            }
        }
    }
    repos.sort();
    repos.dedup();
    repos
}

/// One author commit's extracted facts.
struct CommitFacts {
    id: String,
    when: DateTime<Utc>,
    subject: String,
    body: String,
    files: usize,
    insertions: usize,
    deletions: usize,
}

/// Read a repo into digest-ready sessions: message-style batches (T2) followed
/// by a bounded sample of small-commit diffs (T3). Empty repos yield nothing.
pub fn read_repo(repo_path: &Path, cfg: &GitReadConfig) -> Result<Vec<RawSession>> {
    let repo = Repository::open(repo_path)
        .with_context(|| format!("open git repo {}", repo_path.display()))?;
    let repo_name = repo_path
        .file_name()
        .map(|n| n.to_string_lossy().to_string())
        .unwrap_or_else(|| "repo".to_string());

    let mut walk = repo.revwalk()?;
    walk.push_head().ok(); // empty repo / detached HEAD → no commits
    walk.set_sorting(Sort::TIME)?;

    let emails: Vec<String> = cfg.author_emails.iter().map(|e| e.to_lowercase()).collect();
    let mut commits: Vec<CommitFacts> = Vec::new();

    for oid in walk.flatten() {
        if commits.len() >= cfg.max_commits {
            break;
        }
        let commit = match repo.find_commit(oid) {
            Ok(c) => c,
            Err(_) => continue,
        };
        let author_email = commit.author().email().unwrap_or("").to_lowercase();
        if !emails.is_empty() && !emails.contains(&author_email) {
            continue;
        }
        let (files, insertions, deletions) = commit_stats(&repo, &commit);
        let full = commit.message().unwrap_or("");
        let subject = full.lines().next().unwrap_or("").trim().to_string();
        let body = full
            .lines()
            .skip(1)
            .collect::<Vec<_>>()
            .join("\n")
            .trim()
            .to_string();
        let when = DateTime::from_timestamp(commit.time().seconds(), 0).unwrap_or_else(Utc::now);
        commits.push(CommitFacts {
            id: oid.to_string(),
            when,
            subject,
            body,
            files,
            insertions,
            deletions,
        });
    }

    if commits.is_empty() {
        return Ok(Vec::new());
    }
    // Oldest-first so trees fold chronologically at backfill.
    commits.reverse();

    let mut sessions: Vec<RawSession> = Vec::new();
    build_message_batches(&repo_name, repo_path, &commits, cfg, &mut sessions);
    build_diff_sample(&repo, &repo_name, repo_path, &commits, cfg, &mut sessions);
    Ok(sessions)
}

/// Files-changed / insertions / deletions for a commit vs. its first parent.
fn commit_stats(repo: &Repository, commit: &git2::Commit) -> (usize, usize, usize) {
    let to_tree = match commit.tree() {
        Ok(t) => t,
        Err(_) => return (0, 0, 0),
    };
    let from_tree = commit.parent(0).ok().and_then(|p| p.tree().ok());
    let mut opts = DiffOptions::new();
    let diff = match repo.diff_tree_to_tree(from_tree.as_ref(), Some(&to_tree), Some(&mut opts)) {
        Ok(d) => d,
        Err(_) => return (0, 0, 0),
    };
    match diff.stats() {
        Ok(s) => (s.files_changed(), s.insertions(), s.deletions()),
        Err(_) => (0, 0, 0),
    }
}

/// Emit T2 message-style evidence, batched.
fn build_message_batches(
    repo_name: &str,
    repo_path: &Path,
    commits: &[CommitFacts],
    cfg: &GitReadConfig,
    sessions: &mut Vec<RawSession>,
) {
    for (batch_idx, chunk) in commits.chunks(cfg.batch_size.max(1)).enumerate() {
        let source = EvidenceSource::new(PersonaSourceKind::GitHistory)
            .with_scope(repo_name.to_string())
            .with_session(format!("messages-{batch_idx}"))
            .with_path(repo_path.to_string_lossy().to_string());
        let mut session = RawSession::new(source.clone());
        for c in chunk {
            let mut excerpt = format!(
                "commit {}: {} [{} files, +{} -{}]",
                &c.id[..c.id.len().min(8)],
                c.subject,
                c.files,
                c.insertions,
                c.deletions
            );
            if !c.body.is_empty() {
                let body: String = c.body.chars().take(400).collect();
                excerpt.push_str("\n  body: ");
                excerpt.push_str(&body);
            }
            session.push(PersonaEvidence::new(
                source.clone(),
                c.when,
                EvidenceTier::T2,
                &excerpt,
                vec![],
            ));
        }
        session.raw_bytes = session.kept_bytes;
        if !session.is_empty() {
            sessions.push(session);
        }
    }
}

/// Emit a bounded T3 sample of small-commit diffs.
fn build_diff_sample(
    repo: &Repository,
    repo_name: &str,
    repo_path: &Path,
    commits: &[CommitFacts],
    cfg: &GitReadConfig,
    sessions: &mut Vec<RawSession>,
) {
    if cfg.diff_sample_cap == 0 {
        return;
    }
    // Small commits first (fewest files, then fewest line changes).
    let mut small: Vec<&CommitFacts> = commits
        .iter()
        .filter(|c| c.files > 0 && c.files <= cfg.small_commit_max_files)
        .collect();
    small.sort_by_key(|c| (c.files, c.insertions + c.deletions));
    small.truncate(cfg.diff_sample_cap);

    if small.is_empty() {
        return;
    }
    let source = EvidenceSource::new(PersonaSourceKind::GitHistory)
        .with_scope(repo_name.to_string())
        .with_session("diffs")
        .with_path(repo_path.to_string_lossy().to_string());
    let mut session = RawSession::new(source.clone());
    for c in small {
        if let Some(patch) = sampled_patch(repo, &c.id, cfg.diff_size_cap_bytes) {
            let excerpt = format!("diff for \"{}\":\n{patch}", c.subject);
            session.push(PersonaEvidence::new(
                source.clone(),
                c.when,
                EvidenceTier::T3,
                &excerpt,
                vec![],
            ));
        }
    }
    session.raw_bytes = session.kept_bytes;
    if !session.is_empty() {
        sessions.push(session);
    }
}

/// Build a truncated unified diff for a commit vs. its first parent, capped at
/// `size_cap` bytes.
fn sampled_patch(repo: &Repository, oid_hex: &str, size_cap: usize) -> Option<String> {
    let oid = git2::Oid::from_str(oid_hex).ok()?;
    let commit = repo.find_commit(oid).ok()?;
    let to_tree = commit.tree().ok()?;
    let from_tree = commit.parent(0).ok().and_then(|p| p.tree().ok());
    let mut opts = DiffOptions::new();
    opts.context_lines(1);
    let diff = repo
        .diff_tree_to_tree(from_tree.as_ref(), Some(&to_tree), Some(&mut opts))
        .ok()?;

    let mut out = String::new();
    let mut truncated = false;
    diff.print(git2::DiffFormat::Patch, |_delta, _hunk, line| {
        if out.len() >= size_cap {
            truncated = true;
            return true; // keep iterating cheaply; we just stop appending
        }
        let origin = line.origin();
        if matches!(origin, '+' | '-' | ' ') {
            out.push(origin);
        }
        out.push_str(&String::from_utf8_lossy(line.content()));
        true
    })
    .ok()?;
    if out.trim().is_empty() {
        return None;
    }
    if truncated {
        out.truncate(size_cap);
        out.push_str("\n… [diff truncated]");
    }
    Some(out)
}

#[cfg(test)]
#[path = "git_history_tests.rs"]
mod tests;
