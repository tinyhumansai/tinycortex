//! Live source synchronization engine.

pub mod audit;
pub mod composio;
pub mod dispatcher;
pub mod github;
pub mod periodic;
pub mod persist;
pub mod rebuild;
pub mod state;
pub mod status;
pub mod traits;
pub mod workspace;

pub use audit::{
    append_audit_entry, estimate_cost_usd, read_audit_log, RealCostAccumulator, SyncAuditEntry,
};
pub use composio::{
    ClickUpSyncPipeline, ComposioClient, GitHubSyncPipeline, GmailSyncPipeline, LinearSyncPipeline,
    NotionSyncPipeline, SlackSearchBackfillPipeline, SlackSyncPipeline,
};
pub use dispatcher::{SyncDispatcher, SyncRunResult};
pub use github::GithubRepoSyncPipeline;
pub use periodic::{due_workspace_sources, effective_interval_secs, DEFAULT_SYNC_INTERVAL_SECS};
pub use persist::{KvSkillDocSink, SKILLDOC_NS_PREFIX, SKILL_DOCS_DB};
pub use rebuild::{
    needs_rebuild, raw_coverage, rebuild_tree_from_raw, rebuild_tree_from_raw_with_audit,
    RawCoverage, RawFileRef, RebuildOutcome,
};
pub use state::{DailyBudget, SyncState, SyncStateStore};
pub use status::{list_sync_statuses, FreshnessLabel, MemorySyncStatus, StatusListResponse};
pub use traits::{
    ExternalSourceReader, LocalDocument, LocalDocumentSink, SkillDocSink, SkillDocument,
    SyncContext, SyncEvent, SyncEventSink, SyncOutcome, SyncPipeline, SyncPipelineKind,
    SyncRunError, SyncStage,
};
pub use workspace::WorkspaceSourcePipeline;
