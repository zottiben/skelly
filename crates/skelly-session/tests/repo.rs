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

use skelly_session::{FileStatus, Repo, Skipped, Staging};

/// A 20-line file `line 1..line 20`, where `edit(n)` may override line `n`'s text.
fn numbered(edit: impl Fn(u32) -> Option<&'static str>) -> String {
    use std::fmt::Write as _;
    let mut buf = String::new();
    for n in 1..=20 {
        match edit(n) {
            Some(text) => writeln!(buf, "{text}").unwrap(),
            None => writeln!(buf, "line {n}").unwrap(),
        }
    }
    buf
}

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
fn linked_worktree_stays_scoped_to_its_checkout_and_branch() {
    let dir = tempfile::tempdir().expect("temp dir");
    let main = dir.path().join("main");
    let worktree = dir.path().join("agent-worktree");
    std::fs::create_dir(&main).expect("create main checkout");

    git(&main, &["init", "-b", "main"]);
    std::fs::write(main.join("shared.txt"), "base\n").expect("write base");
    git(&main, &["add", "."]);
    git(&main, &["commit", "-m", "init", "--no-gpg-sign"]);
    git(
        &main,
        &[
            "worktree",
            "add",
            "-b",
            "agent/one",
            worktree.to_str().expect("utf-8 worktree path"),
        ],
    );

    // Only the linked checkout is dirty. Discovery from a nested directory must retain that
    // checkout as the command root rather than collapsing through the shared git dir to `main`.
    std::fs::create_dir(worktree.join("src")).expect("create nested dir");
    std::fs::write(worktree.join("shared.txt"), "agent edit\n").expect("edit worktree");
    let linked = Repo::discover(&worktree.join("src"))
        .expect("discover linked worktree")
        .expect("linked checkout is a repo");
    assert_eq!(
        linked.root().canonicalize().unwrap(),
        worktree.canonicalize().unwrap()
    );

    let linked_status = linked.status().expect("linked status");
    assert_eq!(linked_status.branch.as_deref(), Some("agent/one"));
    assert_eq!(linked_status.files.len(), 1);
    assert_eq!(linked_status.files[0].path, Path::new("shared.txt"));
    assert!(!linked
        .diff(Path::new("shared.txt"), false)
        .unwrap()
        .hunks
        .is_empty());

    // The primary checkout has its own branch, index, and clean working tree.
    let primary = Repo::discover(&main)
        .expect("discover primary")
        .expect("primary checkout is a repo");
    let primary_status = primary.status().expect("primary status");
    assert_eq!(primary_status.branch.as_deref(), Some("main"));
    assert!(primary_status.files.is_empty());

    // Staging through the linked Repo must update that worktree's index only.
    linked
        .stage(Path::new("shared.txt"))
        .expect("stage linked edit");
    let staged = linked.status().expect("linked staged status");
    assert!(staged.files[0].staged && !staged.files[0].unstaged);
    assert!(primary
        .status()
        .expect("primary still clean")
        .files
        .is_empty());
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
fn apply_hunk_stages_and_unstages_a_single_hunk() {
    let dir = tempfile::tempdir().expect("temp dir");
    let root = dir.path();

    git(root, &["init", "-b", "main"]);
    std::fs::write(root.join("f.txt"), numbered(|_| None)).expect("write f");
    git(root, &["add", "."]);
    git(root, &["commit", "-m", "init", "--no-gpg-sign"]);

    // Two well-separated edits produce two independent hunks.
    let edited = numbered(|n| match n {
        3 => Some("line 3 changed"),
        18 => Some("line 18 changed"),
        _ => None,
    });
    std::fs::write(root.join("f.txt"), edited).expect("edit f");

    let repo = Repo::discover(root).expect("discover").expect("in a repo");
    let f = Path::new("f.txt");
    let diff = repo.diff(f, false).expect("unstaged diff");
    assert_eq!(diff.hunks.len(), 2, "two separate hunks");

    // Stage only the first hunk: the file becomes partially staged.
    repo.apply_hunk(f, &diff.hunks[0], false)
        .expect("stage hunk 0");
    let changed = |repo: &Repo| {
        repo.status()
            .expect("status")
            .files
            .into_iter()
            .find(|c| c.path == f)
            .expect("f.txt changed")
    };
    let partial = changed(&repo);
    assert!(partial.staged && partial.unstaged, "partially staged");

    // The staged side holds only the line-3 change; the unstaged side only line-18.
    let staged = repo.diff(f, true).expect("staged diff");
    assert_eq!(staged.hunks.len(), 1);
    assert!(staged.hunks[0]
        .lines
        .iter()
        .any(|l| l.text.contains("line 3 changed")));
    let unstaged = repo.diff(f, false).expect("unstaged diff");
    assert_eq!(unstaged.hunks.len(), 1);
    assert!(unstaged.hunks[0]
        .lines
        .iter()
        .any(|l| l.text.contains("line 18 changed")));

    // Reverse-applying the staged hunk unstages it again.
    repo.apply_hunk(f, &staged.hunks[0], true)
        .expect("unstage hunk 0");
    let after = changed(&repo);
    assert!(!after.staged && after.unstaged, "fully unstaged again");
}

#[test]
fn changes_read_staged_unstaged_and_untracked_work_against_head() {
    let dir = tempfile::tempdir().expect("temp dir");
    let root = dir.path();
    git(root, &["init", "-b", "main"]);
    std::fs::write(root.join("f.txt"), numbered(|_| None)).expect("write f");
    std::fs::write(root.join("gone.txt"), "bye\n").expect("write gone");
    git(root, &["add", "."]);
    git(root, &["commit", "-m", "init", "--no-gpg-sign"]);

    // One edit staged, one not, a deletion, and an untracked directory with a text file and a
    // binary one in it - which `git status` reports as the directory alone.
    std::fs::write(
        root.join("f.txt"),
        numbered(|n| (n == 3).then_some("line 3 staged")),
    )
    .expect("edit f");
    git(root, &["add", "f.txt"]);
    std::fs::write(
        root.join("f.txt"),
        numbered(|n| match n {
            3 => Some("line 3 staged"),
            18 => Some("line 18 unstaged"),
            _ => None,
        }),
    )
    .expect("edit f again");
    std::fs::remove_file(root.join("gone.txt")).expect("delete");
    std::fs::create_dir(root.join("new")).expect("mkdir");
    std::fs::write(root.join("new/a b.txt"), "first\nsecond\n").expect("write new");
    std::fs::write(root.join("new/blob"), [0_u8, 159, 146, 150]).expect("write blob");

    let repo = Repo::discover(root).expect("discover").expect("in a repo");
    let changes = repo.changes().expect("changes");
    let paths: Vec<&str> = changes.iter().map(|c| c.path.to_str().unwrap()).collect();
    assert_eq!(paths, ["f.txt", "gone.txt", "new/a b.txt", "new/blob"]);

    let f = &changes[0];
    assert_eq!(f.status, FileStatus::Modified);
    assert_eq!(
        f.diff.hunks.len(),
        2,
        "the staged and the unstaged edit, together"
    );
    assert_eq!(changes[1].status, FileStatus::Deleted);
    assert_eq!(changes[1].diff.stats(), (0, 1));
    assert_eq!(changes[2].status, FileStatus::Untracked);
    assert_eq!(changes[2].diff.stats(), (2, 0));
    assert_eq!(changes[3].skipped, Some(Skipped::Binary));

    // Reading changes nothing: the same call gives the same fingerprints.
    let again = repo.changes().expect("changes again");
    assert_eq!(again, changes);
}

#[test]
fn changes_in_a_repository_with_no_commits_are_against_nothing() {
    let dir = tempfile::tempdir().expect("temp dir");
    let root = dir.path();
    git(root, &["init", "-b", "main"]);
    std::fs::write(root.join("staged.txt"), "a\n").expect("write");
    git(root, &["add", "."]);
    std::fs::write(root.join("loose.txt"), "b\n").expect("write");
    let repo = Repo::discover(root).expect("discover").expect("in a repo");
    let changes = repo.changes().expect("changes");
    let kinds: Vec<(&str, FileStatus)> = changes
        .iter()
        .map(|c| (c.path.to_str().unwrap(), c.status))
        .collect();
    assert_eq!(
        kinds,
        [
            ("staged.txt", FileStatus::Added),
            ("loose.txt", FileStatus::Untracked),
        ]
    );
}

#[test]
fn toggle_hunk_stages_what_is_unstaged_then_unstages_what_is_staged() {
    let dir = tempfile::tempdir().expect("temp dir");
    let root = dir.path();
    git(root, &["init", "-b", "main"]);
    std::fs::write(root.join("f.txt"), numbered(|_| None)).expect("write f");
    git(root, &["add", "."]);
    git(root, &["commit", "-m", "init", "--no-gpg-sign"]);
    std::fs::write(
        root.join("f.txt"),
        numbered(|n| (n == 3).then_some("line 3 staged")),
    )
    .expect("edit f");
    git(root, &["add", "f.txt"]);
    std::fs::write(
        root.join("f.txt"),
        numbered(|n| match n {
            3 => Some("line 3 staged"),
            18 => Some("line 18 unstaged"),
            _ => None,
        }),
    )
    .expect("edit f again");

    let repo = Repo::discover(root).expect("discover").expect("in a repo");
    let f = Path::new("f.txt");
    let against_head = repo.changes().expect("changes").remove(0).diff;
    let (staged_part, unstaged_part) = (&against_head.hunks[0], &against_head.hunks[1]);

    // The line-3 hunk is already staged, so toggling it unstages it.
    assert_eq!(
        repo.toggle_hunk(f, staged_part).expect("toggle 3"),
        Staging::Unstaged
    );
    assert!(repo.diff(f, true).expect("staged").hunks.is_empty());
    // The line-18 hunk is not, so toggling it stages it - and only it.
    assert_eq!(
        repo.toggle_hunk(f, unstaged_part).expect("toggle 18"),
        Staging::Staged
    );
    let staged = repo.diff(f, true).expect("staged");
    assert_eq!(staged.hunks.len(), 1);
    assert!(staged.hunks[0]
        .lines
        .iter()
        .any(|l| l.text == "line 18 unstaged"));
    // Toggled again, it goes back.
    assert_eq!(
        repo.toggle_hunk(f, unstaged_part).expect("toggle 18 back"),
        Staging::Unstaged
    );
    assert!(repo.diff(f, true).expect("staged").hunks.is_empty());
}

#[test]
fn discover_outside_a_repo_is_none() {
    let dir = tempfile::tempdir().expect("temp dir");
    // A bare temp dir is not a git repo (and not inside one).
    let found = Repo::discover(dir.path()).expect("discover runs");
    assert!(found.is_none());
}

#[test]
fn init_creates_a_repository_that_discovers() {
    let dir = tempfile::tempdir().expect("temp dir");
    let root = dir.path();

    // A bare temp dir is not a repo; the "Init repo" empty-state action creates one.
    assert!(Repo::discover(root).expect("discover runs").is_none());
    let repo = Repo::init(root).expect("git init");

    // The freshly-initialized repo now discovers, and its status is a clean, file-less
    // tree (no commits yet, so line counts fall back to zero without erroring).
    assert!(Repo::discover(root).expect("discover runs").is_some());
    let status = repo.status().expect("status runs on a fresh repo");
    assert!(status.files.is_empty(), "a fresh repo has no changes");
}
