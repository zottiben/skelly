//! Integration test: drive a real `git` against a throwaway repo in a temp dir and
//! assert the read-only diff model reports the right status and diff. This proves the
//! CLI invocation + parsing work end-to-end (the pure parsers are unit-tested in the
//! crate); it is the counterpart to the parser tests per ADR-0006.
//!
//! The repo is created from scratch under `tempfile::tempdir()`, isolated from the
//! user's global/system git config via `GIT_CONFIG_GLOBAL`/`GIT_CONFIG_SYSTEM` so the
//! result is deterministic regardless of the host's git settings.

use std::path::Path;
use std::process::Command;

use skelly_session::{FileStatus, Repo};

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

#[test]
fn discovers_repo_and_reports_status_and_diff() {
    let dir = tempfile::tempdir().expect("temp dir");
    let root = dir.path();

    git(root, &["init", "-b", "main"]);
    std::fs::write(root.join("a.txt"), "one\ntwo\nthree\n").expect("write a");
    git(root, &["add", "."]);
    git(root, &["commit", "-m", "init", "--no-gpg-sign"]);

    // Modify a tracked file (unstaged) and add a new untracked file.
    std::fs::write(root.join("a.txt"), "one\ntwo changed\nthree\nfour\n").expect("edit a");
    std::fs::write(root.join("b.txt"), "new file\n").expect("write b");

    let repo = Repo::discover(root)
        .expect("discover runs")
        .expect("start is inside a repo");
    // `--show-toplevel` and the temp dir both resolve symlinks (macOS /var -> /private/var).
    assert_eq!(
        repo.root().canonicalize().unwrap(),
        root.canonicalize().unwrap()
    );

    let status = repo.status().expect("status");
    assert_eq!(status.branch.as_deref(), Some("main"));

    let a = status
        .files
        .iter()
        .find(|f| f.path == Path::new("a.txt"))
        .expect("a.txt is changed");
    assert_eq!(a.status, FileStatus::Modified);
    assert!(a.unstaged && !a.staged);
    // Counts vs HEAD: "two" -> "two changed" (1 del, 1 add) plus "four" (1 add).
    assert_eq!((a.added, a.removed), (2, 1));

    let b = status
        .files
        .iter()
        .find(|f| f.path == Path::new("b.txt"))
        .expect("b.txt is untracked");
    assert_eq!(b.status, FileStatus::Untracked);

    // The unstaged diff of a.txt has the expected hunk stats.
    let diff = repo.diff(Path::new("a.txt"), false).expect("diff a.txt");
    assert_eq!(diff.stats(), (2, 1));
    assert!(!diff.hunks.is_empty());

    // A path with no changes yields an empty diff, not an error.
    let clean = repo.diff(Path::new("a.txt"), true).expect("staged diff");
    assert!(clean.hunks.is_empty(), "nothing is staged");
}

#[test]
fn staging_moves_a_file_between_unstaged_and_staged() {
    let dir = tempfile::tempdir().expect("temp dir");
    let root = dir.path();

    git(root, &["init", "-b", "main"]);
    std::fs::write(root.join("a.txt"), "one\n").expect("write a");
    git(root, &["add", "."]);
    git(root, &["commit", "-m", "init", "--no-gpg-sign"]);

    // Modify a tracked file (unstaged) and add a new untracked file.
    std::fs::write(root.join("a.txt"), "one\ntwo\n").expect("edit a");
    std::fs::write(root.join("b.txt"), "new\n").expect("write b");

    let repo = Repo::discover(root).expect("discover").expect("in a repo");

    let staged_state = |repo: &Repo, name: &str| {
        repo.status()
            .expect("status")
            .files
            .into_iter()
            .find(|f| f.path == Path::new(name))
            .map(|f| (f.staged, f.unstaged))
    };

    // Both files start fully unstaged.
    assert_eq!(staged_state(&repo, "a.txt"), Some((false, true)));
    assert_eq!(staged_state(&repo, "b.txt"), Some((false, true)));

    // Stage the modified file: now staged, nothing left unstaged.
    repo.stage(Path::new("a.txt")).expect("stage a");
    assert_eq!(staged_state(&repo, "a.txt"), Some((true, false)));

    // Unstage it again: back to unstaged only.
    repo.unstage(Path::new("a.txt")).expect("unstage a");
    assert_eq!(staged_state(&repo, "a.txt"), Some((false, true)));

    // Stage-all stages both (the untracked file becomes an added/staged entry).
    repo.stage_all().expect("stage all");
    assert_eq!(staged_state(&repo, "a.txt"), Some((true, false)));
    assert_eq!(staged_state(&repo, "b.txt").map(|(s, _)| s), Some(true));
}

#[test]
fn commit_stages_into_history_and_undo_soft_resets() {
    let dir = tempfile::tempdir().expect("temp dir");
    let root = dir.path();

    git(root, &["init", "-b", "main"]);
    // Local identity + no signing, so `Repo::commit` (which does not inherit the test's
    // GIT_AUTHOR_* env) has a committer regardless of the host's global git config.
    git(root, &["config", "user.name", "Test"]);
    git(root, &["config", "user.email", "test@example.com"]);
    git(root, &["config", "commit.gpgsign", "false"]);
    std::fs::write(root.join("a.txt"), "one\n").expect("write a");
    git(root, &["add", "."]);
    git(root, &["commit", "-m", "init", "--no-gpg-sign"]);
    let first = head_sha(root);

    // Stage a change and commit it through the model.
    std::fs::write(root.join("a.txt"), "one\ntwo\n").expect("edit a");
    let repo = Repo::discover(root).expect("discover").expect("in a repo");
    repo.stage(Path::new("a.txt")).expect("stage");
    repo.commit("add a line").expect("commit");

    // HEAD advanced, the working tree is clean, and the short SHA resolves.
    let after_commit = head_sha(root);
    assert_ne!(after_commit, first, "HEAD advanced past the initial commit");
    assert!(
        repo.status().expect("status").files.is_empty(),
        "tree is clean"
    );
    let short = repo.head_short().expect("head short");
    assert!(
        after_commit.starts_with(&short),
        "short sha is a prefix of HEAD"
    );

    // Undo soft-resets: HEAD returns to the first commit, but the change is re-staged.
    repo.undo_commit().expect("undo");
    assert_eq!(head_sha(root), first, "HEAD moved back one commit");
    let a = repo
        .status()
        .expect("status")
        .files
        .into_iter()
        .find(|f| f.path == Path::new("a.txt"))
        .expect("a.txt is changed again");
    assert!(
        a.staged && !a.unstaged,
        "the undone change is staged, not lost"
    );
}

/// The full SHA of `HEAD` in `root` (config-isolated), for asserting commit movement.
fn head_sha(root: &Path) -> String {
    let out = Command::new("git")
        .arg("-C")
        .arg(root)
        .args(["rev-parse", "HEAD"])
        .env("GIT_CONFIG_GLOBAL", "/dev/null")
        .env("GIT_CONFIG_SYSTEM", "/dev/null")
        .output()
        .expect("run git rev-parse");
    assert!(out.status.success(), "git rev-parse failed");
    String::from_utf8_lossy(&out.stdout).trim().to_owned()
}

#[test]
fn discover_outside_a_repo_is_none() {
    let dir = tempfile::tempdir().expect("temp dir");
    // A bare temp dir is not a git repo (and not inside one).
    let found = Repo::discover(dir.path()).expect("discover runs");
    assert!(found.is_none());
}
