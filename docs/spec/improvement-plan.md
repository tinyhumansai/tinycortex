# Memory Engine Improvement Plan

> Remediation record (2026-07-14): phases 0, 1, 2, and 4 have been
> implemented with regression coverage. Phase 3's contract consolidation is
> complete for the local reference backend (`InMemoryMemoryStore` implements
> `Memory` and the conflicting store error was renamed); the network backend
> remains a separate future product capability described in
> [configurable-store.md](configurable-store.md), not an implemented feature.
> The architecture alternatives in audits 07–10 remain decision records where
> they propose replacing the shared transactional SQLite boundary.

Derived from the [audit findings](README.md). Phases are ordered by risk:
each phase is independently shippable, and within a phase the workstreams are
parallelizable (use separate worktrees per the repo guidelines). Finding IDs
(`SC-*`, `RS-*`, `TR-*`, `QI-*`, `DS-*`, `CT-*`) refer to the audit documents.

## Phase 0 — Stop the bleeding (small, surgical, high-severity)

Each item is a focused fix + regression test; most are a few lines. Target:
one PR per item or small cluster.

| # | Fix | Findings |
| --- | --- | --- |
| 0.1 | `split_front_matter`: handle EOF fence without trailing newline (panic → `Err`) | SC-1 |
| 0.2 | `yaml_scalar`: quote + escape control chars/newlines; fix `scan_fm_field` unescape order | SC-2, SC-19 |
| 0.3 | Atomic writes: goals `save`, time-tree `write_node`, `stage_summary` replace (temp + rename everywhere) | DS-1, TR-7, SC-3 |
| 0.4 | Ingest gate: claim inside the persist transaction (or compensate on failure) | QI-1 |
| 0.5 | `force_flush_tree`: replace `Option<now>`-as-flag with explicit `force: bool` | TR-3, TR-12 |
| 0.6 | LLM score fallback: gate full `combine` on `llm_importance.is_some()` | RS-1 |
| 0.7 | Stop deleting `chunks.db-wal` before a checkpoint attempt (quarantine instead) | SC-4 |
| 0.8 | Classify payload-parse errors as `JobFailure::unrecoverable` | QI-2 |
| 0.9 | Wire `is_host_io_error` into `backoff_for` | QI-3 |
| 0.10 | Entity file: don't clobber notes when front-matter parse fails; accept EOF fence | RS-14 |

