//! Algorithmic (no-LLM) retrieval over the persona memory layer (doc 06).
//!
//! The persona pipeline folds distilled [`DigestObservation`](super::types::DigestObservation)s into per-facet
//! flavoured trees, but it *also* persists every observation leaf verbatim as a
//! `Document` chunk (`owner = "persona"`, `source_id = "persona/<facet>"`) — see
//! [`reduce::fold_leaf`](super::reduce). Those leaves are the richest, most
//! granular queryable surface: one prescriptive rule per line, each carrying its
//! own confidence tier and provenance, all content-addressed and dedup-safe.
//!
//! This module loads those leaves and ranks them against a natural-language
//! query using a **purely lexical BM25 scorer weighted by evidence tier** — no
//! model call, no network, fully deterministic. It is the "retrieval" half of
//! the persona decision agent: an LLM final pass (the agent harness) filters and
//! synthesises the candidates this stage surfaces, but never chooses them.
//!
//! ## Why BM25 over embeddings
//!
//! The persona pipeline seals its trees with `embedder: None`, so no
//! per-observation vectors exist on disk. Rather than pay an embedding call at
//! query time (which would make retrieval itself model-dependent), the retriever
//! stays lexical: BM25 over the observation text, scaled by the reduce step's own
//! [`tier_score`](super::reduce::tier_score) weighting so a written-down rule (T0)
//! outranks an inferred outcome (T3) at equal lexical relevance.

use std::collections::HashMap;

use anyhow::Result;
use chrono::{DateTime, Utc};

use super::types::{EvidenceTier, PersonaFacet};
use crate::memory::chunks::{list_chunks, ListChunksQuery, SourceKind};
use crate::memory::config::MemoryConfig;

/// BM25 term-frequency saturation. Standard default.
const BM25_K1: f32 = 1.5;
/// BM25 length-normalisation strength. Standard default.
const BM25_B: f32 = 0.75;

/// Owner tag every persona leaf chunk is written under (see `reduce::persona_chunk`).
const PERSONA_OWNER: &str = "persona";

/// One retrieved persona observation, with everything the agent needs to cite it.
#[derive(Debug, Clone, PartialEq)]
pub struct PersonaHit {
    /// Which facet this observation informs.
    pub facet: PersonaFacet,
    /// Confidence tier parsed from the leaf's `[tN]` annotation.
    pub tier: EvidenceTier,
    /// The prescriptive observation text (tier tag stripped, quote inlined).
    pub text: String,
    /// Short supporting quote, when the observation carried one.
    pub quote: Option<String>,
    /// When the underlying evidence was folded.
    pub timestamp: DateTime<Utc>,
    /// Final rank score: `bm25 * tier_weight` (higher is better).
    pub score: f32,
}

/// A single indexed observation document (one `- …` leaf line).
#[derive(Debug, Clone)]
struct ObsDoc {
    facet: PersonaFacet,
    tier: EvidenceTier,
    text: String,
    quote: Option<String>,
    timestamp: DateTime<Utc>,
    /// Per-term frequency in this document's tokenised body.
    term_freqs: HashMap<String, u32>,
    /// Total token count (document length) for BM25 normalisation.
    len: u32,
}

/// A loaded, in-memory BM25 index over the persona observation corpus.
///
/// Construct with [`PersonaRetriever::load`]; query with
/// [`PersonaRetriever::search`]. Loading reads the chunk store once; searching is
/// pure CPU work over the in-memory index.
#[derive(Debug, Clone)]
pub struct PersonaRetriever {
    docs: Vec<ObsDoc>,
    /// Document frequency per term (how many docs contain it).
    doc_freq: HashMap<String, u32>,
    /// Mean document length across the corpus.
    avg_len: f32,
}

impl PersonaRetriever {
    /// Load every persona observation leaf from `config`'s chunk store and build
    /// the BM25 index. Reads all seven facet trees.
    pub fn load(config: &MemoryConfig) -> Result<Self> {
        let mut docs = Vec::new();
        for facet in PersonaFacet::ALL {
            let query = ListChunksQuery {
                source_kind: Some(SourceKind::Document),
                source_id: Some(facet.tree_scope()),
                owner: Some(PERSONA_OWNER.to_string()),
                limit: Some(10_000),
                exclude_dropped: true,
                ..Default::default()
            };
            for chunk in list_chunks(config, &query)? {
                for line in chunk.content.lines() {
                    if let Some(doc) = parse_observation(facet, line, chunk.metadata.timestamp) {
                        docs.push(doc);
                    }
                }
            }
        }
        Ok(Self::from_docs(docs))
    }

