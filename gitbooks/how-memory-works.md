# How Human Memory Works?

## Why AI Memory Should Work Like a Brain

Traditional AI memory systems store everything and retrieve by similarity. But the human brain works differently, it actively forgets. Psychologist Hermann Ebbinghaus showed that memories decay exponentially over time unless reinforced. We should treat it as a feature rather than a flaw. Forgetting keeps your mental model lean, relevant, and fast.

TinyCortex applies the same principles to AI memory.

## The Forgetting Curve → Time-Decay Model

Ebbinghaus's **Forgetting Curve** shows that memory retention drops rapidly after initial learning; Roughly 50% within an hour, 70% within 24 hours; unless the memory is revisited.

TinyCortex implements this as a **time-decay model**: at query time, every candidate memory gets a freshness score that decays exponentially with age (a 7-day half-life by default), computed from when the memory was last updated. Old memories naturally fade from recall, keeping results focused on what's current and relevant — without any destructive deletion, since the underlying markdown is never thrown away.

<figure><img src=".gitbook/assets/memory-decay@2x.png" alt=""><figcaption></figcaption></figure>

## Reinforcement Through Interaction → Interaction Weighting

In the brain, memories are strengthened through repetition and engagement. You remember things you think about, discuss, and act on.

TinyCortex mirrors this with **interaction weighting**. Not all signals are equal; content you authored, replies, direct messages, and mentions each carry a different weight in the admission score, so knowledge people engaged with is scored as more important the moment it is ingested, while low-engagement noise starts closer to the drop threshold and fades from recall as it ages.

## Compression, Not Accumulation → Noise Pruning

The brain doesn't store raw sensory data rather it compresses experiences into meaningful patterns and discards the rest. You remember the gist of a conversation, not every word.

TinyCortex applies **noise pruning** to achieve the same effect. A multi-signal admission gate drops low-value chunks at ingest, and hierarchical summary trees compress what remains into progressively smaller layers — so recall reads a focused digest, not the raw firehose. This compression-first design is what lets the hosted TinyCortex platform operate over very large corpora without recall quality drowning in noise.

## Graph-Based Knowledge → GraphRAG

The brain organizes knowledge as a web of associations of people, places, events, and concepts linked by relationships. It remembers one thing that triggers related memories.

TinyCortex uses a **graph-based retrieval** model. Documents are broken into chunks, and extractors (regex-based plus an optional LLM extractor) pull out **entities** — people, organizations, products, topics — which are indexed against the nodes that mention them. A **co-occurrence graph** links entities that appear together, and queries use this graph alongside keyword and vector signals to retrieve contextually rich, structured information rather than just similar text snippets.

<figure><img src=".gitbook/assets/graphrag-comparison@2x (1).png" alt=""><figcaption></figcaption></figure>

## Brain → TinyCortex

| Brain Concept                    | TinyCortex Feature                        |
| -------------------------------- | ---------------------------------------- |
| Ebbinghaus Forgetting Curve      | Exponential freshness decay at query time |
| Reinforcement through repetition | Interaction-weighted admission scoring   |
| Compression of experiences       | Noise pruning and hierarchical summary trees |
| Associative memory networks      | Entity index and co-occurrence graph     |
| Focused, lean recall             | Hybrid explainable ranking, no manual pruning |
