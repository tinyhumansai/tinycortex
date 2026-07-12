//! Composio-backed synchronization.

pub mod client;
pub mod gmail;
pub mod orchestrator;
pub mod providers;

pub use client::{ActionExecutor, ComposioClient, ExecuteError, ExecuteResponse};
pub use gmail::GmailSyncPipeline;
pub use orchestrator::{run_incremental_sync, IncrementalSource, PageFetch, SyncItem, SyncScope};
pub use providers::{
    ClickUpSyncPipeline, GitHubSyncPipeline, LinearSyncPipeline, NotionSyncPipeline,
    SlackSearchBackfillPipeline, SlackSyncPipeline,
};
