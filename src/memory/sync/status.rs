//! Pull-based synchronization status derived from the authoritative chunk store.

use serde::{Deserialize, Serialize};

use crate::memory::chunks::with_connection;
use crate::memory::config::MemoryConfig;

const WAVE_WINDOW_MS: i64 = 10 * 60 * 1000;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum FreshnessLabel {
    Active,
    Recent,
    Idle,
}

impl FreshnessLabel {
    pub fn from_age_ms(last_chunk_at_ms: Option<i64>, now_ms: i64) -> Self {
        match last_chunk_at_ms {
            None => Self::Idle,
            Some(timestamp) => match now_ms.saturating_sub(timestamp) {
                age if age <= 30_000 => Self::Active,
                age if age <= 5 * 60_000 => Self::Recent,
                _ => Self::Idle,
            },
        }
    }
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub struct MemorySyncStatus {
    pub provider: String,
    pub chunks_synced: u64,
    pub chunks_pending: u64,
    pub batch_total: u64,
    pub batch_processed: u64,
    pub last_chunk_at_ms: Option<i64>,
    pub freshness: FreshnessLabel,
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub struct StatusListResponse {
    pub statuses: Vec<MemorySyncStatus>,
}

pub fn list_sync_statuses(config: &MemoryConfig) -> anyhow::Result<Vec<MemorySyncStatus>> {
    list_sync_statuses_at(config, chrono::Utc::now().timestamp_millis())
}

fn list_sync_statuses_at(
    config: &MemoryConfig,
    now_ms: i64,
) -> anyhow::Result<Vec<MemorySyncStatus>> {
    with_connection(config, |connection| {
        let mut statement = connection.prepare(
            "WITH provider_chunks AS ( \
                SELECT CASE WHEN INSTR(source_id, ':') > 0 \
                    THEN SUBSTR(source_id, 1, INSTR(source_id, ':') - 1) \
                    ELSE source_kind END AS provider, \
                    created_at_ms, \
                    CASE WHEN EXISTS (SELECT 1 FROM mem_tree_chunk_embeddings e WHERE e.chunk_id = c.id) \
                      OR c.lifecycle_status = 'dropped' \
                      OR EXISTS (SELECT 1 FROM mem_tree_chunk_reembed_skipped s WHERE s.chunk_id = c.id) \
                    THEN 1 ELSE 0 END AS resolved, timestamp_ms \
                FROM mem_tree_chunks c \
             ), provider_max AS ( \
                SELECT provider, MAX(created_at_ms) AS max_created FROM provider_chunks GROUP BY provider \
             ), provider_pending AS ( \
                SELECT p.provider, SUM(CASE WHEN p.resolved = 0 AND p.created_at_ms >= m.max_created - ?1 THEN 1 ELSE 0 END) AS pending \
                FROM provider_chunks p JOIN provider_max m ON p.provider = m.provider GROUP BY p.provider \
             ), wave_anchors AS ( \
                SELECT p.provider, MIN(p.created_at_ms) AS anchor \
                FROM provider_chunks p JOIN provider_max m ON p.provider = m.provider \
                JOIN provider_pending pp ON p.provider = pp.provider \
                WHERE pp.pending > 0 AND p.created_at_ms >= m.max_created - ?1 GROUP BY p.provider \
             ) SELECT p.provider, COUNT(*) AS chunks_synced, \
                SUM(CASE WHEN p.resolved = 0 THEN 1 ELSE 0 END) AS chunks_pending, \
                SUM(CASE WHEN w.anchor IS NOT NULL AND p.created_at_ms >= w.anchor THEN 1 ELSE 0 END) AS batch_total, \
                SUM(CASE WHEN w.anchor IS NOT NULL AND p.created_at_ms >= w.anchor AND p.resolved = 1 THEN 1 ELSE 0 END) AS batch_processed, \
                MAX(p.timestamp_ms) AS last_chunk_at_ms \
             FROM provider_chunks p LEFT JOIN wave_anchors w ON p.provider = w.provider \
             GROUP BY p.provider ORDER BY last_chunk_at_ms DESC",
        )?;
        let rows = statement.query_map([WAVE_WINDOW_MS], |row| {
            let last_chunk_at_ms = row.get(5)?;
            Ok(MemorySyncStatus {
                provider: row.get(0)?,
                chunks_synced: nonnegative(row.get(1)?),
                chunks_pending: nonnegative(row.get(2)?),
                batch_total: nonnegative(row.get(3)?),
                batch_processed: nonnegative(row.get(4)?),
                last_chunk_at_ms,
                freshness: FreshnessLabel::from_age_ms(last_chunk_at_ms, now_ms),
            })
        })?;
        Ok(rows.collect::<rusqlite::Result<Vec<_>>>()?)
    })
}

fn nonnegative(value: i64) -> u64 {
    value.max(0) as u64
}

#[cfg(test)]
mod tests {
    use chrono::{TimeZone, Utc};

    use super::*;
    use crate::memory::chunks::{upsert_chunks, Chunk, Metadata, SourceKind};

    fn chunk(id: &str, source_id: &str, created_ms: i64) -> Chunk {
        let timestamp = Utc.timestamp_millis_opt(created_ms).unwrap();
        Chunk {
            id: id.into(),
            content: "content".into(),
            token_count: 1,
            seq_in_source: 0,
            created_at: timestamp,
            partial_message: false,
            metadata: Metadata {
                source_kind: SourceKind::Document,
                source_id: source_id.into(),
                path_scope: None,
                source_ref: None,
                owner: "test".into(),
                timestamp,
                time_range: (timestamp, timestamp),
                tags: Vec::new(),
            },
        }
    }

    #[test]
    fn status_groups_provider_and_tracks_active_wave_resolution() {
        let temp = tempfile::tempdir().unwrap();
        let config = MemoryConfig::new(temp.path());
        let now = 1_777_000_000_000i64;
        upsert_chunks(
            &config,
            &[
                chunk("a", "gmail:conn", now - 2_000),
                chunk("b", "gmail:conn", now - 1_000),
                chunk("c", "slack:conn", now - 600_000),
            ],
        )
        .unwrap();
        with_connection(&config, |connection| {
            connection.execute(
                "INSERT INTO mem_tree_chunk_embeddings (chunk_id, model_signature, vector, dim, created_at) VALUES ('a', 'test', X'00', 1, 0)",
                [],
            )?;
            connection.execute("UPDATE mem_tree_chunks SET lifecycle_status = 'dropped' WHERE id = 'c'", [])?;
            Ok(())
        }).unwrap();

        let statuses = list_sync_statuses_at(&config, now).unwrap();
        let gmail = statuses
            .iter()
            .find(|status| status.provider == "gmail")
            .unwrap();
        assert_eq!(gmail.chunks_synced, 2);
        assert_eq!(gmail.chunks_pending, 1);
        assert_eq!(gmail.batch_total, 2);
        assert_eq!(gmail.batch_processed, 1);
        assert_eq!(gmail.freshness, FreshnessLabel::Active);
        let slack = statuses
            .iter()
            .find(|status| status.provider == "slack")
            .unwrap();
        assert_eq!(slack.chunks_pending, 0);
        assert_eq!(slack.batch_total, 0);
        assert_eq!(slack.freshness, FreshnessLabel::Idle);
    }
}
