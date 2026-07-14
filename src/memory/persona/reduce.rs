//! Distillation reduce step (doc 06 §6.5): fold [`SessionDigest`] observations
//! (and verbatim T0 directive evidence) into the seven facet flavoured trees.
//!
//! Each facet maps to one `TreeFactory::flavoured(scope, ask)` tree. A digest
//! contributes one leaf per facet it has observations for; the existing
//! seal/fold mechanic re-summarises through the facet's `ask` and recompiles the
//! root, so incremental re-distillation comes for free. The reduce step is
//! summariser-agnostic — the deterministic `ConcatSummariser` gives the offline
//! path, a real provider gives distilled prose.

use std::collections::BTreeMap;

use anyhow::Result;
use chrono::{DateTime, Utc};

use super::types::{DigestObservation, EvidenceTier, PersonaEvidence, PersonaFacet, SessionDigest};
use crate::memory::chunks::{chunk_id, upsert_chunks, Chunk, Metadata, SourceKind};
use crate::memory::config::MemoryConfig;
use crate::memory::tree::bucket_seal::LeafRef;
use crate::memory::tree::flavoured::compile_flavoured_root;
use crate::memory::tree::{Summariser, TreeFactory};

/// Per-facet natural-language asks, defaulting to [`PersonaFacet::default_ask`]
/// and overridable via config.
#[derive(Debug, Clone, Default)]
pub struct FacetAsks(pub BTreeMap<PersonaFacet, String>);

impl FacetAsks {
    /// The ask for `facet`, falling back to the built-in default.
    pub fn ask(&self, facet: PersonaFacet) -> String {
        self.0
            .get(&facet)
            .cloned()
            .unwrap_or_else(|| facet.default_ask().to_string())
    }
}

/// Running reduce state threaded across sessions: a monotonically increasing
/// leaf sequence and per-facet observation counts (for the pack's strength
/// annotations).
#[derive(Debug, Default)]
pub struct ReduceState {
    seq: u32,
    /// Observation counts per facet, for "seen in N observations" annotations.
    pub counts: BTreeMap<PersonaFacet, usize>,
    /// Distinct scopes (repos/projects) each facet was observed in.
    pub scopes: BTreeMap<PersonaFacet, std::collections::BTreeSet<String>>,
    /// Verbatim T0 directive rules (from instruction files), deduped in order.
    /// Kept out of the LLM fold so the compiled Directives section stays
    /// near-verbatim regardless of the summariser (§6.5).
    pub directives: Vec<String>,
}

impl ReduceState {
    fn next_seq(&mut self) -> u32 {
        let s = self.seq;
        self.seq += 1;
        s
    }

    fn record(&mut self, facet: PersonaFacet, n: usize, scope: Option<&str>) {
        *self.counts.entry(facet).or_default() += n;
        if let Some(sc) = scope {
            self.scopes.entry(facet).or_default().insert(sc.to_string());
        }
    }
}

/// Weight a leaf by its confidence tier so higher-tier evidence folds first and
/// survives budget pressure.
pub fn tier_score(tier: EvidenceTier) -> f32 {
    match tier {
        EvidenceTier::T0 => 1.0,
        EvidenceTier::T1 => 0.9,
        EvidenceTier::T2 => 0.7,
        EvidenceTier::T3 => 0.4,
    }
}

/// Fold one digest into the facet trees. Groups observations by facet and writes
/// one leaf per facet.
pub async fn fold_digest(
    config: &MemoryConfig,
    digest: &SessionDigest,
    asks: &FacetAsks,
    summariser: &dyn Summariser,
    state: &mut ReduceState,
) -> Result<()> {
    if digest.is_empty() {
        return Ok(());
    }
    let scope = digest.source.scope.clone();
    // Group by facet, preserving order.
    let mut by_facet: BTreeMap<PersonaFacet, Vec<&DigestObservation>> = BTreeMap::new();
    for obs in &digest.observations {
        by_facet.entry(obs.facet).or_default().push(obs);
    }
    for (facet, obs) in by_facet {
        let leaf_text = render_observations(&obs);
        // Highest tier present in this leaf drives its fold weight.
        let top_tier = obs.iter().map(|o| o.tier).max().unwrap_or(EvidenceTier::T3);
        fold_leaf(
            config,
            facet,
            &asks.ask(facet),
            &leaf_text,
            digest_timestamp(digest),
            tier_score(top_tier),
            summariser,
            state,
        )
        .await?;
        state.record(facet, obs.len(), scope.as_deref());
    }
    Ok(())
}

