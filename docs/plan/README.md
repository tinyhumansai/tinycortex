# TinyCortex Completion & OpenHuman Integration Plan

This folder is the working plan for turning TinyCortex from a ported engine
into a shipping, pluggable memory library that OpenHuman consumes. It is based
on a July 2026 audit of both codebases:

- **TinyCortex** (`this repo`): all 18 `src/memory/` modules ported and green —
  1060 unit tests + 1 smoke test pass, zero `todo!()`/`unimplemented!()`. The
  gaps are the *top layer* (no engine facade, no controller/tool registries, no
  real LLM/embedding providers) and the *proof layer* (one integration test, no
  examples, no runnable benchmarks).
- **OpenHuman** (`../openhuman-1`, branch `main` @ `4c98a31`): memory stack is a
  clean downward dependency chain `memory` (orchestration) → `memory_tree`
  (mechanics) → `memory_store` (persistence), with `memory_queue`,
  `embeddings`, `memory_archivist`, `memory_entities` alongside. Nothing in the
  storage core imports upward. Four coupling axes need abstraction: `Config`,
  `ChatProvider` (LLM), `EmbeddingProvider`, and app services
  (`scheduler_gate`, `observability`, event bus, `credentials`).

**Important**: TinyCortex was ported from the older `openhuman-workflow`
checkout. `openhuman-1` is a different (current) checkout — a drift audit is
the first migration goal, not an afterthought.

## Plan documents

| Doc | Covers |
| --- | --- |
| [01-migration-library.md](01-migration-library.md) | What remains to migrate from OpenHuman into the library, drift audit, host-hook traits, wire-format invariants |
| [02-completion-gaps.md](02-completion-gaps.md) | Missing pieces inside TinyCortex: engine facade, providers, registries, features, docs/examples |
| [03-testing-benchmarks.md](03-testing-benchmarks.md) | Unit + e2e testing strategy, retrieval-effectiveness harness, benchmarking |
| [04-openhuman-integration.md](04-openhuman-integration.md) | Pluggability design and phased, non-breaking adoption inside OpenHuman |

## Goal format (`/goal` compatible)

Every goal in these docs follows one machine-readable shape so a goal-driven
workflow can pick them up directly:

```markdown
### <ID> — <title>
Status: todo | in-progress | done | blocked
Depends-on: <IDs or "-">
Definition of done: <one/two sentences, verifiable>

- [ ] task
- [ ] task
```

ID namespaces: `M*` migration, `C*` crate completion, `T*` testing/benchmarks,
`I*` OpenHuman integration. Work each goal on its own feature branch with
small conventional commits, and keep `cargo fmt` + `cargo test` green at every
checkpoint.

## Suggested ordering (dependency-aware)

1. **M1** drift audit → everything else keys off the true source of truth.
2. **C1** engine facade + **C2** Cargo features — unblocks examples, e2e tests,
   and the OpenHuman adapter simultaneously.
3. **M2/M3** host-hook traits + provider extraction, **C3** real providers.
4. **T1–T3** e2e + effectiveness + benches (start T1 as soon as C1 lands).
5. **I1–I4** phased OpenHuman adoption (shadow → leaf swaps → core swap).
