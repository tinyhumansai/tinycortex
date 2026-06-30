# Sources

The **source registry** answers a single question: *what feeds my memory?* It is a typed, TOML-backed catalog of connectors — Composio OAuth connections, local folders, GitHub repos, RSS feeds, Twitter/X queries, web pages, and agent conversations — that the host's sync runner consumes to decide what to ingest and when.

All source code for this domain lives under `src/memory/sources/`:

| Module | Responsibility |
| --- | --- |
| `types.rs` | The `MemorySourceEntry` contract, `SourceKind` discriminator, and reader output types (`SourceItem`, `SourceContent`, `ContentType`). |
| `validation.rs` | Per-kind required-field rules (`validate_entry`) and the shared path-traversal guard (`ensure_within_base`). |
| `registry.rs` | The TOML-backed `SourceRegistry` with the load → modify → validate → save CRUD cycle, plus `MemorySourcePatch`. |
| `readers/` | The `SourceReader` trait and the two local reader implementations (`folder`, `conversation`). |

## Ownership boundary: TinyCortex does not own sync

Per the engine spec, **TinyCortex does not own live sync, polling, or OAuth.** Network-backed kinds keep their type contracts and validation here, but their live fetchers are deliberately host-owned. OpenHuman (or the embedding host) decides *when* to ingest and supplies the payloads; TinyCortex owns everything *after* that boundary.

Only the two local kinds ship real readers:

```text
reader_for(kind) -> Option<Box<dyn SourceReader>>
  Folder        -> Some(FolderReader)
  Conversation  -> Some(ConversationReader)
  Composio      -> None   ┐
  GithubRepo    -> None   │ network-backed:
  TwitterQuery  -> None   │ host-owned fetch
  RssFeed       -> None   │
  WebPage       -> None   ┘
```

`is_locally_readable(kind)` returns `true` only for `Folder` and `Conversation`. A caller that receives `None` from `reader_for` should defer to the host's sync runner.

## Source kinds

`SourceKind` is the discriminator. Its wire representation is **snake_case** and is part of the persisted `config.toml` contract — these strings must stay stable across versions (`SourceKind::as_str`):

| `SourceKind` | Wire string | Required fields | Backed by |
| --- | --- | --- | --- |
| `Composio` | `composio` | `toolkit`, `connection_id` | Host (OAuth) |
| `Conversation` | `conversation` | *(none)* | Local reader |
| `Folder` | `folder` | `path` | Local reader |
| `GithubRepo` | `github_repo` | `url` | Host |
| `TwitterQuery` | `twitter_query` | `query` | Host |
| `RssFeed` | `rss_feed` | `url` | Host |
| `WebPage` | `web_page` | `url` | Host |

Every kind also requires non-empty `id` and `label`.

## The `MemorySourceEntry` contract

All kind-specific fields are flattened onto one struct as `Option`s; the `kind` discriminator decides which are required. Entries are persisted under `[[memory_sources]]` in `config.toml`.

### Common fields (all kinds)

| Field | Type | Notes |
| --- | --- | --- |
| `id` | `String` | Stable unique id, e.g. `src_<uuid>`. |
| `kind` | `SourceKind` | Discriminator. |
| `label` | `String` | Human-readable label. |
| `enabled` | `bool` | Whether the source participates in sync. Defaults to `true`. |

### Kind-specific fields

| Field | Type | Used by |
| --- | --- | --- |
| `toolkit` | `Option<String>` | `composio` (e.g. `gmail`) |
| `connection_id` | `Option<String>` | `composio` |
| `path` | `Option<String>` | `folder` (filesystem root) |
| `glob` | `Option<String>` | `folder` (defaults to `**/*.md`) |
| `url` | `Option<String>` | `github_repo`, `rss_feed`, `web_page` |
| `branch` | `Option<String>` | `github_repo` (repo default when absent) |
| `paths` | `Vec<String>` | `github_repo` path filters |
| `max_commits` | `Option<u32>` | `github_repo` (default 1000 when absent) |
| `max_issues` | `Option<u32>` | `github_repo` (default 1000 when absent) |
| `max_prs` | `Option<u32>` | `github_repo` (default 1000 when absent) |
| `query` | `Option<String>` | `twitter_query` |
| `since_days` | `Option<u32>` | `twitter_query` look-back window |
| `max_items` | `Option<u32>` | `rss_feed` (and Composio caps) |
| `selector` | `Option<String>` | `web_page` CSS selector |

