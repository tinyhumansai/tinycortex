# OpenHuman Memory Module Specs

This directory breaks the OpenHuman memory engine into module-level
specifications for the TinyCortex migration. Each document captures the
observed OpenHuman contract, the required data attributes, invariants, and the
recommended TinyCortex landing area.

Source checkout used for this pass:
`/Users/enamakel/work/tinyhumansai/openhuman-workflow/openhuman/src/openhuman`.

## Documents

- [Core Orchestration](core-orchestration.md): `memory`, high-level trait,
  ingest, remember, query, and RPC envelope responsibilities.
- [Storage Primitives](storage-primitives.md): `memory_store`, content,
  chunks, trees, vectors, KV, unified legacy store, and safety.
- [Sources, Registry, and Sync](sources-registry-sync.md): `memory_sources`,
  `memory_sync`, canonicalizers, readers, source CRUD, and sync status.
- [Tree, Queue, and Retrieval](tree-queue-retrieval.md): `memory_tree`,
  `memory_queue`, scoring, embedding, tree IO, and retrieval tools.
- [Diff Layer](diff-layer.md): `memory_diff`, git ledger, snapshots,
  checkpoints, read markers, and diff RPC/tool contracts.
- [Entities and Graph](entities-graph.md): `memory_entities` and
  `memory_graph`.
- [Conversation and Archivist](conversation-archivist.md):
  `memory_conversations` and `memory_archivist`.
- [Agent, Tool, and Goals Memory](agent-tool-goals-memory.md):
  `agent_memory`, `memory_tools`, and `memory_goals`.
- [Controller and Tool Registry](controller-tool-registry.md):
  controller namespaces, RPC surfaces, and agent tool names.

## Migration Rule

For TinyCortex, port pure contracts first: enums, request/response structs,
validation, deterministic ids, parsers, and renderers. Storage, queues, and
worker side effects should come after the type-level contracts are compiling
and covered by focused tests.

