# 4. Plugging TinyCortex into OpenHuman Without Breaking It

Scope: how OpenHuman adopts TinyCortex incrementally (strangler-fig), what
compatibility guarantees make each step a no-op for users, and the rollback
story at every phase.

## 4.1 Why a drop-in is feasible

- OpenHuman's dependency direction is already clean: `memory` (orchestration)
  → `memory_tree` → `memory_store`; nothing in the storage core imports
  upward. TinyCortex replaces the *bottom* of that stack first.
- TinyCortex preserves the on-disk contract (doc 01 §1.3): same
  `<workspace>/memory_tree/chunks.db` path, same table schemas, same markdown
  vault, same deterministic ids. Old engine and new engine read and write
  the same bytes — which is what makes shadow-running and rollback cheap.
- OpenHuman's own abstractions (`Memory` trait, `ChatProvider`,
  `EmbeddingProvider`) already match TinyCortex's seams almost 1:1.

## 4.2 Integration shape

One new app-side module in OpenHuman: `src/openhuman/tinycortex_adapter/`
(alternatively a small `openhuman-tinycortex` crate). It owns all glue:

- Builds `tinycortex::MemoryConfig` from OpenHuman `Config`
  (`workspace_dir`, `memory_tree_content_root()`, embedding route fields —
  the mapping table from M6).
- Implements TinyCortex's host-hook traits with OpenHuman services:
  `ThrottleGate` → `scheduler_gate`, `ErrorReporter` → `core::observability`,
  `EventSink` → `event_bus::DomainEvent`, `CredentialResolver` →
  `credentials::AuthService`.
- Implements `tinycortex` `ChatProvider`/`Embedder` over OpenHuman's
  `inference::provider` and managed cloud embedding provider.
- Exposes `CortexEngine` behind OpenHuman's existing `Memory` trait so all 30
  `create_memory` call sites are untouched.

A config flag gates everything: `memory.engine = "legacy" | "shadow" |
"tinycortex"` (default `legacy` until I3 completes).

---

### I1 — Adapter + shadow mode (zero behavior change)
Status: todo
Depends-on: C1, M2, M6
Definition of done: OpenHuman builds with `tinycortex` as a dependency; in
`shadow` mode every memory write goes to both engines and every read is
served by legacy while TinyCortex results are compared and logged; `legacy`
mode is byte-identical to today.

- [ ] Add the `tinycortex` path/git dependency + `tinycortex_adapter`
      module; implement the hook traits and provider adapters.
- [ ] Shadow writer: mirror `store`/`ingest_document`/`record_turn` into a
      TinyCortex engine pointed at a **separate** shadow workspace dir (never
      the live DB — the schemas match, but double-writing one DB from two
      engines is the risk we're avoiding).
- [ ] Shadow reader: on `recall`/`query`, also query TinyCortex; diff result
      ids/ordering; emit a parity metric (no user-visible effect).
- [ ] Kill switch: flag flip back to `legacy` requires no data repair.

### I2 — Parity proof
Status: todo
Depends-on: I1, T1, T2
Definition of done: a dual-run parity suite in OpenHuman CI replays a fixture
corpus through both engines and asserts equal retrieval results and
equivalent DB/vault state; one week of shadow-mode telemetry shows ≥99%
query-parity with every divergence triaged.

- [ ] Fixture replay harness: same ingest stream → legacy and TinyCortex →
      compare `mem_tree_*` row counts, tree shapes, retrieval top-k.
- [ ] Triage every divergence into: TinyCortex bug (fix), legacy bug (fix
      upstream or accept improvement), or intentional drift (document in
      drift report).
- [ ] Sign-off checklist recorded in this doc before I3 starts.

### I3 — Cutover by layer (strangler-fig)
Status: todo
Depends-on: I2
Definition of done: with `memory.engine = "tinycortex"`, all listed layers are
served by TinyCortex against the **live** workspace, each step shipped as its
own PR with its own revert path, ordered leaf-first:

- [ ] 1. `memory_archivist` → `tinycortex::memory::archivist` (pure
      transforms, deterministic ids; lowest risk).
- [ ] 2. `memory_entities` → `tinycortex::memory::entities` (file-backed,
      no SQLite).
- [ ] 3. `memory_conversations` store → `tinycortex::memory::conversations`
      (keep the `DomainEvent` subscriber shim app-side, calling into the
      TinyCortex store).
- [ ] 4. `memory_goals` / `memory_tools` → `goals` / `tool_memory`.
- [ ] 5. `memory_store` + `memory_tree` reads/writes → `CortexEngine` over
      the live `chunks.db` (schemas identical per T2 goldens; run
      `PRAGMA integrity_check` + row-count audit on first open).
- [ ] 6. `memory_queue` worker → engine workers with injected
      `ThrottleGate`; drain legacy queue to empty before switching claimers
      (both honor the same `mem_tree_jobs` dedupe semantics — never run both
      claim loops at once).
- [ ] 7. `embeddings` OSS providers → TinyCortex providers (M3); managed
      cloud provider stays app-side, injected.
- [ ] Each step: PR + parity suite green + one-flag revert documented.

### I4 — Retire duplicated code
Status: todo
Depends-on: I3
Definition of done: OpenHuman's `memory_store`, `memory_tree`, `memory_queue`
implementations are deleted (or reduced to re-export shims), `memory`
orchestration and RPC controllers call `CortexEngine`/TinyCortex registries
(C4), and the `Memory::sqlite_conn` escape hatch is gone.

- [ ] Point OpenHuman RPC registration at TinyCortex controller registry
      (C4) — names/schemas pinned by golden tests, so no client changes.
- [ ] Replace `Memory::sqlite_conn` (raw `rusqlite::Connection` leak used by
      `ArchivistHook` for FTS5) with a purpose-built TinyCortex API
      (`fts_search`/`fts_index` or an archivist hook trait) — do this
      *before* deleting the legacy trait, coordinating with OpenHuman.
- [ ] Delete superseded modules; keep `memory` (orchestration), `memory_sync`,
      `memory_sources` connectors, `agent_memory`, `context` app-side as
      TinyCortex consumers.
- [ ] Remove the `shadow` machinery and the `legacy` engine flag after one
      release of `tinycortex`-default with no rollback.

## 4.5 Config mapping (filled by M6)

| OpenHuman `Config` | TinyCortex | Notes |
| --- | --- | --- |
| `workspace_dir` | `MemoryConfig.workspace_dir` | DB + queue root |
| `memory_tree_content_root()` | `MemoryConfig.content_root` | markdown vault |
| embedding provider/model/dims | `EmbeddingConfig` | signature-critical |
| `workload_uses_local("memory")` | host-side, via `ThrottleGate` | stays app policy |
| rate-limit fields | provider middleware config (M3) | |

(Extend this table as M6 lands.)

## 4.6 Risks

- **Two claim loops on one jobs table** (I3.6) — mitigated by drain-then-
  switch and never running both workers concurrently.
- **Embedding-space split** if signature or provider defaults drift —
  mitigated by T2 golden tests on both repos.
- **Schema drift after cutover**: once OpenHuman deletes its schema code,
  TinyCortex owns migrations; version the schema (`store_meta`) and add a
  refuse-to-open-newer guard before I4.
- **Drift audit surprises (M1)**: if `openhuman-1`'s queue/retrieval diverged
  materially from the ported base, I-phase timelines move — M1 is first for
  exactly this reason.
