---
description: Goals and tool memory — two bounded long-term surfaces above retrieval, with markdown-backed goals and per-tool actionable rules pinned into the system prompt.
---

# Goals & Tool Memory

TinyCortex carries two specialized long-term surfaces that sit above the general
retrieval pipeline. Both are small, deliberately bounded, and meant to be cheap
to read on every turn:

- **Goals** (`src/memory/goals/`) — a compact, human-editable list of the user's
  durable objectives, persisted as a single markdown file (`MEMORY_GOALS.md`).
- **Tool memory** (`src/memory/tool_memory/`) — durable, actionable rules
  attached to a specific tool, stored in per-tool namespaces and pinned into the
  system prompt when safety-critical.

Both were ported from OpenHuman and preserve its wire strings, caps, and
invariants so serialized data stays byte-compatible across the boundary.

---

## Goals

The goals surface maintains an ordered list of long-term objectives the agent
holds when interacting with the user. The whole document is intentionally tiny
(~200–500 tokens) so it stays cheap to read and easy for a human to hand-edit.

### Storage shape

| Property | Value | Source |
| --- | --- | --- |
| File name | `MEMORY_GOALS.md` | `GOALS_FILE` |
| Location | memory workspace root (`MemoryConfig::workspace`) | `goals_path` |
| Max items | **8** | `GOALS_MAX_ITEMS` |
| Max rendered chars | **2000** (≈500 tokens) | `GOALS_FILE_MAX_CHARS` |
| Markdown header | `# Long-term Goals` | `types::HEADER` |

Each line is a [`GoalItem`] (`{ id, text }`) rendered as `- [g1] do the thing`.
Ids are stable short tokens (`g1`, `g2`, …) so `edit`/`delete` can address a
specific line without depending on list order. `GoalsDoc::next_id` allocates the
next free `g<N>`.

```text
# Long-term Goals

- [g1] Ship the v1 memory engine to crates.io
- [g2] Keep weekly cadence on the OpenHuman port
```

Parsing is lenient: `GoalsDoc::parse` only recognizes lines shaped like
`- [id] text`; the header, blank lines, and free prose are ignored, so a
hand-edited file degrades gracefully instead of erroring.

### Mutation invariants

All writes go through `load → mutate → save` in `goals::store`, which
re-enforces every invariant on each write:

- **Single-line text only.** `GoalsDoc::validate_text` trims the text and
  rejects it if empty or if it contains `\n`/`\r`. A newline-bearing goal would
  inject extra `- [..]` list lines on reload and corrupt the stored shape.
- **PII / secret rejection.** The same validator rejects text where
  `has_likely_secret` or `has_likely_pii` (from `memory::store::safety`) fires —
  goals must stay free of credentials and personal data.
- **Cap trimming drops the oldest first.** `enforce_caps` removes from the front
  of the list until both the 8-item and 2000-char caps are satisfied. The
  byte-size pass always leaves at least one item so a single oversized entry
  can't pointlessly empty the file.
- **Missing file = empty document.** `load` returns `GoalsDoc::default()` on
  `NotFound` (first run), and treats other I/O errors as `MemoryError::Io`.
- **Lock serialization.** Every mutation takes a process-wide
  `parking_lot::Mutex` (`goals_mutation_lock`, a `OnceLock<Mutex<()>>`) so
  concurrent user edits and background reflection can't clobber each other's
  load→save sequences.
- **Symlink-escape rejection.** `validate_within_workspace` canonicalizes the
  workspace parent and refuses a path that resolves outside it
  (`MemoryError::PathEscape`). If `MEMORY_GOALS.md` already exists as a symlink,
  its target is canonicalized and must also stay inside the workspace, so a
  hostile link can't read or write across the sandbox boundary.

### Mutation API

Path-based primitives and `MemoryConfig`-rooted wrappers both live in
`goals::store`:

| Operation | Path-based | Config-rooted |
| --- | --- | --- |
| List | `load(workspace)` | `list_for(config)` |
| Add | `add(workspace, text)` → `(id, doc)` | `add_for(config, text)` |
| Edit | `edit(workspace, id, text)` → `doc` | `edit_for(config, id, text)` |
| Delete | `delete(workspace, id)` → `doc` | `delete_for(config, id)` |

