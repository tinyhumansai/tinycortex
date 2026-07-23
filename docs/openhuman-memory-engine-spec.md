# OpenHuman Memory Engine Specification

This document captures the OpenHuman memory engine as the target functional
specification for TinyCortex. It is source-derived from
the vendoring host's `src/openhuman/` tree.

## Scope

The memory engine is not a single store. It is a layered system for ingesting
external and local content, canonicalizing it, chunking it, scoring it, writing
durable markdown and SQLite indexes, building summary trees, retrieving context,
tracking source diffs, and exposing tools/RPC surfaces to agents and UI clients.

## Ownership Boundary

TinyCortex does not own memory sync. The sync module is an OpenHuman module:
OpenHuman decides when data should be ingested, owns the upstream trigger path,
and invokes TinyCortex on demand with already selected source payloads or
canonical ingest requests. TinyCortex owns the memory engine contracts and
processing semantics after that boundary: validation, canonical input shapes,
storage/index contracts, chunking, tree updates, diffing, retrieval, and
explainable provenance.

In this specification, "ingest" means "process an OpenHuman-supplied memory
payload through TinyCortex contracts." It does not mean TinyCortex polls user
apps, owns OAuth/webhook callbacks, or decides when to sync data.

TinyCortex should preserve these core properties:

- Local-first storage: user workspace files and local indexes are authoritative.
- Durable provenance: every item carries source identity, timestamps, and taint.
- Inspectable content: markdown files are the source of truth for bodies.
- Derived indexes: SQLite, vectors, trees, and git ledgers accelerate reads but
  must be rebuildable from canonical content where possible.
- Layer boundaries: ingestion and orchestration depend on storage; storage must
  not depend upward on orchestration, tools, or agents.

## Layer Map

| Layer              | OpenHuman module                                                       | Responsibility                                                                         |
| ------------------ | ---------------------------------------------------------------------- | -------------------------------------------------------------------------------------- |
| Source registry    | `memory_sources`                                                       | Defines source contracts; OpenHuman owns active sync configuration and triggering.      |
| Sync pipelines     | `memory_sync`                                                          | Documents pull/on-demand pipeline contracts; OpenHuman owns the sync runner.            |
| Canonicalization   | `memory_sync/canonicalize`                                             | Converts source payloads to canonical markdown plus metadata.                          |
| Orchestration      | `memory`                                                               | Provides shared contracts plus ingest/retrieval Rust APIs; host RPC/tool adapters are deferred. |
| Storage primitives | `memory_store`                                                         | Owns content files, chunks, trees, vectors, KV, and entities; OpenHuman unified legacy surfaces are migration context. |
| Tree mechanics     | `memory_tree`                                                          | Appends leaves, seals buckets, summarizes, scores, embeds, and retrieves.              |
| Async jobs         | `memory_queue`                                                         | Runs extraction, append, seal, backfill, stale flush, and document sealing work.       |
| Retrieval          | `memory_search`, `memory_tree/retrieval`                               | Vector, keyword, graph, tree, entity, and hybrid search.                               |
| Change tracking    | `memory_diff`                                                          | Git-backed snapshots, diffs, checkpoints, and read markers per source.                 |
| Specialized memory | `memory_goals`, `memory_tools`, `agent_memory`, `memory_conversations` | Agent goals, tool rules, OpenHuman/host memory-agent retrieval, and conversation logs. |

## Core Domain Types

### Memory Entries

The high-level `Memory` trait supports storing, recalling, deleting, listing,
and summarizing namespace-scoped memories. A memory entry includes:

- `id`: stable entry identifier.
- `key`: title or caller-supplied logical key.
- `content`: stored text body.
- `namespace`: optional logical partition; absent maps to the global namespace.
- `category`: `core`, `daily`, `conversation`, or custom.
- `timestamp`: ISO 8601 update/create time.
- `session_id`: optional chat/session scope.
- `score`: optional relevance score.
- `taint`: provenance value, either `internal` or `external_sync`.

