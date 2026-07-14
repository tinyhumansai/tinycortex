# Audit 10 — Simplification & Dead Weight

_Audit date: 2026-07-14 · Baseline: `main` @ `1a3fcc5` · ~40.8k non-test LOC
across 325 files._

Scope: duplication, speculative/dead code, dependency weight, over-abstraction,
and the error/async story — everything that makes the crate bigger and harder
to trust than it needs to be. IDs are `SW-*`.

## Findings

### SW-1 (High). The `tinyagents` git dependency blocks publishing and pins trust to one file

`Cargo.toml` depends on
`tinyagents = { git = "https://github.com/senamakel/tinyagents", rev = "8f75355" }` —
a personal-repo git dependency at an unaudited commit. Its **only** consumer is
`src/memory/store/vectors/embedding.rs` (a bridge to
`tinyagents::…::EmbeddingModel`). Consequences: crates.io **rejects git
dependencies**, so the crate as published cannot ship this manifest; and every
downstream build's supply chain includes a mutable personal repo, for the sake
of one trait bridge in one file — a file that itself has no tests (VF-4).

**Fix:** vendor the small trait bridge locally (TinyCortex already defines its
own embedding seams) or depend on a published, versioned crate. Highest-
priority dependency finding in this audit.

### SW-2 (High). Placeholder modules and features shipping nothing

- `src/memory/rpc/mod.rs` — 15 lines, all doc comment, zero code, behind the
  `rpc` feature ("reserved for goal C5").
- `src/memory/providers/mod.rs` — 24 lines whose only item is
  `pub type HttpClient = reqwest::Client;`, behind `providers-http`, which
  pulls the full reqwest+tokio stack for that alias.

For a 0.1.x crate these reserve nothing that a git branch couldn't, while
adding two feature flags, doc surface, and (for `providers-http`) a heavy
optional dependency stack that backs one line of code. **Fix:** delete both
modules and features; reintroduce each feature in the release that ships its
implementation.

### SW-3 (High). The flagship `Memory` trait has zero production implementations

`traits.rs::Memory` (async, 11 methods, re-exported as the crate's headline
API at `mod.rs:103`) is implemented only by `MockMemory` in
`tool_memory/test_helpers.rs`. Its sole consumer, `ToolMemoryStore`
(`tool_memory/store.rs:45`, `Arc<dyn Memory>`), is constructed only in tests.
Meanwhile `store/store.rs::MemoryStore` is a second, *synchronous* core-store
trait with exactly one impl (`InMemoryMemoryStore`). So the crate exports two
competing core abstractions, one with no real impl and one with one — while
everything real is hardwired to filesystem+SQLite (already CT-4/CT-5).

**Fix:** this is the same consolidation the configurable-store spec needs:
collapse to one trait, make the real engine implement it, and either wire
`tool_memory` to the real backend or park the module until it has one.

