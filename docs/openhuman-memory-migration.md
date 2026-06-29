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
