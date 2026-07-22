# OpenHuman Code Reference

This document is a source-grounded reference for porting OpenHuman's memory
engine into TinyCortex. It records the observed module responsibilities,
contract types, operation boundaries, and implementation invariants from:

the vendoring host's `src/openhuman/` tree.

The goal is not to copy OpenHuman wholesale. TinyCortex should preserve the
contracts and failure semantics first, then reimplement storage and workers with
small, testable Rust modules.

Current TinyCortex status: many domain contracts and storage primitives are
ported as Rust APIs, while host-facing controller/schema registries, concrete
agent tools, production sync runners, and the `agent_memory` adapter layer are
not local modules yet. Treat those sections as OpenHuman integration context
unless they point at an implemented `src/memory/*` module.

TinyCortex does not own OpenHuman's memory sync. The OpenHuman application owns
the sync module, upstream event/polling behavior, and the policy for when data
is ingested. TinyCortex documents and implements the memory-engine side of that
boundary: OpenHuman calls it on demand with source-scoped payloads or canonical
ingest requests.

## Module Inventory

| OpenHuman module | TinyCortex concern | Primary contract |
| --- | --- | --- |
| `memory` | Orchestration/RPC policy | high-level `Memory` trait and shared contracts; RPC/tool adapters are deferred |
| `memory_store` | Persistence substrate | content, vectors, KV, entity index, and starter store; chunks/trees live in sibling modules |
| `memory_sources` | Source contracts | source config, validation, item/content reader shapes |
| `memory_sync` | Sync integration contracts | `SyncPipeline`, canonicalizers, sync status; runner remains OpenHuman-owned |
| `memory_tree` | Hierarchical summarization | tree read/write IO, bucket seal, retrieval, scoring |
| `memory_queue` | Async worker queue | job kinds, payloads, dedupe keys, retry states |
| `memory_diff` | Change ledger | snapshots, item diffs, checkpoints, read markers |
| `memory_entities` | Entity catalog | canonical entity ids, aliases, handles |
| `memory_graph` | Relationship graph | weighted co-occurrence edges |
| `memory_conversations` | Thread/message store | JSONL thread/message records |
| `memory_archivist` | Episodic archive | per-turn capture, cleaned tree archival |
| `memory_goals` | Durable goals | markdown-backed ordered goals list |
| `memory_tools` | Tool-scoped rules | priority rules by tool namespace |
| `agent_memory` | Agent recall/benchmarking | OpenHuman/host adapter context; not a current TinyCortex module |
| `memory_search` | Agent search tools | OpenHuman tool context; TinyCortex has retrieval/scoring primitives |

## Source Files Used

These are the primary OpenHuman files referenced by this pass:

- `memory/mod.rs`
- `memory_store/types.rs`
- `memory_store/memory_trait.rs`
- `memory_sources/types.rs`
- `memory_sync/mod.rs`
- `memory_sync/traits.rs`
- `memory_sync/sources/mod.rs`
- `memory_sync/sync_status/types.rs`
- `memory_tree/io.rs`
- `memory_tree/tree/registry.rs`
- `memory_tree/tree_runtime/types.rs`
- `memory_tree/retrieval/rpc.rs`
- `memory_queue/types.rs`
- `memory_diff/types.rs`
- `memory_diff/ops.rs`
- `memory_entities/types.rs`
- `memory_graph/types.rs`
- `memory_conversations/types.rs`
- `memory_archivist/types.rs`
- `memory_goals/types.rs`
- `memory_tools/types.rs`
- `agent_memory/types.rs`
- `agent_memory/memory_loader.rs`
- `memory_search/mod.rs`
- `memory_search/tools/mod.rs`
- `memory_search/tools/hybrid_search.rs`
- `memory_search/scoring.rs`
- `memory/schema/registry.rs`

## `memory`: Orchestration Layer

OpenHuman's `memory/mod.rs` explicitly states that this layer owns routing and
policy, not storage: no SQLite tables, markdown vaults, or vectors should live
here. TinyCortex should keep `memory` as the facade that wires remembering,
OpenHuman-triggered ingestion, retrieval, and RPC/tool adapters together.
Current TinyCortex exposes the shared contracts plus `ingest/` and
`retrieval/` Rust APIs; `query/`, `read_rpc/`, `schemas/`, and `tools/` are
deferred host adapter layers.