/// Collect verbatim T0 directive evidence (from instruction files). These are
/// *not* folded through the LLM — they flow into the pack near-verbatim so the
/// person's explicit rules survive exactly (§6.4/§6.5). Deduped in first-seen
/// order.
pub fn fold_directives(evidence: &[PersonaEvidence], state: &mut ReduceState) {
    for ev in evidence {
        let scope = ev.source.scope.clone();
        let scope_label = scope.as_deref().unwrap_or("global");
        let rule = format!("[{scope_label}] {}", ev.excerpt());
        if !state.directives.contains(&rule) {
            state.directives.push(rule);
        }
        state.record(PersonaFacet::Directives, 1, scope.as_deref());
    }
}

/// Render a facet's observations as a leaf body.
fn render_observations(obs: &[&DigestObservation]) -> String {
    obs.iter()
        .map(|o| {
            if o.quote.trim().is_empty() {
                format!("- {} [{}]", o.observation, o.tier.as_str())
            } else {
                format!(
                    "- {} (\"{}\") [{}]",
                    o.observation,
                    o.quote,
                    o.tier.as_str()
                )
            }
        })
        .collect::<Vec<_>>()
        .join("\n")
}

fn digest_timestamp(_digest: &SessionDigest) -> DateTime<Utc> {
    // Digests don't carry their own time; use now (folds are order-driven by
    // the pipeline's oldest-first backfill, not by this stamp).
    Utc::now()
}

/// Persist `leaf_text` as a chunk and append it to `facet`'s flavoured tree.
#[allow(clippy::too_many_arguments)]
async fn fold_leaf(
    config: &MemoryConfig,
    facet: PersonaFacet,
    ask: &str,
    leaf_text: &str,
    timestamp: DateTime<Utc>,
    score: f32,
    summariser: &dyn Summariser,
    state: &mut ReduceState,
) -> Result<()> {
    let scope = facet.tree_scope();
    let seq = state.next_seq();
    let chunk = persona_chunk(&scope, seq, leaf_text, timestamp);
    upsert_chunks(config, std::slice::from_ref(&chunk))?;

    let leaf = LeafRef {
        chunk_id: chunk.id.clone(),
        token_count: estimate_tokens(leaf_text),
        timestamp,
        content: leaf_text.to_string(),
        entities: Vec::new(),
        topics: Vec::new(),
        score,
    };
    let factory = TreeFactory::flavoured(scope, ask.to_string());
    factory.insert_leaf(config, &leaf, summariser).await?;
    Ok(())
}

/// Build a persona evidence chunk (stored as a `Document` source).
fn persona_chunk(scope: &str, seq: u32, content: &str, timestamp: DateTime<Utc>) -> Chunk {
    Chunk {
        id: chunk_id(SourceKind::Document, scope, seq, content),
        content: content.to_string(),
        metadata: Metadata {
            source_kind: SourceKind::Document,
            source_id: scope.to_string(),
            owner: "persona".to_string(),
            timestamp,
            time_range: (timestamp, timestamp),
            tags: vec!["persona".to_string()],
            source_ref: None,
            path_scope: None,
        },
        token_count: estimate_tokens(content),
        seq_in_source: seq,
        created_at: timestamp,
        partial_message: false,
    }
}

fn estimate_tokens(text: &str) -> u32 {
    ((text.len() / 4) as u32).max(1)
}

/// Seal every facet tree (force-flush the L0 buffer) and recompile its root.
/// Returns the compiled root body per facet (front-matter stripped).
pub async fn seal_and_collect(
    config: &MemoryConfig,
    asks: &FacetAsks,
    summariser: &dyn Summariser,
) -> Result<BTreeMap<PersonaFacet, String>> {
    let mut bodies = BTreeMap::new();
    for facet in PersonaFacet::ALL {
        let factory = TreeFactory::flavoured(facet.tree_scope(), asks.ask(facet));
        let tree = factory.get_or_create(config)?;
        // Only trees that received leaves have a non-empty buffer or root.
        factory.seal_now(config, summariser).await?;
        let markdown = compile_flavoured_root(config, &tree.id)?;
        let body = strip_frontmatter(&markdown);
        if !body.trim().is_empty() {
            bodies.insert(facet, body);
        }
    }
    Ok(bodies)
}

/// Strip a leading YAML front-matter block (`---\n … \n---\n`) from `md`.
pub fn strip_frontmatter(md: &str) -> String {
    let trimmed = md.trim_start();
    if let Some(rest) = trimmed.strip_prefix("---\n") {
        if let Some(end) = rest.find("\n---\n") {
            return rest[end + 5..].trim().to_string();
        }
        if let Some(end) = rest.find("\n---") {
            return rest[end + 4..].trim().to_string();
        }
    }
    md.trim().to_string()
}

#[cfg(test)]
#[path = "reduce_tests.rs"]
mod tests;
