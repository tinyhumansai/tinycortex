---
description: Deterministic, scope-specific retrieval primitives that turn sealed summary trees, chunks, and the entity index into ranked, explainable answers.
---

# Retrieval

The retrieval layer (`src/memory/retrieval/`) turns the sealed summary trees,
chunk store, and entity index into ranked answers. It is a set of
**deterministic, scope-specific primitives** that all emit the same unified
[`RetrievalHit`] / [`QueryResponse`] shape. There is no classifier, gate, or
composer inside this layer: deciding *which* primitive to call and *how* to
combine results is the caller's job. This makes retrieval explainable and
testable — each primitive is a thin, read-only view over already-ported storage
modules.

```text
caller (orchestration: chooses primitives, fuses results)
        │
        ▼
retrieval primitives ── query_source / query_global / query_topic
                        search_entities / drill_down / cover_window / fetch_leaves
        │
        ├── hybrid_score  (folds 4 signals under a WeightProfile)
        ├── mmr_select    (MMR diversification)
        └── rerank        (cosine rerank vs. query embedding)
        │
        ▼
tree (summaries) · chunks (leaves) · score (embeddings + entity index) · graph
```

All public items are re-exported from `crate::memory::retrieval` (see
`mod.rs`).

## The unified hit shape

Every primitive returns the same `RetrievalHit`, defined in
`src/memory/retrieval/types.rs`. A hit is either a raw leaf chunk or a summary
node, discriminated by `node_kind`.

| Field | Type | Notes |
|-------|------|-------|
| `node_id` | `String` | Chunk id (leaf) or summary-node id (summary); stable, globally unique. |
| `node_kind` | `NodeKind` | `Leaf` (wire `"leaf"`) or `Summary` (wire `"summary"`). |
| `tree_id` | `String` | Provenance tree id; empty for bare leaves not yet sealed into a tree. |
| `tree_kind` | `TreeKind` | Provenance tree kind; always `TreeKind::Source` for leaves. |
| `tree_scope` | `String` | Human-readable scope, e.g. `slack:#eng`; empty for bare leaves. |
| `level` | `u32` | `0` for leaf chunks, `≥ 1` for summary nodes. |
| `content` | `String` | Raw chunk text (leaf) or sealed summary text (summary). |
| `entities` | `Vec<String>` | Canonical entity ids; empty on leaves. |
| `topics` | `Vec<String>` | Topic tags (leaf chunk tags or summary topics). |
| `time_range_start` / `time_range_end` | `DateTime<Utc>` | Inclusive time coverage (RFC3339 on the wire). |
| `score` | `f32` | Per-primitive relevance; higher = more relevant. |
| `child_ids` | `Vec<String>` | One level down (chunks for L1, summaries for L2+); empty on leaves. |
| `source_ref` | `Option<String>` | Chunk back-pointer for leaves; `None` for summaries. |

`NodeKind::Leaf` maps to a `mem_tree_chunks` row (level 0); `NodeKind::Summary`
maps to a `mem_tree_summaries` row (level ≥ 1). `NodeKind::as_str()` returns the
stable lowercase wire string, suitable for SQL discriminator columns.

Conversions live in `types.rs`: `hit_from_summary(node, tree_scope)`,
`hit_from_summary_with_tree(node, tree)`, and
`hit_from_chunk(chunk, tree_id, tree_scope, score)`. `leaf_tree_placeholder`
always returns `TreeKind::Source` — raw chunks belong to their originating
source tree even before that tree materialises.

### QueryResponse envelope

The `query_*` and `cover_window` primitives wrap hits in a `QueryResponse`:

```text
QueryResponse {
    hits:      Vec<RetrievalHit>,  // filtered, sorted, capped at the caller's limit
    total:     usize,              // pre-truncation match count
    truncated: bool,               // total > hits.len()
}
```

`QueryResponse::new(hits, total_matches)` takes the match count *before* the
limit is applied so callers can detect whether a higher-limit follow-up would
return more. `QueryResponse::empty()` yields `total = 0, truncated = false`.

## Primitives

| Primitive | Returns | Scope / axis |
|-----------|---------|--------------|
| `query_source` | `QueryResponse` | Per-source-tree summaries. |
| `query_global` | `QueryResponse` | Cross-source digest over a time window. |
| `query_topic` | `QueryResponse` | Entity/topic-scoped, reconstructed from the entity index. |
| `cover_window` | `QueryResponse` | Minimum-node cover of `[since, until]`. |
| `search_entities` | `Vec<EntityMatch>` | Fuzzy `LIKE` lookup over the entity index. |
| `drill_down` | `Vec<RetrievalHit>` | Descend a summary's `child_ids` (BFS). |
| `fetch_leaves` | `Vec<RetrievalHit>` | Batch-hydrate raw chunk leaves by id. |

### query_source

`query_source` (in `source.rs`) reads level ≥ 1 summaries from per-source trees.
Three selection modes, in priority order:

