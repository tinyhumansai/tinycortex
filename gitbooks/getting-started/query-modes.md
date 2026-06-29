# Query Modes

TinyCortex provides several `QueryParam` presets that control the quality/cost/speed tradeoff for queries.

## Presets

| Preset | Entity Threshold | Relation Top-K | Chunk Top-K | Best For |
| --- | --- | --- | --- | --- |
| `balanced()` | 0.005 | 64 | 8 | General Q&A |
| `quality()` | 0.001 | 128 | 16 | High-stakes analysis |
| `economy()` | 0.01 | 32 | 4 | High-volume / batch queries |

```python
from tinycortex import QueryParam

# General-purpose — good quality at reasonable cost
response = rag.query("Who are the investors?", params=QueryParam.balanced())

# Maximum context and best answers
response = rag.query("Who are the investors?", params=QueryParam.quality())

# Lowest cost per query
response = rag.query("Who are the investors?", params=QueryParam.economy())
```

## References Mode

Setting `with_references=True` includes inline source citations `[1]`, `[2]`, etc. in the answer, referencing the retrieved chunks.

```python
response = rag.query(
    "What funding has the company received?",
    params=QueryParam(with_references=True),
)
print(response.response)  # Answer with [1], [2] citations

# Inspect the source chunks
for i, (chunk, score) in enumerate(response.context.chunks):
    print(f"[{i+1}] (score: {score:.4f}) {chunk.content[:150]}...")
```

## Context-Only Mode

Setting `only_context=True` skips LLM answer generation and returns just the scored context. Useful when you want to:

- Build your own answer generation pipeline
- Use the context with a different LLM
- Inspect retrieval quality without paying for answer generation

```python
response = rag.query(
    "Tell me about Dr. James Park",
    params=QueryParam(only_context=True),
)
# response.response is empty — no LLM answer generated
# Access scored entities, relations, and chunks directly:
print(response.context.entities)
print(response.context.relations)
print(response.context.chunks)
```

## Custom Tuning

For fine-grained control, override individual ranking parameters:

```python
# Wide retrieval — cast a broad net
wide = QueryParam(
    entity_ranking_threshold=0.001,
    relation_ranking_top_k=128,
    chunk_ranking_top_k=16,
    entities_max_tokens=6000,
    relations_max_tokens=5000,
    chunks_max_tokens=12000,
)

# Narrow retrieval — precision-focused
narrow = QueryParam(
    entity_ranking_threshold=0.05,
    relation_ranking_top_k=16,
    chunk_ranking_top_k=2,
    entities_max_tokens=2000,
    relations_max_tokens=1000,
    chunks_max_tokens=3000,
)

response = rag.query("What technology does the company use?", params=wide)
```

## Choosing a Preset

| Use Case | Recommended | Why |
| --- | --- | --- |
| General Q&A | `balanced()` | Good quality at reasonable cost |
| High-stakes analysis | `quality()` | Maximum context, best answers |
| High-volume / batch | `economy()` | Lowest cost per query |
| Source attribution | `QueryParam(with_references=True)` | Inline citations |
| Custom pipelines | `QueryParam(only_context=True)` | BYO answer generation |
| Domain-specific | Custom `QueryParam(...)` | Full control over retrieval |
