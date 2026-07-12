//! Incremental Composio provider pipelines.

mod clickup;
mod common;
mod github;
mod linear;
mod notion;
mod slack;

pub use clickup::ClickUpSyncPipeline;
pub use github::GitHubSyncPipeline;
pub use linear::LinearSyncPipeline;
pub use notion::NotionSyncPipeline;
pub use slack::{SlackSearchBackfillPipeline, SlackSyncPipeline};