`edit`/`delete` return `MemoryError::NotFound` for an unknown id. These back both
the RPC handlers and the agent tools (`goals_list` / `goals_add` / `goals_edit` /
`goals_delete`) in OpenHuman.

### Reflection (`GoalsGenerator`)

Beyond explicit edits, the list can be maintained by a turn-based **reflection**
pass in `goals::reflect`. In OpenHuman this is a real multi-turn `goals_agent`;
TinyCortex never calls a model. The LLM step is abstracted behind a trait:

```text
trait GoalsGenerator {
    fn propose(&self, doc: &GoalsDoc, context: &str, first_run: bool)
        -> Vec<GoalMutation>;
}

enum GoalMutation { Add { text }, Edit { id, text }, Delete { id } }
```

The generator decides *what* should change; the deterministic `reflect` driver
decides *how* it is applied. `reflect(config, context, generator)`:

1. Takes the goals mutation lock and `load`s the current document.
2. Computes `first_run = doc.is_empty()`.
3. Asks the generator to `propose` mutations (informed by `context` and
   `first_run`).
4. Applies them via `apply_mutations`: additions are de-duplicated by
   **normalized** text (trim, lowercase, collapse internal whitespace);
   edits/deletes pass through `GoalsDoc::edit`/`delete`. Every rejected mutation
   (duplicate add, unknown id, invalid text) is counted as `skipped`.
5. `save`s with cap enforcement only if anything was applied; otherwise re-loads
   on-disk truth to avoid a needless rewrite.

