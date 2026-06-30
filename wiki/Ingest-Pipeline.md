# Ingest Pipeline

The ingest pipeline turns a host-supplied, source-scoped payload into persisted
chunks, fast scores, and enqueued tree jobs. It is the deterministic core of
TinyCortex's write path, ported from OpenHuman's `memory_sync/canonicalize`,
`memory/ingest_pipeline`, and `memory/ingestion`.

TinyCortex does **not** own live memory sync. The host (OpenHuman or another
embedder) decides *when* to ingest and supplies the payload; this module owns
everything *after* that boundary. The live sync runner/scheduler and the
namespace document/graph store are intentionally out of scope.

Source: [`src/memory/ingest/`](Architecture-Overview) — `mod.rs`,
`pipeline.rs`, `canonicalize/`, plus the chunk model in
`src/memory/chunks/`.

## The path

```text
canonicalize -> write raw markdown -> chunk -> score/extract
  -> persist chunk metadata -> enqueue tree jobs (-> append/seal in worker)
```

The hot path (`pipeline::ingest_canonical`) stops at *enqueue*. Full extraction,
the admission gate, summary-buffer append, and sealing all run later in the
async extract worker driven off the [`TreeJobSink`](#the-treejobsink-seam) — not
on this path. See [Job Queue](Job-Queue) and [Summary Trees](Summary-Trees).

## Source kinds

A chunk's splitting strategy and lifecycle are routed by its `SourceKind`
(`src/memory/chunks/types.rs`). Three kinds exist; the wire/DB string form is
`snake_case`:

| `SourceKind` | wire string | grouping unit | examples |
|---|---|---|---|
| `Chat`     | `chat`     | channel / group | Discord, Telegram, WhatsApp, agent conversations |
| `Email`    | `email`    | thread / participant set | Gmail, generic IMAP / other email |
| `Document` | `document` | none (single record) | Notion page, Drive doc, meeting note, uploaded file |

A finer-grained `DataSource` enum names the concrete provider and maps to exactly
one `SourceKind` via `DataSource::kind()`. It is `#[non_exhaustive]` so new
providers can be added without breaking downstream matches. Current variants:

| `DataSource` | wire string | feeds `SourceKind` |
|---|---|---|
| `Discord`      | `discord`       | `Chat` |
| `Telegram`     | `telegram`      | `Chat` |
| `Whatsapp`     | `whatsapp`      | `Chat` |
| `Conversation` | `conversation`  | `Chat` |
| `Gmail`        | `gmail`         | `Email` |
| `OtherEmail`   | `other_email`   | `Email` |
| `Notion`       | `notion`        | `Document` |
| `MeetingNotes` | `meeting_notes` | `Document` |
| `DriveDocs`    | `drive_docs`    | `Document` |

## Canonicalisers

Canonicalisers (`src/memory/ingest/canonicalize/`) normalise a source-specific
payload into one canonical Markdown blob plus seed [`Metadata`](Core-Concepts).
They normalise *shape and provenance only* — no semantic interpretation; scoring,
extraction, and summarisation happen downstream. Each adapter returns the same
shape:

```text
CanonicalisedSource {
    markdown: String,   // canonical Markdown blob
    metadata: Metadata, // provenance, cloned onto every chunk the chunker emits
}
```

All three adapters accept a flexible timestamp (`deserialize_flexible_timestamp`):
a JSON integer is read as epoch **milliseconds**; a JSON string is parsed as
RFC 3339 / ISO-8601, falling back to a decimal epoch-ms string. An unparseable
string is a hard serde error — never a silent default.

### Chat (`canonicalize/chat.rs`)

Input is a `ChatBatch` (`platform`, `channel_label`, ordered `messages`). Each
`ChatMessage` carries `author`, `timestamp`, `text`, optional `source_ref`.
Messages are defensively sorted by timestamp; each renders as an `H2` block with
no leading `# ` title (platform/channel belong in front matter):

```md
## 2026-04-21T10:12:00Z — Alice
Message body here.

## 2026-04-21T10:12:40Z — Bob
Reply body here.
```

The `## ` boundary is exactly what the chat splitter keys on downstream.
Metadata: `timestamp = first message`, `time_range = (first, last)`,
`source_ref` defaults to the first message's pointer. An empty batch yields
`Ok(None)` ("nothing to ingest").

### Email (`canonicalize/email.rs`)

Input is an `EmailThread` (`provider`, `thread_subject`, ordered `messages`).
Each `EmailMessage` body is passed through `email_clean::clean_body` to strip
reply chains, marketing footers, and legal disclaimers before rendering. Each
message becomes a `---`-delimited block with a small header:

```md
---
From: alice@example.com
To: bob@example.com
Cc: carol@example.com
Subject: Re: launch
Date: 2026-04-21T10:12:00+00:00
List-Unsubscribe: <https://...>

Cleaned body markdown here.
```

`To:`/`Cc:`/`List-Unsubscribe:` lines are omitted when empty/absent. The
`---\nFrom:` separator is what the email splitter keys on. Empty thread →
`Ok(None)`.

### Document (`canonicalize/document.rs`)

Input is a `DocumentInput` (`provider` defaulting to `"unknown"`, `title`,
`body`, `modified_at` defaulting to `Utc::now()`, optional `source_ref`). The
body is trimmed and passed through verbatim (markdown is preserved); no title
header is added. `timestamp` and both ends of `time_range` are `modified_at`.
When both title and body are empty → `Ok(None)`. Documents may also carry a
`path_scope` that overrides the on-disk content directory while leaving
`source_id` as the dedup key.

## Chunking

`chunks::chunk_markdown` (`src/memory/chunks/produce.rs`) slices the canonical
blob into `Chunk`s of at most `ChunkerOptions::max_tokens`
(`DEFAULT_CHUNK_MAX_TOKENS = 3_000`, deliberately well below the summary-tree
input budget). Chunk sizes are bounded by the **conservative** token estimate
(see below), not the GPT `chars/4` heuristic, so dense markdown/hash/code or
multilingual text cannot overflow a downstream embedder.

Dispatch by `SourceKind`:

- **Chat** — split at `## ` message boundaries (`split_chat_messages`), then
  greedy-pack consecutive messages into one chunk until adding the next would
  exceed `max_tokens`. Lines before the first `## ` are dropped; a blob with no
  `## ` becomes a single unit.
- **Email** — split at `---` lines followed (within 8 lines) by a `From:` line
  (`split_email_messages`), then the same greedy-pack as chat. Content before the
  first separator is dropped.
- **Document** — split directly by `split_by_token_budget`, a conservative
  token-estimate splitter that cascades paragraph → sentence → whitespace →
  hard-char with ~12% overlap between adjacent chunks.

**Oversize units.** When a single chat message / email exceeds `max_tokens`, the
accumulator is flushed and that unit alone is run through `split_by_token_budget`;
each resulting piece is emitted with `partial_message = true` so downstream
scorers can lower its weight relative to whole-unit chunks. Document chunks are
always `partial_message = false`.

A degenerate empty input still produces exactly one empty chunk (`seq 0`),
matching the original OpenHuman behaviour.

## The chunk model

`Chunk` (`src/memory/chunks/types.rs`) is the atomic persistence unit — the leaf
of a source tree.

| field | meaning |
|---|---|
| `id` | deterministic id (see below) |
| `content` | canonical Markdown body |
| `metadata` | `Metadata` cloned from the canonicalised source |
| `token_count: u32` | rough GPT estimate, `approx_token_count` (1 token ≈ 4 chars) |
| `seq_in_source: u32` | stable sequence within the logical source, starting at `0` |
| `created_at` | when persisted locally (epoch-ms wire) |
| `partial_message: bool` | `true` for sub-splits of one oversized unit |

`Metadata` carries provenance: `source_kind`, `source_id` (the grouping id:
channel / thread / doc), `owner`, point `timestamp`, covering `time_range`,
pass-through `tags`, optional `source_ref` back-pointer, and optional
`path_scope`.

### Deterministic chunk id

```text
chunk_id = hex(sha256(
    source_kind.as_str() | 0x00 | source_id | 0x00 |
    seq_in_source (big-endian u32) | 0x00 | content
))[..32]
```

The id is the first **32 hex chars** (128 bits) of the SHA-256. Content is folded
in so several ingest calls sharing a `source_id` cannot collide on
`seq = 0,1,2,…`. Re-ingesting identical content under the same `(source_id, seq)`
reproduces the same id, which keeps `upsert_chunks` idempotent — this is how
chat/email replay stays safe without a source-level gate.

### Token estimation

- `approx_token_count` — GPT-family `chars/4`. Drives `token_count` and downstream
  summariser/seal budgeting.
- `conservative_token_estimate` — pessimistic, weights characters by class
  (alphanumeric 0.5 tok/char, whitespace 0.25, punctuation/non-ASCII 1.0).
  Used **only** for embed-safety / split decisions so the chunker never
  under-splits. `truncate_to_conservative_tokens` is the embed-path backstop.

## Orchestration: `ingest_canonical`

`pipeline::ingest_canonical` runs the full path for an already-canonicalised
source. Step by step:

1. **Chunk** the canonical markdown. Empty → `IngestSummary::empty`.
2. **Source gate (documents only).** Transactionally claim
   `claim_source_ingest_tx` *before* any write, so two concurrent ingests of the
   same document can't both proceed. The gate key is `source_id`, or
   `{source_id}@{version_ms}` when `gate_version_ms` is set (lets a later revision
   in non-destructively). Lost claim → `IngestSummary::already_ingested`. Chat and
   email have **no** source gate — their `source_id` is a stream under which many
   batches accumulate, so they rely on deterministic chunk ids for replay
   idempotency.
3. **Stage chunk bodies** to the content store (atomic write + sha256;
   see [Storage Primitives](Storage-Primitives)).
4. **Snapshot prior lifecycle** of each chunk *before* the upsert. A chunk that
   already progressed past `pending_extraction` must not be re-scheduled, or
   already-buffered/sealed content would flow through the tree twice.
5. **Upsert chunk rows** (`upsert_chunks`, idempotent on chunk id). If
   `raw_refs` is set, attach them so a worker can resolve bodies from verbatim
   archive files.
6. **Fast-score** every chunk (`score_chunks_fast`) — cheap, no LLM on the hot
   path. See [Scoring and Extraction](Scoring-and-Extraction).
7. **Persist scores, schedule, enqueue.** For each chunk whose prior lifecycle is
   `None` or `pending_extraction`: persist its score, set status to
   `CHUNK_STATUS_PENDING_EXTRACTION`, and `sink.enqueue_extract(chunk_id)`. The
   fast-score `kept` flag only feeds `chunks_dropped` for reporting — **final
   admission happens later in the worker**, not here.

### The `TreeJobSink` seam

Because the async queue is ported separately, the orchestrator injects the
tree-job enqueue behind a trait rather than hard-depending on
`crate::memory::queue`:

```text
trait TreeJobSink {
    fn enqueue_extract(&self, chunk_id: &str) -> Result<()>;
}
```

`NullJobSink` drops every job (chunks persisted + scored, no tree). A host wires
the real queue behind this trait. `IngestOptions` carries the per-call knobs
(`gate_version_ms`, `raw_refs`).

### Convenience wrappers

`pipeline.rs` exposes per-kind entry points that canonicalise then call
`ingest_canonical`:

| function | notes |
|---|---|
| `ingest_chat` | canonicalise a `ChatBatch`, then ingest |
| `ingest_email` | canonicalise an `EmailThread`, then ingest |
| `ingest_email_with_raw_refs` | as above, attaching `raw_refs` to every chunk |
| `ingest_document` | document, no version |
| `ingest_document_with_scope` | adds an explicit `path_scope` |
| `ingest_document_versioned` | best-effort pre-gate via `is_source_ingested`, then version-keyed claim |

### `IngestSummary`

The result reports work *pending*, not completed: `source_id`, `chunks_written`,
`chunks_dropped` (fast-score would-drop count), `chunk_ids`,
`extract_jobs_enqueued` (leaves handed to the sink), and `already_ingested`.

## See also

- [Scoring and Extraction](Scoring-and-Extraction)
- [Summary Trees](Summary-Trees)
- [Storage Primitives](Storage-Primitives)
- [Job Queue](Job-Queue)
