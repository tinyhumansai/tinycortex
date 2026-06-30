# Benchmarks

This page summarizes the benchmark suite and headline results for **TinyCortex Mark 1** (`mk1` / `tinycortex_v1`). Each benchmark compares TinyCortex against other memory and RAG methods across four dimensions: retrieval quality, temporal reasoning, needle-in-a-haystack recall, and real-world agentic decision-making.

> **Scope note.** These numbers reflect evaluations of the broader TinyCortex system (the `tinycortex_v1` configuration, GraphRAG with time-decay and interaction weighting), not a micro-benchmark of any single Rust function in this crate. They are reproduced here as reported in `gitbooks/benchmarks/`. Treat them as system-level results that the open-source Rust core contributes to, alongside the hosted evaluation harness. All figures are kept as reported.

## Methods Compared

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

## Evaluation Infrastructure

- **Judge model**: GPT-4o
- **RAGAS version**: 0.4.x
- **Chunking**: 1,200 tokens with 200-token overlap
- **Embedding model**: text-embedding-3-small
- **LLM for methods**: gpt-4o-mini

## Summary Results

| Benchmark | What It Measures | TinyCortex Headline |
| --- | --- | --- |
| RAGAS | Retrieval quality | **0.97** Answer Relevancy |
| TemporalBench | Temporal reasoning | **100%** recency accuracy |
| BABILong | Needle in a haystack | **Only method** to retrieve needles |
| Vending-Bench | Agentic decision-making | **~$295** cumulative P&L |

---

## RAGAS — Retrieval Quality

Standard retrieval-augmented generation quality: answer correctness, faithfulness, answer relevancy, context precision, and context recall.

