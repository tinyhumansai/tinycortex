# Conversation and Archivist Spec

OpenHuman modules: `memory_conversations`, `memory_archivist`.

## Responsibility

Conversation storage persists chat threads/messages as inspectable JSONL.
Archivist converts selected conversation turns into cleaned tree leaves for
semantic memory.

## Conversation Store

Storage root:

```text
<workspace>/memory/conversations/
```

Files:

- `threads.jsonl`: append-only thread metadata log with upsert/delete entries.
- `threads/<thread_id>.jsonl`: message records for each thread.

A process-wide mutex serializes all on-disk mutations.

## Conversation Types

Thread metadata should include:

- id, title, active/deleted state.
- created/updated timestamps.
- labels.
- optional parent thread id.
- optional personality id.
- message counts and last message timestamp for summaries.

Message records should include:

- id.
- content.
- type.
- sender.
- created timestamp.
- extra metadata.

Patch operations must update labels, titles, metadata, deletion/purge state,
and message records without corrupting append-only logs.

## Event Persistence

`memory_conversations::bus` subscribes to inbound channel events and mirrors
messages into the store. This makes non-web providers such as Slack or Telegram
use the same local transcript path as UI-driven threads.

## Search Helpers

OpenHuman includes an inverted index and tokenizer. TinyCortex should preserve
language-agnostic normalization for thread/message search, including Unicode
normalization and diacritic-insensitive matching where practical.

## Archivist Flow

Archivist turns raw conversation history into a tree leaf:

```text
Vec<Turn>
  -> clean_conversation()
  -> compose_conversation_md()
  -> archive_to_tree()
  -> memory_tree append/seal pipeline
```

`Turn` includes role, content, optional tool-call JSON, and timestamp.

`clean_conversation` must:

- remove `tool_calls_json`.
- drop turns with role `tool`.
- preserve natural-language user/assistant content.

`compose_conversation_md` emits markdown sections like:

```markdown
## user
...

## assistant
...
```

`archive_to_tree` writes the cleaned blob as one leaf. The chunk id is derived
from `sha256(session_id || markdown)` truncated to the configured id length in
OpenHuman.

## Rationale

Tool-call JSON and tool-result output are verbose, provider-specific, and often
irrelevant out of context. They should not distort embeddings or summaries of
human conversation. Clean natural-language transcript is the archival unit.

## Required Invariants

- Conversation JSONL is transcript persistence, not semantic memory by itself.
- Ingestion into searchable memory is a separate archivist/tree operation.
- Disk mutation is serialized.
- Tool turns and tool-call JSON are removed before archival.
- Archives should cite source session/thread ids.

## TinyCortex Landing Area

```text
src/memory/conversations/
src/memory/archivist/
```

Port order: transcript types, JSONL store contracts, tokenizer/index tests,
archivist clean/compose tests, then tree writer integration.

