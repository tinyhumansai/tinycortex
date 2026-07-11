//! Scoring / admission / enrichment pipeline.
//!
//! Wraps extraction, signal computation, the admission gate, entity
//! canonicalisation, and persistence into one call per chunk. The ingest path
//! passes each chunk through [`score_chunk`] after chunking and before storing.
//!
//! ## Trait seams
//!
//! Two backends are abstracted so the crate never calls a real model:
//! - [`extract::EntityExtractor`] — the always-on regex extractor plus the
//!   optional LLM NER + **importance rater** consulted only on borderline
//!   chunks. The default config uses no LLM (a no-op importance signal of
//!   `0.0`); a host wires an [`extract::LlmEntityExtractor`] over its own
//!   [`extract::ChatProvider`].
//! - [`embed::Embedder`] — the embedding backend, defaulting to the
//!   deterministic [`embed::InertEmbedder`].
//!
//! ## Position in the layer diagram
//!
//! `score` sits between [`crate::memory::chunks`] (produces the [`Chunk`]s
//! it scores) and [`crate::memory::tree`]/[`crate::memory::queue`] (which
//! call [`score_chunk`]/[`score_chunks`] during ingest and act on
//! [`ScoreResult::kept`]). Per the crate-wide layering rule, `score` never
//! calls upward into `tree`, `queue`, or `retrieval`.
//!
//! ## Concurrency / atomicity
//!
//! [`score_chunk`] itself is pure-ish (no store access) and safe to call
//! concurrently for different chunks — the only shared state is whatever the
//! configured [`extract::EntityExtractor`] / LLM provider holds internally.
//! Persistence ([`persist_score`] / [`persist_score_tx`]) is where atomicity
//! matters: use the `_tx` variant when persisting a score alongside its chunk
//! write so both land in the same SQLite transaction (see
//! [`store`]'s module docs for the full write contract).

/// Embedding backends ([`embed::Embedder`]) and the deterministic
/// [`embed::InertEmbedder`] default.
pub mod embed;
/// Entity extraction: the always-on regex extractor plus the optional LLM
/// NER + importance rater seam ([`extract::EntityExtractor`]).
pub mod extract;
/// Entity canonicalisation — folds extracted surface forms into
/// [`resolver::CanonicalEntity`] records for stable indexing.
pub mod resolver;
/// Cheap signal computation and weighted combination
/// ([`signals::ScoreSignals`], [`signals::SignalWeights`]).
pub mod signals;
/// Persistence for score rows and the entity occurrence index.
pub mod store;

use std::sync::Arc;

use anyhow::Result;
use chrono::Utc;
use rusqlite::Transaction;
use serde::{Deserialize, Serialize};

use self::extract::{EntityExtractor, ExtractedEntities};
use self::resolver::{canonicalise, CanonicalEntity};
use self::signals::{ScoreSignals, SignalWeights};
use crate::memory::chunks::{approx_token_count, Chunk, SourceKind};
use crate::memory::config::MemoryConfig;

/// Default drop threshold. Chunks with `total < DEFAULT_DROP_THRESHOLD` are
/// tombstoned and never reach the chunk store.
pub const DEFAULT_DROP_THRESHOLD: f32 = 0.3;

/// If the deterministic (cheap-signals-only) total is at or above this, the
/// chunk is admitted without consulting the LLM extractor.
pub const DEFAULT_DEFINITE_KEEP: f32 = 0.85;

/// If the deterministic total is at or below this, the chunk is dropped without
/// consulting the LLM extractor. Catches obvious noise cheaply.
pub const DEFAULT_DEFINITE_DROP: f32 = 0.15;

/// Metadata tag marking a chunk as high-priority source material. The ingest
/// path stamps this on higher-signal content (e.g. GitHub commit messages and
/// closed/merged issues & PRs), which should be preferred when building the
/// memory tree.
pub const PRIORITY_TAG: &str = "priority_high";

/// Additive score nudge applied to chunks carrying [`PRIORITY_TAG`]. Pushes
/// them above the drop threshold and higher in retrieval / summary ordering
/// without fully overriding the content signals. Clamped to `1.0`.
pub const PRIORITY_BOOST: f32 = 0.25;

