use super::*;

#[test]
fn compose_topic_summary_alias_format() {
    let input = sample_summary_input(SummaryTreeKind::Topic, "person:alex-johnson", 1);
    let composed = compose_summary_md(&input);
    assert!(composed.front_matter.contains("tree_kind: topic"));
    assert!(composed.front_matter.contains("topic"));
}

#[test]
fn compose_summary_with_zero_children() {
    let ts = chrono::Utc.timestamp_millis_opt(1_700_000_000_000).unwrap();
    let input = SummaryComposeInput {
        summary_id: "summary:L0:empty",
        tree_kind: SummaryTreeKind::Source,
        tree_id: "t1",
        tree_scope: "gmail:alice@x.com",
        level: 0,
        child_ids: &[],
        child_basenames: None,
        child_count: 0,
        time_range_start: ts,
        time_range_end: ts,
        sealed_at: ts,
        body: "empty",
    };
    let composed = compose_summary_md(&input);
    assert!(composed.front_matter.contains("children: []"));
    assert!(composed.front_matter.contains("child_count: 0"));
}

#[test]
fn compose_summary_same_start_end_date_single_date_alias() {
    let ts = chrono::Utc.timestamp_millis_opt(1_700_000_000_000).unwrap();
    let input = SummaryComposeInput {
        summary_id: "summary:L1:sameday",
        tree_kind: SummaryTreeKind::Global,
        tree_id: "t1",
        tree_scope: "global",
        level: 1,
        child_ids: &["child-a".to_string()],
        child_basenames: None,
        child_count: 1,
        time_range_start: ts,
        time_range_end: ts,
        sealed_at: ts,
        body: "day recap",
    };
    let composed = compose_summary_md(&input);
    let alias_line = composed
        .front_matter
        .lines()
        .find(|l| l.contains("L1") && l.contains("global digest"))
        .expect("alias line must be present");
    let date_str = ts.format("%Y-%m-%d").to_string();
    assert!(alias_line.contains(&date_str), "got: {alias_line}");
    assert!(!alias_line.contains('\u{2013}'), "got: {alias_line}");
}

#[test]
fn scope_short_label_two_participants() {
    let label = scope_short_label("gmail:alice@x.com|bob@y.com");
    assert_eq!(label, "alice@x.com \u{2194} bob@y.com");
}

#[test]
fn scope_short_label_many_participants() {
    let label = scope_short_label("gmail:alice@x.com|bob@y.com|carol@z.com");
    assert_eq!(label, "alice@x.com + 2 others");
}

#[test]
fn scope_short_label_non_gmail_returns_raw() {
    let label = scope_short_label("slack:#general");
    assert_eq!(label, "slack:#general");
}

#[test]
fn rewrite_summary_tags_delegates_to_rewrite_tags() {
    let ts = chrono::Utc.timestamp_millis_opt(1_700_000_000_000).unwrap();
    let input = SummaryComposeInput {
        summary_id: "sum:L1:rwttest",
        tree_kind: SummaryTreeKind::Source,
        tree_id: "t1",
        tree_scope: "gmail:alice@x.com",
        level: 1,
        child_ids: &["c1".to_string()],
        child_basenames: None,
        child_count: 1,
        time_range_start: ts,
        time_range_end: ts,
        sealed_at: ts,
        body: "summary body text",
    };
    let composed = compose_summary_md(&input);
    let file_bytes = composed.full.as_bytes();
    let new_tags = vec!["person/Alice-Smith".to_string(), "topic/Memory".to_string()];
    let rewritten = rewrite_summary_tags(file_bytes, &new_tags).unwrap();
    let rewritten_str = std::str::from_utf8(&rewritten).unwrap();
    assert!(rewritten_str.contains("  - person/Alice-Smith"));
    assert!(rewritten_str.contains("  - topic/Memory"));
    assert!(!rewritten_str.contains("tags: []"));
    assert!(rewritten_str.contains(&format!(
        "openhuman_core_version: {}",
        OPENHUMAN_CORE_VERSION
    )));
    assert!(rewritten_str.contains(&format!(
        "memory_artifact_format: {}",
        MEMORY_ARTIFACT_FORMAT
    )));
    assert!(rewritten_str.ends_with("summary body text"));
}

#[test]
fn rewrite_summary_tags_backfills_missing_provenance() {
    let file = b"---\nid: legacy\nkind: summary\ntags: []\naliases:\n  - legacy\n---\nlegacy body";
    let rewritten = rewrite_summary_tags(file, &["person/Alice".to_string()]).unwrap();
    let rewritten_str = std::str::from_utf8(&rewritten).unwrap();
    assert!(rewritten_str.contains(&format!(
        "openhuman_core_version: {}",
        OPENHUMAN_CORE_VERSION
    )));
    assert!(rewritten_str.contains(&format!(
        "memory_artifact_format: {}",
        MEMORY_ARTIFACT_FORMAT
    )));
    assert!(rewritten_str.ends_with("legacy body"));
}
