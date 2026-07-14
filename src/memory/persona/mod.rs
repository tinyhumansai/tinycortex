//! Persona distillation (doc 06): turns a person's local coding-agent history,
//! agent instruction files, and git commit history into a durable **persona
//! memory layer** — personality, communication style, coding style, and
//! tool/stack preferences compiled into a small, prompt-ready context pack.
//!
//! The surface is organised around the canonical evidence model in [`types`]:
//! readers ([`readers`]) emit redacted [`PersonaEvidence`], the map step
//! ([`distill`]) turns batches of evidence into [`SessionDigest`]s via a
//! [`ChatProvider`](crate::memory::score::extract::ChatProvider), the reduce
//! step folds digests into seven facet flavoured trees, and the [`compile`]
//! step assembles `persona/PERSONA.md`. [`state`] makes runs incremental and
//! resumable; [`pipeline`] wires it all together for the CLI harness.
//!
//! Everything is local-first and depends only on the crate's `ChatProvider` /
//! `Summariser` / `EmbeddingBackend` trait seams — nothing here names a
//! concrete provider (the OpenRouter reference provider lives under
//! `memory::providers`).

pub mod compile;
pub mod config;
pub mod distill;
pub mod pipeline;
pub mod readers;
pub mod reduce;
pub mod state;
pub mod types;

pub use config::PersonaConfig;
pub use pipeline::{Pipeline, RunMode, RunReport};

pub use types::{
    evidence_id, DigestObservation, EvidenceSource, EvidenceTier, PersonaEvidence, PersonaFacet,
    PersonaSourceKind, SessionDigest,
};
