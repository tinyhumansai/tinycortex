# Audit 09 — Verification Infrastructure (tests, CI, scripts)

_Audit date: 2026-07-14 · Baseline: `main` @ `1a3fcc5`._

Scope: how each building block is verified today — unit/integration coverage
per subsystem, CI feature coverage, setup/testing scripts, hermeticity — and
what is missing for "trustworthy, verifiable building blocks". IDs are `VF-*`.
The cross-cutting *concurrency/crash-test* gap is already the headline of the
improvement plan; it is not repeated here.

## Current state (the good part)

122 sibling `*_tests.rs` files under `src/memory/`; ~60 test modules use
`tempfile` so on-disk tests are hermetic; no unit test touches the network.
Core subsystems are well covered by file count: score (15 test files),
tree (14), queue (11), ingest (11), store (11), retrieval (10). CI runs fmt,
clippy `-D warnings`, an `--all-features` build+test, and a per-feature
`cargo check` matrix. `tests/composio_sync_mock.rs` is a genuinely good
wiremock-driven end-to-end test of all six Composio provider pipelines.

## Findings

### VF-1 (High). The `sync` feature is missing from the CI feature matrix

`.github/workflows/ci.yml` matrix covers `core (--no-default-features)`,
`tokio`, `git-diff`, `providers-http`, `rpc`, and `all` — but not `sync`.
`sync` is the largest feature (23 source files, its own DB, reqwest stack) and
is only ever compiled via `--all-features`, so a `#[cfg(feature = "sync")]`
gating mistake that breaks the *isolated* `--features sync` build (the exact
configuration the Cargo.toml `[[test]]`/`[[example]]` stanzas tell users to
run) would ship green.

**Fix:** add `sync` to the matrix.

### VF-2 (High). Feature-matrix jobs only `cargo check`; release tests less than CI

The matrix runs `cargo check --all-targets` per feature — tests are never
*executed* under `--no-default-features` or any single feature, so a test (or
`dev-dependency` interaction) broken outside `--all-features` is invisible.
Worse, `release.yml` runs plain `cargo test` (default features only) — the
publish gate is weaker than the PR gate.

**Fix:** run `cargo test` at least for `--no-default-features` and
`--features sync` in the matrix, and make release run the same suite as CI.

### VF-3 (High). No setup or test scripts anywhere

There is no Makefile, justfile, or `scripts/` for the crate (the only shell
scripts in the repo build the academic paper under `paper/scripts/`).
Bootstrap is prose: copy `.env.example`, read `Cargo.toml` comments for the
right `cargo run --example … --features …` incantation, read
`benchmarks/README.md` for the bench invocation, `cd viewer && npm i` for the
viewer. Six different invocation styles for one repo, none checked by CI, so
they rot silently.

**Fix:** one `justfile` (or `scripts/`) with the canonical verbs —
`setup`, `check` (fmt+clippy+feature matrix), `test`, `test-sync-mock`,
`bench-effectiveness`, `seed-viewer`, `viewer` — and a CI job that invokes the
same entry points, so the developer path and the CI path cannot diverge. This
is the cheapest single trust win in this audit: every building block gets a
scripted, documented way to verify it.

### VF-4 (Medium). Untested zones map exactly onto the least-trusted code

Zero test files (sibling or inline) for: all six
`sync/composio/providers/*.rs` plus `orchestrator.rs` and `gmail.rs` (only
covered end-to-end via the mock integration test — fine for pipelines, but
provider-specific parsing has no focused tests), `tree/hydrate.rs`,
`archivist/sink.rs`, `diff/{checkpoint,snapshot,diff}.rs`,
`chunks/{schema,store_delete,raw_refs,migrations,produce_split,store_sources,embeddings}.rs`,
`retrieval/rerank.rs`, `conversations/{store_index,store_ops}.rs`,
`score/extract/llm_prompt.rs`, `store/vectors/embedding.rs` (the tinyagents
bridge), `providers/`, `rpc/`. If a module is meant to be a trustworthy block,
"has its own tests runnable in isolation" is the entry bar; these currently
fail it.

### VF-5 (Medium). `tests/smoke.rs` is not a smoke test of the engine

