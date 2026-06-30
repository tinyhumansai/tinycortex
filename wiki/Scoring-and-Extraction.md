# Scoring & Extraction

After [chunking](Ingest-Pipeline), every chunk passes through the scoring /
admission / enrichment stage before it is allowed into the chunk store and the
[summary tree](Summary-Trees). This stage answers one question — *does this
chunk earn a place?* — and produces the enrichment artefacts (entities,
canonical ids, score breakdown) that later layers index and explain against.

The whole stage lives under `src/memory/score/` and is driven by a single
entry point, `score_chunk`, in `src/memory/score/mod.rs`. It composes four
seams:

- **Signals** (`score/signals/`) — cheap, deterministic per-chunk scores.
- **Extraction** (`score/extract/`) — pluggable entity extractors (regex /
  LLM / composite).
- **Resolver** (`score/resolver.rs`) — canonicalises surface forms into stable
  index keys.
- **Embeddings** (`score/embed.rs`) — fixed-dimension vectors for semantic
  rerank.

The crate never calls a real model on this path: the default extractor is
regex-only and the default embedder is the inert zero-vector backend. Hosts
inject real backends behind traits.

## The scoring call

```text
score_chunk(chunk, cfg) -> ScoreResult
  1. extractor.extract(content)                 always-on (regex composite)
  2. compute(signals)  ->  combine_cheap_only   excludes llm_importance
  3. short-circuit on cheap_total:
        >= definite_keep_threshold (0.85)  -> admit, skip LLM
        <= definite_drop_threshold (0.15)  -> drop,   skip LLM
        else (borderline)  -> run llm_extractor, merge, recompute, combine
  4. priority boost (+0.25, clamped to 1.0) if PRIORITY_TAG present
  5. admission gate: kept = total >= drop_threshold (0.3)
                     AND not tiny-and-entity-free
  6. canonicalise(extracted)  (unconditional, even for dropped chunks)
```

The LLM extractor is consulted **only in the borderline band**
`(definite_drop_threshold, definite_keep_threshold)` — obviously-trash and
obviously-substantive chunks never pay the model cost. The async-queue ingest
hot path uses `score_chunks_fast`, which forces `llm_extractor = None`
regardless of config.

### Thresholds (constants in `score/mod.rs`)

| Constant | Value | Meaning |
| --- | --- | --- |
| `DEFAULT_DROP_THRESHOLD` | `0.3` | Final gate; `total <` this → tombstoned, never stored. |
| `DEFAULT_DEFINITE_KEEP` | `0.85` | Cheap total `≥` this → admit without LLM. |
| `DEFAULT_DEFINITE_DROP` | `0.15` | Cheap total `≤` this → drop without LLM. |
| `PRIORITY_TAG` | `"priority_high"` | Marks high-signal source material at ingest. |
| `PRIORITY_BOOST` | `0.25` | Additive nudge for priority-tagged chunks (clamped to `1.0`). |

A chunk whose token count is below `TOKEN_MIN` (10) **and** that produced no
entities is dropped outright as "tiny and entity-free", regardless of the
metadata/source priors — unless it carries `PRIORITY_TAG`, which bypasses that
guard. The `ScoreResult` records `kept`, the final `total`, the full
`ScoreSignals` vector, a human-readable `drop_reason`, the merged
`ExtractedEntities`, and the `canonical_entities`.

## Scoring signals

