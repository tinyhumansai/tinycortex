//! Git-ledger tests over real on-disk `tempfile` repositories. Ported from
//! OpenHuman `memory_diff::git_store` tests.

use super::*;

fn temp_ledger() -> (Ledger, tempfile::TempDir) {
    let dir = tempfile::tempdir().unwrap();
    let ledger = Ledger::open(dir.path()).unwrap();
    (ledger, dir)
}

fn meta(source_id: &str) -> SnapshotMeta {
    SnapshotMeta {
        source_id: source_id.to_string(),
        source_kind: "folder".to_string(),
        label: "Docs".to_string(),
        trigger: SnapshotTrigger::Auto,
    }
}

fn items(pairs: &[(&str, &str)]) -> Vec<(String, String)> {
    pairs
        .iter()
        .map(|(k, v)| (k.to_string(), v.to_string()))
        .collect()
}

#[test]
fn encode_decode_round_trips() {
    for id in [
        "readme.md",
        "path/to/file.md",
        "user@example.com:msg_xxx",
        "weird name (1)!",
        "..",
        ".",
    ] {
        let enc = encode_item_id(id);
        assert!(!enc.contains('/'), "no slash in {enc}");
        assert!(enc != "." && enc != ".." && !enc.is_empty());
        assert_eq!(decode_item_id(&enc), id, "round trip for {id}");
    }
}

#[test]
fn commit_and_list_snapshots() {
    let (ledger, _dir) = temp_ledger();
    assert!(ledger.list_snapshots(None, 10).unwrap().is_empty());

    let snap = ledger
        .commit_snapshot(&meta("src_a"), &items(&[("a", "alpha")]), 1000)
        .unwrap();
    assert_eq!(snap.source_id, "src_a");
    assert_eq!(snap.item_count, 1);
    assert_eq!(snap.taken_at_ms, 1000);

    let listed = ledger.list_snapshots(Some("src_a"), 10).unwrap();
    assert_eq!(listed.len(), 1);
    assert_eq!(listed[0].id, snap.id);

    let fetched = ledger.get_snapshot(&snap.id).unwrap().unwrap();
    assert_eq!(fetched.source_id, "src_a");
    assert_eq!(fetched.label, "Docs");
    assert_eq!(fetched.item_count, 1);
}

#[test]
fn snapshots_carry_other_sources_forward() {
    let (ledger, _dir) = temp_ledger();
    ledger
        .commit_snapshot(&meta("src_a"), &items(&[("a", "alpha")]), 1000)
        .unwrap();
    let b = ledger
        .commit_snapshot(&meta("src_b"), &items(&[("b", "beta")]), 2000)
        .unwrap();

    // src_a remains listable after a src_b commit (carried forward in tree).
    assert_eq!(ledger.list_snapshots(Some("src_a"), 10).unwrap().len(), 1);
    assert_eq!(ledger.list_snapshots(Some("src_b"), 10).unwrap().len(), 1);
    assert_eq!(ledger.list_snapshots(None, 10).unwrap().len(), 2);
    assert_eq!(b.source_id, "src_b");
}

#[test]
fn compute_changes_added_modified_removed_unchanged() {
    let (ledger, _dir) = temp_ledger();
    let from = ledger
        .commit_snapshot(
            &meta("src_a"),
            &items(&[("a", "alpha"), ("b", "beta"), ("c", "gamma")]),
            1000,
        )
        .unwrap();
    let to = ledger
        .commit_snapshot(
            &meta("src_a"),
            &items(&[("a", "alpha"), ("b", "beta v2"), ("d", "delta")]),
            2000,
        )
        .unwrap();

    let (changes, summary) = ledger
        .compute_changes(Some(&from.id), &to.id, "src_a", 3, false)
        .unwrap();
    assert_eq!(summary.added, 1, "d added");
    assert_eq!(summary.modified, 1, "b modified");
    assert_eq!(summary.removed, 1, "c removed");
    assert_eq!(summary.unchanged, 1, "a unchanged");

    let kind_of = |id: &str| {
        changes
            .iter()
            .find(|c| c.item_id == id)
            .map(|c| c.kind.clone())
    };
    assert_eq!(kind_of("d"), Some(ChangeKind::Added));
    assert_eq!(kind_of("b"), Some(ChangeKind::Modified));
    assert_eq!(kind_of("c"), Some(ChangeKind::Removed));
    assert_eq!(kind_of("a"), None);
}

