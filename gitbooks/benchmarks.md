---
description: Headline benchmark results for TinyCortex Mark 1 (mk1) across retrieval quality, temporal reasoning, needle-in-a-haystack recall, and agentic decision-making.
---

# Benchmarks

Benchmark results for TinyCortex Mark 1 (`mk1` / `tinycortex_v1`). All benchmarks compare TinyCortex against other memory and RAG methods including vector databases, FastGraphRAG, Mem0, SuperMemory, and directfeed (raw context window).

{% hint style="info" %}
**Scope note.** These numbers reflect evaluations of the broader TinyCortex system (the `tinycortex_v1` configuration — GraphRAG with time-decay and interaction weighting), not a micro-benchmark of any single Rust function in this crate. Treat them as system-level results that the open-source Rust core contributes to, alongside the hosted evaluation harness. All figures are kept as reported.
{% endhint %}

## Methods compared

| Method | Type |
| --- | --- |
| **tinycortex_v1** | GraphRAG with time-decay and interaction weighting |
| fastgraphrag | Graph-based RAG |
| e2graphrag | Graph-based RAG |
| gemini_vdb | Vector database (Gemini embeddings) |
| mem0 | Memory layer |
| supermemory | Memory layer |
| scratchpad | Simple key-value store |
| directfeed | Raw context window (no retrieval) |

## Summary results

| Benchmark | What It Measures | TinyCortex Headline |
| --- | --- | --- |
| RAGAS | Retrieval quality | **0.97** Answer Relevancy |
| TemporalBench | Temporal reasoning | **100%** recency accuracy |
| BABILong | Needle in a haystack | **Only method** to retrieve needles |
| Vending-Bench | Agentic decision-making | **~$295** cumulative P&L |

---

## RAGAS (Sherlock Holmes Corpus)

**What it measures:** Standard retrieval-augmented generation quality — answer correctness, faithfulness, answer relevancy, context precision, and context recall.

