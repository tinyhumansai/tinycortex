//! Tests for the instruction-file reader / rule normaliser.

use super::*;
use crate::memory::persona::types::{EvidenceTier, PersonaFacet};
use std::io::Write;
use tempfile::TempDir;

const SAMPLE: &str = r#"# Global rules

## Version control

- Always branch before writing code.
- Commit regularly with clear messages.

## Style

Use Rust 2021 and standard cargo fmt style.

```sh
cargo fmt --all
```
"#;

#[test]
fn splits_into_rule_granular_verbatim_chunks() {
    let rules = split_rules(SAMPLE);
    // Two bullets + one paragraph = three rules; headings and the code fence
    // are dropped.
    assert_eq!(rules.len(), 3, "rules: {rules:?}");
    assert!(rules.iter().any(|r| r == "Always branch before writing code."));
    assert!(rules.iter().any(|r| r.starts_with("Use Rust 2021")));
    assert!(!rules.iter().any(|r| r.contains("cargo fmt --all"))); // fenced code dropped
    assert!(!rules.iter().any(|r| r.contains('#'))); // headings dropped
}

#[test]
fn nested_bullets_stay_with_their_rule() {
    let md = "- Parent rule\n  - detail a\n  - detail b\n- Second rule";
    let rules = split_rules(md);
    assert_eq!(rules.len(), 2);
    assert!(rules[0].contains("Parent rule"));
    assert!(rules[0].contains("detail a"));
    assert!(rules[0].contains("detail b"));
    assert_eq!(rules[1], "Second rule");
}

#[test]
fn read_file_emits_t0_directives() {
    let dir = TempDir::new().unwrap();
    let path = dir.path().join("CLAUDE.md");
    write!(std::fs::File::create(&path).unwrap(), "{SAMPLE}").unwrap();
    let file = InstructionFile {
        path: path.clone(),
        scope: RuleScope::Global,
    };
    let session = read_file(&file).unwrap();
    assert_eq!(session.evidence.len(), 3);
    for ev in &session.evidence {
        assert_eq!(ev.tier, EvidenceTier::T0);
        assert_eq!(ev.facets, vec![PersonaFacet::Directives]);
        assert_eq!(ev.source.scope.as_deref(), Some("global"));
    }
}

#[test]
fn discover_matches_names_and_repo_scope() {
    let dir = TempDir::new().unwrap();
    // A fake repo: root/.git + root/CLAUDE.md + root/.github/copilot-instructions.md
    let repo = dir.path().join("myrepo");
    std::fs::create_dir_all(repo.join(".git")).unwrap();
    std::fs::create_dir_all(repo.join(".github")).unwrap();
    std::fs::write(repo.join("CLAUDE.md"), "- rule one").unwrap();
    std::fs::write(repo.join("AGENTS.md"), "- rule two").unwrap();
    std::fs::write(repo.join(".github/copilot-instructions.md"), "- rule three").unwrap();
    // A copilot file NOT under .github must be ignored.
    std::fs::write(repo.join("copilot-instructions.md"), "- ignored").unwrap();
    // An unrelated markdown file must be ignored.
    std::fs::write(repo.join("README.md"), "- nope").unwrap();

    let global = dir.path().join("global-CLAUDE.md");
    std::fs::write(&global, "- global rule").unwrap();

    let found = discover(&[dir.path().to_path_buf()], &[global.clone()]);
    let names: Vec<String> = found
        .iter()
        .map(|f| f.path.file_name().unwrap().to_string_lossy().to_string())
        .collect();
    assert!(names.contains(&"CLAUDE.md".to_string()));
    assert!(names.contains(&"AGENTS.md".to_string()));
    assert!(names.contains(&"copilot-instructions.md".to_string()));
    assert!(!found
        .iter()
        .any(|f| f.path.ends_with("myrepo/copilot-instructions.md")));

    // Global file is Global-scoped; repo files are Repo("myrepo").
    assert!(found.iter().any(|f| f.scope == RuleScope::Global));
    assert!(found
        .iter()
        .any(|f| f.scope == RuleScope::Repo("myrepo".to_string())));
}

#[test]
fn content_sha_is_stable_and_sensitive() {
    assert_eq!(content_sha(b"abc"), content_sha(b"abc"));
    assert_ne!(content_sha(b"abc"), content_sha(b"abd"));
    assert_eq!(content_sha(b"abc").len(), 64);
}
