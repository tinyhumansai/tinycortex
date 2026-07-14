# Flavoured memory trees

_Spec for the ask-driven tree kind added in issue
[#68](https://github.com/tinyhumansai/tinycortex/issues/68)._

A **flavoured tree** is a summary tree that is instantiated with a specific
*ask/flavour* — e.g. "tweet writing style", "email tone", "coding preferences" —
and continuously distills everything ingested into it through that lens. The top
of the tree compiles into a single markdown file (≤ 1000 tokens by default) that
a host can drop directly into a prompt as a style guide / preference profile.

It is a *flavoured* variant of the source/topic/global trees: the same
bucket-seal machinery (`src/memory/tree/`), but every summarisation step is
steered by the ask, and the root has a first-class compiled artifact instead of
being just another `SummaryNode`.

## Contract

### Tree kind & scope

- Wire kind: `flavoured` (`TreeKind::Flavoured`, `src/memory/tree/store/types.rs`).
- `scope` encodes the **ask slug** (e.g. `tweet-style`, `coding-prefs`). The
  `(kind, scope)` pair is unique, so one slug maps to one flavoured tree.
- The full natural-language ask (not just the slug) is persisted on the tree row
  in the nullable `mem_tree_trees.ask` column and surfaced as `Tree.ask`.

### Steered summarisation

- `SummaryContext.ask` carries the tree's ask into each seal
  (`seal_one_level_with_services` populates it from the tree row).
- When `ask` is present, `prepare_summary_prompt`
  (`src/memory/tree/summarise.rs`) emits a **flavour-directed** system prompt
  ("You are distilling evidence into a profile that answers this ask: …") instead
  of the generic folding prompt. The prior profile is merged with new evidence at
  every level, so the root stays a running, prescriptive profile.
- No LLM coupling: hosts inject their own `Summariser`; the ask rides along in
  the context. The deterministic `ConcatSummariser` remains the fallback, so a
  flavoured tree still functions (concatenation-with-truncation) without a
  model.

### Construction

```rust
let factory = TreeFactory::flavoured(
    "tweet-style",
    "Distill the author's tweet-writing style: voice, tone, structure, \
     vocabulary, emoji/punctuation habits, and concrete dos and don'ts.",
);
let tree = factory.get_or_create(&config)?;           // stamps the ask on create
factory.insert_leaf(&config, &leaf, &summariser)?;    // per tweet/thread
let style_md = compile_flavoured_root(&config, &tree.id)?; // ≤ budget, prompt-ready
```

`TreeFactory::flavoured(scope, ask)` uses `LabelStrategy::Empty` (the ask, not
seal-time entity extraction, defines the tree). Re-instantiating a factory for an
existing `(Flavoured, scope)` returns the live row and leaves its stored ask
untouched.

### Ingest

No new ingest machinery: leaves arrive via the existing `append_leaf` /
`direct_ingest::ingest_summary`, or via the archivist `TreeLeafSink`. A host
routes flavour-shaped content (or whole conversations) into the flavoured tree's
sink. Cross-tree fan-in and automatic content routing are out of scope for v1.

## Compiled root artifact (the deliverable)

`compile_flavoured_root(config, tree_id) -> Result<String>`:

1. Fetches the tree, resolves its `root_id`, and reads the root `SummaryNode`
   body (empty before the first seal).
2. Clamps the body to `TreeConfig::flavour_root_token_budget` tokens
   (default `FLAVOUR_ROOT_TOKEN_BUDGET = 1000`).
3. Renders YAML front-matter + body and **stages it in place** at a stable path
   so hosts read a fixed location.

### Artifact format

```
---
kind: flavoured_root
tree_id: "flavoured:…"
scope: "tweet-style"
ask: "Distill the author's tweet-writing style: …"
root_id: "summary:…"          # or null before the first seal
sealed_at: "2026-07-14T…Z"    # tree.last_sealed_at, or null
leaves_folded: 42             # root node child count (evidence changelog)
token_estimate: 812
token_budget: 1000
---
<the ≤ budget-token profile body>
```

`ask`, `tree_id`, `scope`, `root_id`, and `sealed_at` are emitted as escaped
one-line YAML scalars. `leaves_folded` / `token_estimate` / `sealed_at` double as
a cache-busting evidence changelog.

### Staged path

- Relative: `flavoured/<scope_slug>.md` under the content root
  (`flavoured_root_rel_path`); absolute via `flavoured_root_abs_path`.
- Written atomically and **overwritten in place** on every recompile, so the
  path is stable across refreshes.
- Unlike per-node summary staging, the compiled root is *not* tracked in SQLite —
  it is a pure projection of the root node, safe to delete and regenerate.

Per-node summaries of a flavoured tree are staged like any other tree under
`wiki/summaries/flavoured-<scope_slug>/L<level>/<id>.md`
(`SummaryTreeKind::Flavoured`).

### Recompile triggers

The artifact is refreshed whenever a seal may have moved the root:
`cascade_all_from_with_services` recompiles after any non-empty cascade on a
flavoured tree, covering `append_leaf`, `flush_stale_buffers`, and
`force_flush_tree` / `TreeFactory::seal_now`. Recompilation is best-effort — a
failed compile logs and never fails the seal.

## Config

| Field | Default | Meaning |
| --- | --- | --- |
| `TreeConfig::flavour_root_token_budget` | `1000` | Token clamp for the compiled root body. |

Flavoured trees accept the global per-level seal budgets (50k in / 5k out) in
v1; per-kind overrides are a future refinement for sparser style evidence.

## Out of scope (v1)

- In-repo LLM provider (the `Summariser` seam stays host-injected).
- The runtime markdown time-tree — flavoured trees are bucket-seal only.
- Automatic content routing/classification into flavoured trees.
- In-engine fan-out subscriptions mirroring leaves across trees.
