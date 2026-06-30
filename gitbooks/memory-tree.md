---
description: How TinyCortex compresses raw leaves into recursive sealed summaries — the bucket-seal tree, its lifecycle, label strategies, deferred writes, time flush, and tunable budgets.
---

# Memory Tree & Compression

The summary tree is TinyCortex's compression mechanism. Above the raw chunk leaves, the engine folds material into a tree of immutable summary nodes: many leaves seal into one L1 summary, many L1 summaries seal into one L2 summary, and so on. This keeps recall over long histories cheap — a read can answer from a handful of high-level summaries instead of scanning thousands of leaves — while the original markdown content files remain the authoritative source of truth.

## The compression pipeline

Think of the tree as a decay/compression funnel. Fresh material arrives as leaves and accumulates in an unsealed frontier buffer. Once a buffer crosses its gate, the engine **seals** that bucket into a single, immutable summary one level up, then cascades: that new summary becomes a leaf for the next level, which may itself seal, and so on. The detail "decays" upward into progressively terser summaries, but nothing is destroyed — the leaves remain on disk.

The mechanism is spread across a few real modules under [`src/memory/tree/`](architecture.md):

- `tree/summarise` — the `Summariser` seam that folds a bucket of inputs into one summary node's content.
- `tree/bucket_seal` — the append -> seal -> summarise engine, including the seal gates and the cascade.
- `tree/flush` — the time-based trigger that force-seals low-volume buffers so they do not park below threshold forever.
- `tree/store/hotness` — the counters and thresholds that decide when a topic tree is worth materialising.

![Compression pipeline](.gitbook/assets/compression-pipeline@2x.png)

This page documents the **bucket-seal SQLite tree** engine: the kinds, the persisted types, the append -> seal -> summarise lifecycle, label strategies, deferred writes, and the tunable constants. (A separate markdown time-tree, `src/memory/tree/runtime/`, persists a root -> year -> month -> day -> hour hierarchy as markdown files with its own summariser; it is out of scope here.)

## Tree kinds

A tree is keyed by `(kind, scope)` in `mem_tree_trees`. All three kinds share one schema and one seal engine.

| `TreeKind` | Wire string | Scope holds | Materialised |
| --- | --- | --- | --- |
| `Source` | `"source"` | One ingest source id (e.g. `chat:slack:#eng`, `email:gmail:user`) | **Primary** — one tree per source |
| `Topic` | `"topic"` | A per-entity / topic id | Reconstructed when an entity gets hot |
| `Global` | `"global"` | The literal `"global"` (`GLOBAL_SCOPE`) | Singleton cross-source daily digest |

`TreeKind::as_str` / `TreeKind::parse` are the canonical conversions to and from the SQL discriminator column; preserve these wire strings when porting. The enum is `#[non_exhaustive]`.

Source trees are the workhorse. Topic and global trees are **reconstructed** views: a topic tree is only worth materialising once an entity is "hot" enough. The hotness thresholds live in `store::types`:

| Constant | Value | Meaning |
| --- | --- | --- |
| `TOPIC_CREATION_THRESHOLD` | `10.0` | Hotness above which a topic tree is materialised |
| `TOPIC_ARCHIVE_THRESHOLD` | `2.0` | Hotness below which a topic tree becomes an archive candidate |
| `TOPIC_RECHECK_EVERY` | `100` | Ingests touching the entity between full hotness recomputes |

Hotness inputs (`EntityIndexStats`) and the persisted counters (`HotnessCounters`) track 30-day mention counts, distinct sources, last-seen, query hits, and graph centrality.

### Profiles and the factory

`TreeFactory` (in `factory.rs`) is the uniform construction/append API. A `TreeProfile` maps to a `TreeKind` and carries the kind's defaults:

```text
TreeFactory::source(scope)  -> TreeProfile::Source -> TreeKind::Source
TreeFactory::topic(scope)   -> TreeProfile::Topic  -> TreeKind::Topic
TreeFactory::global()       -> TreeProfile::Global -> TreeKind::Global  (scope = "global")
TreeFactory::from_tree(&t)  -> reconstruct the factory from an existing Tree row
```

Key methods: `get_or_create(config) -> Tree`, `insert_leaf(config, &leaf, summariser)`, `seal_now(config, summariser)` (force-flush), and `archive(config)`. The factory also picks the kind's default `label_strategy` (see [Label strategies](#label-strategies)).

## Persisted types

Three row types make up a tree, all in `store::types` and re-exported from the module root.

### `Tree`

One tree instance.

| Field | Type | Notes |
| --- | --- | --- |
| `id` | `String` | Primary key |
| `kind` | `TreeKind` | Family discriminator |
| `scope` | `String` | Source id, entity id, or `"global"` |
| `root_id` | `Option<String>` | Current top summary id; `None` before the first L1 seal |
| `max_level` | `u32` | Highest level ever sealed; `root_id` lives here |
| `status` | `TreeStatus` | `Active` or `Archived` |
| `created_at` | `DateTime<Utc>` | Row creation time |
| `last_sealed_at` | `Option<DateTime<Utc>>` | Most recent seal, or `None` |