/// Whole outcome of [`score_chunk`].
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct ScoreResult {
    /// Id of the scored chunk; mirrors [`Chunk::id`].
    pub chunk_id: String,
    /// Final admission score in `[0.0, 1.0]` after weighting, priority boost,
    /// and clamping. Compared against `drop_threshold` by the gate.
    pub total: f32,
    /// The signal vector the `total` was combined from (kept for diagnostics
    /// and persisted in the score row).
    pub signals: ScoreSignals,
    /// Admission verdict — `true` if the chunk cleared the gate and should be
    /// persisted to the chunk store; `false` if tombstoned.
    pub kept: bool,
    /// Human-readable reason the chunk was dropped; `None` when `kept`.
    pub drop_reason: Option<String>,
    /// Merged extractor output (regex always, plus LLM NER when consulted).
    pub extracted: ExtractedEntities,
    /// Canonicalised entities for the occurrence index; computed
    /// unconditionally so it is inspectable even for dropped chunks.
    pub canonical_entities: Vec<CanonicalEntity>,
}

/// Configuration passed through the ingest pipeline for scoring behaviour.
///
/// Held as a struct so callers can override per-run without mutating global
/// config — useful for tests and explicit threshold tuning.
///
/// The `extractor` field always runs (typically a regex-based composite for
/// cheap mechanical entities). `llm_extractor` is consulted **only when the
/// cheap-signals total falls in the band** `(definite_drop_threshold,
/// definite_keep_threshold)` — chunks that are obviously trash or obviously
/// substantive don't pay the LLM cost.
pub struct ScoringConfig {
    /// Always-on extractor run on every chunk (typically a regex composite).
    pub extractor: Arc<dyn EntityExtractor>,
    /// Per-signal weights used when combining the signal vector into `total`.
    pub weights: SignalWeights,
    /// Final admission gate: chunks with `total < drop_threshold` are dropped.
    pub drop_threshold: f32,
    /// Optional second-pass extractor whose output is **merged** into the regex
    /// output before the final combine. Designed for an LLM-based NER +
    /// importance signal (see [`extract::LlmEntityExtractor`]). `None` means LLM
    /// augmentation is disabled.
    pub llm_extractor: Option<Arc<dyn EntityExtractor>>,
    /// Cheap-signals total ≥ this → admit without consulting LLM.
    pub definite_keep_threshold: f32,
    /// Cheap-signals total ≤ this → drop without consulting LLM.
    pub definite_drop_threshold: f32,
}

impl ScoringConfig {
    /// Default: regex-only extractor, default weights, default threshold.
    pub fn default_regex_only() -> Self {
        Self {
            extractor: Arc::new(extract::CompositeExtractor::regex_only()),
            weights: SignalWeights::default(),
            drop_threshold: DEFAULT_DROP_THRESHOLD,
            llm_extractor: None,
            definite_keep_threshold: DEFAULT_DEFINITE_KEEP,
            definite_drop_threshold: DEFAULT_DEFINITE_DROP,
        }
    }

    /// Convenience constructor: regex always + LLM extractor on borderline
    /// chunks. The `llm_importance` weight is enabled in [`SignalWeights`] so
    /// the LLM signal actually influences the final total.
    pub fn with_llm_extractor(llm: Arc<dyn EntityExtractor>) -> Self {
        Self {
            extractor: Arc::new(extract::CompositeExtractor::regex_only()),
            weights: SignalWeights::with_llm_enabled(),
            drop_threshold: DEFAULT_DROP_THRESHOLD,
            llm_extractor: Some(llm),
            definite_keep_threshold: DEFAULT_DEFINITE_KEEP,
            definite_drop_threshold: DEFAULT_DEFINITE_DROP,
        }
    }
}

