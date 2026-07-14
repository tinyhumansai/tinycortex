# Audit 08 — Configurability as a Library

_Audit date: 2026-07-14 · Baseline: `main` @ `1a3fcc5`._

Scope: whether everything a host embedding TinyCortex might tune flows through
config, whether env vars stay at the edges, and whether feature gates are
clean. IDs are `CF-*`. Config *validation* gaps are already CT-6 (audit 06);
this audit is about coverage and reachability.

## What already works

`src/memory/config.rs` is a genuinely good root: one declarative
`MemoryConfig` with nested `EmbeddingConfig` / `TreeConfig` /
`RetrievalConfig` / `SyncBudgetConfig` / `SyncConfig`, serde round-trippable,
secrets held in a redacting `SecretString` with `#[serde(skip)]`. Config is
threaded by reference (`config: &MemoryConfig`) through ~400 call sites rather
than stashed in globals. Feature gates are clean: `default = []`, each heavy
dependency (`git2`, `reqwest`, `tokio`) is opt-in, module gating matches the
manifest, and no gated type leaks were found. The problems below are about
what the config *doesn't* reach.

## Findings

### CF-1 (High). `sync.interval_secs` is silently floored to 24 hours

`src/memory/sync/periodic.rs:13-19`: `effective_interval_secs` maps
`Some(seconds)` to `seconds.max(DEFAULT_SYNC_INTERVAL_SECS)` where the default
is 24h. A host setting `interval_secs = 3600` gets daily sync with no error,
no log, no doc on the config field stronger than "shorter non-zero values are
clamped" (`config.rs:253-254`). The knob exists but cannot be turned below the
floor — the config surface is misleading.

**Fix:** either make the floor itself a config field
(`min_interval_secs`, default 24h) or honor the configured value and move
rate-limiting concerns to `SyncBudgetConfig` where they belong.

### CF-2 (High). Summariser constants shadow and contradict `TreeConfig`

`src/memory/tree/summarise.rs:17-19` hardcodes
`MAX_SUMMARY_OUTPUT_TOKENS = 5_000`, `NUM_CTX_TOKENS = 60_000`,
`OVERHEAD_RESERVE_TOKENS = 2_048`, and `summarise.rs:99` applies
`ctx.token_budget.min(MAX_SUMMARY_OUTPUT_TOKENS)`. So a host raising
`TreeConfig::output_token_budget` above 5 000 is silently capped, and the
60 000 context figure disagrees with `TreeConfig::input_token_budget`'s
50 000. `bucket_seal.rs` explicitly promises "Budgets are read from
`MemoryConfig::tree`, not hardcoded" — the summariser breaks that promise one
layer down.

**Fix:** move all three into `TreeConfig` (the first two arguably *are*
`output_token_budget`/`input_token_budget` and should be deleted in favor of
the existing fields).

### CF-3 (High). Library code reads `COMPOSIO_API_KEY` from the process env

`src/memory/sync/composio/client.rs:138`:
`.or_else(|| std::env::var("COMPOSIO_API_KEY").ok())` inside `execute_direct`.
This is the only `std::env` read in `src/` — everywhere else env handling
correctly lives in tests/examples (`dotenvy` is dev-only). A library silently
picking up ambient credentials undermines the `ComposioSyncConfig.api_key`
contract and makes behavior differ between hosts in ways config can't explain.

**Fix:** delete the fallback; require the key via config. The
`composio_harness` example already shows the edge doing env → config
translation properly.

### CF-4 (Medium). Three config roots instead of one

