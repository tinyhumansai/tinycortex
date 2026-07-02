# 3. Testing, Effectiveness, and Benchmarking

Scope: how we prove the memory system is correct (unit/e2e), good
(retrieval effectiveness), and fast (benchmarks). The crate already has 1060
passing unit tests in sibling `*_tests.rs` files; the missing layers are
end-to-end pipeline proof, quality measurement, and performance measurement.

## 3.1 Testing pyramid

| Layer | Lives in | Runs | Gate |
| --- | --- | --- | --- |
| Unit (`*_tests.rs`) | beside each module | `cargo test` | every commit |
| Invariant/golden | `tests/invariants/` | `cargo test` | every commit |
| E2E pipeline | `tests/e2e/` | `cargo test` | every commit |
| Live-provider conformance | `tests/providers/` (`#[ignore]`) | manual / nightly | pre-release |
| Effectiveness harness | `benchmarks/effectiveness/` | nightly / on demand | regression alerts |
| Performance benches | `benches/` (criterion) | nightly / on demand | regression alerts |

### T1 — End-to-end pipeline tests
Status: todo
Depends-on: C1
Definition of done: `tests/e2e/` drives ingest→queue-drain→tree-seal→retrieval
through `CortexEngine` on the existing fixtures, deterministically (inert
embedder + `ConcatSummariser`), asserting on retrieved content — not just "no
error".

- [ ] `tests/e2e/ingest_to_query.rs`: ingest
      `tests/fixtures/ingestion/gmail_thread_example.txt` and
      `notion_page_example.txt` (currently unused), `drain_until_idle`,
      assert `query_source`/`query_global` return the expected leaves with
      sane score breakdowns.
- [ ] `tests/e2e/conversation_lifecycle.rs`: record turns → archivist →
      leaf id equals `sha256(session_id‖md)[..32]` → retrieval finds the
      conversation → purge semantics.
- [ ] `tests/e2e/seal_and_flush.rs`: enough leaves to force bucket seals and
      a stale flush; assert tree shape (levels, summary rows) matches the
      config thresholds exactly.
- [ ] `tests/e2e/crash_resume.rs`: kill mid-queue (drop engine between
      `run_once` calls), reopen workspace, `recover_stale_locks`, drain —
      identical end state (idempotency/dedupe proof).
- [ ] Concurrency smoke ported from OpenHuman's `memory_tree_init_smoke`
      (M4): N threads race first-touch schema init.

### T2 — Invariant and golden tests
Status: todo
Depends-on: M1
Definition of done: every wire-format invariant from doc 01 §1.3 has a test
that fails loudly if it drifts, plus golden files pinning schemas and ids.

- [ ] Golden dump of every `CREATE TABLE`/index DDL vs. checked-in `.sql`
      golden files.
- [ ] Embedding-signature format, archivist chunk id, chunk ids,
      `MemoryTaint` fail-closed parsing, `dedupe_key` formats — one pinned
      test each.
- [ ] Property tests (`proptest`) for deterministic ids: same input ⇒ same
      id; distinct sessions ⇒ no collisions in sampled space; canonicalizers
      are idempotent (`canonicalize(canonicalize(x)) == canonicalize(x)`).
- [ ] On-disk vault layout snapshot test (paths, front-matter round-trip).

### T3 — Retrieval effectiveness harness ("perfection & effectiveness")
Status: todo
Depends-on: T1
Definition of done: `benchmarks/effectiveness/` produces recall@k / MRR /
nDCG numbers on labeled datasets from a single command, with results written
to a dated JSON so regressions are diffable across commits.

Correctness tests can't tell us whether retrieval is *good*. Build a small
harness (Rust bin or the existing Python `requirements.txt` toolchain) that:

- [ ] Defines a labeled-dataset format: documents + queries + relevant-ids
      (start with ~50 hand-labeled query/answer pairs over the ingestion
      fixtures; grow with synthetic sets generated once and frozen).
- [ ] Metrics: recall@k (k=1,5,10), MRR, nDCG@10, plus per-weight-profile
      breakdown (BALANCED / SEMANTIC / LEXICAL / GRAPH_FIRST) to validate
      profile differentiation.
- [ ] Two modes: deterministic (inert embedder — measures lexical/graph
      paths only) and real-embedding (Ollama `bge-m3` — full hybrid).
- [ ] Optional LLM-judge groundedness scoring for summarised levels (are
      L1+ summaries faithful to their leaves?) — flagged, off by default.
- [ ] `benchmarks/effectiveness/results/<date>-<git-sha>.json` output +
      a compare script that fails if recall@10 drops >2pts vs. baseline.

### T4 — Performance benchmarking
Status: todo
Depends-on: T1
Definition of done: `cargo bench` runs a criterion suite for hot paths, an
end-to-end throughput bin exists, and `benchmarks/README.md` claims are
reproducible from this repo (or explicitly link the external harness).

- [ ] Criterion micro-benches (`benches/`): chunking, `chunk_id` hashing,
      cosine scan vs. store size (1k/10k/100k vectors), `hybrid_score` +
      `mmr_select`, queue `enqueue`/`claim_next` contention, tree
      `append_leaf`→cascade.
- [ ] Macro bench bin: ingest N docs → drain → M queries; report docs/sec,
      queries/sec, p50/p95 latency, peak RSS, final DB + vault size.
- [ ] SQLite scaling probe: retrieval latency as `mem_tree_chunks` grows to
      1M rows (informs when a real vector index is needed).
- [ ] External-suite wiring: document exactly how the RAGAS / TemporalBench /
      BABILong / Vending-Bench numbers in `benchmarks/README.md` are produced
      (the `run.py`/`scripts/` referenced there don't exist in this repo) —
      either vendor the harness or link the repo + pin versions. Until then,
      mark the README tables as externally produced.
- [ ] Nightly CI job storing bench JSON; alert on >10% regression.

### T5 — CI and coverage gates
Status: todo
Depends-on: T1, T2
Definition of done: CI runs fmt + clippy + test + feature-matrix + e2e on
every PR, with coverage tracked and the live/nightly suites scheduled.

- [ ] GitHub Actions: `cargo fmt --check`, `clippy -D warnings`,
      `cargo test`, feature matrix from C2.
- [ ] `cargo llvm-cov` report; ratchet: coverage may not drop >1pt per PR.
- [ ] Nightly job: `#[ignore]`d live-provider tests against Ollama service
      container, effectiveness harness (T3), benches (T4).
