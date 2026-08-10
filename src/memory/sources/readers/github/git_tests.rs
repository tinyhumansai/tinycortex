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
async fn fetch_existing_bare_refreshes_default_branch_head() {
    // Regression: the clone's default branch can change upstream. The bare
    // clone's HEAD is pinned at clone time, and the fetch refspec updates
    // refs/heads/* but not HEAD, so an unconfigured `git log HEAD` would keep
    // walking the old default while the REST fallback follows the new one.
    // After fetching, HEAD must be repointed to the remote's current default.
    let tmp = tempfile::tempdir().expect("tempdir");
    let src = tmp.path().join("src");
    std::fs::create_dir_all(&src).expect("create repo dir");
    git_ok(&src, &["init", "-q", "-b", "master"]);
    git_ok(&src, &["config", "user.email", "test@example.com"]);
    git_ok(&src, &["config", "user.name", "Test"]);
    std::fs::write(src.join("a.txt"), "one").expect("write file");
    git_ok(&src, &["add", "."]);
    git_ok(&src, &["commit", "-qm", "first"]);

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
    let head_ref = git_ok(&cache, &["symbolic-ref", "HEAD"]);
    assert_eq!(
        head_ref.trim(),
        "refs/heads/master",
        "clone pins default HEAD"
    );

    // Upstream renames its default branch: create `main` and switch HEAD to it
    // while keeping `master` alive (a repo that changes its default branch).
    git_ok(&src, &["checkout", "-q", "-b", "main"]);
    std::fs::write(src.join("b.txt"), "two").expect("write file");
    git_ok(&src, &["add", "."]);
    git_ok(&src, &["commit", "-qm", "second"]);
    git_ok(&src, &["symbolic-ref", "HEAD", "refs/heads/main"]);

    // A plain fetch (without the refresh) would leave HEAD on `master`.
    fetch_existing_bare(&cache).await.expect("fetch succeeds");
    let refreshed = git_ok(&cache, &["symbolic-ref", "HEAD"]);
    assert_eq!(
        refreshed.trim(),
        "refs/heads/main",
        "fetch must repoint HEAD to the remote's new default branch"
    );
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
