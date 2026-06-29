//! End-to-end engine tests over a temp git ledger. The pairwise / read-marker /
//! checkpoint cases are ported from OpenHuman `memory_diff::ops` tests; the
//! injection-seam cases exercise the [`SnapshotItemSource`] decoupling.

use super::ledger::{Ledger, SnapshotMeta};
use super::*;

use tempfile::TempDir;

/// A fresh engine over an isolated temp workspace, with the given in-memory
/// item source. The returned `TempDir` must be kept alive for the test.
fn engine_with(items: InMemoryItemSource) -> (DiffEngine<InMemoryItemSource>, TempDir) {
    let dir = tempfile::tempdir().unwrap();
    let engine = DiffEngine::new(dir.path().to_path_buf(), items);
    (engine, dir)
}

/// A fresh engine with an empty item source.
fn engine() -> (DiffEngine<InMemoryItemSource>, TempDir) {
    engine_with(InMemoryItemSource::new())
}

fn src(id: &str) -> SourceDescriptor {
    SourceDescriptor::new(id, "folder", "Docs")
}

/// Seed a snapshot directly through the ledger (bypassing the item source), with
/// an explicit timestamp so commit-time ordering is deterministic.
fn seed(
    workspace: &std::path::Path,
    source_id: &str,
    taken_at_ms: i64,
    items: &[(&str, &str)],
) -> Snapshot {
    let ledger = Ledger::open(workspace).unwrap();
    let items: Vec<(String, String)> = items
        .iter()
        .map(|(k, v)| (k.to_string(), v.to_string()))
        .collect();
    ledger
        .commit_snapshot(
            &SnapshotMeta {
                source_id: source_id.to_string(),
                source_kind: "folder".to_string(),
                label: "Docs".to_string(),
                trigger: SnapshotTrigger::Auto,
            },
            &items,
            taken_at_ms,
        )
        .unwrap()
}

// ── Snapshot capture via the injected item source ────────────────────────

#[test]
fn take_snapshot_reads_from_injected_item_source() {
    let mut items = InMemoryItemSource::new();
    items.set_source("src_a", &[("a", "alpha"), ("b", "beta")]);
    let (engine, _dir) = engine_with(items);

    let snap = engine
        .take_snapshot(&src("src_a"), SnapshotTrigger::Manual)
        .unwrap();
    assert_eq!(snap.source_id, "src_a");
    assert_eq!(snap.item_count, 2);
    assert_eq!(snap.trigger, SnapshotTrigger::Manual);

    let listed = engine.list_snapshots(Some("src_a"), 10).unwrap();
    assert_eq!(listed.len(), 1);
    assert_eq!(listed[0].id, snap.id);
}

#[test]
fn auto_snapshot_uses_auto_trigger() {
    let mut items = InMemoryItemSource::new();
    items.push_item("src_a", "a", "alpha");
    let (engine, _dir) = engine_with(items);

    let snap = engine.auto_snapshot_after_sync(&src("src_a")).unwrap();
    assert_eq!(snap.trigger, SnapshotTrigger::Auto);
    assert_eq!(snap.item_count, 1);
}

// ── compute_diff ────────────────────────────────────────────────────────

#[test]
fn compute_diff_detects_added_modified_removed() {
    let (engine, _dir) = engine();
    let from = seed(
        engine.workspace(),
        "src_a",
        1000,
        &[("a", "alpha"), ("b", "beta"), ("c", "gamma")],
    );
    let to = seed(
        engine.workspace(),
        "src_a",
        2000,
        &[("a", "alpha"), ("b", "beta v2"), ("d", "delta")],
    );

    let diff = engine.compute_diff(Some(&from.id), &to.id, false).unwrap();

    assert_eq!(diff.summary.added, 1, "d added");
    assert_eq!(diff.summary.modified, 1, "b modified");
    assert_eq!(diff.summary.removed, 1, "c removed");
    assert_eq!(diff.summary.unchanged, 1, "a unchanged");

    let kind_of = |id: &str| {
        diff.changes
            .iter()
            .find(|c| c.item_id == id)
            .map(|c| c.kind.clone())
    };
    assert_eq!(kind_of("d"), Some(ChangeKind::Added));
    assert_eq!(kind_of("b"), Some(ChangeKind::Modified));
    assert_eq!(kind_of("c"), Some(ChangeKind::Removed));
    assert_eq!(kind_of("a"), None, "unchanged items are not in changes");
}

