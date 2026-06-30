<div align="center">
<img src="./docs/images/image-neocortex.png" style="max-width: 400px"/>
<h1>TinyCortex 🧠 — Human-like AI Memory in Rust</h1>

**Forgets the noise ◦ 10M+ token context ◦ ~4000 tokens/sec ◦ Conscious Recall**

[![Crates.io](https://img.shields.io/crates/v/tinycortex.svg)](https://crates.io/crates/tinycortex)
[![docs.rs](https://img.shields.io/docsrs/tinycortex)](https://docs.rs/tinycortex)
[![License: MIT](https://img.shields.io/badge/license-MIT-blue.svg)](./LICENSE)

[Discord](https://discord.tinyhumans.ai) • [Reddit](https://www.reddit.com/r/tinyhumansai/) • [X](https://x.com/tinyhumansai) • [Docs](https://tinyhumans.gitbook.io/tinycortex/)

#### [Benchmarks](#-benchmarks) • [Getting Started](#-getting-started) • [Architecture](https://tinyhumans.gitbook.io/tinycortex/) • [Read the paper](./paper/README.md)

</div>

The human brain is a master at compression. It doesn't try to remember every passing detail; instead it aggressively prunes noise to keep a sharp, focused, easily accessible recall of what truly matters. Traditional AI memory systems do the opposite — they try to remember _everything_ and retrieve whatever is _similar_. But similar doesn't mean important. The result? Your AI drowns in stale, irrelevant context that degrades every response.

**TinyCortex** takes the brain's approach: it **intelligently forgets noise**. Low-value memories naturally decay while the knowledge you recall, interact with, and build upon is reinforced. The result is a memory engine that stays lean and focused, chops through 10M+ tokens accurately at up to ~4000 tokens/second, and gets sharper with every interaction.

TinyCortex scores extremely highly on [RAGAS](https://www.ragas.io/), [BABILong](https://github.com/booydar/babilong/), [Vending-Bench](https://andonlabs.com/evals/vending-bench-2), [LoCoMo](https://github.com/snap-research/locomo), and [HotPotQA](https://hotpotqa.github.io/).

> **What this repository is:** the open-source **Rust core** of TinyCortex — the local-first memory engine — published on [crates.io](https://crates.io/crates/tinycortex) as [`tinycortex`](https://crates.io/crates/tinycortex). It is a _library_: you embed it in your own agent, service, or app. The hosted TinyCortex platform is currently in closed alpha — [reach out](mailto:founders@tinyhumans.ai) for access.

# 🎯 Core Features

## Intelligent Noise Filtering

Memories that aren't accessed naturally decay over time; frequently recalled knowledge becomes more durable. The store stays lean on its own — no manual cleanup.

![Interaction graph highlighting important knowledge](docs/images/gif/AppleEmailGraph.gif)

## Interaction-Aware Scoring

Not all memories are equal. Views, replies, reactions, mentions, and authored content all signal what matters. Knowledge people engage with rises to the top; ignored information fades away.

![Memory decay over time](docs/images/gif/BobMemoryDecayVideo.gif)

## Local-First & Inspectable

Markdown files are the source of truth. SQLite chunk rows, summary trees, vectors, and a git-backed change ledger are _derived_ indexes that accelerate reads and can be rebuilt from canonical content. Every item carries source provenance and a security `taint` (internal vs. external-sync).

## Conscious Recall

Conscious recall proactively surfaces the most relevant memories for the current moment instead of waiting for an explicit query. It tracks recent activity, combines it with time-based decay, and pulls forward the memories that are both recent and repeatedly interacted with — a focused slice of long-term history rather than a noisy dump.

# ⚡ Getting Started

TinyCortex is a Rust library. Add it to your project:

```bash
cargo add tinycortex
```

Store and recall a memory with the built-in in-memory backend:

```rust
use tinycortex::memory::{InMemoryMemoryStore, MemoryInput, MemoryQuery, MemoryStore};

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    let store = InMemoryMemoryStore::new();

    // Store a memory in the "preferences" namespace.
    store
        .insert(MemoryInput::new("preferences", "User prefers dark mode"))
        .await?;

    // Recall it with a keyword query.
    let hits = store.search(MemoryQuery::text("theme preference")).await?;
    for hit in hits {
        println!("{:.3}  {}", hit.score, hit.record.content);
    }
    Ok(())
}
```

The `InMemoryMemoryStore` is the simple reference backend. The full engine — content store, chunking, scoring, summary trees, vector/keyword/graph/hybrid retrieval, the diff ledger, and the async job queue — lives under the [`memory`](https://docs.rs/tinycortex/latest/tinycortex/memory/) module. See the **[documentation](https://tinyhumans.gitbook.io/tinycortex/)** for the architecture, concepts, and end-to-end ingest walkthroughs.

# 🧩 How It Works

TinyCortex is a layered, local-first engine. Content flows through a single ingest pipeline and is served by deterministic retrieval primitives:

```text
source payload
  → canonicalize        normalize chat / email / document into markdown
  → write raw markdown   immutable body files are the source of truth
  → chunk                atomic, deterministically-id'd units
  → score / extract / embed   decide what's worth remembering
  → enqueue tree jobs    append → seal → summarise (async queue)
  → retrieval indexes    vector · keyword · graph · summary tree
```

| Layer                         | What it does                                                                                 |
| ----------------------------- | -------------------------------------------------------------------------------------------- |
| **Storage primitives**        | Markdown content store, SQLite chunks, summary trees, vector DB, KV, entity index            |
| **Ingest**                    | Canonicalize → chunk → score → embed → tree                                                  |
| **Retrieval**                 | Vector, keyword, graph, tree drill-down, and hybrid search with explainable score breakdowns |
| **Diff**                      | Git-backed source snapshots, checkpoints, and read-markers for change awareness              |
| **Entities & Graph**          | Entity markdown files + a co-occurrence graph derived from the entity index                  |
| **Goals / Tool Memory**       | Compact long-term goal list and durable tool-scoped rules                                    |
| **Conversations / Archivist** | Transcript storage and conversion of turns into summary-tree leaves                          |
| **Queue**                     | Async jobs: extract, append, seal, flush-stale, re-embed, seal-document                      |

Full details live in the **[documentation](https://tinyhumans.gitbook.io/tinycortex/)**.

# 📈 Benchmarks

### RAGAS — Retrieval Quality (Sherlock Holmes Corpus)

Standard RAG quality metrics via [RAGAS](https://docs.ragas.io/). TinyCortex leads in **Answer Relevancy (0.97)** and **Context Precision (0.75)**, outperforming FastGraphRAG, Gemini VDB, Mem0, and SuperMemory.

![ragas](docs/images/chart_ragas.png)

### TemporalBench — Temporal Reasoning

Accuracy across ordering, state-at-time, recency, interval, and sequence questions. TinyCortex hits **100% on recency** — surfacing the most recent events thanks to its time-decay model.

![chart_temporalbench](docs/images/chart_temporalbench.png)

### Vending-Bench — Agentic Decision-Making

An agent runs a simulated vending-machine business over 30 days. TinyCortex achieves the **highest cumulative P&L (~$295 by day 30)** — better memory leads to better long-horizon decisions.

![chart_vendingbench](docs/images/chart_vendingbench.png)

See [`benchmarks/`](./benchmarks/README.md) to reproduce these on your own corpus.

# 📚 Documentation

- **[Documentation (GitBook)](https://tinyhumans.gitbook.io/tinycortex/)** — architecture, concepts, getting started, and FAQ
- **[docs.rs/tinycortex](https://docs.rs/tinycortex)** — generated API reference
- **[The paper](./paper/README.md)** — the research behind the memory model
- **[CONTRIBUTING.md](./CONTRIBUTING.md)** — how to build, test, and contribute

# 🤝 Contributing

TinyCortex is built in Rust (2021 edition). Clone the repo and:

```bash
cargo test     # run unit + integration tests
cargo doc      # build the API docs
cargo fmt      # format
```

Issues and PRs are welcome — see [CONTRIBUTING.md](./CONTRIBUTING.md).

---

# ⭐ Star us on GitHub

_Like contributing towards AGI 🧠? Give this repo a star and spread the love ❤️_

<p align="center">
  <a href="https://www.star-history.com/#tinyhumansai/tinycortex&type=date&legend=top-left">
    <picture>
     <source media="(prefers-color-scheme: dark)" srcset="https://api.star-history.com/svg?repos=tinyhumansai/tinycortex&type=date&theme=dark&legend=top-left" />
     <source media="(prefers-color-scheme: light)" srcset="https://api.star-history.com/svg?repos=tinyhumansai/tinycortex&type=date&legend=top-left" />
     <img alt="Star History Chart" src="https://api.star-history.com/svg?repos=tinyhumansai/tinycortex&type=date&legend=top-left" />
    </picture>
  </a>
</p>

# Contributors Hall of Fame

Show some love and end up in the hall of fame.

<a href="https://github.com/tinyhumansai/tinycortex/graphs/contributors">
  <img src="https://contrib.rocks/image?repo=tinyhumansai/tinycortex" alt="TinyCortex contributors" />
</a>
</content>
