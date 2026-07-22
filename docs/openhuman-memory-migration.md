# OpenHuman Memory Migration

This repository now has a Rust crate rooted at the repository root. The first
migration target is the memory core: stable contracts, storage primitives, and
testable in-process behavior before API or UI integrations.

TinyCortex owns the generic sync engine and provider pipelines behind its
optional `sync` feature. OpenHuman retains scheduling, credentials, RPC,
source-scope/redaction policy, and event-bus publishing, and supplies those
product concerns through the sync adapter traits.

## Source Modules

Compare against the vendoring host's `src/openhuman/` tree; do not encode a
developer-specific absolute checkout path in migration notes.

- `memory/`: orchestration for query, remember, OpenHuman-triggered ingest, and
  RPC surfaces.
- `memory_store/`: storage primitives for content, chunks, trees, vectors, KV,
  entities, and the shrinking unified store. TinyCortex splits those across
  `store/`, `chunks/`, and `tree/`; the unified store is migration context.
- `memory_tree/`: tree mechanics, summary sealing, retrieval, scoring, and
  entity extraction.
- `memory_queue/`: SQLite-backed async jobs for extraction, append, seal,
  re-embed backfill, document sealing, and stale flushes. Old topic/digest jobs
  are retired and migration-safe.
- `memory_search/`, `memory_graph/`, `memory_entities/`, `memory_sources/`:
  specialized retrieval, graph, source contracts, and validation layers.

## Target Layout

The memory engine now lives under `src/memory/` as cohesive modules:

- `types.rs`, `traits.rs`, `config.rs`, `error.rs`: stable shared contracts.
- `store/`: content, generic vectors, KV, safety helpers, entity index, and the
  starter store.
- `chunks/`, `tree/`, `queue/`, `retrieval/`, `score/`: chunk/tree pipeline.
- `sources/` and `ingest/`: source contracts, local readers, canonicalizers,
  and on-demand ingest orchestration.
- `diff/`, `entities/`, `graph/`, `goals/`, `tool_memory/`,
  `conversations/`, `archivist/`: specialized memory surfaces.

Future host adapters should keep OpenHuman's layer rule: orchestration depends
on storage, but storage does not depend upward on orchestration. The OpenHuman
memory sync module remains outside this crate.

## Migration Order

1. Port pure data types and tests that do not depend on OpenHuman runtime state.
2. Port content and chunk storage behind the `MemoryStore` contract.
3. Port tree IO and bucket-seal mechanics with storage injected by traits.
4. Port queue workers only after persistence and deterministic drain tests exist.
5. Add OpenHuman-facing on-demand ingest adapters; do not port the live sync
   scheduler into TinyCortex.

## Port Status

The memory engine is ported as the config-driven library under `src/memory/`.
All modules compile together, are wired into `src/memory/mod.rs`, and ship unit
tests in sibling `*_tests.rs` files (1000+ tests; `cargo fmt` and `cargo test`
are the current validation gates).

| Module | OpenHuman source | Notes |
| --- | --- | --- |
| `types` / `traits` / `config` / `error` | `memory`, `memory_store` | Shared contracts; `MemoryConfig` drives all tunables. |
| `chunks` | `memory_store/chunks` | Chunk model, deterministic ids, SQLite chunk store + full `mem_tree_*` schema. |
| `store` | `memory_store` (content/vectors/kv/entities) | Markdown content store, packed-f32 vector DB, KV, entity occurrence index. |
| `score` | `memory_tree/score` | Signals, regex/composite extraction, resolver, score store. LLM rater + embedder behind traits. |
| `tree` | `memory_tree` + `memory_store/trees` | Tree rows, buffers, bucket-seal, summarise, tree-walk read. Summariser/embedder behind traits. |
| `queue` | `memory_queue` | `mem_tree_jobs` store, dedupe/defer/backoff, worker, LLM gate. Handler work behind `QueueDelegates`. |
| `retrieval` | `memory_search` + `memory_tree/retrieval` | Hybrid primitives (`query_source/global/topic`, `drill_down`, `fetch_leaves`), config weight profiles. |
| `ingest` | `memory/ingest_pipeline`, `memory/ingestion`, `memory_sync/canonicalize` | Canonicalizers + on-demand ingest orchestration. Queue enqueue behind a sink trait. |
| `sources` | `memory_sources` | Registry, contracts, validation, local readers. Network readers are trait seams. |
| `diff` | `memory_diff` | git2-backed snapshot/diff/checkpoint ledger. Chunk source injected via trait. |
| `entities` | `memory_entities` | Entity markdown registry, canonicalization, notes-preserving upsert. |
| `graph` | `memory_graph` | Co-occurrence edges over an injected `EntityOccurrenceIndex`. |
| `goals` | `memory_goals` | `MEMORY_GOALS.md` store, caps, symlink-escape safety. LLM reflection behind a trait. |
| `tool_memory` | `memory_tools` | Tool-scoped rules, priority prompt rendering over the `Memory` trait. |
| `conversations` | `memory_conversations` | JSONL transcript store, inverted index, persistence bus. |
| `archivist` | `memory_archivist` | Conversation turns → one tree leaf (tool-JSON stripped). Tree-leaf sink injected. |

Per the ownership boundary, the live sync runner, OAuth/webhook callbacks, and
real LLM/embedding/network backends remain host-owned (OpenHuman) and are
represented here as injectable traits. Known follow-ups: consolidate legacy
`score::store` entity-index helpers around `store::entity_index`; restore the
deferred peripheral surfaces (tree `health`/`nlp`, retrieval RPC/fast paths,
obsidian/wiki-git content, controller/tool registries) as host adapters land.
