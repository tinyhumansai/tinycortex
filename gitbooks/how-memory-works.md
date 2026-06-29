# How Human Memory Works?

## Why AI Memory Should Work Like a Brain

Traditional AI memory systems store everything and retrieve by similarity. But the human brain works differently, it actively forgets. Psychologist Hermann Ebbinghaus showed that memories decay exponentially over time unless reinforced. We should treat it as a feature rather than a flaw. Forgetting keeps your mental model lean, relevant, and fast.

TinyCortex applies the same principles to AI memory.

## The Forgetting Curve → Time-Decay Model

Ebbinghaus's **Forgetting Curve** shows that memory retention drops rapidly after initial learning; Roughly 50% within an hour, 70% within 24 hours; unless the memory is revisited.

TinyCortex implements this as a **time-decay model**: every piece of stored knowledge has a retention score that decreases over time. Old, unaccessed memories naturally fade, keeping the system focused on what's current and relevant.

<figure><img src=".gitbook/assets/memory-decay@2x.png" alt=""><figcaption></figcaption></figure>

## Reinforcement Through Interaction → Interaction Weighting

In the brain, memories are strengthened through repetition and engagement. You remember things you think about, discuss, and act on.

TinyCortex mirrors this with **interaction weighting**. Not all signals are equal; views, reactions, replies, and content creation each carry different weight. Knowledge that people engage with rises in importance; ignored information fades away. The more a piece of knowledge is accessed or referenced, the more durable it becomes.

## Compression, Not Accumulation → Noise Pruning

The brain doesn't store raw sensory data rather it compresses experiences into meaningful patterns and discards the rest. You remember the gist of a conversation, not every word.

TinyCortex applies **noise pruning** to achieve the same effect. Rather than accumulating every token forever, it lets low-value memories decay and removes noise from the knowledge base. This is what allows TinyCortex to handle over 1 billion tokens without degrading in quality.

## Graph-Based Knowledge → GraphRAG

The brain organizes knowledge as a web of associations of people, places, events, and concepts linked by relationships. It remembers one thing that triggers related memories.

TinyCortex uses a **Graph-based Retrieval-Augmented Generation (GraphRAG)** model. Documents are broken into chunks, and an LLM extracts **entities** (people, companies, products) and **relations** (founded, works at, invested in) to build a knowledge graph. Queries traverse this graph to retrieve contextually rich, structured information and not just similar text snippets.

<figure><img src=".gitbook/assets/graphrag-comparison@2x (1).png" alt=""><figcaption></figcaption></figure>

## Brain → TinyCortex

| Brain Concept                    | TinyCortex Feature                        |
| -------------------------------- | ---------------------------------------- |
| Ebbinghaus Forgetting Curve      | Time-decay retention scores              |
| Reinforcement through repetition | Interaction-weighted importance          |
| Compression of experiences       | Noise pruning and memory decay           |
| Associative memory networks      | GraphRAG entity-relation model           |
| Focused, lean recall             | Automatic cleanup without manual pruning |
