//! Tests for the shared [`atomic_write`] primitive.

use super::atomic_write;
use tempfile::TempDir;

#[test]
fn writes_new_file() {
    let dir = TempDir::new().unwrap();
    let path = dir.path().join("out.txt");
    atomic_write(&path, b"hello").unwrap();
    assert_eq!(std::fs::read(&path).unwrap(), b"hello");
}

#[test]
fn replaces_existing_file_with_new_content() {
    let dir = TempDir::new().unwrap();
    let path = dir.path().join("out.txt");
    atomic_write(&path, b"first-version").unwrap();
    atomic_write(&path, b"second").unwrap();
    // Destination holds the fully-replaced new content, never a truncated mix.
    assert_eq!(std::fs::read(&path).unwrap(), b"second");
}

#[test]
fn leaves_no_temp_files_after_success() {
    let dir = TempDir::new().unwrap();
    let path = dir.path().join("out.txt");
    atomic_write(&path, b"payload").unwrap();
    atomic_write(&path, b"payload-2").unwrap();
    let leftovers: Vec<_> = std::fs::read_dir(dir.path())
        .unwrap()
        .map(|e| e.unwrap().file_name().to_string_lossy().to_string())
        .filter(|name| name != "out.txt")
        .collect();
    assert!(
        leftovers.is_empty(),
        "temp files should be renamed away, found: {leftovers:?}"
    );
}

#[test]
fn failed_write_leaves_destination_and_no_temp_litter() {
    let dir = TempDir::new().unwrap();
    let path = dir.path().join("out.txt");
    atomic_write(&path, b"good-original").unwrap();

    // Target a path whose parent does not exist: File::create fails, so the
    // rename never happens and the original destination is untouched.
    let missing_parent = dir.path().join("does-not-exist").join("out.txt");
    let err = atomic_write(&missing_parent, b"never-lands");
    assert!(err.is_err(), "write into a missing directory must fail");

    // The pre-existing destination is unchanged (old-or-new, never torn).
    assert_eq!(std::fs::read(&path).unwrap(), b"good-original");
    // No stray temp files were left behind in the writable directory.
    let leftovers: Vec<_> = std::fs::read_dir(dir.path())
        .unwrap()
        .map(|e| e.unwrap().file_name().to_string_lossy().to_string())
        .filter(|name| name != "out.txt" && name != "does-not-exist")
        .collect();
    assert!(
        leftovers.is_empty(),
        "failed write must clean up its temp file, found: {leftovers:?}"
    );
}
