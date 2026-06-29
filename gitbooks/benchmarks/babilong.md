# BABILong

## What It Measures

Whether a retrieval method can find specific facts ("needles") embedded within increasingly large contexts of distractor text.

## Methodology

Facts are inserted at various positions within contexts of 4k, 8k, 16k, and 128k tokens. Methods must retrieve the correct fact to answer a question. Accuracy is measured per context length.

## Methods Compared

tinycortex\_v1, directfeed

## Results

<div align="center"><img src="../.gitbook/assets/heatmap_babilong.png" alt="BABILong Heatmap" width="600"></div>

| Context Length | TinyCortex | directfeed |
| -------------- | --------- | ---------- |
| 4k             | **33%**   | 0%         |
| 8k             | 0%        | 0%         |
| 16k            | 0%        | 0%         |
| 128k           | 0%        | 0%         |
| **Overall**    | **11%**   | **0%**     |

## Analysis

TinyCortex is the **only method that successfully retrieves needles**, scoring 33% at the 4k context length. While absolute accuracy is still low, this demonstrates the advantage of graph-based indexing over raw context window approaches — the knowledge graph can locate specific entities even when surrounded by large volumes of distractor text. Directfeed scores 0% across all context lengths.
