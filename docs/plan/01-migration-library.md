# 1. Migrating OpenHuman Memory Components into the Library

Scope: what still moves from `../openhuman-1/src/openhuman/*` into TinyCortex,
what stays host-side forever, and the invariants that must survive the move.

## 1.1 What is already ported (do not redo)

All of these live under `src/memory/` with sibling `*_tests.rs` coverage and
match the spec in `docs/openhuman-memory/`:

`types` / `traits` / `config` / `error`, `store` (content vault, packed-f32
vector DB, KV, entity index, safety), `chunks`, `score`, `tree` (bucket-seal +
markdown time-tree), `queue`, `retrieval`, `ingest`, `sources` (registry +
local readers), `diff`, `entities`, `graph`, `goals`, `tool_memory`,
`conversations`, `archivist`.

## 1.2 What stays in OpenHuman (non-goals, by design)

- `memory` (orchestration/RPC glue) — TinyCortex re-exposes the *contracts*
  (`Memory` trait, `MemoryEntry`, `RecallOpts`), not the app glue.
- `memory_sync` + sync runners, OAuth/webhook callbacks, Composio.
- `memory_sources` network connectors (GitHub/RSS/Twitter/web) — TinyCortex
  keeps only the registry, contracts, and local `folder`/`conversation`
  readers.
- `agent_memory` (retrieval sub-agent definition) and `context` (session
  context bookkeeping) — consumers of the memory API, not part of it.
- Credential storage, event bus, observability, scheduler gating — injected
  via the host-hook traits below.

## 1.3 Wire-format invariants (must hold across every goal)

Any migration or refactor that breaks one of these breaks OpenHuman adoption:

1. Embedding signature string: `provider=<name>;model=<model_id>;dims=<dims>`
   (`embeddings/provider_trait.rs:13` in OpenHuman). A mismatch silently
   splits the embedding space.
2. Archivist leaf id: `chunk_id = sha256(session_id ‖ md)[..32]`.
3. All `mem_tree_*`, `kv_*`, `graph_*` SQLite table schemas, including
   `mem_tree_jobs.dedupe_key` and its partial unique index
   (`WHERE status IN ('ready','running')`).
4. `MemoryTaint` DB strings `internal` / `external_sync`, failing **closed**
   to `external_sync` on unknown values.
5. On-disk layout: DB at `<workspace>/memory_tree/chunks.db`, markdown vault
   under the content root, Obsidian-compatible paths.
6. Chunk ids, source-id prefixes (per-source status queries use
   `LIKE 'prefix%'` against `mem_tree_chunks`).

Add a dedicated invariant test suite for these (see T1.4 in doc 03).

---

### M1 — Drift audit: rebase the port onto `openhuman-1`
Status: todo
Depends-on: -
Definition of done: a written drift report (`docs/plan/drift-report.md`) listing
every behavioral/schema difference between TinyCortex modules and their
`openhuman-1` counterparts, each classified as port-forward / ignore /
host-owned, with follow-up goals filed for every "port-forward".

The existing port used the `openhuman-workflow` checkout;
`openhuman-1` is at `4c98a31` and has visibly diverged. Known suspects:

- [ ] `memory_queue`: `openhuman-1` `JobKind` includes `topic_route` and
      `digest_daily`; TinyCortex retired topic/digest jobs. Confirm which
      direction is newer and whether retirement is migration-safe against
      `openhuman-1`'s `mem_tree_jobs` rows.
- [ ] `memory_store`: diff all `CREATE TABLE` / `add_column_if_missing` /
      `migrate_legacy_embeddings_to_sidecar` / `purge_global_topic_trees`
      schema evolution against TinyCortex's `chunks/` schema.
- [ ] `memory_tree` retrieval: diff `walk`, `smart_walk`, `drill_down`,
      `cover_window`, `query_source`, `search_entities`, scoring weights, and
      the E2GraphRAG-style deterministic walk against `retrieval/`.
- [ ] Post-#4249 changes (agent_graph rework moved live summarization out of
      `context` into tinyagents) — check whether archivist/summarise inputs
      changed shape.
- [ ] `memory_conversations`: diff JSONL formats, purge semantics, inverted
      index behavior.
- [ ] Connection layer: verify TinyCortex has the production hardening from
      `memory_store/chunks/connection.rs` — per-path connection cache,
      circuit breaker (threshold 3, cooldown 30s), cold-start WAL/SHM
      transient-error classification (SQLite codes 14/1546/4618/4874/5386/
      8714), mutex-gated one-time schema init. Port whatever is missing.
- [ ] Re-run the invariant list (§1.3) against `openhuman-1` sources and
      correct any that changed upstream.

