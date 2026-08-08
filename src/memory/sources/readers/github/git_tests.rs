use super::*;

use std::process::Command;

/// Run `git` with the given args in `cwd`, asserting success and returning
/// stdout as a string.
fn git_ok(cwd: &Path, args: &[&str]) -> String {
    let out = Command::new("git")
        .args(args)
        .current_dir(cwd)
        .output()
        .expect("spawn git");
    assert!(
        out.status.success(),
        "git {args:?} failed: {}",
        String::from_utf8_lossy(&out.stderr)
    );
    String::from_utf8_lossy(&out.stdout).into_owned()
}

/// Create a source repo with one commit at `dir`.
fn init_repo(dir: &Path) {
    std::fs::create_dir_all(dir).expect("create repo dir");
    git_ok(dir, &["init", "-q"]);
    git_ok(dir, &["config", "user.email", "test@example.com"]);
    git_ok(dir, &["config", "user.name", "Test"]);
    std::fs::write(dir.join("a.txt"), "one").expect("write file");
    git_ok(dir, &["add", "."]);
    git_ok(dir, &["commit", "-qm", "first"]);
}

#[tokio::test]
async fn fetch_existing_bare_advances_local_heads() {
    // A bare clone records no remote.origin.fetch refspec, so a bare `git
    // fetch` (no refspec) would only touch FETCH_HEAD. The explicit
    // `+refs/heads/*:refs/heads/*` must advance refs/heads/* to the remote's
    // new commits, otherwise every later sync silently misses them.
    let tmp = tempfile::tempdir().expect("tempdir");
    let src = tmp.path().join("src");
    init_repo(&src);

    let cache = tmp.path().join("cache.git");
    git_ok(
        tmp.path(),
        &[
            "clone",
            "--bare",
            "-q",
            src.to_str().unwrap(),
            cache.to_str().unwrap(),
        ],
    );
    let first_head = git_ok(&cache, &["rev-parse", "HEAD"]);

    // A second commit lands upstream.
    std::fs::write(src.join("b.txt"), "two").expect("write file");
    git_ok(&src, &["add", "."]);
    git_ok(&src, &["commit", "-qm", "second"]);
    let upstream_head = git_ok(&src, &["rev-parse", "HEAD"]);
    assert_ne!(first_head, upstream_head, "test setup: new commit expected");

    // Fetch into the existing bare clone and confirm the local head advances.
    fetch_existing_bare(&cache).await.expect("fetch succeeds");
    let cached_head = git_ok(&cache, &["rev-parse", "HEAD"]);
    assert_eq!(
        cached_head, upstream_head,
        "fetch must advance refs/heads/* so git log --all sees new commits"
    );
}
