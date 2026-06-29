# Architecture Overview

TinyCortex is a memory engine that gives AI agents persistent, structured long-term memory. Instead of stuffing raw conversation history into ever-growing context windows, TinyCortex compresses information by roughly 1,000:1 using a five-stage pipeline inspired by how biological memory actually works.

This page covers the compression pipeline, how TinyCortex compares to other approaches, and why the architecture matters at scale.

<figure><img src="../.gitbook/assets/compression-pipeline@2x.png" alt=""><figcaption></figcaption></figure>

### The Five-Stage Compression Pipeline

Every conversation that enters TinyCortex passes through five stages before it reaches long-term storage.

#### Stage 1: Ingest

Raw conversation text is received via the API. This is the unprocessed input: full message history, system prompts, tool outputs, and user replies.

#### Stage 2: Selective Context

Low-information tokens are identified and removed. Filler phrases, redundant acknowledgments, and boilerplate content are stripped while preserving semantic meaning. This stage alone achieves roughly 12:1 compression.

The approach draws on research from the EMNLP 2023 Selective Context paper, which demonstrated that LLMs can identify and remove tokens with low self-information without meaningful loss in downstream task performance.

#### Stage 3: Graph Extraction

Entities (people, places, concepts, preferences) and their relationships are extracted from the remaining text and mapped into a knowledge graph. Rather than storing flat text chunks, TinyCortex builds a structured representation of what was actually said.

This is the GraphRAG layer. It allows retrieval to follow relationships ("What does this user think about X, and how does that connect to Y?") rather than relying on cosine similarity between embedding vectors.

This stage contributes an additional \~8:1 compression.

#### Stage 4: Ebbinghaus Decay

Inspired by Hermann Ebbinghaus's 1885 research on human memory retention, this stage applies time-weighted relevance scoring to every node and edge in the graph.

* Memories that are accessed frequently stay in the HOT tier (high retrieval priority)
* Memories that go unaccessed gradually decay through WARM, COOL, and COLD tiers
* Each interaction with a memory "boosts" it back up the curve, mimicking the spacing effect in human learning
* Memories in the COLD tier are not deleted but are deprioritized during retrieval

This means TinyCortex naturally surfaces what matters most, without requiring manual curation or rigid TTL expiration. The decay curve contributes roughly 10:1 additional compression by deprioritizing stale information during retrieval.

#### Stage 5: Compressed Store

The final knowledge graph is stored in a compact, query-optimized format. The total compression from raw input to stored graph is approximately 1,000:1.

At query time, TinyCortex traverses the graph to assemble contextually relevant memory, then returns it as structured context that any LLM can consume.

### Competitive Landscape

<figure><img src="../.gitbook/assets/competitive-landscape@2x.png" alt=""><figcaption></figcaption></figure>

TinyCortex is not the only memory layer for AI agents, but it takes a fundamentally different approach from most alternatives.

**Key differences:**

* **Graph vs. vector:** Most memory systems store flat embeddings and retrieve by similarity. TinyCortex stores structured relationships and retrieves by traversal. This is the difference between "find text that looks similar" and "follow the connections to find what's relevant."
* **Decay vs. accumulation:** Without decay, memory systems accumulate noise over time. Every outdated preference, corrected fact, and irrelevant detail stays at equal priority. TinyCortex's decay curve handles this automatically.
* **Compression depth:** A 1,000:1 ratio means TinyCortex can store roughly 1,000x more conversation history in the same storage footprint. This is what makes the cost difference possible.

### Why This Matters at Scale

<figure><img src="/broken/files/YDXpj1kTc1zgvfPM8n28" alt=""><figcaption></figcaption></figure>

The economics of AI memory change dramatically depending on architecture.

For a deployment serving 100,000 users, each maintaining ongoing conversation history:

* **Frontier model context window** (storing everything in-context): \~$90,000 per user per year. This is the "just make the context window bigger" approach. It works for demos. It does not work at scale.
* **Standard RAG pipeline** (vector DB + retrieval): \~$2.40 per user per year. A massive improvement, but still limited by embedding storage costs and retrieval quality.
* **TinyCortex**: \~$0.72 per user per year. The 1,000:1 compression ratio translates directly to storage and compute savings.

The cost gap widens with usage. The more conversations each user has, the more compression matters.

### vs. Mem0

<figure><img src="../.gitbook/assets/latency-index-cost@2x.png" alt=""><figcaption></figcaption></figure>

Mem0 is the most commonly compared alternative, so it is worth a direct comparison.

**Retrieval latency** (average query response time):

TinyCortex 0.8s, fastgraphrag 1.2s, supermemory 2.4s, mem0 3.1s, gemini\_vdb 16.0s.

**Index cost per conversation:** TinyCortex $0.0004 vs. mem0 $0.0112. That is a 28x difference in indexing cost. For applications that process thousands of conversations daily, this compounds quickly.

The latency advantage comes from graph traversal (direct relationship lookups) versus mem0's hybrid approach, which combines multiple retrieval strategies at query time.

### Benchmark Results

<figure><img src="../.gitbook/assets/benchmarks@2x.png" alt=""><figcaption></figcaption></figure>

All five systems were evaluated on the same Sherlock Holmes corpus using RAGAS 0.4.x with GPT-4o as the judge model.

**Headline numbers for TinyCortex:**

* Answer Relevancy: 0.97 (highest)
* Context Precision: 0.75
* Faithfulness: 0.73
* Answer Correctness: 0.57
* Context Recall: 0.62

TinyCortex leads in Answer Relevancy by a significant margin (0.97 vs. next best 0.88). Performance on other metrics is competitive, with trade-offs depending on the evaluation dimension.
