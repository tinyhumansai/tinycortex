//! Capability families a memory driver may advertise, and the set type used to
//! negotiate them.
//!
//! ## Why capabilities exist
//!
//! A memory driver is not required to implement the whole surface. The kernel
//! asks a driver which families it supports **once**, at bind time, caches the
//! answer, and then unregisters the RPC methods and omits the agent tools that
//! belong to an unadvertised family. Absence beats a registered handler that
//! returns "not implemented": a present-but-failing method teaches a model that
//! the capability exists and makes it retry.
//!
//! Calling an unadvertised capability is therefore a *kernel* bug, not a driver
//! error. [`crate::error::MemoryError::Unsupported`] exists for the one case the
//! kernel cannot pre-empt: an out-of-process driver that answers `501` for a
//! family its handshake claimed.
//!
//! ## Mandatory families
//!
//! [`Capability::Core`], [`Capability::Recall`], and [`Capability::Portability`]
//! are mandatory. Without core and recall a driver is not a memory backend at
//! all; without portability a user cannot leave it, which makes the binding a
//! one-way door. [`Capabilities::validate`] is the single place that rule is
//! encoded — call it at bind time and refuse the bind on `Err`.
//!
//! ## Wire stability
//!
//! The set crosses the process boundary in the driver handshake
//! (`POST /v1/handshake` → `{ contract_version, driver_id, capabilities[] }`),
//! so the serialized form is a JSON **array of stable snake_case strings**, not
//! discriminant integers — inserting a variant in the middle of the enum must
//! not silently re-map an already-deployed driver's advertised set.
//! [`Capability::as_str`] is the authority for those strings and is pinned
//! against the serde derive by a test.
//!
//! ## Deliberately not `#[non_exhaustive]`
//!
//! Adding a family is a [`crate::CONTRACT_VERSION`] **minor** bump and should
//! break every exhaustive `match` in every host that filters registration by
//! family — that compile error is the mechanism which guarantees the new family
//! is actually wired somewhere. Marking this enum `#[non_exhaustive]` would
//! convert that compile-time guarantee into a silent fall-through at the crate
//! boundary (the failure mode recorded for `DataSource` during the M0
//! carve-out). If a future family must be added without breaking downstream
//! matches, bump the **major** version instead.

use serde::{Deserialize, Serialize};
use thiserror::Error;

use crate::error::MemoryError;

/// One capability family a memory driver may advertise.
///
/// The variants are exactly the thirteen families of the memory contract. Each
/// maps to a trait family in the contract, a group of RPC methods, and a group
/// of agent tools; a driver that does not advertise a family simply has that
/// surface absent.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Ord, PartialOrd, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Capability {
    /// Store / get / forget / list / namespaces. **Mandatory.**
    Core,
    /// Ranked retrieval for a query. **Mandatory.**
    Recall,
    /// Document and chat ingestion — the driver owns chunking and embedding.
    Ingest,
    /// The namespace-document tier: put / get / query documents.
    Documents,
    /// Summary-tree query, drill-down, seal, and cascade.
    Tree,
    /// Entity index, entity edges, and hotness.
    Entities,
    /// Key/value graph read and write.
    Graph,
    /// Snapshot capture and change computation.
    Diff,
    /// Goal extraction and goal records.
    Goals,
    /// Per-tool learned memory.
    ToolMemory,
    /// Accepting synced source items; the host still owns credentials and
    /// scheduling.
    Sources,
    /// Re-embed, compact, consolidate ("dream"), and doctor.
    Maintenance,
    /// Export and import of the whole store as a stream. **Mandatory.**
    Portability,
}

impl Capability {
    /// Every family, in declaration order.
    ///
    /// Declaration order is also bit order in [`Capabilities`] and iteration
    /// order in its serialized form, so this slice is the single ordering
    /// authority for the whole module.
    pub const ALL: [Capability; 13] = [
        Capability::Core,
        Capability::Recall,
        Capability::Ingest,
        Capability::Documents,
        Capability::Tree,
        Capability::Entities,
        Capability::Graph,
        Capability::Diff,
        Capability::Goals,
        Capability::ToolMemory,
        Capability::Sources,
        Capability::Maintenance,
        Capability::Portability,
    ];

    /// The families a driver must advertise to be bindable at all.
    ///
    /// See the module docs for why these three and not others.
    pub const MANDATORY: [Capability; 3] = [
        Capability::Core,
        Capability::Recall,
        Capability::Portability,
    ];

