//! Agent instruction-file reader (doc 06 §6.4, Family B — T0 evidence).
//!
//! Explicitly authored rules are the highest-confidence persona evidence there
//! is. This reader discovers `CLAUDE.md` / `AGENTS.md` / `.cursorrules` /
//! `.github/copilot-instructions.md` across configured roots (plus explicit
//! global files), splits each into rule-granular chunks (headings / bullets /
//! paragraphs), and emits **verbatim** T0 `directives` evidence tagged with
//! global-vs-repo scope. No LLM is involved — these flow into the pack with
//! minimal rewriting.

use std::path::{Path, PathBuf};

use anyhow::{Context, Result};
use chrono::{DateTime, Utc};
use sha2::{Digest, Sha256};
use walkdir::WalkDir;

use super::super::types::{EvidenceSource, EvidenceTier, PersonaEvidence, PersonaFacet, PersonaSourceKind};
use super::{keep_walk_entry, RawSession};

/// Filenames recognised as agent instruction files.
const INSTRUCTION_FILENAMES: [&str; 4] = [
    "CLAUDE.md",
    "AGENTS.md",
    ".cursorrules",
    "copilot-instructions.md",
];

/// Default depth walked under each configured root.
const DEFAULT_MAX_DEPTH: usize = 6;

/// Whether a discovered rule applies everywhere or is project-local.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RuleScope {
    /// A global instruction file (`~/.claude/CLAUDE.md`, `~/.codex/AGENTS.md`).
    Global,
    /// A repo-scoped instruction file; carries the repo directory name.
    Repo(String),
}

impl RuleScope {
    /// Scope string used on evidence provenance.
    fn as_scope(&self) -> String {
        match self {
            RuleScope::Global => "global".to_string(),
            RuleScope::Repo(name) => format!("repo({name})"),
        }
    }
}

/// A discovered instruction file plus its resolved scope.
#[derive(Debug, Clone)]
pub struct InstructionFile {
    /// Absolute path of the file.
    pub path: PathBuf,
    /// Global vs. repo-scoped.
    pub scope: RuleScope,
}

/// Discover instruction files under `roots` (repo-scoped) plus explicit
/// `globals` (global-scoped). Repo scope is the nearest ancestor directory that
/// contains a `.git` entry; failing that, the file's parent directory name.
pub fn discover(roots: &[PathBuf], globals: &[PathBuf]) -> Vec<InstructionFile> {
    let mut out: Vec<InstructionFile> = Vec::new();

    for g in globals {
        if g.is_file() {
            out.push(InstructionFile {
                path: g.clone(),
                scope: RuleScope::Global,
            });
        }
    }

    for root in roots {
        for entry in WalkDir::new(root)
            .max_depth(DEFAULT_MAX_DEPTH)
            .into_iter()
            .filter_entry(keep_walk_entry)
            .filter_map(|e| e.ok())
            .filter(|e| e.file_type().is_file())
        {
            let path = entry.path();
            let name = match path.file_name().and_then(|n| n.to_str()) {
                Some(n) => n,
                None => continue,
            };
            if !INSTRUCTION_FILENAMES.contains(&name) {
                continue;
            }
            // copilot-instructions.md only counts under a `.github` directory.
            if name == "copilot-instructions.md"
                && path.parent().and_then(|p| p.file_name()).and_then(|n| n.to_str()) != Some(".github")
            {
                continue;
            }
            let scope = repo_scope_for(path).unwrap_or_else(|| {
                RuleScope::Repo(
                    path.parent()
                        .and_then(|p| p.file_name())
                        .map(|n| n.to_string_lossy().to_string())
                        .unwrap_or_else(|| "unknown".to_string()),
                )
            });
            out.push(InstructionFile {
                path: path.to_path_buf(),
                scope,
            });
        }
    }

    out.sort_by(|a, b| a.path.cmp(&b.path));
    out.dedup_by(|a, b| a.path == b.path);
    out
}

/// Find the nearest ancestor of `path` that is a git repo root and return it as
/// a [`RuleScope::Repo`] named after that directory.
fn repo_scope_for(path: &Path) -> Option<RuleScope> {
    let mut dir = path.parent();
    while let Some(d) = dir {
        if d.join(".git").exists() {
            return Some(RuleScope::Repo(
                d.file_name()?.to_string_lossy().to_string(),
            ));
        }
        dir = d.parent();
    }
    None
}

/// Content sha of a file's bytes, for change detection (feeds P9). Hex sha256.
pub fn content_sha(bytes: &[u8]) -> String {
    let mut h = Sha256::new();
    h.update(bytes);
    h.finalize().iter().map(|b| format!("{b:02x}")).collect()
}