Observed exports:

```rust
pub use traits::{Memory, MemoryCategory, MemoryEntry, MemoryTaint,
    NamespaceSummary, RecallOpts};
pub use crate::openhuman::memory_store::{MemoryClient, UnifiedMemory};
pub use crate::openhuman::memory_store::types::NamespaceDocumentInput;
pub use crate::openhuman::memory_queue as jobs;
```

Porting requirements:

- Keep a single top-level trait for agent-facing storage operations.
- Normalize blank namespaces to `global`.
- Preserve `MemoryTaint` as a safety-critical provenance signal.
- Keep orchestration dependent on storage traits, never the reverse.
- Expose an on-demand ingest boundary for OpenHuman; do not own sync code that
  polls, subscribes to, or schedules upstream sources.

The OpenHuman `UnifiedMemory` implementation maps trait calls into namespace
document storage. `store` is user/internal by default; external sync paths call
`store_with_taint`.

```rust
async fn store_with_taint(
    &self,
    namespace: &str,
    key: &str,
    content: &str,
    category: MemoryCategory,
    session_id: Option<&str>,
    taint: MemoryTaint,
) -> anyhow::Result<()>;
```

Recall combines ranked namespace search, optional category filtering,
same-session episodic recall, and cross-session episodic recall. TinyCortex
should model these as independent retrieval providers rather than hard-wiring
them into one SQL implementation.

## `memory_store`: Storage Primitives

`memory_store` is the durable substrate in OpenHuman. In TinyCortex,
`src/memory/store/` owns content files, generic vectors, KV, safety, and the
entity occurrence index; chunks and trees are sibling modules
(`src/memory/chunks/`, `src/memory/tree/`). OpenHuman's shrinking unified
compatibility facade is migration context, not a current TinyCortex store
module.

Key namespace document shape:

```rust
pub struct NamespaceDocumentInput {
    pub namespace: String,
    pub key: String,
    pub title: String,
    pub content: String,
    pub source_type: String,
    pub priority: String,
    pub tags: Vec<String>,
    pub metadata: serde_json::Value,
    pub category: String,
    pub session_id: Option<String>,
    pub document_id: Option<String>,
    pub taint: MemoryTaint,
}
```

Retrieval should expose both rendered context and structured hits:

```rust
pub struct NamespaceMemoryHit {
    pub id: String,
    pub kind: MemoryItemKind,
    pub namespace: String,
    pub key: String,
    pub title: Option<String>,
    pub content: String,
    pub category: String,
    pub source_type: Option<String>,
    pub updated_at: f64,
    pub score: f64,
    pub score_breakdown: RetrievalScoreBreakdown,
    pub document_id: Option<String>,
    pub chunk_id: Option<String>,
    pub supporting_relations: Vec<GraphRelationRecord>,
    pub taint: MemoryTaint,
}
```

Important invariants:

- `GLOBAL_NAMESPACE` is the fallback namespace.
- `MemoryItemKind` serializes as `document`, `kv`, `episodic`, or `event`.
- Score breakdown includes keyword, vector, graph, episodic, freshness, and
  final score.
- Taint defaults to internal for legacy rows, but unknown persisted taint must
  fail closed at the safety layer.

Implementation note: OpenHuman's store has both canonical file content and
derived indexes. TinyCortex should keep body storage inspectable and make
SQLite/vector/tree indexes rebuildable.

## `memory_sources`: Source Registry

OpenHuman persists memory sources in config and validates each source by kind.
The source model flattens kind-specific fields into one entry.

```rust
pub enum SourceKind {
    Composio,
    Conversation,
    Folder,
    GithubRepo,
    TwitterQuery,
    RssFeed,
    WebPage,
}

pub struct MemorySourceEntry {
    pub id: String,
    pub kind: SourceKind,
    pub label: String,
    pub enabled: bool,
    pub toolkit: Option<String>,
    pub connection_id: Option<String>,
    pub path: Option<String>,
    pub glob: Option<String>,
    pub url: Option<String>,
    pub branch: Option<String>,
    pub paths: Vec<String>,
    pub max_commits: Option<u32>,
    pub max_issues: Option<u32>,
    pub max_prs: Option<u32>,
    pub query: Option<String>,
    pub since_days: Option<u32>,
    pub max_items: Option<u32>,
    pub selector: Option<String>,
    pub max_tokens_per_sync: Option<u64>,
    pub max_cost_per_sync_usd: Option<f64>,
    pub sync_depth_days: Option<u32>,
}
```

