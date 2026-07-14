# Audit 07 — Modularity & Boundaries (Unix-philosophy decomposition)

_Audit date: 2026-07-14 · Baseline: `main` @ `1a3fcc5` · Full suite green
(`cargo check --all-features` + `cargo test`) at time of audit._

Scope: how well `src/memory/` decomposes into small, single-purpose,
**independently verifiable building blocks**, judged against the crate's own
layering docs (`src/memory/mod.rs:24-47`) and the repo guidelines (≤500 LOC
per file, per-module `types.rs`). This audit is about architecture, not bugs —
correctness defects live in audits 01–06. IDs are `MB-*`.

## Verdict in one paragraph

The coarse layering invariant holds: storage never depends upward on
orchestration, and the module graph is acyclic. But the Unix goal — each block
testable through a narrow published contract — is **not** met at the storage
tier. In practice there is one shared SQLite database (`chunks.db`) whose
connection and 15-table schema are owned by `chunks/`, and `tree`, `queue`,
`score`, `graph`, `retrieval`, and `sync` all bind directly to it. Module
boundaries exist in the source tree but not in persistence or in import
discipline: consumers import functions from deep inside sibling modules rather
than through facades. Separately, `store/{kv,vectors,entity_index}` form a
second, cleanly-decomposed persistence stack that the live pipeline largely
does not use.

## Findings

### MB-1 (High). `chunks` is a god-gateway: it owns the connection and the schema for five other subsystems

