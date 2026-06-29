//! Free-form relation extraction: graph-fact / owner / preference / review /
//! action-item regexes, plus the keyword-decision heuristics that run last.

use serde_json::Map;

use super::regex::{
    action_item_regex, classify_entity, explicit_owner_regex, explicit_preference_regex,
    graph_fact_regex, sanitize_entity_name, sanitize_fact_text, will_review_regex,
};
use super::text::normalize_graph_predicate;
use super::types::{ExtractionAccumulator, MemoryIngestionConfig};

/// Try the free-form relation regexes in priority order. Returns `true` when a
/// line is consumed; `false` falls through to keyword-decision handling.
pub(super) fn handle_relations(
    line: &str,
    chunk_index: usize,
    order_index: i64,
    acc: &mut ExtractionAccumulator,
    _config: &MemoryIngestionConfig,
) -> bool {
    if let Some(captures) = graph_fact_regex().captures(line) {
        let subject = captures.name("subject").map(|v| v.as_str()).unwrap_or("");
        let predicate = captures.name("predicate").map(|v| v.as_str()).unwrap_or("");
        let object = captures.name("object").map(|v| v.as_str()).unwrap_or("");
        let subject_type = classify_entity(subject, &acc.known_people);
        let object_type = classify_entity(object, &acc.known_people);
        acc.add_relation(
            subject,
            subject_type,
            predicate,
            object,
            object_type,
            0.87,
            chunk_index,
            order_index,
            Map::new(),
        );
        if normalize_graph_predicate(predicate) == "PREFERS" {
            acc.preferences.insert(format!(
                "{} prefers {}",
                sanitize_entity_name(subject),
                sanitize_fact_text(object)
            ));
            acc.tags.insert("preference".to_string());
            acc.doc_kind = Some("profile".to_string());
        }
        return true;
    }

    if let Some(captures) = explicit_owner_regex().captures(line) {
        let subject = captures.name("subject").map(|v| v.as_str()).unwrap_or("");
        let object = captures.name("object").map(|v| v.as_str()).unwrap_or("");
        let object_type = classify_entity(object, &acc.known_people);
        acc.add_relation(
            subject,
            "PERSON",
            "owns",
            object,
            object_type,
            0.94,
            chunk_index,
            order_index,
            Map::new(),
        );
        acc.tags.insert("owner".to_string());
        return true;
    }

    if let Some(captures) = will_review_regex().captures(line) {
        let subject = captures.name("subject").map(|v| v.as_str()).unwrap_or("");
        let object = captures.name("object").map(|v| v.as_str()).unwrap_or("");
        let object_type = classify_entity(object, &acc.known_people);
        acc.add_relation(
            subject,
            "PERSON",
            "reviews",
            object,
            object_type,
            0.80,
            chunk_index,
            order_index,
            Map::new(),
        );
        acc.tags.insert("owner".to_string());
        return true;
    }

    if let Some(captures) = explicit_preference_regex().captures(line) {
        let subject = captures.name("subject").map(|v| v.as_str()).unwrap_or("");
        let object = captures.name("object").map(|v| v.as_str()).unwrap_or("");
        let object_type = classify_entity(object, &acc.known_people);
        acc.add_relation(
            subject,
            "PERSON",
            "prefers",
            object,
            object_type,
            0.90,
            chunk_index,
            order_index,
            Map::new(),
        );
        acc.preferences.insert(format!(
            "{} prefers {}",
            sanitize_entity_name(subject),
            sanitize_fact_text(object)
        ));
        acc.tags.insert("preference".to_string());
        acc.doc_kind = Some("profile".to_string());
        return true;
    }

    if let Some(value) = line.strip_prefix("I prefer ") {
        if let Some(subject) = acc.current_sender.clone() {
            let preference = sanitize_fact_text(value);
            let preference_type = classify_entity(&preference, &acc.known_people);
            acc.add_relation(
                &subject,
                "PERSON",
                "prefers",
                &preference,
                preference_type,
                0.92,
                chunk_index,
                order_index,
                Map::new(),
            );
            acc.preferences
                .insert(format!("{subject} prefers {preference}"));
            acc.tags.insert("preference".to_string());
            acc.doc_kind = Some("profile".to_string());
            return true;
        }
    }

    if let Some(captures) = action_item_regex().captures(line) {
        let subject = captures.name("subject").map(|v| v.as_str()).unwrap_or("");
        let object = captures.name("object").map(|v| v.as_str()).unwrap_or("");
        if acc
            .known_people
            .contains_key(&sanitize_entity_name(subject))
            || classify_entity(subject, &acc.known_people) == "PERSON"
        {
            let object_type = classify_entity(object, &acc.known_people);
            acc.add_relation(
                subject,
                "PERSON",
                "owns",
                object,
                object_type,
                0.83,
                chunk_index,
                order_index,
                Map::new(),
            );
            acc.tags.insert("owner".to_string());
            return true;
        }
    }

    false
}

/// Keyword-decision heuristics for project-level decisions (JSON-RPC usage,
/// namespace storage key, user_id avoidance). Runs only when no earlier shape
/// matched; never returns — it is the tail of the ladder.
pub(super) fn handle_keyword_decisions(
    line: &str,
    chunk_index: usize,
    order_index: i64,
    acc: &mut ExtractionAccumulator,
) {
    let upper = sanitize_entity_name(line);
    let decision_subject = acc
        .primary_subject
        .clone()
        .or_else(|| acc.document_title.clone())
        .unwrap_or_else(|| "DOCUMENT".to_string());

    if upper.contains("JSON-RPC") {
        acc.add_relation(
            &decision_subject,
            "PROJECT",
            "uses",
            "JSON-RPC",
            "PRODUCT",
            0.86,
            chunk_index,
            order_index,
            Map::new(),
        );
        acc.decisions
            .insert(format!("{decision_subject} uses JSON-RPC"));
        acc.tags.insert("decision".to_string());
        return;
    }

    if upper.contains("SHOULD USE NAMESPACE")
        || upper.contains("USE NAMESPACE AS THE STORAGE")
        || upper.contains("NAMESPACE AS THE MAIN SCOPE KEY")
    {
        acc.add_relation(
            &decision_subject,
            "PROJECT",
            "uses",
            "namespace",
            "TOPIC",
            0.84,
            chunk_index,
            order_index,
            Map::new(),
        );
        acc.decisions
            .insert(format!("{decision_subject} uses namespace"));
        acc.tags.insert("decision".to_string());
        return;
    }

    if upper.contains("USER_ID") && (upper.contains("DO NOT NEED") || upper.contains("AVOID")) {
        acc.add_relation(
            &decision_subject,
            "PROJECT",
            "avoids",
            "user_id",
            "TOPIC",
            0.82,
            chunk_index,
            order_index,
            Map::new(),
        );
        acc.decisions
            .insert(format!("{decision_subject} avoids user_id"));
        acc.tags.insert("decision".to_string());
    }
}
