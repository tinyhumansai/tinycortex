//! Incremental Composio provider pipelines.

mod clickup;
mod common;
mod github;
mod google_calendar;
mod google_docs;
mod google_drive;
mod google_sheets;
mod linear;
mod notion;
mod slack;
mod slack_parse;
mod todoist;

pub use clickup::ClickUpSyncPipeline;
pub use github::GitHubSyncPipeline;
pub use google_calendar::GoogleCalendarSyncPipeline;
pub use google_docs::GoogleDocsSyncPipeline;
pub use google_drive::GoogleDriveSyncPipeline;
pub use google_sheets::GoogleSheetsSyncPipeline;
pub use linear::LinearSyncPipeline;
pub use notion::NotionSyncPipeline;
pub use slack::{SlackSearchBackfillPipeline, SlackSyncPipeline};
pub use todoist::TodoistSyncPipeline;