#[test]
fn compute_diff_against_none_marks_all_added() {
    let (engine, _dir) = engine();
    let to = seed(engine.workspace(), "src_a", 1000, &[("a", "x")]);
    let diff = engine.compute_diff(None, &to.id, false).unwrap();
    assert_eq!(diff.summary.added, 1);
    assert_eq!(diff.from_snapshot_id, None);
}

#[test]
fn compute_diff_rejects_cross_source() {
    let (engine, _dir) = engine();
    let from = seed(engine.workspace(), "src_a", 1000, &[("a", "x")]);
    let to = seed(engine.workspace(), "src_b", 2000, &[("b", "y")]);
    let err = engine
        .compute_diff(Some(&from.id), &to.id, false)
        .unwrap_err()
        .to_string();
    assert!(err.contains("cross-source"), "got: {err}");
}

#[test]
fn compute_diff_text_diff_only_when_requested() {
    let (engine, _dir) = engine();
    let from = seed(
        engine.workspace(),
        "src_a",
        1000,
        &[("a", "line one\nline two\n")],
    );
    let to = seed(
        engine.workspace(),
        "src_a",
        2000,
        &[("a", "line one\nline TWO changed\n")],
    );

    let without = engine.compute_diff(Some(&from.id), &to.id, false).unwrap();
    assert!(without.changes[0].text_diff.is_none());

    let with = engine.compute_diff(Some(&from.id), &to.id, true).unwrap();
    let td = with.changes[0]
        .text_diff
        .as_ref()
        .expect("text diff present");
    assert!(td.contains("line TWO changed"), "got: {td}");
}

// ── diff_since_last ─────────────────────────────────────────────────────

#[test]
fn diff_since_last_handles_zero_one_two_snapshots() {
    let (engine, _dir) = engine();

    // 0 snapshots → error
    assert!(engine.diff_since_last("src_a", false).is_err());

    // 1 snapshot → everything added (diff vs None)
    seed(engine.workspace(), "src_a", 1000, &[("a", "x")]);
    let one = engine.diff_since_last("src_a", false).unwrap();
    assert_eq!(one.summary.added, 1);

    // 2 snapshots → diff latest vs previous
    seed(engine.workspace(), "src_a", 2000, &[("a", "x"), ("b", "y")]);
    let two = engine.diff_since_last("src_a", false).unwrap();
    assert_eq!(two.summary.added, 1, "b is new in s2");
    assert_eq!(two.summary.unchanged, 1, "a unchanged");
}

// ── diff_since_read / mark_read ─────────────────────────────────────────

#[test]
fn diff_since_read_commits_marker_and_returns_only_new_changes() {
    let (engine, _dir) = engine();
    seed(engine.workspace(), "src_a", 1000, &[("a", "x")]);

    // First read: no marker → full diff (a added), and commit advances marker.
    let first = engine.diff_since_read("src_a", false, true).unwrap();
    assert_eq!(first.summary.added, 1);

    // Second read with no new snapshot: marker == head → nothing changed.
    let second = engine.diff_since_read("src_a", false, true).unwrap();
    assert_eq!(second.summary.added, 0);
    assert_eq!(second.summary.modified, 0);
    assert_eq!(second.summary.removed, 0);
    assert!(second.changes.is_empty());

    // New snapshot then read: only the delta since the marker shows.
    seed(engine.workspace(), "src_a", 2000, &[("a", "x"), ("b", "y")]);
    let third = engine.diff_since_read("src_a", false, true).unwrap();
    assert_eq!(third.summary.added, 1, "only b is new since last read");
    assert_eq!(third.summary.unchanged, 1);
}

#[test]
fn diff_since_read_without_commit_does_not_advance_marker() {
    let (engine, _dir) = engine();
    seed(engine.workspace(), "src_a", 1000, &[("a", "x")]);

    // Preview (commit=false) twice → both show the full diff.
    let a = engine.diff_since_read("src_a", false, false).unwrap();
    let b = engine.diff_since_read("src_a", false, false).unwrap();
    assert_eq!(a.summary.added, 1);
    assert_eq!(b.summary.added, 1, "marker was not advanced");
}

