//! The evaluation loop: ingest a dataset into a backend, run every query, and
//! aggregate the ranking metrics into a serializable [`RunReport`].

use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};

use crate::backend::BenchBackend;
use crate::dataset::Dataset;
use crate::metrics;

/// Knobs controlling which cutoffs are reported.
#[derive(Clone, Debug)]
pub struct HarnessConfig {
    /// Cutoffs to report recall@k / precision@k / hit@k at.
    pub k_values: Vec<usize>,
    /// Cutoff for nDCG@k.
    pub ndcg_k: usize,
}

impl Default for HarnessConfig {
    fn default() -> Self {
        Self {
            k_values: vec![1, 5, 10],
            ndcg_k: 10,
        }
    }
}

impl HarnessConfig {
    /// The number of results to request from the backend — the largest cutoff
    /// we score at, so every reported metric has the ranks it needs.
    fn retrieve_k(&self) -> usize {
        self.k_values
            .iter()
            .copied()
            .max()
            .unwrap_or(10)
            .max(self.ndcg_k)
    }
}

/// Metrics for a single query.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct QueryResult {
    /// The query's id from the dataset.
    pub query_id: String,
    /// Ids the backend returned, in rank order (top `retrieve_k`).
    pub retrieved: Vec<String>,
    /// recall@k keyed by k.
    pub recall_at_k: BTreeMap<usize, f64>,
    /// precision@k keyed by k.
    pub precision_at_k: BTreeMap<usize, f64>,
    /// hit@k keyed by k.
    pub hit_at_k: BTreeMap<usize, f64>,
    /// Reciprocal rank of the first relevant hit (0 if none).
    pub reciprocal_rank: f64,
    /// nDCG at [`HarnessConfig::ndcg_k`].
    pub ndcg: f64,
}

/// Dataset-wide aggregate (means over all queries).
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Aggregate {
    /// Number of queries averaged.
    pub query_count: usize,
    /// Mean recall@k keyed by k.
    pub mean_recall_at_k: BTreeMap<usize, f64>,
    /// Mean precision@k keyed by k.
    pub mean_precision_at_k: BTreeMap<usize, f64>,
    /// Mean hit@k keyed by k.
    pub mean_hit_at_k: BTreeMap<usize, f64>,
    /// Mean reciprocal rank == MRR.
    pub mrr: f64,
    /// Mean nDCG@`ndcg_k`.
    pub mean_ndcg: f64,
}

/// A full benchmark run: provenance + per-query rows + the aggregate.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct RunReport {
    /// RFC 3339 UTC timestamp of the run.
    pub timestamp: String,
    /// Free-form build label (e.g. a git sha) for cross-commit diffs.
    pub label: String,
    /// Backend name ([`BenchBackend::name`]).
    pub backend: String,
    /// Dataset name ([`Dataset::name`]).
    pub dataset: String,
    /// nDCG cutoff used.
    pub ndcg_k: usize,
    /// Per-query breakdown.
    pub per_query: Vec<QueryResult>,
    /// Aggregate metrics.
    pub aggregate: Aggregate,
}

/// Ingest `dataset` into `backend`, run every query, and return the report.
///
/// Documents are ingested in dataset order; queries then run against the loaded
/// backend. `label` and `timestamp` are provenance stamped verbatim into the
/// report so the caller controls the clock and build identity.
pub async fn run(
    backend: &dyn BenchBackend,
    dataset: &Dataset,
    config: &HarnessConfig,
    timestamp: String,
    label: String,
) -> anyhow::Result<RunReport> {
    for doc in &dataset.documents {
        backend.ingest(doc).await?;
    }

    let retrieve_k = config.retrieve_k();
    let mut per_query = Vec::with_capacity(dataset.queries.len());

    for query in &dataset.queries {
        let retrieved = backend
            .query(query.namespace.as_deref(), &query.query, retrieve_k)
            .await?;
        let relevant = query.relevant_set();

        let mut recall_at_k = BTreeMap::new();
        let mut precision_at_k = BTreeMap::new();
        let mut hit_at_k = BTreeMap::new();
        for &k in &config.k_values {
            recall_at_k.insert(k, metrics::recall_at_k(&retrieved, &relevant, k));
            precision_at_k.insert(k, metrics::precision_at_k(&retrieved, &relevant, k));
            hit_at_k.insert(k, metrics::hit_at_k(&retrieved, &relevant, k));
        }

        per_query.push(QueryResult {
            query_id: query.id.clone(),
            reciprocal_rank: metrics::reciprocal_rank(&retrieved, &relevant),
            ndcg: metrics::ndcg_at_k(&retrieved, &relevant, config.ndcg_k),
            retrieved,
            recall_at_k,
            precision_at_k,
            hit_at_k,
        });
    }

    let aggregate = aggregate(&per_query, config);

    Ok(RunReport {
        timestamp,
        label,
        backend: backend.name().to_string(),
        dataset: dataset.name.clone(),
        ndcg_k: config.ndcg_k,
        per_query,
        aggregate,
    })
}

/// Mean each metric across the per-query rows.
fn aggregate(per_query: &[QueryResult], config: &HarnessConfig) -> Aggregate {
    let n = per_query.len();
    let denom = n.max(1) as f64;

    let mean_over = |select: &dyn Fn(&QueryResult, usize) -> f64| -> BTreeMap<usize, f64> {
        config
            .k_values
            .iter()
            .map(|&k| {
                let sum: f64 = per_query.iter().map(|q| select(q, k)).sum();
                (k, sum / denom)
            })
            .collect()
    };

    Aggregate {
        query_count: n,
        mean_recall_at_k: mean_over(&|q, k| q.recall_at_k[&k]),
        mean_precision_at_k: mean_over(&|q, k| q.precision_at_k[&k]),
        mean_hit_at_k: mean_over(&|q, k| q.hit_at_k[&k]),
        mrr: per_query.iter().map(|q| q.reciprocal_rank).sum::<f64>() / denom,
        mean_ndcg: per_query.iter().map(|q| q.ndcg).sum::<f64>() / denom,
    }
}
