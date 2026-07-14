//! Config-driven knobs for the memory engine.
//!
//! Every tunable the engine reads at runtime lives here so a host (OpenHuman or
//! a test harness) can construct the whole system from one declarative
//! [`MemoryConfig`]. Defaults mirror the OpenHuman constants documented in
//! `docs/openhuman-memory-engine-spec.md`.

use std::path::PathBuf;

use anyhow::Context;
use serde::{Deserialize, Serialize};

mod policy;
pub use policy::{QueueConfig, RetrievalLimits, ScoringPolicyConfig};

/// OpenHuman tree-summarisation input budget (tokens).
pub const INPUT_TOKEN_BUDGET: u32 = 50_000;
/// OpenHuman tree-summarisation output budget (tokens).
pub const OUTPUT_TOKEN_BUDGET: u32 = 5_000;
/// Prompt/instruction headroom reserved inside the summarisation input budget.
pub const SUMMARY_OVERHEAD_RESERVE_TOKENS: u32 = 2_048;
/// Number of summary siblings before a bucket seals.
pub const SUMMARY_FANOUT: u32 = 10;
/// Default flush age for stale buffers (7 days, in seconds).
pub const DEFAULT_FLUSH_AGE_SECS: u64 = 7 * 24 * 60 * 60;
/// Default token budget for a flavoured tree's compiled root markdown artifact.
/// The compiled profile is clamped to this so it can be dropped verbatim into a
/// prompt as a small, always-fresh style/preference guide (issue #68).
pub const FLAVOUR_ROOT_TOKEN_BUDGET: u32 = 1_000;
/// Fixed embedding dimension used by OpenHuman.
pub const DEFAULT_EMBEDDING_DIM: usize = 768;
/// Folder reader per-file size cap (10 MB).
pub const FOLDER_FILE_SIZE_CAP_BYTES: u64 = 10 * 1024 * 1024;

