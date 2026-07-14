//! Unit tests for the pure Composio connect helpers: entity-id persistence,
//! status classification, and response-field extraction. No network I/O.

use super::*;
use serde_json::json;
use wiremock::matchers::{body_partial_json, header, method, path, query_param};
use wiremock::{Mock, MockServer, ResponseTemplate};

#[test]
fn generated_entity_id_is_prefixed_and_unique() {
    let a = generate_entity_id();
    let b = generate_entity_id();
    assert!(a.starts_with("tinycortex-"), "unexpected id: {a}");
    assert_ne!(a, b, "two generated ids must differ");
}

#[test]
fn status_active_is_case_insensitive() {
    assert!(status_is_active("ACTIVE"));
    assert!(status_is_active("active"));
    assert!(status_is_active("  Active  "));
    assert!(!status_is_active("INITIATED"));
    assert!(!status_is_active("FAILED"));
}

#[test]
fn status_terminal_matches_failure_states_only() {
    for terminal in ["FAILED", "expired", "Revoked", "DELETED"] {
        assert!(
            status_is_terminal(terminal),
            "{terminal} should be terminal"
        );
    }
    for live in ["ACTIVE", "INITIATED", "INITIALIZING", "INACTIVE"] {
        assert!(!status_is_terminal(live), "{live} should not be terminal");
    }
}

#[test]
fn extract_status_probes_top_level_and_nested() {
    assert_eq!(
        extract_status(&json!({"status": "ACTIVE"})).as_deref(),
        Some("ACTIVE")
    );
    assert_eq!(
        extract_status(&json!({"state": {"val": {"status": "INITIATED"}}})).as_deref(),
        Some("INITIATED")
    );
    assert_eq!(
        extract_status(&json!({"connectionData": {"val": {"status": "EXPIRED"}}})).as_deref(),
        Some("EXPIRED")
    );
    assert_eq!(extract_status(&json!({"other": 1})), None);
}

#[test]
fn extract_link_fields_from_v3_shape() {
    let response = json!({
        "connected_account_id": "ca_abc123",
        "redirect_url": "https://backend.composio.dev/oauth/start?token=xyz",
        "link_token": "lt_123",
    });
    assert_eq!(extract_account_id(&response).as_deref(), Some("ca_abc123"));
    assert_eq!(
        extract_redirect_url(&response).as_deref(),
        Some("https://backend.composio.dev/oauth/start?token=xyz")
    );
}

#[test]
fn extract_link_fields_tolerates_legacy_shape() {
    let response = json!({
        "id": "ca_legacy",
        "connectionData": {"val": {"status": "INITIATED", "redirectUrl": "https://x/y"}},
    });
    assert_eq!(extract_account_id(&response).as_deref(), Some("ca_legacy"));
    assert_eq!(
        extract_redirect_url(&response).as_deref(),
        Some("https://x/y")
    );
}

#[test]
fn resolve_auth_config_prefers_matching_toolkit() {
    let list = json!({
        "items": [
            {"id": "ac_github", "toolkit": {"slug": "github"}},
            {"id": "ac_gmail", "toolkit": {"slug": "gmail"}},
        ]
    });
    assert_eq!(
        resolve_auth_config_id(&list, "gmail").as_deref(),
        Some("ac_gmail")
    );
    assert_eq!(
        resolve_auth_config_id(&list, "github").as_deref(),
        Some("ac_github")
    );
}

#[test]
fn resolve_auth_config_falls_back_to_first_when_no_slug_match() {
    // Server already filtered by toolkit_slug; items may not echo a slug shape
    // we recognise, so fall back to the first listed config.
    let list = json!({ "items": [ {"id": "ac_only"} ] });
    assert_eq!(
        resolve_auth_config_id(&list, "gmail").as_deref(),
        Some("ac_only")
    );
    assert_eq!(resolve_auth_config_id(&json!({"items": []}), "gmail"), None);
}

#[test]
fn entity_store_round_trips_and_reuses_ids() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join(".composio-harness.json");

    let mut store = EntityStore::load(&path);
    assert_eq!(store.get("gmail"), None);

    // First resolve for gmail with an explicit override adopts + persists it.
    let gmail_id = store.entity_id_for("gmail", Some("my-entity")).unwrap();
    assert_eq!(gmail_id, "my-entity");

    // A second toolkit with no override generates a fresh id.
    let github_id = store.entity_id_for("github", None).unwrap();
    assert!(github_id.starts_with("tinycortex-"));
    assert_ne!(github_id, gmail_id);

    // Reload from disk: both ids survived and are stable on re-resolve.
    let mut reloaded = EntityStore::load(&path);
    assert_eq!(reloaded.get("gmail"), Some("my-entity"));
    assert_eq!(reloaded.get("github"), Some(github_id.as_str()));
    // Stored value wins even if a different override is passed on re-run.
    assert_eq!(
        reloaded.entity_id_for("gmail", Some("different")).unwrap(),
        "my-entity"
    );
}

