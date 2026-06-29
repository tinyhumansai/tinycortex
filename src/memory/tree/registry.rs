//! Generic tree registry — get-or-create for any tree kind.
//!
//! All tree flavors share `UNIQUE(kind, scope)` and the same race-recovery
//! dance, so there is one implementation parameterised by [`TreeKind`].

use anyhow::Result;
use chrono::Utc;
use uuid::Uuid;

use crate::memory::config::MemoryConfig;
use crate::memory::tree::store::{self, Tree, TreeKind, TreeStatus};

/// Generic get-or-create. Returns the existing tree for `(kind, scope)` or
/// inserts a fresh one, recovering from a `UNIQUE` race by re-querying.
pub fn get_or_create_tree(config: &MemoryConfig, kind: TreeKind, scope: &str) -> Result<Tree> {
    if let Some(existing) = store::get_tree_by_scope(config, kind, scope)? {
        return Ok(existing);
    }

    let tree = Tree {
        id: new_tree_id(kind),
        kind,
        scope: scope.to_string(),
        root_id: None,
        max_level: 0,
        status: TreeStatus::Active,
        created_at: Utc::now(),
        last_sealed_at: None,
    };
    match store::insert_tree(config, &tree) {
        Ok(()) => Ok(tree),
        Err(err) if is_unique_violation(&err) => {
            // Another caller created the same (kind, scope) between our lookup
            // and insert; re-query and return the winner.
            store::get_tree_by_scope(config, kind, scope)?.ok_or_else(|| {
                anyhow::anyhow!(
                    "UNIQUE violation on insert but no row found on re-query for kind={} scope={}",
                    kind.as_str(),
                    scope
                )
            })
        }
        Err(err) => Err(err),
    }
}

/// Return true if `err` represents a SQLite UNIQUE constraint violation.
pub fn is_unique_violation(err: &anyhow::Error) -> bool {
    if let Some(rusqlite::Error::SqliteFailure(sqlite_err, _)) =
        err.downcast_ref::<rusqlite::Error>()
    {
        return sqlite_err.code == rusqlite::ErrorCode::ConstraintViolation;
    }
    format!("{err:#}").contains("UNIQUE constraint failed")
}

/// Generate a stable id for a new tree row, prefixed with the kind discriminator.
pub fn new_tree_id(kind: TreeKind) -> String {
    format!("{}:{}", kind.as_str(), Uuid::new_v4())
}

/// Id generator for summary nodes. The zero-padded Unix-ms timestamp is the
/// leading sort key so `ORDER BY id` is globally chronological across levels;
/// the level is suffixed for filter-by-level queries.
pub fn new_summary_id(level: u32) -> String {
    let ms = Utc::now().timestamp_millis() as u64;
    let rand_tail: u32 = rand::random();
    format!("summary:{ms:013}:L{level}-{rand_tail:08x}")
}

#[cfg(test)]
#[path = "registry_tests.rs"]
mod tests;
