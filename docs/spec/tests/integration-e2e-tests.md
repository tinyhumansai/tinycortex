# Feature-Test Spec — End-to-End Pipeline, Property Tests, Backend Conformance, CI Matrix

Cross-cutting scope: `tests/` (external integration-test crate), plus the new
shared test-support surface those integration tests need. This document does
**not** duplicate the per-module unit-test specs derived from
`docs/spec/audit/01`–`06` (see "Not in scope"); it covers only what can only be
exercised by driving the real pipeline end-to-end, by property-testing a
round-trip invariant across arbitrary inputs, or by running one scenario
against more than one storage backend / feature combination.

Findings referenced (`SC-*`, `RS-*`, `TR-*`, `QI-*`, `DS-*`, `CT-*`) are from
the six audit documents under `docs/spec/audit/`; the phased remediation is in
[`docs/spec/improvement-plan.md`](../improvement-plan.md); the backend design
is in [`docs/spec/configurable-store.md`](../configurable-store.md).

This document specifies test *intent*, not implementation. Cases marked
"pre-fix" describe a regression a fix must remove: the test should fail
against current `main` and pass once the cited improvement-plan item lands.
Cases marked "new coverage" exercise a path the audit found untested but not
necessarily broken.

## Harness & fixtures

### Why `tests/` needs its own support module

`tests/*.rs` files compile as separate crates linked against the built
`tinycortex` library — they cannot see `#[cfg(test)]` items defined inside
`src/` (e.g. `queue::test_support::RecordingDelegates`,
`score::mod_tests::FakeLlm`, the various `StubSummariser`s in
`tree/runtime/engine_tests.rs`). Those fixtures are correctly scoped to their
own module's unit tests per the repo's `*_tests.rs` convention, but an
end-to-end test needs equivalents that are visible from outside the crate.

**New infrastructure required:** a `memory::test_support` module gated by
`#[cfg(any(test, feature = "test_support"))]` (new non-default feature,
dependency-light — no new crates pulled in) exposing:

- `TestSummariser` — deterministic `Summariser` impl. Default mode renders
  `"summary(level={level}, n={buffer_len}): {first_40_chars_of_each_item}"` so
  a test can assert content actually propagated up a level without depending
  on real LLM output. Configurable to block on a `tokio::sync::Notify` (or a
  `std::sync::mpsc` handoff under `--no-default-features`, since `Notify`
  needs the `tokio`/`sync` feature) so a test can pause a seal mid-flight —
  this is the "two-task interleaving helper" the improvement plan's test
  strategy calls for, needed by TR-1/QI-4 cases below.
- `TestEmbedder` — deterministic `Embedder` impl producing a small fixed-dim
  (e.g. 8) vector from a stable hash of the input text, so cosine-similarity
  ordering is reproducible across runs and platforms. (`InertEmbedder`
  already exists in `score/embed.rs` but always returns the same/empty
  vector, so every hit ties — useless for ranking assertions.)
- `TestLlmExtractor` — deterministic entity extractor with three modes:
  `WithImportance(f32)`, `DefaultingSoftFail` (mirrors the real
  `LlmEntityExtractor`'s never-`Err` contract — models RS-1's provider-outage
  case), and `OmitsImportance` (`Ok` with `llm_importance: None`).
- `RealQueueDelegates` — **new**, and the most load-bearing addition here.
  Today the only `QueueDelegates` implementor in the crate is the test-only
  `RecordingDelegates`, which fakes every step instead of calling the real
  `chunks`/`score`/`tree`/`retrieval` modules. There is no reference "this is
  how a host wires the queue to the real engine" implementation anywhere —
  which is itself a gap the improvement plan's Phase 3 (`MemoryBackend`) is
  meant to close. Until that lands, this suite needs its own
  `RealQueueDelegates` that calls the actual `score::score_chunk`,
  `tree::append_leaf`/`cascade_all_from`, `tree::seal_document`, and
  `store::vectors` re-embed path against a real tempdir-backed `MemoryConfig`,
  parameterized by `TestSummariser`/`TestEmbedder`/`TestLlmExtractor`. This
  doubles as living documentation of the intended production wiring and
  should be written once, shared by every E2E case.
- `drain_queue(cfg, delegates, max_iters)` — loops `queue::worker::run_once`
  until it returns "no ready job" or `max_iters` is hit (fail the test on
  hitting the cap, to catch infinite-requeue regressions like QI-2); a
  `drain_queue_with_tick` variant that also calls
  `enqueue_flush_stale`/`self_heal`/`recover_stale_locks` between drains, for
  cases that depend on the scheduler tick (QI-6, QI-9).

### Temp workspace fixture

Every case gets its own `TempDir` + `MemoryConfig::new(tmp.path())`, following
the existing `test_config()` pattern used throughout `src/`
(`ingest/pipeline_tests.rs`, `tree/*_tests.rs`). No shared global state between
cases (SQLite connections are cached per-path — see SC-22 — so distinct
tempdirs also give test isolation for free).