### M2 — Host-hook traits for app services
Status: todo
Depends-on: M1
Definition of done: the four app-service coupling points exist as small traits
in `src/memory/` with no-op defaults, consumed by queue/embeddings/sync-facing
code, each with unit tests exercising a recording fake.

OpenHuman's queue worker and embedding stack call app services directly; the
library needs seams so those calls become injections:

- [ ] `ThrottleGate` (replaces `scheduler_gate::wait_for_capacity`) — async
      permit before each queue job; default: always-open.
- [ ] `ErrorReporter` (replaces `core::observability::report_error`) —
      default: log via `tracing`/stderr.
- [ ] `EventSink` (replaces `DomainEvent` publishes: sync-stage changes, tree
      events, startup breadcrumbs) — typed event enum owned by TinyCortex;
      default: drop.
- [ ] `CredentialResolver` (replaces `credentials::AuthService` lookups for
      provider API keys) — default: env-var lookup; hosts inject keyring.
- [ ] Thread the traits through `queue::worker` config and provider
      factories; keep constructors backward-compatible via builder defaults.

### M3 — Extract the embeddings provider family
Status: todo
Depends-on: M2
Definition of done: TinyCortex ships working `Embedder`/`EmbeddingBackend`
implementations for Ollama, OpenAI-compatible, Voyage, and Cohere behind
feature flags, sharing OpenHuman's signature format, rate limiting, and retry
semantics; OpenHuman's cloud/managed provider remains host-injected.

- [ ] Port `provider_trait.rs` semantics: `format_embedding_signature` as the
      single source of truth (re-export it; add a compile-time-visible doc
      warning about space-splitting).
- [ ] Port OSS providers: `ollama.rs`, `openai.rs` (OpenAI-compatible),
      `voyage.rs`, `cohere.rs`, `noop.rs` — strip `credentials`/`api::config`
      deps in favor of `CredentialResolver` + explicit base URLs in config.
- [ ] Port `rate_limit.rs` (per-endpoint token bucket) and `retry_after.rs`
      (429/503 backoff) as shared provider middleware.
- [ ] Port the slug `catalog.rs` and defaults (cloud `embedding-v1`/1024,
      Ollama `bge-m3`/1024) minus the managed-cloud entry.
- [ ] Unit tests against a local mock HTTP server (no live services), plus
      signature-stability golden tests.

### M4 — Port the connection-hardening + init smoke surface
Status: todo
Depends-on: M1
Definition of done: TinyCortex's SQLite connection layer has the cache,
circuit breaker, and cold-start classification from OpenHuman, and a ported
`memory_tree_init_smoke`-style concurrency test lives in `tests/`.

- [ ] Port/verify per-path connection cache + circuit breaker + transient
      cold-start classification (see M1 checklist item).
- [ ] Port `bin/memory_tree_init_smoke.rs` as an integration test racing N
      threads into first-touch schema init on a temp workspace.
- [ ] Document the connection contract in module docs (`with_connection`
      semantics, busy_timeout, WAL).

### M5 — Fold in remaining small extractables
Status: todo
Depends-on: M1
Definition of done: each item below is either ported with tests or explicitly
recorded as host-owned in this doc.

- [ ] `memory_store/unified/` legacy-store read paths — decide: port a
      read-only compat shim (needed for I2 parity testing) or declare
      host-owned migration context.
- [ ] `memory_store/retrieval/` + `fts5` helpers not yet covered by
      `retrieval/` — diff and port gaps.
- [ ] `memory/preferences.rs` (`USER_PREF_GENERAL_NAMESPACE`,
      `load_general_preferences`, `recall_situational_preferences`) — pure
      functions over the `Memory` trait; good library citizens.
- [ ] `memory/source_scope.rs` (`with_source_scope`) — port if TinyCortex
      `ingest` doesn't already cover scoped ingest.
- [ ] Consolidate legacy `score::store` entity-index helpers around
      `store::entity_index` (known follow-up from the port).
- [ ] Restore deferred peripheral surfaces where they are host-independent:
      tree `health` (corrupt-DB recovery used by the queue worker), tree
      `nlp` helpers.

### M6 — Config-shape parity
Status: todo
Depends-on: M1
Definition of done: `MemoryConfig` (plus `EmbeddingConfig`, `TreeConfig`,
`RetrievalConfig`, `SyncBudgetConfig`) can represent every knob OpenHuman's
memory stack actually reads, and a documented mapping table exists.

- [ ] Enumerate every `config.*` field OpenHuman memory code reads (audit
      found only `workspace_dir`, `memory_tree_content_root()`, embedding
      route/provider fields, `workload_uses_local("memory")`, rate limits).
- [ ] Map each onto TinyCortex config; add missing fields.
- [ ] Write the OpenHuman→TinyCortex config mapping table into
      `docs/plan/04-openhuman-integration.md` §4.5 as it firms up.