#[test]
fn mark_read_advances_marker_for_explicit_sources() {
    let (engine, _dir) = engine();
    seed(engine.workspace(), "src_a", 1000, &[("a", "x")]);

    let marked = engine.mark_read(&["src_a".to_string()]).unwrap();
    assert_eq!(marked, 1);

    // After marking, a read shows no changes (marker already at head).
    let diff = engine.diff_since_read("src_a", false, false).unwrap();
    assert_eq!(diff.summary.added, 0);
    assert!(diff.changes.is_empty());
}

#[test]
fn mark_read_skips_sources_without_snapshots() {
    let (engine, _dir) = engine();
    seed(engine.workspace(), "src_a", 1000, &[("a", "x")]);
    let marked = engine
        .mark_read(&["src_a".to_string(), "src_missing".to_string()])
        .unwrap();
    assert_eq!(marked, 1, "only src_a had a snapshot");
}

// ── checkpoints ─────────────────────────────────────────────────────────

#[test]
fn diff_since_checkpoint_aggregates_across_sources() {
    let (engine, _dir) = engine();
    // Baseline snapshots for two sources, grouped into a checkpoint.
    let a1 = seed(engine.workspace(), "src_a", 1000, &[("a", "x")]);
    let b1 = seed(engine.workspace(), "src_b", 1000, &[("b", "y")]);
    {
        let ledger = Ledger::open(engine.workspace()).unwrap();
        ledger
            .create_checkpoint("ckpt_1", "base", &[a1.id.clone(), b1.id.clone()], 1500)
            .unwrap();
    }

    // src_a gets a new head with a modification; src_b unchanged.
    seed(engine.workspace(), "src_a", 2000, &[("a", "x v2")]);

    let cross = engine.diff_since_checkpoint("ckpt_1", false).unwrap();
    assert_eq!(cross.summary.modified, 1, "src_a 'a' modified");
    assert_eq!(
        cross.per_source.len(),
        1,
        "only src_a changed; unchanged src_b is skipped"
    );
    assert_eq!(cross.per_source[0].source_id, "src_a");
}

#[test]
fn create_checkpoint_baselines_lacking_sources() {
    let mut items = InMemoryItemSource::new();
    items.set_source("src_a", &[("a", "alpha")]);
    items.set_source("src_b", &[("b", "beta")]);
    let (engine, _dir) = engine_with(items);

    let ckpt = engine
        .create_checkpoint("baseline", &[src("src_a"), src("src_b")])
        .unwrap();
    assert!(ckpt.id.starts_with("ckpt_"));
    assert_eq!(ckpt.label, "baseline");
    assert_eq!(ckpt.snapshot_ids.len(), 2, "both sources baselined");

    let listed = engine.list_checkpoints(10).unwrap();
    assert_eq!(listed.len(), 1);
    assert_eq!(listed[0].id, ckpt.id);

    // No changes since the checkpoint → empty cross-source diff.
    let cross = engine.diff_since_checkpoint(&ckpt.id, false).unwrap();
    assert!(cross.per_source.is_empty());
}

#[test]
fn cleanup_removes_old_checkpoints() {
    let (engine, _dir) = engine();
    seed(engine.workspace(), "src_a", 1000, &[("a", "x")]);
    {
        let ledger = Ledger::open(engine.workspace()).unwrap();
        ledger
            .create_checkpoint("ckpt_old", "old", &[], 100)
            .unwrap();
        ledger
            .create_checkpoint("ckpt_new", "new", &[], i64::MAX)
            .unwrap();
    }
    // Everything older than 0 days ago (i.e. now) except the far-future tag.
    let deleted = engine.cleanup(0).unwrap();
    assert_eq!(deleted, 1, "only the old checkpoint is pruned");
    let remaining = engine.list_checkpoints(10).unwrap();
    assert_eq!(remaining.len(), 1);
    assert_eq!(remaining[0].id, "ckpt_new");
}