The "minimal changes unless empty" rule is preserved: `first_run` switches
`build_prompt` between **initial population** ("the goals list is currently
EMPTY … populate an initial set … max ~8") and **incremental maintenance**
("make the MINIMAL set of changes … do not churn goals that are still valid").
The default `NoopGenerator` proposes nothing, so reflecting a non-empty list is a
no-op — exactly the "no churn unless justified" behavior.

`reflect` returns a `ReflectOutcome { first_run, applied, skipped, summary,
goals }`.

---

## Tool memory

Tool memory is a first-class store for **actionable** tool-specific guidance —
corrections, safety constraints, and learned operational rules — distinct from
per-tool effectiveness statistics and from the generic `global` / `skill-*`
namespaces. Ported from OpenHuman's `memory_tools`.

### Namespace convention

Each tool gets its own namespace `tool-{tool_name}`, built via
`tool_memory_namespace(tool_name)` (which trims and lowercases the name).

```text
tool_memory_namespace("Web_Search")  ->  "tool-web_search"
```

The `tool-` prefix is deliberately distinct from `global`, `skill-…`, and
`tool_effectiveness` so list/clear operations can reason about it unambiguously.
Always build the namespace through the helper — never hard-code the format.
`list_tool_names` enumerates tools by scanning `namespace_summaries` for the
`tool-` prefix, excluding empty names and the `__unscoped__` sentinel (used for
edicts captured before any tool ran).

### Rule shape (`ToolMemoryRule`)

| Field | Type | Notes |
| --- | --- | --- |
| `id` | `String` | Stable within `(tool_name)`; `generate_id()` if absent |
| `tool_name` | `String` | e.g. `email`, `shell`, `web_search` |
| `rule` | `String` | Natural-language guidance for the agent |
| `priority` | `ToolMemoryPriority` | `#[serde(default)]` → `Normal` |
| `source` | `ToolMemorySource` | `#[serde(default)]` → `Programmatic` |
| `tags` | `Vec<String>` | Free-form filters, e.g. `safety`, `permission` |
| `created_at` | `String` | RFC3339; preserved across upserts |
| `updated_at` | `String` | RFC3339; refreshed on every write |

Rules are persisted as JSON keyed by `rule/{id}` (`ToolMemoryRule::storage_key`)
inside the tool namespace, so exact-key lookups stay cheap and never block on an
embedding model.

`generate_id()` is notable: it encodes each byte of a v4 UUID as two lowercase
letters in `a..=p` (one per nibble), prefixed with `r`. The result is a
separator-free, **digit-free** token shaped so it never trips a PII boundary
check when used as a storage key.

### Priorities

`ToolMemoryPriority` derives `Ord` with variants declared low→high, so
`Normal < High < Critical`. Wire strings are `snake_case`.

| Priority | Wire string | Behavior |
| --- | --- | --- |
| `Normal` | `normal` | Available on demand via recall; not eagerly injected |
| `High` | `high` | Surfaced alongside critical rules at tool-selection time |
| `Critical` | `critical` | Pinned into the (compression-resistant) system prompt |

`ToolMemoryPriority::is_eager()` returns true for `Critical | High` — the set
that is both pinned into the system prompt and prefetched at session start so it
survives mid-session context compression.

### Sources

`ToolMemorySource` records provenance so consumers can tell user edicts apart
from auto-captured observations. Wire strings are `snake_case`.

| Source | Wire string | Meaning |
| --- | --- | --- |
| `UserExplicit` | `user_explicit` | User asked the agent to remember this rule |
| `PostTurn` | `post_turn` | Captured from a post-turn observation (failure, repeated correction) |
| `Programmatic` | `programmatic` | Written by another subsystem (e.g. integration provisioner); the default |

### Store API (`ToolMemoryStore`)

`ToolMemoryStore` wraps an `Arc<dyn Memory>` backend (see [Storage Primitives](storage-primitives.md)),
so production code can use a SQLite/file backend while tests use a mock. The
store is `Clone` (the backend is reference-counted). Key async methods:

- `put_rule(rule)` — upsert; preserves `created_at` of an existing
  `(tool_name, id)`, refreshes `updated_at`, and rejects empty `tool_name`/`rule`
  bodies. Generates an id if none was supplied.
- `record(tool, body, priority, source, tags)` — convenience constructor +
  persist.
- `get_rule(tool, id)` / `delete_rule(tool, id)` — exact-key fetch/forget.
- `list_rules(tool)` — all rules for a tool, sorted priority-desc then
  `updated_at`-desc. Malformed JSON entries are **skipped**, not fatal, so one
  corrupt row can't hide a tool's valid safety rules.
- `rules_for_prompt(tools)` — the eager (Critical + High) rules to inject,
  grouped by tool name. An empty `tools` slice scans every known tool namespace.
- `list_rules_json(tool)` — rules as a `serde_json::Value` for RPC envelopes.

### Prompt rendering

`rules_for_prompt` collects eager rules, sorts **Critical first, then High,
freshest first within a priority**, and truncates to `TOOL_MEMORY_PROMPT_CAP`
(**30**) — so Critical rules are always preferred over High when the cap is hit.
This bounds the cache-friendly system-prompt prefix even if many rules accrete
over time; lower-priority rules remain reachable via `list_rules` /
`memory_recall`.

The rendering itself lives in `tool_memory::render`. Critical/High rules belong
in the **system prompt** specifically because mid-session compression rewrites
the rolling chat buffer but never the frozen system prompt — a "never email
Sarah" rule must not be silently dropped when the buffer fills up.

`render_tool_memory_rules(rules)` (and the `ToolMemoryRulesSection` wrapper, which
renders once at construction for byte-stability) produce a deterministic block.
Rendering re-sorts internally — Critical→High, then by tool name, rule body, id —
so output never depends on the caller's ordering. An empty input renders to an
empty string. Otherwise it emits the heading `## Tool-scoped rules`
(`TOOL_MEMORY_HEADING`), a fixed "treat every entry as a hard constraint"
preamble, then a `### \`tool\`` subsection per tool with one bullet per rule:

```text
## Tool-scoped rules

These rules are pinned by the user or by the safety pipeline. Treat every
entry as a hard constraint when considering the matching tool — do not
override them silently. Lower-priority guidance lives in the `tool-{name}`
memory namespace and can be queried via `memory_recall` if needed.

### `email`
- **[critical]** Never send mail to sarah@example.com.
- **[high]** Always BCC the compliance alias on external mail.
```

Each bullet is prefixed with a priority marker: `**[critical]**`, `**[high]**`,
or `**[normal]**`.

{% hint style="info" %}
Critical and High rules live in the system prompt on purpose: mid-session
compression rewrites the rolling chat buffer but never the frozen system prompt,
so a safety rule like "never email Sarah" cannot be silently dropped when the
buffer fills up.
{% endhint %}

---

## See also

- [Storage Primitives](storage-primitives.md)
- [Conversations & Archivist](conversations-and-archivist.md)
- [Core Concepts](core-concepts.md)
- [Architecture Overview](architecture.md)
