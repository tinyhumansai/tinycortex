//! Config-driven knobs for the memory engine.
//!
//! Every tunable the engine reads at runtime lives here so a host (OpenHuman or
//! a test harness) can construct the whole system from one declarative
//! [`MemoryConfig`]. Defaults mirror the OpenHuman constants documented in
//! `docs/openhuman-memory-engine-spec.md`.

use std::path::PathBuf;

use serde::{Deserialize, Serialize};

/// OpenHuman tree-summarisation input budget (tokens).
pub const INPUT_TOKEN_BUDGET: u32 = 50_000;
/// OpenHuman tree-summarisation output budget (tokens).
pub const OUTPUT_TOKEN_BUDGET: u32 = 5_000;
/// Number of summary siblings before a bucket seals.
pub const SUMMARY_FANOUT: u32 = 10;
/// Default flush age for stale buffers (7 days, in seconds).
pub const DEFAULT_FLUSH_AGE_SECS: u64 = 7 * 24 * 60 * 60;
/// Fixed embedding dimension used by OpenHuman.
pub const DEFAULT_EMBEDDING_DIM: usize = 768;
/// Folder reader per-file size cap (10 MB).
pub const FOLDER_FILE_SIZE_CAP_BYTES: u64 = 10 * 1024 * 1024;

/// Top-level configuration for a memory engine instance.
///
/// This struct performs no validation or filesystem I/O on its own — building
/// one (via [`Self::new`], `Default::default()` on the nested configs, or
/// deserializing from JSON/TOML) never touches disk or fails. Path sandboxing
/// (rejecting traversal / symlink escapes out of `workspace`) and any other
/// invariant enforcement happen where a config is actually consumed (see
/// `memory::sources::validation`), not here.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MemoryConfig {
    /// Workspace root. Markdown content, SQLite indexes, and ledgers live under
    /// this directory and are authoritative (local-first). Not required to
    /// exist yet at construction time — callers create it (or fail informatively)
    /// when they first open the workspace.
    pub workspace: PathBuf,
    /// Optional override for the chunk/summary content vault. `None` uses
    /// `<workspace>/memory_tree/content`.
    #[serde(default)]
    pub content_root: Option<PathBuf>,
    /// Embedding configuration.
    #[serde(default)]
    pub embedding: EmbeddingConfig,
    /// Summary-tree budgets and fan-out.
    #[serde(default)]
    pub tree: TreeConfig,
    /// Default hybrid retrieval weighting.
    #[serde(default)]
    pub retrieval: RetrievalConfig,
    /// Per-source sync budget ceilings (enforced when a host invokes ingest).
    #[serde(default)]
    pub sync_budget: SyncBudgetConfig,
    /// Live synchronization configuration.
    #[serde(default)]
    pub sync: SyncConfig,
}

impl MemoryConfig {
    /// Construct a config rooted at `workspace` with all other fields default.
    ///
    /// Does not touch the filesystem: `workspace` need not exist yet.
    ///
    /// # Examples
    ///
    /// ```
    /// use tinycortex::memory::config::{MemoryConfig, DEFAULT_EMBEDDING_DIM};
    ///
    /// let cfg = MemoryConfig::new("/tmp/my-workspace");
    /// assert_eq!(cfg.embedding.dim, DEFAULT_EMBEDDING_DIM);
    /// ```
    pub fn new(workspace: impl Into<PathBuf>) -> Self {
        Self {
            workspace: workspace.into(),
            content_root: None,
            embedding: EmbeddingConfig::default(),
            tree: TreeConfig::default(),
            retrieval: RetrievalConfig::default(),
            sync_budget: SyncBudgetConfig::default(),
            sync: SyncConfig::default(),
        }
    }
}

/// Embedding backend configuration.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct EmbeddingConfig {
    /// Vector dimension. OpenHuman fixes this at 768.
    pub dim: usize,
    /// Backend model identifier (default Ollama `nomic-embed-text`).
    pub model: String,
    /// When `true`, ingest fails if embeddings are unavailable instead of
    /// degrading to zero vectors.
    pub strict: bool,
}

impl Default for EmbeddingConfig {
    fn default() -> Self {
        Self {
            dim: DEFAULT_EMBEDDING_DIM,
            model: "nomic-embed-text".to_string(),
            strict: false,
        }
    }
}

/// Summary-tree budgets and sealing behavior.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TreeConfig {
    /// Max input tokens fed into one summarisation pass (see [`INPUT_TOKEN_BUDGET`]).
    pub input_token_budget: u32,
    /// Max tokens a summary may emit (see [`OUTPUT_TOKEN_BUDGET`]).
    pub output_token_budget: u32,
    /// Number of summary siblings accumulated before a bucket seals into a parent
    /// (see [`SUMMARY_FANOUT`]).
    pub summary_fanout: u32,
    /// Age, in seconds, after which an unsealed buffer is force-flushed
    /// (see [`DEFAULT_FLUSH_AGE_SECS`]).
    pub flush_age_secs: u64,
}

impl Default for TreeConfig {
    fn default() -> Self {
        Self {
            input_token_budget: INPUT_TOKEN_BUDGET,
            output_token_budget: OUTPUT_TOKEN_BUDGET,
            summary_fanout: SUMMARY_FANOUT,
            flush_age_secs: DEFAULT_FLUSH_AGE_SECS,
        }
    }
}