**Methodology.** 50 questions generated from the complete Sherlock Holmes corpus across 4 question types (inference, multi-hop, cross-story, analytical), evaluated with [RAGAS 0.4.x](https://docs.ragas.io/) using GPT-4o as judge. Each method ingests the same chunked corpus, then answers all questions; RAGAS scores are computed per-question and aggregated. Methods compared: `tinycortex_v1`, `fastgraphrag`, `gemini_vdb`, `mem0`, `supermemory`.

![RAGAS Benchmark Scores](https://raw.githubusercontent.com/tinyhumansai/tinycortex/main/docs/images/chart_ragas.png)

| Metric | TinyCortex | Best Competitor | Competitor |
| --- | --- | --- | --- |
| Answer Relevancy | **0.97** | 0.88 | supermemory |
| Context Precision | **0.80** | 0.78 | supermemory |
| Faithfulness | **0.97** | 0.79 | gemini_vdb |
| Answer Correctness | **0.78** | 0.59 | gemini_vdb |
| Context Recall | **0.78** | 0.70 | gemini_vdb |

TinyCortex achieves the highest **Answer Relevancy** by a wide margin (0.97 vs 0.88) and stays competitive on **Context Precision**. Graph-based retrieval keeps returned context highly relevant to the query, even when answers require cross-story reasoning.

---

## TemporalBench — Temporal Reasoning

Whether the memory system can correctly answer questions about event ordering, state at a specific time, recency, intervals, and sequences.

**Methodology.** Questions are categorized into 5 temporal reasoning types. Each method ingests time-stamped events and is evaluated on accuracy per question type. Methods compared: `tinycortex_v1`, `directfeed`, `e2graphrag`, `mem0`, `supermemory`.

![TemporalBench Accuracy](https://raw.githubusercontent.com/tinyhumansai/tinycortex/main/docs/images/chart_temporalbench.png)

| Question Type | TinyCortex | Best Competitor | Competitor |
| --- | --- | --- | --- |
| Recency | **100%** | 80% | directfeed |
| Interval | 78% | **97%** | directfeed |
| Ordering | **90%** | 80% | directfeed |
| State at Time | 80% | 80% | e2graphrag |
| Sequence | 80% | 80% | directfeed |

TinyCortex reaches **perfect accuracy on recency questions** (100%), demonstrating the effect of its Ebbinghaus time-decay model — recent memories naturally carry higher retention scores. `directfeed` (full context into the LLM) does well on interval and sequence questions where a complete timeline helps, but that approach does not scale beyond context-window limits.

---

## BABILong — Needle in a Haystack

Whether a retrieval method can find specific facts ("needles") embedded within increasingly large contexts of distractor text.

**Methodology.** Facts are inserted at various positions within contexts of 4k, 8k, 16k, and 128k tokens. Methods must retrieve the correct fact to answer a question; accuracy is measured per context length. Methods compared: `tinycortex_v1`, `directfeed`.

![BABILong Heatmap](https://raw.githubusercontent.com/tinyhumansai/tinycortex/main/docs/images/heatmap_babilong.png)

| Context Length | TinyCortex | directfeed |
| --- | --- | --- |
| 4k | **33%** | 0% |
| 8k | 0% | 0% |
| 16k | 0% | 0% |
| 128k | 0% | 0% |
| **Overall** | **11%** | **0%** |

TinyCortex is the **only method that successfully retrieves needles**, scoring 33% at the 4k context length. Absolute accuracy is still low, but it shows the advantage of graph-based indexing over raw context-window approaches: the knowledge graph can locate specific entities even when surrounded by large volumes of distractor text. `directfeed` scores 0% across all context lengths.

---

## Vending-Bench — Agentic Decision-Making

How well a memory-augmented agent makes business decisions over time. An agent manages a simulated vending-machine operation over 30 days, deciding what products to stock, where to place machines, and how to price items.

**Methodology.** Each method provides the agent's memory layer. The agent receives daily sales data and must make restocking and pricing decisions; performance is cumulative Profit & Loss (P&L) over 30 simulated days. Methods compared: `tinycortex_v1`, `mem0`, `scratchpad`, `supermemory`.

![Vending-Bench P&L](https://raw.githubusercontent.com/tinyhumansai/tinycortex/main/docs/images/chart_vendingbench.png)

| Method | Final P&L (Day 30) |
| --- | --- |
| **tinycortex_v1** | **~$295** |
| scratchpad | ~$285 |
| supermemory | ~$215 |
| mem0 | ~$5 |

TinyCortex achieves the **highest cumulative P&L by day 30** (~$295). Interaction-weighted memory ensures the agent prioritizes learning from high-signal events (successful sales, pricing changes) while forgetting noise (random daily fluctuations). Mem0 barely breaks even, suggesting that without structured memory the agent cannot learn effectively from past decisions.

---

## Running the Suite Yourself

The benchmark suite lives in the TinyCortex repository and is driven by a Python harness (separate from the Rust crate). You can run all benchmarks or target specific methods.

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

Benchmarks are configured via `config.json` in the repository root:

| Parameter | Default | Description |
| --- | --- | --- |
| `corpus` | `sherlock_holmes` | Corpus used for evaluation |
| `methods` | all | Methods to benchmark |
| `max_questions` | `0` (unlimited) | Limit questions per benchmark |
| `top_k` | `8` | Number of chunks to retrieve |
| `chunk_size` | `1200` | Token size per chunk |
| `chunk_overlap` | `200` | Overlap between chunks |
| `openai_model` | `gpt-4o-mini` | LLM used by methods |
| `embedding_model` | `text-embedding-3-small` | Embedding model |
| `ragas_judge_model` | `gpt-4o` | Judge model for RAGAS evaluation |

The benchmark harness calls into the same retrieval and scoring behavior implemented by the Rust core — see [Retrieval](Retrieval) and [Scoring-and-Extraction](Scoring-and-Extraction) for how time-decay, interaction weighting, and graph traversal feed these results.

## See also

- [Retrieval](Retrieval)
- [Scoring-and-Extraction](Scoring-and-Extraction)
- [Architecture-Overview](Architecture-Overview)
- [FAQ](FAQ)
