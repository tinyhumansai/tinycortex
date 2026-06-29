//! Workspace-backed conversation thread/message storage.
//!
//! Conversations are stored as JSONL files under
//! `<workspace>/memory/conversations/` (rooted at
//! [`MemoryConfig::workspace`](crate::memory::config::MemoryConfig)). Thread
//! metadata is an append-only upsert/delete log in `threads.jsonl`; each
//! thread's messages live in a dedicated JSONL file under `threads/<id>.jsonl`
//! for straightforward inspection and recovery.
//!
//! This is **transcript persistence**, not semantic indexing: it owns the raw
//! thread/message records and a local trigram/CJK-bigram inverted index for
//! cross-thread substring search over those transcripts. The richer
//! summary-tree archival of transcripts lives in `super::archivist`.
//!
//! ## Layout
//!
//! - [`types`] — the on-disk wire types (threads, messages, patches, hits).
//! - [`tokenize`] — multilingual normalization + character n-gram tokenizer.
//! - [`inverted_index`] — in-memory trigram/bigram index over message content.
//! - [`store`] — the JSONL [`ConversationStore`] (append/read/update/delete,
//!   process-wide write serialization, warm-index cache, cross-thread search).
//! - [`bus`] — a channel-persistence subscriber that mirrors inbound/processed
//!   channel turns into the store.
//!
//! ## Ported-from-OpenHuman notes
//!
//! - The process-wide write mutex and per-workspace index cache are static
//!   [`std::sync::LazyLock`]`<`[`parking_lot::Mutex`]`>` (OpenHuman used
//!   `once_cell::sync::Lazy`, which is not a dependency here).
//! - The event bus is abstracted behind the [`ConversationEventBus`] /
//!   [`ChannelEventHandler`] traits and a self-contained [`ChannelEvent`] type,
//!   so this module carries no dependency on the host's channel/event-bus
//!   layer (OpenHuman tied directly into `core::event_bus::DomainEvent`).

mod bus;
mod inverted_index;
mod store;
mod tokenize;
mod types;

pub use bus::{
    register_conversation_persistence_subscriber, ChannelEvent, ChannelEventHandler,
    ConversationEventBus, ConversationPersistenceSubscriber,
};
pub use store::{
    append_message, delete_thread, ensure_thread, get_messages, list_threads, purge_threads,
    update_message, update_thread_labels, update_thread_title, ConversationPurgeStats,
    ConversationStore,
};
pub use types::{
    ConversationMessage, ConversationMessagePatch, ConversationThread, CreateConversationThread,
    CrossThreadHit,
};