Validation rules:

- `id` and `label` are always required.
- `composio` requires `toolkit` and `connection_id`.
- `folder` requires `path`.
- `github_repo`, `rss_feed`, and `web_page` require `url`.
- `twitter_query` requires `query`.
- `conversation` has no kind-specific required fields.

Reader contracts:

```rust
pub struct SourceItem { pub id: String, pub title: String,
    pub updated_at_ms: Option<i64> }
pub enum ContentType { Markdown, Html, Plaintext }
pub struct SourceContent { pub id: String, pub title: String,
    pub body: String, pub content_type: ContentType,
    pub metadata: serde_json::Value }
```

TinyCortex should implement source validation and reader shapes separately from
OpenHuman's live sync. If readers are ported, they should be replaceable
adapters that return `SourceItem` and `SourceContent`; OpenHuman still owns the
decision to invoke them.

## `memory_sync`: Sync Integration Contracts and Status

OpenHuman's `memory_sync` unifies upstream pull loops across Composio,
workspace sources, and MCP. For TinyCortex, this is an integration contract
rather than an owned production runner. OpenHuman owns cursors, polling,
webhooks, retries, and "when to ingest" policy; TinyCortex accepts the resulting
on-demand ingest call.

```rust
pub enum SyncPipelineKind { Composio, Workspace, Mcp }

pub struct SyncOutcome {
    pub records_ingested: u32,
    pub more_pending: bool,
    pub note: Option<String>,
}

#[async_trait]
pub trait SyncPipeline: Send + Sync {
    fn id(&self) -> &str;
    fn kind(&self) -> SyncPipelineKind;
    async fn init(&self, config: &Config) -> anyhow::Result<()>;
    async fn tick(&self, config: &Config) -> anyhow::Result<SyncOutcome>;
}
```

Layer rule at the integration boundary: OpenHuman's sync module must write through
TinyCortex's ingest/storage contracts. It must not mutate TinyCortex trees
directly except through the documented ingest boundary.

OpenHuman sync status is pull-derived from chunk rows, not a push event log.
TinyCortex should keep status source-of-truth aligned with persisted chunks.

```rust
pub enum FreshnessLabel { Active, Recent, Idle }
pub struct MemorySyncStatus {
    pub provider: String,
    pub chunks_synced: u64,
    pub chunks_pending: u64,
    pub batch_total: u64,
    pub batch_processed: u64,
    pub last_chunk_at_ms: Option<i64>,
    pub freshness: FreshnessLabel,
}
```

Freshness boundaries are active within 30 seconds, recent within 5 minutes, and
idle otherwise.

## `memory_tree`: Tree IO, Registry, Retrieval, and Legacy Shape

OpenHuman has two relevant tree shapes:

- A legacy time hierarchy with node ids such as `root`, `2024`, `2024/03`,
  `2024/03/15`, and `2024/03/15/14`.
- The newer source-tree API where callers append leaf payloads to a registered
  tree and retrieval walks summaries/leaves.

TinyCortex should prioritize the newer source-tree contract.

Canonical write/read contracts:

```rust
pub struct TreeLeafPayload {
    pub chunk_id: String,
    pub token_count: u32,
    pub timestamp: DateTime<Utc>,
    pub content: String,
    pub entities: Vec<String>,
    pub topics: Vec<String>,
    pub score: f32,
}

pub enum TreeLabelStrategy { Inherit, Extract, Empty }

pub struct TreeWriteRequest {
    pub tree_id: String,
    pub tree_kind: TreeKind,
    pub leaf: TreeLeafPayload,
    pub label_strategy: TreeLabelStrategy,
    pub deferred: bool,
}

pub struct TreeReadRequest {
    pub tree_id: String,
    pub start_node_id: Option<String>,
    pub max_depth: u32,
    pub query: Option<String>,
    pub limit: Option<usize>,
}
```

Registry behavior:

- `get_or_create_tree(kind, scope)` is idempotent over `UNIQUE(kind, scope)`.
- SQLite unique-race recovery re-queries and returns the winner.
- Tree ids are prefixed by kind: `source:...`, `topic:...`, `global:...`.
- Summary ids are lexicographically chronological:
  `summary:{13_digit_ms}:L{level}-{8_hex_entropy}`.

