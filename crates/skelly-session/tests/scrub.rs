//! End-to-end test of what scrubbing the session timeline is supposed to do: replay the exact
//! sequence the binary performs against a real git repo - poll, snapshot, record a moment, pick a
//! past moment, restore, return to now - and assert the files on disk really change.
//!
//! This is the user-visible contract ("select a point on the timeline and the files go back to
//! what they were then", design §10.7) written as a test, because the binary's own glue needs a
//! window. The pieces it drives - [`SnapshotStore`] and [`Timeline`] - are the ones the binary
//! calls; only the winit key/click routing sits above it.
//!
//! The repo is isolated from the host's git config so the result is deterministic.

use std::path::Path;
use std::process::Command;

use skelly_session::{Actor, SessionEvent, SnapshotStore, Timeline};

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

/// The contents of `note.txt` in the working tree.
fn note(root: &Path) -> String {
    std::fs::read_to_string(root.join("note.txt")).expect("read note.txt")
}

#[test]
fn scrubbing_the_timeline_restores_the_files_of_that_moment_and_comes_back() {
    let dir = tempfile::tempdir().expect("temp dir");
    let store_dir = tempfile::tempdir().expect("store dir");
    let root = dir.path();

    git(root, &["init", "-b", "main"]);
    std::fs::write(root.join("note.txt"), "v1\n").expect("write note.txt");
    git(root, &["add", "-A"]);
    git(root, &["commit", "-m", "first", "--no-gpg-sign"]);

    let head_before = git_out(root, &["rev-parse", "HEAD"]);
    let refs_before = git_out(root, &["for-each-ref", "--format=%(refname) %(objectname)"]);
    let store = SnapshotStore::open_in(store_dir.path(), root).expect("open store");
    let mut timeline = Timeline::new();

    // The binary's session-start anchor: the codebase as Skelly first saw it.
    let launch = store.capture().expect("seed snapshot");
    timeline.record(
        SessionEvent::new(Actor::System, "0:00", "Session started", "main").restoring(&launch),
    );

    // Two edits, each observed by a poll cycle and recorded as its own restorable moment. The
    // second is an edit to an already-dirty file - the case that used to record nothing at all.
    std::fs::write(root.join("note.txt"), "v1\nv2\n").expect("edit note.txt");
    let first_edit = store.capture().expect("snapshot the first edit");
    assert_ne!(first_edit, launch, "the poll sees a new moment");
    timeline.record(
        SessionEvent::new(Actor::Human, "0:20", "Edited note.txt", "+1 \u{2212}0")
            .restoring(&first_edit),
    );

    std::fs::write(root.join("note.txt"), "v1\nv2\nv3\n").expect("edit note.txt again");
    let second_edit = store.capture().expect("snapshot the second edit");
    assert_ne!(second_edit, first_edit, "a second edit is its own moment");
    timeline.record(
        SessionEvent::new(Actor::Human, "1:05", "Edited note.txt", "+2 \u{2212}0")
            .restoring(&second_edit),
    );

    // Something the poll has not seen yet, so returning to now has to restore live work, not just
    // the newest recorded moment.
    std::fs::write(root.join("note.txt"), "v1\nv2\nv3\nunsaved\n").expect("live edit");

    // Scrub to the first edit. Every event is restorable, so this is a real past state.
    assert!(!timeline.is_now(1), "the first edit is a past moment");
    let target = timeline.effective_restore(1).expect("a restore target");
    let live = store.restore(target).expect("rewind");
    assert_eq!(note(root), "v1\nv2\n", "the files are what they were then");

    // Scrub further back to the session start.
    let target = timeline.effective_restore(0).expect("a restore target");
    store.restore(target).expect("rewind further");
    assert_eq!(note(root), "v1\n", "back to the launch state");

    // Return to now restores the live working tree, including the work the poll never saw.
    assert!(timeline.is_now(2), "the newest moment is now");
    store.restore(&live).expect("return to now");
    assert_eq!(
        note(root),
        "v1\nv2\nv3\nunsaved\n",
        "nothing was lost to the rewind"
    );

    // And through all of it the repository itself never moved (Hard rule 3).
    assert_eq!(git_out(root, &["rev-parse", "HEAD"]), head_before);
    assert_eq!(
        git_out(root, &["for-each-ref", "--format=%(refname) %(objectname)"]),
        refs_before
    );
}
