# Tree, Queue, and Retrieval Spec

OpenHuman modules: `memory_tree`, `memory_queue`, `memory_search`.

## Responsibility

These modules turn chunks into navigable memory. They score and admit content,
extract entities, embed text, append leaves to buffers, seal summaries, run
async jobs, and expose retrieval primitives for agents and RPC callers.

## Tree IO

Write contract:

- `TreeWriteRequest`: `tree_id`, `tree_kind`, `leaf`, `label_strategy`,
  `deferred`.
- `TreeLeafPayload`: `chunk_id`, `token_count`, `timestamp`, `content`,
  `entities`, `topics`, `score`.
- `TreeLabelStrategy`: `inherit`, `extract`, `empty`.
- `TreeWriteOutcome`: `new_summary_ids`, `seal_pending`.

Read contract:

- `TreeReadRequest`: `tree_id`, optional `start_node_id`, `max_depth`,
  optional natural-language `query`, optional `limit`.
- `TreeReadHit`: `node_id`, `node_kind`, `level`, `content`, score.
- `TreeReadResult`: hits, total, tree id.

## Tree Mechanics

Tree mechanics are kind-agnostic. `bucket_seal`, `flush`, `registry`, and
`summarise` receive tree kind/scope rather than branching on policies. Policy
such as source routing, topic hotness, global cadence, or digest behavior lives
outside generic mechanics.

Tree storage constraints:

- Summary nodes are immutable after seal.
- `child_ids` are fixed at seal time.
- Parent/root pointers change only via controlled seal/root-split updates.
- Latest-document-version filtering uses `doc_id` and `version_ms`, not rewrites.
- Retrieval tolerates missing legacy embeddings by sorting them lower.

## Scoring

`score_chunk` performs extraction, signal computation, optional LLM importance,
admission gating, canonicalization, and persistence of score rationale.

Signals:

- token-count shape.
- unique word diversity.
- metadata weight by source kind.
- provider/source weight.
- interaction bonus.
- entity density.
- optional LLM importance.

Admission defaults include drop/keep bands. TinyCortex should keep score
breakdowns persisted so decisions remain auditable.

## Entity Extraction and Embedding

Extractor contract:

- async `extract(text) -> ExtractedEntities`.
- regex extractor for emails, URLs, handles, hashtags.
- LLM extractor for semantic entities and importance.
- composite extractor merges and tolerates failures.

Embedding contract:

- historical fixed vector length in OpenHuman: 768.
- default Ollama endpoint/model: local `nomic-embed-text`.
- inert zero-vector implementation for tests.
- packing/unpacking helpers for SQLite BLOB storage.
- cosine similarity short-circuits zero magnitude to 0.

TinyCortex stores embedding identity in two places: the generic vector DB
records provider and dimensions in `store_meta`, while chunk-tree embeddings
live in per-model sidecars keyed by `model_signature` (`model@dim`) and store
each row's dimension. Re-embed tombstones record rows that cannot be backfilled
for a given signature.

## Queue Jobs

`memory_queue` stores jobs in `mem_tree_jobs`. Required fields include kind,
payload JSON, status, attempts, lock metadata, availability time, last error,
dedupe key, typed failure reason/class, and lifecycle timestamps.

Job kinds:

- `extract_chunk`
- `append_buffer`
- `seal`
- `flush_stale`
- `reembed_backfill`
- `seal_document`

Retired OpenHuman kinds `topic_route` and `digest_daily` may exist in older
queues and must be migration-safe.

Statuses:

- `ready`, `running`, `done`, `failed`, `cancelled`.

Handler outcomes:

- `Done`
- `Defer { until_ms, reason }`

`Defer` reschedules without burning retry attempts. LLM-bound jobs must take a
global concurrency semaphore. Workers must purge retired job kinds and recover
stale locks at startup. Settlement is guarded by the claimed row snapshot
(`attempts` plus `started_at_ms`) so a stale worker cannot overwrite a newer
claim.

## Queue Payloads

Core payloads:

- `ExtractChunkPayload { chunk_id }`
- `AppendBufferPayload { node, target }`
- `SealPayload { tree_id, level, force_now_ms }`
- `FlushStalePayload { max_age_secs }`, deduped by caller-supplied 3-hour UTC
  block.
- `ReembedBackfillPayload { signature }`
- `SealDocumentPayload { tree_scope, doc_id, version_ms, chunk_ids }`

`NodeRef` can be a leaf chunk or summary node. Append targets can identify a
source tree by source id or a topic tree by tree id. Dedupe keys must suppress
only active duplicate work.

## Retrieval Primitives

Retrieval controller functions:

- `query_source`: source-tree summary retrieval.
- `query_global`: reconstructed cross-source digest from source-tree summaries.
- `query_topic`: reconstructed entity/topic retrieval from the entity index and
  hydrated source-tree nodes.
- `cover_window`: select context covering a time window.
- `search_entities`: fuzzy lookup over entity index.
- `drill_down`: descend summary children.
- `fetch_leaves`: hydrate raw chunk leaves.

OpenHuman also had standalone global/topic tree jobs. Those job kinds are
retired here: old rows are skipped on claim and purged at worker bootstrap, and
the current global/topic query surfaces are projections over source trees.

## Hybrid Search

`memory_search` defines weight profiles:

- `balanced`: graph 0.35, vector 0.35, keyword 0.15, freshness 0.15.
- `semantic`: graph 0.15, vector 0.65, keyword 0.20.
- `lexical`: graph 0.25, vector 0.15, keyword 0.60.
- `graph_first`: graph 0.55, vector 0.30, keyword 0.15.

Hybrid search should surface component scores and final score.

## Runtime and Health

TinyCortex ports the `memory_tree/tree_runtime` engine, summariser abstraction,
types, and filesystem-backed runtime store. OpenHuman's RPC/CLI/bus/schema
surfaces and `memory_tree/health` doctor checks are not local modules yet.
TinyCortex should preserve doctor-style diagnostics for queues, embeddings,
tree stores, and content roots when those host-facing surfaces are added.

## TinyCortex Landing Area

```text
src/memory/tree/
src/memory/queue/
src/memory/score/
src/memory/retrieval/
```

Port order: tree/queue/retrieval types, scoring signal pure functions, entity
resolver, embedding math/packing, in-memory tree tests, then SQLite queue and
worker implementation.
