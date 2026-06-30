# Controller and Tool Registry Spec

OpenHuman modules: memory controller schemas, retrieval schemas,
`memory_sources::schemas`, `memory_diff::schemas`, `memory_goals::schemas`,
and agent tools under memory modules.

## Responsibility

The OpenHuman controller registry exposes memory capabilities as JSON-RPC style
operations with discoverable schemas. Agent tools expose selected operations to
LLM agents. TinyCortex currently provides the underlying domain operations and
wire-stable data types, but does not yet include a `src/memory/controllers/` or
`src/memory/tools/` registry. When those host-facing adapters are added, they
must preserve stable names, structured inputs/outputs, and machine-readable
ids.

## Controller Schema Contract

Each OpenHuman controller schema includes:

- namespace.
- function.
- description.
- input fields with type, comment, required flag.
- output fields with type, comment, required flag.
- handler registration matching the schema.

When TinyCortex ports these adapters, tests should assert schema lists and
registered controllers stay in sync.

## Memory Sources Namespace

Status in TinyCortex: source entry types, validation, local folder/conversation
readers, and a TOML-backed registry are ported. Production sync runners,
provider CRUD controllers, cost estimators, and audit-log RPCs are
OpenHuman/host-owned surfaces.

Namespace: `memory_sources`.

Functions:

- `list`
- `get`
- `add`
- `update`
- `remove`
- `list_items`
- `read_item`
- `sync`
- `reconcile`
- `status_list`
- `supported_toolkits`
- `sync_audit_log`
- `estimate_sync_cost`
- `monthly_cost_summary`
- `apply_all_in`

Inputs include the flattened source fields used by `MemorySourceEntry`:
`toolkit`, `connection_id`, `path`, `glob`, `url`, `branch`, `paths`,
`max_commits`, `max_issues`, `max_prs`, `query`, `since_days`, `max_items`,
`selector`, budgets, and `sync_depth_days`.

## Memory Diff Namespace

Status in TinyCortex: snapshot, diff, checkpoint, read-marker, and cleanup
domain operations are ported on the git-backed ledger. JSON-RPC schema
registration is not ported.

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

Outputs must preserve `Snapshot`, `DiffResult`, `CrossSourceDiff`,
`Checkpoint`, and count values.

## Memory Goals Namespace

Status in TinyCortex: markdown-backed goals store and deterministic reflection
driver are ported. JSON-RPC schema registration and LLM tool wrappers are host
surfaces.

Namespace: `memory_goals`.

Functions:

- `list`
- `add`
- `edit`
- `delete`
- `reflect`

Reflect accepts optional context and returns whether the enrichment agent ran,
a summary string, and the resulting goals document.

## Memory Tree and Retrieval Namespaces

Status in TinyCortex: retrieval primitives are ported as Rust APIs returning
wire-stable data shapes. Controller schema registration is not ported.

Retrieval controllers should include:

- source query.
- global query reconstructed from source-tree summaries.
- topic query reconstructed from entity-index hits.
- cover window.
- entity search.
- drill down.
- fetch leaves.

Tree/runtime controllers include ingest, list chunks, get chunk, backfill
status, pipeline status, retry failed, enable/disable, and tree runtime controls
for summarizer/engine workflows. Standalone global/topic tree jobs are retired
in TinyCortex; old `topic_route` and `digest_daily` rows are skipped/purged by
the queue.

## Memory Read RPC

Read RPC surfaces cover:

- admin state.
- chunk reads.
- entity reads.
- graph reads.
- vault/content reads.

These should be side-effect-light and pagination-aware.

## Agent Tools

Status in TinyCortex: the durable tool-memory rule store and prompt renderer
are ported. Concrete agent-facing tool wrapper structs are not ported.

Known agent-facing memory tools include:

- `MemoryTreeTool`
- `MemoryTreeQuerySourceTool`
- `MemoryTreeCoverWindowTool`
- `MemoryTreeSearchEntitiesTool`
- `MemoryTreeDrillDownTool`
- `MemoryTreeFetchLeavesTool`
- `MemoryTreeIngestDocumentTool`
- `MemoryDiffTool`
- `GoalsListTool` -> `goals_list`
- `GoalsAddTool` -> `goals_add`
- `GoalsEditTool` -> `goals_edit`
- `GoalsDeleteTool` -> `goals_delete`
- `MemoryToolsListTool`
- `MemoryToolsPutTool`
- memory recall/store/forget/doctor tools under `memory/tools`.

Tools should use the same domain operations as RPC handlers. Avoid divergent
behavior between tool calls and RPC calls when the adapter layer is added.

## Output Requirements

Tool and RPC outputs must retain:

- source ids and labels.
- snapshot ids and checkpoint ids.
- chunk ids and summary ids.
- tree ids and node ids.
- counts and pagination metadata.
- score and score breakdown fields.
- taint/provenance.
- logs or stage events when operations are async.

## TinyCortex Landing Area

```text
src/memory/controllers/   # deferred host adapter layer
src/memory/tools/         # deferred agent-tool adapter layer
```

Port order: schema type definitions, controller registry tests, pure handler
request/response structs, tool name/schema tests, then handler implementations.
