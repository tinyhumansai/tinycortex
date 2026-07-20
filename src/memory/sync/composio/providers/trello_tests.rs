use async_trait::async_trait;
use serde_json::json;

use super::*;
use crate::memory::config::{ComposioMode, ComposioSyncConfig, SecretString};
use crate::memory::sync::composio::client::{ActionExecutor, ExecuteResponse};
use crate::memory::sync::composio::ComposioClient;

/// Executor that must never be reached: `document()` for Trello serializes the
/// card in-place and issues no follow-up action call.
struct UnusedExecutor;

#[async_trait]
impl ActionExecutor for UnusedExecutor {
    async fn execute(
        &self,
        action: &str,
        _arguments: serde_json::Value,
        _connection_id: Option<&str>,
    ) -> anyhow::Result<ExecuteResponse> {
        panic!("document() unexpectedly executed action {action}");
    }
}

fn pipeline() -> TrelloSyncPipeline {
    let config = ComposioSyncConfig {
        mode: ComposioMode::Direct,
        base_url: "https://example.invalid".into(),
        api_key: Some(SecretString::new("test-key")),
        bearer_token: None,
        entity_id: None,
    };
    TrelloSyncPipeline::new(ComposioClient::new(config), "trello-conn")
}

#[test]
fn toolkit_and_action_are_stable_slugs() {
    let pipeline = pipeline();
    assert_eq!(pipeline.toolkit(), "trello");
    assert_eq!(pipeline.action(), "TRELLO_GET_BOARDS_CARDS_BY_ID_BOARD");
}

#[test]
fn extract_page_reads_cards_and_pages_on_full_window() {
    let pipeline = pipeline();
    // A full page (page_size cards) yields a `before` cursor = oldest card id.
    let cards: Vec<serde_json::Value> = (0..pipeline.page_size)
        .map(|i| json!({"id": format!("card-{i}"), "name": format!("Card {i}")}))
        .collect();
    let last_id = format!("card-{}", pipeline.page_size - 1);
    let payload = json!({"data": cards});

    let page = pipeline.extract_page(&payload, None);
    assert_eq!(page.items.len(), pipeline.page_size);
    assert_eq!(page.next.as_deref(), Some(last_id.as_str()));
    assert_eq!(page.items[0]["id"], json!("card-0"));

    // A short page drains the board: no further pagination.
    let short = json!({"data": [{"id": "card-x", "name": "solo"}]});
    assert!(pipeline.extract_page(&short, None).next.is_none());
}

#[test]
fn dedup_key_suffixes_activity_for_reingest() {
    let pipeline = pipeline();
    let card = json!({"id": "abc123", "dateLastActivity": "2026-07-20T10:00:00Z"});
    assert_eq!(
        pipeline.dedup_key(&card).as_deref(),
        Some("abc123@2026-07-20T10:00:00Z")
    );
    let no_activity = json!({"id": "abc123"});
    assert_eq!(pipeline.dedup_key(&no_activity).as_deref(), Some("abc123"));
}

#[tokio::test]
async fn document_uses_stable_card_id_as_document_id() {
    let pipeline = pipeline();
    let scope = SyncScope::named("board-9", "board:board-9");
    let raw = json!({
        "id": "card-42",
        "name": "Ship the pipeline",
        "dateLastActivity": "2026-07-20T10:00:00Z"
    });
    let mut state = SyncState::new("trello", "trello-conn");
    let item = SyncItem {
        dedup_key: "card-42@2026-07-20T10:00:00Z".into(),
        sort_cursor: Some("2026-07-20T10:00:00Z".into()),
        raw: raw.clone(),
    };

    let doc = pipeline
        .document(&scope, "trello-conn", item, &UnusedExecutor, &mut state)
        .await
        .expect("document conversion");

    // Stable dedupe: the upsert key is derived from the card id, never a per-run id.
    assert_eq!(doc.document_id, "trello:card-42");
    assert_eq!(doc.namespace_skill_id, "trello");
    assert_eq!(doc.toolkit, "trello");
    assert_eq!(doc.title, "Ship the pipeline");
    assert_eq!(doc.metadata["taint"], json!("external_sync"));
    assert_eq!(doc.metadata["provider_id"], json!("card-42"));
    assert_eq!(doc.metadata["board_id"], json!("board-9"));
}