`ScoringConfig` (`score/mod.rs`: weights, `drop_threshold` 0.3,
`definite_keep` 0.85, `definite_drop` 0.15) and `MemoryIngestionConfig`
(`ingest/extract/types.rs:40`: model name, extraction mode, thresholds, batch
size) are constructed and passed independently — neither is a field of
`MemoryConfig`. A host cannot describe the whole engine in one deserialized
document, which is the stated purpose of `config.rs` ("construct the whole
system from one declarative `MemoryConfig`", config.rs:3-5).

**Fix:** add `scoring: ScoringConfig` and `ingestion: MemoryIngestionConfig`
fields (per-call overrides can stay as parameters).

### CF-5 (Medium). Retrieval is tuned entirely by module constants

`RetrievalConfig` holds only `default_profile`. Every operative limit is a
const: `retrieval/fast.rs:18-22` (`LOOKUP_LIMIT=500`, `DEFAULT_LIMIT=10`,
`MAX_RETRIEVE_LIMIT=100`, `DEFAULT_MAX_HOPS=2`, `MAX_GRAPH_HOPS=4`),
`search.rs:34,36` (5/100), `cover.rs:32,43` (200/5 000), `global.rs:32,38`
(10/200), `graph_adapter.rs:34` (500), `fetch.rs:24` (20), `source.rs:43`
(10), `graph/query.rs:28` (100), plus
`retrieval/scoring.rs:16` `DEFAULT_FRESHNESS_HALF_LIFE_DAYS = 7.0` (the
`freshness()` function takes it as a parameter, but no config field feeds it).

**Fix:** a `RetrievalLimits` struct in `RetrievalConfig` covering the default
limit, hard caps, hop counts, and freshness half-life. Constants can remain as
the `Default` impl values, mirroring how `config.rs` already treats
`INPUT_TOKEN_BUDGET` et al.

### CF-6 (Medium). Queue/retry/backoff policy is hardcoded

`queue/store.rs:36-41`: `DEFAULT_LOCK_DURATION_MS = 300_000`,
`RETRY_BASE_MS = 60_000`, `RETRY_CAP_MS = 3_600_000`,
`DEFAULT_MAX_ATTEMPTS = 5` (per-job override exists but no engine-wide knob);
`queue/gate.rs:23` `DEFAULT_LLM_PERMITS = 1`;
`sync/composio/client.rs:84,118-119` (3 attempts, `250·2ⁿ` ms backoff);
`score/extract/llm.rs:43,172` (`EXTRACTION_MAX_OUTPUT_TOKENS = 8192`,
3 attempts). Hosts running server-side (see
`configurable-store.md`) will need these; a `QueueConfig` /
`RetryPolicy` in `MemoryConfig` is the natural home, and consolidating the
scattered retry loops behind one policy type is also a simplification win
(see SW-5 in audit 10).

### CF-7 (Low). Assorted behavioral constants with no knob

For completeness — each is defensible as a constant, but they are where
tuning pressure will land next:

- `sync/state.rs:9` `DEFAULT_DAILY_REQUEST_LIMIT = 500` (no
  `SyncBudgetConfig` field corresponds).
- `score/mod.rs:85` `PRIORITY_BOOST = 0.25`; signal ramps in
  `score/signals/token_count.rs:12-19` and `unique_words.rs:13`.
- `chunks/produce.rs:29` `DEFAULT_CHUNK_MAX_TOKENS = 3_000`;
  `ingest/extract/types.rs:13` `DEFAULT_CHUNK_TOKENS = 225`;
  `produce_split.rs:12,15` overlap 12%/40%.
- `tree/store/types.rs:223-229` topic lifecycle thresholds (10.0 / 2.0 / 100);
  `bucket_seal.rs:48` `MAX_CASCADE_DEPTH = 32` and doc-subtree fan-in/
  concurrency; `tree/runtime/engine.rs:24` `MAX_SUMMARY_CHARS`.
- `goals/store.rs:41,45` (2 000 chars / 8 items);
  `tool_memory/store.rs:37` (prompt cap 30).
- `chunks/connection.rs:66,68` circuit breaker (3 failures / 30s);
  `chunks/mod.rs:124` busy timeout 15s.
- `diff/ledger.rs:43-44` hardcoded git identity
  (`"TinyCortex Memory"` / `memory-diff@tinycortex.local`) — the one item
  here that plausibly *must* be configurable for multi-tenant hosting.
- Workspace layout names (`memory_tree/chunks.db`, `MEMORY_GOALS.md`,
  `episodic/`, `entities/`, `skill_docs.db`, `threads.jsonl`) are fixed
  relative to `workspace`; only the root and `content_root` are overridable.
  Fixed layout is a reasonable contract — but it should be *documented as*
  the contract in one place rather than discovered across nine constants.

### CF-8 (Low). No config-file loading helper

`MemoryConfig` derives `Deserialize` but the crate ships no
`MemoryConfig::from_toml_file`/`from_json` helper; the only file-config I/O in
the crate is the `SourceRegistry` TOML (`sources/registry.rs:70-125`). Every
host reinvents load-parse-validate. Combined with CT-6 (no `validate()`),
adding `load(path) -> Result<Self>` that parses *and* validates would give
hosts one obvious, safe entry point — and gives the setup scripts (audit 09)
something to generate.

## Priority order

CF-1/CF-2/CF-3 are behavioral traps (config that lies) — fix first and each is
small. CF-4/CF-5/CF-6 are the structural work that makes the crate genuinely
host-tunable. CF-7/CF-8 ride along with whichever module they touch.