`MemoryTaint` is a security contract. External sync sources such as Gmail,
Slack, Notion, Composio, or MCP content must be stored as `external_sync` so
automation can refuse external-effect tools when tainted context is present.
Unknown persisted taint values must fail closed as external.

### Namespace Documents

Namespace document inputs include `namespace`, `key`, `title`, `content`,
`source_type`, `priority`, `tags`, `metadata`, `category`, optional
`session_id`, optional `document_id`, and `taint`. Stored documents additionally
track `created_at`, `updated_at`, and `markdown_rel_path`.

Retrieval hits expose:

- `id`, `kind`, `namespace`, `key`, `title`, `content`, `category`.
- `source_type`, `updated_at`, `document_id`, `chunk_id`.
- `score` and `score_breakdown`.
- `supporting_relations`.
- `taint`.

Score breakdown fields are `keyword_relevance`, `vector_similarity`,
`graph_relevance`, `episodic_relevance`, `freshness`, and `final_score`.

## Memory Store

`memory_store` is the persistence substrate. It owns:

- `content/`: immutable markdown files, YAML front matter, Obsidian vault layout,
  raw source bodies, summary bodies, tag rewrites, and atomic writes.
- `chunks/`: SQLite chunk rows with metadata, lifecycle, raw markdown pointers,
  chunking logic, and source back-pointers.
- `trees/`: summary tree rows, buffers, hotness, tree registry, and summary nodes.
- `vectors/`: local vector DB with packed `f32` embeddings, cosine search, and
  stored provider/dimension metadata.
- `kv.rs`: global and namespace-scoped JSON key-value records.
- `entities.rs`: entity occurrence index over tree nodes.
- OpenHuman `unified/`: legacy shrinking surface for documents, events,
  segments, profile, graph relations, and query behavior still used by the
  OpenHuman generic `Memory` trait; not a current TinyCortex store module.
- `retrieval/`: facade over tree walk, vector, keyword, and param/tag retrieval.
- `safety/`: PII and memory safety helpers.

Memory kinds are: `raw`, `chunk`, `entity`, `tree`, `vector`, `kv`, and
`contact`, where `contact` is currently a raw archive item kind rather than a
standalone address-book store. Adding a new kind requires a catalog entry,
vector compatibility, Obsidian/markdown compatibility, and retrieval delegation.

### Content Invariants

- Chunk markdown body bytes are immutable after write; sealed summary rows are
  immutable, while stale summary markdown files may be replaced during restage
  to keep body hashes aligned with SQLite.
- SQLite stores pointers such as `content_path` and `content_sha256`.
- Mutable metadata should be restricted to front matter fields such as tags.
- Body keyword search should use markdown files, not duplicate body indexes.
- Obsidian-readable files are a first-class product surface, not an export.

## Chunk Model

Chunks are the atomic ingested units. They include:

- `id`: deterministic hash of source kind, source id, sequence, and content.
- `content`: canonical markdown.
- `metadata`: source kind, source id, owner, timestamp, time range, tags,
  optional source reference, and optional path scope.
- `token_count`: approximate token count.
- `seq_in_source`: stable sequence number within the logical source.
- `created_at`: persistence timestamp.
- `partial_message`: true when a logical item was split.

Chunk source kinds are `chat`, `email`, and `document`. Data sources include
Discord, Telegram, WhatsApp, conversation, Gmail, other email, Notion, meeting
notes, and Drive docs. TinyCortex should make provider expansion non-breaking.

## Memory Sources

TinyCortex should model memory source records because source identity,
provenance, validation, and diffing depend on them. It should not own the live
memory sync process that polls or subscribes to those sources. OpenHuman owns
runtime sync and calls TinyCortex with source-scoped content when ingestion is
requested.

`memory_sources` defines configured source entries persisted in `config.toml`
under `[[memory_sources]]`. Source kinds are:

- `composio`: requires `toolkit` and `connection_id`.
- `conversation`: no kind-specific required fields.
- `folder`: requires `path`; optional `glob`.
- `github_repo`: requires `url`; optional `branch`, `paths`, and max counts.
- `twitter_query`: requires `query`; currently sync placeholder.
- `rss_feed`: requires `url`.
- `web_page`: requires `url`; optional CSS `selector`.

All sources have `id`, `kind`, `label`, `enabled`, optional sync budgets
(`max_tokens_per_sync`, `max_cost_per_sync_usd`), and optional
`sync_depth_days`.

Reader outputs are:

- `SourceItem`: `id`, `title`, optional `updated_at_ms`.
- `SourceContent`: `id`, `title`, `body`, `content_type`, and `metadata`.
- `ContentType`: `markdown`, `html`, or `plaintext`.

Manual source sync in OpenHuman emits lifecycle events: requested, fetching,
stored, ingesting, completed, and failed. For TinyCortex, these are integration
events at the boundary, not an owned scheduler requirement.

## Sync Pipelines

The OpenHuman code defines sync pipeline traits and status semantics. TinyCortex
should preserve the trait-compatible contract for integration and tests, but
the production sync runner remains OpenHuman-owned.

`memory_sync` owns upstream pull loops. Pipeline kinds are:

- `composio`: Gmail, Slack, GitHub, Notion, Linear, ClickUp, and similar OAuth
  connectors via Composio.
- `workspace`: local vault files, harness turns, dictation transcripts, folders,
  GitHub repo readers, RSS, and web page sources.
- `mcp`: third-party MCP server pipelines.

Each pipeline implements:

- `id() -> &str`
- `kind() -> SyncPipelineKind`
- `init(config) -> Result<()>`
- `tick(config) -> Result<SyncOutcome>`

`SyncOutcome` includes `records_ingested`, `more_pending`, and optional `note`.
In OpenHuman, pipelines own cursoring and retry policy while orchestration owns
cadence. In TinyCortex, these details are compatibility contracts unless/until
OpenHuman calls into them directly.

## Canonicalization

Canonicalizers normalize payload shape, not meaning. They produce
`CanonicalisedSource { markdown, metadata }`.

- Chat: sorted message blocks like `## <timestamp> - <author>\n<body>`.
- Email: per-message blocks with `From`, `Subject`, `Date`, and cleaned body.
- Document: trimmed markdown body with a modified-time point range.

Canonical markdown must not include a leading title header; titles and source
metadata belong in content-store YAML front matter. Scoring, extraction, and
summarization happen downstream.

## Ingest Pipeline

OpenHuman owns memory sync and decides when this path runs. TinyCortex
assumes ingestion is invoked on demand with a source-scoped payload.

The ingest path is:

```text
source reader or sync provider
  -> canonicalize
  -> write raw markdown
  -> chunk
  -> score/extract/embed
  -> persist chunk metadata
  -> enqueue tree jobs
  -> append to buffers
  -> seal summaries
  -> update retrieval indexes
```

Reader-backed source sync and Composio sync are OpenHuman sync concerns. When
they call into TinyCortex, every provider must still land data through the same
raw markdown, chunk, score, and tree path.

## Scoring, Extraction, and Embedding

Scoring decides whether a chunk enters the tree and records an auditable
rationale in `mem_tree_score`.

Signals include:

- token count shape.
- unique-word diversity.
- metadata weight.
- source/provider weight.
- interaction tags such as sent, reply, direct message, and mention.
- entity density.
- optional LLM-derived importance for borderline cases.

Entity extraction is pluggable:

- regex extractor: emails, URLs, handles, hashtags.
- LLM extractor: semantic NER plus importance rating.
- composite extractor: merges outputs and tolerates per-extractor failures.

Entities include mechanical kinds and semantic kinds such as person,
organization, location, and topic. Canonicalization lowercases emails, strips
handle/topic prefixes, and assigns stable canonical ids.

Embeddings use a fixed dimension of 768 in OpenHuman. The default backend is
Ollama `nomic-embed-text`; strict mode can fail if embeddings are unavailable.
Tests use inert zero embeddings.