/// Compute the score for one chunk.
///
/// Pure-ish function — does not touch the store (only the extractor may have
/// side effects). Callers decide what to persist based on [`ScoreResult::kept`].
///
/// Pipeline:
/// 1. Run the always-on extractor (typically regex).
/// 2. Compute cheap signals; combine **excluding** `llm_importance` weight.
/// 3. Short-circuit:
///    - If cheap total ≥ `definite_keep_threshold`: admit without LLM.
///    - If cheap total ≤ `definite_drop_threshold`: drop without LLM.
///    - Else: borderline — run the LLM extractor (if configured), merge its
///      output, recompute signals, recombine with full weights.
/// 4. Apply the final admission gate against `drop_threshold`.
pub async fn score_chunk(chunk: &Chunk, cfg: &ScoringConfig) -> Result<ScoreResult> {
    let scoring_content = scoring_content_for_chunk(chunk);
    let scoring_token_count = approx_token_count(&scoring_content);

    // 1. Always-on extraction (regex / mechanical).
    let mut extracted = cfg.extractor.extract(&scoring_content).await?;

    // 2. Compute cheap signals + combine excluding LLM importance.
    let mut signals = self::signals::compute(
        &chunk.metadata,
        &scoring_content,
        scoring_token_count,
        &extracted,
    );
    let cheap_total = self::signals::combine_cheap_only(&signals, &cfg.weights);

    // 3. Short-circuit decision.
    let in_band =
        cheap_total > cfg.definite_drop_threshold && cheap_total < cfg.definite_keep_threshold;
    let llm_consulted = if in_band {
        if let Some(llm) = cfg.llm_extractor.as_ref() {
            match llm.extract(&scoring_content).await {
                Ok(more) => {
                    extracted.merge(more);
                    // Recompute signals so llm_importance flows in.
                    signals = self::signals::compute(
                        &chunk.metadata,
                        &scoring_content,
                        scoring_token_count,
                        &extracted,
                    );
                    // `LlmEntityExtractor::extract` never returns `Err`: every
                    // failure path (transport, HTTP status, malformed JSON)
                    // soft-falls back to `Ok(ExtractedEntities::default())`
                    // with `llm_importance: None`. So an `Ok` alone does NOT
                    // mean an importance signal was produced. Only treat the
                    // LLM as "consulted" — and thus switch to the full,
                    // importance-weighted combine — when an importance value is
                    // actually present. Otherwise a `0.0` importance would flow
                    // through the `llm_importance` weight and drag borderline
                    // chunks below the drop threshold, silently discarding them.
                    extracted.llm_importance.is_some()
                }
                Err(_e) => {
                    // LLM extractor failed — fall back to cheap signals only.
                    false
                }
            }
        } else {
            false
        }
    } else {
        false
    };

    // 4. Final weighted combine.
    //
    // If the LLM ran AND produced an importance signal, `llm_consulted` is
    // `true` → use the full `combine` which includes the `llm_importance`
    // weight.
    //
    // If the LLM was skipped (short-circuited or not configured), failed, or
    // soft-fell back to an empty extraction with no importance, using the full
    // combine would pin `llm_importance * w.llm_importance = 0 * 2.0` into the
    // numerator while still dividing by the full denominator — artificially
    // dragging the total down. Fall back to `combine_cheap_only` which excludes
    // that term from both numerator and denominator.
    let mut total = if llm_consulted {
        self::signals::combine(&signals, &cfg.weights)
    } else {
        self::signals::combine_cheap_only(&signals, &cfg.weights)
    };

    // 4b. Priority boost. Chunks tagged PRIORITY_TAG at ingest are higher-signal
    // than ambient activity. Nudge their total up (clamped) so they clear the
    // admission gate more readily and rank higher when the tree is built.
    let priority = chunk.metadata.tags.iter().any(|t| t == PRIORITY_TAG);
    if priority {
        let boosted = (total + PRIORITY_BOOST).min(1.0);
        if boosted > total {
            total = boosted;
        }
    }

    // 5. Admission gate. Source and interaction priors are deliberately
    // non-zero, so guard against very short entity-free chatter being kept by
    // metadata alone. Priority-tagged chunks bypass the tiny/entity-free drop
    // since the tag itself is a strong relevance signal.
    let tiny_entity_free = !priority
        && scoring_token_count < self::signals::token_count::TOKEN_MIN
        && extracted.is_empty();
    let kept = !tiny_entity_free && total >= cfg.drop_threshold;
    let drop_reason = if kept {
        None
    } else if tiny_entity_free {
        Some(format!(
            "token_count {} < minimum {} and no entities extracted",
            scoring_token_count,
            self::signals::token_count::TOKEN_MIN
        ))
    } else {
        Some(format!(
            "total {total:.3} < threshold {:.3}",
            cfg.drop_threshold
        ))
    };

    // 6. Canonicalise for indexing. We canonicalise unconditionally so the
    //    result is inspectable in tests even when the chunk is dropped.
    let canonical_entities = canonicalise(&extracted);

    if !kept {}

    Ok(ScoreResult {
        chunk_id: chunk.id.clone(),
        total,
        signals,
        kept,
        drop_reason,
        extracted,
        canonical_entities,
    })
}

fn scoring_content_for_chunk(chunk: &Chunk) -> String {
    if chunk.metadata.source_kind != SourceKind::Chat {
        return chunk.content.clone();
    }

    chunk
        .content
        .lines()
        .filter(|line| {
            let trimmed = line.trim_start();
            !trimmed.starts_with("# Chat transcript") && !trimmed.starts_with("## ")
        })
        .collect::<Vec<_>>()
        .join("\n")
}

