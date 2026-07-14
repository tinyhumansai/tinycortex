# Doc 06 follow-on — persona memory engine live run + decision agent

A working demonstration of the two things the persona surface was built to do:

1. **One full run of the memory engine** over this machine's real coding-agent
   history, and
2. **User-like intelligence** surfaced from that memory to answer hard coding
   decisions — via an algorithmic retriever plus an LLM final pass.

Two new pieces land alongside the existing `persona` surface:

- `memory::persona::retrieve` — a purely lexical, **no-LLM** BM25 retriever over
  the persona observation leaves (`retrieve.rs` + `retrieve_tests.rs`).
- `examples/persona_agent.rs` — a `tinyagents` agent harness with read-only tools
  over that retriever; DeepSeek v4 Flash (OpenRouter) does the final synthesis.

---

## 1. The live memory-engine run

Ran `examples/persona_harness backfill` over this box, capped at 80 sessions /
$0.60:

```
sources : ~/.claude/projects (Claude Code) + ~/.codex/sessions (Codex)
          + CLAUDE.md / AGENTS.md + git history (author-filtered)
model   : deepseek/deepseek-v4-flash (digest map) via OpenRouter
```

Result:

| metric | value |
|---|---|
| sessions selected / processed / failed | 80 / 75 / 5 |
| non-empty digests | 71 |
| evidence units read | 16,952 |
| observations distilled | 927 |
| directive rules folded (verbatim T0) | 1,372 |
| provider requests | 109 |
| tokens (prompt + completion) | 232,401 + 275,703 |
| **cost** | **$0.0848** |
| wall-clock | 31 min (network-bound, 33% CPU) |
| `PERSONA.md` | 8,590 bytes (~2.1k tokens), 7 facets |

The compiled pack reproduced Steven's real, independently-known preferences with
no fabrication: branch-before-code + small conventional commits, Rust core / Go
(`tinyhumansai/tinyplace`) backend / Tauri+React frontend split, zsh on Linux,
Claude Code + Codex harnesses, "never mock the DB — use the real store", "no
per-handler in-memory caches — use the Redis middleware", terse imperative
prompting style.

The persisted memory layer (what the retriever loads):

```
memory_tree/chunks.db   932 observation leaves + 1,735 directive rules
  Directives  193   Communication 116   Coding style 157   Stack 86
  Workflow    179   Environment    66   Anti-preferences 135
```

---

## 2. Algorithmic retrieval (no LLM)

`PersonaRetriever::load(&config)` reads every `owner="persona"` Document chunk
(one per facet tree), splits each rendered `- <observation> ("<quote>") [tN]`
leaf line into an indexed document, and ranks against a query with:

```
score(doc) = BM25(query, doc; k1=1.5, b=0.75) × tier_weight(doc.tier)
tier_weight: T0 1.0  T1 0.9  T2 0.7  T3 0.4   (mirrors reduce::tier_score)
```

Deterministic, network-free, ties broken by recency. No per-observation
embeddings exist on disk (the pipeline seals with `embedder: None`), and paying
an embedding call at query time would make retrieval itself model-dependent — so
retrieval stays lexical and the LLM only enters at the final pass. 10 unit +
integration tests (parsing, tokenizer, BM25 ranking, facet filter, tier
weighting, real-workspace load roundtrip).

## 3. The decision agent (LLM final pass)

`AgentHarness<PersonaState>` with three read-only tools over the retriever —
`search_persona` (BM25, optional facet filter), `list_directives` (verbatim T0
rules), `persona_overview` (facet coverage). The system prompt forces the model
to ground every claim in retrieved evidence, prefer higher tiers, and flag when
it is extrapolating. Retrieval selects candidates; the LLM only filters,
resolves tier/recency conflicts, and writes the decision in the person's voice.

### Live results (DeepSeek v4 Flash via OpenRouter)

| question | model/tool calls | wall-clock | surfaced (verifiable) |
|---|---|---|---|
| Retry/backoff: dep vs hand-roll? | 8 / 15 | 191s | reasoned by analogy from the t0 "use the Redis middleware, don't hand-roll per-handler caches" rule → pick a focused crate |
| Bug-fix flow from `main`? | 6 / 9 | 186s | `fix/` branch off clean upstream, **failing test first**, separate commits, PR with validation, **babysit CI ~5 min up to 12 ticks** |
| Where do tests go / keep module small? | 5 / 10 | 100s | sibling `*_tests.rs`, ≤500-line cap, export-only `mod.rs`, `types.rs` split — the exact tinycortex/openhuman rules |

Each answer is decisive, in-voice, and cites the specific observations/directives
(with tiers) it relied on — auditable back to the memory layer.

## Reproduce

```sh
# 1. one full memory-engine run (writes the persona layer)
OPENROUTER_API_KEY=… PERSONA_MAX_SESSIONS=80 PERSONA_MAX_COST_USD=0.60 \
TINYCORTEX_WORKSPACE=/path/ws PERSONA_AUTHOR_EMAILS=you@example.com \
  cargo run --example persona_harness --features persona,providers-http,git-diff -- backfill

# 2. ask the decision agent (stage 1 algorithmic retrieval → stage 2 LLM pass)
OPENROUTER_API_KEY=… TINYCORTEX_WORKSPACE=/path/ws \
  cargo run --example persona_agent --features persona -- "<your hard coding question>"
```
