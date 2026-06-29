# Storage Primitives Spec

OpenHuman module: `memory_store`.

## Responsibility

`memory_store` is the only home for persisted memory shapes. It owns local
markdown bodies, SQLite indexes, summary tree rows, vectors, key-value records,
entity occurrences, and legacy unified memory surfaces. Higher layers must call
into it instead of managing persistence directly.

## Stored Kinds

The authoritative memory kind catalog is:

- `raw`: immutable markdown body file.
- `chunk`: metadata row and lifecycle state pointing to raw content.
- `entity`: canonical entity occurrence per tree node.
- `tree`: summary tree and summary node rows.
- `vector`: dense embedding row.
- `kv`: global or namespace-scoped JSON value.
- `contact`: address-book/person facade retained for compatibility.

Adding a kind requires: enum variant, wire string, vector compatibility,
Obsidian/markdown representation, retrieval delegation, and tests.

## Content Store

`content/` is the body source of truth. Required behavior:

- Write bodies atomically.
- Store content under deterministic, inspectable paths.
- Compose and parse YAML front matter.
- Allow tag/front-matter rewrites without rewriting immutable body bytes.
- Provide raw reads by path and Obsidian vault defaults.
- Guard content path generation so logical ids cannot escape the root.

SQLite should store `content_path` and `content_sha256`, not duplicate body
ownership. Keyword search over bodies should be served from markdown files or a
rebuildable index.

## Chunk Store

Chunks represent atomic ingested units. Required attributes:

- deterministic `id`.
- canonical markdown `content`.
- `Metadata`: source kind, source id, owner, point timestamp, time range, tags,
  optional source ref, optional path scope.
- `token_count`, `seq_in_source`, `created_at`, `partial_message`.

Source kinds are `chat`, `email`, and `document`. Data sources include Discord,
Telegram, WhatsApp, conversation, Gmail, other email, Notion, meeting notes, and
Drive docs. Provider expansion must be non-breaking.

The chunk store also owns migrations, connection handling, raw source
references, semantic chunking, embedding columns, lifecycle updates, and chunk
listing/filtering.

## Tree Store

Tree storage owns:

- `Tree`: id, kind, scope, root id, max level, status, timestamps.
- `SummaryNode`: immutable sealed summary content, child ids, entities, topics,
  time range, score, embedding, optional document version identity.
- `Buffer`: unsealed frontier per tree and level.
- hotness/registry helpers.

Tree kinds are `source`, `topic`, and `global`; status is `active` or
`archived`.

Important constants from OpenHuman:

- input seal budget: 50,000 tokens.
- summary output budget: 5,000 tokens.
- summary fanout: 10.
- stale flush age: 7 days.

## Vector Store

The vector store persists packed `f32` embeddings and performs cosine search.
OpenHuman uses 768-dimensional embeddings for chunks and summaries. TinyCortex
should record enough embedding signature metadata to support future model or
dimension migrations; old rows should be backfillable through queue jobs.

## KV Store

KV records can be global or namespace-scoped. A record includes optional
namespace, key, JSON value, and updated timestamp. KV is useful for compact
state that is not a full document body.

## Unified Legacy Store

`unified/` still backs parts of the generic `Memory` trait. Active areas include
documents, query, segments, events, profile, and graph relations. It is marked
as staging for removal in OpenHuman. TinyCortex should decide whether to:

- preserve the unified facade for compatibility, or
- split documents, segments, events, and graph records into explicit stores.

## Retrieval Facade

`retrieval/` fans out to tree walk, vector, keyword, and param/tag retrieval.
It should remain a facade over lower stores, not a new persistence owner.

## Safety

Storage must include:

- PII/safety helpers.
- path traversal defense.
- consistent taint propagation.
- source id and content hash preservation.
- tests for serde defaults and wire string stability.

## TinyCortex Landing Area

```text
src/memory/store/
  content.rs
  chunks.rs
  trees.rs
  vectors.rs
  kv.rs
  unified.rs
  safety.rs
  retrieval.rs
```

Port order: pure types, deterministic ids, front-matter compose/parse,
in-memory tests, then SQLite/file backends.

