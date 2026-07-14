# Memory Engine Audit & Improvement Spec

_Audit date: 2026-07-11 · Baseline: `main` @ `c9d1afd` · All 1025 unit tests + 1 integration test green at time of audit._

This folder holds the results of a full-codebase audit of the TinyCortex memory
engine and the improvement plan derived from it. The audit swept every
subsystem under `src/memory/` with per-module deep reads; every finding was
verified against the actual code (several with standalone reproductions) and
carries `file:line` evidence.

## Contents

| Document | Scope |
| --- | --- |
| [audit/01-store-chunks.md](audit/01-store-chunks.md) | Storage primitives: content store, KV, vectors, SQLite chunk store, migrations, recovery |
| [audit/02-score-retrieval-graph.md](audit/02-score-retrieval-graph.md) | Scoring/decay, entity extraction, vector/keyword/graph/hybrid retrieval |
| [audit/03-tree-archivist-conversations.md](audit/03-tree-archivist-conversations.md) | Summary trees (append/seal/flush), archivist, conversation store |
| [audit/04-queue-ingest.md](audit/04-queue-ingest.md) | Async job queue and the canonicalize → chunk → score → tree pipeline |
| [audit/05-diff-sources-goals-toolmemory.md](audit/05-diff-sources-goals-toolmemory.md) | Git-diff ledger, source registry, goals, tool memory |
| [audit/06-contracts-config-api.md](audit/06-contracts-config-api.md) | Crate-wide contracts, config, feature gates, README/doc claims, hygiene |
| [improvement-plan.md](improvement-plan.md) | Phased remediation plan across all findings |
| [configurable-store.md](configurable-store.md) | Spec: pluggable storage backend so agents can be hosted server-side |
| [audit/07-modularity-boundaries.md](audit/07-modularity-boundaries.md) | Unix-philosophy decomposition: module coupling, persistence ownership, god files |
| [audit/08-configurability.md](audit/08-configurability.md) | Library configurability: config coverage, hardcoded tunables, env-var hygiene |
| [audit/09-verification-infrastructure.md](audit/09-verification-infrastructure.md) | Per-block verifiability: test coverage map, CI feature matrix, setup/test scripts |
| [audit/10-simplification-dead-weight.md](audit/10-simplification-dead-weight.md) | Duplication, dead/speculative code, dependency weight, error/async story |

## Executive summary

The engine is well-layered and richly documented, and the full test suite
passes — but the audit surfaced **4 critical** and **~20 major** verified
defects, clustering into five themes:

1. **Crash-safety / durability gaps.** Several write paths are non-atomic
   (goals file, time-tree nodes, summary re-stage, `rebuild_tree` deletes the
   tree before rewriting it), and the ingest gate commits before any data is
   written, so a failure mid-ingest permanently marks a document as ingested
   with zero chunks. WAL side-file cleanup can discard committed transactions.

2. **Silent data loss under concurrency.** The seal path clears the whole tree
   buffer after an unlocked LLM await (leaves appended in the window are lost);
   `record_turn` drops turns on seq collision; the source registry and several
   read-modify-write paths have no locking.

3. **Text-format injection / parser fragility.** Hand-rolled YAML front-matter
   composition doesn't escape newlines (integrity-hash corruption, prompt-block
   forgery in tool memory, ledger trailer injection via source ids), and
   `split_front_matter` panics on a file missing its trailing newline —
   a single bad `.md` file DoSes the whole content-store read path.

4. **Documented behavior that isn't wired.** The hybrid scoring layer
   (graph/keyword/freshness/MMR) has zero callers; corruption recovery is
   `#[allow(dead_code)]`; `is_host_io_error` (the #63 fix) is never called by
   the runtime loop; `force_flush_tree` with `now=None` forces nothing; the
   archivist's promised tree sink doesn't exist; the README quick-start
   example returns zero hits.

5. **No concurrency or crash-recovery tests anywhere.** Every subsystem audit
   independently flagged this; most of the bugs above live exactly in those
   untested windows.

Separately, [configurable-store.md](configurable-store.md) specs the work to
make the storage backend configurable (local-first vs. server-hosted), which
today is blocked by two parallel store contracts (`Memory` has no production
implementor; everything real is hardwired to filesystem + SQLite).

## Second audit (2026-07-14): simplification & trustworthy building blocks

Audits 07–10 (baseline `main` @ `1a3fcc5`, full suite green) take a different
lens from 01–06: not bugs, but **architecture** — how to make the crate
simpler, more configurable as a library, and decomposed into small,
independently verifiable building blocks (Unix philosophy). Headline
conclusions:

1. **One shared database is the central coupling.** `chunks/` owns the
   connection and a 15-table schema for tree/queue/score/graph/retrieval;
   module boundaries are not persistence boundaries, and siblings bind to each
   other's deep internals rather than facades (MB-1..MB-3). A second,
   cleanly-decomposed persistence stack under `store/` is mostly unused.
2. **Config that lies.** `sync.interval_secs` is silently floored to 24h,
   summariser constants shadow and contradict `TreeConfig`, and library code
   reads `COMPOSIO_API_KEY` from the process env (CF-1..CF-3). Retrieval and
   queue/retry tuning is entirely module constants (CF-5, CF-6).
3. **Verification gaps cluster where trust is lowest.** The `sync` feature is
   missing from the CI matrix, feature builds are only `cargo check`ed, the
   smoke test doesn't touch the real engine, and there are no setup/test
   scripts at all (VF-1..VF-5).
4. **Meaningful dead weight.** A git dependency (`tinyagents`) that blocks
   crates.io publishing, two placeholder feature modules, a flagship async
   `Memory` trait with zero production impls, atomic-write implemented eight
   times, and four dependencies with one call site each (SW-1..SW-6).

Finding IDs: `MB-*` modularity, `CF-*` configurability, `VF-*` verification,
`SW-*` simplification.

## Severity conventions

- **Critical** — verified data loss, panic/DoS, or permanent wedge on a
  realistic path.
- **Major** — correctness/durability defect with a concrete failure scenario,
  or documented behavior that silently doesn't happen.
- **Minor** — edge-case correctness, performance cliffs, silent truncation,
  doc-vs-code drift.

Finding IDs are stable and referenced from the improvement plan:
`SC-*` store/chunks, `RS-*` retrieval/score/graph, `TR-*`
tree/archivist/conversations, `QI-*` queue/ingest, `DS-*`
diff/sources/goals/tool-memory, `CT-*` crate contracts.
