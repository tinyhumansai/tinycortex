# Storage Primitives

The storage layer is the bottom of the TinyCortex stack. It splits cleanly into
two kinds of state:

- **Authoritative content** — immutable markdown `.md` files under a content
  root. These are the source of truth; every higher layer points back to them by
  relative path + SHA-256.
- **Rebuildable derived indexes** — SQLite tables (chunks, summary-tree rows, the
  local vector DB, the KV store, the entity-occurrence index). Each one can be
  dropped and recomputed from the markdown without data loss.

This page covers the primitives that live under `src/memory/store/`:

| Primitive | Module | Backing |
|---|---|---|
| Markdown content store | `store/content/` | `.md` files on disk |
| Local vector DB | `store/vectors/` | SQLite `vectors` table (packed `f32` BLOBs) |
| KV store + safety guard | `store/kv.rs`, `store/safety.rs` | SQLite `kv_global` / `kv_namespace` |
| Entity-occurrence index | `store/entity_index/` | SQLite `mem_tree_entity_index` |

The public re-exports live in `src/memory/store/mod.rs`.

## Content store (source of truth)

Module: `src/memory/store/content/`. Chunk and summary bodies are written to disk
as `.md` files with a YAML front-matter header; SQLite (in the chunk store, ported
separately) holds only the *pointer* (`content_path`) and an *integrity token*
(`content_sha256`).

### File format

Each file is `---\n<front matter>\n---\n<body>` (see `compose/yaml.rs`):

```text
---
source_kind: chat
source_id: slack:#eng
seq: 0
owner: alice
timestamp: 2026-04-28T10:00:00Z
time_range_start: 2026-04-28T10:00:00Z
time_range_end: 2026-04-28T10:05:00Z
tags:
  - source/slack-eng
  - person/Alice-Smith
---
## 2026-04-28T10:00:00Z — alice
Message body here.
```

Front-matter is built by `compose/chunk.rs::compose_chunk_file`, which returns
`(full_file_bytes, body_bytes)`. Key invariants:

- **The SHA-256 is computed over the body bytes only** — everything after the
  closing `---\n`. Tags can therefore be rewritten (`rewrite_tags`) without
  invalidating the content hash.
- A `source/<slug>` tag is always seeded first (`yaml::with_source_tag`) so the
  Obsidian graph view can filter by source regardless of the ingest tag list.
- Email chunks additionally emit `participants:` and `aliases:` parsed from a
  `gmail:{addr1|addr2|...}` `source_id`.
- `compose/yaml.rs::yaml_scalar` quotes values that contain YAML-special
  characters; `split_front_matter` splits a file back into `(front_matter, body)`
  at the second `---` delimiter.

Two format constants are stamped for vault compatibility (`compose/mod.rs`):
`MEMORY_ARTIFACT_FORMAT = 2` and `OPENHUMAN_CORE_VERSION` (the crate version; the
front-matter key keeps the OpenHuman wire name `openhuman_core_version`).

### Paths and Obsidian-vault layout

`content/paths.rs` generates **relative, forward-slash** paths so they stay valid
no matter where the vault is mounted. Chunk layout by source kind:

```text
Email:    <content_root>/email/<participants_slug>/<chunk_id>.md
Chat:     <content_root>/chat/<source_slug>/<chunk_id>.md
Document: <content_root>/document/<source_slug>/<chunk_id>.md
```

Summaries live under a `wiki/` prefix (`WIKI_PREFIX = "wiki"`), keyed by
`SummaryTreeKind` (`Source` / `Global` / `Topic`):

```text
wiki/summaries/source-<scope_slug>/L<level>/<id>.md
wiki/summaries/global/L<level>/<id>.md
wiki/summaries/topic-<scope_slug>/L<level>/<id>.md
```

`SummaryDiskLayout` additionally routes document source-trees into nested
`docs/<doc_slug>/v-<version_ms>/` and `merge/` folders.

`slugify_source_id` is the canonical slugifier (lowercase, `[a-z0-9_-]` only,
collapse repeated `-`, trim separators, preserve interior `_`, truncate to 120
code points, empty → `"unknown"`). The same rules back the `source/<slug>` tag, so
the tag matches the on-disk directory name byte-for-byte. `sanitize_filename`
replaces Windows-illegal characters (`\ / : * ? " < > |`) with `-`.

> The Obsidian-vault registry (`content::obsidian*`) and the git-backed wiki
> mirror (`content::wiki_git`) pull host config and git surfaces beyond the
> storage primitive and are intentionally **not** ported here.

### Atomic writes and immutability

`content/atomic.rs::write_if_new` is the write primitive:

1. If the target already exists, return `Ok(false)` — **bodies are never
   overwritten** (immutability contract).
2. Otherwise write to a sibling temp file `.tmp_<hex>.md`, `fsync` it, then
   `rename` it into place (atomic on any POSIX filesystem).
3. On Unix, the parent directory is `fsync`ed so the rename survives a crash.
4. A lost `rename` race (another writer won) collapses to `Ok(false)`.

