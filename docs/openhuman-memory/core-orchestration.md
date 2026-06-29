# Core Orchestration Spec

OpenHuman module: `memory`.

## Responsibility

`memory` is the orchestration layer over the memory stack. It routes sync,
remember, ingest, query, read RPC, schema registration, and agent-facing tools.
It must not own raw persistence primitives; all durable storage goes through
`memory_store` or specialized sibling modules.

## Public Contract

The high-level `Memory` trait represents a namespace-scoped storage backend:

- `name()`: backend label.
- `store(namespace, key, content, category, session_id)`.
- `store_with_taint(..., taint)`: required for external sync provenance.
- `recall(query, limit, RecallOpts)`.
- deletion/listing/summaries through backend-specific extensions.

`RecallOpts` includes namespace, category, session id, minimum score, and a
`cross_session` flag. Cross-session recall is scoped by workspace: one workspace
is one user boundary.

## Core Types

`MemoryEntry` must carry:

- `id`, `key`, `content`, optional `namespace`.
- `category`: `core`, `daily`, `conversation`, or custom.
- `timestamp`, optional `session_id`, optional `score`.
- `taint`: `internal` or `external_sync`.

`MemoryTaint` is security-sensitive. Unknown persisted values must decode as
external. Any sync path ingesting third-party text must write
`external_sync`; user-authored or internal memory can remain `internal`.

Namespace document inputs include `namespace`, `key`, `title`, `content`,
`source_type`, `priority`, `tags`, `metadata`, `category`, optional
`session_id`, optional `document_id`, and `taint`.

## Ingest Orchestration

The ingest pipeline accepts canonical source documents and must produce the same
downstream shape no matter which upstream source produced them:

```text
canonical source
  -> raw markdown
  -> chunks
  -> scoring/extraction/embedding
  -> chunk rows and indexes
  -> memory queue jobs
  -> tree buffers and summaries
```

The orchestration layer must call storage through stable interfaces. It should
not open SQLite connections directly except through lower-level helpers.

## Remember Orchestration

`remember.rs` classifies memory input sources such as chat history, uploaded
data, and LLM-thought memory. Classification should decide route and category
but not bypass the ingest/storage contracts.

## Query Orchestration

`memory/query` exposes high-level tools that call lower layers:

- `memory_tree` / `MemoryTreeTool`: agentic tree walk.
- `query_source`: query a source tree.
- `cover_window`: cover a time window.
- `drill_down`: descend summary children.
- `fetch_leaves`: hydrate raw chunks.
- `search_entities`: entity lookup.
- `ingest_document`: orchestrator-facing document ingest.

TinyCortex should keep the same boundary: query tools compose retrieval
primitives but do not own tree persistence.

## Read RPC

`memory/read_rpc` provides read-oriented handlers for admin, chunks, entities,
graph, and vault/content. These should remain side-effect-light, with explicit
pagination and concrete ids in responses.

## Schema and Controller Layer

`memory/schema` and `memory/schemas` register operations for documents, files,
KV/graph, learning, sync, and tool memory. Controller schemas are part of the
public contract: generated docs and client tooling depend on stable namespaces,
function names, inputs, and outputs.

## Required Invariants

- Orchestration depends downward on storage, tree, sync, and source modules.
- Storage must not depend on `memory` orchestration, except documented legacy
  facades during migration.
- Machine-readable fields must survive through RPC and tool layers.
- Taint must be preserved from ingest through retrieval hits.
- Query/recall responses should expose enough score/provenance data for callers
  to explain why context was selected.

## TinyCortex Landing Area

```text
src/memory/
  traits.rs
  types.rs
  ingest/
  query/
  read_rpc/
  schemas/
  tools/
```

Port order: taint and memory entry types, namespace document/retrieval types,
ingest request contracts, query request/response types, then schema registry.

