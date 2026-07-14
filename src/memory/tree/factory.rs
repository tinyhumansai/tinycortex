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
use crate::memory::tree::registry::get_or_create_tree_with_ask;
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
    /// Ask-driven flavoured tree: scope is the ask slug; every seal is steered
    /// by the tree's persisted `ask` and the root compiles into a prompt-ready
    /// markdown profile. See [`TreeFactory::flavoured`].
    Flavoured,
}

/// Factory/config object for one tree instance.
#[derive(Debug, Clone)]
pub struct TreeFactory<'a> {
    profile: TreeProfile,
    scope: Cow<'a, str>,
    /// Natural-language ask stamped on a freshly-created [`TreeProfile::Flavoured`]
    /// tree. `None` for every other profile.
    ask: Option<Cow<'a, str>>,
}

impl<'a> TreeFactory<'a> {
    /// Build a [`TreeProfile::Source`] factory scoped to `scope` (a source id).
    pub fn source(scope: impl Into<Cow<'a, str>>) -> Self {
        Self {
            profile: TreeProfile::Source,
            scope: scope.into(),
            ask: None,
        }
    }

    /// Build a [`TreeProfile::Topic`] factory scoped to `scope` (a topic).
    pub fn topic(scope: impl Into<Cow<'a, str>>) -> Self {
        Self {
            profile: TreeProfile::Topic,
            scope: scope.into(),
            ask: None,
        }
    }

    /// Build the singleton [`TreeProfile::Global`] factory, scoped to
    /// [`GLOBAL_SCOPE`].
    pub fn global() -> Self {
        Self {
            profile: TreeProfile::Global,
            scope: Cow::Borrowed(GLOBAL_SCOPE),
            ask: None,
        }
    }

    /// Build a [`TreeProfile::Flavoured`] factory scoped to the ask slug `scope`
    /// (e.g. `"tweet-style"`), carrying the full natural-language `ask` that
    /// steers every summarisation step (e.g. "Distill the author's tweet-writing
    /// style: voice, tone, structure, vocabulary, punctuation habits, dos and
    /// don'ts.").
    ///
    /// The ask is persisted on the tree row the first time the tree is created;
    /// re-instantiating a factory for a `(Flavoured, scope)` that already exists
    /// returns the live row and leaves its stored ask untouched.
    pub fn flavoured(scope: impl Into<Cow<'a, str>>, ask: impl Into<Cow<'a, str>>) -> Self {
        Self {
            profile: TreeProfile::Flavoured,
            scope: scope.into(),
            ask: Some(ask.into()),
        }
    }

    /// Reconstruct the factory matching an existing [`Tree`] row, deriving the
    /// profile from its [`TreeKind`] and reusing its scope. For flavoured trees
    /// the row's persisted `ask` is carried back onto the factory.
    pub fn from_tree(tree: &'a Tree) -> Self {
        match tree.kind {
            TreeKind::Source => Self::source(tree.scope.as_str()),
            TreeKind::Topic => Self::topic(tree.scope.as_str()),
            TreeKind::Global => Self::global(),
            TreeKind::Flavoured => Self {
                profile: TreeProfile::Flavoured,
                scope: Cow::Borrowed(tree.scope.as_str()),
                ask: tree.ask.as_deref().map(Cow::Borrowed),
            },
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
            TreeProfile::Flavoured => TreeKind::Flavoured,
        }
    }

    /// The canonical scope string identifying this tree within its kind.
    pub fn scope(&self) -> &str {
        self.scope.as_ref()
    }

    /// The natural-language ask carried by a flavoured factory, or `None` for
    /// every other profile.
    pub fn ask(&self) -> Option<&str> {
        self.ask.as_deref()
    }

    /// Default seal-time label strategy. Source trees re-extract entities/topics
    /// from synthesised summary text; topic/global trees leave labels empty
    /// (their scope already pins the dominant theme).
    pub fn label_strategy(&self) -> LabelStrategy {
        match self.kind() {
            TreeKind::Source => {
                LabelStrategy::ExtractFromContent(Arc::new(CompositeExtractor::regex_only()))
            }
            TreeKind::Topic | TreeKind::Global | TreeKind::Flavoured => LabelStrategy::Empty,
        }
    }

    /// Look up or create the tree row in the database. For a flavoured factory
    /// the [`ask`](Self::ask) is stamped on the row the first time it is created.
    pub fn get_or_create(&self, config: &MemoryConfig) -> Result<Tree> {
        get_or_create_tree_with_ask(config, self.kind(), self.scope(), self.ask())
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
    ///
    /// Calls [`force_flush_tree`], which always forces the seal: a non-empty
    /// L0 buffer is sealed even when it is still under its token budget
    /// (the disconnect case). Sealing an already-empty buffer is a no-op.
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
