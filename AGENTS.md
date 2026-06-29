# Repository Guidelines

## Project Structure & Module Organization

TinyCortex is a multi-language SDK and plugin repository with a new Rust core in `crates/tinycortex`. Most integration work still lives under `packages/`. SDKs are named `packages/sdk-<language>` such as `sdk-python`, `sdk-typescript`, `sdk-golang`, `sdk-rust`, `sdk-dart`, `sdk-cpp`, `sdk-csharp`, and `sdk-java`. Integrations are named `packages/plugin-<framework>` such as `plugin-langgraph`, `plugin-codex`, `plugin-mastra`, and `plugin-camelai`. Tests live beside each package in `test/`, `tests/`, or language-native test locations such as `src/test`. Shared docs are in `README.md`, `packages/README.md`, `gitbooks/`, `examples/`, `benchmarks/`, and `paper/`.

## Build, Test, and Development Commands

There is no single root build command; run commands inside the package you change.

- `cd packages/sdk-python && make install && make test`: install with `uv` and run `pytest`.
- `cd packages/sdk-typescript && npm install && npm test`: run Vitest for the TypeScript SDK.
- `cd packages/plugin-codex && npm install && npm run build`: compile a TypeScript plugin with `tsc`.
- `cd packages/sdk-golang && make check`: run `go build`, `go vet`, and `go test`.
- `cd packages/sdk-rust && cargo test`: run Rust unit and integration tests.
- `cd packages/sdk-cpp && make test`: configure with CMake and run CTest.
- `cd packages/sdk-csharp && make test`: run non-integration .NET tests.
- `cd packages/sdk-dart && make test`: analyze and run Dart unit tests.
- `cd packages/sdk-java && make test`: run Gradle tests.

Integration tests usually require `TINYHUMANS_TOKEN`; do not hard-code credentials.

## Coding Style & Naming Conventions

Follow each package's native conventions and existing public API names. TypeScript packages use ESM, `src/`, generated `dist/`, and TypeScript 5. Python plugins target Python 3.9+ and use Ruff settings where present, including 88-character lines. Go code should be `gofmt`/`go vet` clean. Rust uses edition 2021 and standard `cargo fmt` style. Keep SDK method names and request/response fields consistent across languages.

## Testing Guidelines

Add focused tests in the package you modify. Prefer existing naming patterns, for example `*_test.go`, `test_*.py`, `*.test.ts`, C# `*Tests.cs`, and Rust files under `tests/`. Keep unit tests independent of live services; put token-dependent checks in the package's integration test path or target.

## Commit & Pull Request Guidelines

Recent history uses Conventional Commit-style subjects such as `fix(sdk-rust): ...`, `fix: ...`, `refactor: ...`, and `chore: ...`. Keep commits scoped to one package or cross-SDK contract change when possible. Pull requests should describe the affected SDK/plugin, summarize behavior changes, list tests run, and link issues when applicable. Include screenshots only for docs or UI-facing changes.
