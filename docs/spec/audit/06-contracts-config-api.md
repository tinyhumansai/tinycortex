# Audit 06 — Crate Contracts, Config, API Surface, Hygiene

Scope: `src/lib.rs`, `src/memory/{mod,types,traits,config,error}.rs`, the
`InMemoryMemoryStore` reference backend, feature gating, `tests/`, and
top-level docs. IDs `CT-*` are referenced from the
[improvement plan](../improvement-plan.md).

Verification runs: `cargo check` with `--no-default-features`,
`--all-features`, and each individual feature — all green.
`cargo test`: 1025 unit + 1 integration test, 0 failures. No unused
dependencies.

## Findings

### CT-1 (High). README Getting Started example silently returns zero results
`README.md:57-76` vs `src/memory/store/store.rs:99-104`

The example compiles, but `MemoryQuery::text("theme preference")` against
"User prefers dark mode" matches nothing: `search` requires the *entire
lowercased query string* to be a substring of the content. Verified by running
the exact README code: 0 hits. The advertised "keyword query" recall demo
prints nothing.

**Fix:** make search term-based (match whitespace-split terms) as the README's
wording implies — which also fixes CT-3 — or change the example query.

### CT-2 (High). Documented taint fail-closed semantics are false for the serde path
`src/memory/types.rs:46-52,101,163,179,229,323`; `src/memory/rpc/mod.rs:14-15`

Five doc sites plus the rpc contract say unknown persisted taint values decode
as `external_sync` (fail closed). Only `MemoryTaint::from_db_str` does; the
serde derive has no `#[serde(other)]`, so unknown strings are a hard error —
and `#[serde(default)]` on every taint field means a *missing* taint fails
**open** to `Internal`.

**Fix:** `#[serde(other)]` on `ExternalSync` (and consider a fail-closed
default), or correct the five doc comments and the rpc contract.

### CT-3 (Medium). `InMemoryMemoryStore` scoring is degenerate (constant per query)
`src/memory/store/store.rs:98-111`

Because a hit first requires the full query phrase as a substring, the
per-term scoring always counts every term — every hit gets the identical
score. The `SearchHit.score` doc ("higher is more relevant") and trait doc
("ordered most relevant first") are not delivered; ordering degrades to
recency.

### CT-4 (Medium). The `Memory` trait has no production implementor; two parallel store contracts
`src/memory/traits.rs:17` vs `src/memory/store/store.rs:24,56`

The crate's headline `Memory` trait (re-exported at `memory::Memory`) is
implemented only by the `#[cfg(test)]` `MockMemory`. `InMemoryMemoryStore`
implements the *different* `MemoryStore` trait. A host cannot construct any
`Arc<dyn Memory>` (which `ToolMemoryStore::new` requires) without writing its
own backend. Methods like `recall_relevant_by_vector` default to `Ok(vec![])`
and are never exercised. This is the central blocker for the
[configurable-store](../configurable-store.md) work.

### CT-5 (Medium). Two different public types named `MemoryError` at the module root
`src/memory/mod.rs:72,83`

`tinycortex::memory::MemoryError` is the tiny 2-variant store enum
(`NotFound`/`EmptyContent`), while the engine error is exported as
`MemoryEngineError`. The store enum can't represent I/O or lock failures —
lock poisoning panics via `.expect("memory store lock poisoned")`
(`store.rs:61,69,78,90`) instead of returning an error.

**Fix:** rename the store enum (e.g. `StoreError`) or stop re-exporting it at
the root.

### CT-6 (Low). Config sub-structs are not partially-deserializable and unvalidated
`src/memory/config.rs:32-42,59-104,150-157`

`EmbeddingConfig`/`TreeConfig`/`RetrievalConfig` lack per-field
`#[serde(default)]`, so `[embedding]\nmodel = "x"` (omitting `dim`) fails to
deserialize — only fully-absent sections get defaults. No validation anywhere:
`dim: 0`, `summary_fanout: 0`, negative weights all accepted.
`WeightProfile::by_name` silently maps unknown names to `BALANCED`.

### CT-7 (Low). `MemoryCategory` Display/serde asymmetry breaks round-trips
`src/memory/types.rs:56-78`

Serde form of `Custom("tool_memory")` is `{"custom":"tool_memory"}`, but
`Display` renders the bare inner string and collides with built-in variants
(`Custom("core")` vs `Core`). Types persisting `category: String` use the
Display label and there is no `from_str` inverse — a Display-serialized
category cannot be decoded back.

### CT-8 (Low). Repo hygiene-rule violations
- Non-test src files over the 500-line limit: `src/memory/diff/ledger.rs`
  (634), `src/memory/queue/types.rs` (538).
- Inline `#[cfg(test)] mod tests` in `src/memory/store/store.rs:132-177`
  contrary to the separate-`*_tests.rs` convention; that file also mixes the
  trait definition with the impl (types belong in `types.rs` per convention).

### CT-9 (Low). 59 rustdoc warnings, including a feature-gating doc break
`cargo doc --no-deps`: `src/memory/mod.rs:17` links `[diff]`, unresolved
whenever docs build without `git-diff` (the default); ~50
public-doc-links-to-private-item warnings across `store/safety.rs`, `chunks`,
`conversations`.

### CT-10 (Info). README module-availability claims omit feature requirements
`README.md:78,99` advertise the diff ledger without mentioning the non-default
`git-diff` feature. `providers` and `rpc` modules are pure stubs (a type alias
and a doc-only module respectively) — honestly labeled in Cargo.toml, but
docs.rs will show empty modules.

## Test-suite observation

`tests/smoke.rs` queries "Rust memory" against content containing that exact
phrase, so it never exercises the phrase-gate bug (CT-1/CT-3). Integration
coverage is effectively a single happy-path smoke test; the engine's real
surfaces (ingest → queue → tree → retrieval end-to-end) have no integration
test.