21 lines: insert one record into `InMemoryMemoryStore`, search it. It
exercises the reference backend only — none of content store, chunks, queue,
tree, or retrieval. The improvement plan already calls for a real
ingest→drain→seal→retrieve integration test with a deterministic fake
summariser/embedder; `examples/seed_memory.rs` proves all the offline pieces
exist (it builds a real tree with `ConcatSummariser`, no keys). Converting its
core into `tests/pipeline_e2e.rs` would give CI the first true end-to-end
signal, and would double as the verification harness for every refactor
proposed in audits 07/10.

### VF-6 (Medium). Time-dependent tests use wall-clock `Utc::now()`

Staleness/decay/flush tests derive offsets from real `now`:
`tree/flush_tests.rs` (6 sites), `tree/runtime/engine_tests.rs` (3),
`tree/store/types_tests.rs` (4), `registry_tests.rs:75`, `io_tests.rs`,
`summarise_tests.rs:7`, `factory_tests.rs:81`,
`bucket_seal_label_tests.rs:252`, plus others. They pass today because
offsets are relative, but they encode a hidden dependency on clock behavior
and block any future property/replay testing. The production code already
threads `now: DateTime<Utc>` parameters in most places — tests should pass
fixed timestamps, and the few APIs that call `Utc::now()` internally should
take a clock/now parameter.

### VF-7 (Low). Viewer and benchmarks are outside all verification

- `viewer/` (Next.js): no test script in `package.json`
  (`dev`/`build`/`start`/`lint` only) and CI never builds or lints it. A
  broken viewer ships invisibly. Minimum bar: a CI job running
  `npm ci && npm run build && npm run lint`.
- `benchmarks/effectiveness/`: a real, runnable harness (10-doc fixture,
  recall/precision/MRR/nDCG, own unit tests) but not a workspace member and
  never invoked by CI, so it can drift from the crate API without notice.
  Either add it to a `[workspace]` so `cargo check` covers it, or add a CI
  step that builds it.

### VF-8 (Low). `docs/spec/tests/` specs are not linked to reality

Six large test-specification documents (~200 KB) describe intended coverage
per subsystem, but nothing maps spec sections to actual `*_tests.rs`, so
there is no way to tell which specified cases exist. As the improvement-plan
test work lands, each spec section should get a pointer to the test(s) that
implement it (or an explicit "not implemented" marker) — otherwise the specs
read as coverage that doesn't exist.

### VF-9 (Low). Live-key surfaces are documented but unscripted

`tests/composio_sync_live.rs` (`#[ignore]`, needs `COMPOSIO_API_KEY`) and
`examples/composio_harness.rs` are the only live-verification paths, run by
hand with env set up per `.env.example`. Fold them into the VF-3 scripts
(`just test-live`, `just harness`) so the "verify against the real service"
procedure is one command, and document that they never run in CI.

## Per-subsystem verification scorecard

| Block | Own tests | Isolated (no sibling setup) | Scripted verify | Notes |
| --- | --- | --- | --- | --- |
| fsutil, sources, entities, goals, tool_memory, pii | ✅ | ✅ | ❌ (VF-3) | best-shape blocks |
| store/{kv,vectors,entity_index} | ✅ | ✅ | ❌ | fate per MB-3 |
| chunks, queue, tree, score | ✅ | ❌ shared `chunks.db` (MB-1) | ❌ | rich tests, entangled fixtures |
| retrieval | ✅ | ❌ needs tree+score stores | ❌ | `test_support.rs` imports both |
| graph, conversations, archivist, diff | ✅ thin spots (VF-4) | partial | ❌ | |
| sync providers/orchestrator | ❌ unit; ✅ mock e2e | ❌ | ❌ | not in CI matrix (VF-1) |
| ingest | ✅ | ❌ | ❌ | no true e2e (VF-5) |
| providers, rpc | ❌ | — | — | placeholder seams (SW-2) |
| viewer, benchmarks | partial | ✅ | ❌ not in CI (VF-7) | |

The rightmost two columns are this audit's summary: almost nothing has a
scripted verification path, and the core cluster can't be verified in
isolation until MB-1/MB-2 land.
