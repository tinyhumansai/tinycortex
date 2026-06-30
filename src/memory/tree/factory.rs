//! Kind/profile factory for tree instances.
//!
//! Centralises the flavor-specific bits so callers get a uniform API: the
//! underlying [`TreeKind`], canonical scope, default seal-time label strategy,
//! and get-or-create / append / seal helpers. The content-mirror and slug rules
//! from OpenHuman are not ported (content staging is deferred).

use std::borrow::Cow;
use std::sync::Arc;

use anyhow::Result;

use crate::memory::config::MemoryConfig;
use crate::memory::score::extract::CompositeExtractor;
use crate::memory::tree::bucket_seal::{append_leaf, LabelStrategy, LeafRef};
use crate::memory::tree::flush::force_flush_tree;
use crate::memory::tree::registry::get_or_create_tree;
use crate::memory::tree::store::{archive_tree, Tree, TreeKind};
use crate::memory::tree::summarise::Summariser;

/// Literal scope used for the singleton global tree.
pub const GLOBAL_SCOPE: &str = "global";

/// High-level tree profile.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TreeProfile {
    /// Per-source tree: scope is a source id; labels are re-extracted at seal.
    Source,
    /// Per-topic tree: scope is the topic; the scope already pins the theme.
    Topic,
    /// Singleton cross-source tree; scope is always [`GLOBAL_SCOPE`].
    Global,
}

/// Factory/config object for one tree instance.
#[derive(Debug, Clone)]
pub struct TreeFactory<'a> {
    profile: TreeProfile,
    scope: Cow<'a, str>,
}

impl<'a> TreeFactory<'a> {
    /// Build a [`TreeProfile::Source`] factory scoped to `scope` (a source id).
    pub fn source(scope: impl Into<Cow<'a, str>>) -> Self {
        Self {
            profile: TreeProfile::Source,
            scope: scope.into(),
        }
    }

    /// Build a [`TreeProfile::Topic`] factory scoped to `scope` (a topic).
    pub fn topic(scope: impl Into<Cow<'a, str>>) -> Self {
        Self {
            profile: TreeProfile::Topic,
            scope: scope.into(),
        }
    }

    /// Build the singleton [`TreeProfile::Global`] factory, scoped to
    /// [`GLOBAL_SCOPE`].
    pub fn global() -> Self {
        Self {
            profile: TreeProfile::Global,
            scope: Cow::Borrowed(GLOBAL_SCOPE),
        }
    }

    /// Reconstruct the factory matching an existing [`Tree`] row, deriving the
    /// profile from its [`TreeKind`] and reusing its scope.
    pub fn from_tree(tree: &'a Tree) -> Self {
        match tree.kind {
            TreeKind::Source => Self::source(tree.scope.as_str()),
            TreeKind::Topic => Self::topic(tree.scope.as_str()),
            TreeKind::Global => Self::global(),
        }
    }

    /// This factory's high-level [`TreeProfile`].
    pub fn profile(&self) -> TreeProfile {
        self.profile
    }

    /// The underlying [`TreeKind`] wire enum for this profile.
    pub fn kind(&self) -> TreeKind {
        match self.profile {
            TreeProfile::Source => TreeKind::Source,
            TreeProfile::Topic => TreeKind::Topic,
            TreeProfile::Global => TreeKind::Global,
        }
    }

    /// The canonical scope string identifying this tree within its kind.
    pub fn scope(&self) -> &str {
        self.scope.as_ref()
    }

    /// Default seal-time label strategy. Source trees re-extract entities/topics
    /// from synthesised summary text; topic/global trees leave labels empty
    /// (their scope already pins the dominant theme).
    pub fn label_strategy(&self) -> LabelStrategy {
        match self.kind() {
            TreeKind::Source => {
                LabelStrategy::ExtractFromContent(Arc::new(CompositeExtractor::regex_only()))
            }
            TreeKind::Topic | TreeKind::Global => LabelStrategy::Empty,
        }
    }

    /// Look up or create the tree row in the database.
    pub fn get_or_create(&self, config: &MemoryConfig) -> Result<Tree> {
        get_or_create_tree(config, self.kind(), self.scope())
    }

    /// Append one leaf using this profile's default labeling policy.
    pub async fn insert_leaf(
        &self,
        config: &MemoryConfig,
        leaf: &LeafRef,
        summariser: &dyn Summariser,
    ) -> Result<Vec<String>> {
        let tree = self.get_or_create(config)?;
        let strategy = self.label_strategy();
        append_leaf(config, &tree, leaf, summariser, &strategy).await
    }

    /// Force-flush/seal this tree profile's currently loaded tree.
    pub async fn seal_now(
        &self,
        config: &MemoryConfig,
        summariser: &dyn Summariser,
    ) -> Result<Vec<String>> {
        let tree = self.get_or_create(config)?;
        let strategy = self.label_strategy();
        force_flush_tree(config, &tree.id, None, summariser, &strategy).await
    }

    /// Archive this tree profile's current tree.
    pub fn archive(&self, config: &MemoryConfig) -> Result<()> {
        let tree = self.get_or_create(config)?;
        archive_tree(config, &tree.id)
    }
}

#[cfg(test)]
#[path = "factory_tests.rs"]
mod tests;