    /// Build an index directly from parsed docs (shared by `load` and tests).
    fn from_docs(docs: Vec<ObsDoc>) -> Self {
        let mut doc_freq: HashMap<String, u32> = HashMap::new();
        let mut total_len: u64 = 0;
        for doc in &docs {
            total_len += doc.len as u64;
            for term in doc.term_freqs.keys() {
                *doc_freq.entry(term.clone()).or_default() += 1;
            }
        }
        let avg_len = if docs.is_empty() {
            0.0
        } else {
            total_len as f32 / docs.len() as f32
        };
        Self {
            docs,
            doc_freq,
            avg_len,
        }
    }

    /// Total number of indexed observations.
    pub fn len(&self) -> usize {
        self.docs.len()
    }

    /// True when no observations were loaded.
    pub fn is_empty(&self) -> bool {
        self.docs.is_empty()
    }

    /// Observation counts per facet, for overview/strength reporting.
    pub fn facet_counts(&self) -> HashMap<PersonaFacet, usize> {
        let mut counts = HashMap::new();
        for doc in &self.docs {
            *counts.entry(doc.facet).or_default() += 1;
        }
        counts
    }

    /// Rank the corpus against `query`, returning the top `k` hits.
    ///
    /// When `facet` is `Some`, only that facet's observations are considered.
    /// Scoring is `bm25(query, doc) * tier_weight(doc.tier)`; ties break toward
    /// the more recent observation. Returns fewer than `k` hits when the corpus
    /// is smaller or nothing matches.
    pub fn search(&self, query: &str, facet: Option<PersonaFacet>, k: usize) -> Vec<PersonaHit> {
        let q_terms = tokenize(query);
        if q_terms.is_empty() || self.docs.is_empty() || k == 0 {
            return Vec::new();
        }
        let n = self.docs.len() as f32;

        let mut scored: Vec<(f32, &ObsDoc)> = self
            .docs
            .iter()
            .filter(|d| facet.map_or(true, |f| d.facet == f))
            .filter_map(|doc| {
                let bm25 = self.bm25(&q_terms, doc, n);
                if bm25 <= 0.0 {
                    return None;
                }
                Some((bm25 * tier_weight(doc.tier), doc))
            })
            .collect();

        scored.sort_by(|a, b| {
            b.0.partial_cmp(&a.0)
                .unwrap_or(std::cmp::Ordering::Equal)
                .then(b.1.timestamp.cmp(&a.1.timestamp))
        });

        scored
            .into_iter()
            .take(k)
            .map(|(score, doc)| PersonaHit {
                facet: doc.facet,
                tier: doc.tier,
                text: doc.text.clone(),
                quote: doc.quote.clone(),
                timestamp: doc.timestamp,
                score,
            })
            .collect()
    }

    /// BM25 score of `doc` for the (deduplicated) query terms.
    fn bm25(&self, q_terms: &[String], doc: &ObsDoc, n: f32) -> f32 {
        let dl = doc.len as f32;
        let mut score = 0.0;
        for term in q_terms {
            let tf = match doc.term_freqs.get(term) {
                Some(&f) => f as f32,
                None => continue,
            };
            let df = self.doc_freq.get(term).copied().unwrap_or(0) as f32;
            // Robertson–Sparck-Jones idf with the +1 shift that keeps it
            // non-negative even for terms in more than half the corpus.
            let idf = ((n - df + 0.5) / (df + 0.5) + 1.0).ln();
            let denom = tf + BM25_K1 * (1.0 - BM25_B + BM25_B * dl / self.avg_len.max(1.0));
            score += idf * (tf * (BM25_K1 + 1.0)) / denom;
        }
        score
    }
}

/// Evidence-tier weight, mirroring [`reduce::tier_score`](super::reduce::tier_score):
/// a written-down rule outranks an inferred outcome at equal lexical relevance.
fn tier_weight(tier: EvidenceTier) -> f32 {
    match tier {
        EvidenceTier::T0 => 1.0,
        EvidenceTier::T1 => 0.9,
        EvidenceTier::T2 => 0.7,
        EvidenceTier::T3 => 0.4,
    }
}

