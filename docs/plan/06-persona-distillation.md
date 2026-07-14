# 6. Persona Distillation: Coding Style & Personality Memory

Scope: a new ingestion + distillation surface that turns a person's local
coding-agent history (Claude Code, Codex, opencode, Cursor, ChatGPT/Claude.ai
exports), their agent instruction files (`CLAUDE.md` / `AGENTS.md` /
`.cursorrules`), and their git commit history into a durable **persona memory
layer**: personality traits, communication style, coding style, and tool/stack
preferences, compiled into a **5–10k-token context pack** that can be injected
into any LLM so it mimics that person. Everything here builds on existing
seams (`archivist`, flavoured trees, `ChatProvider`/`Summariser`,
`SyncStateStore`, the PII guard) and changes no storage semantics.

"Local-first" here means **within this repo**: the reference pipeline runs
entirely on the local machine, with the only network dependency being an
OpenRouter API key for summarisation (DeepSeek v4 Flash by default). When the
crate is embedded into OpenHuman, OH injects its own LLM routes through the
same `ChatProvider` / `Summariser` traits — the OpenRouter provider is a
repo-local reference implementation, not a hard dependency of the pipeline.

## 6.1 Problem & product goal

Agent harnesses accumulate an extraordinarily high-signal record of how a
person works: what they ask for, how they phrase it, what they correct, what
they merge, and what rules they write down for their agents. Today that record
is scattered across five vendors' on-disk formats and is never folded into
memory.

The goal is a **mimic-grade persona pack**:

- A stable markdown artifact, `persona/PERSONA.md`, of **5–10k tokens**
  (configurable), suitable for verbatim injection into a system prompt.
- Backed by per-facet profiles (`persona/facets/<facet>.md`) that are
  continuously re-distilled as new sessions/commits arrive — the flavoured-tree
  mechanic (`src/memory/tree/flavoured.rs`) already does exactly this for a
  single scope; this plan composes several of them.
- Every claim in the pack traceable to evidence (source kind, session/commit
  id, timestamp, evidence tier).
- Cheap to produce: summarisation uses a small fast model
  (`deepseek/deepseek-v4-flash` via OpenRouter by default; model id is
  config, never hardcoded), with hard per-run and daily cost budgets
  mirroring `DailyBudget` (`src/memory/sync/state.rs:24`).

Non-goals for this doc: hosted-platform delivery, real-time watching of
transcript directories (runs are batch/incremental, invoked by a host or CLI),
and fine-tuning. The pack is a *context* artifact, not a model artifact.

## 6.2 Source inventory & wire formats

Three source families, audited on a real machine (July 2026) so the volume
assumptions below are grounded, not guessed.

### 6.2.1 Family A — agent session transcripts

| Source | Discovery path | Format | Observed volume |
| --- | --- | --- | --- |
| Claude Code | `~/.claude/projects/<project-slug>/<uuid>.jsonl` | JSONL event stream: `type` ∈ `user` / `assistant` / `queue-operation` / …; `message.role` + `message.content` (string or block array), `sessionId`, `timestamp`, `parentUuid`, `isSidechain` | 77 projects, ~2,700 files, ~1.6 GB |
| Codex | `~/.codex/sessions/YYYY/MM/DD/rollout-*.jsonl` | JSONL: `session_meta` (cwd, model, `base_instructions`), `response_item`, `event_msg`, `message`, `function_call` / `function_call_output`, `reasoning` | ~1,400 files, ~4.1 GB |
| opencode | `~/.local/share/opencode/opencode.db` (SQLite, WAL) | `session` / `message` / `part` tables; `message.data` JSON carries `role`, `model.providerID/modelID`; `part.data` JSON carries `type` ∈ `text` / `reasoning` / tool parts | 607 sessions |
| Cursor | `~/Library/Application Support/Cursor/User/workspaceStorage/*/state.vscdb` + `globalStorage/state.vscdb` (SQLite KV) | `ItemTable` key/value JSON blobs (prompt history, composer/chat data); schema is unversioned and vendor-volatile — treat as best-effort | one DB per workspace |
| ChatGPT | user-supplied export: `conversations.json` | mapping-tree of nodes (`message.author.role`, `message.content.parts`, `create_time`) | export-file based |
| Claude.ai / Cowork | user-supplied data export JSON | conversation list with `chat_messages[].sender/text` | export-file based |

