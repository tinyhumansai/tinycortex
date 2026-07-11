# Audit 03 — Summary Trees, Archivist, Conversations (`src/memory/tree/`, `archivist/`, `conversations/`)

Verified findings, most severe first. IDs `TR-*` are referenced from the
[improvement plan](../improvement-plan.md).

## Critical

### TR-1. Non-atomic seal: `clear_buffer_tx` wipes items appended after the buffer snapshot
`src/memory/tree/bucket_seal.rs:214,257,291,363` + `buffers.rs:60-62`

`seal_one_level` reads the buffer, then hydrates and awaits the summariser (an
arbitrarily long LLM call, no lock held), and only then opens the transaction
that inserts the summary and **unconditionally clears the whole
`(tree_id, level)` buffer** (upserting `Buffer::empty`). Scenario: task A
snapshots L0 buffer `{c1,c2}`; while A awaits the summariser, task B commits
`append_to_buffer(c3)`; A's seal transaction overwrites the buffer with empty —
`c3` is never summarised and is gone from the tree forever. The same window
lets two concurrent cascades (e.g. `flush_stale_buffers` racing a live
`append_leaf`) both seal the same buffer, producing duplicate summaries over
the same children (the `parent_id IS NULL` backlink guard at `:351`/`:357`
silently masks the duplicate).

**Fix:** inside the seal transaction, re-read the buffer, verify it still
starts with the snapshotted `item_ids` (abort/retry otherwise), and remove only
those ids (set-difference) instead of clearing.

### TR-2. `rebuild_tree` deletes the entire tree on disk while the only copy of the leaves is in RAM
`src/memory/tree/runtime/engine.rs:155-168` + `scan.rs:96-104`

`store::delete_tree` removes the whole tree dir, then hour leaves are rewritten
from the in-memory vec. A crash between the delete and the rewrite completing
permanently destroys every summary in the namespace. Additionally, a crash
between the buffer rename to `tree_buffer_backup` and the restore leaves a
backup dir no later code path ever restores — buffered content orphaned.

**Fix:** rebuild into a sibling temp dir and atomically rename over the old
tree; adopt a leftover `tree_buffer_backup` on startup.

## Major

### TR-3. `force_flush_tree`/`TreeFactory::seal_now` with `now = None` silently force nothing
`bucket_seal.rs:215`, `flush.rs:70-81`, `factory.rs:126-134`

`cascade_all_from` treats `force_now.is_some()` as the force flag;
`seal_now` passes `None`, so the documented "force-seal one tree's L0 buffer
now (e.g. user disconnected)" is a no-op for any under-budget buffer — exactly
the disconnect case it exists for. Tests only exercise `Some(now)` or an
already-empty buffer. (The timestamp value is never used — see TR-12.)

**Fix:** make `cascade_all_from` take a `force: bool`; `force_flush_tree`
always forces.

### TR-4. One poison buffer aborts the entire flush pass, permanently
`flush.rs:48-50` + `bucket_seal.rs:258-264` + `buffers.rs:75`

`flush_stale_buffers` uses `?` per cascade; `seal_one_level` bails when
hydration yields zero inputs (all chunk ids missing — deleted chunks or
archivist synthetic leaves). Such a buffer is stale forever, sorts first
(`oldest_at ASC`), and every future flush pass errors out before reaching any
healthy tree. Recall silently degrades engine-wide.

**Fix:** continue past per-tree failures (collect errors); clear/quarantine
buffers whose items are all unhydratable.

### TR-5. Archivist leaves can never seal; the promised tree-backed sink does not exist
`archivist/mod.rs:43`, `tree_writer.rs:11-16,80-87`, `hydrate.rs:33-35`

Docs say "a tree-backed implementation lives in the `tree` module" — the only
`impl TreeLeafSink` in the crate is the test-only `RecordingSink`. Archivist
chunk ids (`archivist:<hash>`) are not chunk-store rows, so at seal time
hydration skips them all and the seal bails. Once a real sink is wired, the
first over-budget L0 buffer poisons the tree (append commits, cascade fails)
and via TR-4 stalls all flushes. A designed-in dead end for the module's
purpose.

**Fix:** either persist archivist turns as chunk rows, or teach
`hydrate_leaf_inputs` a second hydration source for archivist ids — decide
before wiring a production sink.

### TR-6. `record_turn` silently drops turns on seq collision
`archivist/store.rs:115-123` + `store/content/atomic.rs:20-21`

`next_seq` scans the directory, then `write_if_new` — which returns
`Ok(false)` without writing when the path exists; the boolean is discarded.
Two concurrent `record_turn` calls compute the same seq; the loser's turn
vanishes while the function returns success.

**Fix:** O_EXCL-create and retry with the next seq on collision, or serialise
via a lock and treat `false` as an error.

### TR-7. Time-tree node writes are not atomic
`runtime/store/nodes.rs:52`

