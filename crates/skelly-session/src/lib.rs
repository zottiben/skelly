//! `skelly-session` - the session timeline and git integration.
//!
//! Records the session as a timeline of human, agent, and system events, and
//! restores the codebase to any past moment. Rewind is **non-destructive**: it
//! checks out a shadow worktree and never rewrites history or moves HEAD (Hard
//! rule 3) - the whole trust contract of the feature. Also owns the per-repo git
//! diff model (changed files, hunk staging, commit).
//!
//! Independent of rendering; depends only on the `git` CLI (ADR-0006) and, later, the
//! config. Never on the binary. Git access is split into a thin invocation layer (the
//! private `git_stdout` runner) and pure parsers (the `diff` module + `parse_status`),
//! so the parsing is fully unit-tested without a git process.
//!
//! Status: M4 - the git diff **model** ([`Repo`] discovery, working status, per-file
//! unified diff), **per-file staging** ([`Repo::stage`] / [`Repo::unstage`] /
//! [`Repo::stage_all`]), and **committing** ([`Repo::commit`] / [`Repo::head_short`] /
//! [`Repo::undo_commit`]). Hunk-level staging and the timeline / shadow-worktree rewind
//! are follow-up slices.

#![doc(test(attr(deny(warnings))))]

mod diff;

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::process::Command;

use thiserror::Error;

pub use diff::{DiffLine, FileDiff, Hunk, LineKind};

/// Anything that can go wrong talking to git.
#[derive(Debug, Error)]
pub enum GitError {
    /// The `git` binary could not be launched (not installed / not on `PATH`).
    #[error("running git")]
    Spawn(#[source] std::io::Error),
    /// `git` ran but exited non-zero. Carries the invocation and its stderr.
    #[error("git {args} failed: {stderr}")]
    Command {
        /// The arguments passed to git (for diagnostics).
        args: String,
        /// The trimmed stderr git produced.
        stderr: String,
    },
}

/// A discovered git repository, identified by its working-tree root.
///
/// All queries shell out to `git -C <root>` (ADR-0006). Cheap to clone-by-path; holds
/// no handles or locks.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Repo {
    root: PathBuf,
}

impl Repo {
    /// Discover the repository containing `start` (a directory inside a working tree),
    /// via `git rev-parse --show-toplevel`.
    ///
    /// Returns `Ok(None)` when `start` is not inside a git repository - the diff dock's
    /// empty state, not an error.
    ///
    /// # Errors
    /// Returns [`GitError::Spawn`] only if `git` itself cannot be run.
    pub fn discover(start: &Path) -> Result<Option<Self>, GitError> {
        let output = Command::new("git")
            .arg("-C")
            .arg(start)
            .args(["rev-parse", "--show-toplevel"])
            .output()
            .map_err(GitError::Spawn)?;
        if !output.status.success() {
            // Not a repo (or `start` does not exist): a normal, non-error outcome.
            return Ok(None);
        }
        let root = String::from_utf8_lossy(&output.stdout).trim().to_owned();
        if root.is_empty() {
            return Ok(None);
        }
        Ok(Some(Self {
            root: PathBuf::from(root),
        }))
    }

    /// The repository's working-tree root.
    #[must_use]
    pub fn root(&self) -> &Path {
        &self.root
    }

    /// The current working status: the branch, ahead/behind counts, and the changed
    /// files (staged, unstaged, and untracked) with their line-change counts.
    ///
    /// # Errors
    /// Returns [`GitError`] if `git status` cannot be run or fails. Line counts are
    /// best-effort: if `git diff HEAD` fails (e.g. a repository with no commits yet),
    /// the files are still returned with zero counts.
    pub fn status(&self) -> Result<Status, GitError> {
        // `-c core.quotePath=false` keeps non-ASCII paths raw (unquoted) so parsing is
        // deterministic regardless of the user's git config (ADR-0006: parse defensively).
        let porcelain = self.git_stdout(&[
            "-c",
            "core.quotePath=false",
            "status",
            "--porcelain=v2",
            "--branch",
        ])?;
        let mut status = parse_status(&porcelain);
        if let Ok(numstat) = self.git_stdout(&["diff", "HEAD", "--numstat"]) {
            let counts = parse_numstat(&numstat);
            for file in &mut status.files {
                if let Some(&(added, removed)) = counts.get(&file.path) {
                    file.added = added;
                    file.removed = removed;
                }
            }
        }
        Ok(status)
    }

