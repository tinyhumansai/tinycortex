//! Hierarchical time-based markdown summary tree.
//!
//! Organises summaries as a tree: root → year → month → day → hour (leaf). A
//! summarisation job drains buffered raw content, summarises it into the hour
//! leaf, and propagates updated summaries upward. Stored as markdown files under
//! `{workspace}/memory/namespaces/{ns}/tree/`.
//!
//! Ported from OpenHuman's `memory_tree/tree_runtime/`. The LLM is abstracted
//! behind [`engine::Summariser`]; the RPC/CLI/bus/schema surfaces are not ported.

pub mod engine;
pub mod store;
pub mod types;

pub use engine::{
    discover_active_namespaces, rebuild_tree, rebuild_tree_observed, run_summarization,
    run_summarization_observed, RuntimeObserver, Summariser,
};
pub use types::{
    derive_node_ids, derive_parent_id, estimate_tokens, level_from_node_id, node_id_to_path,
    IngestRequest, NodeLevel, QueryResult, TreeNode, TreeStatus,
};
