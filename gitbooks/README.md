---
description: The memory layer for AI agents. Scales to 1B+ tokens.
---

# Introducing TinyCortex 🧠

Every AI memory system you've used does the same thing: store everything, retrieve by similarity, hope for the best.

The outcome: Your agent drowns in stale context. Responses degrade. Costs inflate. You end up writing custom pruning logic at 2am.

TinyCortex takes a fundamentally different approach. Inspired by how the human brain actually works, it **intelligently forgets noise** so your AI only reasons over what matters. Low-value memories decay naturally over time. Knowledge your users interact with gets reinforced and rises to the top. It doesn't require manual cleanup and there is no context window anxiety.

The result: an AI memory system that processes over **1 billion tokens**, stays lean and focused, and gets smarter with every interaction.

For a deployment serving 100,000 users with ongoing conversation history:

| Approach                                       | Cost per user per year |
| ---------------------------------------------- | ---------------------- |
| Frontier model context (everything in-context) | \~$90,000              |
| Standard RAG pipeline                          | \~$2.40                |
| **TinyCortex**                                  | **\~$0.72**            |

The 1,000:1 compression ratio means storage and compute costs grow slowly relative to conversation volume. TinyCortex indexes a conversation for $0.0004, compared to $0.0112 for Mem0 (28x lower).

## Core Features

### Intelligent Noise Filtering

Memories that aren't accessed naturally decay over time. Frequently recalled knowledge becomes more durable. The system stays lean on its own without manual cleanup and intervention.

<div align="center"><img src=".gitbook/assets/AppleEmailGraph.gif" alt="Memory Decay Simulation" width="700"></div>

### Interaction-Aware

Not all memories are equal. Views, reactions, replies, and content creation all signal what matters. Knowledge people engage with rises to the top; ignored information fades away.



<div align="center"><img src=".gitbook/assets/BobMemoryDecayVideo.gif" alt="Interaction Graph" width="700"></div>

### Low Latency, Low Cost, High Quality

No compromise on speed and quality when processing data with TinyCortex. Everything is processed at low cost and low latency while maintaining high benchmark scores

| Metric          | TinyCortex  | Nearest Competitor        |
| --------------- | ---------- | ------------------------- |
| Average latency | \~1.1s     | \~3.6s (Mem0)             |
| Query cost      | \~$0.00095 | \~$0.00085 (Mem0)         |
| Index cost      | \~$0.0005  | \~$0.014 (Mem0, 28x more) |

## Quick Start

```bash
pip install tinyhumansai
```

```python
import tinyhumansai as api

client = api.TinyHumanMemoryClient("YOUR_APIKEY_HERE")

# Store a memory
client.ingest_memory({
    "key": "user-preference-theme",
    "content": "User prefers dark mode",
    "namespace": "preferences",
    "metadata": {"source": "onboarding"},
})

# Ask a question using stored memory
response = client.recall_with_llm(
    prompt="What is the user's preference for theme?",
    api_key="OPENAI_API_KEY"
)
print(response.text)  # The user prefers dark mode
```

That's it. Ingest memories, recall them with any LLM. TinyCortex handles the hard parts: deduplication, decay, graph-based retrieval, and noise pruning.

{% hint style="info" %}
TinyCortex is currently in **closed alpha**. To get access, [reach out to us](mailto:founders@tinyhumans.ai).
{% endhint %}
