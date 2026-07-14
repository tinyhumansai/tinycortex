use chrono::{TimeZone, Utc};
use tempfile::TempDir;

use super::*;

fn config(tmp: &TempDir) -> MemoryConfig {
    MemoryConfig::new(tmp.path().join("workspace"))
}

#[test]
fn recovery_restores_backup_when_swap_stopped_before_publish() {
    let tmp = TempDir::new().unwrap();
    let config = config(&tmp);
    let namespace = "restore-backup";
    let timestamp = Utc.with_ymd_and_hms(2026, 7, 14, 5, 0, 0).unwrap();
    store::buffer_write(&config, namespace, "pending", &timestamp, None).unwrap();

    let active = store::tree_dir(&config, namespace);
    let backup = active.with_file_name(BACKUP_DIR);
    std::fs::rename(&active, &backup).unwrap();

    recover_interrupted_swap(&config, namespace).unwrap();

    assert!(active.exists());
    assert!(!backup.exists());
    assert_eq!(store::buffer_read(&config, namespace).unwrap().len(), 1);
}

#[test]
fn recovery_adopts_backup_buffer_after_new_tree_is_visible() {
    let tmp = TempDir::new().unwrap();
    let config = config(&tmp);
    let namespace = "adopt-buffer";
    let timestamp = Utc.with_ymd_and_hms(2026, 7, 14, 5, 0, 0).unwrap();
    store::buffer_write(&config, namespace, "pending", &timestamp, None).unwrap();

    let active = store::tree_dir(&config, namespace);
    let backup = active.with_file_name(BACKUP_DIR);
    std::fs::rename(&active, &backup).unwrap();
    std::fs::create_dir_all(active.join("2026/07/14")).unwrap();
    std::fs::write(active.join("2026/07/14/05.md"), "replacement").unwrap();

    recover_interrupted_swap(&config, namespace).unwrap();

    assert!(active.join("2026/07/14/05.md").exists());
    assert!(!backup.exists());
    assert_eq!(store::buffer_read(&config, namespace).unwrap().len(), 1);
}
