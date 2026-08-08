//! Provider payload normalisers for hosts that drive Composio through their
//! own provider abstraction, rather than through the [`SyncPipeline`]
//! implementations in this directory's siblings.
//!
//! These are pure `serde_json::Value` → `Value` transforms: given a raw
//! Composio action response, pull out the fields that make up a task, an
//! issue, a page or a message. They hold no credentials, touch no network,
//! and make no scheduling decisions — provider-specific normalisation is
//! driver-side by definition (see the host's `docs/specs/kernel.md` §4).
//!
//! [`SyncPipeline`]: crate::memory::sync::traits::SyncPipeline

pub mod helpers;