### "Process restart" simulation (in lieu of byte-level crash injection)

True fsync-interruption / partial-write crash injection needs a `Filesystem`
trait seam that does not exist in the crate today (all write paths call
`std::fs` directly). Introducing that seam is implementation work belonging to
the improvement plan's Phase 0/1 items, not this test spec. What this suite
*can* do without a new seam, and what the cases below rely on for
crash-adjacent coverage:

- **Logical mid-pipeline abort**: run the pipeline up to a specific step (gate
  claimed, chunks staged, job enqueued, etc.), stop driving it (drop the
  `RealQueueDelegates`/drain loop), then construct a **fresh**
  `MemoryConfig`/connection pool over the *same* tempdir path (simulating a
  process restart against on-disk state) and assert the recovery/retry
  behavior from there. This exercises QI-1, TR-1-adjacent, and QI-6/QI-10
  scenarios realistically because the on-disk artifacts (partial rows,
  orphaned locks, staged-but-uncommitted files) are real, even though the
  interruption point is chosen logically rather than at an arbitrary byte
  offset.
- **Deterministic interleaving** via the blocking `TestSummariser`/
  `TestLlmExtractor` notify-gate described above, driving two `tokio::test`
  tasks (`#[tokio::test(flavor = "multi_thread", worker_threads = 2)]`) with
  explicit `Notify::notified().await` / `notify_one()` handshakes so the
  "task A snapshots buffer, task B appends, A's transaction commits" race
  (TR-1) and the "two workers both call `LlmGate::acquire`" deadlock shape
  (QI-4) are reproduced deterministically instead of relying on timing.
- True byte-level fault injection (fail after N bytes, crash between rename
  steps) is **out of scope for this document** — see "Not in scope".

### Cargo changes this harness needs

- `[dev-dependencies] proptest = "1"` — for the property-test section below.
  No `proptest` usage exists in the crate today; add it as a dev-only
  dependency (does not affect the dependency-light default feature set).
- `[dev-dependencies] tokio = { version = "1", features = ["macros",
  "rt-multi-thread", "time", "sync"] }` — add `"sync"` so integration tests
  can use `tokio::sync::Notify`/`oneshot` for the interleaving helper without
  requiring the crate's own `tokio` feature to be enabled.
- New non-default feature `test_support = []` (or `test-support`, matching
  existing kebab-case feature names) gating `memory::test_support`, exercised
  by the CI feature-matrix job (see CI section) so it never silently
  bit-rots relative to `--all-features`.

## Test cases

### A. End-to-end: ingest → queue drain → seal → retrieval

