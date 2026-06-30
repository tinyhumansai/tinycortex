---
description: The vocabulary of the TinyCortex crate — memory entries, namespaces, categories, the security taint model, the Memory trait, and interaction-aware scoring and decay.
---

# Core Concepts

This page defines the vocabulary of the TinyCortex crate: the shape of a stored
memory, how memories are partitioned by **namespace** and **category**, the
security **taint** model that travels with every entry, and how
interaction-aware **scoring and decay** shape retrieval. Every concept here is
grounded in real types from `src/memory/types.rs`, `src/memory/traits.rs`, and
`src/memory/config.rs`.

TinyCortex is a Rust library. The types below are the stable public contract
shared across all layers (storage, ingest, retrieval, RPC). They are pure data
with no storage side effects, and are ported faithfully from OpenHuman so the
serialized wire formats (snake_case enum strings, serde defaults) stay
byte-compatible.

## Memory entries

The canonical record returned by recall and lookups is
[`MemoryEntry`](storage-primitives.md) (`src/memory/types.rs`):

```rust
pub struct MemoryEntry {
    pub id: String,               // unique id (usually a UUID)
    pub key: String,              // key or title
    pub content: String,          // the memory body
    pub namespace: Option<String>, // logical partition; None => global
    pub category: MemoryCategory, // organizational category
    pub timestamp: String,        // ISO 8601 create / last-update
    pub session_id: Option<String>, // optional session scope
    pub score: Option<f64>,       // optional relevance/confidence, ~0.0–1.0
    pub taint: MemoryTaint,       // provenance / trust signal
}
```