1. `source_id = Some` → one tree via `(kind = Source, scope = source_id)`.
2. `source_kind = Some` → every source tree whose scope prefix matches the kind
   (`chat` / `email` / `document`), resolved through a `PLATFORM_KINDS` registry
   (e.g. `slack:` and `discord:` classify as `chat`, `gmail:` as `email`,
   `notion:` as `document`).
3. Neither → every source tree.

`time_window_days = Some(d)` keeps only summaries whose
`[time_range_start, time_range_end]` overlaps `[now − d, now]`. When `query` is
`Some`, hits are reranked by cosine similarity against the query embedding;
otherwise they are ordered newest-first by `time_range_end`. `limit` defaults to
**10** when `0`.

### query_global and query_topic

OpenHuman retired standalone global (time) and topic (subject) trees — the
source hierarchy plus the entity index reconstructs both projections. Both live
in `global.rs`.

`query_global(since_ms, until_ms, source_kind, query, …)` is the **time axis**:
it gathers summaries from every source tree (optionally narrowed by
`source_kind`) whose envelope overlaps `[since_ms, until_ms]`, then orders by
recency or semantic similarity. It errors if `until_ms < since_ms`. `limit`
defaults to 10.

`query_topic(entity_id, since_ms, until_ms, query, …)` is the **subject axis**:
it walks the entity index (`lookup_entity`, capped at `TOPIC_LOOKUP_CAP = 200`
nodes) for a canonical id (e.g. `topic:phoenix`, `email:alice@example.com`),
resolves indexed summaries and leaves into hits, optionally restricts to a time
window, and reranks. A blank `entity_id` returns `QueryResponse::empty()`.

### cover_window

`cover_window` (in `cover.rs`) returns the **smallest set of nodes covering
every in-window chunk** — a frontier of summary nodes for fully-in-window
subtrees, plus raw leaf chunks for everything else. This is the read path a
"last 24h" morning brief uses so it summarises only fresh content instead of the
all-time root.

It is purely structural. Because seal sets a summary's envelope to the MIN/MAX
of its children, "envelope ⊆ window" ⇔ "all descendant leaves in window". The
**maximal** frontier is each eligible summary whose parent is not itself
eligible; those stand in for their whole subtree, and any in-window chunk they
don't cover is emitted raw. Versioned document sources get latest-wins handling
so a superseded revision's stale subtree never resurfaces. Results are grouped
by source then ordered ascending by start time; `limit` defaults to
`DEFAULT_LIMIT = 200`, and at most `MAX_WINDOW_CHUNKS = 5000` chunks are
scanned.

### search_entities

`search_entities` (in `search.rs`) is a fuzzy `LIKE` lookup over
`mem_tree_entity_index` — "I'm not sure `alice` is the canonical id, let me
search". Returns `Vec<EntityMatch>`:

| Field | Type | Notes |
|-------|------|-------|
| `canonical_id` | `String` | e.g. `email:alice@example.com`, `topic:phoenix`. |
| `kind` | `EntityKind` | Classification; preserved wire string. |
| `surface` | `String` | An example surface form that matched. |
| `mention_count` | `u64` | Total index rows grouped under this id. |
| `last_seen_ms` | `i64` | Epoch-millis of the newest mention. |

Matching rules: the query is trimmed and lowercased; a row matches when
`LOWER(entity_id) LIKE '%q%'` **OR** `LOWER(surface) LIKE '%q%'`; a non-empty
`kinds` slice narrows by `entity_kind IN (...)`. Output is grouped by canonical
id and ordered `mention_count DESC, last_seen_ms DESC`. A blank query returns no
matches (rather than dumping the index via `LIKE '%%'`). `limit` defaults to
`5` and is clamped to `MAX_LIMIT = 100`.

### drill_down

`drill_down(node_id, max_depth, query, embedder, limit)` (in `drill_down.rs`,
`async`) descends a summary's `child_ids` by BFS. The typical flow: you get a
summary hit from `query_source` / `query_topic` and want the next level down —
more summaries (L2+ nodes) or raw chunks (L1 nodes).

- `max_depth == 0` → empty (documented no-op); `max_depth = 1` is one-step
  expansion.
- Unknown `node_id` or a leaf id → empty (not an error — the caller can
  recover).
- BFS is batched per level: at most four reads (summaries / trees / chunks /
  chunk-embeddings).
- Versioned document sources get latest-wins: a doc-root superseded by a newer
  `version_ms` is skipped, and duplicates at the winning version are deduped;
  `deleted` summaries are dropped.
- When `query = Some`, visited children are reranked by cosine similarity
  against the query embedding (un-embedded children sort last); when `None`,
  children stay in BFS order. `limit`, when set, truncates the final output.

### fetch_leaves

`fetch_leaves(config, chunk_ids)` (in `fetch.rs`) batch-hydrates raw chunks by
id into the unified hit shape — "given these chunk ids, give me full content +
metadata so I can cite". Two batched reads (chunks + scores) replace `2N` per-id
queries. The batch is capped at `MAX_BATCH = 20` (extra ids are truncated, no
error); missing ids are silently skipped, so partial failures are visible via
`hits.len() < ids.len()`. Each hit's `score` comes from `mem_tree_score`, or
`0.0` when the chunk has no score row. Input order is preserved.