Retrieval RPC surfaces:

- `query_source`: optional `source_id`, `source_kind`, `time_window_days`,
  optional natural-language `query`, and `limit`.
- `cover_window`: inclusive `since_ms`/`until_ms`, optional source filters.
- `search_entities`: query plus optional entity kind filter.
- `drill_down`: node id, depth, optional query rerank, optional top-K limit.
- `fetch_leaves`: chunk id list.

Logging invariant: retrieval RPC logs must avoid raw source ids, node ids, and
query text when those can contain PII. Log counts, booleans, lengths, and broad
kind labels.

Legacy time-tree helpers worth preserving as pure utilities:

```rust
pub enum NodeLevel { Root, Year, Month, Day, Hour }
pub fn derive_parent_id(node_id: &str) -> Option<String>;
pub fn level_from_node_id(node_id: &str) -> NodeLevel;
pub fn derive_node_ids(ts: &DateTime<Utc>) -> (String, String, String, String, String);
pub fn node_id_to_path(node_id: &str) -> PathBuf;
```

## `memory_queue`: Async Job Contracts

The queue persists `kind`, JSON payload, status, dedupe key, attempts, and
availability timestamps. Workers branch on `JobKind` and deserialize the
matching payload.

```rust
pub enum JobKind {
    ExtractChunk,
    AppendBuffer,
    Seal,
    FlushStale,
    ReembedBackfill,
    SealDocument,
}

pub enum JobStatus { Ready, Running, Done, Failed, Cancelled }
pub enum JobOutcome { Done, Defer { until_ms: i64, reason: String } }
```

`JobKind::parse` deliberately rejects retired legacy jobs such as
`topic_route` and `digest_daily`. TinyCortex should keep this behavior as an
explicit migration drain path rather than silently treating unknown jobs as noops.

Dedupe patterns:

```rust
ExtractChunkPayload => "extract:{chunk_id}"
AppendBufferPayload => "append:source:{source_id}:{node_ref}"
SealPayload         => "seal:{tree_id}:{level}"
```

LLM-bound jobs are `extract_chunk`, `seal`, `reembed_backfill`, and
`seal_document`; these should pass through a concurrency limiter.

## `memory_diff`: Snapshot and Checkpoint Layer

The diff layer tracks source item state across snapshots and checkpoints.

```rust
pub enum SnapshotTrigger { Auto, Manual }
pub struct Snapshot {
    pub id: String,
    pub source_id: String,
    pub source_kind: String,
    pub label: String,
    pub trigger: SnapshotTrigger,
    pub item_count: u32,
    pub taken_at_ms: i64,
}

pub enum ChangeKind { Added, Removed, Modified }
pub struct ItemChange {
    pub item_id: String,
    pub title: String,
    pub kind: ChangeKind,
    pub old_content_hash: Option<String>,
    pub new_content_hash: Option<String>,
    pub text_diff: Option<String>,
}
```

Higher-level outputs:

```rust
pub struct DiffResult { pub source_id: String, pub summary: DiffSummary,
    pub changes: Vec<ItemChange>, /* plus kind, label, snapshot ids */ }
pub struct Checkpoint { pub id: String, pub label: String,
    pub created_at_ms: i64, pub snapshot_ids: Vec<String> }
pub struct CrossSourceDiff { pub checkpoint_id: Option<String>,
    pub computed_at_ms: i64, pub summary: DiffSummary,
    pub per_source: Vec<DiffResult> }
```

RPC operations to preserve:

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

TinyCortex should keep snapshots independent of the main retrieval index: a
snapshot is a source-state ledger, not a search result.

TinyCortex's git ledger encodes source ids and item ids before using them as
git path/ref components. Public snapshot/change payloads retain the original
logical ids; read markers live under
`refs/openhuman/read/<encoded_source_id>`. Cleanup prunes checkpoint tags and
retains snapshot commits as ledger history.

## `memory_entities` and `memory_graph`

Entities use canonical ids in `<kind>:<value>` form and preserve source handles.

