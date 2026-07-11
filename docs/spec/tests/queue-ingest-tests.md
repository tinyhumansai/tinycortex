# Test Specification — Job Queue & Ingest Pipeline

Scope: `src/memory/queue/` and `src/memory/ingest/` (canonicalizers, pipeline,
chunking hand-off). Source of findings: [`docs/spec/audit/04-queue-ingest.md`](../audit/04-queue-ingest.md)
(`QI-*`). Cross-reference: [`docs/spec/improvement-plan.md`](../improvement-plan.md)
Phase 0.4/0.8/0.9, Phase 1 item 4, Phase 2 item 5/6.

This document specifies the test cases that should exist; it does not
implement them. Case IDs are stable handles for PR descriptions and commit
messages (e.g. `fix(queue): classify payload-parse errors as unrecoverable
(QI-2, QT-07)`).

## 1. Harness & fixtures

Most of what's needed already exists in-tree; this section says what to
reuse and what small additions close the gaps the audit names.

### Already available (reuse, don't rebuild)

- **`test_config()`** — the `TempDir` + `MemoryConfig::new(tmp.path())` pair
  repeated at the top of every `*_tests.rs` in `queue/` and
  `ingest/pipeline_tests.rs`. Every case below assumes this pattern; no
  shared crate-level helper exists today and none is proposed — the
  duplication is intentional per existing convention (each test owns its
  `TempDir` lifetime).
- **`RecordingDelegates`** (`src/memory/queue/test_support.rs`) — a
  deterministic `QueueDelegates` impl with call counters
  (`extract`/`append`/`seal`/`flush`/`seal_document`/`reembed`) and knobs for
  `extract`/`append` decisions, a one-shot `seal_parent` cascade, a
  `reembed` outcome queue, and `uncovered`/`signature`. Used by
  `handlers_tests.rs`, `runtime/tests.rs`. Extend with knobs rather than
  forking a second delegate impl (see §1.1 below for the one addition this
  spec needs).
- **`RecordingJobSink`** (local to `pipeline_tests.rs`) — a `TreeJobSink`
  that records enqueued chunk ids; `NullJobSink` for the empty-batch cases.
- **Direct store helpers**: `enqueue`, `claim_next`, `get_job`,
  `count_by_status`, `count_failed_unrecoverable`, `count_total` — used
  directly against `mem_tree_jobs` rather than through a wrapper, per
  `store_settle_tests.rs` convention. Tests that need to force timing
  (e.g. make a deferred row claimable) reuse the `UPDATE mem_tree_jobs SET
  available_at_ms = 0` raw-SQL idiom already used in `store_settle_tests.rs`.
- **`sqlite_failure(code, extended_code, msg)`** (local helper in
  `worker_tests.rs`) — builds a `rusqlite::Error::SqliteFailure` for the
  classifier unit tests; reuse its shape for the new `is_host_io_error`
  cases (§4).
- **Runtime loop tests**: `fast_worker_opts()` (2 ms backoffs) and the
  `shutdown`-triggering `stopper` future pattern in `runtime/tests.rs`.

### New additions this spec requires

1. **`RecordingDelegates` failure knobs** — two additions to
   `test_support.rs`:
   - An `extract_err: Option<fn() -> anyhow::Error>` (or an `Arc<dyn Fn() ->
     anyhow::Error>`) knob so `extract_chunk` can return `Err(...)` on
     command, used by QT-01/QT-02 to drive a handler error through
     `run_once` → `mark_failed_typed` and assert the resulting
     `failure_class`.
   - A `seal_delay: Option<tokio::sync::oneshot::Receiver<()>>`-style gate (or
     simpler: an `Arc<AtomicBool>` + `parking_lot::Condvar` pair) so
     `seal_level` can block mid-call — the deterministic interleaving hook
     the improvement plan calls "a test summariser that blocks on a
     channel" (Phase 1 item 1), reused here for the seal-vs-append-race case
     QT-24.
   These are additive fields with `Default`-friendly constructors
   (`RecordingDelegates::admitting()` keeps working unchanged); existing
   callers are unaffected.
