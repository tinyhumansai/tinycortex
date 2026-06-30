# TinyCortex Wiki 🧠

**TinyCortex** is the open-source **Rust core** of the TinyCortex memory system — a local-first, config-driven AI memory engine. It is published on [crates.io](https://crates.io/crates/tinycortex) as [`tinycortex`](https://crates.io/crates/tinycortex) and embedded as a library in your own agent, service, or app.

Inspired by how the human brain compresses experience, TinyCortex **intelligently forgets noise**: low-value memories decay while the knowledge you recall and interact with is reinforced. The result is a memory engine that stays lean, scales to millions of tokens, and gets sharper with use.

> This wiki documents the **Rust crate**. The hosted TinyCortex platform (managed API, turnkey conscious recall) is a separate, closed-alpha product — capabilities that belong to it rather than the crate are called out where relevant.

## Start here

- **[Getting Started](Getting-Started)** — `cargo add tinycortex` and your first store + recall.
- **[Architecture Overview](Architecture-Overview)** — the layered engine and how data flows through it.
- **[Core Concepts](Core-Concepts)** — memory items, namespaces, taint, decay, and recall.

## The engine, layer by layer

| Page | What it covers |
| ---- | -------------- |
| [Storage Primitives](Storage-Primitives) | Content store, vectors, KV, entity index — what's authoritative vs. derived |
| [Ingest Pipeline](Ingest-Pipeline) | Canonicalize → chunk → score → embed → tree |
| [Scoring & Extraction](Scoring-and-Extraction) | Signals, entity extractors, embeddings |
| [Summary Trees](Summary-Trees) | Append, seal, summarise — the hierarchical memory |
| [Retrieval](Retrieval) | Vector / keyword / graph / tree / hybrid search |
| [Job Queue](Job-Queue) | The async work model behind ingest and sealing |
| [Diff Layer](Diff-Layer) | Git-backed snapshots and change awareness |
| [Entities & Graph](Entities-and-Graph) | Entity files and the co-occurrence graph |
| [Sources](Sources) | Source registry, readers, and the sync boundary |
| [Goals & Tool Memory](Goals-and-Tool-Memory) | Specialized long-term surfaces |
| [Conversations & Archivist](Conversations-and-Archivist) | Transcript storage and tree archival |

## Reference

- **[Benchmarks](Benchmarks)** — RAGAS, TemporalBench, Vending-Bench, BABILong, and more.
- **[FAQ](FAQ)** — common questions about the crate.
- **[Building & Contributing](Building-and-Contributing)** — build, test, and contribute.
- **[API reference (docs.rs)](https://docs.rs/tinycortex)** — generated rustdoc.

## Community

[Discord](https://discord.tinyhumans.ai) • [Reddit](https://www.reddit.com/r/tinyhumansai/) • [X](https://x.com/tinyhumansai)
