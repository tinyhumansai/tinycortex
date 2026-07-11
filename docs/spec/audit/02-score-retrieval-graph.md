# Audit 02 — Scoring, Retrieval, Graph, Entities (`src/memory/score/`, `retrieval/`, `graph/`, `entities/`)

Verified findings, most severe first. IDs `RS-*` are referenced from the
[improvement plan](../improvement-plan.md).

## Major

### RS-1. LLM soft-fallback silently penalizes borderline chunks
`src/memory/score/mod.rs:181-221` with `src/memory/score/extract/llm.rs:162-187`

`score_chunk` sets `llm_consulted = true` whenever `llm.extract()` returns
`Ok`, then uses the full `combine` (denominator includes `llm_importance`
weight 2.0). But `LlmEntityExtractor::extract` **never returns `Err`** — every
failure soft-falls back to `Ok(ExtractedEntities::default())` with
`llm_importance: None`, and even a successful response may omit `importance`.
In all these cases `llm_importance = 0.0` flows into the full combine, scaling
the total by ~0.82. Scenario: LLM provider offline, borderline chunk with cheap
total 0.36 → final 0.295 < 0.3 threshold → chunk permanently dropped — exactly
what the comment at `mod.rs:212-216` says the code prevents. The existing test
(`mod_tests.rs:159`) simulates an `Err` the real extractor cannot produce.

**Fix:** gate the full combine on `extracted.llm_importance.is_some()`.

### RS-2. `query_topic` applies the time-window filter after a hard 200-row truncation
`src/memory/retrieval/global.rs:33,110` + `src/memory/score/store.rs:414-430`

`lookup_entity` returns the newest 200 rows; the `since_ms/until_ms` retain
runs afterwards. An entity with >200 mentions queried with a window in the past
returns zero or partial hits even though matching rows exist, and `total`
misreports.

**Fix:** push `since/until` into the SQL WHERE clause before the LIMIT.

### RS-3. The documented hybrid scoring layer is never wired
`src/memory/retrieval/mod.rs:23-29` vs `scoring.rs`, `mmr.rs`, `graph_adapter.rs`

Module docs claim scoring "folds graph / vector / keyword / freshness into a
RetrievalScoreBreakdown under the active WeightProfile". In reality
`hybrid_score`, `keyword_relevance`, `freshness`, `mmr_select`, and the entire
graph path have **zero callers** outside their own tests — every primitive
ranks by cosine rerank or recency only. `rerank_by_semantic_similarity`
(`rerank.rs:22-59`) discards the computed similarity, so `RetrievalHit.score`
carries the stored admission score, not the ranking score. `mmr_select` accepts
`query_vec` and ignores it (`mmr.rs:93`).

**Fix:** wire the composition (see improvement plan phase 2) or correct the
module docs and prune/annotate the dead API.

### RS-4. Graph co-occurrence is an N+1 query storm with truncated weights
`src/memory/graph/query.rs:48-62` + `src/memory/retrieval/graph_adapter.rs:20,36-51`

The derivation issues 1 + up-to-500 separate SQL queries (one
`entities_on_node` per subject node) instead of the single self-join the docs
describe. `OCCURRENCE_LOOKUP_LIMIT = 500` truncates newest-first, so edge
weights are systematically undercounted for popular entities and
"strongest neighbor" ordering can be wrong.

**Fix:** add a SQL self-join fast path in the adapter (keep the trait for
tests).

### RS-5. `query_source`/`query_global` materialize the entire summary store per query
`src/memory/retrieval/source.rs:92-139`

`collect_source_hits` loads every non-deleted summary at every level of every
selected tree (plus sidecar embedding hydration for all of them) before the
time-window `retain` and `truncate(limit)` run in Rust.
`list_summaries_in_window` exists (used by `cover.rs:141`) but isn't used here.
A store with years of history pays O(total summaries) for a "last 7 days,
limit 10" query.

**Fix:** push window/level filters into SQL; hydrate embeddings only for
surviving candidates.

## Minor

- **RS-6** `retrieval/search.rs:47-50` — LIKE wildcards not escaped; searching
  `"100%"` or `"_"` matches everything up to the limit. Add escaping +
  `ESCAPE '\'`.
- **RS-7** `retrieval/global.rs:169-173` — `ms_to_utc` falls back to
  `Utc::now()` on out-of-range input despite the doc saying "saturating";
  `i64::MIN`/`MAX` sentinels silently become "now". Saturate to
  `MIN_UTC`/`MAX_UTC`.
- **RS-8** `retrieval/drill_down.rs:154-167` — deleted check runs after
  doc-version bookkeeping; a soft-deleted winning revision suppresses the
  surviving older revision, so the document disappears entirely.
- **RS-9** `retrieval/rerank.rs:6-7` — doc claims un-embedded hits "preserve
  their incoming order", but the comparator tie-breaks them by
  `time_range_end` DESC.
- **RS-10** two divergent `cosine_similarity` impls: `score/embed.rs:71-87`
  (raw f32, can be negative) vs `store/vectors/store.rs:422-441` (f64,
  clamps to [0,1], used by MMR) — MMR can't distinguish anti-correlated from
  orthogonal candidates. Consolidate on one implementation.
- **RS-11** `retrieval/cover.rs:35,68-74` — when `total > limit` truncation is
  alphabetical by `tree_scope` (whole sources dropped), and
  `MAX_WINDOW_CHUNKS = 5_000` silently truncates with no indicator distinct
  from the limit-based `truncated` flag.
- **RS-12** `score/extract/regex.rs:32` — handle regex captures trailing
  punctuation: "ping @alice." indexes `handle:alice.` as a distinct entity from
  `handle:alice`, splitting co-occurrence weight.
- **RS-13** `score/mod.rs:337-353` — `clear_entity_index_for_node` runs only
  when `result.kept`; a kept→dropped re-score leaves phantom entity rows, and
  `resolve_topic_hits` (`global.rs:157-163`) has no dropped-status check, so
  tombstoned chunks can still surface in topic queries. Graph reads also never
  filter soft-deleted summaries.
- **RS-14** `entities/frontmatter.rs:101-107` + `store.rs:55-62` — a
  hand-edited entity file ending in `---` without trailing newline fails the
  fence parse, `extract_notes` returns `""`, and the next `put_entity` rewrites
  the file with an empty body — silently deleting the user's notes. Accept a
  fence at EOF, or refuse to overwrite when parse fails on a non-empty file.

## Test-coverage gaps

- `rerank.rs` has no test file at all.
- The real LLM soft-fallback path (`Ok(default)` with `llm_importance: None`,
  RS-1) is untested.
- No test for `query_topic` beyond `TOPIC_LOOKUP_CAP` with a window (RS-2), or
  that dropped/deleted nodes are excluded from topic results (RS-13).
- No LIKE-metacharacter test (RS-6); no zero-vector / negative-similarity MMR
  test (RS-10).
- `ConfigEntityIndex` against real SQLite (cap interaction, deleted nodes)
  barely tested.
- No frontmatter round-trip test for missing final newline (RS-14); no
  `MAX_WINDOW_CHUNKS` truncation or source-starvation test (RS-11).