`src/memory/chunks/connection.rs` (`get_or_init_connection`, `with_connection`,
process-wide `CONN_CACHE`, circuit breaker) and `src/memory/chunks/schema.rs`
(single `SCHEMA` DDL string) define **15 tables**, most of them belonging to
other modules: `mem_tree_score`, `mem_tree_entity_index`,
`mem_tree_entity_edges` (graph), `mem_tree_trees` / `mem_tree_summaries` /
`mem_tree_buffers` (tree), `mem_tree_jobs` (queue), `mem_tree_ingested_sources`
(ingest). The schema file says so itself (`schema.rs:5-9,33-38`: tables "are
created so future modules can share the same database file").

Consumers of `chunks::with_connection` by module: chunks (~44 sites),
tree (~33), queue (~18), score (~12), retrieval (4), graph (4), sync (3).
`mod.rs:11` bills `chunks` as "canonical chunk model and deterministic ids" —
in reality it is the database substrate for the whole live engine.

**Consequence:** no subsystem that persists anything can be built, tested, or
reasoned about without `chunks`; a schema change for any subsystem is a change
to `chunks`; the "storage primitives" layer (`store`) is bypassed.

**Direction:** make the shared-DB decision explicit and give it a name. Extract
a `db` (or `storage`) module that owns the connection cache, pragmas, circuit
breaker, and a *composed* schema, where each subsystem contributes its own DDL
fragment and table ownership is documented per module. `chunks` then shrinks
back to what its docs claim: the chunk model. (Whether to later split into
per-subsystem DB files is a separate decision; naming the owner is the
prerequisite either way.)

### MB-2 (High). Sibling modules bind to each other's deep internals, not facades

Modules import functions from 2+ levels inside a *sibling*, so no consumer can
be tested against a narrow contract:

- **retrieval → tree internals:** `retrieval/cover.rs:26`
  (`tree::store::{get_tree_by_scope, list_summaries_in_window}`),
  `drill_down.rs:35`, `fast.rs:12`, `global.rs:23`, `source.rs:34`.
- **retrieval → score internals:** `fast.rs:10-11`, `global.rs:21-22`,
  `fetch.rs:18`, `graph_adapter.rs:29`, `rerank.rs:19`, `search.rs:29`.
- **tree → score internals:** `bucket_seal.rs:33-36` pulls `score::embed`,
  `score::extract`, `score::resolver`, and calls
  `score::store::index_summary_entity_ids_tx` *inside tree's own seal
  transaction*; also `direct_ingest.rs:8`, `hydrate.rs:12`.
- **tree → store::content internals:** `bucket_seal.rs:37`,
  `store/summaries.rs:19`.
- **archivist → store::content::atomic:** `archivist/store.rs:25`
  (`write_if_new`), `tree_writer.rs:22` (`sha256_hex`) — utility helpers that
  per `mod.rs:35-38` belong in `fsutil`.
- **goals → store::safety:** `goals/types.rs:17` reaches safety internals from
  a types file.

The measurable cost: `retrieval/test_support.rs:16-20` has to import the score
*and* tree stores just to test retrieval; `retrieval`, `tree`, and `score`
form one entangled cluster that only verifies as a whole.

**Direction:** each module exposes a facade (`pub use` surface in its
`mod.rs`) and siblings import only that. Enforce mechanically — a small
`tests/boundaries.rs` that greps for `use crate::memory::<sibling>::<submodule>::`
patterns (or clippy `disallowed_*` config) turns the layering doc into a
checked invariant instead of prose.

### MB-3 (High). Two parallel persistence stacks; one is mostly disconnected

`store/kv.rs`, `store/vectors/`, and `store/entity_index/` each own their own
SQLite database with their own `open`/`open_in_memory` (kv.rs:56, 
vectors/store.rs:96, entity_index/store.rs:93) — these are exactly the small,
self-contained, independently-testable blocks the project wants. But they
duplicate tables that also exist in `chunks.db`
(`mem_tree_chunk_embeddings`, `mem_tree_entity_index`, `mem_tree_score`), and
the live tree/retrieval path never touches them — `retrieval/mod.rs:50` notes
the `ConfigEntityIndex` adapter is "currently unused by any". Meanwhile
`store::content` (the markdown vault) *does* sit on the live path but imports
the chunk model from `chunks` (`store/content/mod.rs:34`,
`store/content/read.rs:10`), inverting the documented peer relationship.

**Consequence:** two implementations of vectors/entity-index/score persistence
to keep correct, and it is genuinely unclear which one a new feature should
build on. This overlaps CT-4/CT-5 (two store contracts) from audit 06 — the
duplication exists at the data layer too, not just the trait layer.

**Direction:** pick one. Either wire the `store/*` primitives in as the
backends for the live tables (the configurable-store spec pulls this way), or
mark them explicitly as reference implementations and delete the unused
duplicates. Keeping both unlabeled is the worst option.

### MB-4 (Medium). The tree subsystem contains two parallel stacks

`tree/` and `tree/runtime/` each have their own `Summariser` trait
(`tree/summarise.rs` vs `tree/runtime/engine.rs:29` — same name, different
contract), their own store subtree (`tree/store/` vs `tree/runtime/store/`),
and their own atomic-write path (`tree/runtime/engine.rs:228-236`). Persisted
node/summary data overlaps. A reader cannot tell which stack is canonical
without archaeology, and every tree feature must decide which of the two to
extend.

**Direction:** consolidate on one stack (or document `runtime` as a distinct
product surface with a distinct name), and merge the two `Summariser` traits —
two traits with the same name in the same subsystem is a naming hazard by
itself.

### MB-5 (Medium). Oversized files against the repo's own 500-LOC rule

Non-test violators at baseline, with natural seams:

| File | LOC | Natural split |
| --- | --- | --- |
| `store/safety/pii.rs` | 1113 | ~20 independent detectors → one file per family (`phone`, `us`, `eu`, `br`, `in`, `jp_kr`) + shared `Hit`/`NormalizedView` core + tiny facade (`redact_pii`/`has_likely_pii`). Each detector is a textbook standalone unit (`&str -> Sanitized`). |
| `tree/bucket_seal.rs` | 856 | (a) public append/seal API, (b) `SealServices`/`LabelStrategy`/`LeafRef` types → `tree/types.rs` per repo rule, (c) private batching helpers → `tree/batch.rs`. |
| `diff/ledger.rs` | 640 | 27 items: record types vs. read/write ops (already flagged CT-8). |
| `chunks/store.rs` | 572 | read path vs. write path (delete/sources already split out). |
| `queue/types.rs` | 538 | job model (`Job`, `NewJob`, `JobStatus`) vs. per-handler payload structs. |
| `tree/runtime/engine.rs` | 526 | pure helpers (`hour_id_from_buffer_filename`, `derive_node_ids_from_hour_id`, `collect_hour_leaves_recursive`, `floor_char_boundary`) → `tree/runtime/hour_ids.rs`. |
| `sync/composio/providers/slack.rs` | 520 | Slack parsing vs. pipeline impl. |

Near-limit files to watch: `chunks/connection.rs` (498), `sync/rebuild.rs`
(488), `store/vectors/store.rs` (486), `chunks/embeddings.rs` (480),
`sync/workspace.rs` (479).

Also a types-placement violation: `bucket_seal.rs:70-143` defines public types
(`SealServices`, `LabelStrategy`) outside `tree/types.rs`.

### MB-6 (Low). Intra-module cycle in `queue`

`queue/ops.rs:9` imports `queue::handlers::QueueDelegates` while
`queue/handlers.rs:30` imports `queue::ops::set_backfill_in_progress` —
handlers ⇄ ops mutually reference. Not a build problem (same module), but a
sign the handler/ops seam is drawn in the wrong place.

### MB-7 (Low). Modules that already are good building blocks (keep as exemplars)

Worth naming so refactors converge on this shape:

- `fsutil` — leaf utility, zero memory-module imports, own tests.
- `store/safety/pii` — pure `&str -> Sanitized` (oversized per MB-5, but
  contract-clean).
- `sources` — depends only on `config`/`error` + own types/validation.
- `entities` — self-contained markdown store; only external dep is `config`.
- `graph` — one binding away from standalone (`edge_store.rs:8` uses
  `chunks::with_connection`; give it an injected connection and it detaches).
- `store/kv`, `store/vectors`, `store/entity_index` — self-owned DBs with
  in-memory constructors (fate decided by MB-3).

## Target picture

A block qualifies as "trustworthy and verifiable" when: (1) its public
contract is its module facade; (2) its persistence is either self-owned or
obtained through an injected connection/handle, never a sibling's global;
(3) it has sibling `_tests.rs` coverage runnable without constructing other
subsystems; (4) it stays under the 500-LOC file rule. Today `fsutil`,
`sources`, `entities`, `pii`, and the three `store/*` primitives qualify or
nearly qualify; `tree`/`score`/`retrieval`/`queue` fail (2) and (3) via
`chunks::with_connection`, which MB-1's extraction is the single highest-leverage
fix for.
