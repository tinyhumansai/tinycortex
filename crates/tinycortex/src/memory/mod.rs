//! Memory contracts and storage primitives.

pub mod store;
pub mod types;

pub use store::{InMemoryMemoryStore, MemoryStore};
pub use types::{
    MemoryError, MemoryId, MemoryInput, MemoryQuery, MemoryRecord, MemoryResult, SearchHit,
};