/// Named hybrid-retrieval weight profiles (graph / vector / keyword / freshness).
///
/// Consumed by `memory::retrieval::scoring::hybrid_score`, which computes the
/// final ranking score as the plain weighted sum
/// `graph·graph_relevance + vector·vector_similarity + keyword·keyword_relevance
/// + freshness·freshness`. Nothing in this type or its consumer *enforces*
/// that the four weights sum to `1.0` — the built-in profiles are chosen that
/// way by convention so scores land in a familiar `[0.0, 1.0]`-ish range when
/// every signal is itself in `[0.0, 1.0]`, but a custom profile with a
/// different total simply rescales the final score.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct WeightProfile {
    /// Weight on graph/co-occurrence proximity signal.
    pub graph: f64,
    /// Weight on dense vector (cosine) similarity signal.
    pub vector: f64,
    /// Weight on lexical/keyword match signal.
    pub keyword: f64,
    /// Weight on recency; `0.0` disables freshness boosting.
    pub freshness: f64,
}

impl WeightProfile {
    /// `balanced`: graph 0.35, vector 0.35, keyword 0.15, freshness 0.15.
    pub const BALANCED: Self = Self {
        graph: 0.35,
        vector: 0.35,
        keyword: 0.15,
        freshness: 0.15,
    };
    /// `semantic`: graph 0.15, vector 0.65, keyword 0.20.
    pub const SEMANTIC: Self = Self {
        graph: 0.15,
        vector: 0.65,
        keyword: 0.20,
        freshness: 0.0,
    };
    /// `lexical`: graph 0.25, vector 0.15, keyword 0.60.
    pub const LEXICAL: Self = Self {
        graph: 0.25,
        vector: 0.15,
        keyword: 0.60,
        freshness: 0.0,
    };
    /// `graph_first`: graph 0.55, vector 0.30, keyword 0.15.
    pub const GRAPH_FIRST: Self = Self {
        graph: 0.55,
        vector: 0.30,
        keyword: 0.15,
        freshness: 0.0,
    };

    /// Resolve a profile by its wire name. Unknown names fall back to balanced.
    ///
    /// # Examples
    ///
    /// ```
    /// use tinycortex::memory::config::WeightProfile;
    ///
    /// assert_eq!(WeightProfile::by_name("semantic"), WeightProfile::SEMANTIC);
    /// // Unrecognised names fail closed to the balanced default, never an error.
    /// assert_eq!(WeightProfile::by_name("nonexistent"), WeightProfile::BALANCED);
    /// ```
    pub fn by_name(name: &str) -> Self {
        match name {
            "semantic" => Self::SEMANTIC,
            "lexical" => Self::LEXICAL,
            "graph_first" => Self::GRAPH_FIRST,
            _ => Self::BALANCED,
        }
    }
}

/// Default retrieval configuration.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RetrievalConfig {
    /// Default weight profile applied when a query does not specify one.
    pub default_profile: WeightProfile,
}

impl Default for RetrievalConfig {
    fn default() -> Self {
        Self {
            default_profile: WeightProfile::BALANCED,
        }
    }
}

/// Per-sync budget ceilings, enforceable when a host requests ingest.
///
/// This struct is the engine-wide default; distinct, independently-overridable
/// budget fields of the same shape also live per-source on
/// `memory::sources::types` (see that module's registry entries), which is
/// where actual sync-time enforcement is wired today. Treat this type as the
/// fallback a host applies when a source has not set its own override, rather
/// than an already-enforced global cap.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct SyncBudgetConfig {
    /// Maximum records persisted by one sync tick.
    pub max_items: Option<u32>,
    /// Token ceiling per ingest run; `None` leaves token spend unbounded.
    pub max_tokens_per_sync: Option<u64>,
    /// USD cost ceiling per ingest run; `None` leaves cost unbounded.
    pub max_cost_per_sync_usd: Option<f64>,
    /// How many days back a source sync may reach; `None` imposes no horizon.
    pub sync_depth_days: Option<u32>,
}

/// Live synchronization configuration.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct SyncConfig {
    /// Request and ingest ceilings.
    #[serde(default)]
    pub budget: SyncBudgetConfig,
    /// Composio transport configuration; absent disables Composio sync.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub composio: Option<ComposioSyncConfig>,
    /// Global periodic cadence. `Some(0)` means manual-only; shorter non-zero
    /// values are clamped to the 24-hour default.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub interval_secs: Option<u64>,
}

/// Composio HTTP transport mode.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ComposioMode {
    /// Call backend.composio.dev using a BYO API key.
    Direct,
    /// Call a host proxy using a bearer token.
    #[default]
    Proxied,
}

/// Redacted secret value. Debug and Display never reveal the payload.
#[derive(Clone, Default, PartialEq, Eq)]
pub struct SecretString(String);

impl SecretString {
    pub fn new(value: impl Into<String>) -> Self {
        Self(value.into())
    }

    pub fn expose(&self) -> &str {
        &self.0
    }

    pub fn is_empty(&self) -> bool {
        self.0.trim().is_empty()
    }
}

impl std::fmt::Debug for SecretString {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("SecretString([REDACTED])")
    }
}

impl std::fmt::Display for SecretString {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("[REDACTED]")
    }
}

/// Composio connection settings injected by the host.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ComposioSyncConfig {
    #[serde(default)]
    pub mode: ComposioMode,
    pub base_url: String,
    /// Direct-mode key. Never serialized or printed.
    #[serde(skip)]
    pub api_key: Option<SecretString>,
    /// Proxied-mode bearer. Never serialized or printed.
    #[serde(skip)]
    pub bearer_token: Option<SecretString>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub entity_id: Option<String>,
}

#[cfg(test)]
#[path = "config_tests.rs"]
mod tests;