## Summary Trees

Tree storage keeps the OpenHuman tree-kind wire catalog:

- `source`: one tree per ingest source.
- `topic`: legacy/optional per-entity topic tree scope.
- `global`: legacy/optional cross-source digest tree scope.

Current TinyCortex behavior treats `source` trees as the primary persisted
summary hierarchy. Standalone `topic` and `global` trees are compatibility
surfaces rather than required active stores: `query_topic` reconstructs
entity/topic retrieval from the entity index, and `query_global` reconstructs a
time-window digest by scanning source-tree summaries. Older queue jobs from the
retired standalone global/topic pipeline, such as `topic_route` and
`digest_daily`, must be skipped or purged without crashing migrations.

A tree includes `id`, `kind`, `scope`, optional `root_id`, `max_level`,
`status`, `created_at`, and optional `last_sealed_at`. Status is `active` or
`archived`.

Summary nodes include:

- `id`, `tree_id`, `tree_kind`, `level`, optional `parent_id`.
- `child_ids`, immutable after seal.
- summary `content`, `token_count`, `entities`, `topics`.
- `time_range_start`, `time_range_end`.
- `score`, `sealed_at`, `deleted`.
- optional `embedding`.
- optional document version fields `doc_id` and `version_ms`.

Buffers hold unsealed frontier items per `(tree_id, level)` with `item_ids`,
`token_sum`, and `oldest_at`.

OpenHuman constants:

- `INPUT_TOKEN_BUDGET`: 50,000 tokens.
- `OUTPUT_TOKEN_BUDGET`: 5,000 tokens.
- `SUMMARY_FANOUT`: 10 summary siblings.
- `DEFAULT_FLUSH_AGE_SECS`: 7 days.

Write requests append a `TreeLeafPayload` to a tree with a label strategy:
`inherit`, `extract`, or `empty`. Writes can be immediate or deferred. Deferred
writes return `seal_pending` so the queue can schedule sealing.

Read requests specify `tree_id`, optional start node, max depth, optional query,
and optional limit. Results return compact hits with node id, node kind, level,
content, and optional semantic score.

## Memory Queue

`memory_queue` decouples expensive work from ingest. Jobs persist in
`mem_tree_jobs` with a string `kind`, JSON payload, status, attempts, lock
metadata, and dedupe key. Job statuses are `ready`, `running`, `done`, `failed`,
and `cancelled`.

Job kinds include:

- `extract_chunk`: run extraction/scoring over a chunk.
- `append_buffer`: append a leaf or summary node into a source/topic buffer.
- `seal`: seal one tree buffer level.
- `flush_stale`: enqueue seals for stale buffers.
- `reembed_backfill`: re-embed rows after embedding model or signature changes.
- `seal_document`: build a document version subtree and merge it.

Retired kinds such as `topic_route` and `digest_daily` may appear in older
queues and should be handled without crashing migrations.

LLM-bound jobs must be concurrency-limited. Handlers can return `Done` or
`Defer { until_ms, reason }`; deferred jobs should not burn retry budget.
Dedupe keys must suppress in-flight duplicates without preventing future runs
after a job reaches a terminal state.

## Retrieval

The retrieval layer exposes deterministic primitives and leaves composition to
the caller or memory agent:

- `query_source`: source-tree retrieval with optional semantic reranking.
- `query_global`: cross-source digest over a time window, reconstructed from
  source-tree summaries.
- `query_topic`: entity/topic-scoped retrieval, reconstructed from the entity
  occurrence index plus hydrated source-tree nodes.
- `search_entities`: fuzzy lookup over the entity index.
- `drill_down`: descend summary children by BFS and optional semantic rerank.
- `fetch_leaves`: hydrate raw chunks by id, capped per request.

Hybrid search uses weight profiles:

- `balanced`: graph 0.35, vector 0.35, keyword 0.15, freshness 0.15.
- `semantic`: graph 0.15, vector 0.65, keyword 0.20.
- `lexical`: graph 0.25, vector 0.15, keyword 0.60.
- `graph_first`: graph 0.55, vector 0.30, keyword 0.15.

