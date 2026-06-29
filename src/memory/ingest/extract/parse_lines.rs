//! Per-line structured extraction: the line loop, heading/email-header
//! handling, and the prefixed-field ladder (`Subject:`, `Owner:`, …).
//!
//! Free-form relation regexes and keyword-decision heuristics live in the
//! sibling [`super::parse_relations`] module; this file handles the
//! deterministic, prefix-anchored shapes first.

use serde_json::Map;

use super::chunking::find_chunk_index;
use super::header::{detect_primary_subject, extract_people_from_header};
use super::regex::{email_header_regex, sanitize_entity_name, sanitize_fact_text};
use super::types::{ExtractionAccumulator, MemoryIngestionConfig};

/// Walk `content` line by line, attributing each non-empty line to its chunk
/// and dispatching it through the structured / relation / keyword ladders.
pub(super) fn process_content_lines(
    content: &str,
    chunks: &[String],
    acc: &mut ExtractionAccumulator,
    config: &MemoryIngestionConfig,
) {
    let mut chunk_hint = 0_usize;
    for raw_line in content.lines() {
        let line = sanitize_fact_text(raw_line);
        if line.is_empty() {
            continue;
        }

        let chunk_index = find_chunk_index(chunks, &line, chunk_hint);
        chunk_hint = chunk_index;
        let order_index = i64::try_from(chunk_index).unwrap_or(i64::MAX);

        if handle_heading_and_headers(raw_line, &line, chunk_index, order_index, acc) {
            continue;
        }
        if handle_prefixed_fields(&line, chunk_index, order_index, acc) {
            continue;
        }
        if super::parse_relations::handle_relations(&line, chunk_index, order_index, acc, config) {
            continue;
        }
        super::parse_relations::handle_keyword_decisions(&line, chunk_index, order_index, acc);
    }
}

/// Markdown headings (`# …`) set the current section subject; email headers
/// (`From:` / `To:` / `Cc:`) seed people and `communicates_with` edges.
fn handle_heading_and_headers(
    raw_line: &str,
    line: &str,
    chunk_index: usize,
    order_index: i64,
    acc: &mut ExtractionAccumulator,
) -> bool {
    if raw_line.trim_start().starts_with('#') {
        let heading = sanitize_entity_name(raw_line.trim_start_matches('#'));
        if !heading.is_empty() {
            if acc.document_title.is_none() {
                acc.document_title = Some(heading.clone());
            }
            acc.current_subject = Some(heading);
        }
        return true;
    }

    if let Some(captures) = email_header_regex().captures(line) {
        let header_name = captures
            .get(1)
            .map(|value| value.as_str())
            .unwrap_or_default()
            .to_ascii_uppercase();
        let value = captures
            .name("value")
            .map(|value| value.as_str())
            .unwrap_or("");
        let people = extract_people_from_header(value, acc);
        if header_name == "FROM" {
            acc.current_sender = people.first().cloned();
        } else if header_name == "TO" || header_name == "CC" {
            if let Some(sender) = acc.current_sender.clone() {
                for recipient in &people {
                    acc.add_relation(
                        &sender,
                        "PERSON",
                        "communicates_with",
                        recipient,
                        "PERSON",
                        0.82,
                        chunk_index,
                        order_index,
                        Map::new(),
                    );
                }
            }
        }
        return true;
    }

    false
}

