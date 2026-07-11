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
