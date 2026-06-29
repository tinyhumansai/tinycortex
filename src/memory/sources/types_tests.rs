//! Tests for source type contracts, serde wire strings, and validation.

use super::*;

#[test]
fn source_kind_round_trips_via_serde() {
    for kind in [
        SourceKind::Composio,
        SourceKind::Conversation,
        SourceKind::Folder,
        SourceKind::GithubRepo,
        SourceKind::TwitterQuery,
        SourceKind::RssFeed,
        SourceKind::WebPage,
    ] {
        let json = serde_json::to_string(&kind).unwrap();
        let decoded: SourceKind = serde_json::from_str(&json).unwrap();
        assert_eq!(decoded, kind);
    }
}

#[test]
fn source_kind_as_str_matches_wire_strings() {
    assert_eq!(SourceKind::Composio.as_str(), "composio");
    assert_eq!(SourceKind::Conversation.as_str(), "conversation");
    assert_eq!(SourceKind::Folder.as_str(), "folder");
    assert_eq!(SourceKind::GithubRepo.as_str(), "github_repo");
    assert_eq!(SourceKind::TwitterQuery.as_str(), "twitter_query");
    assert_eq!(SourceKind::RssFeed.as_str(), "rss_feed");
    assert_eq!(SourceKind::WebPage.as_str(), "web_page");
}

#[test]
fn validate_composio_requires_toolkit_and_connection_id() {
    let entry = MemorySourceEntry {
        id: "src_1".into(),
        kind: SourceKind::Composio,
        label: "Gmail".into(),
        enabled: true,
        toolkit: Some("gmail".into()),
        connection_id: None,
        ..default_entry()
    };
    assert!(entry.validate().is_err());

    let valid = MemorySourceEntry {
        connection_id: Some("cmp_123".into()),
        ..entry
    };
    assert!(valid.validate().is_ok());
}

#[test]
fn validate_folder_requires_path() {
    let entry = MemorySourceEntry {
        id: "src_2".into(),
        kind: SourceKind::Folder,
        label: "Notes".into(),
        enabled: true,
        path: None,
        ..default_entry()
    };
    assert!(entry.validate().is_err());
}

#[test]
fn validate_github_requires_url() {
    let entry = MemorySourceEntry {
        id: "src_3".into(),
        kind: SourceKind::GithubRepo,
        label: "Repo".into(),
        enabled: true,
        url: Some("https://github.com/org/repo".into()),
        ..default_entry()
    };
    assert!(entry.validate().is_ok());
}

#[test]
fn validate_twitter_requires_query() {
    let entry = MemorySourceEntry {
        id: "src_tw".into(),
        kind: SourceKind::TwitterQuery,
        label: "Tweets".into(),
        enabled: true,
        query: None,
        ..default_entry()
    };
    assert!(entry.validate().is_err());
}

#[test]
fn validate_rss_and_web_page_require_url() {
    let rss = MemorySourceEntry {
        id: "src_rss".into(),
        kind: SourceKind::RssFeed,
        label: "Feed".into(),
        enabled: true,
        url: None,
        ..default_entry()
    };
    assert!(rss.validate().is_err());

    let web = MemorySourceEntry {
        id: "src_web".into(),
        kind: SourceKind::WebPage,
        label: "Page".into(),
        enabled: true,
        url: Some("https://example.com".into()),
        ..default_entry()
    };
    assert!(web.validate().is_ok());
}

#[test]
fn validate_conversation_needs_only_id_and_label() {
    let entry = MemorySourceEntry {
        id: "src_conv".into(),
        kind: SourceKind::Conversation,
        label: "Agent Conversations".into(),
        enabled: true,
        ..default_entry()
    };
    assert!(entry.validate().is_ok());
}

#[test]
fn validate_conversation_fails_with_empty_id() {
    let entry = MemorySourceEntry {
        id: "".into(),
        kind: SourceKind::Conversation,
        label: "Convos".into(),
        enabled: true,
        ..default_entry()
    };
    assert!(entry.validate().is_err());
}

#[test]
fn validate_conversation_fails_with_empty_label() {
    let entry = MemorySourceEntry {
        id: "src_conv".into(),
        kind: SourceKind::Conversation,
        label: "".into(),
        enabled: true,
        ..default_entry()
    };
    assert!(entry.validate().is_err());
}

#[test]
fn conversation_kind_serializes_to_snake_case() {
    let json = serde_json::to_string(&SourceKind::Conversation).unwrap();
    assert_eq!(json, "\"conversation\"");
}

#[test]
fn content_type_serializes_to_snake_case() {
    assert_eq!(
        serde_json::to_string(&ContentType::Markdown).unwrap(),
        "\"markdown\""
    );
    assert_eq!(
        serde_json::to_string(&ContentType::Html).unwrap(),
        "\"html\""
    );
    assert_eq!(
        serde_json::to_string(&ContentType::Plaintext).unwrap(),
        "\"plaintext\""
    );
}

#[test]
fn toml_round_trip() {
    let entry = MemorySourceEntry {
        id: "src_1".into(),
        kind: SourceKind::Folder,
        label: "My notes".into(),
        enabled: true,
        path: Some("/tmp/notes".into()),
        glob: Some("**/*.md".into()),
        ..default_entry()
    };
    let toml_str = toml::to_string_pretty(&entry).unwrap();
    let decoded: MemorySourceEntry = toml::from_str(&toml_str).unwrap();
    assert_eq!(decoded.id, "src_1");
    assert_eq!(decoded.kind, SourceKind::Folder);
    assert_eq!(decoded.path.as_deref(), Some("/tmp/notes"));
}

#[test]
fn conversation_toml_round_trip() {
    let entry = MemorySourceEntry {
        id: "src_conv".into(),
        kind: SourceKind::Conversation,
        label: "Conversations".into(),
        enabled: true,
        ..default_entry()
    };
    let toml_str = toml::to_string_pretty(&entry).unwrap();
    let decoded: MemorySourceEntry = toml::from_str(&toml_str).unwrap();
    assert_eq!(decoded.id, "src_conv");
    assert_eq!(decoded.kind, SourceKind::Conversation);
    assert_eq!(decoded.label, "Conversations");
    assert!(decoded.enabled);
}

#[test]
fn enabled_defaults_to_true_when_absent() {
    let toml_str = r#"
id = "src_x"
kind = "conversation"
label = "Convos"
"#;
    let decoded: MemorySourceEntry = toml::from_str(toml_str).unwrap();
    assert!(decoded.enabled);
}

/// A fully-`None` entry used as a `..default_entry()` base in the tests above.
pub(super) fn default_entry() -> MemorySourceEntry {
    MemorySourceEntry {
        id: String::new(),
        kind: SourceKind::Folder,
        label: String::new(),
        enabled: true,
        toolkit: None,
        connection_id: None,
        path: None,
        glob: None,
        url: None,
        branch: None,
        paths: Vec::new(),
        max_commits: None,
        max_issues: None,
        max_prs: None,
        query: None,
        since_days: None,
        max_items: None,
        selector: None,
        max_tokens_per_sync: None,
        max_cost_per_sync_usd: None,
        sync_depth_days: None,
    }
}