Notes that must survive into the implementation:

- Claude Code and Codex transcripts are **event logs, not chat logs** — the
  overwhelming majority of bytes are tool results, reasoning, and system
  scaffolding. The extraction contract (§6.4) keeps user-authored turns and
  drops the rest, which is what makes ~5.7 GB tractable.
- Codex `session_meta.base_instructions` and Claude Code system reminders are
  **vendor prompts, not user evidence** — they must be excluded or the persona
  will absorb the vendor's personality ("You are Codex…").
- opencode and Cursor stores are live SQLite databases owned by other
  processes: open read-only (`SQLITE_OPEN_READ_ONLY`, `immutable=0`), tolerate
  WAL, and never write.
- ChatGPT/Claude.ai exports are **not auto-discovered**; the config takes
  explicit paths.

### 6.2.2 Family B — agent instruction files (T0 evidence)

Explicitly authored rules — the highest-confidence persona evidence there is:

- Global: `~/.claude/CLAUDE.md`, `~/.codex/AGENTS.md`.
- Per-repo: `CLAUDE.md` / `AGENTS.md` at repo roots (20+ across
  `~/work/tinyhumansai/*` on the audited machine), plus `.cursorrules` and
  `.github/copilot-instructions.md` where present.
- Discovery: configured root dirs walked with `walkdir` (existing dep),
  bounded depth, `.git`-containing dirs treated as repo roots for scoping.

### 6.2.3 Family C — git commit history

- Discovery: configured roots scanned for repos (23 under the audited
  `~/work/tinyhumansai`); per-repo `git log` filtered by a configured set of
  author emails (people have several: work, personal, GitHub noreply).
- Extracted per commit: subject, body, timestamp, file-count/insert/delete
  stats, touched paths/extensions; a *sampled* subset of diffs (small commits
  preferred) for style inference. Read via the existing optional `git2` dep
  (`git-diff` feature) — no shelling out.

## 6.3 Canonical evidence model (`src/memory/persona/`)

New module `src/memory/persona/` (repo conventions: `types.rs` for contracts,
`<name>_tests.rs` siblings, every file < 500 LOC), behind a new default-off
Cargo feature `persona` (core stays dependency-light; `persona` itself adds no
new deps — readers use `rusqlite`/`serde_json`/`walkdir` already in core, git
reading requires `git-diff`).

Core types (`persona/types.rs`), serde snake_case like the rest of the crate:

- `PersonaSourceKind` — `claude_code | codex | opencode | cursor |
  chatgpt_export | claude_export | instruction_file | git_history`.
- `EvidenceTier` — confidence ladder used for weighting and conflict
  resolution:
  - `T0` — explicit instruction files (the person wrote the rule down).
  - `T1` — in-transcript corrections/interrupts (the person stopped an agent
    and redirected it — the single highest inference signal).
  - `T2` — user prompt phrasing, slash-command habits, commit-message style.
  - `T3` — inferred from accepted outcomes (merged diffs, un-corrected agent
    output). Lowest confidence; never allowed to override T0–T1.
- `PersonaEvidence` — one unit of evidence: `source` (kind + provenance:
  project/repo, session or commit id, file path), `timestamp`, `tier`,
  `excerpt` (already redacted), `facets: Vec<PersonaFacet>` (assigned by the
  digest step).
- `PersonaFacet` — the seven distillation lenses:
  1. `communication` — tone, verbosity, directness, phrasing quirks, how they
     give feedback.
  2. `coding_style` — naming, structure, comments, error handling, testing
     habits, size discipline.
  3. `stack` — languages, frameworks, libraries, recurring architectural
     choices.
  4. `workflow` — branching/commit granularity, plan-first vs. dive-in, PR
     habits, review strictness, parallelism (worktrees, subagents).
  5. `environment` — editors/harnesses used, CLIs, package managers, OS.
  6. `directives` — explicit standing rules (mostly T0, near-verbatim).
  7. `anti_preferences` — pet peeves: things they correct agents for,
     revert, or explicitly forbid.
