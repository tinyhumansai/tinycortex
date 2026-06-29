use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use serde_json::{Map, Value};
use thiserror::Error;
use uuid::Uuid;

pub type MemoryId = Uuid;
pub type MemoryResult<T> = Result<T, MemoryError>;

#[derive(Debug, Error, PartialEq, Eq)]
pub enum MemoryError {
    #[error("memory record not found: {0}")]
    NotFound(MemoryId),
    #[error("memory content cannot be empty")]
    EmptyContent,
}

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq)]
pub struct MemoryInput {
    pub namespace: String,
    pub content: String,
    #[serde(default)]
    pub metadata: Map<String, Value>,
}

impl MemoryInput {
    pub fn new(namespace: impl Into<String>, content: impl Into<String>) -> Self {
        Self {
            namespace: namespace.into(),
            content: content.into(),
            metadata: Map::new(),
        }
    }
}

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq)]
pub struct MemoryRecord {
    pub id: MemoryId,
    pub namespace: String,
    pub content: String,
    #[serde(default)]
    pub metadata: Map<String, Value>,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
}

impl MemoryRecord {
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

#[derive(Clone, Debug, Default, Deserialize, Serialize, PartialEq, Eq)]
pub struct MemoryQuery {
    pub namespace: Option<String>,
    pub text: Option<String>,
    pub limit: Option<usize>,
}

impl MemoryQuery {
    pub fn text(text: impl Into<String>) -> Self {
        Self {
            text: Some(text.into()),
            ..Self::default()
        }
    }
}

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq)]
pub struct SearchHit {
    pub record: MemoryRecord,
    pub score: f32,
}
