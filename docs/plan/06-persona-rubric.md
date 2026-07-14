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
| 1 | Small, focused, Conventional-Commit commits | workflow | _tbd_ |
| 2 | Always branch off main; never commit to main directly | workflow / directives | _tbd_ |
| 3 | Rust 2021 + `cargo fmt`; 500-LOC module cap | coding_style / directives | _tbd_ |
| 4 | `types.rs` / `<name>_tests.rs` module conventions | coding_style | _tbd_ |
| 5 | Worktrees / subagents for parallel work | workflow / environment | _tbd_ |
| 6 | Terse, directive communication style | communication | _tbd_ |
| 7 | Insists on regression tests alongside changes | coding_style | _tbd_ |
| 8 | Rust / TinyCortex / agent-harness stack | stack / environment | _tbd_ |

**Precision check:** scan every rule in the pack — is any preference stated
that has *no* supporting evidence in the corpus? List violations here (target:
zero). _tbd_

## First-run scorecard

Recorded from the first live run over this machine (see also the "First live
run" note appended to doc 06 §6.10).

- Mode / cap: _tbd_
- Sources: _tbd_ Claude Code files, _tbd_ Codex rollouts, instruction files, git.
- Sessions digested / observations: _tbd_
- Provider: _tbd_ requests, _tbd_ tokens, **$_tbd_** (DeepSeek v4 Flash + embeddings).
- Wall-clock: _tbd_
- Pack size: _tbd_ (clamped to `[5k, 10k]` tokens).
- Rubric recall: _tbd_ / 8 correct, _tbd_ partial, _tbd_ missing.
- Fabrications: _tbd_.

## Offline guarantee

The whole pipeline also completes with the deterministic `ConcatSummariser` and
**no network** (degraded quality, zero cost) — covered by
`memory::persona::reduce::tests::full_map_reduce_compile_offline` and the
`compile` subcommand. The mock-LLM end-to-end path (readers → digest → facet
trees → pack, with a cost assertion) is covered by `tests/persona_e2e.rs`.
