<div align="center">
<img src="./docs/images/graph.png" style="max-width: 400px"/>
<h1>TinyCortex AI Memory 🧠 - Your Second Brain</h1>

**Human-like AI Memory  ◦  10Mn+ Token Context ◦ 0.2$/Mn tokens** ◦ **Conscious Recall**

[Discord](https://discord.com/invite/k23Kn8nK) • [Reddit](https://www.reddit.com/r/tinyhumansai/) • [X](https://x.com/tinyhumansai) • [Docs](https://tinyhumans.gitbook.io/tinycortex/)

#### [Benchmarks](./benchmarks/README.md)  •   [Getting Started](#-getting-started)  •   [Documentation](https://tinyhumans.gitbook.io/tinycortex/)  •   [Get your API key](https://tinyhumans.ai) 

_NOTE: That this model is currently in closed alpha. To get access [reach out to us](mailto:founders@tinyhumans.ai)_

Read the paper - ([Markdown](./paper/README.md) / [PDF](./paper/out/main.pdf))

</div>

The human brain is a master at compression. It doesn't try to remember every passing detail; instead, it aggressively prunes noise to maintain a sharp, focused, and easily accessible recall of what truly matters. In contrast, traditional AI memory systems try to remember everything. They retrieve whatever is _similar_—but similar doesn't mean important. The result? Your AI drowns in stale, irrelevant context that degrades every response.

Inspired by how the human brain works, **TinyCortex** takes a similar approach to AI memory: it **intelligently forgets noise**. Just like how you don't remember every sentence you've ever read or everything happens every day in your life, TinyCortex lets low-value memories naturally decay while reinforcing the knowledge that matters — the things you interact with, recall, and build upon.

The result? an AI memory system that can chop through over 10 million tokens accurately at speeds of upto 4000 tokens/second, stays lean and focused, and gets smarter with every interaction.

TinyCortex ranks extremely high scores on [RAGAS](https://www.ragas.io/), [Babilong](https://github.com/booydar/babilong/), [Vending Bench](https://andonlabs.com/evals/vending-bench-2), [LoCoMo](https://github.com/snap-research/locomo) and [HotPotQA](https://hotpotqa.github.io/)

# 🎯 Core Features

## Intelligent Noise Filters

Memories that aren't accessed naturally decay over time. Frequently recalled knowledge becomes more durable. No manual cleanup needed — the system stays lean on its own.

![Interaction graph highlighting important knowledge](.github/images/gif/AppleEmailGraph.gif)

## Interaction-Aware

Not all memories are equal. Views, reactions, replies, and content creation all signal what matters. Knowledge people engage with rises to the top; ignored information fades away.

![Memory decay over time](.github/images/gif/BobMemoryDecayVideo.gif)

## Low Latency, Low Cost, High Quality

There's no compromise on speed and quality when processing data with TinyCortex. Everything is processed at low costs and low latency, while maintain high benchmarks.

## Conscious Recall

Conscious recall is a TinyCortex feature that proactively surfaces the most relevant memories for a given moment, instead of waiting for an explicit query.

It continuously tracks what a user has done recently which includes conversations, actions, and signals; and combines that with time-based decay to decide which memories should stay “top of mind.”

When your agent needs context, conscious recall pulls forward the memories that are both recent and repeatedly interacted with, giving the LLM a focused slice of long-term history rather than a noisy dump of everything you’ve ever stored.

# ⚡ Getting Started

TinyCortex ships with SDKs for [Python](./packages/sdk-python), [TypeScript/JavaScript](./packages/sdk-typescript), [Go](./packages/sdk-golang), [Rust](./packages/sdk-rust), [Dart](./packages/sdk-dart), [C++](./packages/sdk-cpp), [C#](./packages/sdk-csharp), and [Java](./packages/sdk-java), plus plugins for [LangGraph](./packages/plugin-langgraph), [OpenClaw](./packages/plugin-openclaw), [ElevenLabs](./packages/plugin-elevenlabs), [CrewAI](./packages/plugin-crewai), [Raycast](./packages/plugin-raycast), [Agno](./packages/plugin-agno) [Pipecat](./packages/plugin-pipecat), [Mastra](./packages/plugin-mastra), [Autogen](./packages/plugin-autogen) and more.

See [packages/README.md](./packages/README.md) for details about all the SDKs/Plugins available to use along with documentation and examples.

Below is a simple quickstart example on getting started with Python.

```python
# pip install tinyhumansai

import tinyhumansai as api

client = api.TinyHumanMemoryClient("YOUR_APIKEY_HERE")

# Store a single memory
client.ingest_memory({
    "key": "user-preference-theme",
    "content": "User prefers dark mode",
    "namespace": "preferences",
    "metadata": {"source": "onboarding"},
})

# Ask a LLM something from the memory
response = client.recall_with_llm(
    prompt="What is the user's preference for theme?",
    api_key="OPENAI_API_KEY"
)
print(response.text) # The user prefers dark mode
```

# Demo Products

Explore TinyCortex in action through a set of real-time demo experiences that show how the memory layer behaves under live usage.

<!-- ![Demo products screenshot](./.github/images/demo.png) -->

- **Real-time chat assistant** – A conversational UI that continuously writes and recalls memories so the assistant remembers users across sessions.
- **Live activity memory feed** – A stream of events (page views, actions, and signals) flowing into TinyCortex, letting you inspect how memories are created, updated, and decayed over time.
- **Agentic decision demo** – A simple agent that uses TinyCortex to make stateful decisions over many steps, highlighting how long-horizon context is preserved.

# Usage with LLMs

Provide context to your LLM by using a dedicated **context** role instead of stuffing facts into the system message. Context ingested this way doesn’t consume expensive LLM tokens, and more context doesn’t hurt accuracy.

![Context and LLM: before vs after](.github/images/context-llm.png)

# 📈 Benchmarks

### RAGAS — Retrieval Quality (Sherlock Holmes Corpus)

Standard RAG quality metrics evaluated using [RAGAS](https://docs.ragas.io/). TinyCortex leads in **Answer Relevancy (0.97)** and **Context Precision (0.75)**, outperforming FastGraphRAG, Gemini VDB, Mem0, and SuperMemory.

![ragas](.github/images/chart_ragas.png)

### TemporalBench — Temporal Reasoning

Accuracy across ordering, state-at-time, recency, interval, and sequence questions. TinyCortex achieves **100% on recency questions** — correctly surfacing the most recent events thanks to its time-decay memory model.

![chart_temporalbench](.github/images/chart_temporalbench.png)

### Vending-Bench — Agentic Decision-Making

An agent manages a simulated vending machine business over 30 days. TinyCortex achieves the **highest cumulative P&L (~$295 by day 30)** — better memory leads to better decisions over time.

![chart_vendingbench](.github/images/chart_vendingbench.png)

---

# Star us on Github

_Like contributing towards AGI 🧠? Give this repo a star and spread the love ❤️_

<p align="center">
  <a href="https://www.star-history.com/#tinyhumansai/tinycortex&type=date&legend=top-left">
    <picture>
     <source media="(prefers-color-scheme: dark)" srcset="https://api.star-history.com/svg?repos=tinyhumansai/tinycortex&type=date&theme=dark&legend=top-left" />
     <source media="(prefers-color-scheme: light)" srcset="https://api.star-history.com/svg?repos=tinyhumansai/tinycortex&type=date&legend=top-left" />
     <img alt="Star History Chart" src="https://api.star-history.com/svg?repos=tinyhumansai/tinycortex&type=date&legend=top-left" />
    </picture>
  </a>
</p>

# Contributors Hall of Fame

Show some love and end up in the hall of fame

<a href="https://github.com/tinyhumansai/tinycortex/graphs/contributors">
  <img src="https://contrib.rocks/image?repo=tinyhumansai/tinycortex" alt="TinyCortex contributors" />
</a>
