//! Content snapshots of a working tree, kept in a Skelly-owned object store **outside**
//! the user's repository.
//!
//! The session timeline needs every recorded moment to be restorable, not just the ones
//! that happen to be commits (ADR-0008 supersedes ADR-0007's commit-only rewind). A
//! snapshot is a plain git tree object written through a private `GIT_DIR` / `GIT_INDEX_FILE`
//! pair pointed at the user's working tree, so:
//!
//! - the user's `HEAD`, branches, refs, reflog, and `.git/index` are **never** touched
//!   (Hard rule 3) - Skelly only ever writes into its own store;
//! - `.gitignore` still applies, so build output (`target/`, `node_modules/`) is neither
//!   snapshotted nor disturbed by a restore;
//! - restoring is `read-tree -u --reset`, which makes the working tree match the snapshot
//!   exactly - content restored, files created since removed, files deleted since brought
//!   back - and leaves everything git ignores alone.
//!
//! [`SnapshotStore::restore`] returns the snapshot of the state it replaced, so a rewind is
//! always reversible: nothing the user had on disk is lost, it is just parked in the store.
//!
//! Stores are per repository and per process, under the OS temp directory, so two Skelly
//! processes never share an index and a session's snapshots cannot grow without bound.

use std::path::{Path, PathBuf};
use std::process::Command;

use crate::GitError;

/// The per-process directory (under the OS temp dir) holding this session's snapshot stores.
/// Snapshots are session state - the timeline itself does not persist across launches - so
/// scoping them to the pid keeps concurrent processes isolated and leaks self-cleaning.
#[must_use]
pub fn session_root() -> PathBuf {
    std::env::temp_dir().join(format!("skelly-snapshots-{}", std::process::id()))
}

/// A working tree's snapshot store: a bare git object store Skelly owns, wired to the
/// repository's working tree through `--work-tree` and a private index.
///
/// Created with [`SnapshotStore::open`]; every operation shells out to `git` (ADR-0006)
/// with `--git-dir` pointed at Skelly's store, so no command can reach the user's `.git`.
#[derive(Debug, Clone)]
pub struct SnapshotStore {
    /// Skelly's bare object store for this repository.
    git_dir: PathBuf,
    /// The user's working tree the snapshots are of.
    work_tree: PathBuf,
    /// Skelly's private index (never the repository's `.git/index`).
    index: PathBuf,
}

impl SnapshotStore {
    /// Open (creating on first use) the snapshot store for the working tree at `work_tree`,
    /// under this process's [`session_root`].
    ///
    /// # Errors
    /// Returns [`GitError`] if the store directory cannot be created or `git init --bare`
    /// cannot be run or fails.
    pub fn open(work_tree: &Path) -> Result<Self, GitError> {
        Self::open_in(&session_root(), work_tree)
    }

    /// Open the snapshot store for `work_tree` under an explicit `store_root` - the seam the
    /// trust-contract tests use to keep their stores in a temp directory.
    ///
    /// # Errors
    /// Returns [`GitError`] if the store directory cannot be created or `git init --bare`
    /// cannot be run or fails.
    pub fn open_in(store_root: &Path, work_tree: &Path) -> Result<Self, GitError> {
        let git_dir = store_root.join(store_name(work_tree));
        std::fs::create_dir_all(store_root).map_err(GitError::Spawn)?;
        if !git_dir.join("HEAD").exists() {
            let output = Command::new("git")
                .args(["init", "--bare", "-q"])
                .arg(&git_dir)
                .output()
                .map_err(GitError::Spawn)?;
            if !output.status.success() {
                return Err(GitError::Command {
                    args: "init --bare".to_owned(),
                    stderr: String::from_utf8_lossy(&output.stderr).trim().to_owned(),
                });
            }
        }
        // Belt and braces: git already refuses to add a nested `.git`, but the store's own
        // exclude file states it, so a repository whose `.git` is a file (a linked worktree)
        // can never end up inside a snapshot either.
        let info = git_dir.join("info");
        std::fs::create_dir_all(&info).map_err(GitError::Spawn)?;
        std::fs::write(info.join("exclude"), "/.git\n/.git/\n").map_err(GitError::Spawn)?;
        Ok(Self {
            index: git_dir.join("skelly-index"),
            git_dir,
            work_tree: work_tree.to_path_buf(),
        })
    }

    /// Skelly's object store directory for this repository (outside the user's `.git`).
    #[must_use]
    pub fn git_dir(&self) -> &Path {
        &self.git_dir
    }

