use serde_json::json;

use super::*;
use crate::memory::config::{ComposioMode, ComposioSyncConfig, MemoryConfig, SecretString};
use crate::memory::sync::state::SyncState;

fn pipeline() -> AsanaSyncPipeline {
    AsanaSyncPipeline::new(
        ComposioClient::new(ComposioSyncConfig {
            mode: ComposioMode::Direct,
            base_url: "http://127.0.0.1:1".into(),
            api_key: Some(SecretString::new("test-key")),
            bearer_token: None,
            entity_id: Some("entity-1".into()),
        }),
        "conn-asana",
    )
}

#[test]
fn toolkit_and_action_use_asana_slugs() {
    let pipeline = pipeline();
    assert_eq!(pipeline.toolkit(), "asana");
    assert_eq!(pipeline.action(), "ASANA_GET_MULTIPLE_TASKS");
    assert!(pipeline.server_side_depth());
}

#[test]
fn extract_page_reads_tasks_and_offset_cursor() {
    let pipeline = pipeline();
    // Composio-wrapped Asana list payload: tasks under `data.data`, the paging
    // token under `data.next_page.offset`.
    let payload = json!({
        "data": {
            "data": [
                {"gid": "1201", "name": "Ship sync", "modified_at": "2026-05-02T10:00:00.000Z"},
                {"gid": "1202", "name": "Write tests", "modified_at": "2026-05-01T09:00:00.000Z"}
            ],
            "next_page": {"offset": "eyJvIjoxfQ==", "path": "/tasks?offset=eyJvIjoxfQ=="}
        }
    });
    let page = pipeline.extract_page(&payload, None);
    assert_eq!(page.items.len(), 2);
    assert_eq!(page.next.as_deref(), Some("eyJvIjoxfQ=="));
}

#[test]
fn extract_page_without_next_page_stops() {
    let pipeline = pipeline();
    let payload = json!({"data": {"data": [{"gid": "9", "modified_at": "2026-01-01T00:00:00Z"}]}});
    let page = pipeline.extract_page(&payload, None);
    assert_eq!(page.items.len(), 1);
    assert_eq!(page.next, None);
}

#[test]
fn dedup_key_pins_the_stable_gid_and_modified_version() {
    let pipeline = pipeline();
    let task =
        json!({"gid": "1201", "name": "Ship sync", "modified_at": "2026-05-02T10:00:00.000Z"});
    assert_eq!(
        pipeline.dedup_key(&task).as_deref(),
        Some("1201@2026-05-02T10:00:00.000Z")
    );
    assert_eq!(
        pipeline.sort_cursor(&task).as_deref(),
        Some("2026-05-02T10:00:00.000Z")
    );
}

#[tokio::test]
async fn document_uses_stable_gid_id_and_project_scope() {
    let pipeline = pipeline();
    let scope = SyncScope::named("77", "project:77")
        .with_metadata(json!({"workspace_id": "ws-9", "project_name": "Launch"}));
    let raw =
        json!({"gid": "1201", "name": "Ship sync", "modified_at": "2026-05-02T10:00:00.000Z"});
    let item = SyncItem {
        dedup_key: "1201@2026-05-02T10:00:00.000Z".into(),
        sort_cursor: Some("2026-05-02T10:00:00.000Z".into()),
        raw,
    };
    let client = ComposioClient::new(ComposioSyncConfig {
        mode: ComposioMode::Direct,
        base_url: "http://127.0.0.1:1".into(),
        api_key: Some(SecretString::new("test-key")),
        bearer_token: None,
        entity_id: Some("entity-1".into()),
    });
    let mut state = SyncState::new("asana", "conn-asana");
    let document = pipeline
        .document(&scope, "conn-asana", item, &client, &mut state)
        .await
        .unwrap();

    // Stable dedupe: id is the gid, not a per-run value; re-syncs upsert in place.
    assert_eq!(document.document_id, "asana:1201");
    assert_eq!(document.title, "Ship sync");
    assert_eq!(document.toolkit, "asana");
    assert_eq!(document.metadata["taint"], "external_sync");
    assert_eq!(document.metadata["path_scope"], "asana/project/77");
    assert_eq!(document.metadata["project_id"], "77");
    assert_eq!(document.metadata["workspace_id"], "ws-9");
}

#[test]
fn arguments_carry_project_and_modified_since_from_cursor() {
    let pipeline = pipeline();
    let scope = SyncScope::named("77", "project:77");
    let config = MemoryConfig::new("/tmp/tinycortex-asana-args");
    let mut state = SyncState::new("asana", "conn-asana");
    state.advance_cursor("2026-05-01T00:00:00Z");
    let args = pipeline.arguments(&scope, &config, &state, Some("offset-2"));
    assert_eq!(args["project"], "77");
    assert_eq!(args["modified_since"], "2026-05-01T00:00:00Z");
    assert_eq!(args["offset"], "offset-2");
    assert_eq!(args["opt_fields"], TASK_OPT_FIELDS);
}
