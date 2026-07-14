//! Compiled root artifact for ask-driven flavoured trees (issue #68).
//!
//! A [`TreeKind::Flavoured`] tree distills everything ingested into it through
//! the lens of its persisted `ask` (see [`crate::memory::tree::TreeFactory::flavoured`]).
//! The *deliverable* is the root of that tree compiled into a single, small,
//! prompt-ready markdown file: a style guide / preference profile a host can
//! inject verbatim into a system prompt.
//!
//! [`compile_flavoured_root`] fetches the tree's current root [`SummaryNode`],
//! clamps its body to [`TreeConfig::flavour_root_token_budget`] tokens, wraps it
//! in light front-matter (ask, tree id, scope, sealed-at, evidence changelog),
//! and stages it at a stable, overwritten-in-place path
//! (`flavoured/<scope_slug>.md`) so hosts can read a fixed location. The engine
//! recompiles it after any seal that touches the root (see the hook in
//! [`crate::memory::tree::bucket_seal::cascade_all_from_with_services`]).
//!
//! Unlike the per-node summary staging in [`crate::memory::store::content`], the
//! compiled root is *not* tracked in SQLite — it is a pure projection of the
//! root node, safe to delete and regenerate at any time.

use std::path::PathBuf;

use anyhow::{Context, Result};

use crate::memory::chunks::content_root;
use crate::memory::config::MemoryConfig;
use crate::memory::store::content::slugify_source_id;
use crate::memory::tree::store::{self, TreeKind};
use crate::memory::tree::summarise::clamp_to_budget;

/// Top-level directory (under the content root) holding compiled flavoured-root
/// artifacts, one `.md` per flavoured tree scope.
pub const FLAVOURED_ROOT_DIR: &str = "flavoured";

/// Relative content path (forward-slash) of the compiled root artifact for a
/// flavoured tree `scope`. Stable and overwritten in place across recompiles so
/// hosts can read a fixed location.
pub fn flavoured_root_rel_path(scope: &str) -> String {
    format!("{FLAVOURED_ROOT_DIR}/{}.md", slugify_source_id(scope))
}

/// Absolute path of the compiled root artifact for `scope` within `config`'s
/// content root.
pub fn flavoured_root_abs_path(config: &MemoryConfig, scope: &str) -> PathBuf {
    content_root(config)
        .join(FLAVOURED_ROOT_DIR)
        .join(format!("{}.md", slugify_source_id(scope)))
}

/// Compile the current root of a flavoured tree into a `≤ budget`-token markdown
/// profile, stage it at [`flavoured_root_rel_path`] (overwriting any prior
/// version in place), and return the full markdown (front-matter + body).
///
/// The body is the tree's root [`SummaryNode`] content clamped to
/// [`TreeConfig::flavour_root_token_budget`](crate::memory::config::TreeConfig::flavour_root_token_budget)
/// tokens. Before the first seal (`root_id == None`) the body is empty; the
/// artifact is still written so hosts always find the fixed path.
///
/// # Errors
/// Returns `Err` if `tree_id` does not exist, if it is not a
/// [`TreeKind::Flavoured`] tree, if the root node cannot be read, or if the
/// artifact cannot be written to disk.
pub fn compile_flavoured_root(config: &MemoryConfig, tree_id: &str) -> Result<String> {
    let tree = store::get_tree(config, tree_id)?
        .ok_or_else(|| anyhow::anyhow!("no tree with id {tree_id}"))?;
    if tree.kind != TreeKind::Flavoured {
        anyhow::bail!(
            "compile_flavoured_root: tree {tree_id} is {}, not flavoured",
            tree.kind.as_str()
        );
    }

    // Pull the root node body (empty until the first seal emits an L1 node).
    let (body, leaves_folded, root_id) = match tree.root_id.as_deref() {
        Some(root_id) => {
            let node = store::get_summary(config, root_id)?.ok_or_else(|| {
                anyhow::anyhow!("flavoured tree {tree_id} root {root_id} missing")
            })?;
            (
                node.content,
                node.child_ids.len(),
                Some(root_id.to_string()),
            )
        }
        None => (String::new(), 0, None),
    };

    let budget = config.tree.flavour_root_token_budget;
    let (clamped, token_estimate) = clamp_to_budget(body.trim(), budget);

    let markdown = render_flavoured_root(&FlavouredRootMeta {
        tree_id: &tree.id,
        scope: &tree.scope,
        ask: tree.ask.as_deref(),
        root_id: root_id.as_deref(),
        sealed_at: tree.last_sealed_at.map(|t| t.to_rfc3339()),
        leaves_folded,
        token_estimate,
        token_budget: budget,
        body: &clamped,
    });

    let abs = flavoured_root_abs_path(config, &tree.scope);
    if let Some(parent) = abs.parent() {
        std::fs::create_dir_all(parent)
            .with_context(|| format!("create flavoured root dir {}", parent.display()))?;
    }
    crate::memory::fsutil::atomic_write(&abs, markdown.as_bytes())
        .with_context(|| format!("write flavoured root {}", abs.display()))?;

    Ok(markdown)
}

/// Front-matter inputs for one compiled flavoured-root artifact.
struct FlavouredRootMeta<'a> {
    tree_id: &'a str,
    scope: &'a str,
    ask: Option<&'a str>,
    root_id: Option<&'a str>,
    sealed_at: Option<String>,
    leaves_folded: usize,
    token_estimate: u32,
    token_budget: u32,
    body: &'a str,
}

/// Render the compiled artifact: YAML front-matter followed by the clamped body.
fn render_flavoured_root(meta: &FlavouredRootMeta<'_>) -> String {
    let mut out = String::new();
    out.push_str("---\n");
    out.push_str("kind: flavoured_root\n");
    out.push_str(&format!("tree_id: {}\n", yaml_quote(meta.tree_id)));
    out.push_str(&format!("scope: {}\n", yaml_quote(meta.scope)));
    out.push_str(&format!("ask: {}\n", yaml_quote(meta.ask.unwrap_or(""))));
    match meta.root_id {
        Some(id) => out.push_str(&format!("root_id: {}\n", yaml_quote(id))),
        None => out.push_str("root_id: null\n"),
    }
    match &meta.sealed_at {
        Some(ts) => out.push_str(&format!("sealed_at: {}\n", yaml_quote(ts))),
        None => out.push_str("sealed_at: null\n"),
    }
    out.push_str(&format!("leaves_folded: {}\n", meta.leaves_folded));
    out.push_str(&format!("token_estimate: {}\n", meta.token_estimate));
    out.push_str(&format!("token_budget: {}\n", meta.token_budget));
    out.push_str("---\n");
    out.push_str(meta.body);
    if !meta.body.is_empty() && !meta.body.ends_with('\n') {
        out.push('\n');
    }
    out
}

/// Minimal YAML double-quoted scalar. Collapses interior newlines to spaces
/// (asks are single-instruction strings) and escapes `\` and `"` so the value
/// is always a valid one-line YAML scalar.
fn yaml_quote(value: &str) -> String {
    let collapsed: String = value.split_whitespace().collect::<Vec<_>>().join(" ");
    let escaped = collapsed.replace('\\', "\\\\").replace('"', "\\\"");
    format!("\"{escaped}\"")
}

#[cfg(test)]
#[path = "flavoured_tests.rs"]
mod tests;
