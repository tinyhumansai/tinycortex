use super::*;
use tempfile::TempDir;

#[test]
fn stages_defaults_into_fresh_root() {
    let tmp = TempDir::new().unwrap();
    ensure_obsidian_defaults(tmp.path()).unwrap();
    let graph = tmp.path().join(".obsidian").join("graph.json");
    let types = tmp.path().join(".obsidian").join("types.json");
    assert!(graph.exists(), "graph.json should be staged");
    assert!(types.exists(), "types.json should be staged");
    // Body must be the bundled content, not empty.
    let g = std::fs::read_to_string(&graph).unwrap();
    assert!(g.contains("colorGroups"), "graph.json missing colorGroups");
}

#[test]
fn does_not_overwrite_existing_file() {
    let tmp = TempDir::new().unwrap();
    let obs = tmp.path().join(".obsidian");
    std::fs::create_dir_all(&obs).unwrap();
    let graph = obs.join("graph.json");
    std::fs::write(&graph, r#"{"user":"custom"}"#).unwrap();

    ensure_obsidian_defaults(tmp.path()).unwrap();

    let body = std::fs::read_to_string(&graph).unwrap();
    assert_eq!(
        body, r#"{"user":"custom"}"#,
        "user-customised graph.json must not be clobbered"
    );
}

#[test]
fn idempotent_second_call_is_no_op() {
    let tmp = TempDir::new().unwrap();
    ensure_obsidian_defaults(tmp.path()).unwrap();
    ensure_obsidian_defaults(tmp.path()).unwrap();
    // Second call must succeed without panicking and must not have
    // duplicated or grown the file.
    let g = std::fs::read_to_string(tmp.path().join(".obsidian/graph.json")).unwrap();
    assert!(g.contains("colorGroups"));
}
