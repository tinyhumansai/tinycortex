# TinyCortex effectiveness harness

A small, backend-agnostic harness that measures **how good** TinyCortex
retrieval is — recall@k, precision@k, MRR, and nDCG@k over labeled datasets.
This complements correctness tests (which prove retrieval doesn't *error*) and
`cargo bench` (which measures *speed*). Corresponds to goal **T3** in
[`docs/plan/03-testing-benchmarks.md`](../../docs/plan/03-testing-benchmarks.md).

It is a standalone dev crate: it path-depends on `tinycortex` and is
intentionally excluded from the published package.

## Run it

```bash
cd benchmarks/effectiveness
cargo run --bin effectiveness
# options: --dataset PATH   (default: data/fixtures_v1.json)
#          --out DIR        (default: results/)
#          --label LABEL    (default: $GIT_SHA or "local")
cargo test    # unit tests for the metrics + dataset validation
```

Output: a summary table on stdout plus a dated JSON report at
`results/<timestamp>-<label>.json` (gitignored) so runs are diffable across
commits.

## Layout

| File | Purpose |
| --- | --- |
| `src/metrics.rs` | Pure ranking metrics: recall@k, precision@k, hit@k, reciprocal rank (MRR), nDCG@k. |
| `src/dataset.rs` | Labeled-dataset format (`Document` + `QueryCase`) with JSON load + validation. |
| `src/backend.rs` | The `BenchBackend` seam + `InMemoryBackend` adapter over `InMemoryMemoryStore`. |
| `src/harness.rs` | Ingest → query → aggregate loop producing a `RunReport`. |
| `src/main.rs` | CLI runner: parse args, run, print, write JSON. |
| `data/fixtures_v1.json` | Seed corpus (10 docs / 12 queries), hand-labeled. |

## Dataset format

```jsonc
{
  "name": "fixtures_v1",
  "description": "...",
  "documents": [
    { "id": "doc-auth", "title": "...", "text": "...", "namespace": "bench" }
  ],
  "queries": [
    { "id": "q-oauth", "query": "oauth token refresh", "relevant_ids": ["doc-auth"] }
  ]
}
```

`relevant_ids` is binary ground truth; every id must reference a document id.
`namespace` defaults to `"bench"`; a query's optional `namespace` scopes its
search (omit to search all).

## Adding a backend

Implement `BenchBackend` (ingest a document; answer a query with a ranked list
of document ids) and hand it to `harness::run`. The seam is deliberately narrow
so an assembled `CortexEngine` (goal C1) or a live-embedding backend
(Ollama `bge-m3`, T3 mode 2) drops in without touching the metrics or dataset
code.

## Status / next steps (T3)

- [x] Metrics: recall@k, precision@k, hit@k, MRR, nDCG@k (unit-tested).
- [x] Labeled-dataset format + validation + seed corpus.
- [x] Runner over the lexical `InMemoryMemoryStore` baseline; dated JSON output.
- [ ] Grow the seed set toward ~50 hand-labeled query/answer pairs; add
      natural-language / paraphrase queries (need an embedding backend — the
      lexical baseline gates on whole-query substring presence).
- [ ] `CortexEngine` backend + per-weight-profile breakdown
      (BALANCED / SEMANTIC / LEXICAL / GRAPH_FIRST).
- [ ] Real-embedding mode (Ollama `bge-m3`); optional LLM-judge groundedness.
- [ ] Compare script that fails when recall@10 regresses > 2pts vs. a baseline.
