//! [`NullMemoryProvider`] — the reference driver that stores nothing.
//!
//! ## What it is for
//!
//! A memory subsystem that is compiled out, disabled by configuration, or
//! explicitly bound to `driver = "null"` still has to bind *something*: the
//! kernel's registry holds exactly one driver per slot, and code that reaches
//! the slot must find a value rather than an `Option` it has to unwrap at every
//! call site. This is that value. It replaces the hand-written per-domain
//! `stub.rs` files with one generic answer.
//!
//! It is also the fixture the capability-degradation tests bind: with it in the
//! slot, the ten optional families are unadvertised, so their RPC methods are
//! unregistered and their agent tools are absent — and the core still boots.
//!
//! And it is the existence proof for the mandatory set: if
//! [`crate::provider::MemoryCore`], [`crate::provider::MemoryRecall`], and
//! [`crate::provider::MemoryPortability`] could not be implemented without a
//! storage engine, they would be the wrong three to have made mandatory.
//!
//! ## `/dev/null` semantics, and what that costs
//!
//! Writes are **accepted and discarded**; reads return empty. This mirrors the
//! Unix device the driver is named after, and it is the only behaviour that
//! lets the mandatory three be advertised honestly: a `store` that returned
//! [`crate::error::MemoryError::Unsupported`] would contradict advertising
//! [`crate::capabilities::Capability::Core`], and one that returned a hard
//! error would turn every optional auto-capture into a user-visible failure.
//!
//! The cost is real: content written here is gone. That is acceptable for a
//! subsystem the operator turned off, and unacceptable as a fallback for a
//! driver that failed to bind — **that** case falls back to the embedded
//! default, never to this. Do not wire it as a general-purpose failure mode.
//!
//! ## Why it implements all thirteen families but advertises three
//!
//! The ten optional families are implemented and every method returns
//! [`crate::error::MemoryError::Unsupported`] naming its family, but the
//! `as_*` accessors return `None` and
//! [`crate::provider::MemoryProvider::capabilities`] lists only the mandatory
//! three. So:
//!
//! - through `&dyn MemoryProvider` — the only way product code sees a driver —
//!   an unadvertised family is simply **unreachable**, which is the intended
//!   degradation;
//! - through the concrete type, a direct call yields a typed, *named*
//!   `Unsupported` error, which is what makes the contract's error mapping
//!   testable without writing a second mock.
//!
//! [`crate::provider::audit_provider`] confirms the two views agree.

use async_trait::async_trait;

use crate::capabilities::{Capabilities, Capability};
use crate::error::MemoryError;
use crate::goals::GoalsDoc;
use crate::health::MemoryHealth;
use crate::provider::types::{
    DiffReport, EntityHit, ExportPage, ExportRecord, ImportOutcome, IngestItem, IngestOutcome,
    MaintenanceReport, SnapshotRef, SourceItem, SourceScope,
};
use crate::provider::{
    MemoryCore, MemoryDiff, MemoryDocuments, MemoryEntities, MemoryGoals, MemoryGraph,
    MemoryIngest, MemoryMaintenance, MemoryPortability, MemoryProvider, MemoryRecall,
    MemorySourceSink, MemoryToolMemory, MemoryTree,
};
use crate::recall::OwnedRecallOpts;
use crate::tool_memory::ToolMemoryRule;
use crate::tree::{IngestRequest, QueryResult, TreeStatus};
use crate::types::{
    GraphRelationRecord, MemoryCategory, MemoryEntry, MemoryKvRecord, MemoryTaint,
    NamespaceDocumentInput, NamespaceRetrievalContext, NamespaceSummary, StoredMemoryDocument,
};

/// The [`driver_id`](MemoryProvider::driver_id) this driver reports.
pub const NULL_DRIVER_ID: &str = "null";

/// Shorthand for the `Unsupported` error every unadvertised family returns.
fn unsupported<T>(capability: Capability) -> Result<T, MemoryError> {
    Err(MemoryError::unsupported(capability))
}

/// A driver that accepts every write, discards it, and returns nothing.
///
/// See the module documentation for what it is for, why writes are silently
/// dropped, and why it implements ten families it does not advertise.
#[derive(Debug, Clone, Copy, Default)]
pub struct NullMemoryProvider;

impl NullMemoryProvider {
    /// Construct the null driver. It holds no state, so every instance is
    /// interchangeable.
    pub const fn new() -> Self {
        Self
    }
}

#[async_trait]
impl MemoryProvider for NullMemoryProvider {
    fn driver_id(&self) -> &str {
        NULL_DRIVER_ID
    }

    /// Exactly the mandatory three. The ten optional families are implemented
    /// below but deliberately not advertised, so they stay unreachable through
    /// the trait object.
    fn capabilities(&self) -> Capabilities {
        Capabilities::mandatory()
    }