    /// Snapshot the working tree as it is now and return the tree object id naming it.
    ///
    /// Content-addressed, so an unchanged working tree yields the same id as last time -
    /// which is how the timeline tells "something actually changed" from an idle poll.
    ///
    /// # Errors
    /// Returns [`GitError`] if `git` cannot be run or the `add` / `write-tree` fails.
    pub fn capture(&self) -> Result<String, GitError> {
        self.git(&["add", "-A"])?;
        Ok(self.git(&["write-tree"])?.trim().to_owned())
    }

    /// Restore the working tree to the snapshot `tree`, returning a fresh snapshot of the
    /// state that was replaced (so the caller can always put it back).
    ///
    /// Files are restored to their snapshot content, files created since are removed, and
    /// files deleted since are brought back. Anything git ignores is left untouched, and so
    /// are the repository's `HEAD`, refs, and index (Hard rule 3).
    ///
    /// # Errors
    /// Returns [`GitError`] if `git` cannot be run or the snapshot / `read-tree` fails. The
    /// replaced state is captured *before* anything is written, so a failure there leaves
    /// the working tree untouched.
    pub fn restore(&self, tree: &str) -> Result<String, GitError> {
        let replaced = self.capture()?;
        self.git(&["read-tree", "-u", "--reset", tree])?;
        Ok(replaced)
    }

    /// Run `git <args>` against Skelly's store with the user's working tree attached, and
    /// return its stdout.
    ///
    /// Every invocation goes through here, which is what makes the trust contract
    /// structural: `--git-dir` is always Skelly's store and `GIT_INDEX_FILE` is always
    /// Skelly's index, so no command has the repository's `.git` in scope. `core.bare=false`
    /// lets a bare store drive a working tree; the filter settings keep a snapshot a
    /// byte-faithful copy of what is on disk rather than a line-ending normalization of it.
    fn git(&self, args: &[&str]) -> Result<String, GitError> {
        let output = Command::new("git")
            .args(["-c", "core.bare=false"])
            .args(["-c", "core.autocrlf=false"])
            .args(["-c", "core.safecrlf=false"])
            .args(["-c", "core.fsmonitor=false"])
            .arg("--git-dir")
            .arg(&self.git_dir)
            .arg("--work-tree")
            .arg(&self.work_tree)
            .args(args)
            .current_dir(&self.work_tree)
            .env("GIT_INDEX_FILE", &self.index)
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

/// A store directory name for `work_tree`: its readable basename plus a hash of the full
/// path, so two checkouts of the same project never collide and the directory is still
/// recognizable when debugging.
fn store_name(work_tree: &Path) -> String {
    let base: String = work_tree
        .file_name()
        .map_or_else(|| "repo".to_owned(), |n| n.to_string_lossy().into_owned())
        .chars()
        .map(|c| if c.is_ascii_alphanumeric() { c } else { '-' })
        .take(32)
        .collect();
    format!("{base}-{:016x}.git", path_hash(work_tree))
}

/// FNV-1a over the path's bytes - a stable, dependency-free name for a store directory.
/// Not security-relevant: a collision would only make two repositories share a store.
fn path_hash(path: &Path) -> u64 {
    let mut hash: u64 = 0xcbf2_9ce4_8422_2325;
    for byte in path.to_string_lossy().as_bytes() {
        hash ^= u64::from(*byte);
        hash = hash.wrapping_mul(0x100_0000_01b3);
    }
    hash
}

#[cfg(test)]
mod tests {
    use super::{path_hash, store_name};
    use std::path::Path;

    #[test]
    fn store_names_are_readable_and_distinct_per_checkout() {
        let a = store_name(Path::new("/Users/dev/src/skelly"));
        let b = store_name(Path::new("/Users/dev/work/skelly"));
        assert!(a.starts_with("skelly-"), "{a} keeps the readable basename");
        assert_eq!(
            Path::new(&a).extension().and_then(|e| e.to_str()),
            Some("git")
        );
        assert_ne!(a, b, "same basename, different checkout - distinct stores");
    }

    #[test]
    fn store_names_sanitize_path_punctuation() {
        let name = store_name(Path::new("/tmp/my repo (v2)"));
        assert!(
            name.starts_with("my-repo--v2-"),
            "{name} replaces punctuation"
        );
    }

    #[test]
    fn path_hash_is_stable_for_the_same_path() {
        assert_eq!(
            path_hash(Path::new("/a/b")),
            path_hash(Path::new("/a/b")),
            "the store must be found again next time it is opened"
        );
    }
}
