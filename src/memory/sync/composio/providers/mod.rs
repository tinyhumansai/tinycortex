//! Incremental Composio provider pipelines.

mod clickup;
mod common;
mod github;
mod google_calendar;
mod google_drive;
mod linear;
mod notion;
mod slack;
mod slack_parse;

pub use clickup::ClickUpSyncPipeline;
pub use github::GitHubSyncPipeline;
pub use google_calendar::GoogleCalendarSyncPipeline;
pub use google_drive::GoogleDriveSyncPipeline;
pub use linear::LinearSyncPipeline;
pub use notion::NotionSyncPipeline;
pub use slack::{SlackSearchBackfillPipeline, SlackSyncPipeline};
