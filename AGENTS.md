# Repository Guidelines

## Project Structure & Module Organization

TinyCortex is a root-level Rust crate. Core source lives in `src/`, integration tests live in `tests/`, and migration/specification docs live under `docs/`. Shared project docs are in `README.md`, `gitbooks/`, `examples/`, `benchmarks/`, and `paper/`.

## Build, Test, and Development Commands

- `cargo fmt --all`: format the Rust crate.
- `cargo test`: run unit and integration tests.
- `cargo check`: quickly validate the crate without running tests.

## Coding Style & Naming Conventions

Use Rust 2021 and standard `cargo fmt` style. Keep public type names direct and domain-specific. Preserve machine-readable ids and enum wire strings when porting contracts from OpenHuman.

Keep modules high-level and cohesive. Avoid letting any source file grow beyond 500 lines of code; split behavior into focused modules before that point. Put all type definitions for a module in a dedicated `type.rs` file inside that module, and keep tests in a separate `test.rs` file rather than mixing tests into implementation files.

Document public APIs, module contracts, and non-obvious behavior thoroughly. Prefer clear module-level docs and item docs over relying on implementation details to explain intent.

## Testing Guidelines

Add focused unit tests beside the module under `src/` and integration tests under `tests/`. Keep tests independent of live services unless explicitly marked and documented.

## Commit & Pull Request Guidelines

Make small commits as much as possible, keeping each commit focused on one coherent change. Recent history uses Conventional Commit-style subjects such as `fix: ...`, `refactor: ...`, and `chore: ...`. Pull requests should summarize the affected module, describe behavior or spec changes, list tests run, and link issues when applicable.

## Parallel Work

Use git worktrees when running tasks in parallel or when launching sub-agents in parallel, so each concurrent effort has an isolated checkout and does not disturb another worktree's files, build output, or branch state.
