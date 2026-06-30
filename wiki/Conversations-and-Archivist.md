# Conversations & Archivist

These two modules sit at the top of the engine and cover the *conversation*
lifecycle:

- **`src/memory/conversations/`** — durable **transcript persistence**. It owns
  the raw thread/message records on disk (JSONL) plus a local inverted index for
  cross-thread substring search. This is storage, **not** semantic indexing.
- **`src/memory/archivist/`** — the bridge from a raw chat conversation into the
  semantic layer. It cleans a conversation (drops tool noise), composes it into a
  single markdown blob, and pushes it into a [summary tree](Summary-Trees) as
  exactly **one leaf**.

Keep the boundary clear: the conversation store is where transcripts *live*; the
archivist is how a transcript becomes *memory*. The richer summary-tree archival
deliberately lives in `archivist`, not in `conversations`.

---

## Transcript storage (`conversations`)

### On-disk layout

Conversations are stored as JSONL under the configured workspace
([`MemoryConfig::workspace`](Core-Concepts)), rooted at
`<workspace>/memory/conversations/`:

```text
<workspace>/memory/conversations/
├── threads.jsonl                 # append-only thread metadata log (upsert/delete/stats)
└── threads/
    ├── <hex(thread_id)>.jsonl     # one file per thread; messages in append order
    └── ...
```

Two filename constants drive this (`src/memory/conversations/store.rs`):

| Constant | Value | Meaning |
| --- | --- | --- |
| `THREADS_FILENAME` | `threads.jsonl` | Append-only thread metadata log |
| `THREAD_MESSAGES_DIR` | `threads` | Subdir of per-thread message logs |

Per-thread message files are named `<hex(thread_id)>.jsonl`. Thread ids are
arbitrary provider strings (e.g. `proactive:morning_briefing`), so the store
lowercase-hex-encodes the id with the local `hex_encode` helper to derive a
filesystem-safe filename. (OpenHuman used the `hex` crate; this crate avoids that
dependency.)

### `threads.jsonl` is an append-only log

Thread metadata is **not** rewritten in place on every change. It is an
append-only log of `ThreadLogEntry` records (`#[serde(tag = "op")]`), folded into
current thread state on read by `thread_index_unlocked`:

| `op` wire string | Variant | Effect when folded |
| --- | --- | --- |
| `upsert` | `Upsert` | Create-or-replace a thread's metadata; latest `Upsert` wins |
| `delete` | `Delete` | Tombstone — removes the thread from folded state |
| `message_appended` | `MessageAppended` | `message_count += 1`, overwrite `last_message_at` |
| `stats` | `Stats` | Absolute snapshot — overrides count + timestamp |