#[test]
fn compute_changes_from_none_marks_all_added() {
    let (ledger, _dir) = temp_ledger();
    let to = ledger
        .commit_snapshot(&meta("src_a"), &items(&[("a", "x")]), 1000)
        .unwrap();
    let (changes, summary) = ledger
        .compute_changes(None, &to.id, "src_a", 1, false)
        .unwrap();
    assert_eq!(summary.added, 1);
    assert_eq!(changes.len(), 1);
}

#[test]
fn compute_changes_text_diff_only_when_requested() {
    let (ledger, _dir) = temp_ledger();
    let from = ledger
        .commit_snapshot(
            &meta("src_a"),
            &items(&[("a", "line one\nline two\n")]),
            1000,
        )
        .unwrap();
    let to = ledger
        .commit_snapshot(
            &meta("src_a"),
            &items(&[("a", "line one\nline TWO changed\n")]),
            2000,
        )
        .unwrap();

    let (without, _) = ledger
        .compute_changes(Some(&from.id), &to.id, "src_a", 1, false)
        .unwrap();
    assert!(without[0].text_diff.is_none());

    let (with, _) = ledger
        .compute_changes(Some(&from.id), &to.id, "src_a", 1, true)
        .unwrap();
    let td = with[0].text_diff.as_ref().expect("text diff present");
    assert!(td.contains("line TWO changed"), "got: {td}");
}

#[test]
fn pathspec_does_not_leak_across_prefixed_sources() {
    let (ledger, _dir) = temp_ledger();
    ledger
        .commit_snapshot(&meta("src_a"), &items(&[("a", "x")]), 1000)
        .unwrap();
    // src_abc shares the "src_a" prefix; its items must not appear in
    // src_a's diff.
    let abc = ledger
        .commit_snapshot(&meta("src_abc"), &items(&[("z", "zeta")]), 2000)
        .unwrap();
    let a2 = ledger
        .commit_snapshot(&meta("src_a"), &items(&[("a", "x"), ("b", "y")]), 3000)
        .unwrap();

    let (changes, summary) = ledger
        .compute_changes(Some(&abc.id), &a2.id, "src_a", 2, false)
        .unwrap();
    assert_eq!(summary.added, 1, "only b is new in src_a");
    assert!(changes.iter().all(|c| c.item_id != "z"));
}

#[test]
fn read_marker_set_and_get() {
    let (ledger, _dir) = temp_ledger();
    assert_eq!(ledger.get_read_marker("src_a").unwrap(), None);
    let snap = ledger
        .commit_snapshot(&meta("src_a"), &items(&[("a", "x")]), 1000)
        .unwrap();
    ledger.set_read_marker("src_a", &snap.id).unwrap();
    assert_eq!(
        ledger.get_read_marker("src_a").unwrap().as_deref(),
        Some(snap.id.as_str())
    );
    assert_eq!(ledger.get_read_marker("src_b").unwrap(), None);
}

#[test]
fn checkpoint_round_trip() {
    let (ledger, _dir) = temp_ledger();
    let a = ledger
        .commit_snapshot(&meta("src_a"), &items(&[("a", "x")]), 1000)
        .unwrap();
    let b = ledger
        .commit_snapshot(&meta("src_b"), &items(&[("b", "y")]), 1000)
        .unwrap();
    ledger
        .create_checkpoint("ckpt_1", "baseline", &[a.id.clone(), b.id.clone()], 1500)
        .unwrap();

    let loaded = ledger.get_checkpoint("ckpt_1").unwrap().unwrap();
    assert_eq!(loaded.label, "baseline");
    assert_eq!(loaded.created_at_ms, 1500);
    assert_eq!(loaded.snapshot_ids.len(), 2);

    let all = ledger.list_checkpoints(10).unwrap();
    assert_eq!(all.len(), 1);
    assert_eq!(all[0].id, "ckpt_1");
}

#[test]
fn cleanup_checkpoints_removes_old_tags() {
    let (ledger, _dir) = temp_ledger();
    ledger
        .commit_snapshot(&meta("src_a"), &items(&[("a", "x")]), 1000)
        .unwrap();
    ledger
        .create_checkpoint("ckpt_old", "old", &[], 100)
        .unwrap();
    ledger
        .create_checkpoint("ckpt_new", "new", &[], 5000)
        .unwrap();

    let deleted = ledger.cleanup_checkpoints(1000).unwrap();
    assert_eq!(deleted, 1);
    let remaining = ledger.list_checkpoints(10).unwrap();
    assert_eq!(remaining.len(), 1);
    assert_eq!(remaining[0].id, "ckpt_new");
}
