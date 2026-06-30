//! Tests for the deterministic reflection driver and its prompt builder.
//! The LLM step is replaced by canned [`super::GoalsGenerator`] impls so the
//! apply / dedupe / cap behaviour is exercised without any model.

use super::*;
use crate::memory::config::MemoryConfig;

/// A generator that populates a fixed set on first run and proposes nothing
/// afterwards — mirroring the OpenHuman "initial population, then minimal"
/// decision logic.
struct InitialPopulationGenerator;

impl GoalsGenerator for InitialPopulationGenerator {
    fn propose(&self, _doc: &GoalsDoc, _context: &str, first_run: bool) -> Vec<GoalMutation> {
        if first_run {
            vec![
                GoalMutation::Add {
                    text: "ship the desktop app".to_string(),
                },
                GoalMutation::Add {
                    text: "keep the rust core authoritative".to_string(),
                },
                // Duplicate of the first — must be de-duped by the driver.
                GoalMutation::Add {
                    text: "Ship   the desktop app".to_string(),
                },
            ]
        } else {
            Vec::new()
        }
    }
}

#[test]
fn first_run_prompt_requests_initial_population() {
    let p = build_prompt("user wants to learn rust", true);
    assert!(p.contains("EMPTY"));
    assert!(p.contains("first run"));
    assert!(p.contains("user wants to learn rust"));
}

#[test]
fn maintenance_prompt_requests_minimal_changes() {
    let p = build_prompt("user finished onboarding", false);
    assert!(p.contains("MINIMAL"));
    assert!(!p.contains("first run"));
    assert!(p.contains("user finished onboarding"));
}

#[test]
fn reflect_populates_initial_set_on_empty_and_dedupes() {
    let tmp = tempfile::tempdir().unwrap();
    let cfg = MemoryConfig::new(tmp.path());

    let outcome = reflect(&cfg, "context nudge", &InitialPopulationGenerator).unwrap();
    assert!(outcome.first_run);
    // Three proposed, one a duplicate -> two applied, one skipped.
    assert_eq!(outcome.applied, 2);
    assert_eq!(outcome.skipped, 1);
    assert_eq!(outcome.goals.items.len(), 2);

    // Persisted to disk.
    let reloaded = store::load(tmp.path()).unwrap();
    assert_eq!(reloaded.items.len(), 2);
}

#[test]
fn reflect_makes_no_change_on_nonempty_with_noop_generator() {
    let tmp = tempfile::tempdir().unwrap();
    let cfg = MemoryConfig::new(tmp.path());
    store::add(tmp.path(), "existing durable goal").unwrap();

    let outcome = reflect(&cfg, "nothing new", &NoopGenerator).unwrap();
    assert!(!outcome.first_run);
    assert_eq!(outcome.applied, 0);
    assert_eq!(outcome.goals.items.len(), 1);
    assert_eq!(outcome.goals.items[0].text, "existing durable goal");
}

#[test]
fn reflect_skips_unknown_id_edits_and_deletes() {
    let tmp = tempfile::tempdir().unwrap();
    let cfg = MemoryConfig::new(tmp.path());
    let (id, _) = store::add(tmp.path(), "keep me").unwrap();

    struct EditDeleteGenerator(String);
    impl GoalsGenerator for EditDeleteGenerator {
        fn propose(&self, _doc: &GoalsDoc, _ctx: &str, _first: bool) -> Vec<GoalMutation> {
            vec![
                GoalMutation::Edit {
                    id: self.0.clone(),
                    text: "kept and updated".to_string(),
                },
                GoalMutation::Delete {
                    id: "g99".to_string(), // unknown -> skipped
                },
            ]
        }
    }

    let outcome = reflect(&cfg, "ctx", &EditDeleteGenerator(id)).unwrap();
    assert_eq!(outcome.applied, 1);
    assert_eq!(outcome.skipped, 1);
    assert_eq!(outcome.goals.items[0].text, "kept and updated");
}

#[test]
fn reflect_skips_secret_or_pii_goal_proposals() {
    let tmp = tempfile::tempdir().unwrap();
    let cfg = MemoryConfig::new(tmp.path());
    store::add(tmp.path(), "ship the memory engine").unwrap();

    struct UnsafeGenerator;
    impl GoalsGenerator for UnsafeGenerator {
        fn propose(&self, doc: &GoalsDoc, _ctx: &str, _first: bool) -> Vec<GoalMutation> {
            vec![
                GoalMutation::Add {
                    text: "email alice@example.com about launch".to_string(),
                },
                GoalMutation::Edit {
                    id: doc.items[0].id.clone(),
                    text: "rotate api_key=sk-abcdefghijklmnopqrstuvwxyz123456".to_string(),
                },
            ]
        }
    }

    let outcome = reflect(&cfg, "ctx", &UnsafeGenerator).unwrap();
    assert_eq!(outcome.applied, 0);
    assert_eq!(outcome.skipped, 2);
    assert_eq!(outcome.goals.items[0].text, "ship the memory engine");
}