#[test]
fn entity_store_load_tolerates_missing_and_corrupt_files() {
    let dir = tempfile::tempdir().unwrap();
    let missing = dir.path().join("nope.json");
    assert_eq!(EntityStore::load(&missing).get("gmail"), None);

    let corrupt = dir.path().join("corrupt.json");
    std::fs::write(&corrupt, "{ not json ").unwrap();
    assert_eq!(EntityStore::load(&corrupt).get("gmail"), None);
}

#[tokio::test]
async fn list_auth_configs_sends_filter_and_decodes_success() {
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path("/api/v3/auth_configs"))
        .and(query_param("toolkit_slug", "gmail"))
        .and(header("x-api-key", "secret"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({
            "items": [{"id": "ac_gmail", "toolkit": {"slug": "gmail"}}]
        })))
        .mount(&server)
        .await;

    let value = list_auth_configs(
        &reqwest::Client::new(),
        &format!("{}/api/v3/", server.uri()),
        "secret",
        Some("gmail"),
    )
    .await
    .unwrap();

    assert_eq!(
        resolve_auth_config_id(&value, "gmail").as_deref(),
        Some("ac_gmail")
    );
}

#[tokio::test]
async fn list_auth_configs_redacts_error_body_and_rejects_invalid_json() {
    let failure = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path("/auth_configs"))
        .respond_with(ResponseTemplate::new(401).set_body_string("echoed-secret"))
        .mount(&failure)
        .await;
    let error = list_auth_configs(
        &reqwest::Client::new(),
        &failure.uri(),
        "echoed-secret",
        None,
    )
    .await
    .unwrap_err()
    .to_string();
    assert!(error.contains("HTTP 401"));
    assert!(!error.contains("echoed-secret"));

    let malformed = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path("/auth_configs"))
        .respond_with(ResponseTemplate::new(200).set_body_string("not-json"))
        .mount(&malformed)
        .await;
    assert!(
        list_auth_configs(&reqwest::Client::new(), &malformed.uri(), "key", None)
            .await
            .unwrap_err()
            .to_string()
            .contains("decode failed")
    );
}

#[tokio::test]
async fn create_connection_link_sends_trimmed_callback_and_extracts_fields() {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/connected_accounts/link"))
        .and(header("x-api-key", "secret"))
        .and(body_partial_json(json!({
            "auth_config_id": "ac_1",
            "user_id": "user_1",
            "callback_url": "https://callback"
        })))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({
            "connected_account_id": "ca_1",
            "redirect_url": "https://oauth"
        })))
        .mount(&server)
        .await;

    let link = create_connection_link(
        &reqwest::Client::new(),
        &server.uri(),
        "secret",
        "ac_1",
        "user_1",
        Some(" https://callback "),
    )
    .await
    .unwrap();
    assert_eq!(link.connected_account_id, "ca_1");
    assert_eq!(link.redirect_url.as_deref(), Some("https://oauth"));
}

#[tokio::test]
async fn create_connection_link_rejects_http_decode_and_missing_id_failures() {
    for (status, body, expected) in [
        (500, "server error", "HTTP 500"),
        (200, "not-json", "decode failed"),
        (200, "{}", "missing a connected account id"),
    ] {
        let server = MockServer::start().await;
        Mock::given(method("POST"))
            .and(path("/connected_accounts/link"))
            .respond_with(ResponseTemplate::new(status).set_body_string(body))
            .mount(&server)
            .await;
        let error = create_connection_link(
            &reqwest::Client::new(),
            &server.uri(),
            "key",
            "ac",
            "user",
            Some("  "),
        )
        .await
        .unwrap_err()
        .to_string();
        assert!(error.contains(expected), "unexpected error: {error}");
    }
}

#[tokio::test]
async fn get_connection_status_handles_success_and_safe_failures() {
    let active = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path("/connected_accounts/ca_1"))
        .and(header("x-api-key", "key"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({"status": "ACTIVE"})))
        .mount(&active)
        .await;
    assert_eq!(
        get_connection_status(&reqwest::Client::new(), &active.uri(), "key", "ca_1")
            .await
            .unwrap()
            .as_deref(),
        Some("ACTIVE")
    );

    for (status, body, expected) in [(403, "key", "HTTP 403"), (200, "invalid", "decode failed")] {
        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .and(path("/connected_accounts/ca_2"))
            .respond_with(ResponseTemplate::new(status).set_body_string(body))
            .mount(&server)
            .await;
        let error = get_connection_status(&reqwest::Client::new(), &server.uri(), "key", "ca_2")
            .await
            .unwrap_err()
            .to_string();
        assert!(error.contains(expected));
    }
}
