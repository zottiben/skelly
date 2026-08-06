//! Integration tests for the **snapshot-backed rewind** trust contract (AGENTS Hard rule
//! 3, ADR-0008): drive a real `git` against a throwaway repo and assert that capturing the
//! working tree and restoring it to an earlier snapshot changes exactly the working tree -
//! never HEAD, the branch, any ref, the reflog, or the repository's own index - and that
//! the state a restore replaces is always recoverable.
//!
//! The repo is isolated from the host's git config (`GIT_CONFIG_GLOBAL`/`_SYSTEM`) so the
//! result is deterministic regardless of the developer's settings.

use std::path::Path;
use std::process::Command;

use skelly_session::SnapshotStore;

/// Run `git -C <root> <args>` in a config-isolated environment, asserting success.
fn git(root: &Path, args: &[&str]) {
    let status = Command::new("git")
        .arg("-C")
        .arg(root)
        .args(args)
        .env("GIT_CONFIG_GLOBAL", "/dev/null")
        .env("GIT_CONFIG_SYSTEM", "/dev/null")
        .env("GIT_AUTHOR_NAME", "Test")
        .env("GIT_AUTHOR_EMAIL", "test@example.com")
        .env("GIT_COMMITTER_NAME", "Test")
        .env("GIT_COMMITTER_EMAIL", "test@example.com")
        .status()
        .expect("run git");
    assert!(status.success(), "git {args:?} failed");
}

/// Capture `git -C <root> <args>` stdout (trimmed), asserting success.
fn git_out(root: &Path, args: &[&str]) -> String {
    let output = Command::new("git")
        .arg("-C")
        .arg(root)
        .args(args)
        .env("GIT_CONFIG_GLOBAL", "/dev/null")
        .env("GIT_CONFIG_SYSTEM", "/dev/null")
        .output()
        .expect("run git");
    assert!(output.status.success(), "git {args:?} failed");
    String::from_utf8_lossy(&output.stdout).trim().to_owned()
}

/// Everything a snapshot restore must leave byte-identical: HEAD, the branch it points at,
/// every ref, and what the repository's own index says is staged.
fn repo_state(root: &Path) -> (String, String, String, String) {
    (
        git_out(root, &["rev-parse", "HEAD"]),
        git_out(root, &["symbolic-ref", "HEAD"]),
        git_out(root, &["for-each-ref", "--format=%(refname) %(objectname)"]),
        git_out(root, &["diff", "--cached", "--name-status"]),
    )
}

/// A repo on `main` with one commit, a `.gitignore` for `build/`, and an ignored artifact.
fn seeded_repo(root: &Path) {
    git(root, &["init", "-b", "main"]);
    std::fs::write(root.join(".gitignore"), "build/\n").expect("write .gitignore");
    std::fs::write(root.join("a.txt"), "one\n").expect("write a.txt");
    std::fs::create_dir_all(root.join("sub")).expect("mkdir sub");
    std::fs::write(root.join("sub/b.txt"), "two\n").expect("write sub/b.txt");
    std::fs::create_dir_all(root.join("build")).expect("mkdir build");
    std::fs::write(root.join("build/out.bin"), "artifact\n").expect("write artifact");
    git(root, &["add", "-A"]);
    git(root, &["commit", "-m", "first", "--no-gpg-sign"]);
}

/// Open a snapshot store for `root` under an isolated `store` directory.
fn store_for(store: &Path, root: &Path) -> SnapshotStore {
    SnapshotStore::open_in(store, root).expect("open snapshot store")
}

