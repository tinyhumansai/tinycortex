//! Entity extraction (Phase 2 / #708).
//!
//! Exposes [`EntityExtractor`] as a pluggable interface and a default
//! `CompositeExtractor` that runs a chain of extractors and merges their
//! output. The mechanical regex extractor is always available; semantic NER
//! (`LlmEntityExtractor`) plugs in behind the `ChatProvider` trait
//! without changing any call sites — and the crate never calls a real model.

#[path = "composite.rs"]
mod composite;
pub mod llm;
pub mod regex;
#[path = "types.rs"]
pub mod types;

pub use composite::{CompositeExtractor, EntityExtractor, RegexEntityExtractor};
pub use llm::{ChatPrompt, ChatProvider, LlmEntityExtractor, LlmExtractorConfig};
pub use types::{EntityKind, ExtractedEntities, ExtractedEntity, ExtractedTopic};