/// Top-level configuration for a memory engine instance.
///
/// Direct construction and serde deserialization do not touch disk; call
/// [`Self::validate`] after constructing manually. [`Self::from_toml_file`]
/// is the safe file-loading entry point and validates before returning. Path
/// sandboxing for individual source paths remains enforced where those paths
/// are consumed (see `memory::sources::validation`).
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
    /// Admission-scoring policy. Runtime extractor implementations remain
    /// injected separately and are combined with this serializable policy.
    #[serde(default)]
    pub scoring: ScoringPolicyConfig,
    /// Deterministic document-extraction policy.
    #[serde(default)]
    pub ingestion: crate::memory::ingest::MemoryIngestionConfig,
    /// Queue locking, retry, and concurrency policy.
    #[serde(default)]
    pub queue: QueueConfig,
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
            scoring: ScoringPolicyConfig::default(),
            ingestion: crate::memory::ingest::MemoryIngestionConfig::default(),
            queue: QueueConfig::default(),
            sync_budget: SyncBudgetConfig::default(),
            sync: SyncConfig::default(),
        }
    }

    /// Load a TOML configuration file and validate its runtime invariants.
    ///
    /// Relative workspace paths remain relative to the host's current working
    /// directory; the loader does not silently reinterpret them relative to the
    /// config file.
    pub fn from_toml_file(path: impl AsRef<std::path::Path>) -> anyhow::Result<Self> {
        let path = path.as_ref();
        let text = std::fs::read_to_string(path)
            .map_err(anyhow::Error::from)
            .with_context(|| format!("failed to read memory config {}", path.display()))?;
        let config: Self = toml::from_str(&text)
            .map_err(anyhow::Error::from)
            .with_context(|| format!("failed to parse memory config {}", path.display()))?;
        config.validate()?;
        Ok(config)
    }

    /// Validate configuration values that would otherwise produce degenerate
    /// stores, budgets, or ranking behavior.
    pub fn validate(&self) -> anyhow::Result<()> {
        anyhow::ensure!(
            !self.workspace.as_os_str().is_empty(),
            "workspace must not be empty"
        );
        self.scoring.validate()?;
        self.queue.validate()?;
        self.retrieval.limits.validate()?;
        anyhow::ensure!(
            (0.0..=1.0).contains(&self.ingestion.entity_threshold)
                && (0.0..=1.0).contains(&self.ingestion.relation_threshold)
                && (0.0..=1.0).contains(&self.ingestion.adjacency_threshold),
            "ingestion thresholds must be between zero and one"
        );
        anyhow::ensure!(
            self.ingestion.batch_size > 0,
            "ingestion.batch_size must be positive"
        );
        anyhow::ensure!(
            self.embedding.dim > 0,
            "embedding.dim must be greater than zero"
        );
        anyhow::ensure!(
            self.tree.input_token_budget > 0,
            "tree.input_token_budget must be greater than zero"
        );
        anyhow::ensure!(
            self.tree.output_token_budget > 0,
            "tree.output_token_budget must be greater than zero"
        );
        anyhow::ensure!(
            self.tree.summary_overhead_reserve_tokens < self.tree.input_token_budget,
            "tree.summary_overhead_reserve_tokens must be smaller than tree.input_token_budget"
        );
        anyhow::ensure!(
            self.tree.summary_fanout > 0,
            "tree.summary_fanout must be greater than zero"
        );
        anyhow::ensure!(
            self.tree.flush_age_secs > 0,
            "tree.flush_age_secs must be greater than zero"
        );
        anyhow::ensure!(
            self.tree.flavour_root_token_budget > 0,
            "tree.flavour_root_token_budget must be greater than zero"
        );
        let weights = [
            self.retrieval.default_profile.graph,
            self.retrieval.default_profile.vector,
            self.retrieval.default_profile.keyword,
            self.retrieval.default_profile.freshness,
        ];
        anyhow::ensure!(
            weights
                .iter()
                .all(|weight| weight.is_finite() && *weight >= 0.0),
            "retrieval weights must be finite and non-negative"
        );
        anyhow::ensure!(
            weights.iter().sum::<f64>() > 0.0,
            "at least one retrieval weight must be positive"
        );
        validate_budget("sync_budget", &self.sync_budget)?;
        validate_budget("sync.budget", &self.sync.budget)?;
        if let Some(composio) = &self.sync.composio {
            let base_url = composio.base_url.trim();
            anyhow::ensure!(
                base_url.starts_with("https://") || base_url.starts_with("http://"),
                "sync.composio.base_url must be an http(s) URL"
            );
            anyhow::ensure!(
                composio.api_key.as_ref().is_none_or(|key| !key.is_empty()),
                "sync.composio.api_key must not be blank"
            );
            anyhow::ensure!(
                composio
                    .bearer_token
                    .as_ref()
                    .is_none_or(|token| !token.is_empty()),
                "sync.composio.bearer_token must not be blank"
            );
        }
        Ok(())
    }
}

fn validate_budget(name: &str, budget: &SyncBudgetConfig) -> anyhow::Result<()> {
    anyhow::ensure!(
        budget.max_items.is_none_or(|value| value > 0),
        "{name}.max_items must be greater than zero"
    );
    anyhow::ensure!(
        budget.max_tokens_per_sync.is_none_or(|value| value > 0),
        "{name}.max_tokens_per_sync must be greater than zero"
    );
    anyhow::ensure!(
        budget
            .max_cost_per_sync_usd
            .is_none_or(|value| value.is_finite() && value >= 0.0),
        "{name}.max_cost_per_sync_usd must be finite and non-negative"
    );
    anyhow::ensure!(
        budget.sync_depth_days.is_none_or(|value| value > 0),
        "{name}.sync_depth_days must be greater than zero"
    );
    Ok(())
}

