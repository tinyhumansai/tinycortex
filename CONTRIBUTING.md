# Contributing to TinyCortex

Thank you for your interest in contributing to **TinyCortex** — the AI memory system that forgets noise and remembers what matters. We welcome contributions to documentation, packages (3rd party integrations), benchmarks, and examples.

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
   git clone https://github.com/YOUR_USERNAME/tinycortex-docs.git
   cd tinycortex-docs
   ```

2. **Set up your environment**
   - **Benchmarks & helpers**: From the repo root, install dependencies:
     ```bash
     pip install -r requirements.txt
     pip install -e .
     ```
   - **Packages**: If you're working on a package under `packages/<name>/`, use that package's install instructions (e.g. `uv sync` or `pip install -e .` in its directory).

3. **Create a branch** for your work:
   ```bash
   git checkout -b your-name/feature-or-fix
   ```

## Project Structure

| Path                   | Description                                                                                                                                             |
| ---------------------- | ------------------------------------------------------------------------------------------------------------------------------------------------------- |
| `packages/`   | **All 3rd party integrations.** Each subfolder is a separate integration. |
| `benchmarks/` | Benchmark notebooks (RAGAS, BABILong, TemporalBench, Vending-Bench, LoCoMo, HotPotQA, etc.) and `nb_helpers/`                                           |
| `examples/`            | Example notebooks and scenarios for using TinyCortex                                                                                                     |
| `gitbooks/`            | Documentation (getting started, how memory works)                                                                                                       |
| `helpers/`             | Shared adapters, chunking, and types used by benchmarks                                                                                                 |
| `scripts/`             | Corpus download, test set generation, evaluation, charting                                                                                              |

## How to Contribute

- **Documentation**
  Fix typos, clarify explanations, or add new guides under `gitbooks/` or in the main `README.md`. Keep tone consistent with the existing docs.

- **Packages (3rd party integrations)**
  The `packages/` directory holds all 3rd party integrations (SDKs, plugins, etc.). Include tests where applicable; some packages have publish workflows that run on pushes to `main` when that package's files change. New integrations belong as new subfolders under `packages/`.

- **Benchmarks**
  Benchmark notebooks live in `benchmarks/`. Use `nb_helpers` for config, datasets, pipeline, and metrics. Ensure runs are reproducible (fixed seeds, documented env).

- **Examples**
  New or updated example notebooks in `examples/notebooks/` or scenarios in `examples/scenarios/` are welcome. Keep them runnable with the current SDK and dependencies.

- **Bug reports & feature ideas**
  Open an issue with a clear description, steps to reproduce (for bugs), and context (version, OS, etc.) where relevant.

## Pull Request Process

1. **Keep changes focused** — One logical change per PR (e.g. one fix, one feature, or one doc section).

2. **Update docs** if your change affects usage, APIs, or setup (e.g. new env vars, new SDK options).

3. **Test locally** — For package changes, run the relevant tests or example scripts; for benchmarks, run the affected notebook(s).

4. **Push your branch** and open a PR against `main`. Describe what you changed and why.

5. **Address review feedback** — Maintainers may request edits; we’ll work with you to get the PR merged.

We may squash commits when merging to keep history clean.

## License

By contributing, you agree that your contributions will be licensed under the [MIT License](LICENSE). Copyright (c) 2026 Tiny Humans Intelligence Inc.

---

Questions? Reach out at [contact@tinyhumans.ai](mailto:contact@tinyhumans.ai).
