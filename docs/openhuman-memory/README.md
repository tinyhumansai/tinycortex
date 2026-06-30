# OpenHuman Memory Module Specs

This directory breaks the OpenHuman memory engine into module-level
specifications for the TinyCortex migration. Each document captures the
observed OpenHuman contract, the required data attributes, invariants, and the
recommended TinyCortex landing area.

Boundary: TinyCortex does not own memory sync. The OpenHuman application
owns the sync module and decides when data is ingested on demand. These specs
describe the contracts TinyCortex exposes after OpenHuman supplies source data.

Source checkout used for this pass:
`/Users/enamakel/work/tinyhumansai/openhuman-workflow/openhuman/src/openhuman`.

## Documents

- [Core Orchestration](core-orchestration.md): `memory`, high-level trait,
  ingest, remember, query, and RPC envelope responsibilities.
- [Storage Primitives](storage-primitives.md): `memory_store`, content,
  chunks, trees, vectors, KV, entity index, OpenHuman unified-legacy context,
  and safety.
- [Sources, Registry, and Sync](sources-registry-sync.md): `memory_sources`,
  `memory_sync`, canonicalizers, readers, source CRUD, sync status, and the
  OpenHuman-owned sync boundary.
- [Tree, Queue, and Retrieval](tree-queue-retrieval.md): `memory_tree`,
  `memory_queue`, scoring, embedding, tree IO, and retrieval tools.
- [Diff Layer](diff-layer.md): `memory_diff`, git ledger, snapshots,
  checkpoints, read markers, and deferred diff RPC/tool adapter contracts.
- [Entities and Graph](entities-graph.md): `memory_entities` and
  `memory_graph`.
- [Conversation and Archivist](conversation-archivist.md):
  `memory_conversations` and `memory_archivist`.
- [Agent, Tool, and Goals Memory](agent-tool-goals-memory.md):
  OpenHuman `agent_memory` context plus TinyCortex `memory_tools` and
  `memory_goals`.
- [Controller and Tool Registry](controller-tool-registry.md):
  deferred controller namespaces, RPC surfaces, and agent tool names.
- [OpenHuman Code Reference](openhuman-code-reference.md): source-derived
  module inventory, Rust contract sketches, invariants, and migration notes for
  each OpenHuman memory subsystem.

## Migration Rule

For TinyCortex, port pure contracts first: enums, request/response structs,
validation, deterministic ids, parsers, and renderers. Storage, queues, and
worker side effects should come after the type-level contracts are compiling
and covered by focused tests.