| ID | Name | Given / When / Then | Findings | Priority |
|----|------|----------------------|----------|----------|
| E-01 | `happy_path_ingest_drain_seal_retrieve` | Given a tempdir engine wired to `RealQueueDelegates` + deterministic fakes, when a chat batch is ingested, the queue is drained, and a source-level L0 buffer seals, then a subsequent retrieval query for content in the batch returns a hit whose summary text contains the `TestSummariser`'s deterministic marker for that buffer. | new coverage (baseline) | P0 |
| E-02 | `readme_quickstart_equivalent_returns_a_hit` | Given the crate's README quick-start steps reproduced against the real engine (not `InMemoryMemoryStore`), when the documented example query runs, then it returns ≥1 hit — pins the corrected behavior once CT-1's term-based search fix lands, at the full-engine layer rather than only the reference-store layer. | CT-1, CT-3 | P1 |
| E-03 | `document_ingest_gate_commits_before_write_loses_document_on_later_failure` | Given a document ingest where `stage_chunks`/scoring is made to fail (via a `TestLlmExtractor`/embedder configured to error) *after* the source gate transaction commits, when ingest is retried, then it does **not** return `already_ingested` with zero chunks forever — either the gate is rolled back or the retry completes the ingest (expected to fail pre-fix; QI-1's exact "gate commits, later stage fails, permanently stuck" scenario, reproduced via process-restart simulation). | QI-1 | P0 |
| E-04 | `ingest_retry_after_transient_failure_succeeds_once` | Given the same fixture but the injected failure is transient (fails once, succeeds on retry), when ingest is retried after the fix, then exactly one set of chunks exists (no duplication) and the document is fully ingested. | QI-1 | P1 |
| E-05 | `poison_pill_payload_does_not_requeue_forever` | Given a job enqueued with a payload that fails to deserialize (constructed by writing a malformed row directly, since the public API can't produce one), when `drain_queue_with_tick` runs for several scheduler ticks, then the job is parked as `unrecoverable`/`failed` and is **not** resurrected by `self_heal` on a later tick (expected to fail pre-fix per QI-2; `drain_queue`'s `max_iters` cap makes an infinite-retry regression a hard test failure, not a hang). | QI-2 | P0 |
| E-06 | `stale_running_lock_recovered_without_process_restart` | Given a job forced into `running` state past its lease (simulated by claiming it and then abandoning the delegate call, i.e. process-restart simulation), when a scheduler tick runs `drain_queue_with_tick` in the *same* process (no restart), then `recover_stale_locks` reclaims it and it completes (expected to fail pre-fix per QI-6, which today only recovers stale locks at startup). | QI-6 | P0 |
| E-07 | `edge_triggered_seal_dedupe_does_not_starve_over_budget_buffer` | Given an L0 buffer that crosses its seal threshold while a seal job for the same `(tree, level)` is already `running` (simulated via the blocking `TestSummariser`), when that running seal completes, then a follow-up seal is enqueued/observed for the buffer instead of waiting for the next hours-later `flush_stale` pass (expected to fail pre-fix per QI-8). | QI-8 | P1 |
| E-08 | `defer_loop_is_bounded` | Given a handler configured (via a delegate hook) to return `Defer` on every call with no progress, when `drain_queue` runs, then the job eventually terminates (fails/parks) within a bounded number of defers/age rather than re-deferring at the same interval forever (expected to fail pre-fix per QI-9; `drain_queue`'s iteration cap turns an unbounded loop into a hard assertion). | QI-9 | P0 |
| E-09 | `graceful_shutdown_then_restart_does_not_burn_attempts_budget` | Given a job claimed then "gracefully shut down" (process-restart simulation calling the equivalent of `release_running_locks`) five times in a row before ever running to completion, when it's finally allowed to run, then it still has attempts remaining (expected to fail pre-fix per QI-10). | QI-10 | P1 |
| E-10 | `two_workers_do_not_deadlock_on_llm_gate` | Given two concurrent `drain_queue` tasks (multi-thread runtime) contending for the single LLM permit via a `TestLlmExtractor` that holds the gate briefly, when both run concurrently against overlapping ready jobs, then both complete within a timeout — no permanent stall (expected to fail/hang pre-fix per QI-4; the test must assert via a timeout, not an infinite `.await`). | QI-4 | P0 |
| E-11 | `crash_after_extract_before_settle_does_not_duplicate_side_effects` | Given the process-restart simulation applied between "delegate's extract side effect completes" and "settlement commits", when the job is recovered and re-run to completion, then the *observable* end state (entity index rows, buffer contents) is idempotent — no duplicated entity rows from the re-executed extraction (expected to fail pre-fix per QI-5, and documents the idempotency contract QI-5 says `QueueDelegates` implementors must uphold). | QI-5 | P1 |
| E-12 | `reembed_backfill_flag_clears_on_terminal_failure` | Given a `ReembedBackfill` job configured to fail terminally via `drain_queue`, when it parks as failed, then a subsequent vector search is not permanently treated as "not searched yet" (expected to fail pre-fix per QI-7). | QI-7 | P2 |
| E-13 | `chat_reingest_with_overlap_does_not_duplicate_or_reflow_tree` | Given a chat source ingested, drained, and sealed, then re-ingested with a batch that overlaps the tail of the first (same messages plus new ones), when drained again, then chunk count for the source reflects deduped content (no duplicate summary content from the same messages flowing through the tree twice) (expected to fail pre-fix per SC-8/QI-13). | SC-8, QI-13 | P0 |
| E-14 | `email_canonicalizer_boundary_injection_does_not_split_message` | Given an email body containing the literal line `"---\nFrom: attacker@example.com"`, when ingested end-to-end, then retrieval never surfaces a synthetic extra message attributed to `attacker@example.com` — the injected line stays inside the original message's content (expected to fail pre-fix per QI-14). | QI-14 | P0 |
| E-15 | `chat_canonicalizer_boundary_injection_does_not_split_message` | Given a chat message body containing a line starting with `"## "` (the chat boundary marker), when ingested end-to-end, then the message is not split into two chunks/messages at that line (expected to fail pre-fix per QI-14). | QI-14 | P0 |
| E-16 | `epoch_seconds_timestamp_rejected_not_silently_rescaled` | Given an ingest payload with a timestamp expressed in epoch-seconds instead of milliseconds (~1.7e9 instead of ~1.7e12), when canonicalized, then ingest either rejects it or the resulting `time_range` is not silently ~1970 — verified by checking the resulting tree node's time range does not predate the fixture's actual clock (expected to fail pre-fix per QI-15). | QI-15 | P0 |
| E-17 | `seal_does_not_lose_items_appended_during_summariser_await` | Given an L0 buffer sealing via the notify-gated `TestSummariser` (paused mid-await), when a second task appends a new leaf to the same buffer before the summariser returns, then after the seal transaction commits, the new leaf is still present in the tree (either in the sealed summary's children or retained in the buffer for the next seal) — not silently dropped (expected to fail pre-fix per TR-1; this is the improvement plan's named "seal-vs-append race test"). | TR-1 | P0 |
| E-18 | `concurrent_double_cascade_does_not_produce_duplicate_summaries` | Given the same notify-gated setup but with two cascades racing the same `(tree_id, level)` buffer (e.g. a live `append_leaf`-triggered cascade racing `flush_stale_buffers`), when both proceed, then exactly one summary is produced for the buffer's content, not two (expected to fail pre-fix per TR-1). | TR-1 | P0 |
| E-19 | `rebuild_tree_survives_process_restart_mid_rebuild` | Given `rebuild_tree` invoked and the process-restart simulation applied between the old tree's deletion and the rewrite completing, when the engine reopens against the same tempdir, then the tree is not permanently empty — either the rebuild resumes correctly or a pre-rebuild backup is restored (expected to fail pre-fix per TR-2). | TR-2 | P0 |
| E-20 | `orphaned_tree_buffer_backup_adopted_on_startup` | Given a `tree_buffer_backup` directory left on disk (simulating a crash between rename-to-backup and restore), when a fresh `MemoryConfig`/engine opens that path, then the backup is adopted (restored) rather than silently ignored forever (expected to fail pre-fix per TR-2). | TR-2 | P1 |
| E-21 | `force_seal_now_flushes_under_budget_buffer` | Given an L0 buffer under its normal seal-budget threshold (the disconnect/"seal now" case), when `TreeFactory::seal_now`/`force_flush_tree` is invoked through the real engine, then the buffer is sealed immediately, not left for the next natural threshold crossing (expected to fail pre-fix per TR-3 — `seal_now` passing `None` today makes this a no-op). | TR-3 | P0 |
| E-22 | `one_poison_buffer_does_not_abort_flush_of_healthy_trees` | Given N stale buffers where one references only deleted/unhydratable chunk ids, when `flush_stale_buffers` runs (via `drain_queue_with_tick`), then the healthy buffers among the N still seal — the poison buffer's failure doesn't short-circuit the pass (expected to fail pre-fix per TR-4). | TR-4 | P0 |
| E-23 | `archived_tree_rejects_new_leaf_end_to_end` | Given a tree whose status is set to archived, when ingest attempts to append a new leaf targeting that tree's scope end-to-end, then the append is rejected (or redirected), not silently accepted into an archived tree (expected to fail pre-fix per TR-9). | TR-9 | P1 |
| E-24 | `time_tree_node_write_survives_partial_write_simulation` | Given a time-tree node write interrupted by the process-restart simulation immediately after the node file's temp write but before any rename step is confirmed, when the engine reopens, then the node is either fully the old value or fully the new value — never a truncated/partial parse (expected to fail pre-fix per TR-7, once the atomic-write fix from Phase 0.3 lands; asserted here at the full round-trip level rather than the unit level). | TR-7 | P1 |
| E-25 | `llm_soft_fallback_does_not_drop_borderline_chunk_end_to_end` | Given a chunk whose cheap score is borderline (~0.36) ingested with a `TestLlmExtractor::DefaultingSoftFail`, when drained through the real scoring step, then the chunk is `kept` and retrievable — the RS-1 drop scenario reproduced through the full pipeline, not just `score_chunk` in isolation. | RS-1 | P0 |
| E-26 | `hybrid_scoring_composition_changes_ranking_end_to_end` | Given a `WeightProfile` other than the default configured on the engine, when a retrieval query runs against a small corpus with distinguishable freshness/keyword/graph signals, then result ordering differs from the pure-cosine-rerank baseline — confirms RS-3's wiring fix is reachable from a real query, not just unit-callable. | RS-3 | P0 |
| E-27 | `query_topic_time_window_returns_matches_beyond_two_hundred_row_cap_e2e` | Given >200 real occurrences of one entity ingested across many chunks with a time window covering only the older ones, when `query_topic` runs through the retrieval API, then in-window hits are returned (expected to fail pre-fix per RS-2, reproduced with real ingested data rather than a hand-seeded fixture). | RS-2 | P1 |
| E-28 | `drill_down_surfaces_older_live_revision_when_newest_is_deleted_e2e` | Given a document ingested twice (two versions) end-to-end and the newer revision soft-deleted through the real deletion path, when `drill_down` retrieval runs, then the older, live revision is still returned (expected to fail pre-fix per RS-8). | RS-8 | P1 |
| E-29 | `source_deletion_removes_raw_archive_bodies_and_gate_rows` | Given an email source ingested with raw archive refs, when the source is deleted through the real deletion path, then no `raw_refs_json`-referenced files remain on disk and the `raw_file` gate rows are cleared (expected to fail pre-fix per SC-7 — the GDPR-delete scenario, exercised end-to-end since it spans chunks + gate + filesystem). | SC-7 | P0 |
| E-30 | `mem_tree_entity_index_coverage_reflects_real_indexed_entities` | Given chunks ingested and drained through the real `EntityIndex`, when `extraction_coverage` is queried, then it reflects the actual indexed count rather than 0 — regression guard against SC-6's "two owners, two databases" defect being reintroduced (expected to fail pre-fix if `EntityIndex` is opened at a divergent path). | SC-6 | P1 |
| E-31 | `corrupt_chunk_db_recovers_instead_of_wedging` | Given a `chunks.db` file replaced with corrupt bytes after a healthy ingest (simulating `SQLITE_CORRUPT`, the closest this suite gets to true low-level fault injection since it's a single-file swap, not a mid-write interruption), when the engine reopens and a query runs, then it recovers (quarantine + rebuild) rather than wedging indefinitely (expected to fail pre-fix per SC-5 — `recover_corrupt_db` is currently dead code). | SC-5 | P0 |
| E-32 | `front_matter_file_missing_trailing_newline_does_not_panic_read_path` | Given a chunk's content file hand-edited to end in `\n---` with no final newline (simulating an externally-synced truncated file), when any read path touches it (`read_chunk_file`/`verify_summary_file`/tag update) end-to-end, then it returns an `Err`, not a panic (expected to fail pre-fix per SC-1 — the single most severe finding in scope, verified at the point a real read path would hit it, not just the isolated parser). | SC-1 | P0 |
| E-33 | `goals_survive_engine_restart_after_crash_simulated_mid_save` | Given a goals mutation interrupted by the process-restart simulation between "old content truncated" and "new content written" (feasible today since `save` is a bare `fs::write`, so this pre-fix case documents actual on-disk truncation), when the engine reopens, then the goals file is not silently emptied (expected to fail pre-fix per DS-1). | DS-1 | P1 |

### B. Property tests

| ID | Name | Given / When / Then | Findings | Priority |
|----|------|----------------------|----------|----------|
| PR-01 | `frontmatter_compose_parse_round_trip_arbitrary_scalars` | Given arbitrary strings for `source_id`/`owner`/`source_ref`/tag values (including empty, unicode, embedded `\n`, `\r\n`, leading/trailing whitespace, `:`, `#`, backslash, and quote characters) generated by `proptest`, when `compose` then `parse` runs, then every field decodes to exactly the input string — no injected front-matter lines, no truncation (expected to fail pre-fix per SC-2/SC-19: today a `\n` in the input corrupts the front-matter boundary). | SC-2, SC-19 | P0 |
| PR-02 | `frontmatter_body_containing_fence_marker_round_trips` | Given an arbitrary chunk body that itself contains the literal substring `"\n---\n"` (proptest-generated body wrapping the marker at random offsets), when composed then parsed, then the body byte-for-byte round-trips and `content_sha256` matches (expected to fail pre-fix — SC-2's stated consequence of unescaped injection). | SC-2 | P0 |
| PR-03 | `frontmatter_missing_trailing_newline_never_panics` | Given `proptest`-generated file contents ending in every combination of `{with, without} final newline` × `{well-formed, truncated, empty}` front-matter fences, when `split_front_matter` runs, then it never panics — always returns `Ok` or a typed `Err` (direct property-test form of SC-1; complements the single hand-crafted regression case in the unit-test spec by fuzzing the boundary instead of pinning one byte offset). | SC-1 | P0 |
| PR-04 | `yaml_scalar_unescape_is_left_inverse_of_escape` | Given arbitrary strings including combinations of `\`, `"`, and `\"`, when `yaml_scalar`/quoting escapes and the corresponding unescape runs, then the result equals the input — regression guard for SC-19's unescape-order bug (`\"` before `\\`). | SC-19 | P0 |
| PR-05 | `entity_frontmatter_round_trip_with_missing_final_newline` | Given `proptest`-generated entity notes bodies and a fence written both with and without a trailing newline, when parsed then composed, then a well-formed input's notes are never silently replaced with an empty string (RS-14, property form of the single hand-crafted case in the retrieval spec). | RS-14 | P1 |
| PR-06 | `time_tree_node_frontmatter_round_trips_arbitrary_metadata` | Given arbitrary `node_id`/`metadata` strings including embedded newlines and dashes, when `write_node`/parse round-trips, then values decode unchanged (TR-14c: `write_node` writes `metadata`/`node_id` unescaped today). | TR-14 | P1 |
| PR-07 | `archivist_lesson_yaml_escape_round_trips_arbitrary_text` | Given arbitrary `lesson` text including `\n---\n`, when `yaml_escape`/parse round-trips through the archivist store, then the lesson round-trips without splicing into the surrounding YAML structure (TR-14b). | TR-14 | P1 |
| PR-08 | `canonicalizer_chunk_count_stable_for_arbitrary_chat_batch` | Given arbitrary chat batches (proptest: 1–50 messages, arbitrary author strings, arbitrary UTF-8 body text including boundary-grammar characters `"## "`, `"---"`, backticks, at random positions) run through the chat canonicalizer twice with identical input, when chunk counts are compared, then they are identical — canonicalization is a pure, deterministic function of its input (new coverage: pins determinism as an invariant, a prerequisite for QI-13's re-ingest-identity fix to even be well-defined). | new coverage | P0 |
| PR-09 | `canonicalizer_chunk_count_unaffected_by_boundary_grammar_content` | Given the same arbitrary-body generator restricted to strings containing the literal boundary markers (`"---\nFrom:"`, lines starting `"## "`), when canonicalized, then the chunk/message count matches a hand-computed expectation from the *logical* message count, not one inflated by spurious boundary splits (expected to fail pre-fix per QI-14 — this is the property-test generalization of E-14/E-15 above, fuzzing placement/repetition of the marker instead of one fixed case). | QI-14 | P0 |
| PR-10 | `canonicalizer_email_chunk_count_stable_for_arbitrary_thread` | Given arbitrary email threads (proptest: 1–20 messages, arbitrary `from`/`to`/`subject` headers including boundary-grammar characters), when canonicalized twice, then chunk counts match, and headers containing `"---\nFrom:"` never inflate the message count (QI-14, email side). | QI-14 | P0 |
| PR-11 | `canonicalizer_timestamp_range_check_rejects_epoch_seconds` | Given a proptest-generated timestamp value uniformly sampled across `{epoch-seconds range, epoch-millis range, epoch-microseconds range, negative, zero}`, when `deserialize_flexible_timestamp` runs, then only the millis-range value is accepted as-is; seconds-range values are rejected or explicitly rescaled, never silently misinterpreted as millis (QI-15, property form generalizing the single ~1970 case). | QI-15 | P0 |
| PR-12 | `goals_render_parse_round_trip_arbitrary_items` | Given an arbitrary ordered list of `GoalItem { id, text }` (proptest: 0–20 items, arbitrary id charset, arbitrary UTF-8 text including newlines, leading `-`, and empty text), when `render` then `parse` runs, then the recognized `- [id] text` lines round-trip id and (trimmed, single-line — see PR-13) text exactly. | new coverage | P0 |
| PR-13 | `goals_render_parse_rejects_or_normalizes_embedded_newlines` | Given a `GoalItem.text` containing an embedded `\n`, when rendered, then the output does not silently produce two lines that `parse` would misread as two goals (either reject at construction, or escape/collapse) — property-generalizes the manual-edit-survival concern from DS-2 into an encoding invariant on the machine-writer side. | DS-2 | P1 |
| PR-14 | `goals_hand_edited_lines_preserved_across_mutation_round_trip` | Given a proptest-generated document consisting of recognized `- [id] text` lines interleaved at arbitrary positions with unrecognized lines (blank lines, prose, sub-bullets), when `parse` then `add`/`edit`/`delete` then `render` runs, then the unrecognized lines are still present in the output in their relative position (expected to fail pre-fix per DS-2 — today `render` emits only header + recognized items, silently dropping everything else). | DS-2 | P0 |
| PR-15 | `goals_edit_to_duplicate_text_is_rejected_like_add` | Given two distinct goals and an `Edit` that rewrites one's text to exactly match the other's (case/whitespace-normalized), when the edit is applied via proptest-generated near-duplicate pairs (varying whitespace/case), then it is rejected or deduped the same way `Add` already is (expected to fail pre-fix per DS-17). | DS-17 | P1 |
| PR-16 | `goals_save_is_atomic_under_process_restart_simulation` | Given a goals document being saved when the process-restart simulation is applied between truncate and write, when the engine reopens, then the file is either the old complete content or the new complete content — never empty/partial (property-run over proptest-generated goal lists of varying size, to also catch size-dependent partial-write windows) (expected to fail pre-fix per DS-1). | DS-1 | P1 |

### C. Backend-conformance suite skeleton

The consolidated `MemoryBackend` trait (configurable-store.md, migration steps
M1–M3) does not exist yet — Phase 3 of the improvement plan. This section
specs the conformance suite's **shape** now, runnable today against the two
partial implementors that already exist (`InMemoryMemoryStore` via
`MemoryStore`, `MockMemory`/a new minimal file-backed impl via `Memory`), so
it can be lifted onto `Arc<dyn MemoryBackend>` with minimal changes once M1
lands — proving out the "same scenarios, parameterized over backend" shape is
itself useful ahead of the trait consolidation (CT-4/CT-5).

**Shape**: one `fn conformance_suite<B: TestBackend>(make: impl Fn() -> B)`
(or a macro, matching whichever idiom the crate's existing generic test
helpers use) instantiated once per backend under test. Each case below runs
against every registered backend; a backend that cannot express a case (e.g.
today's `InMemoryMemoryStore` has no vector search) marks it `#[ignore]` with
a comment citing which finding blocks it — that annotation is itself a live
tracking mechanism for CT-4/CT-5's consolidation.

| ID | Name | Given / When / Then | Findings | Priority |
|----|------|----------------------|----------|----------|
| BC-01 | `insert_then_recall_round_trips_content_and_category` | Given a backend instance, when an entry is stored then recalled by namespace+key, then content, category, and taint round-trip unchanged — run against `InMemoryMemoryStore` and `MockMemory` today, against every `MemoryBackend` impl post-M1. | new coverage | P0 |
| BC-02 | `search_term_based_not_full_phrase_substring` | Given the README's exact "TinyCortex starts as a Rust memory core" / `"theme preference"`-style fixture, when `search`/`recall` runs with a multi-word query where no single contiguous substring match exists, then term-based backends return the hit (expected to fail pre-fix per CT-1/CT-3 against `InMemoryMemoryStore`; documents the exact regression the README reported). | CT-1, CT-3 | P0 |
| BC-03 | `search_result_scores_are_not_all_identical` | Given two stored entries with different term-overlap against a query, when searched, then their scores differ (expected to fail pre-fix per CT-3 — today `InMemoryMemoryStore` gives every hit the same score once the phrase-gate matches). | CT-3 | P0 |
| BC-04 | `memory_trait_constructible_as_arc_dyn_from_every_backend` | Given each registered backend, when wrapped as `Arc<dyn Memory>` (the surface `ToolMemoryStore::new` requires), then construction succeeds without a bespoke adapter per call site (expected to fail today for any backend other than the `#[cfg(test)]` mock per CT-4 — this case is the acceptance test for the CT-4/CT-5 consolidation, not runnable in full until M1/M3 land; track as `#[ignore]` until then). | CT-4 | P1 |
| BC-05 | `store_error_and_engine_error_are_one_type` | Given the renamed `StoreError`/consolidated error type (post CT-5 fix), when a not-found and an I/O failure are both produced by a backend, then both are representable without panicking on lock poisoning (`.expect("lock poisoned")` in today's `InMemoryMemoryStore` — expected to fail pre-fix by triggering a poisoned lock via a panicking search callback, if the test harness allows one; otherwise documents the gap directly). | CT-5 | P1 |
| BC-06 | `namespace_isolation_is_enforced_by_every_backend` | Given two namespaces each with an entry sharing the same key, when either namespace is queried, then only its own entry is returned — run across all registered backends. | new coverage | P0 |
| BC-07 | `unknown_taint_string_decodes_fail_closed_across_the_wire_boundary` | Given a persisted/serialized taint value that is not one of the known variants, when decoded via `serde` (not `from_db_str`), then it resolves to `ExternalSync` (fail-closed), matching the five doc sites' claim, and a *missing* taint field does not silently resolve to `Internal` (expected to fail pre-fix per CT-2 — `serde(other)` is absent and `serde(default)` fails open today). Include as a conformance case since the wire/server backend depends on this exact contract. | CT-2 | P0 |
| BC-08 | `config_validate_rejects_degenerate_values` | Given `MemoryConfig` variants with `dim: 0`, `summary_fanout: 0`, and a negative `WeightProfile` weight, when `MemoryConfig::validate()` is called (post CT-6 fix; today no such method exists), then each is rejected with a descriptive error rather than silently accepted (expected to fail pre-fix per CT-6). | CT-6 | P1 |
| BC-09 | `config_partial_toml_section_deserializes_with_field_defaults` | Given a `[embedding]` TOML section specifying only `model` and omitting `dim`, when deserialized (post CT-6 per-field `#[serde(default)]` fix), then it succeeds using the field default rather than failing outright (expected to fail pre-fix). | CT-6 | P1 |
| BC-10 | `weight_profile_by_name_rejects_unknown_names` | Given `WeightProfile::by_name("not-a-real-profile")`, when called (post CT-6 fix changing the signature to `Option`), then it returns `None`/an error instead of silently mapping to `BALANCED`. | CT-6 | P2 |
| BC-11 | `memory_category_display_and_serde_round_trip_for_custom_variant` | Given `MemoryCategory::Custom("tool_memory")`, when serialized via serde then rendered via `Display`, then a `from_str`/parse inverse (post CT-7 fix) recovers the original variant without colliding with a built-in category name. | CT-7 | P2 |

### D. CI feature-matrix job

`ci.yml`'s existing `features` job (added in `380e430`) already runs
`cargo check --all-targets` across `--no-default-features`, each feature in
isolation, and `--all-features` — this closes the letter of DS/CT's "no CI
matrix entry enforces feature gating" gap for *compilation*, but not for
*behavior*: no leg of the matrix actually **runs** any test.

| ID | Name | Given / When / Then | Findings | Priority |
|----|------|----------------------|----------|----------|
| CI-01 | `feature_matrix_runs_tests_not_just_check` | Given each existing matrix leg (`core`, `tokio`, `git-diff`, `providers-http`, `rpc`, `all features`), when the CI job runs, then it additionally executes `cargo test --all-targets <flags>` (not only `cargo check`) — so a feature-gated code path that compiles but panics/fails at runtime (e.g. a `#[cfg(feature = "git-diff")]` test relying on an un-gated helper) is caught mechanically. | CT-8, CT-9, CT-10 | P0 |
| CI-02 | `feature_matrix_includes_test_support_leg` | Given the new `test_support` feature this document's harness introduces, when the CI matrix runs, then a `test_support` leg (and an `--all-features` leg that already implies it) exercises `cargo test` so the shared E2E fixtures never silently bit-rot relative to the rest of the crate. | new coverage | P0 |
| CI-03 | `feature_matrix_includes_git_diff_plus_tokio_combination` | Given today's matrix only tests each feature in isolation plus the full set, when a `git-diff + tokio` (no `providers-http`/`rpc`) combination leg is added, then it type-checks and its tests pass — this is the combination the async queue runtime + diff ledger would realistically ship together without pulling in the HTTP stack. | new coverage | P1 |
| CI-04 | `doc_build_has_zero_warnings_per_feature_leg` | Given `cargo doc --no-deps` run once per matrix leg (or at minimum for `--no-default-features` and `--all-features`), when the job runs, then it fails on any warning — regression guard for CT-9's 59 rustdoc warnings and the specific `--no-default-features` "`[diff]` link unresolved without `git-diff`" break. | CT-9 | P0 |
| CI-05 | `readme_example_is_compiled_and_run_in_ci` | Given the README's quick-start code block, when extracted (via `doctest`, or a thin `examples/readme_quickstart.rs` mirrored by a CI step), then it is compiled and run as part of the standard test job — regression guard so CT-1's "example silently returns zero results" class of bug is caught by CI rather than manual audit, once BC-02's fix lands. | CT-1 | P1 |
| CI-06 | `feature_matrix_job_fails_fast_is_disabled_for_full_signal` | Given the matrix `strategy.fail-fast: false` (already set), when one leg fails, then the rest still run and report — regression guard pinning the existing configuration so a future edit doesn't silently narrow CI signal. | new coverage | P2 |

## Priority summary

- **P0**: E-01, E-03, E-05, E-06, E-08, E-10, E-13, E-14, E-15, E-16, E-17,
  E-18, E-19, E-21, E-22, E-25, E-26, E-29, E-31, E-32; PR-01, PR-02, PR-03,
  PR-04, PR-08, PR-09, PR-10, PR-11, PR-12, PR-14; BC-01, BC-02, BC-03, BC-06,
  BC-07; CI-01, CI-02, CI-04. (38 cases)
- **P1**: E-02, E-04, E-07, E-09, E-11, E-20, E-23, E-24, E-27, E-28, E-30,
  E-33; PR-05, PR-06, PR-07, PR-13, PR-15, PR-16; BC-04, BC-05, BC-08, BC-09;
  CI-03, CI-05. (24 cases)
- **P2**: E-12; BC-10, BC-11; CI-06. (4 cases)

Total: **66 test cases** (33 E2E, 16 property, 11 backend-conformance, 6 CI).

## Not in scope

- **Per-module unit tests** for storage primitives, scoring/retrieval/graph
  internals, tree/archivist/conversation mechanics, and diff/sources/goals/
  tool-memory — covered by the sibling specs derived from
  `docs/spec/audit/01`, `02` (already written:
  [`retrieval-scoring-tests.md`](retrieval-scoring-tests.md)), `03`, `05`, and
  `06`. This document only re-exercises a finding at the full-pipeline level
  when the bug's *observable effect* genuinely spans multiple subsystems
  (e.g. E-13 spans ingest + chunks + tree) or when the fix's acceptance
  criterion is inherently cross-cutting (e.g. E-26's "does ranking actually
  change end-to-end").
- **Byte-level crash/fault injection** (fail a write after N bytes, interrupt
  between a temp-file write and its rename) requires a `Filesystem`
  abstraction seam that does not exist in the crate today; introducing it is
  implementation work for improvement-plan Phase 0/1, not test-authoring work.
  This document's "process-restart simulation" (see Harness) covers the
  logical/on-disk-state half of those scenarios without that seam; true
  fault-injection unit tests belong in whichever spec covers the module once
  the seam lands.
- **Fine-grained concurrency/interleaving tests scoped to a single module**
  (e.g. two `SourceRegistry::add` calls racing, two `record_turn` calls
  racing) — covered by the per-module specs; this document only uses
  interleaving where the race spans the queue + tree boundary (TR-1, QI-4)
  because that is what "queue drain → seal" as an end-to-end phrase implies.
- **The server (`rpc`/`providers-http`) wire client** and any HTTP-level
  contract/schema tests — out of scope until `configurable-store.md`'s M5
  lands; BC-07's taint-decoding case is included here only because it is
  testable at the serde layer today, independent of the transport.
- **Performance/load benchmarking** of any fix (e.g. RS-4/RS-5's O(n)→O(1)
  query-count fixes) — covered by shape assertions in the retrieval-scoring
  spec; this document's E2E cases assert correctness (right answer) at
  end-to-end scale, not latency or throughput.
- **Release/packaging CI** (`release.yml`) — untouched; CI section D only
  concerns the `ci.yml` `features` job.
