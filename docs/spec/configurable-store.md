# Spec: Configurable Store Backend (Local-First and Server-Hosted)

## Goal

The same agent code must run against either the local-first store
(markdown + SQLite on the host filesystem) or a server-hosted backend, selected
by configuration — so agents can be embedded in a local app **or hosted on a
server**, without forking orchestration code.

## Where we are today

The audit ([06-contracts-config-api.md](audit/06-contracts-config-api.md))
found the crate is not pluggable in practice:

1. **Two parallel, disconnected store contracts** (CT-4, CT-5):
   - `memory::traits::Memory` — the headline async trait
     (`recall_relevant`, `recall_relevant_by_vector`, namespace queries). Its
     only implementor is a `#[cfg(test)]` mock; a host cannot construct
     `Arc<dyn Memory>` today, yet `ToolMemoryStore::new` requires one.
   - `memory::store::MemoryStore` — a smaller trait implemented only by
     `InMemoryMemoryStore` (whose search is degenerate, CT-1/CT-3).
2. **The real engine bypasses both.** The production paths — chunk store,
   content store, KV, vectors, entity index, queue, trees, conversations,
   diff ledger — are concrete structs bound to filesystem paths and a
   process-global SQLite connection cache (`chunks/connection.rs`), with
   sync/blocking APIs (SC-22). Orchestration (`ingest`, `queue::handlers`,
   `tree`) calls these concrete types directly, not traits.
3. **The remote seams are stubs.** The `rpc` feature is a doc-only module
   reserving the wire surface; `providers-http` is a `reqwest` type alias.
4. **Filesystem semantics leak upward**: error classification keys on host-FS
   error strings (`is_host_io_error`), recovery logic manipulates `-wal`/`-shm`
   side files, goals/entities/sources are markdown files edited in place.

Conclusion: "configurable store" is not a feature flag away; it needs a
deliberate trait boundary first. That consolidation is worth doing even if the
server backend ships later — it also fixes CT-4/CT-5.

## Design

### 1. One storage-facade layer: `MemoryBackend`

Introduce a single object-safe facade that owns all storage primitives, and
make orchestration depend on it instead of concrete stores:

```rust
/// Everything the engine needs from a storage backend.
/// Object-safe; all methods async.
pub trait MemoryBackend: Send + Sync {
    fn chunks(&self) -> &dyn ChunkStore;
    fn content(&self) -> &dyn ContentStore;
    fn kv(&self) -> &dyn KvStore;
    fn vectors(&self) -> &dyn VectorStore;
    fn entity_index(&self) -> &dyn EntityIndexStore;
    fn trees(&self) -> &dyn TreeStore;        // buffers + summaries + registry
    fn queue(&self) -> &dyn JobStore;          // claim/settle/enqueue_tx
    fn conversations(&self) -> &dyn ConversationStore;
    fn documents(&self) -> &dyn NamespaceStore; // the current `Memory` surface
}
```

The sub-traits are extracted from the existing concrete impls' public methods
(they are already cohesive modules). Extraction rules:

- **Async-first.** Trait methods are async; the local backend wraps its
  synchronous SQLite/file calls in `spawn_blocking` (fixing SC-22/QI-4-adjacent
  hazards at the boundary instead of at every call site).
- **Transactions become backend operations.** The engine's cross-store atomic
  sequences (gate+upsert+enqueue in ingest QI-1; seal's
  insert-summary+trim-buffer+enqueue TR-1/QI-5) must be expressible without
  handing raw `rusqlite::Transaction` across the boundary. Model each as a
  single compound backend method (e.g. `ChunkStore::ingest_batch(...)`,
  `TreeStore::commit_seal(...)`) rather than exposing a generic transaction
  object — compound methods stay object-safe and map cleanly onto a remote
  API, and they force the atomicity fixes from phases 0–1 to live behind the
  boundary where every backend must honor them.
- **Error taxonomy, not error strings.** Replace host-FS string sniffing with a
  `BackendError { kind: Transient | Unavailable | Corrupt | Conflict |
  Permanent, .. }` enum. The queue's backoff policy (QI-2/QI-3) then keys on
  `kind`, and a server backend maps HTTP/status codes into the same taxonomy.

### 2. Consolidate the public contracts (CT-4, CT-5)

- `memory::traits::Memory` becomes a thin re-export of
  `NamespaceStore` (or is implemented blanket-wise for any `MemoryBackend`),
  so `Arc<dyn Memory>` is constructible from any backend — unblocking
  `ToolMemoryStore` and `goals` on both backends.
- `store::MemoryStore` + `InMemoryMemoryStore` are re-positioned as the
  reference `MemoryBackend` implementation (`backend = "memory"`), useful for
  tests and the README example. Its search is fixed to term-based scoring
  (CT-1/CT-3) as part of the move.
- Rename the 2-variant `store::types::MemoryError` to `StoreError` and stop
  re-exporting it at the module root.

### 3. Config surface

`MemoryConfig` gains a tagged backend section (serde `tag = "kind"`, wire
strings stable per the OpenHuman-contract rule):

