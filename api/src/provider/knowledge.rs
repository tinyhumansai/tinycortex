//! Optional families that expose *derived structure* over stored memory:
//! [`MemoryEntities`], [`MemoryGraph`], and [`MemoryDiff`].
//!
//! Each is independently optional. A driver may have a key/value graph but no
//! entity index, or track source snapshots without either. The kernel filters
//! RPC registration and agent-tool assembly per family, so an absent family is
//! invisible rather than present-and-failing.
//!
//! As in [`crate::provider::content`], no configuration crosses this boundary:
//! extraction models, hotness decay curves, and snapshot retention are driver
//! concerns and appear in none of these signatures.

use async_trait::async_trait;

use crate::error::MemoryError;
use crate::provider::types::{DiffReport, EntityHit, SnapshotRef};
use crate::types::{GraphRelationRecord, MemoryKvRecord};

/// The entity index: who and what the stored memory is about.
#[async_trait]
pub trait MemoryEntities: Send + Sync {
    /// List entities in a namespace, ranked by hotness when `query` is `None`
    /// and by match quality otherwise.
    ///
    /// # Errors
    ///
    /// Backend failures only; an unknown namespace yields an empty vector.
    async fn entities(
        &self,
        namespace: &str,
        query: Option<&str>,
        limit: usize,
    ) -> Result<Vec<EntityHit>, MemoryError>;

    /// Edges incident to one entity, most relevant first.
    ///
    /// Returns [`GraphRelationRecord`] — the same shape [`MemoryGraph`] uses —
    /// so a caller that has both families does not have to reconcile two edge
    /// representations.
    ///
    /// # Errors
    ///
    /// Backend failures only; an unknown `entity_id` yields an empty vector
    /// rather than [`MemoryError::NotFound`], because "no edges" and "no such
    /// entity" are the same answer to this question.
    async fn entity_edges(
        &self,
        namespace: &str,
        entity_id: &str,
        limit: usize,
    ) -> Result<Vec<GraphRelationRecord>, MemoryError>;

    /// Record that these entities were just observed, updating hotness.
    ///
    /// Separate from the read path because hotness is a *write* the host
    /// triggers at known moments (a turn referenced these entities), not
    /// something a driver should infer from being queried — otherwise merely
    /// browsing the index would reshape ranking.
    ///
    /// # Errors
    ///
    /// Backend failures only. Unknown ids are ignored, not rejected.
    async fn touch_entities(
        &self,
        namespace: &str,
        entity_ids: &[String],
    ) -> Result<(), MemoryError>;
}

/// The key/value and relation graph tier.
///
/// `namespace` is `Option<&str>` throughout: `None` addresses the global,
/// namespace-less slice, matching the storage shape of
/// [`MemoryKvRecord::namespace`] and [`GraphRelationRecord::namespace`].
#[async_trait]
pub trait MemoryGraph: Send + Sync {
    /// Read one key/value record.
    ///
    /// # Errors
    ///
    /// A missing key is `Ok(None)`; `Err` is reserved for backend failures.
    async fn kv_get(
        &self,
        namespace: Option<&str>,
        key: &str,
    ) -> Result<Option<MemoryKvRecord>, MemoryError>;

    /// Upsert one key/value record.
    ///
    /// # Errors
    ///
    /// [`MemoryError::Invalid`] for a rejected key, otherwise backend failures.
    async fn kv_put(
        &self,
        namespace: Option<&str>,
        key: &str,
        value: serde_json::Value,
    ) -> Result<(), MemoryError>;

    /// List key/value records, optionally restricted to a key prefix.
    ///
    /// # Errors
    ///
    /// Backend failures only.
    async fn kv_list(
        &self,
        namespace: Option<&str>,
        prefix: Option<&str>,
        limit: usize,
    ) -> Result<Vec<MemoryKvRecord>, MemoryError>;

    /// Query relations, narrowing by subject and/or predicate.
    ///
    /// Both filters are `None`-able so one method covers "everything about this
    /// subject", "every edge of this type", and "the whole slice", instead of
    /// three near-identical methods.
    ///
    /// # Errors
    ///
    /// Backend failures only.
    async fn relations(
        &self,
        namespace: Option<&str>,
        subject: Option<&str>,
        predicate: Option<&str>,
        limit: usize,
    ) -> Result<Vec<GraphRelationRecord>, MemoryError>;

    /// Upsert one relation, keyed by `(namespace, subject, predicate, object)`.
    ///
    /// # Errors
    ///
    /// [`MemoryError::Invalid`] for a malformed edge, otherwise backend
    /// failures.
    async fn put_relation(&self, relation: GraphRelationRecord) -> Result<(), MemoryError>;
}

/// Snapshot capture and change computation over synced sources.
#[async_trait]
pub trait MemoryDiff: Send + Sync {
    /// Capture a snapshot of one source's current items.
    ///
    /// # Errors
    ///
    /// [`MemoryError::NotFound`] for an unknown `source_id`, otherwise backend
    /// failures.
    async fn capture_snapshot(&self, source_id: &str) -> Result<SnapshotRef, MemoryError>;

    /// List snapshots for one source, newest first.
    ///
    /// # Errors
    ///
    /// Backend failures only; an unknown `source_id` yields an empty vector.
    async fn snapshots(
        &self,
        source_id: &str,
        limit: usize,
    ) -> Result<Vec<SnapshotRef>, MemoryError>;

    /// Compute the change set between two snapshots of one source.
    ///
    /// `from` is `Option<&str>` so the first-ever diff — where there is no
    /// baseline and every item is an addition — is expressible without a
    /// separate method or a sentinel id.
    ///
    /// # Errors
    ///
    /// [`MemoryError::NotFound`] when either snapshot id is unknown, otherwise
    /// backend failures.
    async fn diff(
        &self,
        source_id: &str,
        from: Option<&str>,
        to: &str,
    ) -> Result<DiffReport, MemoryError>;
}