```rust
pub enum EntityKind {
    Person, Organization, Topic, Email, Url, Handle, Hashtag, Location,
    Event, Product, Datetime, Technology, Artifact, Quantity, Misc,
}

pub struct Entity {
    pub id: String,
    pub kind: EntityKind,
    pub display_name: Option<String>,
    pub aliases: Vec<String>,
    pub emails: Vec<String>,
    pub handles: Vec<EntityHandle>,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
}
```

The graph is intentionally not a semantic triple store. It stores weighted
co-occurrence edges:

```rust
pub struct GraphEdge {
    pub subject: String,
    pub object: String,
    pub weight: u32,
}
```

TinyCortex should start with co-occurrence graph semantics and only add
predicate-rich relations after the simpler weighted graph is stable.

## `memory_conversations` and `memory_archivist`

Conversation storage is thread/message oriented and uses camelCase wire fields.

```rust
pub struct ConversationThread {
    pub id: String,
    pub title: String,
    pub chat_id: Option<i64>,
    pub is_active: bool,
    pub message_count: usize,
    pub last_message_at: String,
    pub created_at: String,
    pub parent_thread_id: Option<String>,
    pub labels: Vec<String>,
    pub personality_id: Option<String>,
}

pub struct ConversationMessage {
    pub id: String,
    pub content: String,
    pub message_type: String,
    pub extra_metadata: serde_json::Value,
    pub sender: String,
    pub created_at: String,
}
```

Archivist storage is per-turn and migration-compatible with legacy episodic
FTS rows:

```rust
pub struct ArchivedTurn {
    pub session_id: String,
    pub seq: u32,
    pub timestamp_ms: i64,
    pub role: String,
    pub content: String,
    pub lesson: Option<String>,
    pub tool_calls_json: Option<String>,
    pub cost_microdollars: u64,
}

pub struct Turn {
    pub role: String,
    pub content: String,
    pub tool_calls_json: Option<String>,
    pub timestamp: DateTime<Utc>,
}
```

Boundary rule: conversation history, episodic archive, and tree-ingested
conversation summaries are related but distinct. TinyCortex should not conflate
thread JSONL with tree leaves or with recall citations.

TinyCortex stores per-thread conversation messages at
`threads/<hex(thread_id)>.jsonl` so arbitrary provider thread ids never become
raw path components.

## `memory_goals`: Markdown Goals Document

OpenHuman persists goals as `MEMORY_GOALS.md` with one line per stable id.

```rust
pub struct GoalItem { pub id: String, pub text: String }
pub struct GoalsDoc { pub items: Vec<GoalItem> }
```

Parser/rendering contract:

- Header is `# Long-term Goals`.
- Item line shape is `- [g1] concise goal text`.
- Unknown prose and malformed lines are ignored.
- `add` allocates the next unused `g<N>`.
- `add` and `edit` reject empty, multiline, likely-secret, and likely-PII text.
- `delete` errors on unknown ids.

TinyCortex reflection abstracts the LLM step behind `GoalsGenerator`; direct
goal mutations and reflection share the same mutation lock.

This module is a good early TinyCortex port because it is pure parsing,
rendering, validation, and deterministic id allocation.

## `memory_tools`: Tool-Scoped Rules

Tool memory stores durable operational rules per tool. It is separate from
generic memory and from tool effectiveness metrics.

```rust
pub enum ToolMemoryPriority { Normal, High, Critical }
pub enum ToolMemorySource { UserExplicit, PostTurn, Programmatic }

pub struct ToolMemoryRule {
    pub id: String,
    pub tool_name: String,
    pub rule: String,
    pub priority: ToolMemoryPriority,
    pub source: ToolMemorySource,
    pub tags: Vec<String>,
    pub created_at: String,
    pub updated_at: String,
}
```

Rules are stored under `tool-{tool_name}` namespace using keys
`rule/{rule_id}`. Tool names are trimmed and lowercased. `Critical` and `High`
rules are eager/pinned; `Normal` rules are on-demand.

OpenHuman generates opaque ids that avoid digits and separators:

```rust
pub fn storage_key(id: &str) -> String { format!("rule/{id}") }
pub fn tool_memory_namespace(tool_name: &str) -> String {
    format!("tool-{}", tool_name.trim().to_lowercase())
}
```

TinyCortex should preserve priority ordering: `critical > high > normal`.

## `agent_memory`: Retrieval Agent and Benchmarks