`stage_chunks` (in `content/mod.rs`) walks a `&[Chunk]`, writes each body, and
returns `StagedChunk` rows carrying `content_path` + `content_sha256` for SQLite
upsert. **Email chunks skip the disk write** — their content already lives in the
per-message raw archive, so an empty `content_path` is emitted and reads fall back
to `raw/<source>/<kind>/…`.

Summaries use `stage_summary` / `stage_summary_with_layout`, which are *idempotent
re-stages*: if a file exists with a matching body SHA-256 it is returned
unchanged; on mismatch the stale file is removed and rewritten so the DB row and
disk stay consistent.

### Reads and integrity verification

`content/read.rs` exposes the read + integrity surface:

- `read_chunk_file` / `read_summary_file` → `ChunkFileContents { body, sha256 }`,
  recomputing the body SHA-256 on read.
- `verify_chunk_file` / `verify_summary_file` compare against an expected hash;
  the summary variant returns `VerifyResult::{Ok, Mismatch { actual }, Missing}`.
- `resolve_within_content_root` treats stored `content_path` values as
  **untrusted**: it rejects absolute paths and any non-`Normal` component (`..`,
  prefixes) before touching disk, and — when the target exists — canonicalizes and
  asserts the result stays under `content_root`. This blocks a tampered DB row
  like `../../etc/passwd` from turning the reader into a file-disclosure
  primitive.

## Local vector DB

Module: `src/memory/store/vectors/`. A self-contained SQLite vector database with
brute-force cosine search — fast enough for on-device workloads up to ~100K
vectors. It only *persists and searches* vectors; all model calls go through an
injected `EmbeddingBackend`.

### Schema and packing

```text
vectors(id, namespace, text, embedding BLOB, metadata, created_at, updated_at)
  PRIMARY KEY (namespace, id)
store_meta(key, value, updated_at)   -- embed_provider, embed_dims
```

Embeddings are stored as packed little-endian `f32` BLOBs:

```rust
pub fn vec_to_bytes(v: &[f32]) -> Vec<u8>;   // each f32 -> 4 LE bytes
pub fn bytes_to_vec(bytes: &[u8]) -> Vec<f32>; // chunks_exact(4)
pub fn cosine_similarity(a: &[f32], b: &[f32]) -> f64; // 0.0..=1.0
```

`cosine_similarity` returns `0.0` for mismatched lengths, empty inputs, or
zero-magnitude vectors, and clamps the result to `[0.0, 1.0]` (so the inert
all-zero backend yields `0.0`, i.e. keyword-only behaviour).

`VectorStore::search_by_vector` scans every vector in the namespace, scores it,
sorts descending, truncates to `limit`, and only then parses the metadata JSON of
the surviving rows (avoiding N wasted JSON parses). Writes use `INSERT OR REPLACE`
on `(namespace, id)`; `insert_batch` wraps a transaction.

### Dimension guard

On first open, `VectorStore::open` persists `embed_provider` and `embed_dims` to
`store_meta`. On later opens it compares the stored dimension against the runtime
backend and **bails on mismatch**, preventing silent cosine corruption from
mixing vectors of different widths. Reconfiguring to a new embedding space means
deleting the DB or migrating.

### EmbeddingBackend

The model compute surface is abstracted behind a trait (`vectors/embedding.rs`):

```rust
#[async_trait]
pub trait EmbeddingBackend: Send + Sync {
    fn name(&self) -> &str;
    fn model_id(&self) -> &str;
    fn dimensions(&self) -> usize;
    fn signature(&self) -> String; // provider=…;model=…;dims=…
    async fn embed(&self, texts: &[&str]) -> anyhow::Result<Vec<Vec<f32>>>;
    async fn embed_one(&self, text: &str) -> anyhow::Result<Vec<f32>>;
}
```

`format_embedding_signature(name, model_id, dims)` is the single source of truth
for the space signature, so a signature derived from config is byte-identical to
one from a live backend (drift would silently split one embedding space into two).
`InertEmbedding` returns deterministic all-zero vectors of a fixed width
(`DEFAULT_EMBEDDING_DIM = 768`) — used by tests and by hosts that want keyword-only
retrieval without wiring a model. Real backends (Ollama, Voyage, OpenAI, …) are
plugged by the host; the storage primitive carries no network dependency.

## KV store and safety guard

Modules: `src/memory/store/kv.rs`, `src/memory/store/safety.rs`. A SQLite-backed
JSON key-value store with a global scope and per-namespace scopes:

```text
kv_global(key PRIMARY KEY, value_json, updated_at)
kv_namespace(namespace, key, value_json, updated_at) PRIMARY KEY (namespace, key)
```

`updated_at` is seconds-since-epoch as a float (matching OpenHuman). Namespaces are
normalised by `sanitize_namespace` — whitespace and `#` collapse to `_` (case is
preserved) so `"team alpha/#1"` and `"team_alpha/_1"` address the same bucket.
`records_for_scope` returns a namespace's own records plus the global records,
newest first. The connection is behind a `parking_lot::Mutex`.

### Safety / PII guard

Every KV write runs through `store/safety.rs` before it lands:

