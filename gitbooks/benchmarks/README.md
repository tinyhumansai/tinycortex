# Benchmarks

Benchmark results for **TinyCortex Mark 1** (`mk1`). All benchmarks compare TinyCortex against other memory and RAG methods.

## Why Benchmarks Matter

Memory systems are only as good as the answers they help produce. These benchmarks test TinyCortex across four dimensions: retrieval quality, temporal reasoning, needle-in-a-haystack recall, and real-world decision-making.

## Methods Compared

| Method | Type |
| --- | --- |
| **tinycortex\_v1** | GraphRAG with time-decay and interaction weighting |
| fastgraphrag | Graph-based RAG |
| e2graphrag | Graph-based RAG |
| gemini\_vdb | Vector database (Gemini embeddings) |
| mem0 | Memory layer |
| supermemory | Memory layer |
| scratchpad | Simple key-value store |
| directfeed | Raw context window (no retrieval) |

## Evaluation Infrastructure

- **Judge model**: GPT-4o
- **RAGAS version**: 0.4.x
- **Chunking**: 1,200 tokens with 200-token overlap
- **Embedding model**: text-embedding-3-small
- **LLM for methods**: gpt-4o-mini

## Summary Results

| Benchmark | What It Measures | TinyCortex Headline |
| --- | --- | --- |
| [RAGAS](ragas.md) | Retrieval quality | **0.97** Answer Relevancy |
| [TemporalBench](temporalbench.md) | Temporal reasoning | **100%** recency accuracy |
| [BABILong](babilong.md) | Needle in a haystack | **Only method** to retrieve needles |
| [Vending-Bench](vending-bench.md) | Agentic decision-making | **~$295** cumulative P&L |
