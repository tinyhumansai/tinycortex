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
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MemoryConfig {
    /// Workspace root. Markdown content, SQLite indexes, and ledgers live under
    /// this directory and are authoritative (local-first).
    pub workspace: PathBuf,
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
}

impl MemoryConfig {
    /// Construct a config rooted at `workspace` with all other fields default.
    pub fn new(workspace: impl Into<PathBuf>) -> Self {
        Self {
            workspace: workspace.into(),
            embedding: EmbeddingConfig::default(),
            tree: TreeConfig::default(),
            retrieval: RetrievalConfig::default(),
            sync_budget: SyncBudgetConfig::default(),
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
    pub input_token_budget: u32,
    pub output_token_budget: u32,
    pub summary_fanout: u32,
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
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct WeightProfile {
    pub graph: f64,
    pub vector: f64,
    pub keyword: f64,
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
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct SyncBudgetConfig {
    pub max_tokens_per_sync: Option<u64>,
    pub max_cost_per_sync_usd: Option<f64>,
    pub sync_depth_days: Option<u32>,
}

#[cfg(test)]
#[path = "config_tests.rs"]
mod tests;
