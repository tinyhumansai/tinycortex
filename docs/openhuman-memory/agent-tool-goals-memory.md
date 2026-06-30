# Agent, Tool, and Goals Memory Spec

OpenHuman modules: `agent_memory`, `memory_tools`, `memory_goals`.

## Agent Memory

`agent_memory` owns the specialist memory retrieval agent, its prompt, and
performance instrumentation. It is a consumer of retrieval tools, not a storage
owner. TinyCortex does not currently include a `src/memory/agent/` module; the
agent-memory details below are OpenHuman integration context for hosts that
drive the retrieval primitives.

Retrieval strategies:

- vector search.
- keyword search over raw content files.
- entity search.
- hierarchical tree browse.
- direct content reads.
- source listing.

Expected content layout:

```text
memory_tree/content/
  chat/
  episodic/
  raw/
  wiki/summaries/
```

Agent definition files:

- `agent.toml`: tool allowlist, model hint, iteration cap.
- `prompt.md`: prompt archetype.
- `prompt.rs`: dynamic prompt builder.

Performance tooling should preserve benchmark hooks for memory walk latency and
quality.

## Tool Memory

`memory_tools` stores durable per-tool rules. Namespace convention:

```text
tool-{tool_name}
```

`ToolMemoryRule` fields:

- `id`
- `tool_name`
- rule text
- priority
- source
- tags
- `created_at`
- `updated_at`

Priorities:

- normal
- high
- critical

Sources:

- user explicit.
- post-turn capture.
- programmatic.

Store operations:

- put rule.
- get rule.
- list rules.
- delete rule.
- render prompt rules.
- list tool names.
- record/list as JSON.

Rules are stored as JSON under key `rule/{rule_id}` in namespace
`tool-{tool_name}`, where the namespace must be built through the helper that
trims and lowercases tool names. Upserts preserve `created_at`, refresh
`updated_at`, skip malformed rows during list, and ignore the unscoped sentinel
namespace when listing real tool names.

Prompt rendering pins critical/high rules under `## Tool-scoped rules` with a
global cap of 30 eager rules. Rendering is byte-stable: critical before high,
then tool name, rule body, and id. Post-turn capture and agent tool wrappers
(`MemoryToolsListTool`, `MemoryToolsPutTool`) are OpenHuman/host surfaces; the
current TinyCortex module provides the rule store and renderer they would call.

## Goals Memory

`memory_goals` stores a compact long-term goal list at:

```text
<workspace>/MEMORY_GOALS.md
```

Markdown shape:

```markdown
# Long-term Goals

- [g1] ship the desktop app
```

`GoalItem` fields:

- stable short id such as `g1`.
- one-line goal text.

`GoalsDoc` is ordered. Order is meaningful for rendering and cap trimming.

Constraints:

- max 8 items.
- max rendered size 2,000 chars.
- multiline goal text rejected.
- empty goal text rejected.
- likely secrets or PII rejected.
- unknown edit/delete id rejected.
- cap trimming drops oldest items first.
- load missing file as empty document.
- parser ignores unrecognized or malformed lines so hand-edited files degrade
  gracefully.
- storage path must stay inside workspace and reject symlink escapes.
- mutations serialize through a lock.
- ids are allocated as the next unused `g<N>` id.

OpenHuman explicit surfaces:

- RPC: `memory_goals_list`, `memory_goals_add`, `memory_goals_edit`,
  `memory_goals_delete`, `memory_goals_reflect`.
- tools: `goals_list`, `goals_add`, `goals_edit`, `goals_delete`.

TinyCortex exposes `MemoryConfig`-rooted store wrappers (`list_for`, `add_for`,
`edit_for`, `delete_for`) plus the deterministic reflection driver.

Reflection:

- Agent id: `goals_agent`.
- Uses goals tools plus memory recall.
- First run populates up to about 8 durable goals.
- Maintenance runs make minimal justified changes.
- Automatic runs are best-effort background tasks on summarization/context
  close; on-demand reflect waits and returns result.

TinyCortex abstracts the LLM portion behind `GoalsGenerator`. Reflection loads
the current file under the same mutation lock used by direct edits, computes
`first_run` from emptiness, asks the generator for `add`/`edit`/`delete`
mutations, deduplicates additions by normalized text, skips invalid or unknown
mutations, and only saves when a mutation was actually applied.

## Required Invariants

- Tool memory rules must survive prompt compression through critical/high prompt
  rendering.
- Tool rule namespaces must be built by helper, not hard-coded ad hoc.
- Goals are durable, concise, and free of secrets/PII.
- Background reflection failure must not corrupt or block the caller.
- Agent memory must not mutate storage except through explicit tools.
- Direct goal edits and reflection must share the same lock so background
  reflection cannot clobber user edits.

## TinyCortex Landing Area

```text
src/memory/tool_memory/
src/memory/goals/
```

Port order: goals parse/render/cap/path tests, tool rule types/store trait,
prompt rendering, host capture hooks, agent-memory tool contracts.
