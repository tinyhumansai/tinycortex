//! Recall filter contracts — the borrowed engine form and the owned
//! contract/wire form, kept side by side so they cannot drift.
//!
//! ## Why there are two
//!
//! [`RecallOpts`] is the historical, engine-facing shape: it borrows its string
//! filters so a hot retrieval path allocates nothing. That makes it unusable as
//! a contract type in two independent ways — it derives no serde impls, so it
//! cannot be a `POST /v1/memory/recall` request body, and its lifetime
//! parameter would have to be threaded through every `#[async_trait]` recall
//! method, which destroys the object safety the whole driver model rests on.
//!
//! [`OwnedRecallOpts`] is the answer: the same five fields, owned, serde-
//! derived. Contract and wire paths use the owned form; the engine path keeps
//! the borrowed one and converts at the boundary via
//! `RecallOpts::from(&owned)`, which is zero-copy for the string fields.
//!
//! ## Field parity is the contract
//!
//! A field added to one form and not the other is a silent contract hole: the
//! wire would accept a filter the engine never applies, or the engine would
//! offer a filter no remote driver can be told about. Two defences are in
//! place, and both must stay:
//!
//! 1. Both [`From`] impls **exhaustively destructure** their source, so adding
//!    a field to either struct without handling it fails to compile.
//! 2. `owned_and_borrowed_recall_opts_have_identical_fields` in
//!    `recall_tests.rs` round-trips a fully non-default value through both
//!    directions, so a field that is merely *dropped* during conversion fails
//!    the test.
//!
//! Both types live in this module (rather than in `types.rs`) precisely so the
//! pair is read and edited together. They are re-exported from
//! [`crate::types`], so every historical `types::RecallOpts` path — including
//! the engine crate's `tinycortex::memory::types::` alias — keeps resolving.

use serde::{Deserialize, Serialize};

use crate::types::MemoryCategory;

/// Optional filters for recall — the **borrowed, engine-facing** form.
///
/// Borrows its string filters so an engine call path can pass slices of a
/// caller-owned request without allocating. It is deliberately *not*
/// serializable and deliberately *not* used in the driver contract: a lifetime
/// parameter cannot travel through an object-safe `#[async_trait]` method, and
/// a borrowed struct cannot be a request body.
///
/// Use [`OwnedRecallOpts`] for anything that crosses a trait object or the
/// wire, and convert at the boundary with the [`From`] impl below. The two
/// types carry the same fields; a field added to one and not the other is a
/// silent contract hole, which
/// `owned_and_borrowed_recall_opts_have_identical_fields` exists to catch.
#[derive(Debug, Default, Clone)]
pub struct RecallOpts<'a> {
    /// Restrict recall to this namespace; `None` falls back to [`crate::types::GLOBAL_NAMESPACE`].
    pub namespace: Option<&'a str>,
    /// Restrict recall to entries of this category.
    pub category: Option<MemoryCategory>,
    /// Restrict recall to entries scoped to this session.
    pub session_id: Option<&'a str>,
    /// Drop hits scoring below this threshold (typically 0.0–1.0).
    pub min_score: Option<f64>,
    /// When `true`, include conversational hits from other sessions in the same
    /// workspace alongside the namespace recall.
    pub cross_session: bool,
}

/// Optional filters for recall — the **owned, contract-facing** form.
///
/// This is the type the driver contract and the JSON wire protocol use. It
/// exists because [`RecallOpts`] cannot serve either role:
///
/// - it derives no `Serialize`/`Deserialize`, so it cannot be a
///   `POST /v1/memory/recall` request body;
/// - it carries a borrow lifetime, which would have to be threaded through
///   every `#[async_trait]` recall method and destroys object safety at the
///   `dyn` boundary the whole driver model rests on.
///
/// The borrowed form stays for engine-internal use so the embedded driver's
/// hot path allocates nothing: build the owned value once at the contract
/// boundary, then hand `RecallOpts::from(&owned)` down.
///
/// Every field is `#[serde(default)]` so a minimal request body — even `{}` —
/// deserializes to the same value as [`Default::default`].
#[derive(Debug, Default, Clone, PartialEq, Serialize, Deserialize)]
pub struct OwnedRecallOpts {
    /// Restrict recall to this namespace; `None` falls back to [`crate::types::GLOBAL_NAMESPACE`].
    #[serde(default)]
    pub namespace: Option<String>,
    /// Restrict recall to entries of this category.
    #[serde(default)]
    pub category: Option<MemoryCategory>,
    /// Restrict recall to entries scoped to this session.
    #[serde(default)]
    pub session_id: Option<String>,
    /// Drop hits scoring below this threshold (typically 0.0–1.0).
    #[serde(default)]
    pub min_score: Option<f64>,
    /// When `true`, include conversational hits from other sessions in the same
    /// workspace alongside the namespace recall.
    #[serde(default)]
    pub cross_session: bool,
}

impl<'a> From<&'a OwnedRecallOpts> for RecallOpts<'a> {
    /// Borrows the owned form for an engine call. Zero-copy for the two string
    /// fields; [`MemoryCategory`] is cloned because it owns a `String` in its
    /// [`MemoryCategory::Custom`] variant and [`RecallOpts`] holds it by value.
    ///
    /// Exhaustively destructures the source so adding a field to
    /// [`OwnedRecallOpts`] without handling it here is a compile error.
    fn from(owned: &'a OwnedRecallOpts) -> Self {
        let OwnedRecallOpts {
            namespace,
            category,
            session_id,
            min_score,
            cross_session,
        } = owned;
        RecallOpts {
            namespace: namespace.as_deref(),
            category: category.clone(),
            session_id: session_id.as_deref(),
            min_score: *min_score,
            cross_session: *cross_session,
        }
    }
}

impl From<RecallOpts<'_>> for OwnedRecallOpts {
    /// Takes ownership of a borrowed form — the direction a transport adapter
    /// needs when turning an engine-shaped call into a request body.
    ///
    /// Exhaustively destructures the source for the same reason as the inverse
    /// impl.
    fn from(borrowed: RecallOpts<'_>) -> Self {
        let RecallOpts {
            namespace,
            category,
            session_id,
            min_score,
            cross_session,
        } = borrowed;
        OwnedRecallOpts {
            namespace: namespace.map(str::to_string),
            category,
            session_id: session_id.map(str::to_string),
            min_score,
            cross_session,
        }
    }
}

#[cfg(test)]
#[path = "recall_tests.rs"]
mod tests;
