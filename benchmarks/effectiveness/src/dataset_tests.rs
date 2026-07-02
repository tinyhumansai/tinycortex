//! Validation tests for the dataset loader.

use super::*;

fn doc(id: &str) -> Document {
    Document {
        id: id.to_string(),
        title: String::new(),
        text: format!("body of {id}"),
        namespace: "bench".to_string(),
    }
}

fn query(id: &str, relevant: &[&str]) -> QueryCase {
    QueryCase {
        id: id.to_string(),
        query: "q".to_string(),
        relevant_ids: relevant.iter().map(|s| s.to_string()).collect(),
        namespace: None,
    }
}

#[test]
fn accepts_well_formed_dataset() {
    let ds = Dataset {
        name: "t".into(),
        description: String::new(),
        documents: vec![doc("a"), doc("b")],
        queries: vec![query("q1", &["a"])],
    };
    assert!(ds.validate().is_ok());
}

#[test]
fn rejects_duplicate_document_ids() {
    let ds = Dataset {
        name: "t".into(),
        description: String::new(),
        documents: vec![doc("a"), doc("a")],
        queries: vec![],
    };
    assert!(ds.validate().is_err());
}

#[test]
fn rejects_dangling_relevance_label() {
    let ds = Dataset {
        name: "t".into(),
        description: String::new(),
        documents: vec![doc("a")],
        queries: vec![query("q1", &["missing"])],
    };
    assert!(ds.validate().is_err());
}

#[test]
fn rejects_empty_relevant_set() {
    let ds = Dataset {
        name: "t".into(),
        description: String::new(),
        documents: vec![doc("a")],
        queries: vec![query("q1", &[])],
    };
    assert!(ds.validate().is_err());
}
