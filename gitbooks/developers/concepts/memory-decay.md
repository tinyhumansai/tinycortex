# Memory Decay

TinyCortex implements a **time-decay model** inspired by the Ebbinghaus Forgetting Curve. Every memory has a retention score that decreases over time unless reinforced.

## How It Works

* **New memories** start with high retention.
* **Unaccessed memories** gradually decay their importance decreases over time.
* **Recalled or interacted-with memories** are reinforced their retention resets and strengthens.
* **Decayed memories** are effectively pruned, keeping the memory system lean.

<div align="center"><img src="../../.gitbook/assets/AppleEmailGraph.gif" alt="Memory decay simulation" width="700"></div>

## Why Decay?

This is what allows TinyCortex to process over **1 billion tokens** without drowning in stale, irrelevant context. You never need to manually clean up old memories the system handles it.

Traditional memory systems accumulate everything forever. The more data they store, the harder it becomes to find what's relevant. TinyCortex flips this, the system naturally stays lean and focused, just like the human brain.

<figure><img src="/broken/files/KDosK3U7OsuEW0GYrwzu" alt=""><figcaption></figcaption></figure>

## Decay + Interactions

Decay and [interactions](interactions.md) work together:

* A memory that's frequently recalled resists decay it stays strong.
* A memory that was ingested once and never touched again fades over time.
* The combination means your most important, most-used knowledge is always front and center.