/// The prefix-anchored field ladder. Returns `true` once a prefix matches.
fn handle_prefixed_fields(
    line: &str,
    chunk_index: usize,
    order_index: i64,
    acc: &mut ExtractionAccumulator,
) -> bool {
    if let Some(subject) = line.strip_prefix("Subject:") {
        let subject_text = sanitize_fact_text(subject);
        if let Some(primary_subject) = detect_primary_subject(&subject_text) {
            acc.primary_subject = Some(primary_subject);
        }
        return true;
    }

    if let Some(date_text) = line.strip_prefix("Date:") {
        let date_text = sanitize_fact_text(date_text);
        if let Some(sender) = acc.current_sender.clone() {
            acc.add_relation(
                &sender,
                "PERSON",
                "has_deadline",
                &date_text,
                "DATE",
                0.75,
                chunk_index,
                order_index,
                Map::new(),
            );
        }
        return true;
    }

    if let Some(value) = line.strip_prefix("Project name:") {
        let project = sanitize_entity_name(value);
        if !project.is_empty() {
            acc.primary_subject = Some(project.clone());
            let _ = acc.add_entity(&project, "PROJECT", 0.96);
        }
        return true;
    }

    if let Some(value) = line.strip_prefix("Subproject:") {
        let subproject = sanitize_entity_name(value);
        if !subproject.is_empty() {
            let _ = acc.add_entity(&subproject, "PROJECT", 0.92);
        }
        return true;
    }

    if let Some(value) = line.strip_prefix("Owner:") {
        let owner = sanitize_entity_name(value);
        let owned = acc
            .current_subject
            .clone()
            .or_else(|| acc.primary_subject.clone())
            .or_else(|| acc.document_title.clone())
            .unwrap_or_else(|| "DOCUMENT".to_string());
        acc.add_relation(
            &owner,
            "PERSON",
            "owns",
            &owned,
            "WORK_ITEM",
            0.94,
            chunk_index,
            order_index,
            Map::new(),
        );
        return true;
    }

    if let Some(value) = line.strip_prefix("Name:") {
        let name = sanitize_entity_name(value);
        if !name.is_empty() {
            acc.current_subject = Some(name.clone());
            let _ = acc.add_entity(&name, "WORK_ITEM", 0.93);
        }
        return true;
    }

    if let Some(value) = line.strip_prefix("Due date:") {
        let due_date = sanitize_fact_text(value);
        let subject = acc
            .current_subject
            .clone()
            .or_else(|| acc.primary_subject.clone())
            .unwrap_or_else(|| "DOCUMENT".to_string());
        acc.add_relation(
            &subject,
            "WORK_ITEM",
            "has_deadline",
            &due_date,
            "DATE",
            0.92,
            chunk_index,
            order_index,
            Map::new(),
        );
        acc.tags.insert("deadline".to_string());
        return true;
    }

    if let Some(value) = line.strip_prefix("Target milestone:") {
        let due_date = sanitize_fact_text(value);
        let subject = acc
            .primary_subject
            .clone()
            .or_else(|| acc.document_title.clone())
            .unwrap_or_else(|| "DOCUMENT".to_string());
        acc.add_relation(
            &subject,
            "PROJECT",
            "has_deadline",
            &due_date,
            "DATE",
            0.92,
            chunk_index,
            order_index,
            Map::new(),
        );
        acc.tags.insert("deadline".to_string());
        return true;
    }

    if let Some(value) = line.strip_prefix("Preferred embedding model for local experiments:") {
        let model = sanitize_fact_text(value);
        let subject = acc
            .primary_subject
            .clone()
            .or_else(|| acc.document_title.clone())
            .unwrap_or_else(|| "DOCUMENT".to_string());
        acc.add_relation(
            &subject,
            "PROJECT",
            "uses",
            &model,
            "TOOL",
            0.88,
            chunk_index,
            order_index,
            Map::new(),
        );
        acc.decisions.insert(format!("{subject} uses {model}"));
        acc.tags.insert("decision".to_string());
        return true;
    }

    if let Some(value) = line.strip_prefix("Preferred extraction mode to try first:") {
        let mode = sanitize_fact_text(value);
        let subject = acc
            .primary_subject
            .clone()
            .or_else(|| acc.document_title.clone())
            .unwrap_or_else(|| "DOCUMENT".to_string());
        acc.add_relation(
            &subject,
            "PROJECT",
            "uses",
            &mode,
            "MODE",
            0.88,
            chunk_index,
            order_index,
            Map::new(),
        );
        acc.decisions.insert(format!("{subject} uses {mode}"));
        acc.tags.insert("decision".to_string());
        return true;
    }

    false
}
