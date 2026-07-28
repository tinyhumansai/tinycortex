//! Tests for the reference null driver.
//!
//! These pin three separate contracts:
//!
//! 1. the mandatory-three set is genuinely implementable without a store;
//! 2. an unadvertised family is **unreachable** through the trait object, which
//!    is the degradation behaviour the kernel relies on;
//! 3. a direct call to an unadvertised family yields a typed `Unsupported`
//!    error that **names** the family, which is what the transport adapter's
//!    `501` mapping is checked against.
//!
//! ## No async runtime here, on purpose
//!
//! `tinycortex-api` must not depend on tokio (or any executor) — that is the
//! whole point of the crate. Every future in this module completes on its first
//! poll, so a six-line std-only [`block_on`] is sufficient and adds no
//! dependency.

use std::future::Future;
use std::pin::pin;
use std::task::{Context, Poll};

use super::*;
use crate::provider::audit_provider;
use crate::types::MemoryCategory;

/// Drive a future that is ready on first poll to completion, without an
/// executor. Panics rather than spinning if a future ever returns `Pending`,
/// because in this module that would mean a supposedly-inert implementation
/// started doing real work.
fn block_on<F: Future>(future: F) -> F::Output {
    let mut future = pin!(future);
    let mut context = Context::from_waker(std::task::Waker::noop());
    match future.as_mut().poll(&mut context) {
        Poll::Ready(value) => value,
        Poll::Pending => panic!("null driver future must complete on first poll"),
    }
}

#[test]
fn null_driver_advertises_exactly_the_mandatory_families() {
    let driver = NullMemoryProvider::new();
    let capabilities = driver.capabilities();

    assert_eq!(driver.driver_id(), NULL_DRIVER_ID);
    assert_eq!(capabilities.len(), 3);
    for capability in Capability::MANDATORY {
        assert!(
            capabilities.contains(capability),
            "{capability} must be advertised"
        );
    }
}

#[test]
fn null_driver_passes_capability_validation() {
    // The mandatory-three set is the minimum bindable set, so the reference
    // driver must be bindable. If this ever fails, either the mandatory list
    // grew or the null driver stopped implementing it.
    let driver = NullMemoryProvider::new();
    assert_eq!(driver.capabilities().validate(), Ok(()));
}

#[test]
fn null_driver_is_self_consistent() {
    assert_eq!(audit_provider(&NullMemoryProvider::new()), Ok(()));
}

#[test]
fn null_driver_reports_ready() {
    let health = block_on(NullMemoryProvider::new().health());
    assert_eq!(health, MemoryHealth::Ready);
    assert!(health.is_usable());
}

#[test]
fn null_driver_shutdown_is_an_idempotent_no_op() {
    let driver = NullMemoryProvider::new();
    assert!(block_on(driver.shutdown()).is_ok());
    assert!(block_on(driver.shutdown()).is_ok());
}

#[test]
fn mandatory_core_accepts_writes_and_reads_back_empty() {
    let driver = NullMemoryProvider::new();

    block_on(driver.store(
        "global",
        "k",
        "v",
        MemoryCategory::Core,
        None,
        MemoryTaint::ExternalSync,
    ))
    .expect("null store must accept the write");

    assert!(block_on(driver.get("global", "k"))
        .expect("get must succeed")
        .is_none());
    assert!(!block_on(driver.forget("global", "k")).expect("forget must succeed"));
    assert!(block_on(driver.list(None, None, None))
        .expect("list must succeed")
        .is_empty());
    assert!(block_on(driver.namespaces())
        .expect("namespaces must succeed")
        .is_empty());
}

#[test]
fn mandatory_recall_returns_no_hits() {
    let driver = NullMemoryProvider::new();
    let hits = block_on(driver.recall("anything", 10, &OwnedRecallOpts::default(), None))
        .expect("recall must succeed");
    assert!(hits.is_empty());
}