`MemoryEntry` is the unit the [`Memory`](#the-memory-trait) trait stores and
recalls. Deeper engine layers carry richer records for documents, KV rows, graph
edges, and ranked hits — `StoredMemoryDocument`, `MemoryKvRecord`,
`GraphRelationRecord`, and `NamespaceMemoryHit` — but `MemoryEntry` is the
high-level entry every backend speaks.

### Item kinds

A retrieval hit can resolve to different underlying storage shapes. The
`MemoryItemKind` discriminator (wire strings in the table) tells you which:

| Variant | Wire string | Refers to |
| --- | --- | --- |
| `Document` | `document` | A namespace-scoped memory document (`memory_docs` row) |
| `Kv` | `kv` | A key/value record |
| `Episodic` | `episodic` | An episodic / conversational memory |
| `Event` | `event` | A discrete event entry |

### Timestamps

There are two timestamp conventions in the type surface, and the distinction
matters when porting or serializing:

- **`MemoryEntry.timestamp`** is an **ISO 8601 string** for create / last-update.
- Lower-level persisted records (`StoredMemoryDocument`, `MemoryKvRecord`,
  `GraphRelationRecord`, `NamespaceMemoryHit`) carry **`created_at` /
  `updated_at` as `f64` Unix timestamps in seconds**.
- `NamespaceSummary.last_updated` is an `Option<String>` RFC3339 timestamp for
  agent-side discovery.

`updated_at` feeds the freshness signal during ranking (see
[Scoring and decay](#interaction-aware-scoring-and-decay)).

## Namespaces

A **namespace** is a logical partition — think folders. The same `key` can exist
in multiple namespaces without collision, and recall is normally scoped to one
namespace to cut noise.

```text
preferences          → "User prefers dark mode", "Timezone is PST"
conversation-history → "Discussed Q3 roadmap on March 5"
user-facts           → "Works at Acme Corp", "Based in Austin"
```

Namespaces give you:

- **Separation of concerns** — keep preferences apart from conversation logs
  apart from domain knowledge.
- **Scoped queries** — recall only what is relevant to the current task.
- **Scoped deletion** — clean up an entire category of memories without
  touching others.

### The global namespace

When no explicit namespace is supplied, TinyCortex falls back to a single
well-known default exported as a constant:

```rust
pub const GLOBAL_NAMESPACE: &str = "global";
```

`RecallOpts.namespace` is `Option<&str>`; `None` falls back to
`GLOBAL_NAMESPACE`. At the storage layer, a `None` namespace on a
`MemoryKvRecord` or `GraphRelationRecord` denotes a **global row / relation**
rather than a namespace-scoped one — i.e. `Option<String>` namespaces encode
"global" as `None`.

### Namespace discovery

Agents can enumerate what exists without scanning content via
`Memory::namespace_summaries()`, which returns `NamespaceSummary` rows:

```rust
pub struct NamespaceSummary {
    pub namespace: String,
    pub count: usize,                 // entries currently stored
    pub last_updated: Option<String>, // RFC3339 of most recent update
}
```

## Categories

Every entry is filed under a `MemoryCategory` that captures its nature and
lifecycle (`src/memory/types.rs`):

| Variant | Wire / `Display` string | Meaning |
| --- | --- | --- |
| `Core` | `core` | Long-term foundational facts, user preferences, permanent decisions |
| `Daily` | `daily` | Temporal logs — daily activities or ephemeral state |
| `Conversation` | `conversation` | Context derived from active conversations |
| `Custom(String)` | the inner name | A user- or system-defined custom category |

`MemoryCategory` serializes `rename_all = "snake_case"` and implements `Display`
(e.g. `Custom("travel")` renders as `travel`). Categories are a filter axis for
both `Memory::list(..)` and `RecallOpts.category`, orthogonal to namespaces:
namespaces partition *where* a memory lives, categories classify *what kind* it
is.

## Security taint model

Every memory entry carries a provenance / trust signal called its **taint**.
This is a security primitive, not just metadata: it drives downstream policy on
whether automation whose context contains this content may invoke
external-effect tools.

```rust
pub enum MemoryTaint {
    Internal,     // wire: "internal"      — user-driven content
    ExternalSync, // wire: "external_sync" — ingested from a third-party source
}
```

Rules baked into the type (`src/memory/types.rs`):

- **Default is `Internal`.** Legacy rows with no persisted taint column and all
  in-memory defaults are conservatively trusted as user-driven content.
- **Sync paths MUST set `ExternalSync` at write time.** Any path that ingests
  text from third-party services (Gmail, Slack, Notion, Composio, MCP, …) is
  required to mark the content tainted so callers can refuse external-effect
  tools on tainted context. The trait exposes
  `store_with_taint(..)` precisely for this.
- **Unknown decodes fail closed.** `MemoryTaint::from_db_str` maps `"internal"`
  → `Internal`, `"external_sync"` → `ExternalSync`, and **anything else** →
  `ExternalSync` (the more restrictive value), so content of unknown provenance
  is treated as untrusted.
- **Serialization split.** JSON uses snake_case (`internal` / `external_sync`)
  via serde; the SQLite `memory_docs.taint` column uses `as_db_str()` /
  `from_db_str()`, which happen to share the same strings.

Taint rides along on nearly every record in the type surface — `MemoryEntry`,
`NamespaceDocumentInput`, `NamespaceQueryResult`, `StoredMemoryDocument`, and
`NamespaceMemoryHit` all carry a `#[serde(default)]` `taint` field — so the
trust signal is preserved end-to-end from ingest through retrieval.

{% hint style="info" %}
TinyCortex defines and propagates the taint; it does not itself decide tool
policy. The host (OpenHuman or your application) reads the taint on recalled
context and decides whether to allow external-effect tools. See
[Sources](sources.md) for where `ExternalSync` originates.
{% endhint %}

## The `Memory` trait

The core contract every backend implements is `Memory`
(`src/memory/traits.rs`), an `async_trait`. The methods most relevant to these
concepts:

```rust
#[async_trait]
pub trait Memory: Send + Sync {
    fn name(&self) -> &str; // "sqlite", "vector", "in_memory", …

    async fn store(&self, namespace: &str, key: &str, content: &str,
        category: MemoryCategory, session_id: Option<&str>) -> Result<()>;

    async fn store_with_taint(&self, namespace: &str, key: &str, content: &str,
        category: MemoryCategory, session_id: Option<&str>,
        taint: MemoryTaint) -> Result<()>; // default degrades to store()

    async fn recall(&self, query: &str, limit: usize,
        opts: RecallOpts<'_>) -> Result<Vec<MemoryEntry>>;

    async fn get(&self, namespace: &str, key: &str) -> Result<Option<MemoryEntry>>;
    async fn list(&self, namespace: Option<&str>, category: Option<&MemoryCategory>,
        session_id: Option<&str>) -> Result<Vec<MemoryEntry>>;
    async fn forget(&self, namespace: &str, key: &str) -> Result<bool>;
    async fn namespace_summaries(&self) -> Result<Vec<NamespaceSummary>>;
    async fn count(&self) -> Result<usize>;
    async fn health_check(&self) -> bool;
}
```

`store_with_taint` has a default that degrades to `store` for backends that do
not yet persist taint; `recall_relevant_by_vector` defaults to empty so
keyword-only or mock backends opt out cleanly. `InMemoryMemoryStore` is the
reliably runnable implementation today — see [Getting Started](getting-started.md).

### Recall options

`recall` takes a `RecallOpts<'a>` filter struct:

```rust
pub struct RecallOpts<'a> {
    pub namespace: Option<&'a str>,   // None => GLOBAL_NAMESPACE
    pub category: Option<MemoryCategory>,
    pub session_id: Option<&'a str>,
    pub min_score: Option<f64>,       // drop hits below this (~0.0–1.0)
    pub cross_session: bool,          // include other sessions' conversational hits
}
```

`min_score` thresholds out weak hits; `cross_session = true` widens
conversational recall to other sessions in the same workspace alongside the
namespace recall.

## Interaction-aware scoring and decay

Recall ranks candidate memories by combining several signals. The per-hit
explanation is captured in `RetrievalScoreBreakdown` (`src/memory/types.rs`),
which makes every score auditable:

```rust
pub struct RetrievalScoreBreakdown {
    pub keyword_relevance: f64,   // lexical / keyword match
    pub vector_similarity: f64,   // dense cosine similarity
    pub graph_relevance: f64,     // graph-proximity / co-occurrence
    pub episodic_relevance: f64,  // episodic recall
    pub freshness: f64,           // recency
    pub final_score: f64,         // weighted combination used for ranking
}
```

`NamespaceMemoryHit.score` mirrors `final_score`, and each hit also carries the
full breakdown plus any `supporting_relations` that reinforced its ranking — so
retrieval is explainable rather than a black box.

### Weight profiles

How the signals combine is config-driven. `WeightProfile`
(`src/memory/config.rs`) weights graph / vector / keyword / freshness, and four
named presets ship as constants:

| Profile (`by_name`) | graph | vector | keyword | freshness |
| --- | --- | --- | --- | --- |
| `balanced` (default) | 0.35 | 0.35 | 0.15 | 0.15 |
| `semantic` | 0.15 | 0.65 | 0.20 | 0.00 |
| `lexical` | 0.25 | 0.15 | 0.60 | 0.00 |
| `graph_first` | 0.55 | 0.30 | 0.15 | 0.00 |

`WeightProfile::by_name` resolves a wire name and **falls back to `BALANCED`**
for unknown names. `RetrievalConfig.default_profile` (default `BALANCED`) is
applied when a query specifies no profile. A `freshness` weight of `0.0`
disables recency boosting for that profile. See [Retrieval](retrieval.md) for how
profiles are selected per query.

### Decay and reinforcement

TinyCortex applies a **time-decay model** inspired by the Ebbinghaus Forgetting
Curve:

![Memory decay](.gitbook/assets/memory-decay@2x.png)

- **New memories** start with high retention.
- **Unaccessed memories** decay — their importance decreases over time.
- **Recalled or interacted-with memories** are reinforced — retention resets and
  strengthens.
- **Decayed memories** are effectively pruned, keeping the system lean without
  manual cleanup.

Decay and interaction reinforcement work together: a frequently recalled memory
resists decay and stays front and center, while a memory ingested once and never
touched fades. In the type surface this surfaces through the `freshness` signal
(driven by `updated_at`) and the freshness weight in the active profile.

## Conscious recall

The gitbook concept docs describe higher-level product behaviors —
prompt-driven "conscious recall" that returns an LLM-ready context string
alongside structured chunks. Within **this crate**, the closest real type is
`NamespaceRetrievalContext`, which pairs a rendered `context_text` with the
ranked `hits` that back it:

```rust
pub struct NamespaceRetrievalContext {
    pub namespace: String,
    pub query: Option<String>,
    pub context_text: String,            // ready-to-inject rendered context
    pub hits: Vec<NamespaceMemoryHit>,   // ranked hits backing the context
}
```

{% hint style="info" %}
The turnkey "conscious recall" experience, managed APIs, the billion-token /
cost-per-user figures, and any hosted client SDK are part of the **hosted
OpenHuman platform**, not this open-source crate. TinyCortex provides the Rust
primitives (the `Memory` trait, scoring breakdowns, taint, namespaces) those
products are built on.
{% endhint %}

## See also

- [Storage Primitives](storage-primitives.md)
- [Retrieval](retrieval.md)
- [Architecture Overview](architecture.md)
- [Getting Started](getting-started.md)
