# Feature-Test Spec — Scoring, Retrieval, Graph, Entities

Subsystems: `src/memory/score/`, `src/memory/retrieval/`, `src/memory/graph/`,
`src/memory/entities/`. Regresses findings `RS-1`..`RS-14` from
[`docs/spec/audit/02-score-retrieval-graph.md`](../audit/02-score-retrieval-graph.md)
and the coverage gaps listed at the bottom of that document. Cross-referenced
against [`docs/spec/improvement-plan.md`](../improvement-plan.md) phases 0.6,
0.10, and 1–6.

This document specifies test *intent* (given/when/then), not implementation.
Cases marked "pre-fix" describe the regression a fix must remove — until the
corresponding improvement-plan item lands, the test is expected to assert the
*buggy* behavior does not happen (i.e., it should fail against current `main`
and pass once the fix in the audit is applied). Cases marked "new coverage"
exercise paths the audit found untested but did not find broken.

## Harness & fixtures

All cases build on the existing per-module fixture pattern (`test_config()` +
hand-inserted rows), not the full ingest pipeline, so each primitive is
exercised in isolation with deterministic inputs.

- **Store fixture** — `crate::memory::retrieval::test_support` already
  provides `test_config()` (tempdir-backed `MemoryConfig`), `fixed_ts()`,
  `sample_chunk[_at]`, `insert_chunks`, `insert_score`, `insert_tree_row`,
  `insert_summary`, `set_summary_embedding`, `index_entity_occurrence`,
  `source_tree`, and `summary_node`. Extend it (do not fork it) with:
  - `insert_score_at(cfg, chunk_id, total, dropped)` — needed for RS-13
    (kept→dropped re-score) cases; current `insert_score` hardcodes
    `dropped: false`.
  - `index_entity_occurrence_at` variants that accept an explicit `node_kind`
    of `"summary"` so graph/topic tests can seed both chunk- and
    summary-level occurrences without duplicating `CanonicalEntity` wiring.
  - A `bulk_index_entity_occurrences(cfg, entity_id, node_ids: &[(&str, i64)])`
    helper for RS-2/RS-4 cases that need >200/>500 rows without 200 call
    sites per test.
- **Fake LLM extractor** — `score/mod_tests.rs` already defines `FakeLlm` and
  a `FailingLlm` implementing `extract::EntityExtractor`. Reuse this pattern;
  add a `DefaultingLlm` that returns `Ok(ExtractedEntities::default())` (the
  actual behavior of `LlmEntityExtractor::extract` on a provider error) and an
  `OmitsImportanceLlm` that returns `Ok(ExtractedEntities { llm_importance:
  None, .. })` to model a successful-but-incomplete response — these are the
  two real-world shapes RS-1 needs, as opposed to the `Err`-returning
  `FailingLlm` the current test wrongly treats as the realistic case.
- **Fake embedder** — `score::embed::Embedder` trait; a zero-vector /
  fixed-vector stub and a `FailingEmbedder` (always `Err`) are needed for the
  rerank fallback-to-incoming-order cases. Add to `retrieval/test_support.rs`
  alongside the other fakes, mirroring `mmr_tests.rs`'s inline `make_vec`
  style for vector literals.
- **SQL-shape assertions** — `search.rs` already isolates
  `build_sql_and_params` so the generated statement is unit-testable without a
  DB; follow this pattern for the RS-2/RS-5 fixes (a `build_topic_sql`/
  `build_window_sql` helper) so the "filter pushed into WHERE, not applied in
  Rust after LIMIT" contract can be asserted without needing 201+ row
  fixtures for every case (though at least one true end-to-end row-count case
  per finding is still required — see table).
- **Crash/interleaving helpers** — not required for this subsystem's cases;
  all scoring/retrieval/graph/entity operations here are synchronous
  read-modify-write over one SQLite connection or one file. The one exception
  (RS-14, hand-edited file) is a pure function over `&str` (`parse`,
  `extract_notes`, `compose`) plus one `put_entity` round trip through a
  tempdir — no crash injection needed, only a temp file written with a
  specific missing-newline byte sequence.