2. **A gate-injection helper for `pipeline.rs`** — no code hook exists to
   fail *between* the gate commit and `stage_chunks`. Cases QT-11/QT-12
   simulate this by taking the gate claim in a separate call
   (`claim_source_ingest_tx` directly, bypassing `ingest_canonical`) and then
   calling `content::stage_chunks` with a path that errors (e.g. a file
   created where a directory is expected, or an `O_RDONLY`-remounted
   subtree) — no new production code needed, just a test-only arrangement
   of existing public-in-crate functions.
3. **A two-claimant helper** — a small local function
   `claim_from_two_workers(cfg) -> (Job, Job)` is NOT possible (the queue is
   single-claim-per-row by design); instead concurrency cases use two
   *sequential* claim-then-manual-`UPDATE`-backdate steps (the existing
   `stale_worker_*` pattern) to simulate "worker B claimed while worker A's
   snapshot is stale," and `tokio::join!` for cases that need true
   in-process concurrency (append-buffer race, gate contention) driving two
   `run_once` calls against the same `cfg` concurrently.
4. **Canonicalizer boundary-grammar fixtures** — a small fixture module (or
   just inline `const`s in `email_tests.rs`/`chat_tests.rs`) of adversarial
   inputs: an email body containing a bare `\n---\nFrom: attacker@evil\n`
   line, a `From:`/`Subject:` header value containing embedded `\n---\n`,
   and a chat message whose first line is `## fake — header`. No harness
   code needed — these are plain `EmailMessage`/`ChatMessage` literals fed to
   the existing `canonicalise()` + `chunk_markdown()` pipeline, asserting
   chunk *count* and *field* boundaries survive.
5. **Epoch-seconds fixture** — a literal JSON payload
   `{"timestamp": 1700000000}` (10-digit, unambiguously epoch-seconds, ~2023)
   deserialized through `ChatMessage`/`EmailMessage`/`DocumentInput` to hit
   `deserialize_flexible_timestamp`'s `RawTs::Millis` arm and assert the
   resulting year is NOT ~1970 once QI-15 is fixed (and IS ~1970 today,
   documenting current behavior, if written as a pre-fix regression probe).

No new crash-injection-at-byte-offset harness is needed for this subsystem
(that belongs to the tree/durability spec per Phase 1 item 1-2 — see
"Not in scope" below); the queue/ingest crash scenarios in scope here are all
expressible as "claim/gate/write commits, then the next call errs," not
partial-file-write byte-offset injection.

## 2. Test case table

Priority: **P0** = regression test for a Critical/Major audit finding
(QI-1 through QI-6). **P1** = Major/Minor finding or named test-coverage
gap. **P2** = nice-to-have / defense-in-depth, new coverage not tied to a
specific finding.

