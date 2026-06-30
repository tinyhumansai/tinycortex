# Storage Primitives Spec

OpenHuman module: `memory_store`.

## Responsibility

`memory_store` is the only home for persisted memory shapes. It owns local
markdown bodies, SQLite indexes, summary tree rows, vectors, key-value records,
and entity occurrences. OpenHuman also has legacy unified-memory surfaces, but
TinyCortex should treat those as migration context unless an explicit
compatibility facade is added. Higher layers must call into storage primitives
instead of managing persistence directly.

## Stored Kinds

The authoritative memory kind catalog is:

- `raw`: immutable markdown body file.
- `chunk`: metadata row and lifecycle state pointing to raw content.
- `entity`: canonical entity occurrence per tree node.
- `tree`: summary tree and summary node rows.
- `vector`: dense embedding row.
- `kv`: global or namespace-scoped JSON value.
- `contact`: raw archive item kind used for imported contact records, not a
  standalone address-book facade in the current TinyCortex port.

Adding a kind requires: enum variant, wire string, vector compatibility,
Obsidian/markdown representation, retrieval delegation, and tests.

## Content Store

`content/` is the body source of truth. Required behavior:

- Write chunk bodies atomically and never overwrite an existing chunk body.
- Store content under deterministic, inspectable paths.
- Compose and parse YAML front matter.
- Allow tag/front-matter rewrites without rewriting immutable chunk body bytes.
- Provide raw reads by path and Obsidian vault defaults.
- Guard content path generation so logical ids cannot escape the root.
- Stage summary markdown files atomically; summaries are immutable after seal at
  the tree row contract, while restaging may replace a stale on-disk summary
  file to keep `content_sha256` and the body in sync.

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
references, semantic chunking, lifecycle updates, chunk listing/filtering, and
per-model embedding sidecars for chunk and summary vectors.

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

The generic vector store persists packed `f32` embeddings and performs cosine
search. On first open it records the embedding provider and dimension in
`store_meta`, then rejects later opens with an incompatible dimension.

Chunk-tree embeddings use SQLite sidecar tables keyed by row id plus
`model_signature` (`model@dim`). Each sidecar row stores the packed vector,
dimension, and creation timestamp, and re-embed tombstone tables record rows
that cannot be backfilled for a signature. OpenHuman used 768-dimensional
embeddings historically; TinyCortex must treat dimension as signature metadata,
not a fixed global constant.

## KV Store

KV records can be global or namespace-scoped. A record includes optional
namespace, key, JSON value, and updated timestamp. KV is useful for compact
state that is not a full document body.

## Unified Legacy Store

OpenHuman's `unified/` surface still backs parts of its generic `Memory` trait.
Active areas there include documents, query, segments, events, profile, and
graph relations, but the current TinyCortex port does not include a `unified/`
store module. TinyCortex should keep new persistence in explicit stores and
only add a unified facade if compatibility with that OpenHuman trait surface is
required.

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
  content/
  entity_index/
  kv.rs
  safety.rs
  vectors/

src/memory/chunks/
src/memory/tree/
src/memory/retrieval/
```

Port order: pure types, deterministic ids, front-matter compose/parse,
in-memory tests, then SQLite/file backends.
