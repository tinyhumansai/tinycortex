# OpenHuman Memory Migration

This repository now has a Rust crate rooted at the repository root. The first
migration target is the memory core: stable contracts, storage primitives, and
testable in-process behavior before API or UI integrations.

TinyCortex will not own memory sync. OpenHuman owns the sync module and
decides when data is ingested on demand. TinyCortex should assume OpenHuman
supplies source-scoped payloads or canonical ingest requests, then process them
through TinyCortex contracts.

## Source Modules

Use `/Users/enamakel/work/tinyhumansai/openhuman-workflow/openhuman/src/openhuman`
as the current source checkout unless a later branch is chosen.

- `memory/`: orchestration for query, remember, OpenHuman-triggered ingest, and
  RPC surfaces.
- `memory_store/`: storage primitives for content, chunks, trees, vectors, KV,
  entities, and the shrinking unified store.
- `memory_tree/`: tree mechanics, summary sealing, retrieval, scoring, and
  entity extraction.
- `memory_queue/`: SQLite-backed async jobs for extraction, append, seal, topic
  routing, digests, and stale flushes.
- `memory_search/`, `memory_graph/`, `memory_entities/`, `memory_sources/`:
  specialized retrieval, graph, source contracts, and validation layers.

## Target Layout

- `src/memory/types.rs`: stable public memory data contracts.
- `src/memory/store.rs`: backend-agnostic store trait plus
  simple implementations.
- Future modules should keep OpenHuman's layer rule: orchestration depends on
  storage, but storage does not depend upward on orchestration. The OpenHuman
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
tests in sibling `*_tests.rs` files (1000+ tests; `cargo fmt` clean, no warnings).

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
represented here as injectable traits. Known follow-ups: consolidate the
`mem_tree_entity_index` access that currently appears in `store`, `score`, and
`tree`; restore the deferred peripheral surfaces (tree `health`/`nlp`,
retrieval `fast`/rpc, obsidian/wiki-git content) as host adapters land.
