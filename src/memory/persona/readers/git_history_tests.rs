//! Tests for the git-history reader (synthetic repo built with git2).

use super::*;
use crate::memory::persona::types::EvidenceTier;
use git2::{Repository, Signature, Time};
use std::cell::Cell;
use tempfile::TempDir;

thread_local! {
    /// Monotonic commit clock so TIME-sort ordering is unambiguous in tests.
    static CLOCK: Cell<i64> = const { Cell::new(1_700_000_000) };
}

/// Commit `files` (path, contents) as `email` with `message` at a strictly
/// increasing timestamp.
fn commit(repo: &Repository, email: &str, message: &str, files: &[(&str, &str)]) {
    let workdir = repo.workdir().unwrap().to_path_buf();
    for (name, contents) in files {
        std::fs::write(workdir.join(name), contents).unwrap();
    }
    let mut index = repo.index().unwrap();
    for (name, _) in files {
        index.add_path(std::path::Path::new(name)).unwrap();
    }
    index.write().unwrap();
    let tree_id = index.write_tree().unwrap();
    let tree = repo.find_tree(tree_id).unwrap();
    let secs = CLOCK.with(|c| {
        let v = c.get();
        c.set(v + 60);
        v
    });
    let sig = Signature::new("Test Author", email, &Time::new(secs, 0)).unwrap();
    let parent = repo
        .head()
        .ok()
        .and_then(|h| h.target())
        .and_then(|o| repo.find_commit(o).ok());
    let parents: Vec<&git2::Commit> = parent.iter().collect();
    repo.commit(Some("HEAD"), &sig, &sig, message, &tree, &parents)
        .unwrap();
}

fn build_repo(dir: &TempDir) -> Repository {
    let repo = Repository::init(dir.path()).unwrap();
    commit(
        &repo,
        "me@work.com",
        "feat: add parser\n\nImplements the streaming parser.",
        &[("parser.rs", "fn parse() {}\n")],
    );
    commit(
        &repo,
        "me@personal.com",
        "fix: handle empty input",
        &[("parser.rs", "fn parse() { /* guard */ }\n")],
    );
    commit(
        &repo,
        "someone-else@example.com",
        "chore: unrelated",
        &[("other.rs", "// not mine\n")],
    );
    commit(
        &repo,
        "me@work.com",
        "test: add regression test",
        &[("parser_test.rs", "#[test] fn t() {}\n")],
    );
    repo
}

#[test]
fn reads_author_filtered_message_and_diff_evidence() {
    let dir = TempDir::new().unwrap();
    let _repo = build_repo(&dir);

    let cfg = GitReadConfig {
        author_emails: vec!["me@work.com".into(), "me@personal.com".into()],
        batch_size: 100,
        diff_sample_cap: 10,
        ..Default::default()
    };
    let sessions = read_repo(dir.path(), &cfg).unwrap();

    // Message batch (T2) + diff sample (T3).
    let msg: Vec<_> = sessions
        .iter()
        .flat_map(|s| &s.evidence)
        .filter(|e| e.tier == EvidenceTier::T2)
        .collect();
    // Three of the four commits are by the configured authors.
    assert_eq!(
        msg.len(),
        3,
        "message evidence: {:?}",
        msg.iter().map(|e| e.excerpt()).collect::<Vec<_>>()
    );
    assert!(msg.iter().any(|e| e.excerpt().contains("feat: add parser")));
    // The unrelated author's commit is excluded.
    assert!(!msg.iter().any(|e| e.excerpt().contains("unrelated")));

    let diffs: Vec<_> = sessions
        .iter()
        .flat_map(|s| &s.evidence)
        .filter(|e| e.tier == EvidenceTier::T3)
        .collect();
    assert!(!diffs.is_empty(), "expected sampled diff evidence");
    assert!(diffs.iter().any(|e| e.excerpt().contains("fn parse")));

    // Message batch is oldest-first (chronological folding).
    let batch = sessions
        .iter()
        .find(|s| s.source.session_id.as_deref() == Some("messages-0"))
        .unwrap();
    assert!(batch.evidence[0].excerpt().contains("feat: add parser"));
}

#[test]
fn empty_author_set_accepts_all() {
    let dir = TempDir::new().unwrap();
    let _repo = build_repo(&dir);
    let cfg = GitReadConfig::default(); // empty author_emails
    let sessions = read_repo(dir.path(), &cfg).unwrap();
    let msg_count = sessions
        .iter()
        .flat_map(|s| &s.evidence)
        .filter(|e| e.tier == EvidenceTier::T2)
        .count();
    assert_eq!(msg_count, 4, "all four commits accepted");
}

#[test]
fn discover_finds_repo() {
    let dir = TempDir::new().unwrap();
    Repository::init(dir.path()).unwrap();
    let found = discover(&[dir.path().to_path_buf()]);
    assert!(found.iter().any(|p| p == dir.path()));
}

#[test]
fn diff_size_cap_truncates() {
    let dir = TempDir::new().unwrap();
    let repo = Repository::init(dir.path()).unwrap();
    let big = "x = 1;\n".repeat(2000);
    commit(&repo, "me@work.com", "feat: big file", &[("big.rs", &big)]);
    let cfg = GitReadConfig {
        author_emails: vec!["me@work.com".into()],
        diff_size_cap_bytes: 500,
        small_commit_max_files: 5,
        ..Default::default()
    };
    let sessions = read_repo(dir.path(), &cfg).unwrap();
    let diff = sessions
        .iter()
        .flat_map(|s| &s.evidence)
        .find(|e| e.tier == EvidenceTier::T3);
    if let Some(d) = diff {
        assert!(
            d.excerpt().contains("truncated"),
            "large diff should be truncated"
        );
    }
}
