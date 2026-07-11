# Test Spec — Summary-Tree, Archivist & Conversation Concurrency

Subsystem: `src/memory/tree/`, `src/memory/archivist/`, `src/memory/conversations/`.
Source audit: [`docs/spec/audit/03-tree-archivist-conversations.md`](../audit/03-tree-archivist-conversations.md).
Plan cross-reference: [`docs/spec/improvement-plan.md`](../improvement-plan.md) Phase 1
items 1–3, 6 and the "Cross-cutting: test strategy" section.

This spec covers exactly the gap that audit 03 calls out by name: **zero
concurrency tests, zero crash-recovery tests** anywhere in `tree/`,
`archivist/`, or `conversations/`. Every case below is either a regression
test for a numbered `TR-*` finding or new coverage for an interleaving/crash
window the audit flags as untested. Cases are written against the *current*
code so their given/when/then is concrete; cases that regress a finding are
expected to be **red** until the corresponding improvement-plan fix lands, and
green afterwards (per the plan's "every fix lands with a test that fails
before the change" rule).

## 1. Harness & fixtures

### 1.1 Reused, already-present

- `test_config() -> (TempDir, MemoryConfig)` — the per-test temp workspace
  pattern used verbatim in `bucket_seal_tests.rs`, `flush_tests.rs`,
  `archivist/store_tests.rs`. Every new test file in this spec defines its own
  copy (matches existing convention; no shared crate-level fixture module
  exists today).
- `ConcatSummariser` (`tree::summarise::ConcatSummariser`) — deterministic
  `Summariser` for tests that don't need to control timing.
- `seed_chunk` / `mk_leaf` helpers from `bucket_seal_tests.rs` and
  `flush_tests.rs` — persist a real chunk row and build the matching `LeafRef`
  so `hydrate_leaf_inputs` succeeds. Poison-buffer cases deliberately *skip*
  `seed_chunk` to build an unhydratable buffer.
- `upsert_buffer_tx` / `store::get_buffer` — used directly (as
  `flush_tests.rs::flush_does_not_force_seal_under_fanout_upper_buffer`
  already does) to hand-construct buffer states that would be hard to reach
  through the public `append_leaf` path alone (stale L1 buffers, buffers
  referencing dangling ids, buffers pre-seeded to straddle a seal boundary).

### 1.2 New: blocking summarisers (the seal-vs-append interleaving primitive)

Both trees in scope have their own `Summariser` trait
(`tree::summarise::Summariser` for summary trees, `tree::runtime::engine::Summariser`
for the time-tree). Neither has a test double that can pause mid-call, which
is required to reproduce TR-1/TR-11 deterministically (real races are
timing-dependent and flaky). Add two small fakes, colocated with the trait
they implement, `#[cfg(test)]`-gated:

```rust
// tree/summarise.rs, #[cfg(test)] pub(crate) mod test_support (or a sibling
// blocking_tests.rs colocated with bucket_seal_tests.rs — implementer's call,
// but it must be reachable from bucket_seal_tests.rs, flush_tests.rs, and
// engine_tests.rs without duplicating the type).
pub(crate) struct BlockingSummariser {
    inner: ConcatSummariser,
    // Signalled once `summarise()` has been entered — i.e. once the caller
    // has snapshotted the buffer and released any lock — so the driving test
    // knows it is safe to run the "concurrent" side of the race.
    entered: tokio::sync::mpsc::Sender<()>,
    // Awaited before returning the summary, so the driving test controls
    // exactly when the seal transaction resumes.
    release: tokio::sync::Mutex<tokio::sync::mpsc::Receiver<()>>,
}
```

`summarise()` sends on `entered`, then `.recv()`s on `release` before
delegating to `inner`. The test harness owns both channel halves:

```rust
let (entered_tx, mut entered_rx) = mpsc::channel(1);
let (release_tx, release_rx) = mpsc::channel(1);
let summariser = BlockingSummariser::new(entered_tx, release_rx);

let a = tokio::spawn(async move { append_leaf(&cfg_a, &tree_a, &leaf1, &summariser, &strategy).await });
entered_rx.recv().await;               // A is now inside seal_one_level's await,
                                        // buffer already snapshotted, no lock held
append_to_buffer(&cfg, &tree.id, 0, "c3", ...).unwrap(); // B "wins" the race
release_tx.send(()).await.unwrap();    // let A's seal transaction proceed
let sealed = a.await.unwrap().unwrap();
```

This is the single primitive nearly every case in §2–§3 is built on. An
equivalent `BlockingEngineSummariser` wraps the time-tree's
`runtime::engine::Summariser` (its `summarise(system, content)` signature
differs) for the §5 rebuild/propagation cases.

### 1.3 New: two-task interleaving helper

A thin wrapper around `tokio::spawn` + the channel dance above so individual
tests don't hand-roll it:

```rust
/// Runs `driver` to completion, but pauses it at its first `summarise()` call
/// (via a `BlockingSummariser`) until `during` has run, then resumes it.
/// Returns `(driver_result, during_result)`.
async fn interleave<A, B>(driver: impl Future<Output = A>, during: impl Future<Output = B>) -> (A, B)
```

Lives beside the blocking summarisers; used by every "task A / task B" case
below so the given/when/then in §2–§3 can talk about "A" and "B" without
repeating the plumbing.

### 1.4 New: poison-buffer builder

A helper that inserts a buffer row whose `item_ids` reference chunk/summary
ids that do not exist (simulating deleted chunks or archivist synthetic
leaves per TR-5):

```rust
fn seed_poison_buffer(cfg: &MemoryConfig, tree_id: &str, level: u32, dangling_ids: &[&str]);
```

Implemented with `upsert_buffer_tx` directly, mirroring the existing
hand-rolled buffer construction in `flush_tests.rs`.

### 1.5 New: crash-injection seam for `rebuild_tree` (TR-2)

`rebuild_tree` today is one monolithic `async fn` with no fault-injection
point between "delete old tree" and "rewrite leaves", and no way to observe
the `tree_buffer_backup` state mid-flight. This is genuinely new
infrastructure — the improvement plan calls it out explicitly ("Phase 1 …
new test infrastructure: a crash-injection harness for file writes"). Two
options, either is acceptable, and the fix (TR-2) should land with one of
them:

1. **Preferred (matches the planned fix):** once `rebuild_tree` is rewritten
   to build into a sibling temp dir and atomically rename over the old tree,
   split it into `prepare_rebuild` (does everything up to the rename) and
   `commit_rebuild` (the rename). Tests call `prepare_rebuild` alone and then
   assert on-disk state *without* calling `commit_rebuild` — that alone
   simulates "crashed before rename" with no extra test-only hook.
2. **Fallback:** a `#[cfg(test)]` `rebuild_tree_with_fault(cfg, summariser,
   namespace, fault: RebuildFault)` entry point, where `RebuildFault` is a
   small enum (`AfterBufferBackup`, `AfterDelete`, `AfterLeafRewrite`,
   `AfterDayPropagation`) that returns early (simulating a crash) at the named
   point, leaving whatever on-disk state that step produced.

Cases in §5 are written against option 1's shape (`prepare_rebuild` /
`commit_rebuild`) since it requires no test-only code path in the production
binary; note inline where a case needs the fallback shape instead.

### 1.6 New: multi-thread race driver for `record_turn` (TR-6)

`archivist::store::record_turn` is synchronous and file-based (no async
await, no shared lock), so its race is a genuine OS-thread race, not an
`.await`-point race. Use `std::sync::Barrier` to line up N threads at the
`next_seq` read:

```rust
fn concurrent_record_turn(cfg: &MemoryConfig, session: &str, n: usize) -> Vec<Result<ArchivedTurn>> {
    let barrier = Arc::new(std::sync::Barrier::new(n));
    std::thread::scope(|s| {
        (0..n).map(|i| {
            let barrier = barrier.clone();
            s.spawn(move || { barrier.wait(); record_turn(cfg, turn(session, "user", &format!("t{i}"))) })
        }).collect::<Vec<_>>().into_iter().map(|h| h.join().unwrap()).collect()
    })
}
```

This does not guarantee every thread reads `next_seq` at the exact same
instant, but with `N >= 8` on a shared directory it reproduces the collision
in practice; treat occasional non-collision as a pass on an already-safe
build, not a flaky failure (assert the *invariant* — no two returned `seq`
values equal *and* no turn silently vanished — rather than asserting a
collision occurred).

## 2. Not in scope (belongs to other spec docs)

To avoid overlap with sibling test-spec documents covering the same audit:

- Ranking/ordering correctness (TR-13 cosine-vs-token-overlap, TR-16
  lexicographic timestamp ordering, deleted-node filtering in `read_tree`) —
  belongs to a `tree-read-ranking-tests.md` spec. Not a concurrency concern.
- Front-matter/YAML round-trip corruption (TR-14a/b/c) and id-collision-by-
  concatenation (TR-15) — content-integrity bugs, not concurrency; belongs to
  an `archivist-content-integrity-tests.md` / `conversations-identity-tests.md`
  spec. `seed_poison_buffer` and `record_turn` fixtures here are reused there,
  but the round-trip assertions themselves are out of scope for this doc.
- Label-value *semantics* (TR-8's `Some(["general"])` always overriding) is a
  sequential correctness bug with no interleaving required — a single-threaded
  regression test belongs in `conversations/bus_tests.rs` directly, filed
  under a content-correctness spec, not here. This doc covers only the
  concurrency-shaped sub-case (§7): two writers racing on the dedup check.
- Inverted-index trigram fallback (TR-18) and LLM-score gating (RS-1, out of
  audit 03 scope entirely) — unrelated subsystems.
- Queue/job-level concurrency (QI-*) — covered by a queue-ingest concurrency
  spec, not this one, even where a queued job eventually calls into
  `append_leaf_deferred`.

## 3. Test cases

Priority: **P0** = regression test for a Critical/Major `TR-*` finding in
scope. **P1** = Minor finding or explicitly-named test-coverage gap. **P2** =
defense-in-depth / nice-to-have hardening the audit didn't ask for by name.

### 3.1 Seal-vs-append races (TR-1)

| ID | Name | Given | When | Then | Findings | Pri |
| --- | --- | --- | --- | --- | --- | --- |
| TCT-001 | seal_survives_concurrent_append_below_snapshot | L0 buffer holds `{c1,c2}` at the token-budget boundary; a `BlockingSummariser` is wired in for the seal it will trigger | task A appends the leaf that crosses budget (entering `seal_one_level`, buffer snapshot = `{c1,c2,c3}`) and blocks at `summarise()`; task B (nothing to append — this is the baseline no-race case) simply releases A | A's seal completes; summary `child_ids == {c1,c2,c3}`; L0 buffer ends empty | TR-1 (baseline) | P0 |
| TCT-002 | append_during_seal_is_not_lost | L0 buffer at budget boundary with `{c1,c2}`; `BlockingSummariser` wired | task A's `append_leaf(c2)` crosses budget and blocks inside `seal_one_level` after snapshotting `{c1,c2}`; while blocked, task B calls `append_to_buffer(tree, 0, "c3", ...)`; release A | after both complete: `c3` is present in **some** buffer or summary reachable from the tree (not silently dropped) — assert `store::get_buffer(L0)` contains `c3` OR a summary's `child_ids` contains `c3` | **TR-1 (the exact failure scenario)** | P0 |
| TCT-003 | double_cascade_produces_no_duplicate_summary | L0 buffer at budget boundary; two independent cascade entry points both eligible to seal it (`append_leaf` and `flush_stale_buffers` racing the same buffer) | task A runs `cascade_all_from(..., force_now=None, ...)` via a blocking summariser; task B runs `cascade_all_from(..., force_now=Some(now), ...)` on the same `(tree, level)` concurrently, released after A snapshots | at most one summary is created over `{c1,c2}`; `store::count_summaries == 1`; no summary has an empty/subset `child_ids` from a second partial seal | **TR-1 (double cascade)** | P0 |
| TCT-004 | double_cascade_does_not_duplicate_child_backlinks | Same setup as TCT-003 | both cascades run to completion (blocking summariser released for both) | `mem_tree_chunks.parent_summary_id` for `c1`/`c2` points at exactly one summary id (the `parent_id IS NULL` guard prevents a second UPDATE, but the audit warns this "silently masks the duplicate" summary row itself) | TR-1 | P0 |
| TCT-005 | append_after_seal_starts_new_buffer_not_stale_one | L0 buffer sealed with `{c1,c2}`, seal in flight (blocked in summariser) | task B appends `c3` while A is blocked; A releases and completes | after A commits, `c3` lives in a **fresh** L0 buffer (post-seal state), not merged into A's already-sealed summary and not deleted; `store::get_buffer(L0).item_ids == ["c3"]` | TR-1 | P0 |
| TCT-006 | two_concurrent_full_appends_both_survive | Two independent leaves `c1`, `c2`, each individually below budget but together crossing it | task A and task B call `append_leaf` concurrently (unblocked, real timing) for `c1` and `c2` respectively, repeated 50x under `#[tokio::test(flavor = "multi_thread")]` | across all repetitions: `store::count_summaries + get_buffer(L0).item_ids.len()`-derived total item count always equals 2; no run loses an id | TR-1 | P0 |
| TCT-007 | seal_reread_verifies_snapshot_prefix | L0 buffer `{c1,c2}` snapshotted by task A; task B appends `c3` and **also races a second seal** before A resumes (simulating the fixed behavior) | after the TR-1 fix lands: A's seal transaction re-reads the buffer, finds it no longer starts with `{c1,c2}` exactly (extra item present), and takes the documented recovery path (retry sealing the current buffer / abort-and-retry) rather than clobbering | on fixed code: no data loss and no panic; on current code: **expected red**, documents the pre-fix clobber for reviewers | TR-1 | P0 |
| TCT-008 | seal_set_difference_preserves_other_pending_items | L0 buffer `{c1,c2,c3}`; task A snapshots `{c1,c2}` (an earlier append triggered the seal before `c3` was added — construct via direct buffer manipulation) | A's seal completes | post-seal L0 buffer contains exactly `{c3}` (set-difference), not empty | TR-1 (fix acceptance criterion) | P0 |
| TCT-009 | concurrent_seal_of_disjoint_trees_is_independent | Two distinct trees (`tree_a`, `tree_b`), each with an L0 buffer at budget boundary | task A seals `tree_a`, task B seals `tree_b` concurrently via blocking summarisers released in reverse order | both seal independently; no cross-tree interference; `count_summaries` is 1 for each tree | new coverage | P2 |
| TCT-010 | seal_vs_append_repeated_stress | L0 buffer under budget | 200 iterations of: spawn one `append_leaf` per iteration from a pool of 4 concurrent tasks pushing distinct leaves until budget crosses multiple times (multi-level cascades included) | final total leaf count across all sealed summaries' `child_ids` (deduplicated) plus any remaining buffer items equals the number of leaves appended; no `anyhow` errors surfaced | TR-1 | P1 |

### 3.2 Double cascade beyond TR-1 (TR-11, time-tree engine)

| ID | Name | Given | When | Then | Findings | Pri |
| --- | --- | --- | --- | --- | --- | --- |
| TCT-011 | retry_after_partial_propagation_does_not_double_fold | Time-tree buffer has entries for one hour; `run_summarization` writes the hour leaf, then a `BlockingEngineSummariser` is configured to fail on the **day**-level `propagate_node` call only | `run_summarization` runs once (day propagation fails, `failed` non-empty, buffer NOT cleared per the "only clear on full success" rule), then runs a second time with a working summariser | the day node's summary reflects each buffered entry exactly once, not twice (TR-11: "folds the same buffered entries into hour nodes twice") | **TR-11** | P0 |
| TCT-012 | retry_after_hour_leaf_failure_is_idempotent | Buffer has entries for one hour; summariser fails on the **hour**-level call itself | `run_summarization` runs, fails, buffer stays intact; runs again with a working summariser | exactly one hour leaf exists with content folded once; no duplicate `\n\n---\n\n`-joined entry appears twice in the hour summary | TR-11 | P0 |
| TCT-013 | per_node_failure_does_not_swallow_diagnostics | Day-level propagation fails (summariser error) | `run_summarization` completes | the failure is observable to the caller (not just `_e` discarded) — assert the improvement-plan-required surfaced error/count, e.g. a returned `Vec<String>` of failed node ids or a non-`Ok(())` signal; **expected red** against current `_e` swallow | TR-11 | P0 |
| TCT-014 | concurrent_run_summarization_same_namespace_no_double_seal | Buffer has entries for one hour | two tasks call `run_summarization` concurrently on the same namespace (blocking engine summariser to force overlap during the hour-leaf write) | exactly one hour leaf is produced (or, if both proceed, backlink/idempotency guard prevents duplicate propagation) — document via assertion whichever the eventual fix guarantees; **new coverage**, currently unguarded (no per-namespace lock exists) | new coverage (adjacent to TR-1's shape) | P1 |
| TCT-015 | day_propagation_race_does_not_double_count_child_count | Multiple hour leaves for the same day, propagated concurrently by two racing `run_summarization` calls | both complete | day node's `child_count` / summary reflects the true set of hour leaves once each, not duplicated per race | new coverage | P1 |

### 3.3 Poison buffers (TR-4, TR-5, TR-10)

| ID | Name | Given | When | Then | Findings | Pri |
| --- | --- | --- | --- | --- | --- | --- |
| TCT-016 | poison_buffer_bails_seal | `seed_poison_buffer` creates an L0 buffer whose only item id references a deleted/never-inserted chunk, at budget | `seal_one_level` (via `cascade_all_from`) is invoked | returns `Err` ("refused to seal empty buffer") — documents current behavior | TR-4 (setup) | P0 |
| TCT-017 | one_poison_tree_does_not_block_flush_of_healthy_trees | Two trees: `tree_a` has a genuinely stale, healthy L0 buffer (real chunk, past `max_age`); `tree_poison` has a stale L0 buffer built via `seed_poison_buffer`, and sorts first by `oldest_at ASC` | `flush_stale_buffers(cfg, max_age, ...)` runs across both | `tree_a` is still sealed (its summary count increments) despite `tree_poison` failing; **expected red** on current code (`?` propagates the first error and aborts the loop before reaching `tree_a`) | **TR-4** | P0 |
| TCT-018 | flush_stale_buffers_reports_per_tree_errors | Same two-tree setup as TCT-017 | `flush_stale_buffers` runs | the poisoned tree's failure is collected/reported (not just discarded), e.g. as part of a richer return type; assert on whatever the fix's contract is (a `Vec<(tree_id, Error)>` or count) | TR-4 (fix acceptance) | P0 |
| TCT-019 | poisoned_buffer_gets_quarantined_not_retried_forever | `tree_poison`'s buffer is unhydratable and stale | `flush_stale_buffers` runs twice in a row | after the fix: the second run does not re-attempt (and re-fail) the same poison buffer indefinitely — assert it is marked/cleared/quarantined per the improvement-plan fix ("clear/quarantine buffers whose items are all unhydratable") | TR-4 | P0 |
| TCT-020 | archivist_leaf_append_poisons_tree_before_fix | A tree-backed `TreeLeafSink` is wired to append an archivist chunk id (`archivist:<hash>`) as an L0 leaf, crossing budget | the resulting seal attempt runs | hydration skips the archivist id (`hydrate_leaf_inputs` finds no chunk row), buffer ends up unhydratable, seal bails — reproduces "the first over-budget L0 buffer poisons the tree" end-to-end; this test also stands as the integration test TR-5 says is "impossible today" once a real sink exists | **TR-5** | P0 |
| TCT-021 | archivist_poisoned_tree_stalls_all_flushes_transitively | Setup from TCT-020 plus a second, healthy tree with a stale buffer | `flush_stale_buffers` runs | on current code: healthy tree never seals because the archivist-poisoned tree sorts first and the `?` aborts (TR-5 → TR-4 chain); on fixed code: healthy tree seals regardless | TR-5, TR-4 | P0 |
| TCT-022 | hydration_hole_mid_buffer_seals_only_present_children | L0 buffer holds `{c1, c2, c3}`; `c2` is deleted from the chunk store after being buffered but before seal (simulating TR-10's "hydration skips missing children") | seal runs | summary is produced from `{c1, c3}` (2 hydrated inputs); **assert `child_ids == [c1, c3]`, not `[c1, c2, c3]`** — expected red against current code, which stores `buf.item_ids` wholesale | **TR-10** | P0 |
| TCT-023 | hydration_hole_time_range_and_score_match_hydrated_only | Same setup as TCT-022 | seal runs | `time_range_start`/`time_range_end`/`score` are derived only from the 2 hydrated inputs (already true today, since these come from `inputs` not `buf.item_ids`) — regression guard so a future refactor doesn't couple them back to the raw id list | TR-10 (adjacent regression guard) | P1 |
| TCT-024 | all_children_missing_is_indistinguishable_from_poison | L0 buffer holds only ids that are all missing (same shape as TCT-016 but reached via post-buffering deletion instead of never-existing ids) | seal runs | same "refused to seal empty buffer" error path as TCT-016 — confirms TR-4's fix (quarantine) must key off "zero hydrated inputs", not "which specific ids were bad" | TR-4 | P1 |
| TCT-025 | poison_buffer_flush_interleaved_with_healthy_append | `tree_poison` stale+unhydratable; `tree_a` healthy, under budget (not yet stale) | `flush_stale_buffers` runs (touches only `tree_poison`) concurrently with a live `append_leaf` on `tree_a` crossing its own budget | `tree_a`'s live append-triggered seal is unaffected by the concurrent flush failure on `tree_poison` | TR-4 (concurrency + isolation combined) | P1 |

### 3.4 `rebuild_tree` crash windows (TR-2, TR-7)

| ID | Name | Given | When | Then | Findings | Pri |
| --- | --- | --- | --- | --- | --- | --- |
| TCT-026 | rebuild_crash_between_delete_and_rewrite_loses_tree | Time-tree namespace with several hour/day/month/year/root nodes on disk | `store::delete_tree` is invoked directly (simulating the crash point) and the process "stops" before any `write_node` calls run (no `commit_rebuild` / no leaf rewrite) | on current code: every summary in the namespace is gone, `get_tree_status` reports zero nodes — **this is the documented catastrophic failure mode**, test asserts it reproduces (red is the point: it documents current risk until TR-2's temp-dir-plus-rename fix lands, after which this exact sequence becomes unreachable because the delete only happens post-rename) | **TR-2 (critical)** | P0 |
| TCT-027 | rebuild_into_temp_dir_leaves_original_untouched_until_rename | Same namespace as TCT-026; fixed implementation shape (`prepare_rebuild` writes into a sibling temp dir) | `prepare_rebuild` runs to completion but `commit_rebuild` (the atomic rename) is never called (simulated crash) | the *original* tree dir is byte-for-byte unchanged — every original node still reads back correctly via `read_node` | TR-2 (fix acceptance criterion) | P0 |
| TCT-028 | rebuild_commit_is_atomic_rename | Same as TCT-027, `prepare_rebuild` succeeded | `commit_rebuild` runs | the tree dir now reflects the rebuilt content in a single filesystem operation — no window where a `read_node` call in a concurrent reader thread sees a half-old/half-new tree (poll via a background thread reading `read_node("root")` in a tight loop during commit; every read is either fully-old or fully-new) | TR-2 (fix acceptance) | P0 |
| TCT-029 | buffer_backup_orphan_adopted_on_startup | `rebuild_tree` renames the buffer dir to `tree_buffer_backup`, then the simulated crash happens before the restore-rename runs | the engine's startup/init path runs (per the fix: "adopt a leftover `tree_buffer_backup` on startup") | the orphaned `tree_buffer_backup` is detected and restored to the live buffer path — buffered content is not permanently orphaned; **expected red** until the adoption routine exists | **TR-2** | P0 |
| TCT-030 | buffer_backup_orphan_survives_repeated_startup_without_duplication | Orphaned `tree_buffer_backup` exists (as in TCT-029) | startup adoption runs twice in a row (idempotency check) | second run is a no-op (no duplicate buffer content, no error) since the backup dir no longer exists after the first adoption | TR-2 (adjacent) | P1 |
| TCT-031 | rebuild_preserves_unsummarised_buffer_content_happy_path | Namespace has hour/day/month/year/root nodes plus a non-empty *live* (not backup) buffer | `rebuild_tree` runs successfully end-to-end (no simulated crash) | the live buffer's content is present after rebuild, unchanged, and hour leaves are correctly rewritten from the in-memory `hour_leaves` vec | TR-2 (happy-path regression guard, currently covered informally — make it explicit) | P1 |
| TCT-032 | node_write_truncated_by_crash_is_silently_corrupted_today | A time-tree node file is partially written (simulate by writing a truncated byte prefix of a valid front-matter+body file directly to the node path, bypassing `write_node`) | `parse_node_markdown` reads it back | every field defaults silently (no error) — reproduces TR-7's "the corruption is silent and gets baked into future re-summarisation" | **TR-7** | P0 |
| TCT-033 | node_write_uses_atomic_temp_plus_rename | `write_node` fixed per TR-7 (reuses `write_if_new`'s temp+rename contract) | `write_node` is called while a concurrent reader polls `read_node` on the same node id in a tight loop | the reader never observes a partially-written file — every read is either the previous content or the fully-new content, never a truncated prefix; **expected red** on current `std::fs::write`-based code (a large enough payload plus injected I/O delay can be observed as partial under current impl — if not reliably reproducible via real timing, assert indirectly: the final path is never opened for direct writing, only via `rename`, checkable by intercepting `std::fs::write` calls is not feasible in-process, so this case is written as a code-path assertion once the fix lands: `write_node`'s implementation goes through `write_if_new`/an equivalent temp-file helper) | **TR-7** | P0 |
| TCT-034 | rebuild_after_crash_mid_leaf_rewrite_is_recoverable | Simulated crash: `delete_tree` ran, some but not all hour leaves were rewritten via `write_node` before "crash" | the fixed rebuild path (temp-dir-based) is re-run from scratch | rebuild is idempotent — re-running `rebuild_tree` from the in-memory `hour_leaves` (still available, since they were collected before delete) reconstructs the full tree with no missing or duplicated nodes | TR-2 | P1 |
| TCT-035 | rebuild_day_month_year_propagation_failure_is_partial_success | `rebuild_tree` runs with a summariser that fails for one specific month id | rebuild completes | day/year/root propagation for unrelated branches still succeeds (already true today per the `let _ = propagate_node(...)` pattern); the one failed month's node is left at its pre-rebuild content, not corrupted to empty — regression guard for existing partial-success behavior surviving the TR-2 fix | TR-2 (regression guard against fix regressing this) | P1 |

### 3.5 Archived-tree invariants (TR-9)

| ID | Name | Given | When | Then | Findings | Pri |
| --- | --- | --- | --- | --- | --- | --- |
| TCT-036 | append_leaf_rejected_on_archived_tree | A tree with `status = TreeStatus::Archived` (set directly via a store-level status update, since no public API sets it today — construct via SQL/`store` helper in the test) | `append_leaf` is called against it | call returns `Err` (or a documented no-op) instead of silently accepting the leaf; **expected red** — current `append_leaf` never checks `tree.status` | **TR-9** | P0 |
| TCT-037 | append_leaf_deferred_rejected_on_archived_tree | Same archived tree | `append_leaf_deferred` is called | same rejection as TCT-036 — both append paths must enforce the invariant, not just one | TR-9 | P0 |
| TCT-038 | cascade_all_from_rejected_on_archived_tree | Archived tree with a *pre-existing* non-empty buffer (e.g. archived mid-flight with an unsealed buffer) | `cascade_all_from` / `force_flush_tree` is called against it | seal is refused — an archived tree must not seal, even to clear a leftover buffer, until explicitly reactivated | **TR-9** | P0 |
| TCT-039 | get_or_create_tree_archived_scope_semantics_decided | `(kind, scope)` already has an *archived* tree row | `get_or_create_tree(kind, scope)` is called again (e.g. a new message arrives for a scope whose tree was archived) | behavior matches whatever the fix decides (audit says "decide get-or-create semantics for an archived tree" — either: (a) returns the archived tree unchanged and callers must check status themselves, or (b) creates a *new* Active tree for the same scope) — this test locks in whichever choice is made so a future change is a deliberate decision, not an accident | **TR-9** | P0 |
| TCT-040 | archived_tree_still_readable | Archived tree with existing summaries | `read_tree` / query paths are exercised | archived trees remain fully queryable (per the doc promise "archived trees don't accept new leaves" — implying they *do* stay queryable) — regression guard that the fix doesn't accidentally hide archived content from reads | TR-9 (regression guard) | P1 |
| TCT-041 | concurrent_archive_and_append_race | A tree transitions Active → Archived (status update) concurrently with an in-flight `append_leaf` call that started while still Active | both operations run concurrently (archive status update racing the append's buffer-read step) | the append either completes cleanly (started-before-archive semantics honored) or is cleanly rejected — no partial state where a leaf is buffered against an archived tree with no seal ever able to clear it | new coverage (concurrency-shaped extension of TR-9) | P1 |
| TCT-042 | flush_stale_buffers_skips_archived_trees | An archived tree has a stale, non-empty L0 buffer (archived mid-flight) alongside a healthy active tree also stale | `flush_stale_buffers` runs | the active tree is sealed; the archived tree's buffer is left alone (not force-sealed) — extends TR-9 to the flush path specifically, since `flush.rs` also never checks `tree.status` today | TR-9 | P0 |
| TCT-043 | reactivating_archived_tree_restores_append_ability | Archived tree; a (future) `reactivate_tree` or equivalent transitions it back to `Active` | `append_leaf` is called after reactivation | append succeeds normally — confirms the invariant check is a live status read, not a one-time gate baked in at tree-creation time | TR-9 (fix acceptance completeness) | P2 |

### 3.6 Archivist seq collisions (TR-6)

| ID | Name | Given | When | Then | Findings | Pri |
| --- | --- | --- | --- | --- | --- | --- |
| TCT-044 | concurrent_record_turn_no_silent_loss | Empty session directory | `concurrent_record_turn(cfg, "s1", 16)` races 16 threads via the barrier helper (§1.6) | every thread's `Ok(turn)` result has a **distinct** `seq`, and `session_entries` afterward returns exactly 16 turns — one per thread, none silently dropped; **expected red** on current code (TR-6: `write_if_new`'s `Ok(false)` on collision is discarded, so the loser's turn "vanishes while the function returns success") | **TR-6** | P0 |
| TCT-045 | concurrent_record_turn_reports_error_on_uncontested_collision_path | Same 16-thread race | at least one collision is forced deterministically: two threads are pinned (via a second, tighter barrier) to call `next_seq` before either has written, guaranteeing both compute the same seq | the losing thread's `record_turn` call returns `Err` (or internally retries and succeeds with a bumped seq) rather than `Ok` with silently-unwritten content — assert whichever contract the fix picks (audit suggests "O_EXCL-create and retry" or "treat `false` as an error") | TR-6 (fix acceptance) | P0 |
| TCT-046 | concurrent_record_turn_across_different_sessions_is_unaffected | Two sessions `s1`, `s2` | threads racing on `s1` run concurrently with threads racing on `s2` | no cross-session interference — `s1` and `s2` each end up with the correct, independent seq sequences (directories are disjoint, so this should already hold; regression guard against a future single-directory refactor) | new coverage | P2 |
| TCT-047 | record_turn_retry_after_collision_is_gap_free_or_documented | Two threads collide on seq `N` (as in TCT-045) | the fix's retry logic runs | resulting seq sequence for the session is either gap-free (`0..count`) or the gap policy is explicitly asserted (some designs accept a hole after a retry-abandoned attempt) — pick and lock in one behavior | TR-6 | P1 |
| TCT-048 | high_contention_stress_no_duplicate_seq_ever | 64 threads hammer `record_turn` on one session | all complete | across the full 64-turn set, `session_entries` has 64 distinct, monotonically-increasing (as read back sorted) seq values with matching content — stress variant of TCT-044 | TR-6 | P1 |
| TCT-049 | concurrent_record_turn_survives_directory_not_yet_created | Session directory does not exist yet (first-ever turn for the session) | N threads all call `record_turn` for a brand-new session simultaneously (racing `fs::create_dir_all` too) | `create_dir_all` is idempotent so this doesn't itself lose data, but combined with the seq race (TCT-044) the same invariant must hold on a cold session, not just a warm one | TR-6 (edge case) | P1 |
| TCT-050 | sequential_record_turn_regression_guard | Single-threaded, sequential calls (no race) | 5 turns recorded one after another | seq is exactly `0,1,2,3,4` — baseline regression guard that the concurrency fix (O_EXCL + retry, or locking) doesn't change the well-behaved sequential contract already tested in `store_tests.rs::append_increments_seq` | TR-6 (non-regression) | P2 |

### 3.7 Conversation-store crash windows & writer concurrency (TR-8-adjacent, TR-17)

| ID | Name | Given | When | Then | Findings | Pri |
| --- | --- | --- | --- | --- | --- | --- |
| TCT-051 | concurrent_append_message_dedup_holds_under_lock | A channel turn's `persisted_message_id` is about to be appended by two threads simultaneously (same `thread_id`, same `descriptor.message_id`) | both threads call `persist_channel_turn` concurrently | exactly one message with that id is persisted — `CONVERSATION_STORE_LOCK` serializes the dedup-check-then-append sequence, so this should already pass; written as an explicit regression guard against a future change that narrows the lock's scope (e.g. moving the dedup read outside the guard for performance) | new coverage (protects against reintroducing a TR-8-shaped race) | P1 |
| TCT-052 | crash_between_message_append_and_stats_event_skews_count | A message is appended to `thread_messages_path`; the process is simulated to crash before the paired `ThreadLogEntry::MessageAppended` entry is written to `threads.jsonl` (call the message-append half directly, skip the stats-event append) | `list_threads` / thread stats are read afterward | `message_count`/`last_message_at` for the thread under-counts (stale) — reproduces TR-17; then assert the backfill path: per the finding, backfill "only fires when count is `None`", so a *second* message append after the simulated crash does **not** self-heal the undercount | **TR-17** | P0 |
| TCT-053 | crash_between_message_append_and_stats_event_recoverable_after_fix | Same simulated-crash setup as TCT-052 | the eventual fix (single atomic append, or startup reconciliation) runs | `message_count` matches the true number of messages in `thread_messages_path` regardless of the crash window | TR-17 (fix acceptance) | P1 |
| TCT-054 | concurrent_ensure_thread_creation_race | Thread `t1` does not exist yet | two threads call `ensure_thread` for the same new `thread_id` concurrently | both calls succeed (idempotent upsert), and exactly one thread row exists afterward with consistent `created_at` — regression guard for the lock already providing this | new coverage | P2 |
| TCT-055 | concurrent_append_message_preserves_order_within_thread | A thread exists; N threads each append one message concurrently | all N `append_message` calls complete | `get_messages` returns all N messages (no loss under the lock); ordering is stable append order per the JSONL log, not required to match wall-clock spawn order but must be internally consistent (matches `threads.jsonl` MessageAppended trail order) | new coverage (protects the "concurrent conversation writers" gap named in the audit) | P1 |
| TCT-056 | index_rebuild_race_does_not_lose_concurrent_append | `search_cross_thread_messages`'s documented "accepted tradeoff": a cold-path index rebuild racing a concurrent `append_message` may miss that one message until cache eviction | trigger a cold rebuild (first search) concurrently with an `append_message` on some thread | the append itself is never lost from `get_messages`/JSONL (only the *search index* may be transiently stale, per the documented tradeoff) — this test locks in that the tradeoff is scoped to search staleness only, not data loss | new coverage (documents an accepted, bounded risk — regression guard it doesn't widen) | P2 |

## 4. Coverage cross-check

| Finding | Cases |
| --- | --- |
| TR-1 | TCT-001–010 |
| TR-2 | TCT-026–031, TCT-034–035 |
| TR-3 / TR-12 | out of primary scope here — covered by Phase-0 fix's own regression test per the improvement plan (`force_flush_tree`/`seal_now` semantics); note only, no case number reserved since it is a sequential (non-concurrency) bug once the `force: bool` fix lands |
| TR-4 | TCT-016–019, TCT-021, TCT-024–025 |
| TR-5 | TCT-020–021 |
| TR-6 | TCT-044–050 |
| TR-7 | TCT-032–033 |
| TR-9 | TCT-036–043 |
| TR-10 | TCT-022–024 |
| TR-11 | TCT-011–015 |
| TR-17 | TCT-052–053 |

Every Critical and Major finding in audit 03 that is concurrency- or
crash-window-shaped has at least one P0 case above. TR-3/TR-12 are
intentionally left to the Phase-0 fix's own single-threaded regression test
(they are a dead-flag bug, not a race) — flagged here only so a reviewer
doesn't wonder why they're missing.
