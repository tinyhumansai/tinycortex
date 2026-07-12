use tinycortex::memory::config::{ComposioMode, ComposioSyncConfig, SecretString};
use tinycortex::memory::sync::ComposioClient;

#[tokio::test]
#[ignore = "requires COMPOSIO_API_KEY and a connected Gmail account"]
async fn direct_gmail_sync_live_smoke() {
    let key = std::env::var("COMPOSIO_API_KEY").expect("COMPOSIO_API_KEY");
    let connection_id = std::env::var("COMPOSIO_GMAIL_CONNECTION_ID").ok();
    let client = ComposioClient::new(ComposioSyncConfig {
        mode: ComposioMode::Direct,
        base_url: "https://backend.composio.dev/api/v3".into(),
        api_key: Some(SecretString::new(key)),
        bearer_token: None,
        entity_id: std::env::var("COMPOSIO_ENTITY_ID").ok(),
    });
    let response = client
        .execute(
            "GMAIL_FETCH_EMAILS",
            serde_json::json!({"max_results": 5, "include_payload": true}),
            connection_id.as_deref(),
        )
        .await
        .unwrap();
    assert!(response.successful, "{:?}", response.error);
    assert!(response.data.is_object());
}
