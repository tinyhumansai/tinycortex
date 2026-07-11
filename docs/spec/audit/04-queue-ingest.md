# Audit 04 — Job Queue & Ingest Pipeline (`src/memory/queue/`, `src/memory/ingest/`)

Verified findings, most severe first. IDs `QI-*` are referenced from the
[improvement plan](../improvement-plan.md).

## Critical

### QI-1. Document ingest gate commits before any data is written — a later failure permanently loses the document
`src/memory/ingest/pipeline.rs:63-78`

The source gate (`claim_source_ingest_tx`) is claimed and committed in its own
transaction, then content staging, chunk upsert, scoring (which can `bail!`),
and per-chunk persist/enqueue all run afterward with no rollback or
compensation. Scenario: `stage_chunks` hits ENOSPC (or the process crashes)
after the gate commit → every retry returns
`IngestSummary::already_ingested` and the document is never ingested — zero
chunks, no jobs, silently "done" forever. This contradicts the gate's own doc
(`chunks/store_sources.rs:95-96`: "lives inside the persist transaction").

**Fix:** claim the gate in the same transaction as the chunk upsert + job
enqueues (`enqueue_tx` exists for exactly this), or delete the gate row on any
error path after the claim.

## Major

### QI-2. Poison-pill jobs are misclassified as transient and retried forever by `self_heal`
`queue/handlers.rs:171-172` + `store_settle.rs:201-206` + `runtime/mod.rs:93`

A malformed payload is a permanent defect, but it surfaces as a plain `anyhow`
error → `failure_class` stays NULL → after 5 attempts the job parks as
`failed` with NULL class → `requeue_transient_failed` (predicate
`failure_class IS NULL OR != 'unrecoverable'`) resurrects it on every 10-minute
scheduler tick, forever — repeatedly seizing the global LLM permit before
parsing for LLM-bound kinds.

**Fix:** wrap payload-parse errors (and other deterministic input errors) in
`JobFailure::unrecoverable(...)`.

### QI-3. `is_host_io_error` (the #63 fix) is dead code in the runtime loop
`runtime/mod.rs:128-142` vs `worker.rs:167-183`

`backoff_for` checks corrupt/disk-full/io-transient/busy but never calls
`is_host_io_error`; the classifier has no non-test caller. A dying disk
returning EIO from a handler gets the generic 500 ms `error_backoff` and floods
retries — precisely the CORE-RUST-19J flood the classifier's own doc says it
exists to prevent. The adjacent unfixed variant of commit `e352435`.

**Fix:** add a `is_host_io_error` arm to `backoff_for` (ordered after
`is_sqlite_corrupt`) mapping to a long backoff.

### QI-4. Blocking `LlmGate::acquire()` inside an async task — executor stall / deadlock
`worker.rs:61-65` + `gate.rs:67-76` + `store.rs:36`

`acquire()` is a `parking_lot` condvar wait that blocks the OS thread, called
inside `async fn run_once`. Two worker loops on a `current_thread` runtime can
permanently deadlock (A holds the single permit and needs the executor; B
blocks the only executor thread in `cond.wait`). On multi-thread runtimes it
pins a worker thread. The 5-minute lease clock also keeps running while a
claimed job waits at the gate.

**Fix:** `try_acquire` + `Defer`, or an async semaphore under the `tokio`
feature; and/or acquire the permit before claiming.

### QI-5. Handler side effects and settlement are not atomic — LLM work re-executed on crash
`handlers.rs:173-199` + `worker.rs:67-70`

`handle_extract` runs the LLM extraction, then enqueues the follow-up with
plain `store::enqueue` (not `enqueue_tx`, despite `queue/mod.rs:22-24`
advertising that pattern), and only afterwards `mark_done` runs. A crash or
`SQLITE_BUSY` after the delegate succeeded → row recovered as `ready` → the
whole extraction re-runs. At-least-once is fine by design, but the
`QueueDelegates` trait docs never state the idempotency contract the delegates
must therefore satisfy. Same shape in `handle_seal`.

**Fix:** document the idempotency requirement on the trait; enqueue follow-ups
and settlement in one transaction where possible.

