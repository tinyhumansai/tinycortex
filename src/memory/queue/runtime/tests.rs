use std::time::Duration;

use super::*;
use crate::memory::config::MemoryConfig;
use crate::memory::queue::store::{count_by_status, enqueue};
use crate::memory::queue::test_support::RecordingDelegates;
use crate::memory::queue::types::{FlushStalePayload, JobStatus, NewJob};
use tempfile::TempDir;
use tokio::time::sleep;

fn test_config() -> (TempDir, MemoryConfig) {
    let tmp = TempDir::new().unwrap();
    let cfg = MemoryConfig::new(tmp.path());
    (tmp, cfg)
}

/// Small backoffs so the loops make progress quickly under test.
fn fast_worker_opts() -> WorkerLoopConfig {
    WorkerLoopConfig {
        idle_backoff: Duration::from_millis(2),
        busy_backoff: Duration::from_millis(2),
        io_backoff: Duration::from_millis(2),
        disk_full_backoff: Duration::from_millis(2),
        error_backoff: Duration::from_millis(2),
    }
}

#[tokio::test]
async fn worker_loop_drains_queued_jobs_then_stops_on_shutdown() {
    let (_tmp, cfg) = test_config();
    let d = RecordingDelegates::admitting();
    let new_job = NewJob::flush_stale(&FlushStalePayload::default(), "2026-05-24", 3).unwrap();
    enqueue(&cfg, &new_job).unwrap().expect("enqueued");

    let opts = fast_worker_opts();
    let shutdown = Shutdown::new();

    // Concurrent stopper: once the flush job has been settled, ask the worker to
    // stop. `join!` polls both on this task, so no `Send` bound is required.
    let stopper = async {
        loop {
            if count_by_status(&cfg, JobStatus::Done).unwrap() >= 1 {
                shutdown.trigger();
                return;
            }
            sleep(Duration::from_millis(1)).await;
        }
    };

    let (worker_res, ()) = tokio::join!(run_worker(&cfg, &d, &opts, &shutdown), stopper);
    worker_res.unwrap();

    assert_eq!(count_by_status(&cfg, JobStatus::Done).unwrap(), 1);
}

#[tokio::test]
async fn worker_loop_returns_immediately_when_already_shut_down() {
    let (_tmp, cfg) = test_config();
    let d = RecordingDelegates::admitting();
    let opts = fast_worker_opts();
    let shutdown = Shutdown::new();
    shutdown.trigger();

    // Pre-triggered: the loop condition is false on entry, so it never polls.
    run_worker(&cfg, &d, &opts, &shutdown).await.unwrap();
    assert!(shutdown.is_triggered());
}

#[tokio::test]
async fn scheduler_loop_enqueues_flush_stale_then_stops() {
    let (_tmp, cfg) = test_config();
    let opts = SchedulerLoopConfig {
        tick: Duration::from_millis(5),
    };
    let shutdown = Shutdown::new();

    let stopper = async {
        loop {
            let queued = count_by_status(&cfg, JobStatus::Ready).unwrap();
            if queued >= 1 {
                shutdown.trigger();
                return;
            }
            sleep(Duration::from_millis(1)).await;
        }
    };

    let (sched_res, ()) = tokio::join!(run_scheduler(&cfg, &opts, &shutdown), stopper);
    sched_res.unwrap();

    // At least one flush_stale job was enqueued by the loop.
    assert!(count_by_status(&cfg, JobStatus::Ready).unwrap() >= 1);
}

#[test]
fn backoff_classifies_corruption_as_fatal() {
    let opts = WorkerLoopConfig::default();
    let corrupt = anyhow::Error::from(rusqlite::Error::SqliteFailure(
        rusqlite::ffi::Error {
            code: rusqlite::ErrorCode::DatabaseCorrupt,
            extended_code: 11,
        },
        Some("database disk image is malformed".into()),
    ));
    assert!(backoff_for(&corrupt, &opts).is_none());
}

#[test]
fn backoff_classifies_busy_as_transient() {
    let opts = WorkerLoopConfig::default();
    let busy = anyhow::Error::from(rusqlite::Error::SqliteFailure(
        rusqlite::ffi::Error {
            code: rusqlite::ErrorCode::DatabaseBusy,
            extended_code: 5,
        },
        Some("database is locked".into()),
    ));
    assert_eq!(backoff_for(&busy, &opts), Some(opts.busy_backoff));
}