    /// Always [`MemoryHealth::Ready`]: a driver with no backing store has
    /// nothing that can be unreachable, and reporting `Degraded` would make
    /// every status view of a deliberately-disabled subsystem look broken.
    async fn health(&self) -> MemoryHealth {
        MemoryHealth::Ready
    }

    // The `as_*` accessors are all left at their `None` defaults: nothing
    // optional is reachable through the trait object. That absence is the whole
    // point of this driver, so overriding any of them would be the bug.
}

#[async_trait]
impl MemoryCore for NullMemoryProvider {
    /// Accepts and discards. See the module docs on `/dev/null` semantics.
    async fn store(
        &self,
        _namespace: &str,
        _key: &str,
        _content: &str,
        _category: MemoryCategory,
        _session_id: Option<&str>,
        _taint: MemoryTaint,
    ) -> Result<(), MemoryError> {
        Ok(())
    }

    async fn get(&self, _namespace: &str, _key: &str) -> Result<Option<MemoryEntry>, MemoryError> {
        Ok(None)
    }

    /// Always `Ok(false)`: nothing was ever stored, so nothing existed to
    /// forget. Consistent with the idempotence the family requires.
    async fn forget(&self, _namespace: &str, _key: &str) -> Result<bool, MemoryError> {
        Ok(false)
    }

    async fn list(
        &self,
        _namespace: Option<&str>,
        _category: Option<&MemoryCategory>,
        _session_id: Option<&str>,
    ) -> Result<Vec<MemoryEntry>, MemoryError> {
        Ok(Vec::new())
    }

    async fn namespaces(&self) -> Result<Vec<NamespaceSummary>, MemoryError> {
        Ok(Vec::new())
    }
}

#[async_trait]
impl MemoryRecall for NullMemoryProvider {
    async fn recall(
        &self,
        _query: &str,
        _limit: usize,
        _opts: &OwnedRecallOpts,
        _scope: Option<&SourceScope>,
    ) -> Result<Vec<MemoryEntry>, MemoryError> {
        Ok(Vec::new())
    }
}

#[async_trait]
impl MemoryPortability for NullMemoryProvider {
    /// One empty, terminal page: no records and no continuation cursor, so a
    /// caller's export loop terminates on the first iteration.
    ///
    /// This driver never issues a cursor (every page is the first and only
    /// page), so any `Some(_)` cursor a caller passes back is necessarily one
    /// this driver did not hand out — reject it rather than silently treating
    /// it as a valid terminal page.
    async fn export_page(
        &self,
        cursor: Option<&str>,
        _limit: usize,
    ) -> Result<ExportPage, MemoryError> {
        if cursor.is_some() {
            return Err(MemoryError::Invalid(
                "null provider does not issue export cursors".into(),
            ));
        }

        Ok(ExportPage::default())
    }

    /// Counts every record as skipped rather than imported. Reporting them as
    /// imported would tell a migration its data landed somewhere it did not.
    async fn import_records(
        &self,
        records: Vec<ExportRecord>,
    ) -> Result<ImportOutcome, MemoryError> {
        Ok(ImportOutcome {
            imported: 0,
            skipped: u32::try_from(records.len()).unwrap_or(u32::MAX),
            failed: 0,
            errors: Vec::new(),
        })
    }
}

#[async_trait]
impl MemoryIngest for NullMemoryProvider {
    async fn ingest_document(&self, _item: IngestItem) -> Result<IngestOutcome, MemoryError> {
        unsupported(Capability::Ingest)
    }

    async fn ingest_chat(&self, _messages: Vec<IngestItem>) -> Result<IngestOutcome, MemoryError> {
        unsupported(Capability::Ingest)
    }
}

#[async_trait]
impl MemoryDocuments for NullMemoryProvider {
    async fn put_document(&self, _input: NamespaceDocumentInput) -> Result<String, MemoryError> {
        unsupported(Capability::Documents)
    }

    async fn get_document(
        &self,
        _namespace: &str,
        _key: &str,
    ) -> Result<Option<StoredMemoryDocument>, MemoryError> {
        unsupported(Capability::Documents)
    }

    async fn query_documents(
        &self,
        _namespace: &str,
        _query: &str,
        _limit: usize,
    ) -> Result<NamespaceRetrievalContext, MemoryError> {
        unsupported(Capability::Documents)
    }
}

#[async_trait]
impl MemoryTree for NullMemoryProvider {
    async fn append(&self, _request: IngestRequest) -> Result<(), MemoryError> {
        unsupported(Capability::Tree)
    }

    async fn query_source(
        &self,
        _namespace: &str,
        _source_id: &str,
        _limit: usize,
        _scope: Option<&SourceScope>,
    ) -> Result<Vec<crate::chunks::Chunk>, MemoryError> {
        unsupported(Capability::Tree)
    }

    async fn drill_down(
        &self,
        _namespace: &str,
        _node_id: &str,
    ) -> Result<QueryResult, MemoryError> {
        unsupported(Capability::Tree)
    }

    async fn seal(&self, _namespace: &str) -> Result<TreeStatus, MemoryError> {
        unsupported(Capability::Tree)
    }

