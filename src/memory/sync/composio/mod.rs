//! Composio-backed synchronization.

pub mod client;
pub mod connect;
pub mod gmail;
pub mod orchestrator;
pub mod providers;

pub use client::{ActionExecutor, ComposioClient, ExecuteError, ExecuteResponse};
pub use connect::{
    create_connection_link, generate_entity_id, get_connection_status, list_auth_configs,
    resolve_auth_config_id, status_is_active, status_is_terminal, ConnectionLink, EntityStore,
};
pub use gmail::GmailSyncPipeline;
pub use orchestrator::{run_incremental_sync, IncrementalSource, PageFetch, SyncItem, SyncScope};
pub use providers::{
    ClickUpSyncPipeline, GitHubSyncPipeline, LinearSyncPipeline, NotionSyncPipeline,
    SlackSearchBackfillPipeline, SlackSyncPipeline, TelegramSyncPipeline,
};