/// Embedding backend configuration.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
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
#[serde(default)]
pub struct TreeConfig {
    /// Max input tokens fed into one summarisation pass (see [`INPUT_TOKEN_BUDGET`]).
    pub input_token_budget: u32,
    /// Max tokens a summary may emit (see [`OUTPUT_TOKEN_BUDGET`]).
    pub output_token_budget: u32,
    /// Tokens reserved for provider instructions and formatting rather than
    /// source inputs.
    #[serde(default = "default_summary_overhead_reserve_tokens")]
    pub summary_overhead_reserve_tokens: u32,
    /// Number of summary siblings accumulated before a bucket seals into a parent
    /// (see [`SUMMARY_FANOUT`]).
    pub summary_fanout: u32,
    /// Age, in seconds, after which an unsealed buffer is force-flushed
    /// (see [`DEFAULT_FLUSH_AGE_SECS`]).
    pub flush_age_secs: u64,
    /// Token budget for a [`TreeKind::Flavoured`](crate::memory::tree::TreeKind::Flavoured)
    /// tree's compiled root markdown artifact (see [`FLAVOUR_ROOT_TOKEN_BUDGET`]).
    /// The compiled profile body is clamped to this before it is staged.
    #[serde(default = "default_flavour_root_token_budget")]
    pub flavour_root_token_budget: u32,
}

/// Serde default for [`TreeConfig::flavour_root_token_budget`] so configs
/// deserialised from older payloads (without the field) keep the 1000-token
/// default instead of `0`.
fn default_flavour_root_token_budget() -> u32 {
    FLAVOUR_ROOT_TOKEN_BUDGET
}

fn default_summary_overhead_reserve_tokens() -> u32 {
    SUMMARY_OVERHEAD_RESERVE_TOKENS
}

impl Default for TreeConfig {
    fn default() -> Self {
        Self {
            input_token_budget: INPUT_TOKEN_BUDGET,
            output_token_budget: OUTPUT_TOKEN_BUDGET,
            summary_overhead_reserve_tokens: SUMMARY_OVERHEAD_RESERVE_TOKENS,
            summary_fanout: SUMMARY_FANOUT,
            flush_age_secs: DEFAULT_FLUSH_AGE_SECS,
            flavour_root_token_budget: FLAVOUR_ROOT_TOKEN_BUDGET,
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
///
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

    /// Resolve a profile by its wire name, returning `None` for unknown names.
    ///
    /// # Examples
    ///
    /// ```
    /// use tinycortex::memory::config::WeightProfile;
    ///
    /// assert_eq!(WeightProfile::by_name("semantic"), Some(WeightProfile::SEMANTIC));
    /// assert_eq!(WeightProfile::by_name("nonexistent"), None);
    /// ```
    pub fn by_name(name: &str) -> Option<Self> {
        match name {
            "balanced" => Some(Self::BALANCED),
            "semantic" => Some(Self::SEMANTIC),
            "lexical" => Some(Self::LEXICAL),
            "graph_first" => Some(Self::GRAPH_FIRST),
            _ => None,
        }
    }
}

/// Default retrieval configuration.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct RetrievalConfig {
    /// Default weight profile applied when a query does not specify one.
    pub default_profile: WeightProfile,
    /// Operative result, candidate, graph, and paging bounds.
    pub limits: RetrievalLimits,
}

impl Default for RetrievalConfig {
    fn default() -> Self {
        Self {
            default_profile: WeightProfile::BALANCED,
            limits: RetrievalLimits::default(),
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
#[serde(default)]
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
#[serde(default)]
pub struct SyncConfig {
    /// Request and ingest ceilings.
    #[serde(default)]
    pub budget: SyncBudgetConfig,
    /// Composio transport configuration; absent disables Composio sync.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub composio: Option<ComposioSyncConfig>,
    /// Global periodic cadence. `Some(0)` means manual-only; every other
    /// configured value is honoured exactly.
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