`agent_memory` contains the agent-facing memory loader and retrieval
benchmarking types. It should not own persistence. It should consume the memory
facade and produce citations/context windows.
This is not currently a TinyCortex module; hosts can layer it over the ported
retrieval APIs.

Benchmarking contracts:

```rust
pub struct RetrievalStep {
    pub turn: usize,
    pub action: String,
    pub args_summary: String,
    pub result_preview: String,
    pub elapsed: Duration,
    pub chunks_returned: usize,
    pub bytes_scanned: u64,
}

pub struct WalkBenchmark {
    pub query: String,
    pub namespace: String,
    pub content_root: String,
    pub total_elapsed: Duration,
    pub steps: Vec<RetrievalStep>,
    pub total_turns: usize,
    pub total_chunks_retrieved: usize,
    pub total_bytes_scanned: u64,
    pub answer: String,
    pub stop_reason: String,
}
```

`BenchmarkSummary::from_benchmarks` computes run count, average, p50, p95,
average turns, average chunks, and total bytes scanned. TinyCortex should keep
these as deterministic pure aggregation tests.

## `memory_search`: Agent-Facing Retrieval Tools

`memory_search` consolidates retrieval tools and re-exports lower-level query
tools from `memory::query` and `memory_store::tools`.
TinyCortex does not currently have a `memory_search` module; the retrieval
primitives and scoring profiles live under `src/memory/retrieval/` and
`src/memory/config.rs`.

Tool families:

- `memory_hybrid_search`: graph + vector + keyword + freshness.
- `memory_vector_search`: vector-only retrieval.
- `memory_chunk_context`: surrounding chunk context.
- raw store tools: kinds, raw chunks, raw search.
- tree tools: query source, drill down, fetch leaves, search entities.

Hybrid scoring profiles:

```rust
pub struct WeightProfile {
    pub graph: f64,
    pub vector: f64,
    pub keyword: f64,
    pub freshness: f64,
}

BALANCED    = graph .35, vector .35, keyword .15, freshness .15
SEMANTIC    = graph .15, vector .65, keyword .20, freshness .00
LEXICAL     = graph .25, vector .15, keyword .60, freshness .00
GRAPH_FIRST = graph .55, vector .30, keyword .15, freshness .00
```

Tool argument constraints from `memory_hybrid_search`:

- `query` and `namespace` are required and cannot be blank.
- `mode` is one of `balanced`, `semantic`, `lexical`, `graph_first`.
- `limit` is clamped to `1..=50`.
- `include_breakdown` toggles per-signal score rendering.

TinyCortex should keep scoring profiles data-driven so application clients can
choose retrieval behavior without changing storage.

## Controller Registry Surfaces

The memory controller registry advertises schemas and handlers for the high
level memory tree surface. OpenHuman registers:

`ingest`, `list_chunks`, `get_chunk`, `memory_backfill_status`,
`list_sources`, `search`, `recall`, `entity_index_for`, `chunks_for_entity`,
`top_entities`, `chunk_score`, `delete_chunk`, `delete_source`,
`graph_export`, `obsidian_vault_status`, `vault_health_check`, `flush_now`,
`flush_source`, `wipe_all`, `reset_tree`, `pipeline_status`, `set_enabled`,
`smart_walk`, `doctor`, and `retry_failed`.

TinyCortex should model controller registration as data: schema plus handler.
This avoids coupling CLI, JSON-RPC, and agent tool exposure to separate lists.
The controller registry is deferred; current code provides domain operations
and wire-stable types for future adapters.

## Migration Checklist

1. Port pure enums and structs with serde behavior and parser/render tests.
2. Add namespace normalization and taint handling before any retrieval work.
3. Port goals, tool memory, entities, graph edges, and source validation first.
4. Add storage traits around namespace documents, chunks, trees, vectors, KV,
   and archive records.
5. Implement source reader shapes and optional sync adapters behind the
   `SyncPipeline` trait; keep the live sync runner in OpenHuman.
6. Implement tree registry and tree IO before queue workers.
7. Add queue dedupe keys, status transitions, and deferred outcomes.
8. Add diff snapshots/checkpoints as a separate ledger.
9. Add search scoring profiles and retrieval tool adapters after storage and
   tree traversal are deterministic.
10. Add controller/tool registries last, once module contracts are stable.
