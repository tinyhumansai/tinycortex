# TinyCortex (Draft)

Building Artificial Consciousness with a High-Throughput Context System

### Authors

- <a href="mailto:enamakel@tinyhumans.ai">Steven Enamakel (Tiny Humans)</a>

### Table of contents

- [Abstract](#abstract)
- [Introduction](#introduction)
- [Related Work](#related-work)
  - [Titans and MIRAS: Long-Term Memory for Language Models](#titans-and-miras-long-term-memory-for-language-models)
  - [GraphRAG and Graph-Based Memory and Retrieval](#graphrag-and-graph-based-memory-and-retrieval)
  - [MemoryBank: Retention-Aware Memory for Language Models](#memorybank-retention-aware-memory-for-language-models)
- [Understanding the Human Brain](#understanding-the-human-brain)
  - [Purkinje Cells](#purkinje-cells)
  - [Ebbinghaus Forgetting Curve](#ebbinghaus-forgetting-curve)
  - [Conscious and Subconscious Processing](#conscious-and-subconscious-processing)
- [Consciousness Loop](#consciousness-loop)
  - [Phase 1: Large-Scale Contextual Ingestion](#phase-1-large-scale-contextual-ingestion)
  - [Phase 2: Interval-Based Recall and Thought Synthesis](#phase-2-interval-based-recall-and-thought-synthesis)
  - [Phase 3: Action Decision](#phase-3-action-decision)
  - [Phase 4: Memory Update and Thought Persistence](#phase-4-memory-update-and-thought-persistence)
  - [Why This Loop Matters](#why-this-loop-matters)
- [Implementation & Results](#implementation-results)
  - [Current Evaluation Status](#current-evaluation-status)
  - [Observed Behavior](#observed-behavior)
  - [Limitations and Next Measurement Steps](#limitations-and-next-measurement-steps)
- [Conclusion](#conclusion)

# Abstract

Large language models are good at short tasks, but they still struggle with long-term coherence and adaptation. Most systems retrieve information, answer, and move on.

present **TinyCortex**, a simple ongoing intelligence loop built on top of memory. The loop continuously processes experience, learns from feedback, forgets low-value noise, and updates a persistent internal state over time. has two components: **(1)** a memory layer and **(2)** a consciousness loop. Together they provide persistent context and continuous adaptation.

**Current result.** In internal usage, this approach improves continuity across sessions and helps the system adapt faster to a user’s evolving context. Our claim is practical: stronger feedback, learning, and forgetting dynamics can move AI systems closer to consciousness-like behavior and, over time, to AGI-level adaptability.

# Introduction

Modern LLMs are strong at local reasoning but weak at long-horizon coherence. Three constraints repeatedly appear in production systems: **(i)** finite context windows, **(ii)** retrieval pollution in RAG-style pipelines (irrelevant or stale evidence), and **(iii)** lack of instinct-like consciousness, which keeps behavior narrow and reactive. Increasing context length helps only partially, because cost scales with sequence length and quality often drops when the active prompt carries too much low-signal text.

This shifts the design focus from “more context” to “better memory control.” Practical systems must decide which evidence to admit, which evidence to decay, and which state transitions to preserve as durable history.

TinyCortex is explicitly two things: a memory layer and a consciousness loop. The memory layer provides durable context; the consciousness loop provides continuous adaptation through feedback, learning, and forgetting. Together, they transform stored context into evolving intelligence.

Human memory provides a useful engineering analogy. Biological systems handle continuous noisy input through selective gating, reinforcement by use, and time-based forgetting. Most current LLM memory stacks implement storage and retrieval well, but still under-specify these three control functions: **what to keep**, **what to strengthen**, and **what to update over time**.

We introduce **TinyCortex**, an architecture for conscious intelligence designed around those control functions. It combines:

- Semantic and graph retrieval for topical and relational recall;

- An ordered state-event ledger for deterministic temporal/state queries;

- Retention-aware reweighting and decay to suppress memory pollution;

- Interval-based thought synthesis to maintain latent internal state between explicit prompts.

Figure <a href="#fig:consciousness-architecture" data-reference-type="ref" data-reference="fig:consciousness-architecture">5</a> summarizes the architecture. Our central claim is operational: consciousness-like continuity requires both an adaptive memory substrate and a recurrent intelligence loop. The memory layer and the consciousness loop work together to produce adaptation over time.

# Related Work

## Titans and MIRAS: Long-Term Memory for Language Models

Google’s Titans line reframes sequence modeling by separating fast in-context processing from a slower persistent memory path (Behrouz, Zhong, et al. 2025). Instead of relying only on larger context windows, Titans introduces a learned long-term mechanism that complements attention on long horizons.

The MIRAS perspective extends this idea by treating sequence models as associative memory systems with explicit retention and biasing dynamics (Behrouz, Razaviyayn, et al. 2025). Retrieval and retention are first-class inference-time mechanisms rather than side effects of attention alone.

These directions are promising but remain mostly research-stage.

## GraphRAG and Graph-Based Memory and Retrieval

GraphRAG demonstrates that graph structure can improve retrieval beyond flat vector similarity by enabling entity-centric and global reasoning over corpora (Edge et al. 2024; Microsoft Research 2025). A recurring tradeoff is ingestion overhead: high-quality graph extraction and linking can be expensive and slow at large scale.

## MemoryBank: Retention-Aware Memory for Language Models

MemoryBank (Zhong et al. 2023) is a direct precursor to retention-aware LLM memory. It models memory strength over time and updates retention from interaction signals, rather than treating memory as static retrieved text.

# Understanding the Human Brain

Before implementation details, we ground the design in biological principles. The goal is not neural equivalence, but transferable mechanisms for memory selection, retention, and consolidation.

## Purkinje Cells

*The “we problem”* in artificial consciousness can be framed as coordination: how distributed signals become a coherent self-model that acts as a unified agent. In biology, this coherence emerges from layered circuits, not a single unit.

Purkinje cells offer a useful analogy for combining *background activity* with *coordinated control*. They integrate high-dimensional parallel input while maintaining ongoing, partly stochastic firing. This endogenous activity supports fine timing and internal adjustment, not only stimulus-response reactivity.

For memory systems that treat ambient *noise* and *conscious thoughts* as first-class signals (Figure <a href="#fig:consciousness-architecture" data-reference-type="ref" data-reference="fig:consciousness-architecture">5</a>), the key lesson is permission for controlled spontaneous initiation. Internally generated candidates can surface and then be amplified or suppressed through feedback. Three principles carry over: **(i)** high-dimensional integration, **(ii)** time-varying endogenous candidate generation, and **(iii)** selective inhibitory gating calibrated by interaction.

<figure id="fig:purkinje-cell-diagram" data-latex-placement="H">
<img src="figures/purkinje-cell.png" style="width:75.0%" />
<figcaption>Purkinje cells as biological reference: parallel integration, ongoing spontaneous firing, and inhibitory gating of downstream pathways.</figcaption>
</figure>

This motivates a controller that does more than nearest-neighbor retrieval. It must combine semantic, episodic, and temporal evidence, then gate what enters active context, including internally generated candidates. The target is not maximal storage, but reliable influence control at decision time.

## Ebbinghaus Forgetting Curve

The Ebbinghaus forgetting curve captures a practical principle: memory retention drops without reinforcement (<span class="nocase">Lewandowsky et al.</span> 2015). For LLM memory systems, this implies that useful information should gain weight through reuse, while low-value details should decay automatically.

<figure id="fig:forgetting-curve" data-latex-placement="H">
<img src="figures/forgetting-curve.png" style="width:75.0%" />
<figcaption>Ebbinghaus-inspired forgetting dynamics: retention drops rapidly without reinforcement, motivating selective memory maintenance.</figcaption>
</figure>

This yields a concrete strategy: encode candidate memories at write time, then update retention from access frequency, recency, and utility. Periodic pruning and reweighting reduce stale noise while preserving salient patterns.

## Conscious and Subconscious Processing

Another useful distinction is conscious versus subconscious processing. The conscious layer is task-facing and prompt-responsive. The subconscious layer is continuous and background: it consolidates experience, updates associations, and reweights salience between explicit tasks. This supports a key design choice in TinyCortex: recall quality depends on both query-time retrieval and ongoing maintenance cycles.

# Consciousness Loop

We now describe the core operational mechanism: a recurring four-phase loop that updates memory and policy state continuously.

In production, most incoming data is initially **noise**: high-volume, redundant, and weakly structured. Phase 1 filters and normalizes this stream before storage. The loop then executes: **(1)** ingestion, **(2)** interval recall and thought synthesis, **(3)** action decision, and **(4)** memory reweighting plus thought persistence. Over time, this converts raw interaction history into a compact evolving internal model.

<figure id="fig:consciousness-loop" data-latex-placement="H">
<img src="figures/consciousness-loop.png" style="width:98.0%" />
<figcaption>Control loop: raw <em>noise</em> feeds ingest, which filters and structures data for recall and thought synthesis; action may emit side effects into the <em>real world</em> (email, APIs, UI), while memory reweighting feeds back into recall—not into ingest or the raw-noise path.</figcaption>
</figure>

## Phase 1: Large-Scale Contextual Ingestion

Phase 1 ingests heterogeneous signals that describe entity state and behavior: emails, direct messages, documents, notes, tickets, logs, and related artifacts. Inputs are normalized, chunked, and mapped into multiple stores: semantic vectors, entity-relation graphs, and ordered state-transition events.

This enables later recall from complementary views: “what is relevant,” “what is connected,” and “what changed over time.” Importantly, ingestion is not passive accumulation; early filtering suppresses repetitive, low-signal, and non-actionable fragments.

## Phase 2: Interval-Based Recall and Thought Synthesis

Phase 2 runs on a fixed interval, independent of user prompts. Each cycle recalls a compact high-salience set using recency, relevance, interaction frequency, and surprise-weighted signals. Instead of passing large raw context to a heavy model, the system sends this compact packet to a lightweight LLM.

The objective is **thought production**, not long-form text generation: short latent-state updates such as “new preference inferred,” “contradiction detected,” “follow-up risk,” or “candidate next action.”

## Phase 3: Action Decision

Phase 3 decides whether to take **external action** or remain passive. Given recalled context and synthesized thoughts, a policy checks confidence and priority thresholds for outbound effects (for example reminders, follow-ups, or external system updates). If thresholds are not met, the loop continues with internal updates only.

## Phase 4: Memory Update and Thought Persistence

Phase 4 closes the cycle by **reweighting** recalled memories. Items that improved the decision are reinforced; items that were unused or misleading are decayed. Figure <a href="#fig:reinforcement-weights" data-reference-type="ref" data-reference="fig:reinforcement-weights">4</a> illustrates this with a toy graph where one activated path is strengthened while alternatives remain weak.

<figure id="fig:reinforcement-weights" data-latex-placement="H">
<img src="figures/reinforcement-weights.png" />
<figcaption>Reinforcement of weights in Phase 4 (toy subgraph): before recall, scattered weak branches; middle, recall selects one path; after reweighting, only that trace is strongly reinforced.</figcaption>
</figure>

Thoughts produced in Phase 2 are also **written back** as durable memory artifacts, so future cycles can retrieve both source evidence and prior latent-state summaries. Reweighting plus thought write-back implements subconscious consolidation between intervals.

## Why This Loop Matters

This loop separates expensive context accumulation from cheap continuous cognition. Periodic recall, lightweight prompting, and explicit write-back maintain evolving internal state without requiring a heavy model call at every interaction.

More importantly, this loop creates system-level traits associated with conscious intelligence: **feedback** (actions and outcomes alter future weights), **learning** (useful patterns are reinforced into latent state), and **forgetting** (noise decays unless revalidated). In this view, TinyCortex is not merely a memory layer; it is a practical control mechanism for adaptive intelligence growth.

# Implementation & Results

We implement TinyCortex as a custom memory stack coupled to a lightweight LLM within the loop above. We then modify the inference layer of an open-source LLM runtime to consume TinyCortex recall packets and write back thought/memory updates each cycle.

<figure id="fig:consciousness-architecture" data-latex-placement="H">
<p><img src="figures/consciousness-architecture.png" style="height:36.0%" alt="image" /> <span id="fig:consciousness-architecture" data-label="fig:consciousness-architecture"></span></p>
</figure>

No LLM is required in ingestion or deterministic recall routing. LLM usage is concentrated in thought synthesis and action decision, which keeps the system practical under large context volumes.

Evaluation is ongoing across reasoning and memory benchmarks to measure both decision quality and retrieval/state accuracy.

## Current Evaluation Status

This draft reports architecture and loop behavior. We do not claim full artificial consciousness or benchmark leadership at this stage. The current evidence is implementation-oriented and qualitative, but it already indicates improvements in feedback-driven adaptation and learning/forgetting balance.

## Observed Behavior

Across long-running internal traces, we observe three recurring effects: **(1)** stronger cross-session continuity from explicit state-event tracking, **(2)** lower active-context noise from retention-aware decay, and **(3)** better performance on temporal/state queries through ledger-based routing.

These gains are most visible in mixed tasks where systems must distinguish “relevant” from “currently true.”

## Limitations and Next Measurement Steps

Current evidence is primarily qualitative. Formal benchmarking, including ablations for routing, forgetting, and thought write-back, remains in progress. The next version will add: **(i)** quantitative deltas against baselines, **(ii)** latency and token-cost decomposition, and **(iii)** error analysis by query type.

# Conclusion

Prior work including Titans/MIRAS, GraphRAG, and MemoryBank demonstrates the importance of structured memory and retention-aware mechanisms. TinyCortex extends this direction with an operational conscious-intelligence loop that integrates ingestion, interval recall, action policy, and write-back reweighting in one production workflow.

Biological inspiration from the human brain (Purkinje-style endogenous activity, Ebbinghaus-style decay, and conscious/subconscious separation) maps to three engineering requirements: selective integration, principled forgetting, and continuous consolidation.

Combined with a strong memory system and an explicit consciousness loop (ingest, recall, act, reinforce), these ingredients yield better intelligence in practice.

<div id="refs" class="references csl-bib-body hanging-indent">

<div id="ref-behrouz2025allconnected" class="csl-entry">

Behrouz, Ali, Meisam Razaviyayn, Peilin Zhong, and Vahab Mirrokni. 2025. “It’s All Connected: A Journey Through Test-Time Memorization, Attentional Bias, Retention, and Online Optimization.” *arXiv Preprint arXiv:2504.13173*.

</div>

<div id="ref-behrouz2025titans" class="csl-entry">

Behrouz, Ali, Peilin Zhong, and Vahab Mirrokni. 2025. “Titans: Learning to Memorize at Test Time.” *arXiv Preprint arXiv:2501.00663*.

</div>

<div id="ref-clewett2024predictions" class="csl-entry">

<span class="nocase">Clewett, Anne, and colleagues</span>. 2024. “Predictions Transform Memories: How Expected Versus Unexpected Events Shape Memory.” *Neuroscience and Biobehavioral Reviews*.

</div>

<div id="ref-edge2024graphrag" class="csl-entry">

Edge, Darren, Ha Trinh, Newman Cheng, et al. 2024. “From Local to Global: A Graph RAG Approach to Query-Focused Summarization.” *arXiv Preprint arXiv:2404.16130*.

</div>

<div id="ref-fountas2024emllm" class="csl-entry">

Fountas, Zafeirios, Martin A. Benfeghoul, Adnan Oomerjee, et al. 2024. “Human-Like Episodic Memory for Infinite Context LLMs.” *arXiv Preprint arXiv:2407.09450*.

</div>

<div id="ref-jimenez2024hipporag" class="csl-entry">

Jiménez Gutiérrez, Bernal, Yiheng Shu, Yu Gu, Michihiro Yasunaga, and Yu Su. 2024. “HippoRAG: Neurobiologically Inspired Long-Term Memory for Large Language Models.” *arXiv Preprint arXiv:2405.14831*.

</div>

<div id="ref-kim2024predictionerror" class="csl-entry">

<span class="nocase">Kim, and colleagues</span>. 2024. “Prediction Error Determines How Memories Are Organized in the Brain.” *eLife*.

</div>

<div id="ref-lewandowsky2015ebbinghaus" class="csl-entry">

<span class="nocase">Lewandowsky, Stephan, Sergio E. Hartwig, and colleagues</span>. 2015. “Replication and Analysis of Ebbinghaus’ Forgetting Curve.” *PLOS ONE*.

</div>

<div id="ref-microsoft2025graphragdocs" class="csl-entry">

Microsoft Research. 2025. *GraphRAG Documentation*. <a href="https://microsoft.github.io/graphrag/" class="uri">Https://microsoft.github.io/graphrag/</a>.

</div>

<div id="ref-wu2025survey" class="csl-entry">

<span class="nocase">Wu, Yaxiong, Xinyue Wang, Yue Zhang, et al.</span> 2025. “From Human Memory to AI Memory: A Survey on Memory Mechanisms in the Era of LLMs.” *arXiv Preprint arXiv:2504.15965*.

</div>

<div id="ref-zhong2023memorybank" class="csl-entry">

Zhong, Wanjun, Lianghong Guo, Qiqi Gao, He Ye, and Yanlin Wang. 2023. “MemoryBank: Enhancing Large Language Models with Long-Term Memory.” *arXiv Preprint arXiv:2305.10250*.

</div>

</div>