`MessageAppended` exists so `list_threads` stays `O(threads.jsonl)` instead of
`O(total messages)`: appending a message bumps the running count via a tiny log
entry rather than re-scanning the per-thread file. `Stats` backfills legacy
threads whose messages predate `MessageAppended` (folded `message_count` /
`last_message_at` are `Option`; `None` means "no history, read the file once to
backfill").

Per-thread message files, by contrast, are **straight append**: each message is
one JSONL line appended in order.

### Wire types

The on-disk records mirror the OpenHuman contract byte-for-byte. The
`camelCase` serde renames and the `type` / `extraMetadata` wire keys must be
preserved so existing transcripts keep loading
(`src/memory/conversations/types.rs`).

**`ConversationThread`** (one entry in `threads.jsonl`, post-fold):

| Field | Wire key | Notes |
| --- | --- | --- |
| `id` | `id` | Stable thread id |
| `title` | `title` | Human-readable title |
| `chat_id` | `chatId` | Optional host chat id (`i64`); omitted when `None` |
| `is_active` | `isActive` | Not archived/closed |
| `message_count` | `messageCount` | Cached count |
| `last_message_at` | `lastMessageAt` | ISO-8601 |
| `created_at` | `createdAt` | ISO-8601 |
| `parent_thread_id` | `parentThreadId` | Set when branched; omitted when `None` |
| `labels` | `labels` | Free-form tags; defaults empty |
| `personality_id` | `personalityId` | Optional bound persona; omitted when `None` |

**`ConversationMessage`** (one line in a per-thread log):

| Field | Wire key | Notes |
| --- | --- | --- |
| `id` | `id` | Unique within the thread |
| `content` | `content` | Body text |
| `message_type` | `type` | Message kind, e.g. `"text"` |
| `extra_metadata` | `extraMetadata` | Arbitrary `serde_json::Value`; defaults to JSON null |
| `sender` | `sender` | Sender/role |
| `created_at` | `createdAt` | ISO-8601 |

Labels are normalised on write. `normalize_labels` canonicalises legacy
spellings (`work` → `general`, `from_reflection` / `subconscious_tick` →
`subconscious`, `agent-task` / `worker` → `tasks`) and dedupes. A thread whose
`Upsert` carried no labels gets a default from `infer_labels` based on its id
namespace: `proactive:morning_briefing` → `["briefing"]`, other `proactive:*` →
`["notification"]`, everything else → `["general"]`.

### Process-wide write serialization

This is the key concurrency invariant. **Every on-disk mutation serialises
through a single process-wide mutex** so concurrent RPC handlers never interleave
writes to `threads.jsonl` or the per-thread logs:

```text
static CONVERSATION_STORE_LOCK: LazyLock<Mutex<()>>   // parking_lot::Mutex
```

A second static caches the per-workspace inverted index:

```text
static CONVERSATION_INDEX_CACHE: LazyLock<Mutex<HashMap<PathBuf, InvertedIndex>>>
```

keyed by the `memory/conversations` root, so multiple cloned `ConversationStore`
handles pointing at the same workspace share one index.

**Lock ordering:** when both locks are held, `CONVERSATION_STORE_LOCK` is
acquired first, then `CONVERSATION_INDEX_CACHE`. Cross-thread search has a fast
warm-cache path that takes *only* the cache lock (no outer store lock); the cold
path snapshots the thread list under the store lock, releases it before the slow
per-thread JSONL walk, then takes the cache lock alone to insert the built index.
It never holds both across the slow walk. (Ported from OpenHuman, which used
`once_cell::sync::Lazy`; here it is `std::sync::LazyLock`.)

### Durability of writes

JSONL helpers in `store.rs` give crash-safe writes:

- `append_jsonl` — `create(true).append(true)`, then `sync_all()` (fsync) before
  returning.
- `rewrite_jsonl` — write to a sibling `.conversations-<uuid>.tmp`, `sync_all`,
  then atomic `fs::rename` over the target, so a crash mid-write never leaves a
  partially-written transcript. (Replaces OpenHuman's dev-only
  `tempfile::NamedTempFile`.)
- `read_jsonl` — skips blank and invalid lines, so one corrupt line never loses
  the rest of a transcript.

### Public surface

The store is a cloneable `PathBuf` wrapper:

```rust
let store = ConversationStore::new(workspace_dir);
// or from the engine config:
let store = ConversationStore::from_config(&config);
```

Core CRUD + search methods (each also has a free-function shim taking
`workspace_dir`):

| Method / shim | Purpose |
| --- | --- |
| `ensure_thread` | Create-or-update a thread (appends `Upsert`) |
| `list_threads` | All live threads, folding the upsert/delete log |
| `get_messages` | Every message for a thread, append order |
| `append_message` | Append one message + a `MessageAppended` log entry |
| `update_thread_title` / `update_thread_labels` | Metadata edits |
| `update_message` | Patch a message (e.g. rewrite `extraMetadata`) |
| `delete_thread` | Tombstone a thread |
| `purge_threads` | Remove all threads; returns `ConversationPurgeStats` |
| `search_cross_thread_messages` | Substring search across all threads |

`search_cross_thread_messages` returns `CrossThreadHit`s, each carrying the
source `thread_id` (for provenance), `message_id`, `role`, `content`,
`created_at`, and a `score`. It is backed by the in-memory trigram / CJK-bigram
`InvertedIndex`, preserving the scoring contract `score = matched_terms /
total_terms` with a recency tiebreak. This exists for cross-chat continuity:
surfacing context a user shared in chat A when they ask a dependent question in
chat B (issue #1505).

> Note: a `bus` submodule (`ConversationEventBus` / `ChannelEventHandler` and a
> `ChannelEvent` type) lets a host mirror inbound/processed channel turns into
> the store without coupling this crate to the host's event-bus layer.

---

## The archivist (`archivist`)

The archivist's one job: take a chat conversation, **strip the noisy tool-call
payloads**, compose it as markdown, and push the result into a memory tree as a
single leaf. The tree owns persistence and retrieval from there
(`src/memory/archivist/mod.rs`).

### Flow

```text
  Vec<Turn>          raw conversation, tool calls included
        │
        ▼
  clean_conversation()   strip tool_calls_json; drop "tool"-role turns
        │
        ▼
  compose_conversation_md()   one md blob: ## role\n<content>\n\n... per turn
        │
        ▼
  archive_to_tree()      append ONE leaf via the injected TreeLeafSink
        │
        ▼
  TreeLeafSink           tree append + cascade seal (supplied by caller)
```

### `Turn` — the input shape

```text
struct Turn {
    role: String,                       // "user" / "assistant" / "system" / "tool" (free-form)
    content: String,                    // natural-language body
    tool_calls_json: Option<String>,    // raw model-side tool-call payload; dropped during clipping
    timestamp: DateTime<Utc>,           // used as the tree leaf timestamp
}
```

`Turn::new(role, content)` builds a turn with no tool payload, stamped at the
current time. Roles are intentionally free-form so the archivist doesn't fight a
specific harness's role taxonomy.

### Step 1 — clean (`clean_conversation`)

A pure transform (no IO) in `clip.rs`:

1. **Drop every `tool`-role turn** — its content is a tool *result* (stdout
   dumps, JSON responses), noisy and rarely useful out of context.
2. **Strip `tool_calls_json` to `None`** on every surviving turn.

```rust
let cleaned = clean_conversation(&turns); // Vec<Turn>, tool noise removed
```

**Why strip tool calls?** Tool-call JSON is verbose, model-specific, and rarely
meaningful out of context; tool-result turns distort vector embeddings of the
surrounding human conversation. Removing both *before* the conversation lands in
the tree keeps summaries and embeddings focused on natural-language content. This
is the heart of the archivist.

### Step 2 — compose (`compose_conversation_md`)

A pure transform in `compose.rs` that renders the cleaned turns into a single
markdown blob — `## <role>` followed by the content, one blank line between
consecutive turns:

```text
## user
What's the weather like?

## assistant
It's sunny and 24°C.
```

Plain markdown, **no YAML front-matter** — the tree leaf already carries
timestamps and provenance. An empty slice yields an empty string; a trailing
newline is added to a turn's content only if it lacks one.

### Step 3 — archive (`archive_to_tree`)

The end-to-end entry point in `tree_writer.rs`:

```rust
let outcome = archive_to_tree(&sink, session_id, &turns)?;
// outcome.chunk_id        -> deterministic leaf id
// outcome.new_summary_ids -> summaries that sealed during the cascade
// outcome.seal_pending    -> always false for archivist leaves
```

It cleans, composes, computes the leaf id, estimates a token count
(`md.len() / 4`, min 1), and stamps the leaf with the **last cleaned turn's
timestamp** (or "now" for an empty conversation). Then it hands the markdown plus
a `LeafMeta` to the sink as exactly **one** `append_leaf` call. The returned
`ArchiveOutcome` mirrors OpenHuman's `TreeWriteOutcome`; `seal_pending` is always
`false` because archivist leaves never trigger an immediate seal.

**Deterministic, idempotent leaf ids.** `chunk_id_for_session` hashes
`session_id ‖ \0 ‖ markdown` with SHA-256 and takes the first 32 hex chars:

```text
archivist:<sha256(session_id ‖ 0x00 ‖ markdown)[..32]>
```

The same `(session_id, markdown)` pair always produces the same id, so retries of
an unchanged conversation are idempotent; a distinct session or an edited
transcript hashes to a fresh id.

### The `TreeLeafSink` write contract

The archivist must not hard-depend on `tree` internals (the tree module is ported
concurrently), so it appends through a small trait in `sink.rs`:

```rust
pub trait TreeLeafSink {
    /// Append `markdown` as one leaf; return ids of any summaries that sealed.
    fn append_leaf(&self, markdown: &str, meta: &LeafMeta) -> anyhow::Result<Vec<String>>;
}
```

`LeafMeta` carries the `chunk_id`, the source `session_id` (archives must cite
their source session — a spec invariant), the heuristic `token_count`, and the
`timestamp`. A tree-backed implementation lives in the `tree` module;
`RecordingSink` is a zero-IO implementation used in tests to assert the cleaned
markdown and single-leaf behaviour (optionally returning fixed "sealed" ids via
`RecordingSink::with_seal_ids`).

> Archivist leaves are **synthetic conversation snapshots**, not chunk-store
> rows. In OpenHuman they participate in the L0 buffer contract only; sealing
> archivist-only source trees upward needs a dedicated hydration path (the
> extractor-derived `entities` / `topics` / `score` are always empty for cleaned
> conversations, so they're omitted from `LeafMeta`).

### Episodic capture (`record_turn` / `session_entries`)

Distinct from the batch tree-leaf flow, `archivist/store.rs` offers a per-turn
disk capture surface — one markdown file per turn:

```text
<workspace>/memory_tree/content/episodic/<session_id>/<seq:06>.md
```

- `record_turn(config, ArchivedTurn)` — appends a turn. The on-disk directory is
  the source of truth for sequencing: `turn.seq` is ignored on input, the next
  free `NNNNNN` is computed from existing files, and the returned `ArchivedTurn`
  carries the actually-assigned seq. Writes go through the atomic
  `write_if_new` tempfile+rename contract. Session ids are sanitised (non
  `[A-Za-z0-9_-]` → `_`) into a safe single path component.
- `session_entries(config, session_id)` — reads every turn for a session, sorted
  by seq ascending; a missing directory yields an empty vec.

Each file is a YAML front-matter block (`session_id`, `seq`, `timestamp_ms`,
`role`, `cost_microdollars`, and optional `lesson` / `tool_calls_json`) followed
by the body. `ArchivedTurn` field names mirror the legacy OpenHuman
`EpisodicEntry`, so a harness migrating off the old capture path can dual-write
the same payload. Note that this surface *retains* `tool_calls_json` (it is raw
capture); the tool-stripping only happens in the batch `archive_to_tree` path.

---

## See also

- [Summary-Trees](Summary-Trees) — where archived leaves land and seal upward.
- [Storage-Primitives](Storage-Primitives) — the markdown-authoritative,
  rebuildable-index storage model.
- [Retrieval](Retrieval) — how archived conversation content is later recalled.
- [Architecture-Overview](Architecture-Overview) — where these modules sit in the
  layered engine.
