//! Tests for the derived graph edge shape. Ported from OpenHuman
//! `memory_graph::types`.

use super::*;
use serde_json::json;

#[test]
fn graph_edge_is_constructible() {
    let edge = GraphEdge {
        subject: "person:alice".into(),
        object: "topic:phoenix".into(),
        weight: 2,
    };
    assert_eq!(edge.weight, 2);
    assert_eq!(edge.subject, "person:alice");
}

#[test]
fn graph_edge_roundtrips_via_serde() {
    let edge = GraphEdge {
        subject: "person:alice".into(),
        object: "project:openhuman".into(),
        weight: 3,
    };
    let value = serde_json::to_value(&edge).unwrap();
    assert_eq!(
        value,
        json!({
            "subject": "person:alice",
            "object": "project:openhuman",
            "weight": 3
        })
    );

    let decoded: GraphEdge = serde_json::from_value(value).unwrap();
    assert_eq!(decoded, edge);
}