    /// The unified diff for one `path` (repo-relative), either the staged change
    /// (index vs HEAD, `staged = true`) or the unstaged change (working tree vs index).
    /// An unchanged path yields an empty [`FileDiff`].
    ///
    /// # Errors
    /// Returns [`GitError`] if `git diff` cannot be run or fails.
    pub fn diff(&self, path: &Path, staged: bool) -> Result<FileDiff, GitError> {
        let path = path.to_string_lossy();
        let mut args = vec!["diff", "--no-color", "-U3"];
        if staged {
            args.push("--cached");
        }
        args.push("--");
        args.push(&path);
        let out = self.git_stdout(&args)?;
        Ok(diff::parse_unified_diff(&out))
    }

    /// Stage `path` (repo-relative): `git add -- <path>`, which stages a modification, a
    /// deletion, or a previously-untracked file alike.
    ///
    /// # Errors
    /// Returns [`GitError`] if `git add` cannot be run or fails.
    pub fn stage(&self, path: &Path) -> Result<(), GitError> {
        let path = path.to_string_lossy();
        self.git_stdout(&["add", "--", &path])?;
        Ok(())
    }

    /// Unstage `path` (repo-relative): `git reset -q HEAD -- <path>`, restoring the index
    /// entry to HEAD (leaving the working tree untouched).
    ///
    /// # Errors
    /// Returns [`GitError`] if `git reset` cannot be run or fails - including in a
    /// repository with no commits yet (there is no HEAD to reset to).
    pub fn unstage(&self, path: &Path) -> Result<(), GitError> {
        let path = path.to_string_lossy();
        self.git_stdout(&["reset", "-q", "HEAD", "--", &path])?;
        Ok(())
    }

    /// Stage every change in the working tree: `git add -A` (modifications, deletions,
    /// and untracked files).
    ///
    /// # Errors
    /// Returns [`GitError`] if `git add` cannot be run or fails.
    pub fn stage_all(&self) -> Result<(), GitError> {
        self.git_stdout(&["add", "-A"])?;
        Ok(())
    }

    /// Commit the currently-staged changes with `message`: `git commit -m <message>`.
    /// The caller is responsible for ensuring something is staged and the message is
    /// non-empty (git rejects both). Signing follows the user's git config; a repo that
    /// requires a passphrase-prompted key can block, since the call is synchronous
    /// (moving git off the UI thread is a tracked follow-up).
    ///
    /// # Errors
    /// Returns [`GitError`] if `git commit` cannot be run or fails (nothing staged, an
    /// empty message, a failed hook, or a signing error).
    pub fn commit(&self, message: &str) -> Result<(), GitError> {
        self.git_stdout(&["commit", "-m", message])?;
        Ok(())
    }

    /// The short SHA of `HEAD`: `git rev-parse --short HEAD`.
    ///
    /// # Errors
    /// Returns [`GitError`] if `git rev-parse` cannot be run or fails (e.g. no commits).
    pub fn head_short(&self) -> Result<String, GitError> {
        Ok(self
            .git_stdout(&["rev-parse", "--short", "HEAD"])?
            .trim()
            .to_owned())
    }

    /// Undo the last commit, keeping its changes staged: `git reset --soft HEAD^`. This
    /// reverses a just-made [`Self::commit`] (moving `HEAD` back one, working tree and
    /// index untouched) - the "Undo" on the commit-success toast, distinct from the
    /// session-timeline rewind (Hard rule 3).
    ///
    /// # Errors
    /// Returns [`GitError`] if `git reset` cannot be run or fails - including when the
    /// last commit is the initial one (there is no `HEAD^` parent).
    pub fn undo_commit(&self) -> Result<(), GitError> {
        self.git_stdout(&["reset", "--soft", "HEAD^"])?;
        Ok(())
    }

