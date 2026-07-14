//! Markdown file-based persistence for the time-based summary tree.
//!
//! Each node is a markdown file with YAML frontmatter under
//! `{workspace}/memory/namespaces/{namespace}/tree/`. The folder hierarchy
//! mirrors the time hierarchy (`root.md`, `2024/summary.md`,
//! `2024/03/15/14.md`). Ported from OpenHuman's `tree_runtime/store.rs` with
//! logging stripped and `Config` →
//! [`crate::memory::config::MemoryConfig`]. Split across files to keep each
//! under the repo's 500-line cap: `paths` (path/validation), `nodes` (node
//! read/write + markdown parsing), `scan` (counts/status/collection), and
//! `buffer` (ingestion buffer).

mod buffer;
mod nodes;
mod paths;
mod scan;

pub use buffer::{buffer_delete, buffer_drain, buffer_read, buffer_write};
#[cfg(test)]
pub(crate) use nodes::split_frontmatter;
pub use nodes::{parse_node_markdown_pub, read_ancestors, read_children, read_node, write_node};
pub use paths::{buffer_dir, node_file_path, tree_dir, validate_namespace, validate_node_id};
pub use scan::{
    collect_root_summaries_with_caps, count_nodes, delete_tree, get_tree_status,
    list_namespaces_with_root,
};

#[cfg(test)]
#[path = "store_tests.rs"]
mod tests;
