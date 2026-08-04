//! Tests for the advertised-vs-implemented honesty check.
//!
//! Two deliberately dishonest fixtures sit here — one that over-claims and one
//! that under-claims — because the whole value of [`audit_provider`] is
//! catching drivers that disagree with themselves, and neither direction is
//! reachable from an honest driver.

use async_trait::async_trait;

use super::*;
use crate::capabilities::Capabilities;
use crate::health::MemoryHealth;
use crate::null::NullMemoryProvider;
use crate::provider::types::{ExportPage, ExportRecord, ImportOutcome, SourceScope};
use crate::provider::{MemoryCore, MemoryPortability, MemoryRecall, MemoryTree};
use crate::recall::OwnedRecallOpts;
use crate::types::{MemoryCategory, MemoryEntry, MemoryTaint, NamespaceSummary};

/// A provider that forwards the mandatory three to [`NullMemoryProvider`] so
/// each fixture below only has to describe the thing it is lying about.
struct Fixture {
    inner: NullMemoryProvider,
    advertised: Capabilities,
    expose_tree: bool,
}

impl Fixture {
    fn new(advertised: Capabilities, expose_tree: bool) -> Self {
        Self {
            inner: NullMemoryProvider::new(),
            advertised,
            expose_tree,
        }
    }
}

#[async_trait]
impl MemoryCore for Fixture {
    async fn store(
        &self,
        namespace: &str,
        key: &str,
        content: &str,
        category: MemoryCategory,
        session_id: Option<&str>,
        taint: MemoryTaint,
    ) -> Result<(), MemoryError> {
        self.inner
            .store(namespace, key, content, category, session_id, taint)
            .await
    }

    async fn get(&self, namespace: &str, key: &str) -> Result<Option<MemoryEntry>, MemoryError> {
        self.inner.get(namespace, key).await
    }

    async fn forget(&self, namespace: &str, key: &str) -> Result<bool, MemoryError> {
        self.inner.forget(namespace, key).await
    }

    async fn list(
        &self,
        namespace: Option<&str>,
        category: Option<&MemoryCategory>,
        session_id: Option<&str>,
    ) -> Result<Vec<MemoryEntry>, MemoryError> {
        self.inner.list(namespace, category, session_id).await
    }

    async fn namespaces(&self) -> Result<Vec<NamespaceSummary>, MemoryError> {
        self.inner.namespaces().await
    }
}

#[async_trait]
impl MemoryRecall for Fixture {
    async fn recall(
        &self,
        query: &str,
        limit: usize,
        opts: &OwnedRecallOpts,
        scope: Option<&SourceScope>,
    ) -> Result<Vec<MemoryEntry>, MemoryError> {
        self.inner.recall(query, limit, opts, scope).await
    }
}

#[async_trait]
impl MemoryPortability for Fixture {
    async fn export_page(
        &self,
        cursor: Option<&str>,
        limit: usize,
    ) -> Result<ExportPage, MemoryError> {
        self.inner.export_page(cursor, limit).await
    }

    async fn import_records(
        &self,
        records: Vec<ExportRecord>,
    ) -> Result<ImportOutcome, MemoryError> {
        self.inner.import_records(records).await
    }
}

#[async_trait]
impl MemoryProvider for Fixture {
    fn driver_id(&self) -> &str {
        "fixture"
    }

    fn capabilities(&self) -> Capabilities {
        self.advertised
    }

    async fn health(&self) -> MemoryHealth {
        MemoryHealth::Ready
    }

    fn as_tree(&self) -> Option<&dyn MemoryTree> {
        if self.expose_tree {
            Some(&self.inner)
        } else {
            None
        }
    }
}

#[test]
fn honest_driver_passes_the_audit() {
    let honest = Fixture::new(Capabilities::mandatory().with(Capability::Tree), true);
    assert_eq!(audit_provider(&honest), Ok(()));
}

#[test]
fn over_claiming_driver_is_reported_as_advertised_but_absent() {
    // Advertises everything, exposes no optional accessor. Every one of the ten
    // optional families would fail on first call — the exact
    // registered-but-failing outcome the capability filter exists to prevent.
    let liar = Fixture::new(Capabilities::all(), false);

    let audit = audit_provider(&liar).expect_err("over-claiming driver must fail the audit");
    assert_eq!(audit.present_but_unadvertised, Vec::new());
    assert_eq!(audit.advertised_but_absent.len(), 10);
    assert!(audit.advertised_but_absent.contains(&Capability::Tree));
    // The mandatory three are supertraits, so they can never be missing.
    assert!(!audit.advertised_but_absent.contains(&Capability::Core));
    assert!(!audit.advertised_but_absent.contains(&Capability::Recall));
    assert!(!audit
        .advertised_but_absent
        .contains(&Capability::Portability));
}

#[test]
fn under_claiming_driver_is_reported_as_present_but_unadvertised() {
    // Implements the tree but forgot to list it: the family works and is
    // completely unreachable, because the kernel filters from the advertised
    // set.
    let shy = Fixture::new(Capabilities::mandatory(), true);

    let audit = audit_provider(&shy).expect_err("under-claiming driver must fail the audit");
    assert_eq!(audit.advertised_but_absent, Vec::new());
    assert_eq!(audit.present_but_unadvertised, vec![Capability::Tree]);
}

#[test]
fn audit_findings_are_reported_in_declaration_order() {
    let liar = Fixture::new(Capabilities::all(), false);
    let audit = audit_provider(&liar).expect_err("expected a mismatch");

    let declaration_order: Vec<Capability> = Capability::ALL
        .into_iter()
        .filter(|cap| audit.advertised_but_absent.contains(cap))
        .collect();
    assert_eq!(audit.advertised_but_absent, declaration_order);
}

#[test]
fn audit_error_names_every_mismatched_family_and_maps_to_invalid() {
    let liar = Fixture::new(Capabilities::all(), false);
    let audit = audit_provider(&liar).expect_err("expected a mismatch");

    let rendered = audit.to_string();
    for capability in &audit.advertised_but_absent {
        assert!(
            rendered.contains(capability.as_str()),
            "audit message must name {capability}: {rendered}"
        );
    }

    // A driver lying about itself is a config/implementation error, not an
    // unsupported call.
    let error: MemoryError = audit.into();
    assert!(matches!(error, MemoryError::Invalid(_)));
}
