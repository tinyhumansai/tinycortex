---
description: The hub for TinyCortex's design — layered architecture, end-to-end data flow, per-module map, core invariants, the strict layer-dependency rule, and the ownership boundary with the host.
---

# Architecture Overview

TinyCortex (crate [`tinycortex`](https://crates.io/crates/tinycortex)) is the open-source Rust core of the TinyCortex memory system: a **local-first, config-driven** memory engine ported from OpenHuman. It is a library you embed, not a hosted service. The entire public surface lives under one module — [`memory`](core-concepts.md) — declared in `src/lib.rs`, and re-exported through `src/memory/mod.rs`.

This page is the hub for the engine's design: the layered architecture, the end-to-end data flow, the per-module map, the core invariants, the strict layer-dependency rule, and the ownership boundary with OpenHuman/the host.

{% hint style="info" %}
Scope note: TinyCortex owns memory *processing* (validation, canonical inputs, storage/index contracts, chunking, tree updates, diffing, retrieval, provenance). It does **not** own memory *sync* — the host decides when to ingest and supplies the payloads. See [Ownership Boundary](#ownership-boundary).
{% endhint %}

---

## The layered model

The engine is organized bottom-up, from durable storage primitives to async orchestration. Each layer depends only on the layers below it. The crate-level docs in `src/memory/mod.rs` state the rule directly:

> orchestration and ingestion depend on storage; storage never depends upward on orchestration, tools, or agents.

```text
┌─────────────────────────────────────────────────────────────┐
│  queue          async job model (extract, append, seal,      │
│                 flush_stale, reembed_backfill, seal_document) │
├─────────────────────────────────────────────────────────────┤
│  conversations · archivist     transcript log → tree leaves   │
│  goals · tool_memory           specialized long-term surfaces │
├─────────────────────────────────────────────────────────────┤
│  diff           git-backed source snapshots & change tracking │
│  entities · graph   entity files + derived co-occurrence graph│
├─────────────────────────────────────────────────────────────┤
│  retrieval      vector / keyword / graph / tree / hybrid      │
├─────────────────────────────────────────────────────────────┤
│  ingest         canonicalize → raw md → chunk → score → tree  │
│  tree           append leaves, seal buffers, summarize        │
│  score          scoring, entity extraction, embedding         │
│  chunks         deterministic chunk ids + metadata            │
│  sources        source registry contracts + validation        │
├─────────────────────────────────────────────────────────────┤
│  store          storage primitives (SOURCE OF TRUTH below)    │
│                 content · chunks · trees · vectors · kv ·     │
│                 entity_index · safety                         │
├─────────────────────────────────────────────────────────────┤
│  config · error · traits · types     stable shared contracts  │
└─────────────────────────────────────────────────────────────┘
```

The bottom band (`config`, `error`, `traits`, `types`) is the stable contract surface every other layer builds on. The `store` layer holds the actual persistence substrate. Everything above is processing and orchestration.

---

## End-to-end data flow

Once the host hands TinyCortex a source-scoped payload, every provider lands data through the same path (from the spec's ingest pipeline; see [Ingest Pipeline](ingest-pipeline.md)):

```text
source reader / sync provider          (host-owned, outside the crate)
  ─────────────────── ownership boundary ───────────────────
  -> canonicalize          normalize payload shape → CanonicalisedSource
  -> write raw markdown     immutable body in content/ (source of truth)
  -> chunk                  deterministic ids over source kind/id/seq/content
  -> score / extract / embed   tree-entry decision + entities + 768-d vector
  -> persist chunk metadata    SQLite rows pointing at content_path/sha256
  -> enqueue tree jobs      mem_tree_jobs (extract, append, seal, …)
  -> append to buffers      unsealed frontier per (tree_id, level)
  -> seal summaries         immutable summary nodes once a bucket fills
  -> update retrieval indexes
```

The reverse path — **retrieval** — exposes deterministic primitives (`query_source`, `query_global`, `query_topic`, `search_entities`, `drill_down`, `fetch_leaves`) and leaves composition to the caller. Hybrid search blends graph/vector/keyword/freshness signals via named weight profiles, and every hit carries an explainable [`RetrievalScoreBreakdown`](retrieval.md). See [Retrieval](retrieval.md).

---

## Module map

One row per top-level module under `src/memory/` (declared in `src/memory/mod.rs`). Paths are relative to `src/memory/`.

| Module | Path | Responsibility | Page |
| --- | --- | --- | --- |
| `config` | `config.rs` | `MemoryConfig`, `WeightProfile` and engine configuration | [Retrieval](retrieval.md) |
| `error` | `error.rs` | `MemoryError`/`MemoryEngineError` and `MemoryEngineResult` | — |
| `traits` | `traits.rs` | The core `Memory` trait (store/recall/delete/list/summarize) | [Core Concepts](core-concepts.md) |
| `types` | `types.rs` | Shared domain types: `MemoryEntry`, `MemoryTaint`, namespace docs, retrieval hits, score breakdown | [Core Concepts](core-concepts.md) |
| `store` | `store/` | Storage primitives: `content/`, chunks, `trees/`, `vectors/`, `kv.rs`, `entity_index/`, `safety.rs`; also the starter `InMemoryMemoryStore` | [Storage Primitives](storage-primitives.md) |
| `chunks` | `chunks/` | Canonical chunk model + deterministic ids | [Ingest Pipeline](ingest-pipeline.md) |
| `sources` | `sources/` | Source registry contracts and validation | [Sources](sources.md) |
| `score` | `score/` | Scoring signals, entity extraction, embedding | [Scoring and Extraction](scoring-and-extraction.md) |
| `tree` | `tree/` | Summary-tree mechanics: append, seal, summarize, score, embed, retrieve | [Summary Trees](memory-tree.md) |
| `queue` | `queue/` | Async job model: extract, append, seal, flush, backfill, document seal | [Job Queue](job-queue.md) |
| `retrieval` | `retrieval/` | Vector / keyword / graph / tree / hybrid search | [Retrieval](retrieval.md) |
| `diff` | `diff/` | Git-backed source snapshots, diffs, checkpoints, read markers | [Diff Layer](diff-layer.md) |
| `entities` | `entities/` | Entity markdown files (`entities/<kind>/<canonical_id>.md`) | [Entities and Graph](entities-and-graph.md) |
| `graph` | `graph/` | Co-occurrence graph derived from the entity index | [Entities and Graph](entities-and-graph.md) |
| `goals` | `goals/` | Compact long-term goal list (`MEMORY_GOALS.md`) | [Goals and Tool Memory](goals-and-tool-memory.md) |
| `tool_memory` | `tool_memory/` | Durable tool-scoped rules in `tool-{tool_name}` namespaces | [Goals and Tool Memory](goals-and-tool-memory.md) |
| `conversations` | `conversations/` | Thread metadata + message JSONL transcript storage | [Conversations and Archivist](conversations-and-archivist.md) |
| `archivist` | `archivist/` | Converts conversation turns into tree leaves | [Conversations and Archivist](conversations-and-archivist.md) |
| `ingest` | `ingest/` | The canonicalize → chunk → score → tree pipeline | [Ingest Pipeline](ingest-pipeline.md) |

The public re-exports that pull these together live at the bottom of `src/memory/mod.rs`: the high-level contracts (`Memory`, `MemoryConfig`, `WeightProfile`, `MemoryEntry`, `MemoryTaint`, namespace types, `RetrievalScoreBreakdown`, `GLOBAL_NAMESPACE`) plus the starter `InMemoryMemoryStore` / `MemoryStore` API used by the smoke test and as a simple reference backend.

---

## Core invariants

These properties hold across every layer (from the spec's "core properties" and content invariants):

- **Local-first & authoritative markdown.** User workspace files and local indexes are authoritative. Immutable markdown content files under `content/` are the **source of truth** for bodies; chunk markdown body bytes are immutable after write, and sealed summary rows are immutable.
- **Derived indexes are rebuildable.** SQLite chunk rows, summary-tree rows, the local vector DB, KV records, and the entity-occurrence index accelerate reads but must be rebuildable from canonical content where possible. SQLite stores pointers such as `content_path` and `content_sha256`, not duplicate bodies.
- **Durable provenance.** Every item carries source identity, timestamps, and a security taint.
- **Taint fails closed.** [`MemoryTaint`](core-concepts.md) is a security contract: external sync sources (Gmail, Slack, Notion, Composio, MCP, …) are stored as `external_sync` so automation can refuse external-effect tools when tainted context is present. **Unknown persisted taint values must decode as `external`.**
- **Inspectable content.** Obsidian-readable markdown is a first-class product surface, not an export. Body keyword search reads markdown files rather than a duplicate body index.
- **Strict layer boundaries.** Ingestion and orchestration depend on storage; storage must never depend upward on orchestration, tools, or agents.

---

## The layer-dependency rule

The single most important structural constraint is one-directional dependency:

```text
        depends on
queue ───────────────▶ retrieval / diff / entities ─────▶ tree / ingest
                                                              │
                                                              ▼
                                                            store
                                                              │
                                                              ▼
                                              config · error · traits · types
```

Storage primitives (`store`) and the shared contracts beneath them know nothing about ingest, retrieval, queues, tools, or agents. This keeps the storage substrate independently testable, lets derived indexes be rebuilt without touching orchestration, and makes the engine embeddable: a host can drive the lower layers directly without pulling in the job queue or higher-level surfaces. New storage kinds (`raw`, `chunk`, `entity`, `tree`, `vector`, `kv`, `contact`) are added at the bottom and surfaced upward, never the reverse.

---

## Ownership boundary

TinyCortex does **not** own memory sync. From the spec's Ownership Boundary section:

> OpenHuman decides when data should be ingested, owns the upstream trigger path, and invokes TinyCortex on demand with already selected source payloads or canonical ingest requests. TinyCortex owns the memory engine contracts and processing semantics after that boundary.

```text
   HOST / OpenHuman                    │            TinyCortex (this crate)
   ──────────────────                  │            ──────────────────────
   • decides WHEN to sync              │  payload   • validate & canonicalize
   • owns OAuth / webhooks / polling   │  ───────▶  • chunk · score · embed
   • selects source payloads           │            • build summary trees
   • runs the sync runner / pipelines  │            • diff · retrieve · provenance
                                       │
                          ownership boundary
```

In this crate, "ingest" means *process a host-supplied memory payload through TinyCortex contracts* — it does **not** mean TinyCortex polls apps, owns OAuth/webhook callbacks, or decides when to sync. TinyCortex *models* source records (identity, provenance, validation, diffing depend on them) and preserves trait-compatible sync/pipeline contracts for integration and tests, but the production sync runner stays host-owned. See [Sources](sources.md).

{% hint style="info" %}
Hosted-platform note: managed APIs, a turnkey "conscious recall" product, and per-user cost figures belong to the hosted TinyCortex platform, not this open-source crate. This crate is the embeddable Rust engine only.
{% endhint %}

---

## What's runnable today vs. described

The reliably end-to-end runnable surface today is the starter store: `InMemoryMemoryStore` with `MemoryInput` / `MemoryQuery` / `MemoryResult` / `SearchHit` (the `MemoryStore` trait), plus the high-level `Memory` trait. The deeper engine layers (`tree`, `queue`, `diff`, etc.) are documented here by their real types, fields, enum wire-strings, and contracts. See [Getting Started](getting-started.md) for a working example against the starter store.

---

## See also

- [Core Concepts](core-concepts.md)
- [Storage Primitives](storage-primitives.md)
- [Ingest Pipeline](ingest-pipeline.md)
- [Getting Started](getting-started.md)
