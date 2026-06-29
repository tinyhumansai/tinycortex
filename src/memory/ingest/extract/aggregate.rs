//! Aggregation stage: alias-canonicalise entities, merge duplicate relations,
//! apply confidence thresholds, and assemble the final [`ParsedIngestion`].

use std::collections::BTreeMap;

use serde_json::{json, Value};

use super::alias::{build_alias_map, resolve_alias, reverse_aliases};
use super::types::{
    ExtractedEntity, ExtractedRelation, ExtractionAccumulator, MemoryIngestionConfig,
    ParsedIngestion, RawEntity, RawRelation,
};

/// Collapse the accumulator into the final parsed result for `chunk_count`
/// chunks, applying entity/relation thresholds from `config`.
pub(super) fn finalize(
    accumulator: ExtractionAccumulator,
    config: &MemoryIngestionConfig,
    chunk_count: usize,
) -> ParsedIngestion {
    let aliases = build_alias_map(&accumulator.entities);
    let reverse_alias = reverse_aliases(&aliases);
    let mut canonical_entities = BTreeMap::<String, RawEntity>::new();
    for entity in accumulator.entities.values() {
        let canonical = resolve_alias(&entity.name, &aliases);
        let entry = canonical_entities
            .entry(canonical.clone())
            .or_insert_with(|| RawEntity {
                name: canonical.clone(),
                entity_type: entity.entity_type.clone(),
                confidence: entity.confidence,
            });
        if entity.confidence > entry.confidence {
            entry.confidence = entity.confidence;
            entry.entity_type = entity.entity_type.clone();
        }
    }

    let mut aggregated_relations = BTreeMap::<(String, String, String), RawRelation>::new();
    for relation in accumulator.relations {
        let subject = resolve_alias(&relation.subject, &aliases);
        let object = resolve_alias(&relation.object, &aliases);
        if subject == object {
            continue;
        }
        let key = (subject.clone(), relation.predicate.clone(), object.clone());
        let entry = aggregated_relations
            .entry(key)
            .or_insert_with(|| RawRelation {
                subject,
                subject_type: relation.subject_type.clone(),
                predicate: relation.predicate.clone(),
                object,
                object_type: relation.object_type.clone(),
                confidence: relation.confidence,
                chunk_indexes: relation.chunk_indexes.clone(),
                order_index: relation.order_index,
                metadata: relation.metadata.clone(),
            });
        entry.confidence = entry.confidence.max(relation.confidence);
        entry.order_index = entry.order_index.min(relation.order_index);
        entry.chunk_indexes.extend(relation.chunk_indexes);
    }

    let entities = canonical_entities
        .into_values()
        .filter(|entity| entity.confidence >= config.entity_threshold)
        .map(|entity| ExtractedEntity {
            name: entity.name.clone(),
            entity_type: entity.entity_type,
            aliases: reverse_alias.get(&entity.name).cloned().unwrap_or_default(),
        })
        .collect::<Vec<_>>();

    let relations = aggregated_relations
        .into_values()
        .filter(|relation| relation.confidence >= config.relation_threshold)
        .map(|relation| ExtractedRelation {
            subject: relation.subject,
            subject_type: relation.subject_type,
            predicate: relation.predicate,
            object: relation.object,
            object_type: relation.object_type,
            confidence: relation.confidence,
            evidence_count: u32::try_from(relation.chunk_indexes.len()).unwrap_or(u32::MAX),
            chunk_ids: relation
                .chunk_indexes
                .iter()
                .map(|index| format!("chunk:{index}"))
                .collect::<Vec<_>>(),
            order_index: Some(relation.order_index),
            metadata: Value::Object(relation.metadata),
        })
        .collect::<Vec<_>>();

    let mut tags = accumulator.tags.into_iter().collect::<Vec<_>>();
    tags.sort();
    let metadata = json!({
        "kind": accumulator.doc_kind.or_else(|| {
            if !accumulator.preferences.is_empty() || !accumulator.decisions.is_empty() {
                Some("profile".to_string())
            } else {
                None
            }
        }),
        "primary_subject": accumulator.primary_subject,
        "decisions": accumulator.decisions.iter().cloned().collect::<Vec<_>>(),
        "preferences": accumulator.preferences.iter().cloned().collect::<Vec<_>>(),
        "extracted_entities": entities.iter().map(|entity| {
            json!({
                "name": entity.name,
                "entity_type": entity.entity_type,
                "aliases": entity.aliases,
            })
        }).collect::<Vec<_>>(),
    });

    ParsedIngestion {
        tags,
        metadata,
        entities,
        relations,
        chunk_count,
        preference_count: accumulator.preferences.len(),
        decision_count: accumulator.decisions.len(),
    }
}
