# Contributing to TinyCortex

Thank you for your interest in contributing to **TinyCortex** — the AI memory system that forgets noise and remembers what matters. This repository is the open-source **Rust core**: a single library crate (`tinycortex` on crates.io). We welcome contributions to the crate, its tests, the benchmark harness, and the documentation.

## Table of Contents

- [Code of Conduct](#code-of-conduct)
- [Getting Started](#getting-started)
- [Project Structure](#project-structure)
- [How to Contribute](#how-to-contribute)
- [Pull Request Process](#pull-request-process)
- [License](#license)

## Code of Conduct

By participating in this project, you agree to be respectful and constructive. We aim to maintain a welcoming environment for everyone.

## Getting Started

1. **Fork and clone the repository**

   ```bash
   git clone https://github.com/YOUR_USERNAME/tinycortex.git
   cd tinycortex
   ```

2. **Set up your environment** — you need a stable Rust toolchain ([rustup](https://rustup.rs/)) and a C compiler (`rusqlite` builds a bundled SQLite; no system SQLite needed). Then:

   ```bash
   cargo check                  # fast validation
   cargo test                   # unit + integration tests (default features)
   cargo test --all-features    # everything, including feature-gated modules — this is what CI runs
   cargo fmt --all              # format before committing
   ```

   The default feature set is empty; `tokio` (async queue runtime), `git-diff` (git-backed diff ledger, needs libgit2 via `git2`), `providers-http`, and `rpc` gate whole modules. See [gitbooks/contributing.md](gitbooks/contributing.md) for details.

3. **Create a branch** for your work:
   ```bash
   git checkout -b your-name/feature-or-fix
   ```

## Project Structure

| Path          | Description                                                                                              |
| ------------- | -------------------------------------------------------------------------------------------------------- |
| `src/`        | The Rust crate source — all library code lives under `src/memory/`                                        |
| `tests/`      | Integration tests that drive the crate's public API                                                       |
| `benchmarks/` | The retrieval-effectiveness harness (`effectiveness/`, a standalone Rust crate) plus reported platform evaluation results |
| `examples/`   | Example scenarios for using TinyCortex                                                                    |
| `gitbooks/`   | Long-form documentation (getting started, architecture, module guides)                                    |
| `docs/`       | Migration and specification docs (OpenHuman port notes)                                                   |
| `paper/`      | Research paper sources                                                                                    |

## How to Contribute

- **Code**
  Follow the module conventions: `types.rs` for a module's type definitions, one `<name>_tests.rs` per implementation file, files around ~500 lines or less. Preserve machine-readable ids and enum wire strings when porting contracts from OpenHuman. Document public APIs with rustdoc.

- **Documentation**
  Fix typos, clarify explanations, or add new guides under `gitbooks/` or in the main `README.md`. Keep tone consistent with the existing docs, and keep claims in parity with the code.

- **Benchmarks**
  The runnable benchmark is `benchmarks/effectiveness` (`cd benchmarks/effectiveness && cargo run --bin effectiveness`). Growing the labeled dataset or adding backends (see its README's roadmap) are welcome contributions. Ensure runs are reproducible and document the environment for any reported numbers.

- **Bug reports & feature ideas**
  Open an issue with a clear description, steps to reproduce (for bugs), and context (version, OS, etc.) where relevant.

## Pull Request Process

1. **Keep changes focused** — One logical change per PR (e.g. one fix, one feature, or one doc section). Prefer small, focused commits with Conventional Commit-style subjects (`fix:`, `feat:`, `docs:`, `refactor:`, `chore:`, `test:`).

2. **Update docs** if your change affects usage, APIs, or setup (e.g. new feature flags, changed wire strings, scoring weights).

3. **Test locally** — Run `cargo test --all-features` and `cargo fmt --all`; for benchmark changes, run the effectiveness harness.

4. **Push your branch** and open a PR against `main`. Describe what you changed and why, and list the tests you ran.

5. **Address review feedback** — Maintainers may request edits; we'll work with you to get the PR merged.

We may squash commits when merging to keep history clean.

## License

By contributing, you agree that your contributions will be licensed under the [MIT License](LICENSE). Copyright (c) 2026 Tiny Humans Intelligence Inc.

---

Questions? Reach out at [contact@tinyhumans.ai](mailto:contact@tinyhumans.ai).