**Methodology:** 50 questions generated from the complete Sherlock Holmes corpus (4 types: inference, multi-hop, cross-story, analytical). Evaluated using [RAGAS 0.4.x](https://docs.ragas.io/) with GPT-4o as the judge model. Each method ingests the same chunked corpus, then answers all questions. RAGAS scores are computed per-question and aggregated.

**Methods compared:** tinycortex_v1, fastgraphrag, gemini_vdb, mem0, supermemory

<div align="center">
<img src=".gitbook/assets/chart_ragas.png" alt="RAGAS Benchmark Scores" width="700"/>
</div>

**Key results:**

| Metric | TinyCortex | Best Competitor | Competitor |
| ------ | --------- | --------------- | ---------- |
| Answer Relevancy | **0.97** | 0.88 | supermemory |
| Context Precision | **0.75** | 0.76 | supermemory |
| Faithfulness | 0.73 | **0.79** | gemini_vdb |
| Answer Correctness | 0.57 | **0.59** | gemini_vdb |
| Context Recall | 0.62 | **0.70** | gemini_vdb |

TinyCortex achieves the highest Answer Relevancy score by a significant margin (0.97 vs 0.88) and is competitive on Context Precision. The graph-based retrieval ensures that returned context is highly relevant to the query, even when the answer requires cross-story reasoning.

---

## TemporalBench

**What it measures:** Temporal reasoning accuracy — can the memory system correctly answer questions about event ordering, state at a specific time, recency, intervals, and sequences?

**Methodology:** Questions are categorized into 5 temporal reasoning types. Each method ingests time-stamped events and is evaluated on accuracy per question type.

**Methods compared:** tinycortex_v1, directfeed, e2graphrag, mem0, supermemory

<div align="center">
<img src=".gitbook/assets/chart_temporalbench.png" alt="TemporalBench Accuracy" width="700"/>
</div>

**Key results:**

| Question Type | TinyCortex | Best Competitor | Competitor |
| ------------- | --------- | --------------- | ---------- |
| Recency | **100%** | 80% | directfeed |
| Interval | 68% | **97%** | directfeed |
| Ordering | 60% | **80%** | directfeed |
| State at Time | 60% | **80%** | e2graphrag |
| Sequence | 30% | **80%** | directfeed |

TinyCortex achieves **perfect accuracy on recency questions** (100%), directly demonstrating the effectiveness of its Ebbinghaus time-decay model — recent memories naturally have higher retention scores. The directfeed method (feeding full context to the LLM) performs well on interval and sequence questions where having the complete timeline helps, but this approach doesn't scale beyond context window limits.

---

## BABILong (Needle in a Haystack)

**What it measures:** Whether a retrieval method can find specific facts ("needles") embedded within increasingly large contexts of distractor text.

**Methodology:** Facts are inserted at various positions within contexts of 4k, 8k, 16k, and 128k tokens. Methods must retrieve the correct fact to answer a question. Accuracy is measured per context length.

**Methods compared:** tinycortex_v1, directfeed

<div align="center">
<img src=".gitbook/assets/heatmap_babilong.png" alt="BABILong Heatmap" width="600"/>
</div>

**Key results:**

| Context Length | TinyCortex | directfeed |
| -------------- | --------- | ---------- |
| 4k | **33%** | 0% |
| 8k | 0% | 0% |
| 16k | 0% | 0% |
| 128k | 0% | 0% |
| **Overall** | **11%** | **0%** |

TinyCortex is the **only method that successfully retrieves needles**, scoring 33% at the 4k context length. While absolute accuracy is still low, this demonstrates the advantage of graph-based indexing over raw context window approaches — the knowledge graph can locate specific entities even when surrounded by large volumes of distractor text. Directfeed scores 0% across all context lengths.

---

## Vending-Bench (Agentic Decision-Making)

**What it measures:** How well a memory-augmented agent makes business decisions over time. An agent manages a simulated vending machine operation over 30 days, deciding what products to stock, where to place machines, and how to price items.

**Methodology:** Each method provides the agent's memory layer. The agent receives daily sales data and must make restocking and pricing decisions. Performance is measured by cumulative Profit & Loss (P&L) over 30 simulated days.

**Methods compared:** tinycortex_v1, mem0, scratchpad, supermemory

<div align="center">
<img src=".gitbook/assets/chart_vendingbench.png" alt="Vending-Bench P&L" width="700"/>
</div>

**Key results:**

| Method | Final P&L (Day 30) |
| ------ | ------------------- |
| **tinycortex_v1** | **~$295** |
| scratchpad | ~$285 |
| supermemory | ~$215 |
| mem0 | ~$5 |

TinyCortex achieves the **highest cumulative P&L by day 30** (~$295). The interaction-weighted memory ensures the agent prioritizes learning from high-signal events (successful sales, pricing changes) while forgetting noise (random daily fluctuations). Mem0 barely breaks even, suggesting that without structured memory, the agent cannot learn from past decisions effectively.

---

## Run your own

The benchmark suite lives in the [`benchmarks/`](https://github.com/tinyhumansai/tinycortex/tree/main/benchmarks) directory of the repository and is driven by a Python harness (separate from the Rust crate). You can run all benchmarks or target specific methods:

```bash
# Setup
pip install -r requirements.txt
bash scripts/download_corpus.sh

# Run all benchmarks
python run.py

# Run specific methods
python run.py --methods tinycortex,vdb --max-questions 10

# View results
python scripts/chart.py --chart bar
```

The harness depends on `requirements.txt` in the benchmarks directory and a downloadable corpus. It calls into the same retrieval and scoring behavior implemented by the Rust core — see [Retrieval](retrieval.md) and [Scoring and Extraction](scoring-and-extraction.md) for how time-decay, interaction weighting, and graph traversal feed these results.

## See also

- [Retrieval](retrieval.md)
- [Scoring and Extraction](scoring-and-extraction.md)
- [Architecture Overview](architecture.md)
- [FAQ](faq.md)
