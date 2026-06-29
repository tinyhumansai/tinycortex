# Diff Layer Spec

OpenHuman module: `memory_diff`.

## Responsibility

`memory_diff` tracks how memory sources change over time. It does not fetch
upstream data. It snapshots the already-ingested chunk store, materializes a
derived git ledger, and exposes diffs, checkpoints, and read markers so agents
can understand what changed since a sync or since their last read.

## Source of Truth

`mem_tree_chunks` remains authoritative. The git ledger at
`<workspace>/memory_diff/repo` is rebuildable derived state.

## Git Mapping

- Snapshot: git commit.
- Snapshot id: commit SHA.
- Checkpoint: annotated tag `ckpt_<uuid>`.
- Read marker: `refs/openhuman/read/<source_id>`.
- Diff: git tree diff between commit trees scoped to source path.

Each snapshot commit replaces one source subtree and carries other source
subtrees forward from the parent. All writes must serialize through a process
lock because HEAD and parent resolution are read-modify-write operations.

## Snapshot Model

`Snapshot` fields:

- `id`
- `source_id`
- `source_kind`
- `label`
- `trigger`: `auto` or `manual`
- `item_count`
- `taken_at_ms`

Snapshot metadata rides in commit message trailers. Snapshot content is one
flat blob per source item under `<source_id>/`; item id is encoded into a
git-safe path component.

## Item Diff Model

Change kinds:

- `added`
- `removed`
- `modified`

`ItemChange` includes:

- `item_id`
- `title`
- `kind`
- optional `old_content_hash`
- optional `new_content_hash`
- optional bounded `text_diff`

`DiffSummary` includes added, removed, modified, and unchanged counts.
`DiffResult` includes source metadata, from/to snapshot ids, summary, and
per-item changes. `CrossSourceDiff` aggregates per-source diffs for a
checkpoint.

## Snapshot Capture

Snapshot capture:

1. Build a source id prefix.
2. Query `mem_tree_chunks` matching the prefix.
3. Group chunk content by item id.
4. Order content by source id and sequence.
5. Commit one blob per item into the ledger.
6. Publish `MemoryDiffSnapshotTaken` event.

Manual snapshot uses `SnapshotTrigger::Manual`. Successful source sync should
call `auto_snapshot_after_sync` with `SnapshotTrigger::Auto`.

## Diff Operations

Required business operations:

- `take_snapshot(source, config, trigger)`
- `auto_snapshot_after_sync(source, config)`
- `compute_diff(config, from_snapshot_id, to_snapshot_id, include_text_diff)`
- `diff_since_last(source, config, include_text_diff)`
- `diff_since_read(source, config, include_text_diff, commit)`
- `mark_read(source_ids | all)`
- `create_checkpoint(label)`
- `list_checkpoints(limit)`
- `diff_since_checkpoint(checkpoint_id, include_text_diff)`
- `cleanup(older_than_days)`

Cross-source direct diff must be rejected when from/to snapshots belong to
different sources.

## Read Markers

`diff_since_read` compares a source's head snapshot to its read marker. If
`commit = true`, it advances the marker after computing the diff. Default
behavior acknowledges reads so repeated calls return only newer changes.
`commit = false` previews without acknowledgement.

## Controller Namespace

Namespace: `memory_diff`.

Functions:

- `take_snapshot`
- `list_snapshots`
- `diff`
- `diff_since_last`
- `diff_since_read`
- `mark_read`
- `create_checkpoint`
- `list_checkpoints`
- `diff_since_checkpoint`
- `cleanup`

## Agent Tool

`MemoryDiffTool` exposes in-conversation change awareness. Tool output must
preserve ids, source labels, counts, and item changes. Text diffs should remain
bounded to avoid overwhelming context.

## Required Invariants

- Never re-fetch upstream data to compute a diff.
- Ledger writes are serialized.
- Snapshot item identity is item id, not title.
- Item rename is represented as removed plus added.
- Commit/message metadata must round-trip into `Snapshot`.
- Text diffs are capped.
- Ledger errors must surface as unhealthy diff state, not silent empty results.

## TinyCortex Landing Area

```text
src/memory/diff/
  types.rs
  ledger.rs
  ops.rs
  rpc.rs
  tools.rs
```

Port order: types and serde tests, item-id encoding, snapshot metadata
round-trip, in-memory/git tempdir tests, then chunk-store integration.

