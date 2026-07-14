//! Transaction-scoped entity-index writes.

use rusqlite::{params, Transaction};

use super::store::{
    canonical_id_is_user, NoSelfIdentity, SelfIdentity, UPSERT_PRESERVE_USER_SQL, UPSERT_SQL,
};
use super::types::CanonicalEntity;

/// Index canonical entities inside the caller's transaction.
///
/// New rows are inserted with `is_user = false` because [`NoSelfIdentity`]
/// performs no identity classification. On conflict,
/// `UPSERT_PRESERVE_USER_SQL` deliberately preserves the existing `is_user`
/// value rather than resetting a prior identity match.
pub fn index_entities_tx(
    tx: &Transaction<'_>,
    entities: &[CanonicalEntity],
    node_id: &str,
    node_kind: &str,
    timestamp_ms: i64,
    tree_id: Option<&str>,
) -> anyhow::Result<usize> {
    index_entities_tx_inner(
        tx,
        entities,
        node_id,
        node_kind,
        timestamp_ms,
        tree_id,
        &NoSelfIdentity,
        UPSERT_PRESERVE_USER_SQL,
    )
}

/// Index canonical entities with self-identity classification in one transaction.
pub fn index_entities_tx_with_identity(
    tx: &Transaction<'_>,
    entities: &[CanonicalEntity],
    node_id: &str,
    node_kind: &str,
    timestamp_ms: i64,
    tree_id: Option<&str>,
    identity: &dyn SelfIdentity,
) -> anyhow::Result<usize> {
    index_entities_tx_inner(
        tx,
        entities,
        node_id,
        node_kind,
        timestamp_ms,
        tree_id,
        identity,
        UPSERT_SQL,
    )
}

#[allow(clippy::too_many_arguments)]
fn index_entities_tx_inner(
    tx: &Transaction<'_>,
    entities: &[CanonicalEntity],
    node_id: &str,
    node_kind: &str,
    timestamp_ms: i64,
    tree_id: Option<&str>,
    identity: &dyn SelfIdentity,
    sql: &str,
) -> anyhow::Result<usize> {
    let mut statement = tx.prepare(sql)?;
    for entity in entities {
        statement.execute(params![
            entity.canonical_id,
            node_id,
            node_kind,
            entity.kind.as_str(),
            entity.surface,
            entity.score,
            timestamp_ms,
            tree_id,
            identity.is_self(entity.kind, &entity.surface) as i32,
        ])?;
    }
    Ok(entities.len())
}

/// Delete all entity-index rows for a node inside the caller's transaction.
pub fn clear_entity_index_for_node_tx(
    tx: &Transaction<'_>,
    node_id: &str,
) -> anyhow::Result<usize> {
    Ok(tx.execute(
        "DELETE FROM mem_tree_entity_index WHERE node_id = ?1",
        params![node_id],
    )?)
}

/// Index canonical summary entity ids with self-identity classification.
pub fn index_summary_entity_ids_tx_with_identity(
    tx: &Transaction<'_>,
    entity_ids: &[String],
    node_id: &str,
    score: f32,
    timestamp_ms: i64,
    tree_id: Option<&str>,
    identity: &dyn SelfIdentity,
) -> anyhow::Result<usize> {
    let mut statement = tx.prepare(UPSERT_SQL)?;
    for canonical_id in entity_ids {
        let entity_kind = canonical_id
            .split_once(':')
            .map_or(canonical_id.as_str(), |(kind, _)| kind);
        statement.execute(params![
            canonical_id,
            node_id,
            "summary",
            entity_kind,
            canonical_id,
            score,
            timestamp_ms,
            tree_id,
            canonical_id_is_user(identity, canonical_id) as i32,
        ])?;
    }
    Ok(entity_ids.len())
}