### Sync budgets (all kinds)

Every source can carry per-run budget caps. The host's sync runner is expected to honour them and stop once a cap is reached:

| Field | Type | Meaning |
| --- | --- | --- |
| `max_tokens_per_sync` | `Option<u64>` | Stop syncing once this token budget is hit. |
| `max_cost_per_sync_usd` | `Option<f64>` | Refuse LLM calls once this USD cost is reached. |
| `sync_depth_days` | `Option<u32>` | Only fetch items from the last N days. |

`config.toml` serialization skips any `None`/empty field, so an entry only records the fields that matter for its kind.

### Example persisted entries

```toml
[[memory_sources]]
id = "src_3f9c…"
kind = "folder"
label = "Project notes"
path = "/Users/me/notes"
glob = "**/*.md"
max_tokens_per_sync = 50000

[[memory_sources]]
id = "src_a17b…"
kind = "composio"
label = "Gmail"
toolkit = "gmail"
connection_id = "conn_123"
max_items = 100
sync_depth_days = 30
```

## The registry

`SourceRegistry::new(config_path)` wraps a single TOML file. The file need not exist yet — reads return an empty list and the first write creates it (and any missing parent directories). Only the `memory_sources` array is rewritten; **all other top-level config keys are preserved** across writes.

```text
SourceRegistry
  list()                              -> Vec<MemorySourceEntry>
  list_enabled_by_kind(kind)          -> enabled entries of one kind
  get(id)                             -> Option<MemorySourceEntry>
  add(entry)                          -> validate, reject duplicate id, append
  update(id, MemorySourcePatch)       -> patch in place, re-validate, save
  remove(id)                          -> bool
  remove_composio_source_by_connection_id(conn_id) -> usize
  upsert_composio_source(toolkit, conn_id, label)  -> entry
  apply_all_in()                      -> enable all, clear every cap
```

Every mutation follows the spec's **atomic load-modify-validate-save** cycle: load the current file, apply the change in memory, validate, and persist. Writes themselves are atomic — the new TOML is written to a same-directory temp file (`.<name>.tmp-<uuid>`), `fsync`'d, then renamed over the config so a crashed write can never leave a truncated `config.toml`.

### Composio upserts and All-In mode

Composio sources are keyed on `connection_id`, not their `src_*` id. `upsert_composio_source` updates an existing connection's label or inserts a fresh entry with conservative per-toolkit caps; it never clobbers user-customised caps. Those defaults come from `memory_sync_defaults_for_toolkit`, returning `(max_items, sync_depth_days)`:

| Toolkit | `max_items` | `sync_depth_days` |
| --- | --- | --- |
| `gmail` | 100 | 30 |
| `slack` | 50 | 14 |
| `notion` | 30 | 30 |
| `linear` | 50 | 30 |
| `clickup` | 50 | 30 |
| `github` | 50 | 30 |
| *(any other)* | 30 | 14 |

`apply_all_in()` flips every source to `enabled = true` and clears all per-source caps (`max_items`, `since_days`, `sync_depth_days`, `max_commits`, `max_issues`, `max_prs`, `max_tokens_per_sync`, `max_cost_per_sync_usd`) — the unbounded "All In" sync mode.

`MemorySourcePatch` is the partial-update payload for `update`: absent fields are left unchanged; present fields overwrite. It deliberately does **not** expose `id` or `kind` — a source's identity and discriminator are immutable.

## Validation

