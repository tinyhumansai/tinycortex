//! Extraction-unit pass: recipient ("X gave Y to Z") and spatial
//! ("Kitchen is north of the Garden") relations, run over sentence/chunk units.

use serde_json::Map;

use super::chunking::build_units;
use super::regex::{classify_entity, recipient_regex, spatial_regex};
use super::types::{ExtractionAccumulator, MemoryIngestionConfig};

/// Process every extraction unit for recipient and spatial relations.
pub(super) fn process_units(
    chunks: &[String],
    acc: &mut ExtractionAccumulator,
    config: &MemoryIngestionConfig,
) {
    for unit in build_units(chunks, config.extraction_mode) {
        if let Some(captures) = recipient_regex().captures(&unit.text) {
            let giver = captures.name("giver").map(|v| v.as_str()).unwrap_or("");
            let object = captures.name("object").map(|v| v.as_str()).unwrap_or("");
            let recipient = captures.name("recipient").map(|v| v.as_str()).unwrap_or("");
            let giver_object_type = classify_entity(object, &acc.known_people);
            acc.add_relation(
                giver,
                "PERSON",
                "uses",
                object,
                giver_object_type,
                config.adjacency_threshold.max(0.62),
                unit.chunk_index,
                unit.order_index,
                Map::new(),
            );
            let recipient_object_type = classify_entity(object, &acc.known_people);
            acc.add_relation(
                recipient,
                "PERSON",
                "uses",
                object,
                recipient_object_type,
                (config.adjacency_threshold * 0.9).max(0.55),
                unit.chunk_index,
                unit.order_index,
                Map::new(),
            );
        }

        if let Some(captures) = spatial_regex().captures(&unit.text) {
            let head = captures.name("head").map(|v| v.as_str()).unwrap_or("");
            let direction = captures.name("direction").map(|v| v.as_str()).unwrap_or("");
            let tail = captures.name("tail").map(|v| v.as_str()).unwrap_or("");
            let inverse = match direction.to_ascii_lowercase().as_str() {
                "north" => "south_of",
                "south" => "north_of",
                "east" => "west_of",
                "west" => "east_of",
                _ => "",
            };
            let predicate = format!("{direction}_of");
            acc.add_relation(
                head,
                "ROOM",
                &predicate,
                tail,
                "ROOM",
                config.adjacency_threshold.max(0.70),
                unit.chunk_index,
                unit.order_index,
                Map::new(),
            );
            if !inverse.is_empty() {
                acc.add_relation(
                    tail,
                    "ROOM",
                    inverse,
                    head,
                    "ROOM",
                    config.adjacency_threshold.max(0.70),
                    unit.chunk_index,
                    unit.order_index,
                    Map::new(),
                );
            }
        }
    }
}