    async fn cascade(&self, _namespace: &str) -> Result<TreeStatus, MemoryError> {
        unsupported(Capability::Tree)
    }
}

#[async_trait]
impl MemoryEntities for NullMemoryProvider {
    async fn entities(
        &self,
        _namespace: &str,
        _query: Option<&str>,
        _limit: usize,
    ) -> Result<Vec<EntityHit>, MemoryError> {
        unsupported(Capability::Entities)
    }

    async fn entity_edges(
        &self,
        _namespace: &str,
        _entity_id: &str,
        _limit: usize,
    ) -> Result<Vec<GraphRelationRecord>, MemoryError> {
        unsupported(Capability::Entities)
    }

    async fn touch_entities(
        &self,
        _namespace: &str,
        _entity_ids: &[String],
    ) -> Result<(), MemoryError> {
        unsupported(Capability::Entities)
    }
}

#[async_trait]
impl MemoryGraph for NullMemoryProvider {
    async fn kv_get(
        &self,
        _namespace: Option<&str>,
        _key: &str,
    ) -> Result<Option<MemoryKvRecord>, MemoryError> {
        unsupported(Capability::Graph)
    }

    async fn kv_put(
        &self,
        _namespace: Option<&str>,
        _key: &str,
        _value: serde_json::Value,
    ) -> Result<(), MemoryError> {
        unsupported(Capability::Graph)
    }

    async fn kv_list(
        &self,
        _namespace: Option<&str>,
        _prefix: Option<&str>,
        _limit: usize,
    ) -> Result<Vec<MemoryKvRecord>, MemoryError> {
        unsupported(Capability::Graph)
    }

    async fn relations(
        &self,
        _namespace: Option<&str>,
        _subject: Option<&str>,
        _predicate: Option<&str>,
        _limit: usize,
    ) -> Result<Vec<GraphRelationRecord>, MemoryError> {
        unsupported(Capability::Graph)
    }

    async fn put_relation(&self, _relation: GraphRelationRecord) -> Result<(), MemoryError> {
        unsupported(Capability::Graph)
    }
}

#[async_trait]
impl MemoryDiff for NullMemoryProvider {
    async fn capture_snapshot(&self, _source_id: &str) -> Result<SnapshotRef, MemoryError> {
        unsupported(Capability::Diff)
    }

    async fn snapshots(
        &self,
        _source_id: &str,
        _limit: usize,
    ) -> Result<Vec<SnapshotRef>, MemoryError> {
        unsupported(Capability::Diff)
    }

    async fn diff(
        &self,
        _source_id: &str,
        _from: Option<&str>,
        _to: &str,
    ) -> Result<DiffReport, MemoryError> {
        unsupported(Capability::Diff)
    }
}

#[async_trait]
impl MemoryGoals for NullMemoryProvider {
    async fn goals(&self) -> Result<GoalsDoc, MemoryError> {
        unsupported(Capability::Goals)
    }

    async fn set_goals(&self, _goals: GoalsDoc) -> Result<(), MemoryError> {
        unsupported(Capability::Goals)
    }
}

#[async_trait]
impl MemoryToolMemory for NullMemoryProvider {
    async fn tool_rules(&self, _tool_name: &str) -> Result<Vec<ToolMemoryRule>, MemoryError> {
        unsupported(Capability::ToolMemory)
    }

    async fn put_tool_rule(&self, _rule: ToolMemoryRule) -> Result<(), MemoryError> {
        unsupported(Capability::ToolMemory)
    }

    async fn delete_tool_rule(
        &self,
        _tool_name: &str,
        _rule_id: &str,
    ) -> Result<bool, MemoryError> {
        unsupported(Capability::ToolMemory)
    }
}

#[async_trait]
impl MemorySourceSink for NullMemoryProvider {
    async fn accept_source_items(
        &self,
        _source_id: &str,
        _source_kind: &str,
        _items: Vec<SourceItem>,
        _taint: MemoryTaint,
    ) -> Result<IngestOutcome, MemoryError> {
        unsupported(Capability::Sources)
    }

    async fn forget_source(&self, _source_id: &str) -> Result<u64, MemoryError> {
        unsupported(Capability::Sources)
    }
}

#[async_trait]
impl MemoryMaintenance for NullMemoryProvider {
    async fn reembed(&self) -> Result<MaintenanceReport, MemoryError> {
        unsupported(Capability::Maintenance)
    }

    async fn compact(&self) -> Result<MaintenanceReport, MemoryError> {
        unsupported(Capability::Maintenance)
    }

    async fn consolidate(&self) -> Result<MaintenanceReport, MemoryError> {
        unsupported(Capability::Maintenance)
    }

    async fn doctor(&self) -> Result<MaintenanceReport, MemoryError> {
        unsupported(Capability::Maintenance)
    }
}

#[cfg(test)]
#[path = "null_tests.rs"]
mod tests;