`MemorySourceEntry::validate()` delegates to `validate_entry`, which returns a human-readable message on the first failing rule. Because kind-specific fields are flattened onto one struct, the checks are runtime/discriminator-based:

1. `id` must be non-empty.
2. `label` must be non-empty.
3. Each kind's required fields (table above) must be present and non-empty.

Validation runs at both `add` and `update` time, so an invalid entry can never be persisted.

## Readers and their outputs

A `SourceReader` knows how to **list** the items available in a source and **read** the content of one item. The trait is intentionally narrow:

```text
#[async_trait]
trait SourceReader {
    fn kind(&self) -> SourceKind;
    async fn list_items(&self, source, config) -> Vec<SourceItem>;
    async fn read_item(&self, source, item_id, config) -> SourceContent;
}
```

Readers are synchronous internally but expose an async surface so a host-owned network reader can satisfy the same contract.

### `SourceItem` (from `list_items`)

| Field | Type | Notes |
| --- | --- | --- |
| `id` | `String` | Reader-scoped id (folder-relative path, thread id). Stable enough to pass back into `read_item`. |
| `title` | `String` | Human-readable title. |
| `updated_at_ms` | `Option<i64>` | Last-modified time in epoch milliseconds, when known. |

### `SourceContent` (from `read_item`)

| Field | Type | Notes |
| --- | --- | --- |
| `id` | `String` | Matches the `SourceItem::id` it was read from. |
| `title` | `String` | Human-readable title. |
| `body` | `String` | The item body, rendered per `content_type`. |
| `content_type` | `ContentType` | `markdown` \| `html` \| `plaintext` (snake_case wire strings). |
| `metadata` | `serde_json::Value` | Reader-specific JSON object. |

These output contracts are shared across every reader so the host can ingest payloads uniformly regardless of origin.

## Local readers and path-traversal defenses

The shared guard `ensure_within_base(base, target)` canonicalizes both paths through `std::fs::canonicalize` (resolving symlinks and `..` segments) and returns `MemoryError::PathEscape("path traversal denied")` if the resolved target escapes the base directory. Both local readers rely on it.

### `FolderReader` (`folder`)

Walks files under `path` with `walkdir` (symlinks not followed), matching the source's `glob` (default `**/*.md`). Globs are compiled to an anchored `regex` over the slash-normalised relative path, supporting `*` (non-separator run), `?` (one non-separator char), `**` (any run including separators), and `**/` (zero or more leading directories).

Safety properties:

- **10 MB file-size cap.** Files larger than `FOLDER_FILE_SIZE_CAP_BYTES` (10 MB) are skipped during `list_items` and rejected during `read_item`, so a huge file can't blow up the renderer or chunker.
- **Path-traversal guard on read.** `read_item` joins `item_id` onto the folder root, then runs `ensure_within_base` to reject `..` traversal and symlink escapes.
- **Content-type inference.** `.md` → `markdown`, `.html`/`.htm` → `html`, otherwise `plaintext`.

### `ConversationReader` (`conversation`)

Treats every agent conversation thread as a source item. Threads are JSON files under `<workspace>/threads/`; `list_items` enumerates `*.json` (item id = file stem) and `read_item` renders the thread to markdown.

Safety properties:

- **`item_id` rejection.** `read_item` rejects any `item_id` containing `..`, `/`, or `\` *before* touching the filesystem.
- **Containment re-check.** After resolving the `<item_id>.json` path it re-runs `ensure_within_base` against the threads directory.
- **Markdown rendering.** A thread `{ title, messages: [{ role, content }] }` becomes a `# title` heading followed by `**role**: content` blocks; empty-content messages are skipped. Output `metadata` carries `source_type: "conversation"` and `thread_id`.

## See also

- [Ingest-Pipeline](Ingest-Pipeline)
- [Storage-Primitives](Storage-Primitives)
- [Conversations-and-Archivist](Conversations-and-Archivist)
- [Architecture-Overview](Architecture-Overview)