- `SessionDigest` — output of the LLM map step (§6.5): structured JSON with
  per-facet observations, each carrying a short supporting quote and tier.

Contracts:

- **Taint**: all persona sources are local, user-owned data →
  `MemoryTaint::Internal`. Export files are still user-owned local files. The
  fail-closed decode rule from `src/memory/types.rs` is untouched.
- **Redaction**: every excerpt passes `redact_pii`
  (`src/memory/store/safety/pii.rs:171`) *before* it is stored or sent to any
  LLM. Transcripts are full of tokens, keys, and email bodies; nothing
  unredacted leaves the reader layer.
- **Determinism**: evidence ids are content-addressed
  (`sha256(source_id ‖ excerpt)[..32]`), mirroring the archivist leaf-id
  convention, so re-runs dedupe naturally.

## 6.4 Extraction strategy per source

The shared principle: **extract the person, discard the machine.** Per family:

### Transcripts (Family A)

Stream-parse (JSONL line-at-a-time; SQLite row cursors) — never load a whole
session file. Keep, per session:

- All **user-authored turns** (Claude Code `type:"user"` with a real user
  message — skipping synthetic tool-result user turns and system-reminder
  content; Codex `message` items with `role:"user"` net of
  `base_instructions`; opencode `part.data.type:"text"` under user-role
  messages; export formats analogously).
- **Interrupt/correction markers** (T1): user messages that arrive mid-turn,
  rejections of proposed actions, "no, do X instead" follow-ups. Claude Code
  marks mid-turn traffic explicitly; Codex interleaves `event_msg`; detection
  heuristics live in the reader, tier assignment in the digest step.
- **Slash-command and tool-habit traces** (T2): which commands/skills the
  person invokes (`/plan`, `/code-review`, custom skills) — habits, not
  content.
- A **thin assistant context**: first ~200 chars of the assistant turn
  immediately preceding each user correction, so the digest model can see what
  was being corrected. Everything else assistant/tool-side is dropped —
  the same clipping philosophy as `archivist::clip::clean_conversation`.
- Session metadata: cwd/project, model, start/end timestamps, turn counts.

Expected reduction: ≥ 95% of raw bytes discarded before the LLM sees anything.

### Instruction files (Family B)

No LLM required. Each file is split into rule-granular chunks (headings /
bullets), normalised into T0 `directives` evidence with `scope: global |
repo(<name>)`. Verbatim text is preserved — these flow into the pack with
minimal rewriting, and repo-scoped rules are labeled so the compiler can keep
universal rules ("small conventional commits") and demote project-local ones
("this repo's crate layout").

### Git history (Family C)

Two evidence streams:

- **Message style** (T2): subjects/bodies in batches of ~100 commits per
  digest call → conventions (Conventional Commits, tense, subject length,
  scope usage, body/no-body habits), cadence, granularity (files/commit,
  insertions distribution).
- **Code style** (T3): sampled small diffs (bounded count and size per repo)
  → naming, comment density, test-alongside-change habits, formatting
  signals. Explicitly T3 — merged code includes agent-written code, so it can
  corroborate but never establish a preference by itself.

## 6.5 Distillation pipeline (map–reduce over flavoured trees)

```
readers (A/B/C) ──▶ PersonaEvidence ──▶ redact ──▶ [map] SessionDigest (LLM)
                                                        │
                                    ┌───────────────────┘
                                    ▼
                 7 × flavoured trees, scope "persona/<facet>"
                 (leaf = digest slice; seal/fold = [reduce], LLM-steered)
                                    │
                                    ▼
                 persona compiler ──▶ persona/PERSONA.md  (5–10k tokens)
                                 └──▶ persona/facets/<facet>.md
```