/// Score a batch of chunks. Errors from any single chunk fail the batch —
/// scoring is pure-ish (only the extractor may error) and a failure here is a
/// real bug, not a per-chunk issue to tolerate silently.
pub async fn score_chunks(chunks: &[Chunk], cfg: &ScoringConfig) -> Result<Vec<ScoreResult>> {
    let mut out = Vec::with_capacity(chunks.len());
    for chunk in chunks {
        out.push(score_chunk(chunk, cfg).await?);
    }
    Ok(out)
}

/// Cheap-only batch scoring path used by the async queue ingest pipeline.
///
/// Preserves the same thresholds and admission gate as [`score_chunks`] but
/// guarantees no LLM extractor is consulted on the ingest hot path.
pub async fn score_chunks_fast(chunks: &[Chunk], cfg: &ScoringConfig) -> Result<Vec<ScoreResult>> {
    let fast_cfg = ScoringConfig {
        extractor: cfg.extractor.clone(),
        weights: cfg.weights.clone(),
        drop_threshold: cfg.drop_threshold,
        llm_extractor: None,
        definite_keep_threshold: cfg.definite_keep_threshold,
        definite_drop_threshold: cfg.definite_drop_threshold,
    };
    score_chunks(chunks, &fast_cfg).await
}

// ── Persistence helpers used by the ingest orchestrator ─────────────────

/// Persist the score row + entity-index rows for one kept chunk.
///
/// The caller is responsible for having already written the chunk itself into
/// `mem_tree_chunks`. Dropped chunks still get a score row persisted for
/// diagnostics — callers should pass `None` for `tree_id` in that case.
///
/// Co-occurrence graph edges (OpenHuman's E2GraphRAG accumulation) are not
/// written here: TinyCortex's `memory::graph` module is not yet ported, so this
/// helper persists the score row and the entity index only.
///
/// NOTE (RS-13): the entity index is only cleared/re-indexed when
/// `result.kept` is `true`. If a chunk is scored a second time (e.g. a
/// re-score after content edit) and flips from kept to dropped, this function
/// takes the `!result.kept` branch and never calls
/// [`store::clear_entity_index_for_node`] — the entity-index rows from the
/// earlier, kept scoring are left behind as phantom rows pointing at a
/// now-dropped chunk. Callers that need drop-time cleanup must clear the
/// entity index themselves before calling this function.
pub fn persist_score(
    config: &MemoryConfig,
    result: &ScoreResult,
    timestamp_ms: i64,
    tree_id: Option<&str>,
) -> Result<()> {
    let row = score_row(result);
    store::upsert_score(config, &row)?;

    if result.kept {
        // Clear any stale entity-index rows for this chunk before re-indexing.
        // INSERT OR REPLACE never deletes rows whose entity_id is no longer
        // present in the new extraction — so a re-score that drops an entity
        // would otherwise leave a phantom index row.
        store::clear_entity_index_for_node(config, &result.chunk_id)?;
        if !result.canonical_entities.is_empty() {
            store::index_entities(
                config,
                &result.canonical_entities,
                &result.chunk_id,
                "leaf",
                timestamp_ms,
                tree_id,
            )?;
        }
    }

    Ok(())
}

/// Transactional variant of [`persist_score`] — writes the score row and
/// entity-index rows on the caller's open [`Transaction`] so the persistence
/// is atomic with the surrounding ingest write. Same kept/clear-before-reindex
/// semantics as [`persist_score`], including the same phantom-row gotcha on a
/// kept→dropped re-score (see the RS-13 note there).
pub fn persist_score_tx(
    tx: &Transaction<'_>,
    result: &ScoreResult,
    timestamp_ms: i64,
    tree_id: Option<&str>,
) -> Result<()> {
    let row = score_row(result);
    store::upsert_score_tx(tx, &row)?;

    if result.kept {
        // See persist_score for why we clear before re-indexing.
        store::clear_entity_index_for_node_tx(tx, &result.chunk_id)?;
        if !result.canonical_entities.is_empty() {
            store::index_entities_tx(
                tx,
                &result.canonical_entities,
                &result.chunk_id,
                "leaf",
                timestamp_ms,
                tree_id,
            )?;
        }
    }

    Ok(())
}

fn score_row(result: &ScoreResult) -> store::ScoreRow {
    // Score rows keep wall-clock scoring time; the separate timestamp_ms
    // argument used for entity indexes is the source/ingest ordering time.
    store::ScoreRow {
        chunk_id: result.chunk_id.clone(),
        total: result.total,
        signals: result.signals.clone(),
        dropped: !result.kept,
        reason: result.drop_reason.clone(),
        computed_at_ms: Utc::now().timestamp_millis(),
        llm_importance_reason: result.extracted.llm_importance_reason.clone(),
    }
}

#[cfg(test)]
#[path = "mod_tests.rs"]
mod tests;