Definition of done: every fix lands with a test that fails before the change
(the audit's "test-coverage gaps" name the exact missing cases).

## Phase 1 — Durability & concurrency correctness

The bugs here share one shape: an unlocked read → long await → blind write.

1. **Seal transaction integrity** (TR-1): re-read the buffer inside the seal
   transaction, verify the snapshot prefix, remove sealed ids by
   set-difference. Add a seal-vs-append race test (deterministic interleaving
   via a test summariser that blocks on a channel).
2. **`rebuild_tree` crash safety** (TR-2): rebuild into a temp sibling dir +
   atomic rename; adopt orphaned `tree_buffer_backup` on startup.
3. **Flush isolation** (TR-4): per-tree error collection in
   `flush_stale_buffers`; quarantine unhydratable buffers.
4. **Queue settlement** (QI-5, QI-6): follow-up enqueues via `enqueue_tx` in
   the settle transaction; document the delegate idempotency contract; run
   `recover_stale_locks` on the scheduler tick.
5. **Async hygiene** (QI-4, SC-22): async-safe LLM gate (`try_acquire` +
   `Defer`, or tokio semaphore behind the `tokio` feature); document/enforce
   `spawn_blocking` for `with_connection` from async contexts.
6. **Locking for read-modify-write files** (DS-3, DS-7, DS-15, TR-6):
   mutation mutex for `SourceRegistry` and `put_rule`; O_EXCL + retry for
   `record_turn`; marker-regression guard in `set_read_marker`.
7. **Corruption recovery wiring** (SC-5): hold the init lock across
   quarantine+rebuild; call `recover_corrupt_db` from error classification.
8. **Single owner for `mem_tree_entity_index`** (SC-6): `EntityIndex` wraps
   the shared chunk-DB connection.

New test infrastructure this phase should introduce:
- A crash-injection harness for file writes (fail after N bytes / between
  rename steps).
- A two-task interleaving helper for SQLite read-modify-write races.

## Phase 2 — Retrieval & scoring correctness

1. **Decide the hybrid-scoring story** (RS-3): either wire
   `hybrid_score`/`freshness`/`keyword_relevance`/MMR/graph into
   `query_*` under `WeightProfile`, or delete/feature-flag the dead layer and
   fix the module docs. Recommendation: wire it — it is the crate's
   headline claim ("interaction-aware scoring") and all the parts exist.
2. **Push filters into SQL** (RS-2, RS-5): time-window + level filters before
   LIMIT in `lookup_entity` and `collect_source_hits`; hydrate embeddings only
   for survivors.
3. **Graph efficiency + hygiene** (RS-4, RS-13): SQL self-join for
   co-occurrence; clear entity rows on kept→dropped; filter deleted/dropped
   nodes in graph and topic reads.
4. **Ranking consistency** (RS-8 – RS-11, TR-13, TR-16): one shared
   `cosine_similarity`; expose rank scores on `RetrievalHit`; deterministic
   deleted-revision fallback in `drill_down`; epoch-ms timestamps in
   conversation ranking.
5. **Re-ingest identity** (SC-8, QI-13): per-message content-hash dedup so
   overlapping re-deliveries don't duplicate content; delete superseded rows
   transactionally.
6. **Input robustness** (RS-6, RS-7, RS-12, QI-14, QI-15): LIKE escaping,
   timestamp saturation + epoch-seconds rejection, handle-regex trailing
   punctuation, canonicalizer boundary-grammar escaping.

## Phase 3 — Configurable store (server hosting)

Specced separately in [configurable-store.md](configurable-store.md). Summary:
consolidate the two store contracts (CT-4, CT-5), extract backend traits for
the storage primitives, add backend selection to `MemoryConfig`, and implement
a server backend behind the reserved `rpc`/`providers-http` seams. Phases 0–1
are prerequisites: a remote backend amplifies every non-atomic multi-step
write into a distributed-consistency bug.

## Phase 4 — Contracts, injection hardening, and hygiene

1. **Taint semantics** (CT-2): serde fail-closed for unknown taints; decide
   the missing-taint default deliberately.
2. **Prompt/ledger injection** (DS-4, DS-5, DS-19, TR-14): source-id charset
   validation, tool-memory rule sanitization, trailer-block-only parsing,
   archivist YAML escaping.
3. **Reference backend** (CT-1, CT-3): term-based scoring in
   `InMemoryMemoryStore`; fix the README example; make it implement the
   consolidated trait from phase 3.
4. **Config validation** (CT-6): per-field `#[serde(default)]`, a
   `MemoryConfig::validate()` called at engine construction, `by_name` →
   `Option`.
5. **Invariant enforcement** (TR-9, TR-10, DS-16, QI-8, QI-9): archived-tree
   write rejection, hydrated-only `child_ids`, never truncate Critical rules,
   level-triggered seal re-check, defer-loop caps.
6. **Cleanup completeness** (SC-7, SC-16, SC-17, DS-13): raw-archive cascade
   delete (GDPR path), lifecycle re-admission on re-ingest, patch
   clear-semantics.
7. **Hygiene** (CT-8, CT-9, CT-10): split `diff/ledger.rs` and
   `queue/types.rs` under 500 lines; move inline tests out of
   `store/store.rs`; fix rustdoc warnings; README feature-flag notes; add a
   CI feature-matrix check (`--no-default-features`, each feature,
   `--all-features`).

## Cross-cutting: test strategy

Every subsystem audit independently found **zero concurrency tests and zero
crash-recovery tests**. Alongside the per-fix regression tests above:

- **Integration test** for the real path: ingest → queue drain → seal →
  retrieval, against a temp dir, with a deterministic fake
  summariser/embedder. Today `tests/` holds a single in-memory smoke test.
- **Property tests** for round-trips: front-matter compose/parse, goals
  render/parse, canonicalizer boundary grammar (arbitrary message bodies must
  never change chunk count).
- **Crash-injection** for every temp+rename site introduced in phases 0–1.
- **Feature-matrix CI job** so gating regressions are caught mechanically.

## Suggested sequencing

Phase 0 items are independent — land immediately as small PRs. Phase 1 items
1–3 (tree durability) and 4–6 (queue/locking) are two parallel workstreams.
Phase 2 depends only on Phase 0. Phase 3 (configurable store) can start its
trait-consolidation step in parallel but should not ship a remote backend
until Phase 1 lands.
