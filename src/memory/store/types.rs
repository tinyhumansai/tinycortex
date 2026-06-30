//! Core value types for the [`Memory`](crate::memory::Memory) store contract.
//!
//! Defines the records that flow through the store: an untrusted [`MemoryInput`]
//! supplied by callers, the persisted [`MemoryRecord`] minted from it, the
//! [`MemoryQuery`] filter shape, the [`SearchHit`] returned by retrieval, and
//! the [`MemoryError`] failure modes. These are pure data contracts; persistence
//! and retrieval behavior live in sibling store modules.

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use serde_json::{Map, Value};
use thiserror::Error;
use uuid::Uuid;

/// Stable, machine-readable identity for a [`MemoryRecord`]; a v4 UUID.
pub type MemoryId = Uuid;
/// Result alias for fallible store operations, carrying [`MemoryError`].
pub type MemoryResult<T> = Result<T, MemoryError>;

/// Failure modes for memory store operations.
#[derive(Debug, Error, PartialEq, Eq)]
pub enum MemoryError {
    /// No record exists for the requested [`MemoryId`].
    #[error("memory record not found: {0}")]
    NotFound(MemoryId),
    /// Input content was empty (or whitespace-only) after trimming.
    #[error("memory content cannot be empty")]
    EmptyContent,
}

/// Caller-supplied, not-yet-persisted memory item.
///
/// Untrusted by contract: content is validated and trimmed when promoted to a
/// [`MemoryRecord`] via [`MemoryRecord::from_input`].
#[derive(Clone, Debug, Deserialize, Serialize, PartialEq)]
pub struct MemoryInput {
    /// Logical partition the item belongs to; scopes storage and queries.
    pub namespace: String,
    /// Raw item text; trimmed and required non-empty on promotion.
    pub content: String,
    /// Free-form provenance/attributes; defaults to empty when absent.
    #[serde(default)]
    pub metadata: Map<String, Value>,
}

impl MemoryInput {
    /// Builds an input for `namespace`/`content` with empty metadata.
    pub fn new(namespace: impl Into<String>, content: impl Into<String>) -> Self {
        Self {
            namespace: namespace.into(),
            content: content.into(),
            metadata: Map::new(),
        }
    }
}

/// A persisted memory item with assigned identity and timestamps.
#[derive(Clone, Debug, Deserialize, Serialize, PartialEq)]
pub struct MemoryRecord {
    /// Stable record identity, minted on creation.
    pub id: MemoryId,
    /// Logical partition the record belongs to.
    pub namespace: String,
    /// Stored item text; trimmed and guaranteed non-empty.
    pub content: String,
    /// Free-form provenance/attributes carried from the input.
    #[serde(default)]
    pub metadata: Map<String, Value>,
    /// UTC instant the record was first created.
    pub created_at: DateTime<Utc>,
    /// UTC instant of the last update; equals `created_at` on creation.
    pub updated_at: DateTime<Utc>,
}

impl MemoryRecord {
    /// Promotes a [`MemoryInput`] into a persisted record.
    ///
    /// Trims `content` and rejects empty results with
    /// [`MemoryError::EmptyContent`]. Mints a fresh [`MemoryId`] and stamps both
    /// timestamps with the current UTC instant.
    pub fn from_input(input: MemoryInput) -> MemoryResult<Self> {
        let content = input.content.trim().to_owned();
        if content.is_empty() {
            return Err(MemoryError::EmptyContent);
        }

        let now = Utc::now();
        Ok(Self {
            id: Uuid::new_v4(),
            namespace: input.namespace,
            content,
            metadata: input.metadata,
            created_at: now,
            updated_at: now,
        })
    }
}

/// Filter shape for retrieving records; all fields are optional and combine
/// conjunctively, with `None` meaning "unconstrained".
#[derive(Clone, Debug, Default, Deserialize, Serialize, PartialEq, Eq)]
pub struct MemoryQuery {
    /// Restrict to a single namespace; `None` searches all.
    pub namespace: Option<String>,
    /// Free-text match term; `None` matches regardless of content.
    pub text: Option<String>,
    /// Maximum number of hits to return; `None` leaves it to the store.
    pub limit: Option<usize>,
}

impl MemoryQuery {
    /// Builds a query matching `text` with no namespace or limit constraint.
    pub fn text(text: impl Into<String>) -> Self {
        Self {
            text: Some(text.into()),
            ..Self::default()
        }
    }
}

/// A retrieval result pairing a matched record with its relevance score.
#[derive(Clone, Debug, Deserialize, Serialize, PartialEq)]
pub struct SearchHit {
    /// The matched record.
    pub record: MemoryRecord,
    /// Relevance score; higher is more relevant. Scale is store-defined.
    pub score: f32,
}
