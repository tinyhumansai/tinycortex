use super::*;
use crate::memory::graph::{pairs_from_entities, upsert_edges, PairDistance};
use crate::memory::retrieval::test_support::{
    fixed_ts, index_entity_occurrence, insert_chunks, insert_summary, insert_tree_row,
    sample_chunk, source_tree, summary_node, test_config,
};
use crate::memory::score::embed::InertEmbedder;
use crate::memory::score::extract::EntityKind;

#[test]
fn options_and_ids_are_bounded_deterministically() {
    assert_eq!(FastRetrieveOptions::default().limit, 10);
    assert_eq!(FastRetrieveOptions::default().max_hops, 2);
    assert_eq!(
        dedup_ids(vec!["a".into(), "a".into(), "b".into()].into_iter()),
        vec!["a", "b"]
    );
}

#[tokio::test]
async fn blank_query_is_empty_without_opening_storage() {
    let (_temp, config) = test_config();
    let response = fast_retrieve(
        &config,
        "  ",
        &[],
        &InertEmbedder,
        None,
        FastRetrieveOptions::default(),
    )
    .await
    .unwrap();
    assert!(response.hits.is_empty());
}

#[tokio::test]
async fn dense_fallback_filters_scope_before_limit() {
    let (_temp, mut config) = test_config();
    config.retrieval.limits.max_limit = 1;
    for (id, scope, timestamp) in [
        ("allowed", "slack:#allowed", fixed_ts()),
        (
            "denied",
            "slack:#denied",
            fixed_ts() + chrono::Duration::seconds(1),
        ),
    ] {
        let tree_id = format!("tree-{id}");
        insert_tree_row(&config, &source_tree(&tree_id, scope, Some(id), 1));
        insert_summary(
            &config,
            &summary_node(id, &tree_id, 1, None, &[], id, timestamp),
        );
    }
    let scope = HashSet::from(["slack:#allowed".to_string()]);

    let response = fast_retrieve(
        &config,
        "anything",
        &[],
        &InertEmbedder,
        Some(&scope),
        FastRetrieveOptions {
            limit: 1,
            max_hops: 99,
            time_window_days: None,
        },
    )
    .await
    .unwrap();

    assert_eq!(response.total, 1);
    assert_eq!(response.hits[0].node_id, "allowed");
}

#[tokio::test]
async fn occurrence_fallback_prioritizes_hits_containing_query_entities() {
    let (_temp, config) = test_config();
    let tree = source_tree("tree", "slack:#eng", Some("matching"), 1);
    insert_tree_row(&config, &tree);
    for (id, entities) in [
        ("unrelated", Vec::new()),
        ("matching", vec!["person:alice".to_string()]),
    ] {
        let mut node = summary_node(id, "tree", 1, None, &[], id, fixed_ts());
        node.entities = entities;
        insert_summary(&config, &node);
    }

    let response = fast_retrieve(
        &config,
        "alice",
        &["person:alice".into()],
        &InertEmbedder,
        None,
        FastRetrieveOptions {
            limit: 1,
            max_hops: 0,
            time_window_days: None,
        },
    )
    .await
    .unwrap();

    assert_eq!(response.total, 2);
    assert_eq!(response.hits[0].node_id, "matching");
    assert!(response.truncated);
}

#[test]
fn local_candidates_intersect_occurrences_and_keep_latest_timestamp() {
    let (_temp, config) = test_config();
    for (entity, kind, timestamp) in [
        ("person:alice", EntityKind::Person, 10),
        ("topic:launch", EntityKind::Topic, 20),
    ] {
        index_entity_occurrence(
            &config,
            entity,
            kind,
            entity,
            "shared-node",
            "summary",
            timestamp,
            Some("tree"),
        );
    }
    let pairs = vec![PairDistance {
        a: "person:alice".into(),
        b: "topic:launch".into(),
        dist: 1,
    }];

    let candidates = local_candidates(&config, &pairs).unwrap();

    assert_eq!(candidates["shared-node"].matched.len(), 2);
    assert_eq!(candidates["shared-node"].latest_ts, 20);
}

#[test]
fn resolve_local_hydrates_leaf_and_summary_and_skips_missing_or_out_of_scope() {
    let (_temp, config) = test_config();
    let leaf = sample_chunk("slack:#allowed", 0, "leaf");
    insert_chunks(&config, std::slice::from_ref(&leaf));
    insert_tree_row(
        &config,
        &source_tree("summary-tree", "slack:#allowed", Some("summary"), 1),
    );
    insert_summary(
        &config,
        &summary_node(
            "summary",
            "summary-tree",
            1,
            None,
            &[],
            "summary",
            fixed_ts(),
        ),
    );
    let candidate = |kind: &str, matched: &[&str], latest_ts| Candidate {
        node_kind: kind.into(),
        matched: matched.iter().map(|value| (*value).to_string()).collect(),
        latest_ts,
    };
    let candidates = HashMap::from([
        (leaf.id.clone(), candidate("leaf", &["a"], 3)),
        ("summary".into(), candidate("summary", &["a", "b"], 2)),
        ("missing".into(), candidate("summary", &["a", "b", "c"], 1)),
    ]);
    let scope = HashSet::from(["slack:#allowed".to_string()]);

    let response = resolve_local(&config, candidates, Some(&scope), 10).unwrap();

    assert_eq!(response.total, 2);
    assert_eq!(response.hits[0].node_id, "summary");
    assert_eq!(response.hits[0].score, 2.0);
    assert_eq!(response.hits[1].node_id, leaf.id);
}

#[tokio::test]
async fn local_branch_intersects_entities_and_applies_scope_before_limit() {
    let (_temp, config) = test_config();
    let allowed_tree = source_tree("allowed-tree", "slack:#allowed", Some("allowed"), 1);
    let denied_tree = source_tree("denied-tree", "slack:#denied", Some("denied"), 1);
    insert_tree_row(&config, &allowed_tree);
    insert_tree_row(&config, &denied_tree);
    for (id, tree_id) in [("allowed", "allowed-tree"), ("denied", "denied-tree")] {
        insert_summary(
            &config,
            &summary_node(id, tree_id, 1, None, &[], id, fixed_ts()),
        );
        for (entity, kind) in [
            ("person:alice", EntityKind::Person),
            ("topic:launch", EntityKind::Topic),
        ] {
            index_entity_occurrence(
                &config,
                entity,
                kind,
                entity,
                id,
                "summary",
                fixed_ts().timestamp_millis(),
                Some(tree_id),
            );
        }
    }
    upsert_edges(
        &config,
        &pairs_from_entities(&["person:alice".into(), "topic:launch".into()]),
        fixed_ts().timestamp_millis(),
    )
    .unwrap();
    let scope = HashSet::from(["slack:#allowed".to_string()]);

    let response = fast_retrieve(
        &config,
        "alice launch",
        &["person:alice".into(), "topic:launch".into()],
        &InertEmbedder,
        Some(&scope),
        FastRetrieveOptions {
            limit: 1,
            ..Default::default()
        },
    )
    .await
    .unwrap();

    assert_eq!(response.total, 1);
    assert_eq!(response.hits[0].node_id, "allowed");
    assert_eq!(response.hits[0].score, 2.0);
}