## Hybrid scoring

`scoring.rs` supplies the deterministic signal functions and folds four signals
into a `RetrievalScoreBreakdown` under the active `WeightProfile`. The weight
profiles themselves live in `crate::memory::config` and are read from config —
never hardcoded in the scorer.

### Signal functions

`keyword_relevance(query, content) -> f64` — the fraction of distinct
lowercased query tokens that appear as distinct tokens in the content, in
`[0.0, 1.0]`. An empty query or content scores `0.0`. Deliberately simple and
dependency-free: it is a keyword *signal*, not a ranking function on its own.

`freshness(updated_at_ms, now_ms, half_life_days) -> f64` — exponential
half-life decay in `[0.0, 1.0]`. A hit at `now` scores `1.0`; one
`half_life_days` old scores `0.5` (`0.5^(age_days / half_life_days)`). Future
timestamps (clock skew) clamp to `1.0`; a non-positive half-life degrades to a
hard `1.0` (no decay). The default half-life is
`DEFAULT_FRESHNESS_HALF_LIFE_DAYS = 7.0`.

### hybrid_score and the breakdown

```rust
pub fn hybrid_score(
    profile: &WeightProfile,
    graph_relevance: f64,
    vector_similarity: f64,
    keyword_relevance: f64,
    freshness: f64,
) -> RetrievalScoreBreakdown
```

Each signal is expected in `[0.0, 1.0]`. The final score is the weighted sum:

```text
final_score = profile.graph     · graph_relevance
            + profile.vector     · vector_similarity
            + profile.keyword    · keyword_relevance
            + profile.freshness  · freshness
```

The returned `RetrievalScoreBreakdown` (from `crate::memory::types`) carries
each contribution so a caller can both rank and *explain*:

| Field | Source signal |
|-------|---------------|
| `keyword_relevance` | Lexical / keyword overlap. |
| `vector_similarity` | Dense vector (cosine) similarity. |
| `graph_relevance` | Graph / co-occurrence proximity. |
| `episodic_relevance` | Always `0.0` here — episodic memory is not a tree-retrieval signal, but the field is carried for wire compatibility. |
| `freshness` | Recency decay. |
| `final_score` | The weighted combination used for ranking. |

### Weight profiles

`WeightProfile` has four `f64` weights (`graph`, `vector`, `keyword`,
`freshness`). The four named constants (from `config.rs`):

| Profile (wire name) | graph | vector | keyword | freshness |
|---------------------|------:|-------:|--------:|----------:|
| `balanced` | 0.35 | 0.35 | 0.15 | 0.15 |
| `semantic` | 0.15 | 0.65 | 0.20 | 0.00 |
| `lexical` | 0.25 | 0.15 | 0.60 | 0.00 |
| `graph_first` | 0.55 | 0.30 | 0.15 | 0.00 |

`WeightProfile::by_name(name)` resolves a profile by its wire string;
**unknown names fall back to `balanced`**. `RetrievalConfig::default_profile`
is `BALANCED`, applied when a query does not specify a profile.

## MMR diversification

`mmr.rs` provides Maximal Marginal Relevance to pick a diverse subset that
balances query relevance against intra-set redundancy.

```rust
pub fn mmr_select(
    query_vec: &[f32],
    candidates: &[MmrCandidate<'_>],
    limit: usize,
    lambda: f64,
) -> Vec<MmrResult>
```

Each `MmrCandidate` carries a caller-side `index` (echoed back so the result
resolves to its original record), an `embedding`, and a precomputed `relevance`.
At each greedy step the selected item maximises:

```text
mmr(c) = lambda · relevance(c) − (1 − lambda) · max_similarity(c, selected)
```

`lambda` is clamped to `[0.0, 1.0]`: `1.0` = pure relevance (no diversity),
`0.0` = pure diversity, `0.7` is the recommended default. Cosine similarity is
reused from `crate::memory::store::vectors`. Each `MmrResult` returns the
`index` plus the MMR `score` at the step it was selected — not comparable across
runs with different `lambda`. Empty candidates or `limit = 0` return an empty
vector.

## Semantic rerank

The `query_*` and `drill_down` primitives share an internal rerank helper
(`rerank.rs`): when `query` is `Some`, each hit is decorated with the cosine
similarity between the query embedding and the hit's stored embedding, then
sorted similarity-DESC with `time_range_end`-DESC as a tiebreak. Hits with no
embedding sort to the bottom while preserving incoming order. Embedding failures
(e.g. a local model being unavailable) never surface as an error — the helper
falls back to the incoming order.

## See also

- [Summary Trees](memory-tree.md)
- [Scoring and Extraction](scoring-and-extraction.md)
- [Storage Primitives](storage-primitives.md)
- [Entities and Graph](entities-and-graph.md)
