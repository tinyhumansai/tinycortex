# Controller and Tool Registry Spec

OpenHuman modules: memory controller schemas, retrieval schemas,
`memory_sources::schemas`, `memory_diff::schemas`, `memory_goals::schemas`,
and agent tools under memory modules.

## Responsibility

The controller registry exposes memory capabilities as JSON-RPC style
operations with discoverable schemas. Agent tools expose selected operations to
LLM agents. TinyCortex must preserve stable names, structured inputs/outputs,
and machine-readable ids.

## Controller Schema Contract

Each controller schema includes:

- namespace.
- function.
- description.
- input fields with type, comment, required flag.
- output fields with type, comment, required flag.
- handler registration matching the schema.

Tests should assert schema lists and registered controllers stay in sync.

## Memory Sources Namespace

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

Retrieval controllers include:

- source query.
- cover window.
- entity search.
- drill down.
- fetch leaves.

Tree/runtime controllers include ingest, list chunks, get chunk, backfill
status, pipeline status, retry failed, enable/disable, and tree runtime controls
for summarizer/engine workflows. Exact active function set should be verified
against current OpenHuman code during port because global/topic tree surfaces
have drifted.

## Memory Read RPC

Read RPC surfaces cover:

- admin state.
- chunk reads.
- entity reads.
- graph reads.
- vault/content reads.

These should be side-effect-light and pagination-aware.

## Agent Tools

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
behavior between tool calls and RPC calls.

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
src/memory/controllers/
src/memory/tools/
```

Port order: schema type definitions, controller registry tests, pure handler
request/response structs, tool name/schema tests, then handler implementations.

