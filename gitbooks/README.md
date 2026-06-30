---
description: The local-first AI memory engine — a Rust crate that learns what matters and forgets the noise.
---

# Introduction

# TinyCortex 🧠

**TinyCortex** is a local-first AI memory engine, shipped as the open-source Rust crate [`tinycortex`](https://crates.io/crates/tinycortex). It gives your agents a memory that works the way a brain does: it **intelligently forgets noise** so the model only reasons over what matters.

Every AI memory system you have used does the same thing — store everything, retrieve by similarity, hope for the best. The outcome is an agent that drowns in stale context: responses degrade and costs inflate. TinyCortex takes the opposite approach. Low-value memories **decay** over time, while the knowledge your users recall and interact with is **reinforced** and rises to the top. There is no manual cleanup and no context-window anxiety.

The engine ingests content, canonicalizes and chunks it, scores what is worth keeping, and **compresses** it into a hierarchical summary tree. Retrieval then serves a focused, explainable slice of long-term history — vector, keyword, graph, and tree search combined — instead of a noisy dump of everything ever stored.

{% hint style="info" %}
This documentation covers the **open-source Rust crate**. The hosted TinyCortex platform (managed API, language SDKs) is a separate product in **closed alpha** — [reach out](mailto:founders@tinyhumans.ai) for access. Crate-only vs. hosted-only capabilities are called out throughout.
{% endhint %}

## Quickstart

```bash
cargo add tinycortex
```

```rust
use tinycortex::memory::{InMemoryMemoryStore, MemoryInput, MemoryQuery, MemoryStore};

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    let store = InMemoryMemoryStore::new();

    store
        .insert(MemoryInput::new("preferences", "User prefers dark mode"))
        .await?;

    let hits = store.search(MemoryQuery::text("theme preference")).await?;
    for hit in hits {
        println!("{:.3}  {}", hit.score, hit.record.content);
    }
    Ok(())
}
```

See **[Getting Started](getting-started.md)** for the full walkthrough, or jump to the **[Architecture Overview](architecture.md)** to understand how the engine fits together.

## Why TinyCortex

* **Intelligent noise filtering** — memories that are not accessed decay; frequently recalled knowledge becomes durable. The store stays lean on its own.
* **Interaction-aware** — views, replies, reactions, and authored content all signal what matters.
* **Local-first & inspectable** — markdown files are the source of truth; SQLite, vectors, summary trees, and a git ledger are rebuildable derived indexes.
* **Explainable retrieval** — every hit carries a score breakdown across graph, vector, keyword, and freshness signals.
* **Provenance & safety** — every item carries source identity and a security `taint` (internal vs. external-sync).

## Where to go next

| If you want to… | Read |
| --------------- | ---- |
| Install and run your first store | [Getting Started](getting-started.md) |
| Understand the layered design | [Architecture Overview](architecture.md) |
| Learn the vocabulary (namespaces, taint, decay, recall) | [Core Concepts](core-concepts.md) |
| See how memories are compressed into a tree | [Memory Tree & Compression](memory-tree.md) |
| Query memory | [Retrieval](retrieval.md) |
| Read the generated API reference | [docs.rs/tinycortex](https://docs.rs/tinycortex) |

[Discord](https://discord.tinyhumans.ai) • [Reddit](https://www.reddit.com/r/tinyhumansai/) • [X](https://x.com/tinyhumansai) • [crates.io](https://crates.io/crates/tinycortex)