/// Read one instruction file into a [`RawSession`] of T0 `directives` evidence,
/// one unit per rule-granular chunk (verbatim).
pub fn read_file(file: &InstructionFile) -> Result<RawSession> {
    let bytes = std::fs::read(&file.path)
        .with_context(|| format!("read instruction file {}", file.path.display()))?;
    let text = String::from_utf8_lossy(&bytes);
    let mtime = file_mtime(&file.path).unwrap_or_else(Utc::now);

    let source = EvidenceSource::new(PersonaSourceKind::InstructionFile)
        .with_scope(file.scope.as_scope())
        .with_path(file.path.to_string_lossy().to_string());
    let mut session = RawSession::new(source.clone());
    session.raw_bytes = bytes.len() as u64;

    for rule in split_rules(&text) {
        session.push(PersonaEvidence::new(
            source.clone(),
            mtime,
            EvidenceTier::T0,
            &rule,
            vec![PersonaFacet::Directives],
        ));
    }
    Ok(session)
}

/// File modification time as UTC.
fn file_mtime(path: &Path) -> Option<DateTime<Utc>> {
    let meta = std::fs::metadata(path).ok()?;
    let modified = meta.modified().ok()?;
    Some(DateTime::<Utc>::from(modified))
}

/// Split markdown into rule-granular, verbatim chunks: each top-level bullet is
/// one rule; consecutive non-bullet lines form a paragraph rule. Headings and
/// fenced code blocks are dropped (headings are structure, not rules). Bullets
/// carry their nested continuation lines so multi-line rules stay intact.
pub fn split_rules(md: &str) -> Vec<String> {
    let mut rules: Vec<String> = Vec::new();
    let mut para: Vec<String> = Vec::new();
    let mut bullet: Vec<String> = Vec::new();
    let mut in_fence = false;

    let flush_para = |para: &mut Vec<String>, rules: &mut Vec<String>| {
        if !para.is_empty() {
            let joined = para.join("\n").trim().to_string();
            if joined.len() >= 3 {
                rules.push(joined);
            }
            para.clear();
        }
    };
    let flush_bullet = |bullet: &mut Vec<String>, rules: &mut Vec<String>| {
        if !bullet.is_empty() {
            let joined = bullet.join("\n").trim().to_string();
            if joined.len() >= 3 {
                rules.push(joined);
            }
            bullet.clear();
        }
    };

    for raw_line in md.lines() {
        let line = raw_line.trim_end();
        let trimmed = line.trim_start();

        if trimmed.starts_with("```") || trimmed.starts_with("~~~") {
            in_fence = !in_fence;
            flush_para(&mut para, &mut rules);
            flush_bullet(&mut bullet, &mut rules);
            continue;
        }
        if in_fence {
            continue;
        }
        if trimmed.is_empty() {
            flush_para(&mut para, &mut rules);
            flush_bullet(&mut bullet, &mut rules);
            continue;
        }
        if trimmed.starts_with('#') {
            flush_para(&mut para, &mut rules);
            flush_bullet(&mut bullet, &mut rules);
            continue; // heading = structure
        }

        let indent = line.len() - trimmed.len();
        if is_bullet(trimmed) {
            if indent == 0 {
                // New top-level bullet — close the previous one.
                flush_para(&mut para, &mut rules);
                flush_bullet(&mut bullet, &mut rules);
                bullet.push(strip_bullet(trimmed).to_string());
            } else if !bullet.is_empty() {
                // Nested bullet — part of the current rule.
                bullet.push(trimmed.to_string());
            } else {
                bullet.push(strip_bullet(trimmed).to_string());
            }
        } else if !bullet.is_empty() {
            // Continuation line of the current bullet rule.
            bullet.push(trimmed.to_string());
        } else {
            para.push(line.to_string());
        }
    }
    flush_para(&mut para, &mut rules);
    flush_bullet(&mut bullet, &mut rules);
    rules
}

fn is_bullet(t: &str) -> bool {
    t.starts_with("- ")
        || t.starts_with("* ")
        || t.starts_with("+ ")
        || t.chars().next().is_some_and(|c| c.is_ascii_digit())
            && (t.contains(". ") || t.contains(") "))
}

fn strip_bullet(t: &str) -> &str {
    if let Some(rest) = t.strip_prefix("- ").or_else(|| t.strip_prefix("* ")).or_else(|| t.strip_prefix("+ ")) {
        return rest.trim_start();
    }
    // Numbered list: strip the leading "N. " / "N) ".
    if let Some(pos) = t.find(['.', ')']) {
        if t[..pos].chars().all(|c| c.is_ascii_digit()) {
            return t[pos + 1..].trim_start();
        }
    }
    t
}

#[cfg(test)]
#[path = "instruction_tests.rs"]
mod tests;
