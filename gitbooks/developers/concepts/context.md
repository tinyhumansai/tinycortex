# Context

**Context** is a pre-formatted string built from recalled memories. It's designed to be injected directly into an LLM prompt as system context.

## Format

```
[preferences:fact-1]
User is vegetarian

[preferences:fact-2]
Allergic to peanuts
```

Each memory is labeled with its `[namespace:key]` so the LLM understands where the information came from.

## Usage

When you call `recall_memory`, you get back a `context` string along with the individual items:

```python
ctx = client.recall_memory(
    namespace="preferences",
    prompt="What does the user eat?",
    num_chunks=10,
)

# Use the formatted context in your own LLM call
messages = [
    {"role": "system", "content": f"Use this context:\n{ctx.context}"},
    {"role": "user", "content": "What should I order for lunch?"},
]
```

## Or Let TinyCortex Handle It

If you don't want to manage the LLM call yourself, use `recall_with_llm`  it fetches context and queries the LLM in one step:

```python
response = client.recall_with_llm(
    prompt="What should I order for lunch?",
    namespace="preferences",
    api_key="your-openai-key",
)
print(response.text)
```
