# Core Orchestration Spec

OpenHuman module: `memory`.

## Responsibility

In OpenHuman, `memory` is the orchestration layer over the memory stack. It
routes sync, remember, ingest, query, read RPC, schema registration, and
agent-facing tools. In TinyCortex, `src/memory/` currently provides the shared
contracts and the lower-level Rust modules those adapters call; host-facing
`query/`, `read_rpc/`, `schemas/`, and `tools/` adapter directories are not
ported yet. The layering rule still applies: orchestration must not own raw
persistence primitives; all durable storage goes through storage modules or
specialized siblings.

## Ownership Boundary

TinyCortex does not own memory sync. OpenHuman owns the sync module and the
decision to ingest data on demand. TinyCortex should expose the contracts and
processing path OpenHuman calls after it has selected or fetched source data.
That means TinyCortex can define ingest request shapes, taint rules, storage
effects, and retrieval outputs, but it must not become the owner of polling,
OAuth/webhook callbacks, or "when should this source be sunk?" policy.

## Public Contract

The high-level `Memory` trait represents a namespace-scoped storage backend:

- `name()`: backend label.
- `store(namespace, key, content, category, session_id)`.
- `store_with_taint(..., taint)`: required for external sync provenance.
- `recall(query, limit, RecallOpts)`.
- `recall_relevant_by_vector(...)`: optional vector-only recall; defaults empty.
- exact `get`, `list`, `forget`, namespace summaries, count, and health check.

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

The ingest pipeline accepts OpenHuman-supplied canonical source documents and
must produce the same downstream shape no matter which upstream source produced
them:

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
not open SQLite connections directly except through lower-level helpers. The
OpenHuman-owned sync module may trigger this path, but sync itself is outside the
TinyCortex module boundary.

Current TinyCortex ingest lives under `src/memory/ingest/` and composes
canonicalization, extraction, chunking, score/tree writes, and queue arming.
Controller/RPC wrappers around ingest are deferred host adapters.

## Remember Orchestration

OpenHuman `remember.rs` classifies memory input sources such as chat history,
uploaded data, and LLM-thought memory. TinyCortex has not ported a standalone
remember adapter; host callers should classify route/category before invoking
the ingest/storage contracts, and classification must not bypass those
contracts.

## Query Orchestration

OpenHuman `memory/query` exposes high-level tools that call lower layers:

- `memory_tree` / `MemoryTreeTool`: agentic tree walk.
- `query_source`: query a source tree.
- `query_global`: reconstruct cross-source digest from source-tree summaries.
- `query_topic`: reconstruct entity/topic retrieval from the entity index.
- `cover_window`: cover a time window.
- `drill_down`: descend summary children.
- `fetch_leaves`: hydrate raw chunks.
- `search_entities`: entity lookup.
- `ingest_document`: orchestrator-facing document ingest.

TinyCortex should keep the same boundary: query tools compose retrieval
primitives but do not own tree persistence. Current TinyCortex exposes these as
Rust retrieval APIs under `src/memory/retrieval/`; the OpenHuman tool wrappers
are not ported.

## Read RPC

OpenHuman `memory/read_rpc` provides read-oriented handlers for admin, chunks,
entities, graph, and vault/content. TinyCortex has the underlying read APIs in
chunks, entities, graph, retrieval, and content storage, but the read-RPC
handler layer itself is not ported. When added, it should remain
side-effect-light, with explicit pagination and concrete ids in responses.

## Schema and Controller Layer

OpenHuman `memory/schema` and `memory/schemas` register operations for
documents, files, KV/graph, learning, sync, and tool memory. TinyCortex does
not yet port schema registration. Controller schemas remain a future public
contract: generated docs and client tooling depend on stable namespaces,
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
  retrieval/
  controllers/   # deferred host adapter layer
  read_rpc/      # deferred host adapter layer
  schemas/       # deferred host adapter layer
  tools/         # deferred agent-tool adapter layer
```

Port order: taint and memory entry types, namespace document/retrieval types,
OpenHuman-facing ingest request contracts, query request/response types, then
schema registry.