    /// Stage (or, with `reverse`, unstage) a single `hunk` of `path` (repo-relative) by
    /// piping a reconstructed one-hunk patch to `git apply --cached [--reverse]`. Stage
    /// the hunk when the dock is showing the working-tree (unstaged) diff; unstage it
    /// (`reverse = true`) when showing the index (staged) diff.
    ///
    /// # Errors
    /// Returns [`GitError`] if `git apply` cannot be run or the patch does not apply
    /// cleanly (a stale diff, or a hunk whose last line lacks a trailing newline).
    pub fn apply_hunk(&self, path: &Path, hunk: &Hunk, reverse: bool) -> Result<(), GitError> {
        let buf = diff::hunk_patch(&path.to_string_lossy(), hunk);
        let mut args = vec!["apply", "--cached"];
        if reverse {
            args.push("--reverse");
        }
        self.git_apply(&args, &buf)
    }

    /// Run `git -C <root> <args>` feeding `stdin` to it, erroring on a non-zero exit. Used
    /// for `git apply`, which reads its patch from standard input.
    fn git_apply(&self, args: &[&str], stdin: &str) -> Result<(), GitError> {
        use std::io::Write as _;
        use std::process::Stdio;

        let mut child = Command::new("git")
            .arg("-C")
            .arg(&self.root)
            .args(args)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .map_err(GitError::Spawn)?;
        // Write the patch, then drop the handle to close stdin (EOF) before waiting, so
        // git does not block reading while we block on its output.
        if let Some(mut sink) = child.stdin.take() {
            sink.write_all(stdin.as_bytes()).map_err(GitError::Spawn)?;
        }
        let output = child.wait_with_output().map_err(GitError::Spawn)?;
        if !output.status.success() {
            return Err(GitError::Command {
                args: args.join(" "),
                stderr: String::from_utf8_lossy(&output.stderr).trim().to_owned(),
            });
        }
        Ok(())
    }

    /// Run `git -C <root> <args>` and return its stdout, erroring on a non-zero exit.
    fn git_stdout(&self, args: &[&str]) -> Result<String, GitError> {
        let output = Command::new("git")
            .arg("-C")
            .arg(&self.root)
            .args(args)
            .output()
            .map_err(GitError::Spawn)?;
        if !output.status.success() {
            return Err(GitError::Command {
                args: args.join(" "),
                stderr: String::from_utf8_lossy(&output.stderr).trim().to_owned(),
            });
        }
        Ok(String::from_utf8_lossy(&output.stdout).into_owned())
    }
}

/// The repository's working status: branch, upstream distance, and changed files.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct Status {
    /// The checked-out branch name, or `None` when HEAD is detached.
    pub branch: Option<String>,
    /// Commits ahead of the upstream branch (`0` when there is no upstream).
    pub ahead: u32,
    /// Commits behind the upstream branch (`0` when there is no upstream).
    pub behind: u32,
    /// The changed files (staged, unstaged, and untracked), in git's report order.
    pub files: Vec<ChangedFile>,
}

/// One changed file in the working status.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ChangedFile {
    /// The file's path, relative to the repository root.
    pub path: PathBuf,
    /// The original path, for a rename or copy (`None` otherwise).
    pub orig_path: Option<PathBuf>,
    /// The file's change kind (for the status letter + color).
    pub status: FileStatus,
    /// Whether the index differs from HEAD (there is something staged).
    pub staged: bool,
    /// Whether the working tree differs from the index (unstaged changes / untracked).
    pub unstaged: bool,
    /// Lines added vs HEAD (best-effort; `0` for untracked or when unavailable).
    pub added: u32,
    /// Lines removed vs HEAD (best-effort; `0` for untracked or when unavailable).
    pub removed: u32,
}

/// The kind of change to a file.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FileStatus {
    /// A newly added (or staged-new) file.
    Added,
    /// A modified file.
    Modified,
    /// A deleted file.
    Deleted,
    /// A renamed file.
    Renamed,
    /// A copied file.
    Copied,
    /// A type change (e.g. file <-> symlink).
    TypeChange,
    /// An unmerged (conflicted) file.
    Unmerged,
    /// An untracked file.
    Untracked,
}