    /// Every family, in declaration order. Slice form of [`Self::ALL`], for
    /// callers that want to iterate without naming the array length.
    pub fn all() -> &'static [Capability] {
        &Self::ALL
    }

    /// Stable snake_case identifier used on the wire, in config, and in logs.
    ///
    /// This is the authority for the serialized form; the serde derive is
    /// pinned against it by `capability_as_str_matches_serde_representation`.
    /// Changing a string here is a breaking change for every already-deployed
    /// driver and requires a [`crate::CONTRACT_VERSION`] major bump.
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Core => "core",
            Self::Recall => "recall",
            Self::Ingest => "ingest",
            Self::Documents => "documents",
            Self::Tree => "tree",
            Self::Entities => "entities",
            Self::Graph => "graph",
            Self::Diff => "diff",
            Self::Goals => "goals",
            Self::ToolMemory => "tool_memory",
            Self::Sources => "sources",
            Self::Maintenance => "maintenance",
            Self::Portability => "portability",
        }
    }

    /// Parse back from the on-wire form.
    ///
    /// # Errors
    ///
    /// Returns the unrecognised input in an error message. An unknown string is
    /// expected in practice: a driver speaking a newer minor contract version
    /// may advertise a family this build has never heard of. Callers
    /// negotiating a handshake should **skip** unknown families rather than
    /// fail the bind — an unknown family is one this kernel would never call.
    pub fn parse(raw: &str) -> Result<Self, String> {
        Self::ALL
            .iter()
            .copied()
            .find(|cap| cap.as_str() == raw)
            .ok_or_else(|| format!("unknown memory capability: {raw}"))
    }

    /// Whether this family is mandatory for every driver.
    pub fn is_mandatory(self) -> bool {
        Self::MANDATORY.contains(&self)
    }

    /// Position of this family in [`Self::ALL`]; also its bit index in
    /// [`Capabilities`].
    fn index(self) -> u16 {
        match self {
            Self::Core => 0,
            Self::Recall => 1,
            Self::Ingest => 2,
            Self::Documents => 3,
            Self::Tree => 4,
            Self::Entities => 5,
            Self::Graph => 6,
            Self::Diff => 7,
            Self::Goals => 8,
            Self::ToolMemory => 9,
            Self::Sources => 10,
            Self::Maintenance => 11,
            Self::Portability => 12,
        }
    }

    /// Single-bit mask for this family within a [`Capabilities`] set.
    fn bit(self) -> u64 {
        1u64 << self.index()
    }
}

impl std::fmt::Display for Capability {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.as_str())
    }
}

impl std::str::FromStr for Capability {
    type Err = String;

    fn from_str(raw: &str) -> Result<Self, Self::Err> {
        Self::parse(raw)
    }
}

/// A driver's advertised capability set.
///
/// Internally a bitset, so `contains` is a single mask test on the hot path and
/// the type is `Copy`. Externally it serializes as a JSON array of
/// [`Capability::as_str`] strings in [`Capability::ALL`] order — duplicates in
/// the input collapse, and ordering in the input is not preserved, because a
/// set has neither.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
pub struct Capabilities {
    bits: u64,
}

impl Capabilities {
    /// The empty default capability set. The `null` driver advertises
    /// [`Self::mandatory`] via its [`MemoryProvider::capabilities`](crate::provider::MemoryProvider::capabilities)
    /// implementation, not this.
    pub const fn empty() -> Self {
        Self { bits: 0 }
    }

    /// Every family. Advertised by the embedded `tinycortex` driver.
    pub fn all() -> Self {
        Capability::ALL.into_iter().collect()
    }

    /// Exactly the mandatory families — the minimum bindable set.
    pub fn mandatory() -> Self {
        Capability::MANDATORY.into_iter().collect()
    }

    /// Whether `capability` is advertised.
    pub fn contains(&self, capability: Capability) -> bool {
        self.bits & capability.bit() != 0
    }

    /// Whether every family in `other` is advertised here.
    pub fn contains_all(&self, other: Capabilities) -> bool {
        self.bits & other.bits == other.bits
    }

    /// Adds `capability` in place. Idempotent.
    pub fn insert(&mut self, capability: Capability) {
        self.bits |= capability.bit();
    }

