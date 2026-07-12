//! Append-only synchronization cost and outcome audit log.

use std::io::Write;

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};

use crate::memory::config::MemoryConfig;

const AUDIT_FILENAME: &str = "sync_audit.jsonl";

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct SyncAuditEntry {
    pub timestamp: DateTime<Utc>,
    pub source_id: String,
    pub source_kind: String,
    pub scope: String,
    pub items_fetched: u32,
    pub batches: u32,
    pub input_tokens: u64,
    pub output_tokens: u64,
    pub estimated_cost_usd: f64,
    #[serde(default)]
    pub composio_actions_called: u32,
    #[serde(default)]
    pub composio_cost_usd: f64,
    #[serde(default)]
    pub actual_charged_usd: Option<f64>,
    pub duration_ms: u64,
    pub success: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
}

impl SyncAuditEntry {
    pub fn combined_cost_usd(&self) -> f64 {
        self.effective_cost_usd()
    }

    pub fn effective_cost_usd(&self) -> f64 {
        self.actual_charged_usd.unwrap_or(self.estimated_cost_usd) + self.composio_cost_usd
    }
}

pub fn append_audit_entry(config: &MemoryConfig, entry: &SyncAuditEntry) -> anyhow::Result<()> {
    let directory = config.workspace.join("memory_tree");
    std::fs::create_dir_all(&directory)?;
    let mut file = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(directory.join(AUDIT_FILENAME))?;
    serde_json::to_writer(&mut file, entry)?;
    writeln!(file)?;
    tracing::debug!(source_id = %entry.source_id, success = entry.success, "[memory_sync:audit] entry appended");
    Ok(())
}

pub fn read_audit_log(config: &MemoryConfig) -> anyhow::Result<Vec<SyncAuditEntry>> {
    let path = config.workspace.join("memory_tree").join(AUDIT_FILENAME);
    let content = match std::fs::read_to_string(path) {
        Ok(content) => content,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(Vec::new()),
        Err(error) => return Err(error.into()),
    };
    let mut entries: Vec<_> = content
        .lines()
        .filter(|line| !line.trim().is_empty())
        .filter_map(|line| match serde_json::from_str(line) {
            Ok(entry) => Some(entry),
            Err(error) => {
                tracing::warn!(%error, "[memory_sync:audit] malformed audit line skipped");
                None
            }
        })
        .collect();
    entries.reverse();
    Ok(entries)
}

pub fn estimate_cost_usd(input_tokens: u64, output_tokens: u64) -> f64 {
    input_tokens as f64 * 0.07 / 1_000_000.0 + output_tokens as f64 * 0.28 / 1_000_000.0
}

#[derive(Debug, Default, Clone)]
pub struct RealCostAccumulator {
    total_batches: u32,
    batches_with_usage: u32,
    batches_with_charge: u32,
    est_input_tokens: u64,
    est_output_tokens: u64,
    real_input_tokens: u64,
    real_output_tokens: u64,
    real_charged_usd: f64,
}

impl RealCostAccumulator {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn add_batch(
        &mut self,
        est_input: u64,
        est_output: u64,
        real_input: u64,
        real_output: u64,
        charge: Option<f64>,
    ) {
        self.total_batches = self.total_batches.saturating_add(1);
        self.est_input_tokens = self.est_input_tokens.saturating_add(est_input);
        self.est_output_tokens = self.est_output_tokens.saturating_add(est_output);
        if real_input > 0 || real_output > 0 {
            self.batches_with_usage = self.batches_with_usage.saturating_add(1);
            self.real_input_tokens = self.real_input_tokens.saturating_add(real_input);
            self.real_output_tokens = self.real_output_tokens.saturating_add(real_output);
        }
        if let Some(charge) = charge {
            self.batches_with_charge = self.batches_with_charge.saturating_add(1);
            self.real_charged_usd += charge;
        }
    }

    fn usage_is_complete(&self) -> bool {
        self.total_batches > 0 && self.batches_with_usage == self.total_batches
    }

    fn charge_is_complete(&self) -> bool {
        self.total_batches > 0 && self.batches_with_charge == self.total_batches
    }

    pub fn audit_input_tokens(&self) -> u64 {
        if self.usage_is_complete() {
            self.real_input_tokens
        } else {
            self.est_input_tokens
        }
    }

    pub fn audit_output_tokens(&self) -> u64 {
        if self.usage_is_complete() {
            self.real_output_tokens
        } else {
            self.est_output_tokens
        }
    }

    pub fn estimated_cost(&self) -> f64 {
        estimate_cost_usd(self.est_input_tokens, self.est_output_tokens)
    }

    pub fn actual_charged_usd(&self) -> Option<f64> {
        self.charge_is_complete().then_some(self.real_charged_usd)
    }

    pub fn usage_is_real(&self) -> bool {
        self.usage_is_complete()
    }

    pub fn effective_cost_usd(&self) -> f64 {
        self.actual_charged_usd()
            .unwrap_or_else(|| self.estimated_cost())
    }

    pub fn cost_is_actual(&self) -> bool {
        self.charge_is_complete()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn entry(id: &str, timestamp: DateTime<Utc>) -> SyncAuditEntry {
        SyncAuditEntry {
            timestamp,
            source_id: id.into(),
            source_kind: "test".into(),
            scope: "all".into(),
            items_fetched: 1,
            batches: 1,
            input_tokens: 10,
            output_tokens: 2,
            estimated_cost_usd: 0.1,
            composio_actions_called: 1,
            composio_cost_usd: 0.02,
            actual_charged_usd: None,
            duration_ms: 5,
            success: true,
            error: None,
        }
    }

    #[test]
    fn audit_round_trip_is_newest_first_and_skips_malformed_lines() {
        let temp = tempfile::tempdir().unwrap();
        let config = MemoryConfig::new(temp.path());
        let first = entry("first", Utc::now());
        let second = entry("second", Utc::now());
        append_audit_entry(&config, &first).unwrap();
        std::fs::OpenOptions::new()
            .append(true)
            .open(temp.path().join("memory_tree/sync_audit.jsonl"))
            .unwrap()
            .write_all(b"not-json\n")
            .unwrap();
        append_audit_entry(&config, &second).unwrap();
        let entries = read_audit_log(&config).unwrap();
        assert_eq!(entries.len(), 2);
        assert_eq!(entries[0].source_id, "second");
        assert_eq!(entries[1].source_id, "first");
    }

    #[test]
    fn accumulator_uses_real_values_only_when_every_batch_reports_them() {
        let mut complete = RealCostAccumulator::new();
        complete.add_batch(100, 10, 80, 8, Some(0.01));
        complete.add_batch(100, 10, 90, 9, Some(0.02));
        assert_eq!(complete.audit_input_tokens(), 170);
        assert_eq!(complete.actual_charged_usd(), Some(0.03));
        assert!(complete.cost_is_actual());

        let mut partial = RealCostAccumulator::new();
        partial.add_batch(100, 10, 80, 8, Some(0.01));
        partial.add_batch(200, 20, 0, 0, None);
        assert_eq!(partial.audit_input_tokens(), 300);
        assert_eq!(partial.actual_charged_usd(), None);
        assert!(!partial.usage_is_real());
    }
}
