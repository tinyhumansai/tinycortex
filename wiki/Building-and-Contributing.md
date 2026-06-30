# Building & Contributing

This page covers how to build, test, format, and document the **TinyCortex** Rust crate (`tinycortex` on crates.io), the repository layout conventions contributors are expected to follow, the commit/PR style, and how to run the benchmark suite that lives alongside the crate.

TinyCortex is a single root-level Rust library crate. There is no hosted service, server binary, or language SDK to build here — everything in `src/` compiles into one `cdylib`/`rlib` library that host applications (OpenHuman or your own) embed.

## Prerequisites

You need a stable Rust toolchain with Rust 2021 edition support. Install via [rustup](https://rustup.rs/) and confirm `cargo` is on your `PATH`.

The crate pulls in a few native dependencies through Cargo, so a working C toolchain is required for the first build:

| Dependency | Why it needs a native build |
| --- | --- |
| `rusqlite` (with the `bundled` feature) | compiles a vendored SQLite from C; no system SQLite needed |
| `git2` | links against libgit2 for the git-backed [Diff Layer](Diff-Layer) |
| `sha2`, `uuid`, `rand` | content hashing, ids, and sampling |

Because `rusqlite` is configured with `bundled`, you do **not** need a system SQLite install — but you do need a C compiler (`cc`/clang) available for the bundled build. The full dependency set is declared in `Cargo.toml` at the repo root.

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

Test layout follows two conventions from `AGENTS.md`:

- **Unit tests** live next to the module they cover, in a dedicated `test.rs` file inside the module directory — not inline at the bottom of the implementation file.
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
| `benchmarks/` | benchmark notebooks and helpers (RAGAS, BABILong, TemporalBench, LoCoMo, HotPotQA, etc.) |
| `examples/` | example notebooks and scenarios |
| `gitbooks/` | long-form prose documentation |
| `paper/` | research paper sources |
| `README.md` | top-level project overview |

Note that `Cargo.toml` deliberately **excludes** the non-Rust directories (`.github/`, `.vscode/`, `benchmarks/`, `docs/`, `examples/`, `gitbooks/`, `paper/`, and `requirements.txt`) from the published crate via its `exclude` list, so the package shipped to crates.io contains only the library and what it needs to compile.

### Module conventions

Modules are organized to stay high-level and cohesive. Three rules from `AGENTS.md` are enforced by convention in review:

1. **`type.rs` for types.** All type definitions for a module go in a dedicated `type.rs` file inside that module directory.
2. **`test.rs` for tests.** Tests live in a separate `test.rs` file, not mixed into implementation files.
3. **500-line cap.** Avoid letting any source file grow beyond ~500 lines of code. Split behavior into focused submodules before you reach that point.

A typical engine module (for example under `src/memory/tree/`) therefore looks like:

```text
src/memory/<area>/
  mod.rs      # module docs (//!), re-exports, wiring
  type.rs     # struct/enum/trait definitions for this area
  <impl>.rs   # focused behavior files, each well under ~500 LOC
  test.rs     # unit tests for this area
```

### Coding style

- Rust 2021 edition, standard `cargo fmt` formatting — run `cargo fmt --all` before committing.
- Keep public type names direct and domain-specific.
- When porting contracts from OpenHuman, **preserve machine-readable ids and enum wire strings exactly** so derived indexes and external payloads stay compatible. The `taint` and provenance invariants described in [Core-Concepts](Core-Concepts) depend on these wire values decoding consistently (e.g. an unknown taint decodes as `external`).
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

The benchmarks are **not** part of the Rust crate — they live under `benchmarks/` as Python notebooks and helpers and are excluded from the published package. They evaluate the memory system against external suites (RAGAS, BABILong, TemporalBench, Vending-Bench, LoCoMo, HotPotQA, and others) using the shared `nb_helpers/` utilities for config, datasets, pipeline, and metrics.

To set up the benchmark/helper environment from the repo root:

```bash
pip install -r requirements.txt
pip install -e .
```

Then run the relevant notebook(s) under `benchmarks/`. For reproducibility, fix random seeds and document the environment used for any reported numbers. Corpus download, test-set generation, evaluation, and charting are driven by scripts under `scripts/`.

See [Benchmarks](Benchmarks) for what each suite measures and how results are reported.

## See also

- [Getting-Started](Getting-Started) — embed the crate and run your first query
- [Architecture-Overview](Architecture-Overview) — the layered design contributors must respect
- [Core-Concepts](Core-Concepts) — provenance, taint, and the wire-string invariants
- [Benchmarks](Benchmarks) — the evaluation suites and how to interpret them