| ID | Name | Given / When / Then | Findings | Priority |
|----|------|----------------------|----------|----------|
| QT-01 | payload_parse_error_classified_unrecoverable | Given an `ExtractChunk` job with `payload_json` truncated to invalid JSON; when `run_once` claims and runs it; then the row settles `Failed` with `failure_class = "unrecoverable"` on the very first attempt (no retry burn). | QI-2 | P0 |
| QT-02 | payload_parse_error_excluded_from_self_heal | Given the row from QT-01 already `Failed` unrecoverable; when `self_heal`/`requeue_transient_failed` runs; then the row is NOT resurrected (`count_failed_unrecoverable` still includes it, status stays `Failed`). | QI-2 | P0 |
| QT-03 | untyped_handler_error_stays_transient_and_is_healed | Given a handler that returns a plain `anyhow!("upstream 503")` (not a payload-parse error, no `JobFailure`); when it fails and then `self_heal` runs; then the row requeues to `Ready` (contrast case proving QT-01/02 didn't over-broaden the unrecoverable net). | new coverage | P1 |
| QT-04 | malformed_payload_never_seizes_llm_permit | Given an `ExtractChunk` job (LLM-bound kind) with an unparseable payload; when `run_once` processes it; then `payload_json` parsing fails before `LLM_GATE.acquire()`/`try_acquire()` is reached — assert via a gate that starts fully held (`try_acquire` returns `None` beforehand) that the job still fails fast rather than blocking on the gate. | QI-2 | P1 |
| QT-05 | is_host_io_error_wired_into_backoff_for | Given `backoff_for` and a `std::io::Error` with `raw_os_error() == Some(5)` (EIO) wrapped in an `anyhow::Error` (simulating a handler bubbling a host FS failure); when classified; then it returns `Some(opts.io_backoff)` or a dedicated long backoff, NOT the generic `error_backoff` fallback — written as a change-detector so it fails before the `QI-3` fix lands. | QI-3 | P0 |
| QT-06 | backoff_for_disk_full_still_wins_over_host_io_text_overlap | Given an error whose message contains both "database or disk is full" and "(os error 28)" (ENOSPC text can appear in either family); when classified; then `is_sqlite_disk_full` wins per the documented classification order (checked first among the transient families) — pins the ordering contract so wiring QI-3 doesn't silently reorder it. | QI-3 | P1 |
| QT-07 | backoff_for_host_io_via_context_wrapped_error | Given an EIO `std::io::Error` wrapped through two `anyhow::Context::context(...)` layers (as it would arrive from `create_dir_all`/`File::write` deep in a handler); when `is_host_io_error` is called on the wrapped error; then it still matches (mirrors the existing `is_sqlite_busy_matches_through_context_and_text` pattern). | QI-3 | P1 |
| QT-08 | backoff_for_host_io_enospc_and_erofs | Given `std::io::Error::from_raw_os_error(28)` (ENOSPC) and `(30)` (EROFS) separately; when classified via `backoff_for`; then both route to the host-IO backoff arm (extends the existing EIO-only classifier unit tests to the full documented family and to the loop-level `backoff_for`, not just the bare classifier). | QI-3 | P1 |
| QT-09 | backoff_for_excludes_eacces_enoent | Given `std::io::Error::from_raw_os_error(13)` (EACCES) and `(2)` (ENOENT); when classified; then `is_host_io_error` returns `false` for both (these are genuine bugs that must keep reporting per the module doc) — regression against over-broadening the match. | new coverage | P2 |
| QT-10 | llm_gate_try_acquire_defer_instead_of_blocking | Given the gate fully held (one `Permit` outstanding) and a claimed LLM-bound job; when the worker is exercised via a `try_acquire`-based path (once QI-4's fix lands) instead of blocking `acquire()`; then the job settles `Defer` (lease-safe) rather than blocking the async task — written against the CURRENT blocking `acquire()` call this test documents the pre-fix hazard: assert that `try_acquire()` on an exhausted gate returns `None` deterministically (the seam the fix will consume) and that two `run_once` futures polled via `tokio::join!` under a `current_thread`-flavored single-permit gate do not both proceed concurrently. | QI-4 | P0 |
| QT-11 | gate_permit_is_dropped_before_settle_write | Given an admitting `RecordingDelegates` and a claimed LLM-bound job; when `run_once` completes; then `LLM_GATE.available_permits()` is back to `DEFAULT_LLM_PERMITS` strictly before/at the point `mark_done` writes settle — i.e. the permit does not leak across the settle boundary even on the success path (regression once QI-4's ordering fix moves the acquire point). | QI-4 | P1 |
| QT-12 | two_concurrent_llm_jobs_serialize_through_single_permit_gate | Given two enqueued `ExtractChunk` jobs and `DEFAULT_LLM_PERMITS == 1`; when two `run_once` calls are driven concurrently via `tokio::join!` against the same `cfg` with a `RecordingDelegates` whose `extract_chunk` blocks until signaled; then only one is inside its handler at a time (assert via the counters that increments are never both "in flight" — a shared `AtomicUsize` high-water mark never exceeds 1). | QI-4 | P1 |
| QT-13 | handle_extract_followup_enqueue_survives_crash_before_mark_done | Given `handle_extract` runs to completion (delegate says `kept`, follow-up `append_buffer` enqueued via plain `store::enqueue`) but the process is simulated to crash before `mark_done` (test calls the handler directly and stops short of `settle_job`); when `recover_stale_locks` runs and the row is reclaimed; then the *whole* extraction re-runs and a SECOND `append_buffer` job is enqueued — this pins the documented at-least-once re-execution behavior (QI-5) as a known, tested contract rather than a silent bug, and asserts `RecordingDelegates.counts.extract == 2`. | QI-5 | P0 |
| QT-14 | queue_delegates_trait_doc_states_idempotency_contract | Given the `QueueDelegates` trait's rustdoc; when read; then it states the idempotency requirement (each delegate method must be safe to invoke more than once for the same job payload after a crash between side-effect and settle) — this is a doc-presence check (grep/`assert!` on the doc comment text extracted at compile time is impractical in Rust; implement as a `# Panics`-style doctest-adjacent assertion is not applicable — mark this case as **doc review**, not an automated test: track it as a checklist item in the PR description instead of a `#[test]`). | QI-5 | P1 |
| QT-15 | handle_seal_followup_enqueue_survives_crash_before_mark_done | Same shape as QT-13 for `handle_seal`'s cascade-to-parent enqueue: given a `seal_parent` cascade configured on `RecordingDelegates`, when the handler runs twice (simulating a crash-and-reclaim), then two parent `Seal` jobs are enqueued (`counts.seal == 2`, two rows with `kind = "seal"` for the parent) — documents the same at-least-once hazard for the seal path. | QI-5 | P1 |
| QT-16 | recover_stale_locks_called_from_scheduler_tick | Given a job claimed with an already-expired lease (`claim_next(&cfg, -1)`), left `Running` and never recovered at startup; when the scheduler tick function that also runs `self_heal`/`enqueue_flush_stale` executes (post-QI-6 fix: `recover_stale_locks` wired into that same tick); then the row returns to `Ready` without a process restart — written as a change-detector: today `run_scheduler`'s loop body never calls `recover_stale_locks`, so this test fails until QI-6 lands. | QI-6 | P0 |
| QT-17 | stale_lock_recovery_absent_from_long_lived_scheduler_today | Given the CURRENT `run_scheduler` loop (unmodified) and a stranded `Running` row with an expired lease; when several scheduler ticks elapse (via `fast_worker_opts`-style fast cadence) with no call to `bootstrap`/`recover_stale_locks`; then the row is still `Running` after N ticks — a documented "known gap" test that must be *deleted or inverted* once QI-6 ships (kept here so the fix's PR diff includes flipping this exact assertion). | QI-6 | P1 |
| QT-18 | document_gate_commits_then_stage_chunks_fails_permanently_loses_doc | Given a document ingest where `claim_source_ingest_tx` is committed (gate claimed) and then `content::stage_chunks` is forced to fail (test injects a failure per §1.1.2); when `ingest_document` is retried with identical input; then `IngestSummary::already_ingested` is returned FOREVER and `count_chunks == 0` — this is the QI-1 regression, written to FAIL before the fix (gate-in-persist-tx or compensating delete) and PASS after: post-fix, the retry must either roll the gate back or the same call must complete the ingest transactionally so a retry is not needed. | QI-1 | P0 |
| QT-19 | document_gate_and_chunk_upsert_share_one_transaction_post_fix | Given the QI-1 fix landed (gate claim inside the persist transaction); when `stage_chunks` (or any step through chunk upsert) fails after the gate claim; then the gate row is NOT present afterward (rolled back with the rest of the transaction) — assert via `is_source_ingested` returning `false` so a subsequent retry can actually succeed. | QI-1 | P0 |
| QT-20 | non_document_sources_have_no_gate_and_rely_on_chunk_id_idempotency | Given a chat/email ingest (no `claim_source_ingest_tx` call at all per the current design); when the same batch is ingested twice; then both calls succeed (`already_ingested == false` both times) and re-delivery safety comes entirely from deterministic chunk ids — a baseline/contrast case so QT-18/19's document-only gate assumption doesn't leak into chat/email expectations. | new coverage | P2 |
| QT-21 | ingest_chunk_and_job_enqueue_survive_crash_between_lifecycle_write_and_enqueue | Given `ingest_canonical` step 7's per-chunk sequence (`persist_score` → `set_chunk_lifecycle_status(pending_extraction)` → `sink.enqueue_extract`); when a fault is injected between the lifecycle write and the enqueue call (test-only `TreeJobSink` whose `enqueue_extract` fails for the Nth chunk); then that chunk is left `pending_extraction` with no corresponding job — for chat/email a retry naturally re-admits it (chunk id ids are stable and `needs_processing` re-checks `pending_extraction`), so assert the retry DOES re-enqueue; for a Document source (where QI-1's gate has already been claimed) assert the chunk is instead PERMANENTLY stranded (`already_ingested` short-circuits before reaching this chunk again) — this is the "unrecoverable given QI-1" half of QI-12. | QI-1, QI-12 | P0 |
| QT-22 | extract_worker_admits_chunk_between_snapshot_and_lifecycle_reset_toctou | Given a chunk whose prior lifecycle snapshot (`ingest_canonical` step 4) is read as `pending_extraction`; when, between that snapshot read and the subsequent `set_chunk_lifecycle_status` write, a concurrent extract-worker admits the SAME chunk and advances it past `pending_extraction` (simulate by calling `set_chunk_lifecycle_status(config, id, "admitted")` — or whatever the real post-extraction status constant is — in between the two pipeline calls, using the pipeline's own building blocks directly rather than going through `ingest_canonical` end-to-end); then the ingest step re-enqueues an extract job for the already-progressed chunk, causing double admission — a failing regression for the TOCTOU half of QI-12, to be fixed by moving snapshot-check + enqueue into one transaction with a lifecycle guard in the WHERE clause. | QI-12 | P0 |
| QT-23 | ingest_canonical_snapshot_check_and_enqueue_atomic_post_fix | Given the QI-12 fix (single transaction: snapshot-check + score + lifecycle + `enqueue_tx`); when the same interleaving as QT-22 is attempted; then the WHERE-clause lifecycle guard causes the stale-snapshot branch to no-op instead of re-enqueueing — the positive-path twin of QT-22. | QI-12 | P1 |
| QT-24 | seal_dedupe_suppresses_reseal_while_prior_seal_still_running | Given a buffer that crosses its seal gate twice in quick succession (second push happens while the first `Seal` job for the same `(tree, level)` dedupe key is still `Running`, simulated by blocking `seal_level` on a gate per §1.1.1); when the second `append_buffer` handler runs and tries to enqueue its own `Seal`; then the dedupe key (`enqueue`'s `INSERT OR IGNORE` on `dedupe_key`) silently drops the second seal request and the buffer stays over-budget until the next `flush_stale` tick — QI-8's suppressed-signal hazard, asserting `store::enqueue` returns `Ok(None)` for the second attempt. | QI-8 | P0 |
| QT-25 | seal_completion_rechecks_gate_post_fix | Given the QI-8 fix (re-check the gate at seal completion, or level-triggered `should_seal`); when the scenario in QT-24 plays out; then a follow-up `Seal` is enqueued once the running seal completes, instead of waiting for the next `flush_stale` window — positive-path twin of QT-24. | QI-8 | P1 |
| QT-26 | reembed_backfill_flag_leaks_true_on_terminal_failure | Given `handle_reembed_backfill` mid-chain (`set_backfill_in_progress(true)` already called by a prior `Wrote{more_pending:true}` batch); when the NEXT batch's job instead terminates as `Failed` (max attempts exhausted on a delegate error, not a normal `Wrote`/`Covered`/`NoProvider` outcome); then `set_backfill_in_progress` is never reset to `false` and stays `true` indefinitely — QI-7's flag-leak, asserting via `crate::memory::queue::ops`'s flag accessor that it stays stuck `true` after the job reaches `Failed` with no other job to clear it. | QI-7 | P1 |
| QT-27 | reembed_backfill_flag_cleared_on_terminal_failure_post_fix | Given the QI-7 fix (clear the flag when a `ReembedBackfill` job terminates `Failed`); when the same scenario as QT-26 plays out; then the flag reads `false` after the job settles `Failed`. | QI-7 | P1 |
| QT-28 | mark_deferred_reverts_attempts_bump_documented_behavior | Given a claimed job (`attempts == 1`); when `mark_deferred` runs; then `attempts` reverts to `0` — this already exists as `mark_deferred_does_not_increment_attempts` in `store_settle_tests.rs`; listed here only to confirm coverage, no new test needed. | QI-9 (baseline) | P2 |
| QT-29 | defer_loop_is_unbounded_without_a_cap | Given a job whose handler always returns `Defer{until_ms: now, reason: "no_progress"}` on every attempt; when `run_once` is driven in a loop 50 times (with `available_at_ms` backdated between iterations so each call is immediately claimable); then `attempts` never increases past `0`/`1` and the job never reaches `Failed` — demonstrating the QI-9 hazard (zero budget consumed, infinite defer loop) as a failing-by-design characterization test today. | QI-9 | P0 |
| QT-30 | defer_loop_capped_post_fix | Given the QI-9 fix (cap total defers or job age); when the same 50-iteration loop from QT-29 runs; then the job terminates `Failed` (or some bounded-defer status) once the cap is hit, rather than looping forever. | QI-9 | P1 |
| QT-31 | release_running_locks_does_not_revert_attempts_bump | Given a job claimed (`attempts == 1`, `Running`); when `release_running_locks` (graceful shutdown path) runs; then the row returns to `Ready` but `attempts` STAYS at `1` (contrast with `mark_deferred`, which reverts it) — this already exists as `release_running_locks_resets_running_rows_regardless_of_lease` but does not currently assert on `attempts`; extend it (or add a sibling case) to assert `attempts == 1` post-release, pinning QI-10's finding precisely. | QI-10 | P0 |
| QT-32 | five_shutdowns_burn_the_whole_attempts_budget | Given `max_attempts == 5` and a job claimed+released via `release_running_locks` five times in a row (claim → release → claim → release ...); when the fifth release completes; then `attempts == 5` and the job is one claim away from permanent failure despite having done zero actual failed work — QI-10's "five shutdowns burn the whole budget" scenario made concrete and quantitative. | QI-10 | P0 |
| QT-33 | release_running_locks_attempts_preserved_or_capped_post_fix | Given the QI-10 fix (whatever shape it takes — reverting the bump like `mark_deferred`, or excluding shutdown-released attempts from the budget); when the QT-32 scenario repeats; then the job is NOT one claim away from failure purely from graceful restarts. | QI-10 | P1 |
| QT-34 | set_chunk_lifecycle_status_on_nonexistent_chunk_silently_succeeds | Given a chunk id that does not exist in the chunk store; when `set_chunk_lifecycle_status` is called on it; then it returns `Ok(())` with zero rows changed (the `if changed == 0 {}` empty block swallows the condition) — QI-11's silent-no-op, written to fail once the fix makes this case return an error/log/assert instead. | QI-11 | P1 |
| QT-35 | set_chunk_lifecycle_status_nonexistent_chunk_surfaces_post_fix | Given the QI-11 fix (error or explicit logged no-op instead of a bare empty block); when the same call as QT-34 is made; then the caller can distinguish "chunk existed and was updated" from "chunk did not exist" (exact shape depends on the fix — `Result<bool>` returning `false`, or an explicit `Err`). | QI-11 | P2 |
| QT-36 | chat_replay_with_overlapping_but_nonidentical_batch_duplicates_content | Given a chat batch of 2 messages ingested once, then a SECOND batch re-delivered with an overlapping message set but a shifted per-batch `seq` (e.g. redelivery starts one message earlier, or a third trailing message is appended) so `f(kind, source_id, seq, content)` produces all-new chunk ids; when both batches are ingested; then `count_chunks` after the second ingest is greater than after the first by the full second-batch chunk count — i.e. the overlapping message(s) are duplicated rather than deduped, demonstrating QI-13's replay-idempotency gap (same root cause as SC-8, out of scope for the fix itself here). | QI-13 | P0 |
| QT-37 | email_thread_replay_with_reordered_messages_duplicates_content | Same shape as QT-36 for `ingest_email`: a thread re-delivered with one previously-seen message plus reordering that changes greedy packing/seq; assert duplication rather than dedup. | QI-13 | P1 |
| QT-38 | identical_batch_replay_is_a_true_noop | Given a chat batch ingested once; when the EXACT SAME batch (byte-identical) is ingested again; then `count_chunks` does not increase (chunk ids are `f(kind, source_id, seq, content)` and content+seq are unchanged) — the positive control proving today's guarantee ("exactly-once for byte-identical redelivery") holds, so QT-36/37 are read as "overlapping-but-not-identical" failures, not "any redelivery" failures. | QI-13 (contrast) | P1 |
| QT-39 | per_message_content_hash_dedup_post_fix | Given the QI-13 fix (per-message content-hash dedup, independent of batch/seq shape); when the QT-36 scenario repeats; then the overlapping message is NOT duplicated. | QI-13 | P2 |
| QT-40 | email_body_boundary_injection_splits_into_bogus_extra_message | Given an `EmailMessage.body` containing a literal `"\n---\nFrom: attacker@evil.example\nSubject: fake\n\ninjected\n"` line sequence (mimicking the chunker's own `---\nFrom:` boundary grammar); when the thread is canonicalised and chunked; then the chunker treats it as a genuine message boundary and produces an EXTRA chunk whose metadata attributes the injected `From:`/`Subject:` to the (fake) sender — demonstrating QI-14: a message body can forge a sibling message. | QI-14 | P0 |
| QT-41 | email_header_value_containing_boundary_sequence_corrupts_parsing | Given an `EmailMessage.subject` (or `from`/`to`) containing an embedded `\n---\nFrom: ` sequence; when canonicalised; then the rendered markdown's own header block is corrupted such that re-chunking misparses the boundary — demonstrates the unsanitised-header half of QI-14 (`md_escape` exists but is unused in `email.rs`/`chat.rs`). | QI-14 | P0 |
| QT-42 | chat_message_starting_with_heading_marker_splits_mid_message | Given a `ChatMessage.text` whose first line is literally `"## fake — injected"` (the chunker's own chat boundary grammar); when canonicalised and chunked; then the chunker splits the message into two chunks at the fake boundary, misattributing the remainder as a new message from a spoofed author/timestamp — QI-14 for the chat adapter. | QI-14 | P0 |
| QT-43 | boundary_injection_escaped_or_indented_post_fix | Given the QI-14 fix (escape/indent values that collide with the boundary grammar); when the QT-40/41/42 inputs are replayed; then the chunk count matches the true message count (no spurious split) — the positive-path twin covering all three adapters. | QI-14 | P1 |
| QT-44 | arbitrary_message_body_never_changes_chunk_count_property | Given a property-style test (per the improvement plan's "Property tests" cross-cutting recommendation) generating message bodies containing random combinations of `---`, `From:`, `## `, and newlines at random positions; when canonicalised+chunked pre- and post-QI-14-fix; then post-fix the chunk count equals the input message count for every generated case (pre-fix this is expected to fail on a subset — kept as documentation of the fix's acceptance bar rather than a CI-blocking test until QI-14 lands). | QI-14 | P2 |
| QT-45 | epoch_seconds_silently_accepted_as_milliseconds_today | Given a chat message JSON payload `{"timestamp": 1700000000, ...}` (10-digit epoch-seconds, which int-decodes fine as epoch-MILLISECONDS under `RawTs::Millis`); when deserialized through `ChatMessage`; then `timestamp` resolves to on/around 1970-01-20 (poisoned near-epoch date), NOT the intended ~2023 date — QI-15's silent-acceptance bug, documented as current (pre-fix) behavior. | QI-15 | P0 |
| QT-46 | epoch_seconds_rejected_or_upconverted_post_fix | Given the QI-15 fix (range-check values below ~1e11 and reject or upconvert); when the same payload as QT-45 is deserialized; then either a `serde::de::Error` is returned, or the value is correctly reinterpreted as seconds and upconverted to ~2023 — whichever the fix chooses, assert the year is no longer ~1970. | QI-15 | P0 |
| QT-47 | legitimate_small_millisecond_values_still_rejected_or_flagged_consistently | Given a synthetic legitimate epoch-ms value just above the QI-15 threshold (e.g. `1e11 + 1`, which is ~1973 — still implausibly old for this product, but a boundary case); when deserialized post-fix; then behavior matches whatever threshold QI-15's fix documents (this case exists to pin the exact boundary value in a test rather than leaving it implicit in a comment). | QI-15 | P1 |
| QT-48 | document_and_email_timestamps_share_the_same_epoch_seconds_guard | Given the QI-15 fixture applied to `DocumentInput.modified_at` and `EmailMessage.sent_at` (not just `ChatMessage.timestamp`); when deserialized; then all three adapters exhibit the same corrected behavior — `deserialize_flexible_timestamp` is shared, so one shared unit test parameterized over the three call sites (or three near-identical `#[test]`s, matching the existing per-adapter `*_tests.rs` split) is required to avoid a fix landing for chat but not document/email. | QI-15 | P1 |

## 3. Not in scope (belongs to other spec docs)

- **Tree/summariser durability** (seal transaction snapshot integrity,
  `rebuild_tree` crash safety, `force_flush_tree` boolean flag, per-tree
  flush isolation) — audit `03-tree-summariser.md` / Phase 1 items 1-3.
  QT-24's seal-dedupe case here stops at "the second seal enqueue is
  suppressed"; it does not re-verify the seal transaction's own snapshot
  correctness once it runs.
- **Retrieval/scoring correctness** (hybrid scoring wiring, SQL-pushed
  filters, ranking consistency, `cosine_similarity`) — audit
  `02-retrieval-scoring.md` / Phase 2 items 1, 3, 4. The `extract_chunk`
  delegate's *admission decision* is exercised here only as an opaque input
  (`ExtractDecision.kept`); the scoring logic that produces it is out of
  scope.
- **Content-store atomic writes and WAL/corruption recovery** (`stage_chunks`
  temp+rename correctness, `-wal` quarantine instead of delete,
  `recover_corrupt_db` wiring) — audit `01-storage-durability.md` / Phase 0
  items 0.3, 0.7 and Phase 1 item 7. QT-18 injects a `stage_chunks` FAILURE
  as an opaque fault to exercise the gate/ingest interaction; it does not
  test `stage_chunks`'s own atomicity.
- **Extraction/parsing correctness inside `extract/`** (rule parsing, regex,
  aggregation) — has its own `*_tests.rs` per module already; this spec
  covers only the queue-level `ExtractChunk`/`Seal`/`AppendBuffer` handler
  dispatch, not the LLM extraction content itself (which is delegate-opaque
  here by design).
- **Configurable store / remote backend** — `configurable-store.md` /
  improvement-plan Phase 3. None of the cases above assume a specific store
  backend beyond the existing local-SQLite `MemoryConfig`.
- **Prompt/ledger injection, taint semantics, config validation** — audit
  `05-contracts.md` / `06-...` and Phase 4 items 1, 2, 4. QI-14 here is
  specifically the *canonicalizer chunk-boundary* injection (structural),
  not prompt-injection into an LLM context window — that's DS-4/DS-5/DS-19
  territory in a different subsystem's spec.
- **Feature-matrix CI / module-size hygiene / rustdoc warnings** — Phase 4
  item 7 (`CT-8`–`CT-10`); mechanical, not a queue/ingest behavior test.

## 4. Coverage check

Every `QI-*` finding and named test-coverage gap in
`docs/spec/audit/04-queue-ingest.md` maps to at least one case above:

| Finding | Cases |
|---|---|
| QI-1 | QT-18, QT-19, QT-21 |
| QI-2 | QT-01, QT-02, QT-03, QT-04 |
| QI-3 | QT-05, QT-06, QT-07, QT-08, QT-09 |
| QI-4 | QT-10, QT-11, QT-12 |
| QI-5 | QT-13, QT-14, QT-15 |
| QI-6 | QT-16, QT-17 |
| QI-7 | QT-26, QT-27 |
| QI-8 | QT-24, QT-25 |
| QI-9 | QT-29, QT-30 (+ QT-28 baseline) |
| QI-10 | QT-31, QT-32, QT-33 |
| QI-11 | QT-34, QT-35 |
| QI-12 | QT-21, QT-22, QT-23 |
| QI-13 | QT-36, QT-37, QT-38, QT-39 |
| QI-14 | QT-40, QT-41, QT-42, QT-43, QT-44 |
| QI-15 | QT-45, QT-46, QT-47, QT-48 |

Total: 48 cases (44 automatable `#[test]`/`#[tokio::test]` cases + QT-14,
which is a doc-review checklist item, + QT-28, an existing-coverage
confirmation, both listed for completeness rather than as new work).
