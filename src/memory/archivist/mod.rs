//! Memory archivist — chat conversation → tree leaf.
//!
//! The archivist's one job is to take a chat conversation, strip the noisy
//! tool-call payloads from it, and push the resulting text into a memory tree
//! as a single leaf. The tree owns persistence and retrieval from there on.
//!
//! ## Flow
//!
//! ```text
//!   Vec<Turn>          (raw conversation, tool calls included)
//!         │
//!         ▼
//!   clip::clean()      (strip tool_calls_json; drop "tool" turns)
//!         │
//!         ▼
//!   compose::md()      (one md blob: ## role\n<content>\n\n... per turn)
//!         │
//!         ▼
//!   tree_writer        (append ONE leaf via the injected TreeLeafSink)
//!         │
//!         ▼
//!   TreeLeafSink       (tree append + cascade seal — supplied by caller)
//! ```
//!
//! ## API
//!
//! - [`Turn`] — input shape, one per role/content/tool_calls record.
//! - [`clean_conversation`] — pure transform; returns a `Vec<Turn>` with
//!   tool-call payloads dropped and `tool`-role turns removed.
//! - [`compose_conversation_md`] — pure transform; returns the markdown blob
//!   that will become a single tree leaf.
//! - [`archive_to_tree`] — end-to-end: clean → compose → append leaf to the
//!   injected [`TreeLeafSink`].
//! - [`record_turn`] / [`session_entries`] — the per-turn episodic disk
//!   capture surface (distinct from the batch tree-leaf flow).
//!
//! ## Decoupling from the tree
//!
//! OpenHuman's archivist calls straight into `memory_tree`'s `append_leaf`. In
//! TinyCortex the `tree` module is ported concurrently, so the archivist
//! appends through the small [`TreeLeafSink`] trait instead of hard-depending
//! on tree internals. A tree-backed implementation lives in the `tree` module;
//! [`RecordingSink`] is a zero-IO test implementation.
//!
//! ## Why strip tool calls?
//!
//! Tool-call JSON is verbose, model-specific, and rarely meaningful out of
//! context. Tool-result turns are noisy (stdout dumps, JSON responses) and
//! distort vector embeddings of the surrounding human conversation. Stripping
//! both before the conversation lands in the tree keeps summaries and
//! embeddings focused on natural-language content.

pub mod clip;
pub mod compose;
pub mod sink;
pub mod store;
pub mod tree_writer;
pub mod types;

pub use clip::clean_conversation;
pub use compose::compose_conversation_md;
pub use sink::{LeafMeta, RecordedLeaf, RecordingSink, TreeLeafSink};
pub use store::{record_turn, session_entries};
pub use tree_writer::{archive_to_tree, chunk_id_for_session, ArchiveOutcome};
pub use types::{ArchivedTurn, Turn};
