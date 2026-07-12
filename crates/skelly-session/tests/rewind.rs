//! Integration test for the **non-destructive rewind** trust contract (AGENTS Hard rule
//! 3, ADR-0007): drive a real `git` against a throwaway repo and assert that checking out a
//! past commit into a shadow worktree - and later discarding it - never moves HEAD, the
//! branch, or any ref. This is the feature's whole trust contract, so it is tested
//! adversarially (including the case where the checkout itself fails).
//!
//! The repo is isolated from the host's git config (`GIT_CONFIG_GLOBAL`/`_SYSTEM`) so the
//! result is deterministic regardless of the developer's settings.

use std::path::Path;
use std::process::Command;

use skelly_session::Repo;

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

/// A snapshot of every ref-ish thing the rewind must never touch.
fn ref_state(root: &Path) -> (String, String, String) {
    (
        git_out(root, &["rev-parse", "HEAD"]),
        git_out(root, &["symbolic-ref", "HEAD"]),
        git_out(root, &["for-each-ref", "--format=%(refname) %(objectname)"]),
    )
}

/// Build a repo with two commits on `main`; return `(root, sha_first, sha_head)`.
fn two_commit_repo(root: &Path) -> (String, String) {
    git(root, &["init", "-b", "main"]);
    std::fs::write(root.join("f.txt"), "first\n").expect("write v1");
    git(root, &["add", "."]);
    git(root, &["commit", "-m", "first", "--no-gpg-sign"]);
    let first = git_out(root, &["rev-parse", "HEAD"]);

    std::fs::write(root.join("f.txt"), "second\n").expect("write v2");
    git(root, &["add", "."]);
    git(root, &["commit", "-m", "second", "--no-gpg-sign"]);
    let head = git_out(root, &["rev-parse", "HEAD"]);
    (first, head)
}

#[test]
fn shadow_checkout_restores_past_state_without_moving_head_or_refs() {
    let dir = tempfile::tempdir().expect("temp dir");
    let root = dir.path();
    let (first, head) = two_commit_repo(root);

    let before = ref_state(root);
    let repo = Repo::discover(root)
        .expect("discover runs")
        .expect("in a repo");

    // Rewind to the first commit in a shadow worktree.
    let shadow = repo.shadow_checkout(&first).expect("shadow checkout");

    // The shadow worktree holds the PAST state (f.txt == "first").
    let shadow_file = shadow.path().join("f.txt");
    assert_eq!(
        std::fs::read_to_string(&shadow_file).expect("read shadow file"),
        "first\n",
        "the shadow worktree is checked out to the first commit"
    );
    assert_eq!(shadow.committish(), first);
    // git knows about the shadow worktree (registered, detached).
    let worktrees = git_out(root, &["worktree", "list"]);
    assert!(
        worktrees.contains(&shadow.path().to_string_lossy().to_string())
            || worktrees.lines().count() >= 2,
        "the shadow worktree is registered: {worktrees}"
    );

    // THE TRUST CONTRACT: the main worktree's HEAD / branch / refs are untouched.
    assert_eq!(
        ref_state(root),
        before,
        "rewind must not move HEAD or any ref"
    );
    assert_eq!(git_out(root, &["rev-parse", "HEAD"]), head);
    // ...and the main working tree still holds the current state, not the past one.
    assert_eq!(
        std::fs::read_to_string(root.join("f.txt")).expect("read main file"),
        "second\n",
        "the main worktree is not disturbed by the rewind"
    );

    // Return to now: discard the shadow worktree.
    let shadow_path = shadow.path().to_path_buf();
    shadow.discard().expect("discard");
    assert!(!shadow_path.exists(), "the shadow worktree is removed");
    assert_eq!(
        ref_state(root),
        before,
        "return-to-now must also leave HEAD and every ref untouched"
    );
}

#[test]
fn a_failed_shadow_checkout_leaves_refs_untouched() {
    let dir = tempfile::tempdir().expect("temp dir");
    let root = dir.path();
    let _ = two_commit_repo(root);

    let before = ref_state(root);
    let repo = Repo::discover(root)
        .expect("discover runs")
        .expect("in a repo");

    // An invalid committish must fail cleanly - and adversarially, still touch nothing.
    let result = repo.shadow_checkout("0000000000000000000000000000000000000000");
    assert!(result.is_err(), "checking out a bogus commit fails");
    assert_eq!(
        ref_state(root),
        before,
        "a failed rewind must not move HEAD or any ref"
    );
}

#[test]
fn dropping_a_shadow_worktree_cleans_it_up() {
    let dir = tempfile::tempdir().expect("temp dir");
    let root = dir.path();
    let (first, _) = two_commit_repo(root);
    let repo = Repo::discover(root)
        .expect("discover runs")
        .expect("in a repo");

    let shadow_path = {
        let shadow = repo.shadow_checkout(&first).expect("shadow checkout");
        let path = shadow.path().to_path_buf();
        assert!(path.exists());
        path
        // `shadow` dropped here without an explicit discard - the drop guard tidies it.
    };
    assert!(
        !shadow_path.exists(),
        "dropping the handle removes the shadow worktree"
    );
}