Retrieval results should expose enough score breakdown to explain ranking.

## Diff Layer

`memory_diff` tracks source changes after sync without re-calling upstream APIs.
It builds snapshots from already-ingested chunk rows and stores a derived git
ledger at `<workspace>/memory_diff/repo`.

Mapping:

- Snapshot -> git commit.
- Checkpoint -> annotated tag `ckpt_<uuid>`.
- Read marker -> ref `refs/openhuman/read/<encoded_source_id>`.
- Diff -> git tree diff scoped to a source path.

Source ids and item ids are encoded before becoming git path/ref components;
the original logical ids remain in snapshot metadata and tool/RPC payloads.

Snapshot fields:

- `id`: commit SHA.
- `source_id`, `source_kind`, `label`.
- `trigger`: `auto` or `manual`.
- `item_count`, `taken_at_ms`.

Change kinds are `added`, `removed`, and `modified`. A changed item includes
`item_id`, `title`, kind, old/new content hashes, and optional bounded text
diff. Diff summaries count added, removed, modified, and unchanged items.

Required operations:

- take snapshot for one source.
- list snapshots with optional source filter.
- diff explicit snapshot pair.
- diff latest against previous.
- diff latest against read marker and optionally advance marker.
- mark one or more sources read.
- create/list checkpoints.
- diff all sources since a checkpoint.
- cleanup old checkpoint tags while retaining snapshot commits as ledger
  history.

The chunk store remains authoritative; the git ledger is rebuildable derived
state used for change awareness.

## Memory Sources and Diff Interaction

After successful source sync, `memory_diff::auto_snapshot_after_sync` captures a
source snapshot. For source-backed diffs, items are grouped from `mem_tree_chunks`
by source id prefix and ordered by source id and sequence. This means item ids
and source-id composition are part of the diff contract.

## Entity Registry and Graph

`memory_entities` stores named things as markdown files under:

```text
<content_root>/entities/<kind>/<canonical_id>.md
```

Entity files include YAML fields such as `id`, `kind`, `display_name`,
`aliases`, `emails`, `handles`, `created_at`, and `updated_at`, plus a
human-editable notes body. Upserts must preserve notes.

`memory_graph` derives relationships from `mem_tree_entity_index` instead of
owning a parallel triple store. Two entities co-occurring on the same node form
an edge; weight is the count of distinct shared nodes. Required graph queries:
co-occurring entities, neighbors, and grouping by weight.

## Conversation Storage and Archivist

`memory_conversations` stores thread metadata and message JSONL under:

```text
<workspace>/memory/conversations/
```

Thread metadata is appended to `threads.jsonl`; messages live in
`threads/<hex(thread_id)>.jsonl` so arbitrary provider/thread ids never become
raw path components. A process-wide mutex serializes disk mutation.
This storage is transcript persistence, not semantic memory indexing.

`memory_archivist` converts conversation turns into tree leaves:

```text
raw turns -> remove tool-call JSON and tool turns -> compose markdown
  -> append as one tree leaf -> downstream summary tree
```

Tool-call JSON and tool-result turns are intentionally dropped before archival
to prevent noisy provider-specific data from distorting embeddings.

## Agent Memory

OpenHuman `agent_memory` owns the specialist retrieval agent. It combines:

- vector search.
- keyword search over raw content files.
- entity search.
- tree browsing and drill-down.
- direct content reads.
- source listing.

Its content layout expectation is:

```text
memory_tree/content/
  chat/
  episodic/
  raw/
  wiki/summaries/
```

TinyCortex does not currently include a local `agent_memory` module. Hosts can
layer this agent over TinyCortex retrieval APIs; the agent should remain a
consumer of retrieval tools, not the owner of storage mechanics.

## Tool Memory