### QI-6. Stale-lock recovery runs only at process startup
`worker.rs:46-50` (only caller of `recover_stale_locks`); scheduler tick
(`runtime/mod.rs:87-99`) calls `enqueue_flush_stale` + `self_heal` but not
recovery. Any row stranded in `running` in a long-lived process (failed settle
write, crashed sibling process sharing `chunks.db`) stays invisible to
`claim_next` until some process restarts.

**Fix:** call `recover_stale_locks` from the scheduler tick alongside
`self_heal`.

## Minor

- **QI-7** `handlers.rs:286-307` — `BACKFILL_IN_PROGRESS` flag leaks `true`
  when the backfill job fails terminally; retrieval then treats every empty
  vector search as "not searched yet" until a restart. Clear the flag when a
  `ReembedBackfill` job terminates as failed.
- **QI-8** `handlers.rs:217-224` + `types.rs:332-334` — edge-triggered seal
  enqueue is silently suppressed by the dedupe key while a seal for the same
  `(tree, level)` is still `running`; the over-budget buffer then waits for the
  next `flush_stale` (hours). Re-check the gate at seal completion or make
  `should_seal` level-triggered.
- **QI-9** `store_settle.rs:121-149` — `mark_deferred` reverts the attempts
  bump, so a no-progress `Defer` loop re-defers every 750 ms forever with zero
  budget consumed. Cap total defers or job age.
- **QI-10** `store_settle.rs:154-186` — `release_running_locks` (graceful
  shutdown) returns rows to `ready` without reverting the claim's `attempts`
  bump; five shutdowns mid-job burn the whole `max_attempts` budget.
- **QI-11** `chunks/store_sources.rs:42` — `if changed == 0 {}` empty block:
  setting lifecycle status on a nonexistent chunk silently succeeds.
- **QI-12** `pipeline.rs:89-92,133-135` — ingest step 7 is a non-atomic
  3-write sequence per chunk with a TOCTOU on the prior-lifecycle snapshot:
  (a) crash between lifecycle write and enqueue strands a `pending_extraction`
  chunk (for documents, unrecoverable given QI-1); (b) an extract worker
  admitting the chunk between snapshot and reset makes already-buffered content
  flow through the tree twice — the exact hazard the step-4 comment says must
  not happen. Do snapshot-check + score + lifecycle + `enqueue_tx` in one
  transaction with a lifecycle guard in the WHERE clause.
- **QI-13** `pipeline.rs:36-38` vs `chunks/produce.rs:114-140` — chat/email
  replay idempotency is weaker than documented: ids are
  `f(kind, source_id, seq, content)` with per-batch seq and boundary-dependent
  greedy packing, so any overlapping-but-not-identical re-delivery produces
  all-new ids and the same messages flow through the tree again. Per-message
  content-hash dedup, or document the exactly-once-batch contract loudly.
  (Same root cause as SC-8.)
- **QI-14** `canonicalize/email.rs:81-89`, `chat.rs:75-82` — canonicalisers
  allow structural injection into chunk boundaries: unsanitised
  `from`/`to`/`subject` headers (`md_escape` exists but is unused here), a body
  containing `---\nFrom:` splits one email into bogus messages, a chat line
  starting `## ` splits mid-message. Escape/indent values that collide with the
  boundary grammar.
- **QI-15** `canonicalize/mod.rs:44-47` — `deserialize_flexible_timestamp`
  silently accepts epoch-seconds as milliseconds (~Jan 1970), poisoning
  `time_range` ordering and flush-staleness math. Range-check values below
  ~1e11.

## Test-coverage gaps

- No test that a handler error flows through `run_once` →
  `mark_failed_typed`; no test pinning what failure class a payload-parse
  error gets (QI-2 would have been caught).
- No document-gate failure-path test (QI-1): gate claimed, later stage fails,
  retry behavior.
- `backoff_for` tested for corrupt and busy only — no host-IO, disk-full, or
  io-transient case (QI-3).
- No concurrency tests: two claimants, gate contention, snapshot-vs-worker
  TOCTOU (QI-12), seal-dedupe suppression race (QI-8).
- No `Defer`-loop boundedness test (QI-9);
  `release_running_locks`/`recover_stale_locks` attempts semantics untested
  (QI-10).
- Canonicalizer tests don't cover boundary-grammar injection or epoch-seconds
  timestamps (QI-14, QI-15).