/// Parse one rendered leaf line (`- <obs> ("<quote>") [tN]`) into an [`ObsDoc`].
///
/// Returns `None` for lines that are not observation bullets. Mirrors the render
/// format in [`reduce::render_observations`](super::reduce); tolerant of a
/// missing quote or a missing/garbled tier tag (defaults to `T3`).
fn parse_observation(facet: PersonaFacet, line: &str, timestamp: DateTime<Utc>) -> Option<ObsDoc> {
    let body = line.trim().strip_prefix("- ")?.trim();
    if body.is_empty() {
        return None;
    }

    // Split off a trailing `[tN]` tier tag, if present.
    let (mut text, tier) = match (body.rfind('['), body.ends_with(']')) {
        (Some(open), true) => {
            let tag = &body[open + 1..body.len() - 1];
            match EvidenceTier::parse_loose(tag) {
                Some(t) => (body[..open].trim().to_string(), t),
                None => (body.to_string(), EvidenceTier::T3),
            }
        }
        _ => (body.to_string(), EvidenceTier::T3),
    };

    // Extract an inline `("<quote>")` suffix, if present.
    let quote = extract_quote(&text).map(|(clean, q)| {
        text = clean;
        q
    });

    let searchable = match &quote {
        Some(q) => format!("{text} {q}"),
        None => text.clone(),
    };
    let (term_freqs, len) = term_frequencies(&searchable);
    if len == 0 {
        return None;
    }

    Some(ObsDoc {
        facet,
        tier,
        text,
        quote,
        timestamp,
        term_freqs,
        len,
    })
}

/// Split a trailing `("<quote>")` off an observation, returning the cleaned
/// observation text and the quote body.
fn extract_quote(text: &str) -> Option<(String, String)> {
    let trimmed = text.trim_end();
    let inner = trimmed.strip_suffix("\")")?;
    let open = inner.rfind("(\"")?;
    let quote = inner[open + 2..].to_string();
    let clean = inner[..open].trim().to_string();
    Some((clean, quote))
}

/// Tokenise into a per-term frequency map, returning `(freqs, total_tokens)`.
fn term_frequencies(text: &str) -> (HashMap<String, u32>, u32) {
    let mut freqs: HashMap<String, u32> = HashMap::new();
    let mut total = 0u32;
    for term in tokenize(text) {
        *freqs.entry(term).or_default() += 1;
        total += 1;
    }
    (freqs, total)
}

/// Lowercase, split on non-alphanumeric boundaries (keeping `_` inside
/// identifiers), drop 1-char tokens and a small English stoplist.
fn tokenize(text: &str) -> Vec<String> {
    let mut out = Vec::new();
    let mut cur = String::new();
    for ch in text.chars() {
        if ch.is_alphanumeric() || ch == '_' {
            cur.extend(ch.to_lowercase());
        } else if !cur.is_empty() {
            push_token(&mut out, std::mem::take(&mut cur));
        }
    }
    if !cur.is_empty() {
        push_token(&mut out, cur);
    }
    out
}

fn push_token(out: &mut Vec<String>, tok: String) {
    if tok.len() >= 2 && !is_stopword(&tok) {
        out.push(tok);
    }
}

/// A deliberately small stoplist — enough to keep BM25 idf meaningful on short
/// prescriptive observations without discarding domain words.
fn is_stopword(tok: &str) -> bool {
    matches!(
        tok,
        "the"
            | "and"
            | "for"
            | "are"
            | "but"
            | "not"
            | "you"
            | "all"
            | "any"
            | "can"
            | "her"
            | "was"
            | "one"
            | "our"
            | "out"
            | "use"
            | "with"
            | "this"
            | "that"
            | "they"
            | "them"
            | "when"
            | "what"
            | "your"
            | "from"
            | "have"
            | "has"
            | "should"
            | "would"
            | "will"
            | "into"
            | "than"
            | "then"
            | "their"
            | "there"
            | "which"
            | "while"
            | "who"
            | "whom"
    )
}

#[cfg(test)]
#[path = "retrieve_tests.rs"]
mod tests;