#[test]
fn mandatory_portability_round_trips_as_an_empty_store() {
    let driver = NullMemoryProvider::new();

    let page = block_on(driver.export_page(None, 100)).expect("export must succeed");
    assert!(page.records.is_empty());
    assert!(
        page.next_cursor.is_none(),
        "the absent cursor is what terminates the caller's export loop"
    );

    let outcome = block_on(driver.import_records(vec![ExportRecord {
        kind: "entry".to_string(),
        id: "rec-1".to_string(),
        namespace: None,
        taint: MemoryTaint::Internal,
        payload: serde_json::Value::Null,
    }]))
    .expect("import must succeed");

    // Skipped, never imported: reporting an import would tell a migration its
    // data landed somewhere it did not.
    assert_eq!(outcome.imported, 0);
    assert_eq!(outcome.skipped, 1);
    assert_eq!(outcome.failed, 0);
}

#[test]
fn every_unadvertised_family_is_unreachable_through_the_trait_object() {
    let driver = NullMemoryProvider::new();
    let provider: &dyn MemoryProvider = &driver;

    assert!(provider.as_ingest().is_none());
    assert!(provider.as_documents().is_none());
    assert!(provider.as_tree().is_none());
    assert!(provider.as_entities().is_none());
    assert!(provider.as_graph().is_none());
    assert!(provider.as_diff().is_none());
    assert!(provider.as_goals().is_none());
    assert!(provider.as_tool_memory().is_none());
    assert!(provider.as_sources().is_none());
    assert!(provider.as_maintenance().is_none());
}

#[test]
fn advertised_and_reachable_agree_for_every_family() {
    // The invariant that keeps the capability set honest, checked family by
    // family rather than only through the aggregate audit.
    let driver = NullMemoryProvider::new();
    let provider: &dyn MemoryProvider = &driver;
    let advertised = provider.capabilities();

    for capability in Capability::ALL {
        assert_eq!(
            advertised.contains(capability),
            provider.provides(capability),
            "{capability}: advertised and reachable must agree"
        );
    }
}

/// Assert a result is `Unsupported` and names the expected family.
fn assert_unsupported<T: std::fmt::Debug>(result: Result<T, MemoryError>, expected: Capability) {
    match result {
        Err(MemoryError::Unsupported { capability }) => {
            assert_eq!(capability, expected.as_str());
        }
        other => panic!("expected Unsupported({expected}), got {other:?}"),
    }
}

#[test]
fn unadvertised_families_return_unsupported_naming_their_capability() {
    let driver = NullMemoryProvider::new();

    assert_unsupported(block_on(driver.ingest_chat(Vec::new())), Capability::Ingest);
    assert_unsupported(
        block_on(driver.get_document("global", "k")),
        Capability::Documents,
    );
    assert_unsupported(block_on(driver.seal("global")), Capability::Tree);
    assert_unsupported(
        block_on(driver.entities("global", None, 10)),
        Capability::Entities,
    );
    assert_unsupported(block_on(driver.kv_get(None, "k")), Capability::Graph);
    assert_unsupported(
        block_on(driver.capture_snapshot("src-abc")),
        Capability::Diff,
    );
    assert_unsupported(block_on(driver.goals()), Capability::Goals);
    assert_unsupported(block_on(driver.tool_rules("shell")), Capability::ToolMemory);
    assert_unsupported(
        block_on(driver.forget_source("src-abc")),
        Capability::Sources,
    );
    assert_unsupported(block_on(driver.doctor()), Capability::Maintenance);
}

#[test]
fn provider_is_usable_as_a_shared_trait_object() {
    // The registry binds `Arc<dyn MemoryProvider>`, so the trait object must be
    // `Send + Sync` and every family trait must be object-safe. This test fails
    // to *compile* rather than to run if that ever regresses.
    fn assert_send_sync<T: Send + Sync>(_value: &T) {}

    let provider: std::sync::Arc<dyn MemoryProvider> =
        std::sync::Arc::new(NullMemoryProvider::new());
    assert_send_sync(&provider);
    assert_eq!(provider.driver_id(), NULL_DRIVER_ID);
    assert!(block_on(provider.list(None, None, None))
        .expect("list through the trait object")
        .is_empty());
}
