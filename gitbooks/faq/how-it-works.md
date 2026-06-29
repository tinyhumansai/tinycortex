# How it Works

### How does memory decay work?

Every memory item has a retention score that decreases over time following an exponential decay curve (inspired by the Ebbinghaus Forgetting Curve). Memories that aren't accessed gradually fade. Memories that are recalled, referenced, or interacted with get their retention reinforced.

This happens automatically. You don't need to write cleanup jobs or manually prune old data.

See [Memory Decay](../developers/concepts/memory-decay.md) for the full technical explanation.

### Can I disable memory decay?

Memory decay is a core part of how TinyCortex maintains retrieval quality at scale. Disabling it would effectively turn it into a standard vector store. If you have a use case where certain memories should never decay (compliance records, core identity facts), you can use interaction signals to keep them reinforced, or reach out to discuss your needs.

### What are interactions and how do they affect memory?

Interactions are signals that tell TinyCortex a piece of knowledge is important. Different types carry different weights:

* **Views**: Lowest signal, but repeated views compound
* **Reactions**: Moderate signal
* **Replies**: Strong signal
* **Content creation**: Strongest signal

When a memory receives interactions, its retention score is boosted. The system learns what matters organically, based on actual usage patterns.

See [Interactions](../developers/concepts/interactions.md) for more details.

### What is GraphRAG and why does it matter?

Standard RAG chunks documents, embeds them, and retrieves the most similar chunks. This works for simple lookups but fails when the answer requires connecting information across multiple sources.

GraphRAG extracts **entities** and **relations** from your data to build a knowledge graph. Queries traverse this graph to pull in contextually related information, not just the nearest embedding match.

Example: if you ask "What's the status of the Q4 project?", GraphRAG connects the project entity to related team members, recent updates, and deadline changes across different sources, even if those pieces don't share similar text.

#### What's the difference between ingest, recall, and context?

* **Ingest**: Store memories. Upsert operation (duplicate keys update rather than create duplicates).
* **Recall**: Retrieve memories based on semantic similarity, graph relationships, retention scores, and interaction weights.
* **Context**: Higher-level operation that retrieves and formats memories as a context block ready to inject into an LLM prompt.

***

#### Getting Help

* **Discord**: [discord.com/invite/k23Kn8nK](https://discord.com/invite/k23Kn8nK)
* **Reddit**: [r/alphahuman](https://www.reddit.com/r/alphahuman/)
* **GitHub**: [github.com/tinyhumansai](https://github.com/tinyhumansai)
* **Email**: [founders@tinyhumans.ai](mailto:founders@tinyhumans.ai)