`memory_tools` stores durable tool-scoped rules in namespaces named
`tool-{tool_name}`. Rules include `id`, `tool_name`, rule text, priority,
source, tags, and timestamps.

Priorities are normal, high, and critical. Sources include user-explicit,
post-turn, and programmatic. Critical/high rules are rendered into prompt
sections so they survive compression. The current TinyCortex module provides
the durable rule store and renderer; concrete agent tools can list and upsert
rules when the host adapter layer is added.

## Goals Memory

`memory_goals` owns a compact long-term goal list stored in:

```text
<workspace>/MEMORY_GOALS.md
```

The markdown shape is:

```markdown
# Long-term Goals

- [g1] concise durable goal
```

Constraints:

- maximum 8 items.
- maximum rendered size 2,000 chars.
- each goal is single-line.
- likely secrets or PII are rejected.
- mutations serialize through a lock.
- path validation must reject symlink escapes outside the workspace.

OpenHuman goals can be mutated by RPC/tools or by a turn-based `goals_agent`
that uses goals tools plus memory recall. TinyCortex exposes direct store
wrappers plus a deterministic reflection driver behind `GoalsGenerator`.
Reflection should make minimal changes unless the list is empty, in which case
it populates an initial set.

## RPC and Tool Surfaces

OpenHuman exposes controller schemas and handlers for:

- memory initialization, query, recall, store, forget, doctor, and read RPC.
- memory tree retrieval and ingest operations.
- memory source list/get/add/update/remove/list_items/read_item/sync/status.
- memory diff snapshot, diff, checkpoint, read-marker, and cleanup operations.
- goals list/add/edit/delete/reflect operations.
- tool-memory list and put operations.

Agent-facing tools must use the same underlying contracts as RPC handlers where
possible. Tool outputs should preserve machine-readable ids, counts, scores,
source ids, and taint values.

TinyCortex currently provides the domain operations and wire-stable types those
handlers need. The concrete controller/schema/agent-tool registry layer is a
deferred host adapter surface.

## Security and Safety Requirements

- External sync content must carry `external_sync` taint.
- Unknown taint values must decode as external.
- Source readers must defend against path traversal.
- Folder readers cap file sizes at 10 MB.
- Goals storage must reject symlink escapes.
- Sync budgets must be enforceable per source.
- Live upstream fetchers must not leak provider-specific models into storage
  contracts.
- Tool-call archives should strip model-specific tool JSON before tree ingest.

## TinyCortex Target Modules

Current module layout:

```text
src/
  memory/
    config.rs
    error.rs
    types.rs
    traits.rs
    archivist/
    chunks/
    conversations/
    diff/
    entities/
    goals/
    graph/
    ingest/
    queue/
    retrieval/
    score/
    sources/
    store/
      content/
      entity_index/
      kv.rs
      safety.rs
      vectors/
    tool_memory/
    tree/
```

The first implementation milestone should port pure types and deterministic
logic before storage side effects:

1. Core memory traits, taint, namespace docs, retrieval hit types.
2. Source registry types and validation.
3. Chunk metadata and deterministic ids.
4. Tree, summary node, buffer, and tree IO contracts.
5. Queue job discriminators and payloads.
6. Diff snapshot/change/checkpoint types.
7. Goals document parse/render/cap logic.
8. Tool memory rule types.
9. Entity file types and graph edge types.
10. Storage backends and queue workers after contracts compile.

## Open Questions

- Whether TinyCortex should keep OpenHuman's legacy `unified` store surface or
  immediately model documents, chunks, graph relations, and episodes as
  separate stores.
- Whether standalone topic/global trees should ever be reintroduced as optional
  extensions; the current implementation reconstructs those projections from
  source trees and the entity index.
- Whether the git-backed diff ledger should be mandatory or pluggable behind a
  trait for non-git embedded deployments.
- Whether all embedding callers should converge on a single signature type; the
  current implementation already treats dimension as stored metadata/signature,
  not as a fixed global constant.
- How much of agent-specific prompt behavior belongs in TinyCortex versus an
  adapter crate.