impl FileStatus {
    /// The single-letter status code git uses (`A`/`M`/`D`/`R`/`C`/`T`/`U`/`?`).
    #[must_use]
    pub fn code(self) -> char {
        match self {
            FileStatus::Added => 'A',
            FileStatus::Modified => 'M',
            FileStatus::Deleted => 'D',
            FileStatus::Renamed => 'R',
            FileStatus::Copied => 'C',
            FileStatus::TypeChange => 'T',
            FileStatus::Unmerged => 'U',
            FileStatus::Untracked => '?',
        }
    }

    /// Map a porcelain XY status character to a [`FileStatus`].
    fn from_code(c: char) -> Self {
        match c {
            'A' => FileStatus::Added,
            'D' => FileStatus::Deleted,
            'R' => FileStatus::Renamed,
            'C' => FileStatus::Copied,
            'T' => FileStatus::TypeChange,
            'U' => FileStatus::Unmerged,
            _ => FileStatus::Modified,
        }
    }
}

/// Parse `git status --porcelain=v2 --branch` output into a [`Status`].
fn parse_status(porcelain: &str) -> Status {
    let mut status = Status::default();
    for line in porcelain.lines() {
        if let Some(head) = line.strip_prefix("# branch.head ") {
            status.branch = (head != "(detached)").then(|| head.to_owned());
        } else if let Some(ab) = line.strip_prefix("# branch.ab ") {
            for tok in ab.split_whitespace() {
                if let Some(a) = tok.strip_prefix('+') {
                    status.ahead = a.parse().unwrap_or(0);
                } else if let Some(b) = tok.strip_prefix('-') {
                    status.behind = b.parse().unwrap_or(0);
                }
            }
        } else if let Some(rest) = line.strip_prefix("1 ") {
            status.files.extend(parse_ordinary(rest));
        } else if let Some(rest) = line.strip_prefix("2 ") {
            status.files.extend(parse_rename(rest));
        } else if let Some(rest) = line.strip_prefix("u ") {
            status.files.extend(parse_unmerged(rest));
        } else if let Some(path) = line.strip_prefix("? ") {
            status.files.push(ChangedFile {
                path: PathBuf::from(path),
                orig_path: None,
                status: FileStatus::Untracked,
                staged: false,
                unstaged: true,
                added: 0,
                removed: 0,
            });
        }
        // "! <path>" (ignored) and "# branch.oid/upstream" are not surfaced.
    }
    status
}

/// Classify a two-char porcelain `XY` field into `(staged, unstaged, primary status)`.
/// `X` is the index-vs-HEAD state, `Y` the worktree-vs-index state; `.` means unchanged.
fn classify(xy: &str) -> (bool, bool, FileStatus) {
    let mut chars = xy.chars();
    let x = chars.next().unwrap_or('.');
    let y = chars.next().unwrap_or('.');
    let staged = x != '.';
    let unstaged = y != '.';
    // Prefer the staged code for the displayed kind, else the worktree code.
    let primary = if staged { x } else { y };
    (staged, unstaged, FileStatus::from_code(primary))
}

/// Parse an ordinary changed entry: `<XY> <sub> <mH> <mI> <mW> <hH> <hI> <path>`.
fn parse_ordinary(rest: &str) -> Option<ChangedFile> {
    // 7 fixed space-separated fields, then the path (which may contain spaces).
    let mut fields = rest.splitn(8, ' ');
    let xy = fields.next()?;
    for _ in 0..6 {
        fields.next()?;
    }
    let path = fields.next()?;
    let (staged, unstaged, status) = classify(xy);
    Some(ChangedFile {
        path: PathBuf::from(path),
        orig_path: None,
        status,
        staged,
        unstaged,
        added: 0,
        removed: 0,
    })
}

/// Parse a rename/copy entry: `<XY> <sub> ... <hI> <Xscore> <path>\t<origPath>`.
fn parse_rename(rest: &str) -> Option<ChangedFile> {
    // 8 fixed fields (XY..Xscore), then "<path>\t<origPath>".
    let mut fields = rest.splitn(9, ' ');
    let xy = fields.next()?;
    for _ in 0..7 {
        fields.next()?;
    }
    let paths = fields.next()?;
    let (path, orig) = paths.split_once('\t')?;
    let (staged, unstaged, primary) = classify(xy);
    let status = if matches!(primary, FileStatus::Copied) {
        FileStatus::Copied
    } else {
        FileStatus::Renamed
    };
    Some(ChangedFile {
        path: PathBuf::from(path),
        orig_path: Some(PathBuf::from(orig)),
        status,
        staged,
        unstaged,
        added: 0,
        removed: 0,
    })
}

