# Persona distillation — quality rubric & first-run scorecard

Companion to `docs/plan/06-persona-distillation.md` (goal **P11**). The rubric is
a **manual, documented** check (not CI-gated): it verifies the compiled
`persona/PERSONA.md` reproduces independently-known preferences of the person,
with a precision bar of *no fabricated preference without citable evidence*.

## How to reproduce a run

```sh
cp .env.example .env          # set OPENROUTER_API_KEY
OPENROUTER_API_KEY=... \
PERSONA_IDENTITY="you@example.com" \
PERSONA_AUTHOR_EMAILS="you@work.com,you@personal.com" \
PERSONA_PROJECT_ROOTS="$HOME/work/oneproject" \
PERSONA_MAX_SESSIONS=40 \
TINYCORTEX_WORKSPACE=./persona-workspace \
cargo run --example persona_harness \
  --features persona,providers-http,git-diff -- backfill
```

`compile` re-assembles the pack from the facet trees with **no** LLM calls;
`incremental` re-processes only changed files; `status` prints cursors + the
last pack path.

## Rubric — known-preference recall

Score each row: ✅ present & correct, ⚠️ partial/weak, ❌ missing or wrong.
The reference preferences below are independently known (from this repo's
`CLAUDE.md`, git history, and working style), not derived from the pack.

| # | Known preference | Facet | Score |
|---|------------------|-------|-------|
| 1 | Small, focused, Conventional-Commit commits | workflow | ✅ verbatim in Directives + Workflow |
| 2 | Always branch off main; never commit to main directly | workflow / directives | ✅ verbatim ("Always branch before writing code. Never commit directly to `main`") |
| 3 | Rust 2021 + `cargo fmt`; 500-LOC module cap | coding_style / directives | ✅ verbatim in Directives |
| 4 | `types.rs` / `<name>_tests.rs` module conventions | coding_style | ✅ verbatim in Directives |
| 5 | Worktrees / subagents for parallel work | workflow / environment | ✅ verbatim ("Use git worktrees when running tasks in parallel") |
| 6 | Terse, directive communication style | communication | ✅ "terse, imperative, goal-oriented … you tell, not ask" |
| 7 | Insists on regression tests alongside changes | coding_style | ✅ testing discipline in Coding style + Directives |
| 8 | Rust / Tauri / agent-harness stack | stack / environment | ✅ "React + Tauri v2 … Rust core embedded as a tokio task"; Claude Code harness |

**Precision check:** every rule in the pack traces to corpus evidence. No
fabricated preferences observed. One caveat: the **Environment** section
generalises session-specific facts (e.g. "macOS 15 … `/home/droid/work/backend-tinyplace`
… 2026-06-11") that came from particular sessions rather than a stable global
truth — evidence-grounded but project-specific, so treat Environment as the
lowest-confidence facet (consistent with its T3-heavy inputs).

## First-run scorecard

First live run over this machine (2026-07-14), OpenRouter + DeepSeek v4 Flash.

- Mode / cap: `backfill`, `PERSONA_MAX_SESSIONS=40` (budget hit → clean checkpoint).
- Sources: 44 units seen — Claude Code + Codex transcripts (oldest-first),
  global `~/.claude/CLAUDE.md` + `~/work/tinycortex/CLAUDE.md`, and the
  tinycortex git repo.
- Sessions digested: 40 → **774 observations** across 7 facets (workflow 187,
  directives 186, coding_style 118, stack 89, anti_preferences 87,
  communication 78, environment 41); 12 verbatim T0 directive rules.
- Provider: **75 requests**, 185,003 prompt + 173,495 completion tokens,
  **$0.063** (chat + embeddings).
- Wall-clock: **~46 min** (DeepSeek v4 Flash is a reasoning model and the map +
  fold calls run sequentially — the dominant cost; a follow-up should batch
  digests concurrently and/or use a non-reasoning fast model).
- Pack size: 6,792 bytes (~1.7k tokens) — under the 10k ceiling; below the 5k
  floor because the capped corpus produced concise facet bodies (the floor is
  aspirational, never padded).
- Rubric recall: **8 / 8 correct**, 0 partial, 0 missing.
- Fabrications: none (Environment section is project-specific but evidence-grounded).

Scaling note: at ~$0.0016/session and this wall-clock, a full backfill of the
machine's ~4,000 sessions is ≈ $6 and many hours sequentially — which is exactly
why the run budgets (§6.7) and `incremental` mode exist. Batch the digest map
concurrently before attempting a full backfill.

## Offline guarantee

The whole pipeline also completes with the deterministic `ConcatSummariser` and
**no network** (degraded quality, zero cost) — covered by
`memory::persona::reduce::tests::full_map_reduce_compile_offline` and the
`compile` subcommand. The mock-LLM end-to-end path (readers → digest → facet
trees → pack, with a cost assertion) is covered by `tests/persona_e2e.rs`.
