# Recall

Recall is how you fetch relevant memory context for a given task.

## How Recall Works

TinyCortex ranks candidate memories using:

- semantic relevance to the query
- recency/decay behavior
- interaction reinforcement signals

The output can include both structured chunks and an LLM-ready context string.

## Prompt-Driven Retrieval

Most applications recall with a query such as:

- "What dietary restrictions does the user have?"
- "Summarize prior project decisions"

This keeps retrieval dynamic and task-specific.

## Scope With Namespaces

Always scope recall to the namespace most relevant to the current task. This reduces noise and improves answer quality.

For implementation examples in all supported languages, see [Recalling Memories](../sdk-functions/recalling-memories.md).
