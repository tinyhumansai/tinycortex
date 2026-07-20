//! Incremental Composio provider pipelines.

mod clickup;
mod common;
mod github;
mod jira;
mod linear;
mod notion;
mod slack;
mod slack_parse;

pub use clickup::ClickUpSyncPipeline;
pub use github::GitHubSyncPipeline;
pub use jira::JiraSyncPipeline;
pub use linear::LinearSyncPipeline;
pub use notion::NotionSyncPipeline;
pub use slack::{SlackSearchBackfillPipeline, SlackSyncPipeline};