    /// Removes `capability` in place. Idempotent.
    pub fn remove(&mut self, capability: Capability) {
        self.bits &= !capability.bit();
    }

    /// Builder form of [`Self::insert`].
    pub fn with(mut self, capability: Capability) -> Self {
        self.insert(capability);
        self
    }

    /// Builder form of [`Self::remove`].
    pub fn without(mut self, capability: Capability) -> Self {
        self.remove(capability);
        self
    }

    /// Advertised families in [`Capability::ALL`] order.
    pub fn iter(&self) -> impl Iterator<Item = Capability> + '_ {
        Capability::ALL
            .into_iter()
            .filter(move |cap| self.contains(*cap))
    }

    /// Number of advertised families.
    pub fn len(&self) -> usize {
        self.bits.count_ones() as usize
    }

    /// Whether no family is advertised.
    pub fn is_empty(&self) -> bool {
        self.bits == 0
    }

    /// Mandatory families this set is missing, in [`Capability::ALL`] order.
    /// Empty when the set is bindable.
    pub fn missing_mandatory(&self) -> Vec<Capability> {
        Capability::MANDATORY
            .into_iter()
            .filter(|cap| !self.contains(*cap))
            .collect()
    }

    /// Rejects a set that is missing any mandatory family.
    ///
    /// Call this at bind time; on `Err` refuse the bind and fall back to the
    /// embedded default rather than binding a driver a user could not leave.
    ///
    /// # Errors
    ///
    /// Returns [`MissingMandatoryCapabilities`] listing **every** missing
    /// mandatory family, not just the first, so the operator sees the whole gap
    /// in one message.
    pub fn validate(&self) -> Result<(), MissingMandatoryCapabilities> {
        let missing = self.missing_mandatory();
        if missing.is_empty() {
            Ok(())
        } else {
            Err(MissingMandatoryCapabilities { missing })
        }
    }
}

impl FromIterator<Capability> for Capabilities {
    fn from_iter<I: IntoIterator<Item = Capability>>(iter: I) -> Self {
        let mut set = Self::empty();
        for capability in iter {
            set.insert(capability);
        }
        set
    }
}

impl Extend<Capability> for Capabilities {
    fn extend<I: IntoIterator<Item = Capability>>(&mut self, iter: I) {
        for capability in iter {
            self.insert(capability);
        }
    }
}

impl Serialize for Capabilities {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: serde::Serializer,
    {
        serializer.collect_seq(self.iter())
    }
}

impl<'de> Deserialize<'de> for Capabilities {
    /// Skips any family string this build does not recognise, rather than
    /// failing the whole deserialize.
    ///
    /// A remote driver speaking a newer minor contract version may advertise a
    /// family this build has never heard of — see [`Capability::parse`] and the
    /// module-level "wire stability" docs. Rejecting the whole handshake on one
    /// unknown string would refuse an otherwise-compatible driver; the correct
    /// behaviour is to drop the family this kernel could never call anyway.
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        let raw = Vec::<String>::deserialize(deserializer)?;
        let families = raw
            .into_iter()
            .filter_map(|family| Capability::parse(&family).ok());
        Ok(families.collect())
    }
}

/// A driver advertised a capability set missing at least one mandatory family.
///
/// Carries the missing families rather than a formatted string so the caller
/// can report them structurally (status RPC, bind-failure event) as well as in
/// a log line.
#[derive(Debug, Clone, PartialEq, Eq, Error)]
#[error(
    "memory driver advertises an incomplete capability set; missing mandatory families: {}",
    .missing.iter().map(|c| c.as_str()).collect::<Vec<_>>().join(", ")
)]
pub struct MissingMandatoryCapabilities {
    /// Mandatory families absent from the advertised set, in
    /// [`Capability::ALL`] order. Never empty.
    pub missing: Vec<Capability>,
}

impl From<MissingMandatoryCapabilities> for MemoryError {
    /// An incomplete advertised set is a caller/config error, not an
    /// unsupported call: the driver said something invalid about itself, which
    /// is why this maps to [`MemoryError::Invalid`] and not
    /// [`MemoryError::Unsupported`].
    fn from(value: MissingMandatoryCapabilities) -> Self {
        MemoryError::Invalid(value.to_string())
    }
}

#[cfg(test)]
#[path = "capabilities_tests.rs"]
mod tests;