/// Parse an unmerged entry: `<xy> <sub> <m1> <m2> <m3> <mW> <h1> <h2> <h3> <path>`.
fn parse_unmerged(rest: &str) -> Option<ChangedFile> {
    // 9 fixed fields, then the path.
    let mut fields = rest.splitn(10, ' ');
    fields.next()?; // xy (both sides conflicted)
    for _ in 0..8 {
        fields.next()?;
    }
    let path = fields.next()?;
    Some(ChangedFile {
        path: PathBuf::from(path),
        orig_path: None,
        status: FileStatus::Unmerged,
        staged: true,
        unstaged: true,
        added: 0,
        removed: 0,
    })
}

/// Parse `git diff --numstat` output into a `path -> (added, removed)` map. Binary
/// files (reported as `-`) map to `(0, 0)`.
fn parse_numstat(text: &str) -> HashMap<PathBuf, (u32, u32)> {
    let mut counts = HashMap::new();
    for line in text.lines() {
        let mut fields = line.splitn(3, '\t');
        if let (Some(added), Some(removed), Some(path)) =
            (fields.next(), fields.next(), fields.next())
        {
            counts.insert(
                PathBuf::from(path),
                (added.parse().unwrap_or(0), removed.parse().unwrap_or(0)),
            );
        }
    }
    counts
}

#[cfg(test)]
mod tests {
    use super::{parse_numstat, parse_status, FileStatus, PathBuf};

    #[test]
    fn parses_branch_and_ahead_behind() {
        let porcelain = "# branch.oid abc123\n\
             # branch.head feat/x\n\
             # branch.upstream origin/feat/x\n\
             # branch.ab +2 -1\n";
        let status = parse_status(porcelain);
        assert_eq!(status.branch.as_deref(), Some("feat/x"));
        assert_eq!((status.ahead, status.behind), (2, 1));
        assert!(status.files.is_empty());
    }

    #[test]
    fn a_detached_head_has_no_branch() {
        let status = parse_status("# branch.head (detached)\n");
        assert_eq!(status.branch, None);
    }

    #[test]
    fn classifies_staged_modified_untracked_and_renamed_files() {
        // A staged-modified file (X=M, Y=.), an unstaged-modified file (X=., Y=M),
        // a rename (X=R), and an untracked file.
        let porcelain = "# branch.head main\n\
             1 M. N... 100644 100644 100644 aaaa bbbb src/staged mod.rs\n\
             1 .M N... 100644 100644 100644 cccc dddd src/worktree.rs\n\
             2 R. N... 100644 100644 100644 eeee ffff R100 new/name.rs\told/name.rs\n\
             ? untracked.txt\n";
        let status = parse_status(porcelain);
        assert_eq!(status.files.len(), 4);

        let staged = &status.files[0];
        assert_eq!(staged.path, PathBuf::from("src/staged mod.rs")); // path with a space
        assert_eq!(staged.status, FileStatus::Modified);
        assert!(staged.staged && !staged.unstaged);

        let worktree = &status.files[1];
        assert_eq!(worktree.status, FileStatus::Modified);
        assert!(!worktree.staged && worktree.unstaged);

        let renamed = &status.files[2];
        assert_eq!(renamed.status, FileStatus::Renamed);
        assert_eq!(renamed.path, PathBuf::from("new/name.rs"));
        assert_eq!(renamed.orig_path, Some(PathBuf::from("old/name.rs")));

        let untracked = &status.files[3];
        assert_eq!(untracked.status, FileStatus::Untracked);
        assert_eq!(untracked.status.code(), '?');
    }

    #[test]
    fn numstat_maps_paths_to_counts_and_tolerates_binary() {
        let counts = parse_numstat("12\t3\tsrc/a.rs\n-\t-\tlogo.png\n");
        assert_eq!(counts.get(&PathBuf::from("src/a.rs")), Some(&(12, 3)));
        assert_eq!(counts.get(&PathBuf::from("logo.png")), Some(&(0, 0)));
    }
}
