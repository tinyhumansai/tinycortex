# FAQ

A developer FAQ for the **`tinycortex`** crate — the open-source Rust core of the
TinyCortex memory system. If you are looking for conceptual depth, follow the
links into the rest of the wiki; this page answers the common "what is this /
how do I…" questions and corrects assumptions carried over from the old hosted
framing.

## What is TinyCortex?

TinyCortex is a **local-first, config-driven AI memory engine, shipped as a Rust
library**. You embed the crate in your own application so an agent can ingest
source-scoped payloads, canonicalize and chunk them, score/extract/embed them,
build summary trees, and retrieve explainable context across sessions.

The crate is published as [`tinycortex`](https://crates.io/crates/tinycortex) and
its public surface lives under the `memory` module (see `src/lib.rs`). It was
ported from OpenHuman's memory engine into a standalone, test-driven crate.

## Is TinyCortex a hosted service or an API I call?

No. **This repository is a Rust library, not a hosted service.** There is no API
key, no `pip install`, no JS/Python SDK, and no managed client to sign up for in
this crate. You add `tinycortex` to your `Cargo.toml` and call it in-process.

A separate managed/hosted platform may exist commercially (with its own pricing,
managed ingestion, and "conscious recall" product framing), but **none of that is
part of this crate** and is out of scope for this wiki. Anything you see in older
docs about per-token API pricing, closed-alpha API keys, or multi-language SDKs
refers to that hosted product, not the open-source library.

## What languages are supported?

**Rust only.** The crate is Rust 2021. There are no first-party Python,
TypeScript, LangGraph, or other-language bindings in this repository. You consume
it from Rust like any other crate.

## What is the simplest thing I can run today?

The reliably end-to-end runnable surface is the `MemoryStore` contract and its
in-process reference implementation, `InMemoryMemoryStore`, together with
`MemoryInput` / `MemoryQuery` / `SearchHit` (all re-exported from
`crate::memory`).

```rust
use tinycortex::memory::{InMemoryMemoryStore, MemoryInput, MemoryQuery, MemoryStore};

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    let store = InMemoryMemoryStore::new();

    let record = store
        .insert(MemoryInput::new("profile", "User prefers dark mode"))
        .await?;
    println!("stored {}", record.id);

    let hits = store
        .search(MemoryQuery {
            namespace: Some("profile".into()),
            text: Some("dark".into()),
            limit: Some(10),
        })
        .await?;

    for hit in hits {
        println!("{:.2}  {}", hit.score, hit.record.content);
    }
    Ok(())
}
```

`InMemoryMemoryStore` is a volatile `BTreeMap`-backed store (contents are lost on
drop) intended for tests and as the simplest conforming backend — not for durable
storage. See [Getting-Started](Getting-Started) and
[Storage-Primitives](Storage-Primitives).

## What is the core contract?

The high-level engine contract is the `Memory` trait (`src/memory/traits.rs`).
Any backend (SQLite, vector DB, in-memory, …) implements it. Its key methods:

| Method | Purpose |
| --- | --- |
| `store` / `store_with_taint` | Upsert an entry by `(namespace, key)`; the taint variant records provenance |
| `recall` | Keyword/semantic retrieval into `Vec<MemoryEntry>` |
| `recall_relevant_by_vector` | Vector-similarity-only recall above a threshold (defaults to empty) |
| `get` | Fetch one entry by exact `(namespace, key)` |
| `list` | List entries, optionally scoped by namespace/category/session |
| `forget` | Delete an entry; returns whether it existed |
| `namespace_summaries` / `count` / `health_check` | Discovery and ops |

`store`/`get`/`forget`/`list` are keyed by `(namespace, key)`, so ingest is an
**upsert**: the same key updates rather than creating a duplicate.

## How is data organized? What are namespaces?

A `namespace` is a free-form string that logically scopes storage and queries.
Each item belongs to a namespace, and for the `Memory` trait `(namespace, key)`
uniquely identifies an entry. Entries also carry a `MemoryCategory` for
organization:

| Category | Wire string | Meaning |
| --- | --- | --- |
| `Core` | `core` | Long-term foundational facts, preferences, permanent decisions |
| `Daily` | `daily` | Temporal logs / ephemeral state |
| `Conversation` | `conversation` | Context derived from active conversations |
| `Custom(String)` | the custom name | User/system-defined category |

See [Core-Concepts](Core-Concepts) and [Sources](Sources).

## Where is my data stored? Is it local-first?

Yes — **local-first**. The architectural source of truth is immutable markdown
content files on disk. SQLite chunk rows, the summary-tree, the local vector DB,
the KV store, and the entity-occurrence index are all **derived indexes** that
are rebuildable from the markdown vault. TinyCortex never makes a network call on
its own; everything runs in your process against local storage.

(The `InMemoryMemoryStore` reference backend is volatile and keeps nothing on
disk; durable storage and the authoritative markdown vault live in the higher
layers. See [Storage-Primitives](Storage-Primitives).)

## Who decides when to ingest?

TinyCortex does **not** own memory sync. The host (OpenHuman or your application)
decides *when* to ingest and supplies the source payloads; TinyCortex owns all
processing *after* that boundary — canonicalize → raw markdown → chunk →
score/extract/embed → tree jobs. See [Ingest-Pipeline](Ingest-Pipeline).

## What is "taint" and why does every item have one?

Every item carries a security provenance taint so callers can refuse
external-effect tools on untrusted context. The `MemoryTaint` enum
(`src/memory/types.rs`) has two variants:

| Variant | Wire string | Use |
| --- | --- | --- |
| `Internal` | `internal` | First-party content the host authored/trusts |
| `ExternalSync` | `external_sync` | Third-party text ingested via sync (Notion / Composio / MCP / …) |

Two important invariants enforced in code:

- **Defaults to `Internal`** for legacy rows / JSON with no persisted taint, so
  old data stays usable.
- **Fails closed**: unknown or unrecognized persisted values decode to the more
  restrictive `ExternalSync` (e.g. `from_db_str("")`, `"EXTERNAL_SYNC"`, and any
  future string all resolve to `ExternalSync`).

Sync paths ingesting third-party text MUST call `store_with_taint(..,
MemoryTaint::ExternalSync)`. See [Core-Concepts](Core-Concepts) and
[Sources](Sources).

## Is my data used to train models? Does raw data leave my machine?

The crate itself performs **no** network I/O and no training — it is a local
library operating on local files. Embedding and LLM calls only happen if *you*
wire in a backend that makes them (see below). What leaves your machine is
entirely a function of the adapters you plug in.

## How does decay / freshness work?

Retrieval ranking includes a **freshness** signal computed via exponential
half-life decay (`src/memory/retrieval/scoring.rs`):

```text
freshness(updated_at, now, half_life_days):
    if half_life_days <= 0      -> 1.0   (decay disabled)
    if updated_at in the future -> 1.0   (clock-skew clamp)
    else                        -> 0.5 ^ (age_days / half_life_days)
```

So an item updated *now* scores `1.0` on the freshness axis, and one
`half_life_days` old scores `0.5`. The default half-life is
`DEFAULT_FRESHNESS_HALF_LIFE_DAYS = 7.0` days. Freshness is one of four signals
folded into the final score under a weight profile (graph, vector, keyword,
freshness) — it does not delete data; it down-weights stale hits in ranking.

### Can I disable decay?

Yes, per the formula above: a non-positive `half_life_days` degrades to a hard
`1.0` (no decay), and the freshness weight in a profile can be set so freshness
contributes nothing. The named profiles in `crate::memory::config`
(`balanced`, `semantic`, `lexical`, `graph_first`) set different weights; unknown
profile names fall back to `balanced`. See [Retrieval](Retrieval) and
[Scoring-and-Extraction](Scoring-and-Extraction).

## How do I plug in embeddings?

Embeddings are abstracted behind the `Embedder` trait (`src/memory/score/embed.rs`).
The crate ships only the deterministic `InertEmbedder` (zero vectors) for tests —
**real backends (Ollama, OpenAI-compatible, cloud) are wired in by a host adapter
that implements `Embedder`**. Contract:

- `embed(&self, text) -> Result<Vec<f32>>` must return exactly `EMBEDDING_DIM`
  floats. `EMBEDDING_DIM` is fixed at **768** (`DEFAULT_EMBEDDING_DIM`); mixing
  dimensions mid-run corrupts cosine comparisons, so it is validated at the trait
  level.
- `embed_batch` defaults to sequential `embed` calls; override it to collapse N
  round-trips into one. Each input position gets its own `Result`, so one failing
  text does not strand the rest of the batch.

See [Scoring-and-Extraction](Scoring-and-Extraction).

## How do I plug in an LLM (entity extraction)?

LLM-backed entity extraction is abstracted behind the `ChatProvider` trait
(`src/memory/score/extract/llm.rs`). The crate ships **no** real implementation —
tests inject a mock, and hosts wire their own. A provider implements:

```text
trait ChatProvider {
    fn name(&self) -> &str;
    async fn chat_for_json(&self, prompt: &ChatPrompt) -> anyhow::Result<String>;
}
```

`LlmExtractorConfig` controls behavior: the target `model` (diagnostic only —
actual selection happens inside your `ChatProvider`), `allowed_kinds`,
`strict_kinds`, and optional topic emission. Extraction also has a dependency-free
regex backend, so the LLM path is optional. See
[Scoring-and-Extraction](Scoring-and-Extraction) and
[Entities-and-Graph](Entities-and-Graph).

## What is the license?

**MIT** (`LICENSE`, `Cargo.toml`), Copyright Tiny Humans Intelligence Inc. You
can embed and ship it under permissive terms.

## How do I build and test it?

- `cargo check` — quick validation
- `cargo test` — unit + integration tests
- `cargo fmt --all` — formatting

See [Building-and-Contributing](Building-and-Contributing).

## See also

- [Getting-Started](Getting-Started)
- [Architecture-Overview](Architecture-Overview)
- [Core-Concepts](Core-Concepts)
- [Retrieval](Retrieval)
