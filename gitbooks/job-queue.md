---
description: TinyCortex's SQLite-backed async job queue — kinds, statuses, dedupe keys, claiming, the worker loop, and the QueueDelegates heavy-work seam.
---

# Job Queue

TinyCortex replaces the synchronous `append_leaf → cascade_seal → LLM summarise` chain on the ingest hot path with a SQLite-backed **async job queue** and a worker driver. Ingest persists a chunk row and enqueues one follow-up job in the same transaction; a host then drives a claim → handle → settle loop that fans work out into per-kind handlers (extract, append, seal, flush, re-embed, document seal).

All queue state lives in the same `chunks.db` as `mem_tree_chunks` — the `mem_tree_jobs` table and its dedupe index are owned by the shared chunks schema, so a producer can commit its side-effect and its follow-up job atomically. The queue module owns the in-crate control flow (payload parsing, follow-up enqueues, gating, defer); the genuinely external heavy work sits behind the [`QueueDelegates`](#queuedelegates-the-heavy-work-seam) trait.

Source: `src/memory/queue/` (`types.rs`, `store.rs`, `handlers.rs`, `worker.rs`, `scheduler.rs`, `gate.rs`, plus `ops.rs`, `redact.rs`, `store_settle.rs`, and the async `runtime/` submodule, which is compiled only with the crate's `tokio` Cargo feature).

## Pipeline shape

```text
ingest::persist
  └── writes chunk row (lifecycle = pending_extraction)
      enqueues `extract_chunk`

run_once (driven in a host loop) claims jobs by kind:
  extract_chunk    → score/admit → enqueue append_buffer + arm reembed
  append_buffer    → push to L0 → enqueue seal if gate met
  seal             → seal one level → enqueue parent seal if cascading
  flush_stale      → enqueue force-seals for time-stale buffers
  reembed_backfill → embed a bounded batch → Defer until covered
  seal_document    → build one document version's subtree
```

## Job kinds

`JobKind` (`types.rs`) is the discriminator persisted in `mem_tree_jobs.kind` as a snake-case wire string. `JobKind::parse` is the inverse and rejects retired kinds (see below).

| Kind | Wire string | Purpose | LLM-bound | Default `max_attempts` |
|------|-------------|---------|-----------|------------------------|
| `ExtractChunk` | `extract_chunk` | Run LLM scoring + entity extraction over one chunk, decide admission, persist score/lifecycle. | yes | 5 |
| `AppendBuffer` | `append_buffer` | Push an admitted leaf/summary node into a tree's L0 buffer. | no | 5 |
| `Seal` | `seal` | Seal exactly one buffer level of one tree; cascades enqueue a parent `seal`. | yes | 5 |
| `FlushStale` | `flush_stale` | Scan stale buffers and enqueue force-`seal` jobs for any past the age cap. | no | 5 |
| `ReembedBackfill` | `reembed_backfill` | Re-embed a bounded batch of chunks/summaries lacking a vector at the active signature; self-continues. | yes | 3 |
| `SealDocument` | `seal_document` | Build one document version's per-doc subtree and merge its root into the connection tree. | yes | 5 |

`max_attempts` defaults come from the `NewJob::*` constructors: `reembed_backfill` overrides to `3`; all others pass `None`, so the store applies `DEFAULT_MAX_ATTEMPTS = 5` (`store.rs`).

### LLM concurrency

`JobKind::is_llm_bound()` is `true` for `ExtractChunk`, `Seal`, `ReembedBackfill`, and `SealDocument`. The worker acquires a permit from a process-wide gate for the lifetime of such a handler; `AppendBuffer` and `FlushStale` run without one.

The gate (`gate.rs`) is a runtime-agnostic counting semaphore built on `parking_lot`. `DEFAULT_LLM_PERMITS = 1` — a single slot mirrors the upstream single-permit semaphore (laptop-RAM safety for local models). `LlmGate::new(permits)` clamps `0` to `1` so the gate can never deadlock the only worker. `acquire()` blocks for a free slot and returns an RAII `Permit` that returns the slot on drop; `try_acquire()` is the non-blocking seam tests use to assert the gate limits concurrency.

```rust
// worker::run_once
let permit: Option<Permit> = if job.kind.is_llm_bound() {
    Some(LLM_GATE.acquire())
} else {
    None
};
let result = handlers::handle_job(config, &job, delegates).await;
drop(permit); // release before settling the row
```

### Retired kinds

`topic_route` and `digest_daily` are legacy kinds from the removed global/topic trees (`RETIRED_JOB_KINDS`). They are handled gracefully rather than crashing:

- `JobKind::parse` returns `Err` for them, so they are never treated as a live kind.
- `claim_next` excludes them with `AND kind NOT IN ('topic_route', 'digest_daily')`, so a leftover row never reaches `row_to_job` (which would fail to parse it).
- `purge_retired_jobs` deletes them at worker startup (run by `bootstrap`).
- `is_retired_kind(&str)` lets callers recognise a raw kind without parsing.

## Statuses

`JobStatus` (`types.rs`) is persisted on `mem_tree_jobs.status`. Workers transition `ready → running → done|failed`.

| Status | Wire string | Meaning | Terminal |
|--------|-------------|---------|----------|
| `Ready` | `ready` | Claimable; waiting for a worker. | no |
| `Running` | `running` | Claimed and in flight under a lease. | no |
| `Done` | `done` | Settled successfully. | yes |
| `Failed` | `failed` | Retries exhausted or unrecoverable classification. | yes |
| `Cancelled` | `cancelled` | Reserved for explicit admin action — no producer surfaced yet. | yes |

`JobStatus::is_terminal()` is `true` for `Done`, `Failed`, and `Cancelled`.

## Job outcomes: `Done` vs `Defer`

A handler returns `Result<JobOutcome>`. `JobOutcome` (`types.rs`) has two success variants:

- `Done` — the handler ran to completion; the worker calls `mark_done` and the row settles `done`.
- `Defer { until_ms, reason }` — the handler chose not to make progress yet (cloud rate-limited, dependency unavailable, model warming up). The worker calls `mark_deferred`, which reschedules the row to `available_at_ms = until_ms` (UTC ms) **and reverts the claim's `attempts` bump**, so a defer does **not** burn the failure budget. `reason` is recorded in `last_error` for visibility.

Real errors must still be surfaced via `Err(_)`: that path runs `mark_failed_typed`, which burns the attempt budget and applies the exponential-backoff retry logic.

The only current `Defer` producer is `reembed_backfill`: when a batch wrote rows and more remain, it reschedules its **own** row `REEMBED_BACKFILL_REVISIT_MS = 750` ms out (no re-enqueue, so the per-signature dedupe key stays valid).

## Dedupe keys

Every `NewJob::*` constructor sets a `dedupe_key`, backed by a partial `UNIQUE` index that only covers `status IN ('ready', 'running')`. `enqueue` uses `INSERT OR IGNORE`, so a duplicate enqueue while a matching job is queued or in flight is a silent no-op (`enqueue` returns `Ok(None)`); once the first row completes, the key is released and a fresh enqueue creates a new row.

| Payload | Dedupe key | Uniqueness scope |
|---------|-----------|------------------|
| `ExtractChunkPayload` | `extract:{chunk_id}` | per chunk |
| `AppendBufferPayload` (source) | `append:source:{source_id}:{node_part}` | per (source tree, node) |
| `AppendBufferPayload` (topic) | `append:topic:{tree_id}:{node_part}` | per (topic tree, node) |
| `SealPayload` | `seal:{tree_id}:{level}` | one active seal per (tree, level) |
| `FlushStalePayload` | `flush_stale:{date_iso}-h{hour_block}` | one per 3-hour UTC block (≤8×/day) |
| `ReembedBackfillPayload` | `reembed_backfill:{signature}` | one in-flight chain per embedding signature |
| `SealDocumentPayload` | `seal_doc:{doc_id}@{version_ms}` (or `seal_doc:{doc_id}` when unversioned) | one per (doc, version) |

`node_part` comes from `NodeRef::dedupe_fragment()` — `leaf:{chunk_id}` or `summary:{summary_id}`. `FlushStalePayload::dedupe_key` takes `date_iso` and `hour_block` from a single `Utc::now()` reading so the key stays deterministic and boundary-safe (`hour_block = hour / 3`, range `0..=7`).

## The `Job` row

`Job` (`types.rs`) is one row of `mem_tree_jobs`. `payload_json` stays a raw string, parsed lazily by the handler based on `kind`. Key attempt/lock/failure fields:

```text
attempts          u32          failed attempts so far (bumped on each retryable error)
max_attempts      u32          budget; once attempts reaches it the job settles `failed`
available_at_ms   i64          earliest UTC ms the row may be claimed (delays/retries)
locked_until_ms   Option<i64>  lease expiry in UTC ms while `running`; reclaimable once past
last_error        Option<String>  freeform last-error text (not machine-readable)
failure_reason    Option<String>  typed code, e.g. "budget_exhausted"
failure_class     Option<String>  "transient" | "unrecoverable"
created_at_ms / started_at_ms / completed_at_ms   lifecycle timestamps
```

`NewJob` is the caller-side enqueue bundle — `Job` minus the persistence-only columns (id, timestamps, lock metadata) the store mints.

### Typed failures

`JobFailure` (`types.rs`) classifies an error so the store can fail fast on causes that retrying cannot fix. It is the in-crate stand-in for OpenHuman's `memory_tree::health::PipelineFailure`. It carries a machine-readable `code` and a `class` (`"transient"` or `"unrecoverable"`), implements `std::error::Error` so handlers can attach it to an `anyhow` chain, and the worker downcasts it back out at settle time:

```rust
// worker::settle_job
let message = format!("{err:#}");           // full anyhow cause chain → last_error
let typed = err.downcast_ref::<JobFailure>();
mark_failed_typed(config, job, &message, typed)
```

Constructors: `JobFailure::transient(code)`, `JobFailure::unrecoverable(code)`, and the convenience `JobFailure::budget_exhausted()` (`unrecoverable("budget_exhausted")`). `is_unrecoverable()` gates fail-fast: an unrecoverable failure is terminal on the first attempt; a transient one keeps the attempts-bounded retry-with-backoff path. `count_failed_unrecoverable` reports jobs deliberately parked because retrying can't help.

## Claiming and ordering

`claim_next` (`store.rs`) atomically leases the next due job with a single statement — `UPDATE … WHERE id = (SELECT … LIMIT 1) RETURNING …`. Since SQLite serialises writes, no two workers claim the same row. The claim sets `status='running'`, bumps `attempts`, and stamps `started_at_ms` and `locked_until_ms = now + lock_duration_ms`.

Eligibility is `status='ready' AND available_at_ms <= now AND kind NOT IN (retired)`. Ordering **drains forward rather than widening** — most-downstream kinds run first so a slow LLM-bound `extract_chunk` can't starve the seal pipeline behind it:

```text
ORDER BY CASE kind
           WHEN 'seal'          THEN 1
           WHEN 'flush_stale'   THEN 2
           WHEN 'append_buffer' THEN 3
           ELSE 4                 -- extract_chunk, reembed_backfill, seal_document
         END ASC,
         available_at_ms ASC
```

`DEFAULT_LOCK_DURATION_MS = 5 * 60 * 1000` (5 min) — comfortably larger than any expected single-job runtime, so a crashed worker's row is recovered after the window without leaving real failures stuck for hours. Retry backoff is exponential: `backoff_ms(attempts)` = `min(60s * 2^(attempts-1), 1h)` (`RETRY_BASE_MS = 60s`, `RETRY_CAP_MS = 1h`), so the first retry waits 60s, then 120s, 240s, … capped at one hour.

## Worker loop

`worker.rs` exposes the durable primitives a host drives in its own loop (OpenHuman's `tokio` worker pool + wall-clock scheduler are reduced to plain functions, since `tokio` is dev-only here):

- `bootstrap(config) -> (purged, recovered)` — startup housekeeping: `purge_retired_jobs` + `recover_stale_locks`. Call once before the loop.
- `run_once(config, delegates) -> bool` — claim one job, run its handler under the LLM gate, settle. Returns `true` when work was processed, `false` when nothing was eligible.
- `llm_gate()` — the process-wide `LLM_GATE` (a `LazyLock<LlmGate>`), exposed so hosts/tests can inspect or share it.

```rust
// worker::run_once (abridged)
pub async fn run_once(config: &MemoryConfig, delegates: &dyn QueueDelegates) -> Result<bool> {
    let Some(job) = claim_next(config, DEFAULT_LOCK_DURATION_MS)? else {
        return Ok(false);
    };
    let permit = job.kind.is_llm_bound().then(|| LLM_GATE.acquire());
    let result = handlers::handle_job(config, &job, delegates).await;
    drop(permit);
    settle_job(config, &job, result)?;
    Ok(true)
}
```

`settle_job` maps the handler result: `Ok(Done) → mark_done`, `Ok(Defer{..}) → mark_deferred` (no budget burn), `Err → mark_failed_typed` (with any downcast `JobFailure`).

For tests, `drain_until_idle` (`testing` module) calls `run_once` repeatedly to settle the queue deterministically. The worker also ports OpenHuman's SQLite error classifiers verbatim so a host loop can reproduce the "back off, don't page" policy: `is_sqlite_busy` (`SQLITE_BUSY`/`SQLITE_LOCKED`), `is_sqlite_io_transient` (CANTOPEN, WAL truncate, `-shm` family, circuit breaker), `is_sqlite_disk_full` (`SQLITE_FULL`), and `is_sqlite_corrupt` (`SQLITE_CORRUPT`/`SQLITE_NOTADB`).

## Scheduler helpers

`scheduler.rs` exposes the periodic loop body as plain functions a host calls on its own cadence:

- `enqueue_flush_stale(config) -> Option<id>` — enqueue a `flush_stale` job scoped to the current 3-hour UTC block. Dedupe-suppressed per block, so a host can call it freely; returns `Ok(None)` when already queued for the block.
- `self_heal(config) -> u64` — requeue transiently-failed jobs (network blips, timeouts, `SQLITE_BUSY`) so chunks never sit unprocessed until the next manual sync, while leaving unrecoverable failures parked. Delegates to `requeue_transient_failed`.

## QueueDelegates: the heavy-work seam

`handlers.rs` keeps the queue's own control flow in-crate and pushes only the external heavy work — LLM scoring/extraction, buffer pushes, sealing, embedding — behind the `async_trait` `QueueDelegates`. Those operations are exposed only `pub(crate)` to `memory::tree` / `memory::score` / `memory::chunks` (and some aren't part of the ported surface at all), so the trait is the seam: a host that owns visibility supplies a real implementation; tests supply a deterministic one. Methods include `extract_chunk`, `append_node`, `seal_level`, `list_stale_buffers`, `seal_document`, `reembed_batch`, `active_signature`, and `has_uncovered_reembed_work`. Their return types (`ExtractDecision`, `AppendDecision`, `StaleBuffer`, `ReembedProgress`) drive the queue's follow-up enqueues and `Done`/`Defer` choice.

## See also

- [Summary Trees](memory-tree.md)
- [Ingest Pipeline](ingest-pipeline.md)
- [Scoring and Extraction](scoring-and-extraction.md)
- [Storage Primitives](storage-primitives.md)