```toml
[storage]
kind = "local"              # "memory" | "local" | "server"
root = "~/.tinycortex"      # local only

# server-hosted:
# kind = "server"
# base_url = "https://cortex.internal:8443"
# auth = { kind = "bearer_env", var = "TINYCORTEX_TOKEN" }
# namespace = "agent-7"     # tenant scoping
# request_timeout_ms = 10000
```

Construction becomes a single factory:

```rust
let backend: Arc<dyn MemoryBackend> = tinycortex::memory::open(&config).await?;
let engine = MemoryEngine::new(backend, config);
```

`open` validates the config (CT-6's `validate()` lands here) and returns:
- `memory` → `InMemoryBackend` (no features needed),
- `local` → `LocalBackend` (today's SQLite+markdown engine, default feature),
- `server` → `ServerBackend` (requires the `server-client` feature; compile
  error with a clear message otherwise).

### 4. Server backend (`server-client` feature)

Fills the reserved `rpc` + `providers-http` seams:

- **Wire protocol:** JSON envelopes over HTTP as already sketched in
  `rpc/mod.rs` doc comments (ids and enum wire strings preserved from
  OpenHuman). One endpoint per compound backend method; idempotency keys on
  every mutating call (the queue's at-least-once semantics require server-side
  dedup — QI-5's idempotency contract becomes part of the wire spec).
- **Client:** `reqwest`-based, retry with jittered backoff on
  `Transient`/`Unavailable`, never on `Permanent`/`Conflict`; deadline
  propagation from `request_timeout_ms`.
- **Taint fail-closed on the wire** (CT-2): unknown taint strings decode as
  `external_sync`; missing taint is a decode error, not `Internal`.
- **What stays client-side vs server-side:** in server mode the queue
  *workers* (LLM calls, summarisation) can run either co-located with the
  server or in the agent process. v1: workers run server-side only — the agent
  process does ingest + retrieval calls; the server owns the queue, trees, and
  scoring. This avoids shipping the LLM gate and lease semantics over the
  wire.
- **Server binary is out of scope for this crate** (the crate is a library);
  the spec only defines the client + wire contract so the hosted platform can
  implement the server against it.

### 5. What must NOT leak across the boundary

Checklist for review (each is a current local-backend implementation detail):

- `chunks.db` path handling, journal-mode pragmas, `-wal`/`-shm` recovery
  (SC-4/SC-5) — internal to `LocalBackend`.
- Markdown front-matter composition (SC-1/SC-2) — internal to `LocalBackend`'s
  `ContentStore`.
- `with_connection` global cache and its mutex — internal; the facade is the
  only public entry.
- Host-FS error-string classification (`is_host_io_error`) — replaced by the
  `BackendError` taxonomy at the boundary; local backend keeps the string
  sniffing inside its error-mapping layer.
- Direct file paths in `goals`, `entities`, `sources`, `diff` — these modules
  currently open files themselves. v1 scopes them to local mode
  (feature-documented); v2 moves them onto `KvStore`/`ContentStore` so
  server-hosted agents get goals/tool-memory too. `diff` (git-backed) stays
  local-only by nature.

## Migration plan

Incremental, each step keeps `cargo test` green:

1. **M1 — extract traits, local impl adopts them.** Define the sub-traits +
   `MemoryBackend`; implement for the existing concrete structs (mostly
   mechanical `impl` blocks + `spawn_blocking` wrappers). No caller changes.
2. **M2 — orchestration flips to the traits.** `ingest::pipeline`,
   `queue::handlers`, `tree`, `retrieval` take `&dyn MemoryBackend` (or
   generics where hot). Compound methods introduced here must absorb the
   phase-0/1 atomicity fixes (QI-1, TR-1, QI-5) — do those fixes first or
   together.
3. **M3 — config + factory.** `[storage]` section, `open()`, `memory`
   backend re-positioned, `Memory` trait consolidation (CT-4/CT-5).
4. **M4 — error taxonomy.** `BackendError` + queue backoff keyed on `kind`
   (subsumes QI-2/QI-3 fixes if not already landed).
5. **M5 — wire contract + server client.** Serde envelope types under `rpc`
   (schema tests only, no I/O), then the `reqwest` client under
   `server-client`, tested against a local axum/wiremock stub.
6. **M6 — conformance suite.** One shared test suite (`backend_conformance`)
   run against `memory`, `local`, and the stubbed `server` client — the same
   scenarios the integration tests from the improvement plan's test strategy
   introduce. This is the guarantee that "configurable" means "substitutable".

## Open questions

1. **Multi-tenancy model for server mode** — one namespace per agent
   (proposed above) vs. per-user; affects wire auth scoping.
2. **Vector search on the server** — reuse the local cosine scan contract, or
   allow the server to substitute ANN (results contract: top-k with scores;
   exactness not promised)?
3. **Should `goals`/`tool_memory` be v1 server features** (they only need
   KV + one markdown doc) or wait for v2? Recommendation: v1 — they're the
   surfaces hosted agents need most.
4. **Generics vs `dyn` on hot paths** — retrieval loops may want
   monomorphization; measure before committing (benchmarks/ exists).