#[test]
fn restoring_a_snapshot_rewinds_the_working_tree_without_touching_head_or_refs() {
    let dir = tempfile::tempdir().expect("temp dir");
    let store_dir = tempfile::tempdir().expect("store dir");
    let root = dir.path();
    seeded_repo(root);
    let store = store_for(store_dir.path(), root);

    // The "past" moment: one edit on top of the commit, staged in the user's index so the
    // restore has something to leave alone.
    std::fs::write(root.join("a.txt"), "one edited\n").expect("edit a.txt");
    git(root, &["add", "a.txt"]);
    let past = store.capture().expect("capture past");
    let before = repo_state(root);

    // Work on: change a file, add one, delete one.
    std::fs::write(root.join("a.txt"), "one edited twice\n").expect("re-edit a.txt");
    std::fs::write(root.join("c.txt"), "three\n").expect("write c.txt");
    std::fs::remove_file(root.join("sub/b.txt")).expect("delete sub/b.txt");
    let now = store.capture().expect("capture now");
    assert_ne!(past, now, "the working tree changed, so the snapshot did");

    let replaced = store.restore(&past).expect("restore the past snapshot");

    // The working tree is exactly as it was at `past`.
    assert_eq!(
        std::fs::read_to_string(root.join("a.txt")).expect("read a.txt"),
        "one edited\n",
        "content is rewound"
    );
    assert!(
        !root.join("c.txt").exists(),
        "a file created after the snapshot is removed"
    );
    assert_eq!(
        std::fs::read_to_string(root.join("sub/b.txt")).expect("read sub/b.txt"),
        "two\n",
        "a file deleted after the snapshot is brought back"
    );
    // Ignored build output is not part of a snapshot, so a rewind never disturbs it.
    assert_eq!(
        std::fs::read_to_string(root.join("build/out.bin")).expect("read artifact"),
        "artifact\n",
        "ignored files are left alone"
    );

    // The trust contract: nothing about the repository itself moved.
    assert_eq!(
        repo_state(root),
        before,
        "HEAD, refs, and the index are untouched"
    );
    assert!(
        git_out(root, &["reflog", "--format=%H %gs"])
            .lines()
            .count()
            <= 1,
        "no reflog entries were written"
    );

    // And the state the restore replaced is recoverable, byte for byte.
    assert_eq!(
        replaced, now,
        "restore returns a snapshot of what it replaced"
    );
    store.restore(&replaced).expect("return to now");
    assert_eq!(
        std::fs::read_to_string(root.join("a.txt")).expect("read a.txt"),
        "one edited twice\n"
    );
    assert_eq!(
        std::fs::read_to_string(root.join("c.txt")).expect("read c.txt"),
        "three\n"
    );
    assert!(!root.join("sub/b.txt").exists(), "the deletion is back");
    assert_eq!(
        repo_state(root),
        before,
        "returning to now is just as inert"
    );
}

#[test]
fn an_unchanged_working_tree_snapshots_to_the_same_id() {
    let dir = tempfile::tempdir().expect("temp dir");
    let store_dir = tempfile::tempdir().expect("store dir");
    let root = dir.path();
    seeded_repo(root);
    let store = store_for(store_dir.path(), root);

    let first = store.capture().expect("capture");
    let second = store.capture().expect("capture again");
    assert_eq!(
        first, second,
        "snapshots are content-addressed, so an idle poll records no new moment"
    );

    // Writing an ignored file is not a change to the codebase, either.
    std::fs::write(root.join("build/out.bin"), "rebuilt\n").expect("rewrite artifact");
    assert_eq!(
        store.capture().expect("capture after a rebuild"),
        first,
        "build output does not create a timeline moment"
    );
}

#[test]
fn the_store_lives_outside_the_repository() {
    let dir = tempfile::tempdir().expect("temp dir");
    let store_dir = tempfile::tempdir().expect("store dir");
    let root = dir.path();
    seeded_repo(root);
    let store = store_for(store_dir.path(), root);
    let objects = |root: &Path| {
        git_out(root, &["count-objects", "-v"])
            .lines()
            .find(|l| l.starts_with("count:"))
            .expect("count line")
            .to_owned()
    };
    let before = objects(root);

    std::fs::write(root.join("a.txt"), "brand new content\n").expect("edit a.txt");
    store.capture().expect("capture");

    assert!(
        store.git_dir().starts_with(store_dir.path()),
        "snapshots are written to Skelly's store, never into the user's .git"
    );
    assert!(
        !store.git_dir().starts_with(root),
        "the store is outside the working tree so it can never snapshot itself"
    );
    // The repo's object store gained nothing: no snapshot objects leaked into it.
    assert_eq!(
        objects(root),
        before,
        "loose objects in the user's repo are unchanged by snapshotting"
    );
}

#[test]
fn restoring_an_unknown_snapshot_leaves_the_working_tree_alone() {
    let dir = tempfile::tempdir().expect("temp dir");
    let store_dir = tempfile::tempdir().expect("store dir");
    let root = dir.path();
    seeded_repo(root);
    let store = store_for(store_dir.path(), root);
    store.capture().expect("capture");

    std::fs::write(root.join("a.txt"), "live work\n").expect("edit a.txt");
    let before = repo_state(root);

    let err = store
        .restore("0000000000000000000000000000000000000000")
        .expect_err("restoring a nonexistent tree fails");
    assert!(
        matches!(err, skelly_session::GitError::Command { .. }),
        "a git failure, not a spawn failure: {err}"
    );
    assert_eq!(
        std::fs::read_to_string(root.join("a.txt")).expect("read a.txt"),
        "live work\n",
        "a failed rewind is not half-applied"
    );
    assert_eq!(repo_state(root), before, "and still touches nothing");
}