- `set_global` / `set_namespace` **reject** the key (and namespace) outright when
  `has_likely_secret` or `has_likely_pii` matches — KV identifiers must never
  carry credentials or personal identifiers.
- Values are scrubbed by `sanitize_json` before storage: sensitive object keys are
  replaced wholesale (`is_sensitive_key` — anything ending in `token`/`apikey`/
  `clientsecret`/`key`, or containing `password`/`secret`), every string value runs
  through `sanitize_text`, and nesting past `MAX_JSON_SANITIZE_DEPTH = 128` collapses
  the subtree.

`sanitize_text` applies three pattern passes and reports a `SanitizationReport`
(per-category counts):

| Pass | Examples | Replacement |
|---|---|---|
| `BLOCK_PATTERNS` | PEM / OpenSSH / PGP `PRIVATE KEY` blocks | `[REDACTED_PRIVATE_KEY]` |
| `REDACTION_PATTERNS` | `Bearer …`, `api_key=…`, `sk-…`, `ghp_…`, `AKIA…`, JWTs, Stripe/Slack/GitLab/SendGrid/Anthropic/Google keys | `[REDACTED]` |
| `PII_PATTERNS` | email, US SSN, Brazilian CPF, E.164 phone | `[REDACTED_PII]` |

The guard is conservative by design — it prefers false positives over leaking a
credential into a long-lived store. The exhaustive multilingual national-ID PII
module is **deferred**; the lightweight PII screen here is sufficient for the KV
"reject PII-like key" contract, and hosts can layer full national-ID redaction on
top.

## Entity-occurrence index

Module: `src/memory/store/entity_index/`. An inverted index mapping a canonical
entity id to every tree node (chunk or summary) it appears in, so retrieval can
resolve entity-scoped and co-occurrence queries in O(lookup). This is the
*occurrence index over tree nodes* — **not** the markdown contact registry.

```text
mem_tree_entity_index(
  entity_id, node_id, node_kind, entity_kind, surface,
  score, timestamp_ms, tree_id, is_user)
  PRIMARY KEY (entity_id, node_id)        -- idempotent re-index
  INDEX idx_entity_index_entity (entity_id)
  INDEX idx_entity_index_node   (node_id)
```

The composite primary key makes re-indexing the same `(entity, node)` association a
no-op `INSERT OR REPLACE`. Because that never *deletes*,
`clear_entity_index_for_node` must be called before re-indexing a re-scored node so
entities dropped from the new extraction don't leak.

Key operations (`entity_index/store.rs`):

- `index_entity` / `index_entities` — index a `CanonicalEntity` (or a batch in one
  transaction) against a node.
- `index_summary_entity_ids` — index LLM-curated canonical ids for a summary node
  (the `"<kind>"` prefix is written into `entity_kind`; the full id doubles as the
  `surface` placeholder).
- `lookup_entity(entity_id, limit)` → `Vec<EntityHit>`, newest first (`limit`
  clamped to `i64::MAX` so a huge `usize` can't wrap into a negative SQL `LIMIT`).
- `list_entity_ids_for_node` — distinct entity ids for a node, ordered by score
  then recency (drives topic-tree routing).
- `with_transaction` + `index_entities_tx` — fold entity indexing into a larger
  atomic write.

`EntityKind` (`entity_index/types.rs`) is a `#[non_exhaustive]` enum with stable
snake-case wire strings used in SQL and JSON. Mechanical kinds (`email`, `url`,
`handle`, `hashtag`) come from deterministic regex extraction (score `1.0`);
semantic kinds (`person`, `organization`, `location`, `event`, `product`,
`datetime`, `technology`, `artifact`, `quantity`, `misc`, `topic`) are
LLM-extracted. `EntityKind::parse` returns `Err` for unknown strings, so a corrupt
row fails the lookup loudly rather than decoding silently.

The `is_user` flag is resolved at index time through an injectable `SelfIdentity`
trait — the storage primitive must not depend on the host's identity registry, so
it defaults to `NoSelfIdentity` (always `false`). Hosts plug a real resolver.

## Content vs. derived: what is rebuildable

| State | Authoritative? | Rebuildable from |
|---|---|---|
| Markdown `.md` bodies (`content/`) | **Yes — source of truth** | n/a |
| `content_path` / `content_sha256` pointers | derived | the `.md` body bytes |
| `vectors` table (vector DB) | derived | re-embed the markdown via `EmbeddingBackend` |
| `kv_global` / `kv_namespace` | host-supplied state (not from markdown) | n/a (own source) |
| `mem_tree_entity_index` | derived | re-run extraction over tree nodes |

The contract: never trust a derived index over the markdown. A SHA-256 mismatch on
read means the derived row is stale and should be rebuilt; the immutable body is
always correct. Every stored item also carries provenance and a security `taint`
(internal vs `external_sync`, with unknown decoding as external) so untrusted
content stays attributable across the layers above.

## See also

- [Architecture-Overview](Architecture-Overview)
- [Ingest-Pipeline](Ingest-Pipeline)
- [Retrieval](Retrieval)
- [Entities-and-Graph](Entities-and-Graph)