Each signal is a function returning a value in `[0.0, 1.0]`, computed by
`signals::compute`. They are deliberately cheap and explainable: the per-signal
values are persisted alongside the total so an admit/drop decision is auditable
after the fact (see [ScoreRow](#scorerow-persistence)).

| Signal | Module | What it measures |
| --- | --- | --- |
| `token_count` | `signals/token_count.rs` | Length shape — penalises very short noise and oversized walls of text. |
| `unique_words` | `signals/unique_words.rs` | Lexical diversity (type-token ratio) — fires on repetitive low-information text. |
| `metadata_weight` | `signals/metadata_weight.rs` | Base weight from `SourceKind` (audience shape). |
| `source_weight` | `signals/source_weight.rs` | Per-provider authority inferred from a `provider:` tag. |
| `interaction` | `signals/interaction.rs` | Direct user engagement inferred from reserved tags. |
| `entity_density` | `signals/ops.rs` | Distinct entities per token, capped. |
| `llm_importance` | (extractor-supplied) | Optional LLM importance rating; `0.0` when no LLM ran. |

### Token-count shape

A plateau with linear ramps on both sides (`token_count.rs`):

```text
score
 1.0          ┌──────────────────────┐
              │                       │
 0.5  0───────┘                       └────────
      |       |                       |       |
     <10     30                     3000    8000  (tokens)
   TOKEN_MIN RAMP_LOW            RAMP_HIGH  TOKEN_MAX
```

`< TOKEN_MIN (10)` → `0.0`; linear `0→1` over `[10, 30]`; flat `1.0` over
`[30, 3000]`; linear `1.0→0.5` over `[3000, 8000]`; clamped `0.5` above
`TOKEN_MAX`. Oversized content keeps a floor of `0.5` — it still carries
information but loses the plateau bonus.

### Unique-word diversity

Type-token ratio (distinct words / total words, alphanumeric-trimmed,
lowercased). Below `MIN_TOTAL_WORDS` (5) the ratio is unreliable, so the signal
returns a neutral `0.5`. Otherwise: ratio `≤ 0.3` → `0.0` (heavy repetition),
ratio `≥ 0.7` → `1.0`, linear in between.

### Metadata weight (audience shape)

One weight per `SourceKind`:

| `SourceKind` | Weight |
| --- | --- |
| `Document` | `0.9` |
| `Email` | `0.8` |
| `Chat` | `0.5` |

### Source weight (provenance authority)

`infer_data_source` scans `Metadata.tags` for a `provider:<snake_case>` tag and
parses it to a `DataSource`. If present, the per-provider weight applies;
otherwise a kind-level fallback is used (`Email` `0.75`, `Document` `0.7`,
`Chat` `0.5`).

| `DataSource` | Weight | | `DataSource` | Weight |
| --- | --- | --- | --- | --- |
| `Conversation` | `0.9` | | `Telegram` | `0.6` |
| `MeetingNotes` | `0.85` | | `DriveDocs` | `0.6` |
| `Gmail` | `0.8` | | `Discord` | `0.5` |
| `Whatsapp` | `0.75` | | `OtherEmail` | `0.7` |
| `Notion` | `0.75` | | | |

### Interaction (engagement tags)

Inferred from reserved tags on the chunk; multiple tags stack and are clamped to
`1.0`. Absent all of them → neutral `0.5` (most content lacks explicit
engagement tags, so this signal alone never drops a chunk).

| Tag | Constant | Boost |
| --- | --- | --- |
| `sent` | `TAG_SENT` | `+0.6` (author) |
| `reply` | `TAG_REPLY` | `+0.5` (active dialogue) |
| `dm` | `TAG_DM` | `+0.3` (scoped audience) |
| `mention` | `TAG_MENTION` | `+0.2` (addressed) |

### Entity density

`entity_density_score(token_count, extracted)` = `unique_entity_count /
token_count`, normalised so `0.01` entities/token (1 entity per 100 tokens) maxes
the signal at `1.0`. Zero tokens → `0.0`.

### Weighted combine

`combine` is a weight-normalised sum (`weighted / total_weight`, clamped to
`[0.0, 1.0]`). Default weights (`SignalWeights::default`):

| Signal | Weight |
| --- | --- |
| `interaction` | `3.0` (strongest — direct engagement) |
| `metadata_weight` | `1.5` |
| `source_weight` | `1.5` |
| `token_count` | `1.0` |
| `unique_words` | `1.0` |
| `entity_density` | `1.0` |
| `llm_importance` | `0.0` (disabled) |

`combine_cheap_only` drops `llm_importance` from **both** numerator and
denominator. This matters: the short-circuit and any LLM-skipped/failed path use
`combine_cheap_only` so a zero importance term doesn't get divided into the full
denominator and artificially drag the total down. Only when the LLM actually ran
does the final score use the full `combine`. `SignalWeights::with_llm_enabled`
bumps `llm_importance` to `2.0`.

## Entity extraction

Extractors implement the async `EntityExtractor` trait
(`score/extract/composite.rs`):

```text
trait EntityExtractor {
    fn name(&self) -> &'static str;
    async fn extract(&self, text: &str) -> Result<ExtractedEntities>;
}
```

`CompositeExtractor` runs a chain and `merge`s their output; an extractor that
errors is skipped, never aborting ingest. The default,
`CompositeExtractor::regex_only()`, wraps a single `RegexEntityExtractor`.

### Regex extractor (always-on, mechanical)

`score/extract/regex.rs` finds deterministic identifier shapes — stable, high
precision, limited recall. Every match has `score = 1.0`, and spans are
**char** offsets (UTF-8 safe). It produces these `EntityKind`s:

| Kind | Shape | Notes |
| --- | --- | --- |
| `Email` | `a@b.com` | RFC-ish, boundary-guarded, case-insensitive. |
| `Url` | `http(s)://…` | Stops before trailing punctuation. |
| `Handle` | `@alice`, `alice#1234` | Discord-style discriminators included. |
| `Hashtag` | `#launch-q2` | Also emits a matching `ExtractedTopic`. |

### LLM extractor (semantic NER + importance)

`LlmEntityExtractor` (`score/extract/llm.rs`) is the optional second pass for
borderline chunks. It builds a `(system, user)` `ChatPrompt` (temperature `0.0`,
`max_tokens` 8192), hands it to a host-supplied `Arc<dyn ChatProvider>`, and
parses a structured-JSON response into entities, optional topics, and an
`importance` rating. **The crate ships no real provider** — tests inject a mock,
hosts wire their own.

Robustness contracts:

- **Span recovery** — LLMs are unreliable about offsets, so each returned
  surface is re-found in the source via string search; surfaces that can't be
  located (hallucinations) are dropped. LLM entities get `score = 0.85`.
- **Kind mapping** — `parse_kind` maps loose labels (`org`, `place`, `tool`,
  `ticket`, …) onto `EntityKind`. Kinds outside `allowed_kinds` become `Misc`,
  or are dropped under `strict_kinds`.
- **Soft fallback + retry** — up to 3 attempts; transport/EOF failures retry,
  permanent client errors (402/401/403, quota, bad key — matched by
  `is_non_retryable`) and wrong-shape JSON degrade immediately to an empty
  `ExtractedEntities`. Ingest never blocks on model availability.

Semantic kinds the LLM can emit go well beyond the mechanical four: `Person`,
`Organization`, `Location`, `Event`, `Product`, `Datetime`, `Technology`,
`Artifact`, `Quantity`, `Misc`, plus thematic `Topic`. `EntityKind::is_mechanical`
returns true only for `Email`/`Url`/`Handle`/`Hashtag`.

### Merging extractor output

`ExtractedEntities::merge` dedups entities by `(kind, lowercased_text,
span_start)` and topics by `label`. LLM importance merges by **maximum** (if
either side rated the chunk important, the higher rating survives, and the
reason follows the winner).

## Entity canonicalisation

`canonicalise` (`score/resolver.rs`) folds extracted surface forms into
`CanonicalEntity` records keyed by a stable `canonical_id` of the form
`<kind>:<normalised-surface>` (V1 = exact-match only; fuzzy cross-platform merge
is deferred). Normalisation per kind:

| Kind | Canonical id |
| --- | --- |
| `Email` | `email:<lowercased>` |
| `Handle` | `handle:<lowercased, leading @ stripped>` |
| `Hashtag` | `hashtag:<lowercased, leading # stripped>` |
| `Url` | `url:<trimmed, case preserved>` (path/query exact match) |
| semantic | `<kind>:<lowercased surface>` |

One `CanonicalEntity` is emitted per occurrence (spans preserved). Extracted
**topics** are promoted into the canonical stream under `EntityKind::Topic`
(span `0/0`, deduped by canonical id) so [topic trees](Summary-Trees) can route
on themes the same way they route on people and orgs. Note `topic:launch` and
`hashtag:launch` are intentionally kept separate.

## ScoreRow persistence

`score/store.rs` owns two derived tables (schema declared centrally in
`memory::chunks`):

- `mem_tree_score` — one `ScoreRow` per chunk: the `total`, the per-signal
  breakdown, `dropped`/`reason`, and `computed_at_ms`. Dropped chunks still get
  a row for diagnostics.
- `mem_tree_entity_index` — inverted index `entity_id → node_id` so retrieval
  resolves entity-scoped queries in O(lookup).

`ScoreRow` mirrors the persisted row:

```text
ScoreRow {
    chunk_id: String,
    total: f32,                 // the single persisted scalar
    signals: ScoreSignals,      // token_count, unique_words, metadata_weight,
                                //   source_weight, interaction, entity_density
    dropped: bool,
    reason: Option<String>,
    computed_at_ms: i64,
    llm_importance_reason: Option<String>,  // NOT persisted; reads back None
}
```

**Divergence from OpenHuman:** `mem_tree_score` has no `llm_importance` /
`llm_importance_reason` columns and no identity registry. So `llm_importance` is
an admission-time-only signal (it influenced `total`, but reads back as `0.0`),
`llm_importance_reason` reads back as `None`, and the `is_user` column is always
written `0`. The persistence helpers `persist_score` / `persist_score_tx` write
the score row plus, for kept chunks only, the entity-index rows — clearing stale
index rows first so a re-score that drops an entity doesn't leave a phantom
(`INSERT OR REPLACE` never deletes). Co-occurrence graph edges are **not**
written here — the [graph layer](Entities-and-Graph) is not yet ported.

## Embeddings

`score/embed.rs` produces a fixed-dimension vector per chunk/summary for
semantic rerank in [retrieval](Retrieval), behind the `Embedder` trait:

```text
trait Embedder {
    fn name(&self) -> &'static str;
    async fn embed(&self, text: &str) -> Result<Vec<f32>>;             // exactly EMBEDDING_DIM floats
    async fn embed_batch(&self, texts: &[&str]) -> Vec<Result<Vec<f32>>>;  // one Result per input slot
}
```

- **Dimension** is fixed at `EMBEDDING_DIM` = **768** (from
  `DEFAULT_EMBEDDING_DIM`). Mixing dimensions mid-run would corrupt cosine
  comparisons, so `check_embed_dim` / `pack_checked` / `unpack_embedding` enforce
  it at write and read time. Vectors are stored as little-endian BLOBs
  (`pack_embedding` / `unpack_embedding`); legacy NULL blobs decode as `None`.
- **Backend** — the crate ships only `InertEmbedder`, a deterministic
  zero-vector embedder (a ZST, no network, no randomness). Real network-backed
  backends (Ollama / OpenAI-compatible / cloud) are wired in by a host adapter.
- **Inert embeddings are inert by design** — because every text gets the same
  zero vector, `cosine_similarity` between any two is always `0.0` (it returns
  `0.0` for zero-magnitude or length-mismatched inputs to keep the rerank sort
  stable instead of surfacing `NaN`). Tests that need real reranking must stitch
  embeddings in via store accessors rather than rely on the inert path.

A failed `embed` is treated as "don't persist the row" so retries stay
idempotent on `chunk_id`; `embed_batch` isolates failures per slot so one bad
text doesn't strand the rest of a batch.

## See also

- [Ingest Pipeline](Ingest-Pipeline) — where scoring is invoked in the chunk flow.
- [Summary Trees](Summary-Trees) — how kept chunks and entities feed the tree.
- [Retrieval](Retrieval) — how embeddings and the entity index are queried.
- [Entities and Graph](Entities-and-Graph) — downstream use of canonical entities.
