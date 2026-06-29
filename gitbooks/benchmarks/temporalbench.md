# TemporalBench

## What It Measures

Temporal reasoning accuracy measures if the memory system correctly answer questions about event ordering, state at a specific time, recency, intervals, and sequences

## Methodology

Questions are categorized into 5 temporal reasoning types. Each method ingests time-stamped events and is evaluated on accuracy per question type.

## Methods Compared

tinycortex\_v1, directfeed, e2graphrag, mem0, supermemory

## Results

<div align="center"><img src="../.gitbook/assets/chart_temporalbench.png" alt="TemporalBench Accuracy" width="700"></div>

| Question Type | TinyCortex | Best Competitor | Competitor |
| ------------- | --------- | --------------- | ---------- |
| Recency       | **100%**  | 80%             | directfeed |
| Interval      | 78%       | **97%**         | directfeed |
| Ordering      | **90%**   | 8&#x30;**%**    | directfeed |
| State at Time | 80%       | 80%             | e2graphrag |
| Sequence      | 80%       | 80%             | directfeed |

## Analysis

TinyCortex achieves **perfect accuracy on recency questions** (100%), directly demonstrating the effectiveness of its Ebbinghaus time-decay model. Recent memories naturally have higher retention scores. The directfeed method (feeding full context to the LLM) performs well on interval and sequence questions where having the complete timeline helps, but this approach doesn't scale beyond context window limits.
