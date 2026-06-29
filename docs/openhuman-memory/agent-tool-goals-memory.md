# Agent, Tool, and Goals Memory Spec

OpenHuman modules: `agent_memory`, `memory_tools`, `memory_goals`.

## Agent Memory

`agent_memory` owns the specialist memory retrieval agent, its prompt, and
performance instrumentation. It is a consumer of retrieval tools, not a storage
owner.

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

Prompt rendering pins critical/high rules under `## Tool-scoped rules` with a
cap. Post-turn capture detects user edicts and repeated tool failures. Agent
tools are `MemoryToolsListTool` and `MemoryToolsPutTool`.

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
- unknown edit/delete id rejected.
- cap trimming drops oldest items first.
- load missing file as empty document.
- storage path must stay inside workspace and reject symlink escapes.
- mutations serialize through a lock.

Explicit surfaces:

- RPC: `memory_goals_list`, `memory_goals_add`, `memory_goals_edit`,
  `memory_goals_delete`, `memory_goals_reflect`.
- tools: `goals_list`, `goals_add`, `goals_edit`, `goals_delete`.

Reflection:

- Agent id: `goals_agent`.
- Uses goals tools plus memory recall.
- First run populates up to about 8 durable goals.
- Maintenance runs make minimal justified changes.
- Automatic runs are best-effort background tasks on summarization/context
  close; on-demand reflect waits and returns result.

## Required Invariants

- Tool memory rules must survive prompt compression through critical/high prompt
  rendering.
- Tool rule namespaces must be built by helper, not hard-coded ad hoc.
- Goals are durable, concise, and free of secrets/PII.
- Background reflection failure must not corrupt or block the caller.
- Agent memory must not mutate storage except through explicit tools.

## TinyCortex Landing Area

```text
src/memory/agent/
src/memory/tool_memory/
src/memory/goals/
```

Port order: goals parse/render/cap/path tests, tool rule types/store trait,
prompt rendering, capture hooks, agent-memory tool contracts.

