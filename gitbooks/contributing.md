---
description: How to build, test, format, document, and contribute to the TinyCortex Rust crate, plus repository conventions and running the benchmark suite.
---

# Building & Contributing

This page covers how to build, test, format, and document the **TinyCortex** Rust crate (`tinycortex` on crates.io), the repository layout conventions contributors are expected to follow, the commit/PR style, and how to run the benchmark suite that lives alongside the crate.

TinyCortex is a single root-level Rust library crate. There is no hosted service, server binary, or language SDK to build here — everything in `src/` compiles into one plain Rust library (`rlib`) that host applications (OpenHuman or your own) embed as a Cargo dependency.

## Prerequisites

You need a stable Rust toolchain with Rust 2021 edition support. Install via [rustup](https://rustup.rs/) and confirm `cargo` is on your `PATH`.

The crate pulls in one native dependency by default, so a working C toolchain is required for the first build:

| Dependency | When | Why it needs a native build |
| --- | --- | --- |
| `rusqlite` (with the `bundled` feature) | always | compiles a vendored SQLite from C; no system SQLite needed |
| `git2` | only with the `git-diff` feature | links against libgit2 for the git-backed [Diff Layer](diff-layer.md) |

Because `rusqlite` is configured with `bundled`, you do **not** need a system SQLite install — but you do need a C compiler (`cc`/clang) available for the bundled build. `git2` is **optional and off by default**; a plain `cargo build` never compiles it. The full dependency set is declared in `Cargo.toml` at the repo root.

### Cargo features

The default feature set is empty (`default = []`), and several whole modules are compiled out unless you enable their feature:

| Feature | Gates | What it enables |
| --- | --- | --- |
| `tokio` | `memory::queue::runtime` | async background worker loops for the job queue (turns `tokio` into a real dependency) |
| `git-diff` | `memory::diff` | the git-backed diff ledger, snapshots, checkpoints, and read markers (pulls in `git2`/libgit2) |
| `providers-http` | `memory::providers` | HTTP-backed embedding/LLM providers |
| `rpc` | `memory::rpc` | the RPC surface |

A bare `cargo test` therefore silently skips the `diff`, `providers`, `rpc`, and `queue::runtime` modules. CI runs `cargo test --all-features` plus a per-feature matrix — to test everything the way CI does, run:

```bash
cargo test --all-features
```

## Build, test, and check

The three everyday commands (also listed in `AGENTS.md`):

```bash
cargo check        # quickly validate the crate without running tests
cargo test         # run unit tests (in src/) and integration tests (in tests/)
cargo fmt --all    # format the crate with standard rustfmt style
```

`cargo check` is the fast inner-loop command: it type-checks and borrow-checks without producing a final binary, which is the quickest way to confirm a change compiles. Use `cargo build` (or `cargo build --release`) only when you actually need the compiled artifact.

### Testing

```bash
cargo test                 # everything: unit + integration tests
cargo test --lib           # only the unit tests embedded in src/
cargo test --test <name>   # a single integration test file under tests/
cargo test <substring>     # only tests whose name matches the substring
```

Test layout follows two conventions:

- **Unit tests** live next to the module they cover, in a dedicated `<name>_tests.rs` file per implementation file (e.g. `store.rs` + `store_tests.rs`) — not inline at the bottom of the implementation file.
- **Integration tests** live under the top-level `tests/` directory and exercise the crate through its public API.

Dev-only dependencies used by tests are declared under `[dev-dependencies]` in `Cargo.toml`:

| Dev dependency | Used for |
| --- | --- |
| `tempfile` | scratch directories for storage/diff tests so nothing touches the real filesystem state |
| `tokio` (`macros`, `rt-multi-thread`) | driving the `async` surface (the `Memory` trait is `async`, via `async-trait`) in tests |

Keep tests independent of live or networked services unless a test is explicitly marked and documented as requiring them. Most tests should run fully offline against in-memory or temp-dir fixtures.

### Generating docs

The crate documents its public APIs, module contracts, and non-obvious behavior with rustdoc. Build and open the docs locally with:

```bash
cargo doc --no-deps --open   # build docs for this crate only, open in a browser
```

Drop `--no-deps` if you want the rendered docs for dependencies too. When adding or changing public items, write module-level (`//!`) and item-level (`///`) docs that explain intent and contracts rather than leaving readers to infer behavior from the implementation.

## Repository layout

| Path | Contents |
| --- | --- |
| `src/` | the Rust crate source — all library code |
| `tests/` | integration tests that drive the public API |
| `docs/` | migration and specification docs (OpenHuman port notes) |
| `benchmarks/` | the retrieval-effectiveness harness (`effectiveness/`) plus reported platform evaluation results |
| `examples/` | example notebooks and scenarios |
| `gitbooks/` | long-form prose documentation |
| `paper/` | research paper sources |
| `README.md` | top-level project overview |

Note that `Cargo.toml` deliberately **excludes** the non-Rust directories (`.github/`, `.vscode/`, `benchmarks/`, `docs/`, `examples/`, `gitbooks/`, `paper/`, and `requirements.txt`) from the published crate via its `exclude` list, so the package shipped to crates.io contains only the library and what it needs to compile.

### Module conventions

Modules are organized to stay high-level and cohesive. Three rules from `AGENTS.md` are enforced by convention in review:

1. **`types.rs` for types.** All type definitions for a module go in a dedicated `types.rs` file inside that module directory.
2. **`<name>_tests.rs` for tests.** Each implementation file gets its own sibling test file (`store_tests.rs`, `types_tests.rs`, …), not tests mixed into implementation files.
3. **~500-line guideline.** Avoid letting a source file grow much beyond ~500 lines of code; split behavior into focused submodules before that point. (A couple of legacy files still exceed this — new code shouldn't add more.)

A typical engine module (for example under `src/memory/tree/`) therefore looks like:

```text
src/memory/<area>/
  mod.rs           # module docs (//!), re-exports, wiring
  types.rs         # struct/enum/trait definitions for this area
  <impl>.rs        # focused behavior files, each around ~500 LOC or less
  <impl>_tests.rs  # unit tests for the matching implementation file
```

### Coding style

- Rust 2021 edition, standard `cargo fmt` formatting — run `cargo fmt --all` before committing.
- Keep public type names direct and domain-specific.
- When porting contracts from OpenHuman, **preserve machine-readable ids and enum wire strings exactly** so derived indexes and external payloads stay compatible. The `taint` and provenance invariants described in [Core-Concepts](core-concepts.md) depend on these wire values decoding consistently (e.g. an unknown taint decodes as `external`).
- Respect the strict layer boundaries (storage -> ingest -> retrieval -> diff -> entities -> goals/tool_memory -> conversations/archivist -> job queue); don't reach across layers.

## Commit & pull request guidelines

Make small, focused commits — one coherent change per commit. Recent history uses Conventional Commit-style subjects:

```text
fix:      a bug fix
refactor: a behavior-preserving restructuring
chore:    tooling, version bumps, housekeeping
docs:     documentation-only changes
test:     test additions or changes
feat:     a new capability
```

Pull requests should:

- Summarize the affected module(s).
- Describe behavior or spec changes (note any changed wire strings, enum values, or scoring weights).
- List the tests you ran (`cargo test`, plus any benchmark runs).
- Link related issues where applicable.

Keep each PR to one logical change. Maintainers may squash commits on merge to keep history clean. Contributions are licensed under the MIT License.

### Working in parallel

When running multiple tasks (or parallel sub-agents) against the repo at once, use **git worktrees** so each effort has an isolated checkout. This keeps build output, branch state, and working files from one task out of another's way:

```bash
git worktree add ../neocortex-feature-x -b your-name/feature-x
```

## Running benchmarks

The benchmarks are **not** part of the published Rust crate — they live under `benchmarks/` and are excluded from the package. The runnable suite is the **retrieval-effectiveness harness**, a standalone Rust crate that path-depends on `tinycortex`:

```bash
cd benchmarks/effectiveness
cargo run --bin effectiveness
cargo test    # metrics + dataset-validation unit tests
```

It measures recall@k, precision@k, hit@k, MRR, and nDCG@k over labeled datasets and writes dated JSON reports under `results/` (gitignored). The RAGAS / TemporalBench / BABILong / Vending-Bench figures shown on the [Benchmarks](benchmarks.md) page come from a hosted evaluation harness that is not in this repository.

{% hint style="info" %}
See [Benchmarks](benchmarks.md) for what each suite measures and how results are reported.
{% endhint %}

## See also

- [Getting-Started](getting-started.md) — embed the crate and run your first query
- [Architecture-Overview](architecture.md) — the layered design contributors must respect
- [Core-Concepts](core-concepts.md) — provenance, taint, and the wire-string invariants
- [Benchmarks](benchmarks.md) — the evaluation suites and how to interpret them
