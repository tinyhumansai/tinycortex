---
description: Embed the open-source TinyCortex Rust crate and build a working store-and-recall loop in minutes.
---

# Getting Started

TinyCortex is the open-source **Rust core** of the TinyCortex memory system, published on [crates.io](https://crates.io/crates/tinycortex) as the `tinycortex` crate. It is a **library**, not a hosted service: you embed it in your own agent, service, or app. This page gets you from an empty project to a working store-and-recall loop, then points at where the full engine lives.

{% hint style="info" %}
The hosted TinyCortex platform (managed API, turnkey "conscious recall" product, per-user cost figures) is a separate closed-alpha offering. Everything on this page is the open-source crate.
{% endhint %}

## Add the crate

TinyCortex targets **Rust 2021**. Add it to an existing project:

```bash
cargo add tinycortex
```

The end-to-end runnable surface today — `InMemoryMemoryStore` plus the `MemoryStore` trait — is `async`, so you also need an async runtime to drive it. The crate itself does not pull in a runtime; the examples here use Tokio (the same runtime the crate's own dev-tests use):

```bash
cargo add tokio --features macros,rt-multi-thread
cargo add anyhow   # only for the `?` ergonomics in the example below
```

## Quickstart

Store a memory and recall it with a keyword query, using the built-in in-process backend:

```rust
use tinycortex::memory::{InMemoryMemoryStore, MemoryInput, MemoryQuery, MemoryStore};

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    // Volatile, BTreeMap-backed reference store. Cheaply cloneable; contents are
    // lost on drop.
    let store = InMemoryMemoryStore::new();

    // Insert a memory into the "preferences" namespace. `insert` mints the id and
    // timestamps and returns the persisted record.
    let record = store
        .insert(MemoryInput::new("preferences", "User prefers dark mode"))
        .await?;
    println!("stored {} in namespace {}", record.id, record.namespace);

    // Recall by keyword. `search` returns scored hits, most relevant first.
    let hits = store.search(MemoryQuery::text("dark mode")).await?;
    for hit in hits {
        println!("{:.3}  {}", hit.score, hit.record.content);
    }

    Ok(())
}
```

Run it:

```bash
cargo run
```

### What just happened

| Step | Type | Notes |
| --- | --- | --- |
| `MemoryInput::new(ns, content)` | `MemoryInput` | Caller-supplied, untrusted, not yet persisted. Carries a `namespace`, raw `content`, and free-form `metadata` (empty by default). |
| `store.insert(input)` | `-> MemoryResult<MemoryRecord>` | Trims `content` and rejects empty/whitespace-only input with `MemoryError::EmptyContent`. Mints a v4 UUID (`MemoryId`) and stamps `created_at`/`updated_at`. |
| `MemoryQuery::text(t)` | `MemoryQuery` | Convenience constructor; sets `text`, leaves `namespace` and `limit` unconstrained. |
| `store.search(query)` | `-> MemoryResult<Vec<SearchHit>>` | Returns `SearchHit { record, score }`, sorted by score then recency. |

## The `MemoryStore` contract

Every storage backend satisfies the `MemoryStore` trait (`src/memory/store/store.rs`). It is `Send + Sync` so a single store can be shared across async tasks:

```text
#[async_trait]
pub trait MemoryStore: Send + Sync {
    async fn insert(&self, input: MemoryInput) -> MemoryResult<MemoryRecord>;
    async fn get(&self, id: MemoryId)           -> MemoryResult<MemoryRecord>;
    async fn delete(&self, id: MemoryId)        -> MemoryResult<MemoryRecord>;
    async fn search(&self, query: MemoryQuery)  -> MemoryResult<Vec<SearchHit>>;
}
```

### Core value types

These live in `src/memory/store/types.rs` and are the data contracts that flow through any store:

| Type | Role | Key fields |
| --- | --- | --- |
| `MemoryInput` | Untrusted, caller-supplied item | `namespace: String`, `content: String`, `metadata: Map<String, Value>` |
| `MemoryRecord` | Persisted item with identity | `id: MemoryId`, `namespace`, `content`, `metadata`, `created_at`, `updated_at` |
| `MemoryQuery` | Conjunctive filter (all fields optional, `None` = unconstrained) | `namespace: Option<String>`, `text: Option<String>`, `limit: Option<usize>` |
| `SearchHit` | A retrieval result | `record: MemoryRecord`, `score: f32` (higher is more relevant; scale is store-defined) |
| `MemoryId` | Stable identity | type alias for `uuid::Uuid` (v4) |
| `MemoryError` | Failure modes | `NotFound(MemoryId)`, `EmptyContent` |
| `MemoryResult<T>` | Result alias | `Result<T, MemoryError>` |

### Building a query directly

`MemoryQuery::text` is the convenience path; you can also build the struct to scope a namespace and a limit:

```rust
let query = MemoryQuery {
    namespace: Some("project".to_owned()),
    text: Some("durable".to_owned()),
    limit: Some(10),
};
let hits = store.search(query).await?;
```

## What the simple store does — and what it doesn't

`InMemoryMemoryStore` is the **simplest conforming backend**, intended for tests and getting started, not durable production use. Concretely:

- **Volatile.** Records live in an `Arc<RwLock<BTreeMap<MemoryId, MemoryRecord>>>`; contents are lost on drop. The store is cheaply cloneable and shareable across tasks.
- **Naive keyword search.** `search` lower-cases the query text, keeps records whose `content` contains the full needle, and scores by the count of matching whitespace-delimited terms (minimum `1.0`). With no `text`, every record in scope scores `1.0`.
- **Conjunctive namespace filter.** A `namespace` constraint must match exactly; `None` searches all namespaces.
- **Default limit of 20.** When `MemoryQuery::limit` is `None`, `search` returns at most 20 hits. Results are sorted by score, ties broken by `created_at` (newer first).

What it deliberately does **not** do: no durability, no markdown vault, no chunking, scoring, decay, embeddings, summary trees, graph, or explainable score breakdowns. Those are the full engine.

## Where the full engine lives

The whole public surface sits under the `memory` module (`src/memory/`). The reference store above is one corner of it; the layered engine — content store, ingest, scoring/extraction, summary trees, retrieval, diff ledger, entities/graph, goals/tool-memory, conversations/archivist, and the async job queue — is organized into focused submodules:

| Area | Module path | Wiki page |
| --- | --- | --- |
| Minimal store + reference backend | `src/memory/store/` | this page |
| Markdown vault, SQLite chunks, vectors, KV, entity index | storage primitives | [Storage Primitives](storage-primitives.md) |
| Canonicalize → chunk → score → embed → tree | ingest | [Ingest Pipeline](ingest-pipeline.md) |
| Value scoring + extraction | scoring | [Scoring and Extraction](scoring-and-extraction.md) |
| Append → seal → summarise hierarchy | `src/memory/tree/` | [Summary Trees](memory-tree.md) |
| Vector / keyword / graph / tree / hybrid search | retrieval | [Retrieval](retrieval.md) |
| Git-backed snapshots and read-markers | diff | [Diff Layer](diff-layer.md) |
| Entity files + co-occurrence graph | entities | [Entities and Graph](entities-and-graph.md) |
| Goals + tool-scoped rules | goals / tool_memory | [Goals and Tool Memory](goals-and-tool-memory.md) |
| Transcripts → summary-tree leaves | conversations / archivist | [Conversations and Archivist](conversations-and-archivist.md) |
| Async jobs (extract, append, seal, flush-stale, re-embed) | queue | [Job Queue](job-queue.md) |

The crate root (`src/lib.rs`) re-exports just `pub mod memory`; everything public is reached through it. See the [Architecture Overview](architecture.md) for how the layers fit together and the [Core Concepts](core-concepts.md) for invariants like local-first markdown authority, rebuildable derived indexes, provenance, and the security `taint`.

## Build and test from source

Clone the canonical repo and use the standard Cargo workflow:

```bash
git clone https://github.com/tinyhumansai/tinycortex
cd tinycortex

cargo check          # fast type-check without running tests
cargo test           # unit tests (in src/) + integration tests (in tests/)
cargo fmt --all      # format the crate
cargo doc --open     # build and open the API reference
```

The smoke test in `tests/smoke.rs` is the smallest end-to-end check — it inserts a record and asserts a keyword search finds it:

```rust
let store = InMemoryMemoryStore::new();
store
    .insert(MemoryInput::new("default", "TinyCortex starts as a Rust memory core"))
    .await
    .expect("insert memory");

let hits = store
    .search(MemoryQuery::text("Rust memory"))
    .await
    .expect("search memory");

assert_eq!(hits.len(), 1);
assert_eq!(hits[0].record.namespace, "default");
```

Heavier dependencies are bundled, so a from-source build is self-contained: `rusqlite` is pulled in with the `bundled` feature (no system SQLite needed), and `git2` backs the diff ledger when the optional `git-diff` feature is enabled. `tempfile` is a dev-only dependency used by the tests. Tokio appears twice: as a dev-dependency for the test suite, and as a **real optional dependency** behind the crate's `tokio` feature, which powers the async job-queue worker loops in `memory::queue::runtime`. With the default (empty) feature set, none of these optional dependencies are compiled.

## See also

- [Architecture Overview](architecture.md) — how the layers fit together
- [Core Concepts](core-concepts.md) — invariants, provenance, and the security taint
- [Storage Primitives](storage-primitives.md) — the markdown vault and derived indexes
- [Building and Contributing](contributing.md) — full dev workflow and conventions
