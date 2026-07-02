# 2. Completing TinyCortex: Missing Pieces

Scope: work that is internal to this crate — the layers named in
`docs/openhuman-memory/` but not yet in `src/`, plus library ergonomics.
The engine below the orchestrator is done and green; everything here builds
on top of it without changing storage semantics.

### C1 — Engine facade (`CortexEngine`)
Status: todo
Depends-on: -
Definition of done: a host can go from `MemoryConfig` + a workspace dir to a
working ingest→queue→tree→retrieval loop with ~10 lines of code, without
touching any submodule directly; covered by an integration test and an
example.

Today a host must hand-wire `config → ingest → queue → tree → retrieval`.
This is the single biggest adoption blocker (`core-orchestration.md` is the
spec; the OpenHuman equivalent is the `memory` module's orchestration role).

- [ ] New module `src/memory/engine/` (types.rs / test.rs convention, <500
      LOC per file) exposing `CortexEngine`.
- [ ] Builder: `CortexEngine::builder(config)` with injection points for
      `Memory`, `Embedder`, `ChatProvider`, `Summariser`, `GoalsGenerator`,
      `ThrottleGate`, `ErrorReporter`, `EventSink`, `CredentialResolver` —
      every seam defaults to the existing inert/no-op impl so
      `builder(config).build()?` always works deterministically.
- [ ] Facade methods mirroring OpenHuman's orchestration surface:
      `ingest_document / ingest_chat / ingest_email` (delegating to
      `ingest::*` + enqueueing via the real queue, not `NullJobSink`),
      `query` (hybrid retrieval with weight profiles), `recall` / `remember`
      / `forget` / `get` / `list` (via the `Memory` trait),
      `archive_conversation`, `goals()`, `entities()`, `diff()` accessors.
- [ ] Queue driving: `engine.tick()?` (one `run_once`) and
      `engine.drain_until_idle()?`; behind a `tokio` feature, a
      `spawn_workers(n)` loop with stale-lock recovery on start (mirrors
      OpenHuman's 3-worker pool + `recover_stale_locks`).
- [ ] Wire `QueueDelegates` to the real handlers (extract → score → append →
      seal → flush → reembed-backfill) so a drained queue leaves a sealed
      tree.
- [ ] Re-export `CortexEngine` from `lib.rs` as the front door.

### C2 — Cargo features and crate hygiene
Status: todo
Depends-on: -
Definition of done: `Cargo.toml` has a `[features]` section; `cargo check
--no-default-features` and each feature combo compile; heavyweight deps are
opt-in.

- [ ] `default = []` core stays sync + dependency-light.
- [ ] `tokio` — background worker loops (moves tokio out of dev-deps).
- [ ] `git-diff` — gate `git2` (heavy native dep) behind a feature; `diff/`
      compiles out cleanly.
- [ ] `providers-http` — reqwest-based embedding/LLM providers (C3/M3).
- [ ] `rpc` — serde schema/envelope surface (C5).
- [ ] CI matrix job compiling the power set (or a curated subset) of
      features.

### C3 — Real provider implementations
Status: todo
Depends-on: C2, M3
Definition of done: with `providers-http` enabled, a user can run the engine
against a local Ollama (embeddings + chat + summarise) and against any
OpenAI-compatible endpoint, with integration tests marked `#[ignore]` for
live-service runs.

- [ ] `Embedder`/`EmbeddingBackend` impls: delivered by M3 (Ollama,
      OpenAI-compatible, Voyage, Cohere).
- [ ] `ChatProvider` impls: Ollama + OpenAI-compatible chat completions
      (this unblocks `LlmEntityExtractor`).
- [ ] `Summariser` impl backed by `ChatProvider` (port the prompt shape from
      OpenHuman `memory_tree/summarise.rs`).
- [ ] `GoalsGenerator` impl backed by `ChatProvider` (goal reflection).
- [ ] Provider conformance test suite: one shared test template run against
      every impl via a mock HTTP server; live tests `#[ignore]`d and
      documented.

### C4 — Controller registry and agent tool registry
Status: todo
Depends-on: C1
Definition of done: `src/memory/controllers/` and `src/memory/tools/` exist
per `controller-tool-registry.md`, exposing schema+handler pairs over
`CortexEngine`, with the OpenHuman controller namespaces and agent tool names
preserved verbatim.

- [ ] Controller registry: JSON-schema'd request/response handlers for the
      `memory_*`, `memory_tree_*`, `retrieval_*`, `tree_summarizer_*`,
      `memory_goals_*`, `memory_diff_*` namespaces (the surface OpenHuman
      registers in `core/all.rs`). Handlers call `CortexEngine`; transport
      stays host-owned.
- [ ] Agent tool registry: LLM-facing tool definitions (walk, smart_walk,
      drill_down, fetch_leaves, cover_window, search_entities,
      memory_recall, query_memory, goals CRUD, tool-memory rules) with the
      exact OpenHuman tool names/ids as wire strings.
- [ ] Golden tests pinning every controller/tool name and schema (wire
      compatibility is the whole point of this goal).

### C5 — RPC envelope / schema surface
Status: todo
Depends-on: C4
Definition of done: the `query/`, `read_rpc/`, `schemas/` request/response
envelopes from OpenHuman's `memory` module exist as serde types behind the
`rpc` feature, round-trip-tested against captured OpenHuman JSON fixtures.

- [ ] Capture real request/response JSON from `openhuman-1` tests as
      fixtures.
- [ ] Port envelope types + validation; keep field names byte-identical.
- [ ] Round-trip serde tests over the fixtures.

### C6 — Documentation, examples, and packaging
Status: todo
Depends-on: C1
Definition of done: `cargo doc` is clean with `#![warn(missing_docs)]`;
`examples/` demonstrates the three core journeys; the crate is publishable
(`cargo publish --dry-run` passes).

- [ ] `examples/ingest_and_query.rs` — folder source → ingest → drain →
      query (inert providers; fully offline).
- [ ] `examples/conversation_archive.rs` — record turns → archive → tree →
      retrieval.
- [ ] `examples/ollama_engine.rs` — real providers behind `providers-http`.
- [ ] `#![warn(missing_docs)]` in `lib.rs`; fill gaps.
- [ ] README "quick start" updated to the `CortexEngine` API.

### C7 — Memory dynamics (paper alignment; stretch)
Status: todo
Depends-on: C1
Definition of done: reinforcement-on-recall and write-back decay exist as an
opt-in policy module with deterministic tests; the interval "consciousness
loop" is expressible as a host-driven schedule over `CortexEngine`, and the
paper's Phase 2–4 claims each map to either shipped code or an explicit
"future work" note.

Only retrieval-time freshness decay exists today; the paper's reweighting /
reinforcement / interval-recall loop is unimplemented. Keep it strictly
additive and feature-gated — it must not change default scoring.

- [ ] `score` write-back: reinforcement bump on recall hit, Ebbinghaus-style
      decay job (`JobKind` addition — coordinate with M1 queue drift
      findings before touching job kinds).
- [ ] `engine.reflect()` — interval recall + thought-synthesis hook via
      `ChatProvider` (host decides cadence; no internal timer).
- [ ] Update `paper/` or crate docs so claims and code agree.