Related zero-impl traits to delete outright:
`conversations/bus.rs::ConversationEventBus` (no implementors anywhere) and
`score/extract/llm.rs::ChatProvider` (test impls only; the real provider was
deferred with SW-2's `providers` seam).

### SW-4 (Medium). Atomic-write is implemented eight times; the shared helper already exists

`fsutil::atomic_write` (`fsutil.rs:26`) is the designated crash-safe
temp+fsync+rename helper per `mod.rs:35-38` — yet the same dance is hand-rolled
in: `store/content/atomic.rs:31-64` (which *also* calls `fsutil::atomic_write`
at `:89` — two mechanisms in one file, plus a bespoke non-UUID
`uuid_v4_hex()` at `:177` despite `uuid` being a dependency),
`store/content/raw.rs:170`, `store/content/tags.rs:45-52`,
`entities/store.rs:126`, `conversations/store.rs:299,317`,
`sources/registry.rs:149,162`, and `tree/runtime/engine.rs:228-236`. The
`fsutil` doc-comment even claims stores "share this helper rather than
re-implementing it" — they don't. Crash-safety is a *headline invariant* of
this crate; it currently has eight implementations to audit instead of one.

**Fix:** route every site through `fsutil` (add the write-if-new variant it
lacks); delete `uuid_v4_hex`. Each conversion is mechanical and ~15–40 lines
removed. Prior findings DS-1/TR-7/SC-3 (non-atomic writes) are the same
disease — sites that *couldn't* copy-paste a shared helper wrote unsafe code.

### SW-5 (Medium). More copy-paste clusters with an obvious single owner

- **SQLite open+pragmas:** `OWNED_CONNECTION_PRAGMAS` is defined twice
  verbatim (`store/kv.rs:38`, `store/entity_index/store.rs:61`); six more
  open-and-configure sequences live in `store/vectors/store.rs:91`,
  `chunks/connection.rs:406`, `store/mod.rs`, `sync/persist.rs:71`,
  `diff/ledger.rs:80`. One `open_owned(path, pragmas)` helper (natural home:
  the `db` module proposed in MB-1) collapses all of them.
- **Front-matter split:** two independent implementations of the same
  `---\n…\n---` grammar — `entities/frontmatter.rs:107` (private) and
  `store/content/compose/yaml.rs:91` (pub). SC-1's EOF-fence panic had to be
  found and fixed per-copy; a grammar should exist once.
- **SHA-256 hex:** `content/atomic.rs:164` plus inline copies at
  `chunks/types.rs:288` and `chunks/mod.rs:152`.
- **Retry/backoff:** ad-hoc loops and per-module retryability predicates
  (`queue/*`, `sync/composio/client.rs:242,251`, `score/extract/llm.rs:241`,
  `chunks/recovery.rs`) with no shared `RetryPolicy` type — see also CF-6;
  one policy type serves both the config story and the dedup.

### SW-6 (Medium). Four dependencies exist for one call site each

| Dep | Sole non-test consumer | Replacement |
| --- | --- | --- |
| `futures` | `tree/bucket_seal.rs` (`stream::iter(…).buffered`) | small bounded-concurrency loop, or keep only if the tokio feature grows |
| `schemars` | `sources/types.rs` (one `#[derive(JsonSchema)]`) | drop the derive or gate it behind `rpc` when that ships |
| `rand` | `tree/registry.rs:67` (`rand::random::<u32>()` id tail) | `uuid` (already a dep) |
| `walkdir` | `sources/readers/folder.rs` | `std::fs` recursion (or keep — it's small; the point is it's one call) |

Each removal shrinks the supply-chain and audit surface of the *default*
build. `toml` (only `sources/registry.rs`) is legitimate but worth knowing.

### SW-7 (Medium). "Fully synchronous core" isn't: async-trait machinery is unconditional

`lib.rs` advertises a dependency-light synchronous core, and `tokio` is
correctly optional — but `async-trait` and `futures` compile into the
*default* feature set because the default-built traits are async:
`traits.rs::Memory`, both `Summariser`s, `SealObserver`,
`conversations/bus.rs`, `queue/handlers.rs`, `score/embed.rs::Embedder`,
`score/extract/*`, `sources/readers/*` (~30 files). The default build carries
async signatures it has no runtime to drive.

**Fix (direction):** decide per trait whether it is genuinely async. The
zero-impl traits (SW-3) disappear anyway; for the rest, either make them sync
in the default build or gate the async variants behind `tokio`. Done fully,
`async-trait` and `futures` leave the default dependency set.

### SW-8 (Medium). Split-brain error story at the library boundary

A typed `MemoryError` exists (`error.rs`, thiserror, exported as
`MemoryEngineError`) but is used in ~4 files; meanwhile `anyhow` appears in
61 non-test files and the core traits deliberately return `anyhow::Result`
("treat `Err` as opaque", `traits.rs:10-16`). Hosts embedding a library
generally want typed errors at the boundary; today they get opaque strings
everywhere and an almost-unused enum. **Fix:** pick one story. Pragmatic
version: keep `anyhow` internally, but make the *public facade* methods (the
consolidated trait from SW-3, ingest/retrieval entry points) return
`MemoryError` with a small set of actionable variants (`Io`, `Corrupt`,
`BudgetExceeded`, `NotFound`, `Invalid`), and delete the enum variants nothing
constructs.

### SW-9 (Low). `#[allow(dead_code)]` items documented as unused

Nine non-test sites, several self-describing as speculative:
`chunks/recovery.rs:60,260` ("nothing in connection.rs calls this"),
`chunks/store.rs:156`, `chunks/connection.rs:427`,
`chunks/store_sources.rs:39`, `chunks/mod.rs:149`,
`score/extract/llm.rs:400,437`, `ingest/extract/types.rs:243`,
`ingest/extract/header.rs:53`, `conversations/inverted_index.rs:184`. Note
`chunks/recovery.rs` overlaps SC-5 (recovery exists but is never wired) — for
that one the fix is to *wire it*, not delete it; the rest are deletion
candidates.

### SW-10 (Low). `viewer/` is cleanly separate — keep it that way deliberately

The Next.js viewer reads the workspace directly via `better-sqlite3` (no FFI,
no crate linkage), is excluded from the published package, and its
`node_modules/`/`.next/` are properly git-ignored (`viewer/.gitignore`; only
19 viewer files are tracked). No entanglement found. Two follow-ups belong to
audit 09 (CI build + lint, VF-7); the one architectural note is that the
viewer now constitutes a *second reader* of the on-disk schema, so any schema
change has an untyped, untested downstream consumer — one more reason to give
the schema a single named owner (MB-1) and a documented layout contract
(CF-7).

## Ranked simplification sequence

1. **SW-1** vendor/replace `tinyagents` (publishing blocker, one file).
2. **SW-2** delete `rpc/` + `providers/` modules and features.
3. **SW-4** collapse atomic-write onto `fsutil` (this also closes the class
   behind DS-1/TR-7/SC-3 recurrence).
4. **SW-3** one store trait; delete zero-impl traits.
5. **SW-5** shared sqlite-open, front-matter, sha256, retry-policy helpers.
6. **SW-6/SW-7** dependency diet: drop `schemars`/`rand`/`walkdir`/`futures`,
   then de-async the default build.
7. **SW-8** typed errors at the public boundary.

Items 1–3 and 5–6 are individually small, independently landable, and each
makes the crate strictly smaller — good first slices per the repo's
small-commit workflow.
