---
description: Change awareness over an agent's memory via git-backed snapshots and tree diffs.
---

# Diff Layer

The diff layer gives an agent **change awareness** over its own memory: after a
source syncs, what did its world view gain, lose, or revise? TinyCortex answers
this by snapshotting a source's already-ingested items into a git repository and
diffing those snapshots with git's native tree-diff machinery.

The module is compiled only when the crate's **`git-diff` Cargo feature** is
enabled (it is off by default and pulls in `git2`/libgit2):

```bash
cargo test --features git-diff
```

The module lives under `src/memory/diff/`. The public entry point is
[`DiffEngine`](#diffengine), generic over an injected
[`SnapshotItemSource`](sources.md). Domain types
(`Snapshot`, `ItemChange`, `DiffResult`, `Checkpoint`, `CrossSourceDiff`, …)
are in `src/memory/diff/types.rs`; the git persistence ("the ledger") is in
`src/memory/diff/ledger.rs`.

## Why git

Change tracking is a problem git already solves: content-addressed blobs,
tree-to-tree diffs, named baselines (tags), and lightweight pointers (refs).
The diff layer maps its domain directly onto these primitives instead of
reimplementing them.

| Diff concept | Git primitive | Identifier |
| --- | --- | --- |
| Snapshot | commit | `Snapshot.id` **is** the commit SHA |
| Checkpoint | annotated tag at HEAD | `ckpt_<uuid>` |
| Read marker | ref | `refs/openhuman/read/<encoded_source_id>` → commit SHA |
| Diff | `git diff <from-tree>..<to-tree>` scoped to one source path | — |

The ledger is a libgit2 repository at `<workspace>/memory_diff/repo`
(`Ledger::open` creates it on first use, mirroring `MemoryConfig::workspace`).

## Authoritativeness: the ledger is derived

The diff ledger is **not** a source of truth. The chunk store (whatever backs
the injected item source) stays authoritative for memory content; the ledger is
a *derived, rebuildable* git view used purely for change tracking. This honours
the core TinyCortex invariant that derived indexes can be discarded and rebuilt.

Two consequences:

- Snapshots are built from **already-ingested** data supplied by the item
  source — never by re-calling upstream source readers. Diffs therefore cost
  **zero** upstream API calls.
- The diff engine is decoupled from `chunks` and `sources`. It takes a
  `SnapshotItemSource` by injection (`InMemoryItemSource` is the
  reference/test backend) and callers pass a `SourceDescriptor`
  (`id` / `kind` / `label`) they already hold, rather than the engine reaching
  into a source registry.

## DiffEngine

```text
pub struct DiffEngine<S: SnapshotItemSource> {
    workspace: PathBuf,   // ledger lives at <workspace>/memory_diff/repo
    items: S,             // injected, yields a source's already-ingested items
}

DiffEngine::new(workspace, items) -> Self
```

All operations are synchronous. Every git mutation (commit, tag, ref update)
serialises through a process-global `WRITE_LOCK` inside the `Ledger`, because
libgit2's HEAD/parent resolution is read-modify-write and concurrent commits
could otherwise fork history or lose a snapshot.

### Operations

| Method | Purpose |
| --- | --- |
| `take_snapshot(source, trigger)` | Capture one source's current items as a commit |
| `auto_snapshot_after_sync(source)` | `take_snapshot` with `SnapshotTrigger::Auto` |
| `list_snapshots(source_id, limit)` | Snapshots newest-first, optionally filtered |
| `compute_diff(from, to, include_text_diff)` | Diff an explicit snapshot pair (same source) |
| `diff_since_last(source_id, …)` | Latest snapshot vs the previous one |
| `diff_since_read(source_id, …, commit)` | Latest vs the source's read marker, optionally advancing it |
| `mark_read(source_ids)` | Advance read markers to current heads |
| `create_checkpoint` / `list_checkpoints` | Named cross-source baselines (tags) |
| `diff_since_checkpoint` | Aggregate diff across sources since a checkpoint |
| `cleanup` | Prune old checkpoint tags |

Descriptor-keyed conveniences `diff_since_last_for` and `diff_since_read_for`
take a `SourceDescriptor` for callers that already hold one.

## Snapshots

A `Snapshot` is a point-in-time capture of **one** source's ingested items:

```text
Snapshot {
    id: String,              // commit SHA — the snapshot's stable id
    source_id: String,
    source_kind: String,     // wire string, e.g. "folder", "composio"
    label: String,
    trigger: SnapshotTrigger,// auto | manual
    item_count: u32,
    taken_at_ms: i64,
}
```

`take_snapshot` reads `items_for_source(&source.id)` from the injected source
(grouped by item id and ordered), then `Ledger::commit_snapshot` writes **one
git blob per item** into a subtree named for the source, grafts that subtree
into the root tree carried forward from the parent commit (so HEAD always
reflects the whole world across every source), and commits to `HEAD`. If the
item list is empty, the source's subtree is removed instead.

Metadata with no natural git home rides in the commit message as `Key: value`
**trailers** (`Source-Id`, `Source-Kind`, `Source-Label`, `Trigger`,
`Item-Count`, `Taken-At-Ms`). `snapshot_from_commit` reconstructs the
`Snapshot` by parsing those trailers, falling back to the commit time when the
millisecond trailer is missing and defaulting an unrecognised trigger to
`Auto`. Label values are sanitised to a single line so a multi-line label
cannot corrupt the trailer block.

### `SnapshotTrigger` and `auto_snapshot_after_sync`

| Variant | Wire string | When |
| --- | --- | --- |
| `Auto` | `auto` | Captured automatically after a successful source sync |
| `Manual` | `manual` | Captured on explicit user/agent request, or as checkpoint baselining |

`auto_snapshot_after_sync(source)` is the hook a host calls right after it has
finished ingesting a sync. Because TinyCortex does not own *when* to sync
(OpenHuman or the host decides that — see [Sources](sources.md)), this is the
boundary call that records "the sync landed; here is the new state." It is
exactly `take_snapshot(source, SnapshotTrigger::Auto)`.

## Encoded source and item ids

Ids are turned into git-safe path components by `encode_item_id` /
`encode_source_id` (`encode_source_id` is the same encoding, kept as a named
helper so the source-vs-item boundary is explicit at call sites):

- An `i_` prefix keeps the result clear of the reserved names `.`, `..`, and
  empty.
- Each byte outside `[A-Za-z0-9._-]` becomes `%XX` (uppercase hex).

So an item id `notes/2026.md` encodes to `i_notes%2F2026.md`.
`decode_item_id` is the exact inverse and recovers the original id from a
ledger path component. The tree layout is therefore flat per source:

```text
<repo>/
  i_<encoded_source_id>/
    i_<encoded_item_id>     # one blob = full concatenated item content
    i_<encoded_item_id>
  i_<encoded_other_source>/
    ...
```

**Item identity is the file name, never the title.** A content change keeps the
same blob name → `Modified`; renaming an item id is reported as `Removed` plus
`Added`, matching the id-keyed semantics.

## Diffing

`compute_diff` resolves both snapshots, **rejects cross-source pairs** (`from`
and `to` must share a `source_id`), and delegates to `Ledger::compute_changes`,
which runs `diff_tree_to_tree` with a pathspec scoped to the encoded source id
and 3 context lines. The pathspec match is re-checked with a trailing-slash
prefix (`<encoded_source_id>/`) to guard against prefix overreach (so `src_a`
does not match `src_abc/...`).

Each git delta maps to a `ChangeKind`:

| `git2::Delta` | `ChangeKind` |
| --- | --- |
| `Added` / `Copied` / `Untracked` | `Added` |
| `Deleted` | `Removed` |
| `Modified` / `Renamed` / `Typechange` | `Modified` |
| unmodified / ignored / conflicted | (skipped) |

Each surviving delta becomes an `ItemChange`:

```text
ItemChange {
    item_id: String,                 // decoded from the ledger path
    title: String,                   // first non-empty content line, '#' stripped, ≤120 chars; id fallback
    kind: ChangeKind,                // added | removed | modified
    old_content_hash: Option<String>,// blob oid on the `from` side (None when added)
    new_content_hash: Option<String>,// blob oid on the `to` side  (None when removed)
    text_diff: Option<String>,       // bounded unified patch, modifications only, on request
}
```

Content hashes are the git blob oids (the zero oid on an absent side decodes as
`None`). When `include_text_diff` is set, a `Modified` item also carries a
unified patch rendered from the delta and truncated to `MAX_TEXT_DIFF_CHARS`
(2000 chars, with a `…(truncated)` marker).

The `DiffSummary` aggregates counts. Git only reports *changed* entries, so
`unchanged` is derived as `to_item_count − added − modified` (saturating):

```text
DiffResult {
    source_id, source_kind, source_label,
    from_snapshot_id: Option<String>,  // None = first-ever diff (all added)
    to_snapshot_id: String,
    summary: DiffSummary { added, removed, modified, unchanged },
    changes: Vec<ItemChange>,
}
```

### Convenience diffs

- **`diff_since_last(source_id, …)`** — fetches the two latest snapshots for the
  source. Two → diff previous vs latest; one → the whole source is reported as
  added (`from = None`); zero → error.
- **`diff_since_read(source_id, …, commit)`** — diffs the latest snapshot
  against the source's read marker, i.e. everything that changed since the agent
  last *read* this source's diff. With `commit = true` the marker (a git ref) is
  advanced to the head snapshot after the diff is computed, so the next call
  returns only newer changes; `commit = false` previews without acknowledging.
  A marker pointing at a commit that no longer resolves is treated as unread (a
  full diff).

## Read markers

A read marker is a git ref `refs/openhuman/read/<encoded_source_id>` pointing at
the last snapshot the agent acknowledged for that source.
`get_read_marker` / `set_read_marker` read and advance it.

`mark_read(source_ids)` advances each listed source's marker to its current head
snapshot, skipping sources with no snapshots, and returns how many markers were
set. The caller supplies the ids explicitly — the diff layer does not own the
source registry, so to "mark all read" you pass every enabled source's id.

## Checkpoints and cross-source diffs

A `Checkpoint` is a named, **cross-source** baseline: the latest snapshot per
source at one moment, recorded as an annotated git tag `ckpt_<uuid>` at HEAD.

```text
Checkpoint {
    id: String,           // tag name ckpt_<uuid>
    label: String,
    created_at_ms: i64,
    snapshot_ids: Vec<String>,  // per-source head snapshot ids at checkpoint time
}
```

The label and per-source head ids ride in the tag message as JSON
(`checkpoint_message` / `checkpoint_from_message`). `create_checkpoint`
requires at least one snapshot (HEAD must exist). `list_checkpoints` enumerates
`ckpt_*` tags newest-first.

`diff_since_checkpoint` produces a `CrossSourceDiff` — a per-source diff
aggregated across every source that changed since the checkpoint (unchanged
sources are omitted) plus a summed `DiffSummary`.

## Cleanup

`cleanup` (`cleanup_checkpoints`) deletes checkpoint tags created before a
cutoff and returns the count removed. Snapshot **commits are retained** — git
history *is* the ledger — so cleanup only prunes named baselines, never the
underlying change record.

{% hint style="info" %}
The ledger is fully rebuildable: because snapshots derive from already-ingested
items and never from upstream reads, you can delete `<workspace>/memory_diff/repo`
and regenerate it without touching authoritative memory content.
{% endhint %}

## See also

- [Sources](sources.md)
- [Ingest Pipeline](ingest-pipeline.md)
- [Storage Primitives](storage-primitives.md)
- [Architecture Overview](architecture.md)