Plain `std::fs::write` for every hour/day/month/year/root node — no
temp-file + rename, in a codebase that already has `write_if_new` doing it
correctly. A crash mid-write leaves a truncated file; `parse_node_markdown`
happily parses the remains (every field defaults, `nodes.rs:181-214`), so the
corruption is silent and gets baked into future re-summarisation and
`rebuild_tree`.

**Fix:** write-to-temp + rename (reuse the atomic helper).

### TR-8. Channel persistence clobbers user-set thread labels on every message
`conversations/bus.rs:281` + `store_index.rs:317-319`

Every channel turn passes `labels: Some(vec!["general"])` to `ensure_thread`,
and the log fold lets `Some` override existing labels. A user's
`update_thread_labels(thread, ["tasks"])` is reset to `["general"]` by the next
message. Side note: one `Upsert` + one `MessageAppended` appended per message,
and the dedup check (`bus.rs:287-292`) re-reads the whole thread per turn —
unbounded log growth and O(n²).

**Fix:** pass `labels: None` on per-turn upserts; only set labels on true
thread creation.

### TR-9. Archived trees accept new leaves and seals — the invariant is enforced nowhere
`store/types.rs:61-67` vs `bucket_seal.rs:136`, `registry.rs:15`

Docs promise "archived trees don't accept new leaves", but `append_leaf`,
`get_or_create_tree`, and `cascade_all_from` never check `tree.status`.

**Fix:** check `TreeStatus` in `append_leaf`/`append_leaf_deferred`; decide
get-or-create semantics for an archived `(kind, scope)`.

## Minor

- **TR-10** `bucket_seal.rs:306` — sealed summaries record `buf.item_ids`
  wholesale even though hydration skipped missing children; `child_ids` claims
  children the summary content/time-range/score exclude. Store only hydrated
  ids.
- **TR-11** `runtime/engine.rs:60-120` — `run_summarization` retry after
  partial propagation failure folds the same buffered entries into hour nodes
  twice; per-node failures fully swallowed (`_e`). Delete buffer entries per
  successfully-written hour leaf.
- **TR-12** `bucket_seal.rs:215` — `force_now` timestamp is dead weight (only
  `is_some()` is tested); the API invites the TR-3 misuse.
- **TR-13** `read.rs:41,61,67-99` vs `io.rs:143-165` — docs promise
  cosine-similarity reranking, code does lowercase token-overlap; `read_tree`
  never checks `start.deleted` or tree membership of the start node; the
  L1→chunk path has no deleted-chunk filter; with multiple nodes at
  `max_level`, `root_id` points at one of them and a root walk silently misses
  sibling subtrees.
- **TR-14** front-matter round-trip corruption: (a)
  `strip_buffer_frontmatter` (`runtime/store/buffer.rs:88-101`) truncates
  user content that merely begins with `---`; (b) `yaml_escape`
  (`archivist/store.rs:100-109`) doesn't escape newlines — a `lesson`
  containing `\n---\n` splices YAML into the body; (c) `write_node` writes
  `metadata`/`node_id` unescaped.
- **TR-15** id-collision-by-concatenation: `bus.rs:329` thread ids
  `{channel}_{sender}_{reply_target}` (`("slack","a_b","c")` ≡
  `("slack","a","b_c")`); same pattern in `paths.rs:29-34` (plus the
  single-pass `replace("__","_")`) and `archivist/store.rs:43-53`. Use
  length-prefixing or hex encoding (as `thread_messages_path` already does).
- **TR-16** `inverted_index.rs:295,391`, `store_index.rs:224-228` — recency
  ranking compares RFC3339 timestamp strings lexicographically; mixed
  `+00:00`/`Z`/non-UTC offsets misorder. Parse to epoch ms at insert.
- **TR-17** `store_ops.rs:127-138` — message append and `MessageAppended`
  stats event are two separate fsync'd appends; a crash between them skews
  `message_count`/`last_message_at` forever (backfill only fires when count is
  `None`).
- **TR-18** `inverted_index.rs:246-252` — query terms that pass the 3-byte
  filter but produce no trigram fall back to a full-corpus candidate list,
  tripping `LARGE_CANDIDATE_LIMIT` and returning score-0.0 recency hits
  indistinguishable from real matches.

## Test-coverage gaps

- **No concurrency tests anywhere in scope**: seal-vs-append (TR-1), double
  cascade, concurrent `record_turn` (TR-6), concurrent conversation writers.
- `seal_now`/`force_flush_tree` with `None` and a non-empty under-budget
  buffer — the exact case exposing TR-3 — untested.
- No poison-buffer flush test (TR-4); no crash/partial-write recovery tests
  for `write_node`, `rebuild_tree`, or the `tree_buffer_backup` orphan path.
- No archivist ↔ real tree integration (impossible today, TR-5).
- No label-interaction test (TR-8), mixed-format timestamps (TR-16), buffer
  content beginning with `---` (TR-14a), or newline-bearing lessons (TR-14b).