- **Time control** — use `test_support::fixed_ts()` and explicit
  `chrono::TimeZone::timestamp_millis_opt` offsets for window-boundary cases;
  never `Utc::now()` in an assertion (already the convention in
  `mod_tests.rs`/`global_tests.rs`).

## Test cases

| ID | Name | Given / When / Then | Findings | Priority |
|----|------|----------------------|----------|----------|
| RS-T01 | `llm_ok_default_gates_cheap_only` | Given a `DefaultingLlm` (models a soft-failed provider call), when `score_chunk` runs on a borderline chunk, then `total` equals `combine_cheap_only`, not the LLM-weighted `combine`, and `llm_consulted`/signals reflect no usable importance. | RS-1 | P0 |
| RS-T02 | `llm_ok_missing_importance_gates_cheap_only` | Given `OmitsImportanceLlm` returns `Ok` with `llm_importance: None`, when scored, then the full `combine` (weight 2.0 denominator) is **not** applied and total matches cheap-only. | RS-1 | P0 |
| RS-T03 | `borderline_chunk_survives_llm_outage` | Given a chunk whose cheap total is 0.36 (just above 0.3 drop threshold) and a `DefaultingLlm`, when scored, then `kept == true` — the audit's exact "0.36 → 0.295 → dropped" regression must not reproduce. | RS-1 | P0 |
| RS-T04 | `llm_full_combine_only_when_importance_present` | Given a `FakeLlm` returning `Some(importance)`, when scored, then `total` matches `combine` (full, LLM-weighted), confirming the gate doesn't also suppress the legitimate success path. | RS-1 | P0 |
| RS-T05 | `real_llm_extractor_never_returns_err` | Given the production `LlmEntityExtractor` wired to an HTTP stub that returns a 500, when `.extract()` is called, then it returns `Ok(ExtractedEntities::default())` (documents the never-`Err` contract so future tests don't reintroduce the `FailingLlm`-only assumption). | RS-1 | P1 |
| RS-T06 | `query_topic_window_beyond_cap_returns_matches` | Given an entity with 250 indexed occurrences (newest 200 outside `[since,until]`, 50 older ones inside it), when `query_topic` runs with that window, then it returns the 50 in-window hits, not zero. | RS-2 | P0 |
| RS-T07 | `query_topic_total_reflects_windowed_count` | Given the same 250-row fixture, when `query_topic` runs with the window, then the reported `total` equals the true in-window count, not the post-truncation-then-filtered count. | RS-2 | P0 |
| RS-T08 | `query_topic_window_within_cap_unaffected` | Given an entity with 50 occurrences all newer than `since_ms`, when `query_topic` runs with a window, then all 50 are returned (regression guard: the fix must not narrow the already-correct case). | RS-2 | P1 |
| RS-T09 | `query_topic_no_window_uses_cap` | Given an entity with 300 occurrences and no window args, when `query_topic` runs, then exactly `TOPIC_LOOKUP_CAP` (200) newest hits are considered, preserving today's no-window behavior. | new coverage | P1 |
| RS-T10 | `lookup_entity_since_until_in_sql_not_rust` | Given `build_topic_sql`/equivalent (or `lookup_entity`'s generated statement) with `since_ms`/`until_ms` set, when inspected, then the WHERE clause contains the bounds and the `LIMIT` is not the first-applied constraint (asserts the SQL shape directly, per `search.rs`'s `build_sql_and_params` pattern). | RS-2 | P1 |
| RS-T11 | `rerank_returns_ranking_score_not_admission_score` | Given two hits with equal stored admission `score` but different query-similarity, when `rerank_by_semantic_similarity` runs and the caller adopts its output ordering *and* score, then `RetrievalHit.score` reflects the computed similarity rank, not the untouched admission score (drives the RS-3 fix contract at the `rerank.rs` boundary). | RS-3 | P0 |
| RS-T12 | `mmr_select_orders_by_query_relevance` | Given two equidistant-from-each-other candidates with different `relevance` computed from two different query vectors, when `mmr_select` is called once per query, then the selection order changes between the two calls (demonstrates `query_vec` is not a no-op once wired; currently the parameter is discarded via `let _ = query_vec;` in `mmr.rs:93`, so this case is expected to fail pre-fix). | RS-3 | P0 |
| RS-T13 | `hybrid_score_has_a_query_path_caller` | Given `hybrid_score`/`keyword_relevance`/`freshness` (scoring.rs) and `mmr_select`, when a full `query_source`/`query_global` call is made with a `WeightProfile` other than the default, then the resulting order differs measurably from pure-cosine rerank order (asserts the composition is actually wired into a retrieval entry point, not just unit-tested in isolation). | RS-3 | P0 |
| RS-T14 | `graph_adapter_never_called_regression_guard` | Given `co_occurring_entities`/`ConfigEntityIndex`, when a retrieval query for an entity-scoped result runs, then `entities_on_node`/`nodes_for_entity` are invoked at least once (a call-counting wrapper around the fake index) — guards against RS-3's "zero callers outside own tests" regressing further once wired. | RS-3 | P1 |
| RS-T15 | `keyword_relevance_empty_query_scores_zero` | Given an empty query string, when `keyword_relevance` runs, then it returns `0.0` (existing behavior, pin as regression guard before wiring). | new coverage | P2 |
| RS-T16 | `freshness_future_timestamp_clamps_to_one` | Given `updated_at_ms > now_ms` (clock skew), when `freshness` runs, then it returns `1.0`. | new coverage | P2 |
| RS-T17 | `freshness_zero_half_life_disables_decay` | Given `half_life_days <= 0.0`, when `freshness` runs at any age, then it returns `1.0`. | new coverage | P2 |
| RS-T18 | `co_occurring_entities_uses_self_join_not_n_plus_one` | Given 50 subject nodes for an entity, when `co_occurring_entities` runs against a query-counting connection wrapper, then the number of SQL statements issued is `O(1)` (or a small constant), not `1 + 50` (asserts the RS-4 fix; expected to fail pre-fix against the current per-node `entities_on_node` loop in `graph/query.rs:48-62`). | RS-4 | P0 |
| RS-T19 | `co_occurrence_weight_beyond_lookup_limit_not_truncated` | Given an entity with 600 co-occurring node hits (100 beyond `OCCURRENCE_LOOKUP_LIMIT = 500`), when edge weights are derived, then the strongest-neighbor ordering matches the true full-history weight, not the newest-500-only weight (expected to fail pre-fix). | RS-4 | P0 |
| RS-T20 | `entities_on_node_batched_for_subject_set` | Given `ConfigEntityIndex::entities_on_node` called for a batch of subject node ids, when the adapter's SQL-shape is inspected post-fix, then it issues one `IN (...)` query rather than one query per node id. | RS-4 | P1 |
| RS-T21 | `query_source_pushes_window_into_sql` | Given a source tree with 5,000 summaries spanning 2 years and a "last 7 days, limit 10" query, when `query_source` runs against a query-counting/row-counting connection wrapper, then the number of summary rows read from SQLite is bounded near the windowed count, not the full 5,000 (expected to fail pre-fix per RS-5; mirrors the existing `list_summaries_in_window` path used by `cover.rs`). | RS-5 | P0 |
| RS-T22 | `query_source_hydrates_embeddings_only_for_survivors` | Given the same fixture, when `query_source` runs, then sidecar embedding hydration is invoked only for the post-window, post-truncate candidate set, not for every non-deleted summary in the tree (row-count assertion on the embedding-hydration call). | RS-5 | P0 |
| RS-T23 | `query_global_pushes_window_into_sql` | Given the same large-history fixture across multiple trees, when `query_global` runs with a window, then per-tree summary reads are bounded by the window, matching `query_source`'s fixed behavior. | RS-5 | P0 |
| RS-T24 | `query_source_uses_list_summaries_in_window_helper` | Given `collect_source_hits` post-fix, when compared to `cover.rs:141`'s existing `list_summaries_in_window` caller, then both paths share the same underlying windowed-fetch helper (no duplicated hand-rolled window logic). | RS-5 | P1 |
| RS-T25 | `search_entities_escapes_percent_wildcard` | Given an entity indexed with canonical id containing a literal `%`, when `search_entities(config, "100%", ...)` is called, then only rows containing the literal substring `100%` match — not every row up to `limit` (expected to fail pre-fix; `search.rs:47-50` doesn't escape `%`/`_`). | RS-6 | P0 |
| RS-T26 | `search_entities_escapes_underscore_wildcard` | Given entities `"alice_bot"` and `"alicexbot"` indexed, when searching for `"alice_bot"`, then only `"alice_bot"` matches (currently `_` matches any single character, so `"alicexbot"` would also match). | RS-6 | P0 |
| RS-T27 | `build_sql_and_params_includes_escape_clause` | Given the SQL-shape helper post-fix, when inspected, then the generated `LIKE` predicate includes `ESCAPE '\'` and the bound pattern has `%`/`_`/`\` backslash-escaped. | RS-6 | P1 |
| RS-T28 | `search_entities_blank_query_returns_empty` | Given a whitespace-only query, when `search_entities` runs, then it returns `Ok(vec![])` without touching the DB (existing behavior, regression guard). | new coverage | P2 |
| RS-T29 | `search_entities_kind_filter_narrows_results` | Given entities of two different `EntityKind`s matching the same substring, when `search_entities` is called with `kinds = Some(&[one_kind])`, then only that kind's matches are returned. | new coverage | P2 |
| RS-T30 | `ms_to_utc_saturates_on_i64_min` | Given `since_ms = i64::MIN`, when `ms_to_utc` converts it, then the result is `DateTime::<Utc>::MIN_UTC`, not `Utc::now()` (expected to fail pre-fix per RS-7). | RS-7 | P0 |
| RS-T31 | `ms_to_utc_saturates_on_i64_max` | Given `until_ms = i64::MAX`, when `ms_to_utc` converts it, then the result is `DateTime::<Utc>::MAX_UTC`, not `Utc::now()`. | RS-7 | P0 |
| RS-T32 | `ms_to_utc_normal_value_round_trips` | Given an in-range epoch-millis value, when converted, then it round-trips exactly (regression guard alongside the saturation fix). | RS-7 | P2 |
| RS-T33 | `drill_down_deleted_check_precedes_doc_version_bookkeeping` | Given a doc with two revisions where the newer (winning) revision is soft-deleted and the older revision is not, when `drill_down` walks the tree, then the older, non-deleted revision is still surfaced (expected to fail pre-fix; currently the deleted check at `drill_down.rs:154-167` runs after `emitted_docs`/`max_version_by_doc` bookkeeping, so the whole doc disappears). | RS-8 | P0 |
| RS-T34 | `drill_down_deleted_winning_revision_does_not_suppress_older` | Given the same fixture, when compared to the pre-fix ordering, then no doc id is fully absent from output solely because its winning revision was deleted. | RS-8 | P1 |
| RS-T35 | `drill_down_non_deleted_winning_revision_still_wins` | Given two revisions, both live, when `drill_down` runs, then the newer one wins and the older is suppressed (regression guard: the fix must not disable latest-wins entirely). | new coverage | P1 |
| RS-T36 | `rerank_module_has_test_file` | New test file exists for `rerank.rs` (RS coverage gap — the audit notes zero tests today). This row records the file's creation; concrete behavior is covered by RS-T37..RS-T41. | new coverage | P0 |
| RS-T37 | `rerank_orders_embedded_hits_by_similarity_desc` | Given three hits with distinct embeddings and a query, when reranked, then output order is descending cosine similarity to the query. | new coverage | P1 |
| RS-T38 | `rerank_unembedded_hits_sort_last_preserving_incoming_order` | Given two embedded and two un-embedded hits (in a specific incoming order), when reranked, then the un-embedded pair appears after all embedded hits, in their original relative order — **not** resorted by `time_range_end` (expected to fail pre-fix per RS-9; `rerank.rs:6-7`'s doc comment claims order-preservation but the comparator at line ~55 tie-breaks by `time_range_end` DESC even among the "unranked" branch, since both-unranked falls into the shared `_ =>` arm). | RS-9 | P0 |
| RS-T39 | `rerank_all_embedded_ties_break_by_recency_desc` | Given two hits with identical cosine similarity (e.g. duplicate embeddings) but different `time_range_end`, when reranked, then the more recent one sorts first (documents the *intended* tie-break, scoped to the embedded case only, once RS-9 is resolved). | RS-9 | P1 |
| RS-T40 | `rerank_embed_failure_falls_back_to_incoming_order` | Given a `FailingEmbedder`, when `rerank_by_semantic_similarity` runs, then the returned order is byte-for-byte identical to the input order. | new coverage | P1 |
| RS-T41 | `rerank_dimension_mismatch_treated_as_unranked` | Given a hit whose stored embedding has a different length than the query embedding, when reranked, then that hit is treated as un-embedded (sorts last) rather than causing a panic or wrong similarity. | new coverage | P1 |
| RS-T42 | `cosine_similarity_consolidated_single_impl` | Given the same two anti-correlated (opposite-direction) vectors, when passed to whichever single `cosine_similarity` remains post-consolidation, then callers requiring `[-1,1]` (e.g. `score/embed.rs`'s raw semantics) and callers requiring `[0,1]` (e.g. MMR) get behavior appropriate to their contract via one shared implementation with an explicit clamp parameter/wrapper — not two diverging functions (expected to fail pre-fix: today `score/embed.rs::cosine_similarity` returns a negative value for anti-correlated vectors while `store/vectors/store.rs::cosine_similarity` clamps to `0.0`). | RS-10 | P0 |
| RS-T43 | `mmr_zero_vector_candidate_does_not_panic_or_nan` | Given a candidate embedding of all zeros, when `mmr_select` runs, then similarity to it is `0.0` (not `NaN`) and the candidate is still selectable/scored deterministically. | RS-10 | P0 |
| RS-T44 | `mmr_distinguishes_anti_correlated_from_orthogonal` | Given one candidate orthogonal to a selected item and another anti-correlated (opposite direction) to it, when MMR runs post-RS-10-fix, then the anti-correlated candidate is penalized less than or differently from the orthogonal one (today both clamp-collapse to the same `0.0` similarity via the vectors-store impl, so this is expected to fail pre-fix). | RS-10 | P0 |
| RS-T45 | `mmr_empty_query_vector_handled` | Given an empty `query_vec` slice, when `mmr_select` runs (query_vec currently unused, but must not panic once wired per RS-3/RS-T12), then it returns a result without panicking. | RS-3 | P2 |
| RS-T46 | `cover_truncation_is_not_alphabetical_by_tree_scope` | Given 3 source trees `("zzz", "aaa", "mmm")` each contributing chunks, with `total > limit`, when the window-cover truncates, then surviving chunks are not simply "first N alphabetically by tree_scope" — some representation from each scope should survive proportionally (or the truncation policy is at least documented and tested as intentional) (expected to fail pre-fix per RS-11's "whole sources dropped" note if the fix changes the policy; if the improvement plan keeps alphabetical truncation but adds an indicator, adjust this case to assert the indicator instead — see RS-T48). | RS-11 | P0 |
| RS-T47 | `cover_max_window_chunks_truncation_has_distinct_indicator` | Given more than `MAX_WINDOW_CHUNKS` (5,000) chunks in the window, when `cover.rs` truncates at the 5,000 internal cap (separate from the caller's `limit`), then the returned result flags this distinctly from ordinary `limit`-based truncation (expected to fail pre-fix; today both collapse into the same `truncated` bool). | RS-11 | P0 |
| RS-T48 | `cover_limit_truncation_below_max_window_chunks_flagged` | Given `total > limit` but `total < MAX_WINDOW_CHUNKS`, when cover runs, then `truncated == true` with the ordinary (non-`MAX_WINDOW_CHUNKS`) reason — regression guard distinguishing the two truncation paths introduced by RS-T47. | RS-11 | P1 |
| RS-T49 | `cover_source_starvation_one_scope_dominates` | Given one tree_scope with 4,900 chunks and another with 200, all within the window, and `limit = 100`, when cover truncates, then the case documents whether/how the second scope is starved (asserts current or fixed policy explicitly rather than leaving it unspecified). | RS-11 | P1 |
| RS-T50 | `regex_extractor_trims_trailing_punctuation_from_handle` | Given the text `"ping @alice."`, when the regex entity extractor runs, then the extracted handle canonical id is `handle:alice`, not `handle:alice.` (expected to fail pre-fix per RS-12; `score/extract/regex.rs:32`). | RS-12 | P0 |
| RS-T51 | `regex_extractor_trims_trailing_punctuation_variants` | Given handles/mentions followed by `,`, `;`, `!`, `?`, or a closing paren, when extracted, then trailing punctuation is stripped from each. | RS-12 | P1 |
| RS-T52 | `regex_extractor_preserves_internal_punctuation` | Given `"@alice.smith"` (a dot inside the handle, not trailing), when extracted, then the full `alice.smith` is preserved (regression guard: trimming must be trailing-only). | RS-12 | P1 |
| RS-T53 | `regex_extractor_two_mentions_same_person_co_occur` | Given `"ping @alice and @alice."` (bare and punctuated) in one chunk, when extracted post-fix, then both mentions canonicalize to the same `handle:alice` id and contribute to one co-occurrence weight, not two split entities (direct RS-12 co-occurrence-splitting scenario from the audit). | RS-12 | P0 |
| RS-T54 | `kept_to_dropped_rescore_clears_entity_index` | Given a chunk previously scored `kept = true` with entity-index rows written, when it is rescored and now evaluates `kept = false`, then `clear_entity_index_for_node` still runs and the entity-index rows for that chunk are removed (expected to fail pre-fix per RS-13; `score/mod.rs:337` only clears `if result.kept`). | RS-13 | P0 |
| RS-T55 | `phantom_entity_rows_do_not_surface_in_topic_query` | Given the same kept→dropped fixture without the RS-13 fix applied, when `query_topic` runs for that entity, then the dropped chunk's phantom occurrence does not appear in results (post-fix acceptance test; pre-fix this documents the visible bug). | RS-13 | P0 |
| RS-T56 | `resolve_topic_hits_excludes_dropped_scores` | Given a node whose score row has `dropped = true`, when `resolve_topic_hits` runs, then that node is excluded from topic results even if an entity-index row still references it (expected to fail pre-fix; `global.rs:157-163` has no dropped-status check). | RS-13 | P0 |
| RS-T57 | `resolve_topic_hits_excludes_soft_deleted_summaries` | Given a summary node with `deleted = true` still referenced by the entity index, when `resolve_topic_hits`/graph reads run, then it is excluded (graph reads currently never filter soft-deleted summaries per RS-13). | RS-13 | P0 |
| RS-T58 | `entity_index_config_cap_interaction_real_sqlite` | Given a real (non-mocked) `MemoryConfig`-backed SQLite store with an entity indexed on 600 nodes, when `ConfigEntityIndex::nodes_for_entity` is called, then it returns at most `OCCURRENCE_LOOKUP_LIMIT` (500) node ids, exercising the cap against real SQLite rather than a fake (RS coverage gap: "barely tested against real SQLite"). | new coverage | P1 |
| RS-T59 | `entity_index_config_excludes_deleted_nodes_real_sqlite` | Given the same real-SQLite fixture with some occurrence nodes marked deleted, when `ConfigEntityIndex` reads run, then deleted nodes are excluded from both `nodes_for_entity` and `entities_on_node`. | RS-13 | P1 |
| RS-T60 | `frontmatter_missing_final_newline_parses_notes_not_empty` | Given a hand-edited entity file whose content ends in `"---"` with no trailing `\n` after the closing fence, when `extract_notes`/`parse` run, then the notes body is recognized (non-empty when the source had notes) rather than silently returning `""` (expected to fail pre-fix per RS-14; `split_front_matter`'s `find("\n---\n")` requires a trailing newline after the closing fence). | RS-14 | P0 |
| RS-T61 | `put_entity_refuses_overwrite_on_unparseable_nonempty_file` | Given an on-disk entity file that fails `parse()` (e.g. malformed fence) but has non-empty content, when `put_entity` is called, then it either preserves the existing body or returns an error — it must not silently rewrite the file with an empty notes body (expected to fail pre-fix; documents whichever remediation the improvement plan picks: accept-EOF-fence vs refuse-to-overwrite). | RS-14 | P0 |
| RS-T62 | `frontmatter_round_trip_with_trailing_newline_unaffected` | Given a normally-terminated entity file (fence + `\n`), when parsed and re-composed, then the notes body round-trips byte-for-byte (regression guard alongside the RS-14 fix). | RS-14 | P1 |
| RS-T63 | `frontmatter_compose_always_ends_in_single_newline` | Given notes with and without a trailing newline, when `compose` runs, then the output ends in exactly one `\n` in both cases (existing behavior, pin as guard). | new coverage | P2 |
| RS-T64 | `frontmatter_handles_list_round_trips` | Given an entity with multiple `EntityHandle { kind, value }` pairs, when composed then parsed, then all handles round-trip in order, including a handle value containing a colon (forces the `yaml_string` quoting path). | new coverage | P1 |
| RS-T65 | `frontmatter_alias_containing_yaml_metachar_round_trips` | Given an alias containing `:`, `#`, or a literal `"`, when composed then parsed, then `yaml_string`/`unquote` round-trip the value exactly. | new coverage | P1 |

## Priority summary

- **P0** (regression for a Major/Minor finding with a concrete failure
  scenario): RS-T01–04, RS-T06–07, RS-T11–13, RS-T18–19, RS-T21–23, RS-T25–26,
  RS-T30–31, RS-T33, RS-T36, RS-T38, RS-T42–44, RS-T46–47, RS-T50, RS-T53–58,
  RS-T60–61. (37 cases)
- **P1** (major gap / secondary angle on a finding, or a first-class new-coverage
  item called out by the audit's gap list): RS-T05, RS-T08, RS-T10, RS-T14,
  RS-T20, RS-T24, RS-T27, RS-T34–35, RS-T37, RS-T39–41, RS-T48–49, RS-T51–52,
  RS-T58–59, RS-T62, RS-T64–65. (23 cases)
- **P2** (nice-to-have / pins existing-and-correct behavior as a guard):
  RS-T09, RS-T15–17, RS-T28–29, RS-T32, RS-T45, RS-T63. (8 cases, folded into
  the totals above where a P1/P2 split exists per row — see table for the
  authoritative per-case tag)

Total: **65 test cases**.

## Not in scope

- **Ingest/queue mechanics** (chunking, dedup, crash-safe write paths, host-FS
  I/O error classification) — covered by the queue-ingest spec derived from
  `docs/spec/audit/04-queue-ingest.md`.
- **Store/chunks persistence layer** (schema, migrations, vector BLOB packing
  correctness in isolation from scoring/retrieval callers) — covered by the
  store-chunks spec derived from `docs/spec/audit/01-store-chunks.md`. This
  document only tests `cosine_similarity` and vector plumbing at the point
  they're consumed by MMR/rerank (RS-10), not the storage layer itself.
- **Tree/archivist/conversation summarization** (sealing, tree-kind
  transitions, conversation boundary detection) — covered by
  `docs/spec/audit/03-tree-archivist-conversations.md`'s spec. This document
  only touches `tree::store` insofar as retrieval fixtures need seeded summary
  nodes.
- **Sources/goals/tool-memory diff behavior** and **contracts/config/API
  surface** — out of scope per `docs/spec/audit/05-*` and `06-*`; this
  document does not test `WeightProfile` config *parsing*, only its
  consumption once parsed (RS-3/RS-T13).
- **Performance/load testing** of the N+1 and O(total) fixes (RS-4, RS-5) —
  this spec asserts *shape* (query count, row count) with fixtures sized
  large enough to distinguish `O(1)`/`O(window)` from `O(n)`/`O(total)`, but
  does not benchmark wall-clock time or test at production scale.
- **LLM provider integration** (actual network calls to a real LLM API) — RS-1
  cases use fakes; provider-specific retry/timeout behavior belongs to
  whatever module owns `LlmEntityExtractor`'s HTTP client, if a separate spec
  exists for it.