`TreeStatus::Archived` trees stay queryable but accept no new leaves or seals.

### `SummaryNode`

A sealed, **immutable** node one level above its children. `child_ids` is fixed at seal time: for L1 nodes those are leaf `chunk.id`s, for L2+ they are lower-level summary ids.

| Field | Type | Notes |
| --- | --- | --- |
| `id`, `tree_id`, `tree_kind` | | Identity + denormalised owning kind |
| `level` | `u32` | 1 = summary over leaves, 2 = over L1, ... |
| `parent_id` | `Option<String>` | `None` while this node is the current root |
| `child_ids` | `Vec<String>` | Children sealed under this node, fixed at seal time |
| `content` | `String` | Summariser output |
| `token_count` | `u32` | Token count of `content` |
| `entities` | `Vec<String>` | Curated subset of children's canonical entity ids |
| `topics` | `Vec<String>` | Curated topic labels |
| `time_range_start` / `time_range_end` | `DateTime<Utc>` | Earliest / latest child timestamp covered |
| `score` | `f32` | Max of children's scores at seal time |
| `sealed_at` | `DateTime<Utc>` | When this node sealed |
| `deleted` | `bool` | Tombstone; stays `false` on new seals |
| `embedding` | `Option<Vec<f32>>` | Optional content embedding; the legacy column is left NULL (embeddings persist to a per-model sidecar table) |
| `doc_id` / `version_ms` | `Option<...>` | Document identity/version for document source trees |

### `Buffer`

The unsealed frontier at a given `(tree_id, level)` — one row per level per tree.

| Field | Type | Notes |
| --- | --- | --- |
| `tree_id` | `String` | Owning tree |
| `level` | `u32` | Level whose frontier this holds; L0 buffers raw leaves |
| `item_ids` | `Vec<String>` | Pending child ids awaiting the next seal, in arrival order |
| `token_sum` | `i64` | Running token total; drives the L0 budget gate |
| `oldest_at` | `Option<DateTime<Utc>>` | Arrival time of the oldest item; `None` when empty; drives the time flush |

Helpers: `Buffer::empty(tree_id, level)`, `is_empty()`, and `is_stale(now, max_age)` (false for an empty buffer).

## The append -> seal -> summarise lifecycle

The engine lives in `bucket_seal.rs`. The flow for an append:

```text
append_leaf(config, &tree, &leaf, summariser, &strategy)
  1. append_to_buffer(tree, level=0, chunk_id, token_count, timestamp)
        - dedup on item_id (idempotent retry after a failed cascade)
        - token_sum += token_count;  oldest_at = min(oldest_at, timestamp)
  2. cascade_all_from(tree, start_level=0, force_now=None, ...)
        loop (max MAX_CASCADE_DEPTH = 32):
          buf = get_buffer(tree, level)
          if !should_seal(buf): break
          if buf.is_empty(): break
          summary_id = seal_one_level(tree, buf, ...)   // seals level -> level+1
          level += 1
        return sealed summary ids
```

![Memory decay](.gitbook/assets/memory-decay@2x.png)

### Seal gates (`should_seal`)

Gates differ by level. Budgets are read from `MemoryConfig::tree` at runtime, **not** hardcoded:

- **L0 (leaves -> L1):** seal when `token_sum >= input_token_budget`.
- **L>=1 (summaries -> next level):** seal when `item_ids.len() >= summary_fanout`.

Counting siblings at upper levels keeps the tree's fan-in stable regardless of summariser quality. An empty buffer never seals.

### Sealing one level (`seal_one_level`)

When a buffer seals at `level` into a node at `target_level = level + 1`:

1. `hydrate_inputs` loads the buffered `item_ids` into `SummaryInput`s (raw leaf text at L0, lower-level summary text above). Refuses to seal if hydration yields nothing.
2. The node's `time_range_start`/`end` come from the min/max of input ranges; `score` is the max input score (clamped `>= 0.0`).
3. `summariser.summarise(inputs, ctx)` folds the inputs, where `ctx.token_budget = output_token_budget`. **A summariser error or a blank result falls back to `fallback_summary`** so `content = ""` is never persisted.
4. `resolve_labels` populates `entities`/`topics` per the [label strategy](#label-strategies).
5. In one transaction: insert the immutable `SummaryNode`, index its entity ids, **backlink** each child to the new parent (`mem_tree_chunks.parent_summary_id` at L0, `mem_tree_summaries.parent_id` above) only where still NULL, clear the sealed buffer, push the new summary into the parent-level buffer (updating its `token_sum`/`oldest_at`), and — if `target_level > max_level` — update the tree's `root_id`/`max_level`, else just refresh `last_sealed_at`.

`MAX_CASCADE_DEPTH = 32` caps the cascade as a guard against runaway loops if token accounting ever slips.

### The summariser seam

The LLM is abstracted behind the `Summariser` trait so the crate never depends on a network backend:

```rust
#[async_trait]
pub trait Summariser: Send + Sync {
    fn name(&self) -> &str { "summariser" }
    async fn summarise(
        &self,
        inputs: &[SummaryInput],
        ctx: &SummaryContext<'_>,
    ) -> Result<SummaryOutput>;
}
```

The default `ConcatSummariser` (name `"concat"`) is fully deterministic and dependency-free: it delegates to `fallback_summary`, which sorts inputs **priority-first by score** (stable sort, so equal scores keep chronological order), joins non-blank inputs with a `"— "` provenance prefix, and clamps the result to budget via `clamp_to_budget` (a char ceiling of `budget * 4`). The built-in summarisers always emit empty `entities`/`topics` on `SummaryOutput`; those fields are filled separately by the label strategy. A host wires its own LLM-backed `Summariser` over this same fallback.

## Label strategies

How a sealed node's `entities`/`topics` get populated. The serde-facing contract enum is `TreeLabelStrategy` (default `Inherit`); it resolves into the runtime `LabelStrategy` used by the seal:

| `TreeLabelStrategy` | Runtime `LabelStrategy` | Behaviour |
| --- | --- | --- |
| `Inherit` (default) | `UnionFromChildren` | Dedup-merge each input's `entities` and `topics` into the parent |
| `Extract` | `ExtractFromContent(extractor)` | Run the extractor on the new summary text, canonicalise to entity ids + topic labels |
| `Empty` | `Empty` | Leave both fields empty |

`TreeLabelStrategy::Extract` requires a caller-supplied `EntityExtractor`; when none is supplied, `resolve` degrades gracefully to `Inherit`/`UnionFromChildren`.

Per-kind defaults from `TreeFactory::label_strategy`:

- **Source** trees re-extract from synthesised summary text (`ExtractFromContent` with a `CompositeExtractor::regex_only()`).
- **Topic** and **Global** trees use `Empty` — their scope already pins the dominant theme.

## Deferred writes and `seal_pending`

For queue-driven ingest, the cascade can be split off from the append. The contract type `TreeWriteRequest` carries a `deferred: bool`; when set, the engine only stages the leaf in the L0 buffer and reports back via `TreeWriteOutcome`:

```text
TreeWriteOutcome {
    new_summary_ids: Vec<String>,  // ids sealed during this call (empty when deferred)
    seal_pending: bool,            // caller should enqueue a follow-up seal job
}
```

The engine entry point is `append_leaf_deferred(config, &tree, &leaf) -> Result<bool>`: it appends to the L0 buffer (no cascade) and returns whether `should_seal` is now true for that buffer, i.e. whether a seal job should be enqueued. This hands the WHEN-to-seal decision to the [Job Queue](job-queue.md) instead of blocking the ingest path.

## Time-based flush

The token/fan-in gates can leave a low-volume source's buffer parked below threshold indefinitely, hurting recall. `flush.rs` adds a time trigger:

- `flush_stale_buffers(config, max_age, ...)` force-seals every **L0** buffer whose `oldest_at` is older than `max_age`, returning the number of seal calls fired.
- `flush_stale_buffers_default(...)` uses `DEFAULT_FLUSH_AGE_SECS`.
- `force_flush_tree(config, tree_id, now, ...)` force-seals one tree's L0 buffer immediately (e.g. "user disconnected this account").

A forced flush passes `force_now = Some(now)` to `cascade_all_from`, which seals the start (L0) buffer regardless of the token budget; upper levels are **never** force-sealed (that would create degenerate single-child summaries and collapse the tree into a chain — they gate on fan-in naturally).

## Key constants

Compile-time defaults from `crate::memory::config` (re-exported through `store::types`). The engine reads the live values from `MemoryConfig::tree` (`TreeConfig`) at runtime; these consts are the defaults and test references.

| Constant | Value | `TreeConfig` field | Role |
| --- | --- | --- | --- |
| `INPUT_TOKEN_BUDGET` | `50_000` | `input_token_budget` | L0 seal gate: seal when buffered `token_sum` reaches this |
| `OUTPUT_TOKEN_BUDGET` | `5_000` | `output_token_budget` | Max tokens a produced summary may occupy (`ctx.token_budget`) |
| `SUMMARY_FANOUT` | `10` | `summary_fanout` | L>=1 seal gate: seal when this many siblings buffer |
| `DEFAULT_FLUSH_AGE_SECS` | `604_800` (7 days) | `flush_age_secs` | Default age at which a non-empty L0 buffer is force-sealed |

`store::types::DEFAULT_FLUSH_AGE_SECS` mirrors the config value as `i64` to compose with `chrono::Duration::seconds`.

## See also

- [Ingest Pipeline](ingest-pipeline.md)
- [Scoring and Extraction](scoring-and-extraction.md)
- [Retrieval](retrieval.md)
- [Job Queue](job-queue.md)