- **Map**: one `ChatProvider::chat_for_json`
  (`src/memory/score/extract/llm.rs:76`) call per extracted session (or
  commit batch), producing a `SessionDigest`. Prompting follows the
  `LlmEntityExtractor` pattern: strict-JSON instruction, schema in the
  prompt, bounded retries, soft-fallback (a failed digest skips the session,
  never aborts the run). Oversized sessions are windowed and digested in
  parts.
- **Reduce**: each digest is written as a leaf into the matching facet's
  flavoured tree via `TreeFactory::flavoured(scope, ask)`
  (`src/memory/tree/factory.rs:87`). Each facet gets a purpose-written `ask`
  (e.g. `anti_preferences`: *"Distill the things this person dislikes,
  corrects agents for, or forbids — phrased as rules an agent must not
  break."*). The existing seal/fold mechanic re-summarises through
  `prepare_summary_prompt` / `finish_provider_summary`
  (`src/memory/tree/summarise.rs`) and recompiles the root to
  `flavoured/persona-<facet>.md` after each seal — incremental re-distillation
  comes for free.
- **Compile**: a deterministic (non-LLM) compiler assembles the pack:
  1. Header: identity line + top-line trait summary.
  2. `directives` facet near-verbatim (T0 budget-protected first).
  3. Remaining facets in fixed order, trimmed to per-facet token budgets.
  4. An evidence appendix of the strongest T1 quotes (optional, off when the
     5k floor is tight).
  Per-facet budgets extend the existing `TreeConfig::flavour_root_token_budget`
  knob (`src/memory/config.rs:130`); the pack total is clamped to
  `[5_000, 10_000]` tokens by default using the crate's existing token
  estimation.
- **Conflicts**: higher tier wins; within a tier, newer wins; the compiler
  keeps observation counts ("prefers small commits — seen in 14 sessions, 3
  repos") so downstream consumers can judge strength. T3 may only corroborate.

## 6.6 OpenRouter reference provider (`providers/openrouter.rs`)

This closes the long-standing C3/M3 provider seam
(`docs/plan/02-completion-gaps.md` C3) with the crate's first concrete LLM
provider — but strictly as a **reference implementation**:

- `OpenRouterProvider` implements both `ChatProvider` and `Summariser`,
  behind the existing `providers-http` feature (reqwest + rustls, already
  optional deps). The persona pipeline itself depends only on the traits;
  **OpenHuman embeds the pipeline and injects its own LLM routes** — nothing
  in `persona/` may name OpenRouter.
- API: OpenAI-compatible `POST {base}/chat/completions`,
  base default `https://openrouter.ai/api/v1`, model from config
  (`deepseek/deepseek-v4-flash` default), JSON-mode for digests.
- Secrets: `OPENROUTER_API_KEY` read at the edge (harness/example), stored as
  the existing `SecretString` (redacted Debug, `serde(skip)`) — same rules as
  `ComposioSyncConfig`.
- Resilience: retry/backoff on 429/5xx mirroring `ComposioClient`;
  non-retryable classification reuses the strings already handled in
  `llm.rs::is_non_retryable` (402 / "requires more credits" — these are
  OpenRouter's own error shapes).
- Cost: token usage from response `usage`, accumulated per-run and per-day via
  the `DailyBudget` pattern (`src/memory/sync/state.rs:24`); the run aborts
  cleanly (checkpointed) when the budget is hit.
- Tests: wiremock (existing dev-dep) — happy path, JSON-mode digest, 429
  retry, 402 fail-fast, budget cutoff.

## 6.7 Incremental runs & checkpointing

Backfilling 4,000+ sessions must be resumable and re-runnable. Reuse the
`SyncStateStore` pattern (`src/memory/sync/state.rs:13`) with a persona-scoped
namespace (`persona-sync-state`, keyed `<source_kind>:<root>`):

- JSONL sources: per-file cursor `(path, mtime, byte_offset)`; files are
  append-only in practice, so unchanged `(mtime, len)` skips the file.
- SQLite sources: max `rowid` / `time_created` watermark per table.
- Git: last-distilled commit sha per repo (+ author-set hash so changing the
  email list forces re-scan).
- Instruction files: content sha per path — re-digest only on change.
- Dedup: evidence ids are content-addressed (§6.3), so overlapping cursors
  are harmless; digest results are cached by session id + extractor version.
- Modes: `backfill` (walk everything, oldest-first so trees fold
  chronologically) and `incremental` (cursor-forward only). Both honor
  `SyncBudgetConfig`-style caps (max sessions, max LLM calls, max cost per
  run).

## 6.8 CLI harness & config

- `examples/persona_harness.rs`, mirroring `examples/composio_harness.rs`:
  `OPENROUTER_API_KEY=... cargo run --example persona_harness --features
  persona,providers-http,git-diff -- [backfill|incremental|compile|status]`.
  `compile` re-assembles the pack without LLM calls; `status` prints cursors,
  evidence counts per facet/tier, and spend.
- `PersonaConfig` added to `MemoryConfig` (`src/memory/config.rs`), fully
  serde-declarative like everything else: source roots (with sensible
  platform defaults for the five vendors), export file paths, author emails,
  instruction-file glob roots, model id, per-facet asks (defaulted,
  overridable), per-facet + total token budgets, run budgets.
- `.env.example` gains `OPENROUTER_API_KEY`, `TINYCORTEX_LLM_MODEL`,
  `TINYCORTEX_PERSONA_ROOTS`.

## 6.9 Output & injection contract

- Stable paths under the workspace: `persona/PERSONA.md` (the pack),
  `persona/facets/<facet>.md` (per-facet roots), `persona/evidence/` (redacted
  evidence store, markdown + frontmatter like the content store).
- The pack's markdown shape is itself a wire contract (hosts inject it
  verbatim): H1 identity header, `## Directives`, `## Communication style`,
  `## Coding style`, `## Stack`, `## Workflow`, `## Environment`,
  `## Anti-preferences`, each a flat rule list with strength annotations.
- Export helper `persona::export`: emits the pack (a) as a system-prompt
  block, (b) as CLAUDE.md/AGENTS.md-style directives (so the loop closes: the
  distilled persona can be written back as instruction files), with token
  accounting reported for both.
- OpenHuman integration: OH constructs the pipeline with its own
  `ChatProvider`/`Summariser` and reads `persona/PERSONA.md` from the
  workspace — no new RPC surface needed; add an `X*` row to doc 05 when wired.

## 6.10 Verification & evaluation

- **Fixtures**: `tests/fixtures/persona/` — small, hand-redacted sample files
  for each format (Claude Code JSONL, Codex JSONL, opencode DB built in-test
  via rusqlite, Cursor KV DB, ChatGPT `conversations.json`, Claude export,
  instruction files, a synthetic git repo built with `git2` in a tempdir).
- **Reader unit tests** (`<name>_tests.rs` siblings): exact extraction
  counts, vendor-prompt exclusion, redaction applied, deterministic evidence
  ids.
- **Mock-LLM e2e** (wiremock): backfill over all fixtures → digests → facet
  trees → compiled pack; assert pack structure, token clamp, tier ordering,
  cursor resume after a mid-run abort, budget cutoff.
- **Offline fallback**: the whole pipeline must complete with the
  deterministic `ConcatSummariser` and no network — degraded quality, zero
  cost (same guarantee flavoured trees already make).
- **Quality rubric** (live, manual, documented not CI-gated): run against the
  real machine and check the pack reproduces independently-known preferences —
  e.g. small conventional commits, feature-branch discipline, Rust 2021 +
  `cargo fmt`, 500-LOC module cap, `types.rs`/`_tests.rs` conventions,
  worktrees for parallel work. Precision target: no fabricated preference
  without citable evidence.
- **Cost assertion**: mock e2e asserts total request count and estimated
  spend stay under the configured run budget.

## Goals

ID namespace: `P*` (persona). Same `/goal` shape as the rest of this folder;
work each goal on its own branch with small conventional commits, `cargo fmt`
+ `cargo test` green at every checkpoint.

### P1 — Persona evidence model & facet taxonomy
Status: todo
Depends-on: -
Definition of done: `src/memory/persona/` exists behind a default-off
`persona` feature with `types.rs` defining `PersonaSourceKind`,
`EvidenceTier`, `PersonaFacet`, `PersonaEvidence`, `SessionDigest`
(serde snake_case, content-addressed evidence ids), redaction wired through
`redact_pii`, unit-tested.

- [ ] Module skeleton + `persona` feature in `Cargo.toml` (no new deps).
- [ ] Types + id derivation (`sha256(source_id ‖ excerpt)[..32]`) + tests.
- [ ] Redaction boundary: constructor-enforced (evidence cannot be built from
      unredacted text) + tests with seeded secrets/emails.
- [ ] Facet asks: default `ask` string per facet, overridable via config.

### P2 — Claude Code + Codex JSONL readers
Status: todo
Depends-on: P1
Definition of done: streaming readers for `~/.claude/projects/**.jsonl` and
`~/.codex/sessions/**/rollout-*.jsonl` that emit `PersonaEvidence` — user
turns, interrupts (T1), slash-command traces, thin assistant context —
excluding vendor prompts/system reminders; ≥95% byte reduction demonstrated on
fixtures.

- [ ] Line-streaming JSONL parser tolerant of unknown event types.
- [ ] Claude Code: user-turn extraction, synthetic-turn/system-reminder
      exclusion, sidechain (`isSidechain`) exclusion, interrupt detection.
- [ ] Codex: `message`/`event_msg` extraction, `base_instructions` exclusion,
      session_meta (cwd/model) → provenance.
- [ ] Fixtures + reader tests (counts, exclusions, ids, redaction).

### P3 — opencode + Cursor SQLite readers
Status: todo
Depends-on: P1
Definition of done: read-only rusqlite readers for `opencode.db`
(session/message/part) and Cursor `state.vscdb` KV stores that emit
`PersonaEvidence`, safe against live/WAL databases, best-effort on Cursor's
unversioned schema (unknown keys skipped, never fatal).

- [ ] opencode: join session→message→part, user text parts only, model/agent
      metadata → provenance.
- [ ] Cursor: `ItemTable` scan for prompt/composer keys, defensive JSON
      decoding.
- [ ] Read-only open flags + in-test fixture DBs built with rusqlite.

### P4 — ChatGPT + Claude.ai export readers
Status: todo
Depends-on: P1
Definition of done: readers for user-supplied `conversations.json` (ChatGPT
mapping-tree) and Claude.ai export JSON that emit user-turn `PersonaEvidence`;
paths come from config only (no auto-discovery).

- [ ] ChatGPT mapping-tree walk (linearise by `create_time`, user turns only).
- [ ] Claude export `chat_messages` extraction.
- [ ] Fixtures + tests.

### P5 — Instruction-file reader & rule normaliser
Status: todo
Depends-on: P1
Definition of done: discovery + parsing of `CLAUDE.md`, `AGENTS.md`,
`.cursorrules`, `.github/copilot-instructions.md` across configured roots into
rule-granular T0 `directives` evidence with global-vs-repo scope; verbatim
text preserved.

- [ ] Walkdir discovery (repo-root detection via `.git`), configured globs.
- [ ] Heading/bullet rule splitter; scope labeling.
- [ ] Change detection by content sha (feeds P9).

### P6 — Git-history reader
Status: todo
Depends-on: P1
Definition of done: `git2`-based reader (requires `git-diff` feature) over
configured repo roots, author-filtered by a configured email set, emitting
commit-message-style batches (T2) and bounded sampled-diff evidence (T3).

- [ ] Repo discovery + multi-email author matching.
- [ ] Message-batch evidence (subject/body/stats) — ~100 commits per unit.
- [ ] Diff sampling policy (small commits first, hard size/count caps).
- [ ] Synthetic-repo fixture tests.

### P7 — OpenRouter reference provider
Status: todo
Depends-on: -
Definition of done: `providers/openrouter.rs` behind `providers-http`
implementing `ChatProvider` + `Summariser` against an OpenAI-compatible
endpoint, with `SecretString` key handling, retry/backoff, non-retryable
classification, `DailyBudget`-style cost tracking, wiremock-tested; the first
concrete provider closing the C3 seam. Nothing under `persona/` references it.

- [ ] Client + config (base url, model id, timeouts) + JSON-mode support.
- [ ] Retry/backoff (429/5xx) and fail-fast (401/402/403) paths.
- [ ] Usage/cost accounting + budget cutoff.
- [ ] Wiremock suite (happy, retry, fail-fast, budget).

### P8 — Distillation pipeline & persona compiler
Status: todo
Depends-on: P1, P7
Definition of done: map step (`SessionDigest` via `chat_for_json`, windowing,
soft-fallback), reduce step (7 facet flavoured trees via
`TreeFactory::flavoured`), and deterministic compiler emitting
`persona/PERSONA.md` clamped to the configured 5–10k token budget with tier
ordering, conflict resolution (tier > recency; T3 corroborates only), and
observation counts; mock-LLM e2e green including the `ConcatSummariser`
offline path.

- [ ] Digest prompt + strict-JSON schema + windowing + digest cache.
- [ ] Facet-tree ingestion + seal-driven re-distillation.
- [ ] Compiler: fixed section order, per-facet budgets, total clamp,
      evidence appendix toggle.
- [ ] Conflict/count semantics + tests.

### P9 — Checkpointing, incremental state & run budgets
Status: todo
Depends-on: P2, P8
Definition of done: persona-scoped `SyncStateStore` state (JSONL
path/mtime/offset cursors, SQLite watermarks, git shas, instruction-file
shas), `backfill` and `incremental` modes, resume-after-abort proven in the
mock e2e, run capped by max-sessions/max-calls/max-cost.

- [ ] Cursor types + persistence under `persona-sync-state`.
- [ ] Oldest-first backfill ordering; cursor-forward incremental.
- [ ] Abort/resume e2e + budget-cutoff checkpoint test.

### P10 — CLI harness, config & first local run
Status: todo
Depends-on: P2, P5, P6, P8
Definition of done: `examples/persona_harness.rs`
(`backfill|incremental|compile|status`), `PersonaConfig` in `MemoryConfig`
with platform-default source roots and per-facet asks, `.env.example`
updated, and a documented first full run over the three launch sources
(transcripts + instruction files + git) producing a real `persona/PERSONA.md`.

- [ ] Harness subcommands + progress/spend reporting.
- [ ] `PersonaConfig` (+ serde tests, defaults).
- [ ] `.env.example` + README/gitbooks pointer.
- [ ] Recorded first-run notes (volume, cost, wall-clock) appended to this
      doc.

### P11 — Verification fixtures, e2e & quality rubric
Status: todo
Depends-on: P8
Definition of done: `tests/fixtures/persona/` covering every source format,
mock-LLM end-to-end integration test under `tests/`, offline
`ConcatSummariser` path exercised, cost assertion, and the manual quality
rubric documented with results from the first live run.

- [ ] Fixture set (hand-redacted, small, license-clean).
- [ ] `tests/persona_e2e.rs` (`required-features = ["persona",
      "providers-http", "git-diff"]`, wiremock).
- [ ] Rubric doc + first-run scorecard.

## Suggested ordering (dependency-aware)

1. **P1 + P7** in parallel — the model and the provider are independent.
2. **P2 + P5 + P6** — the three launch sources ("3–4 source" milestone:
   agent transcripts, instruction files, git history).
3. **P8** — pipeline + compiler; first real pack.
4. **P9 → P10** — make it resumable, then runnable end-to-end by a human.
5. **P11** — lock quality; **P3/P4** (opencode/Cursor/exports) as follow-on
   sources once the pack quality is proven.
