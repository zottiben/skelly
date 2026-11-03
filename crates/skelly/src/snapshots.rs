//! The session timeline's snapshot stores, and the worker that rewinds through them (ADR-0008).
//!
//! Snapshot `git` work is slow in a large repository - a repo's first capture hashes every file it
//! tracks, tens of seconds in a checkout of tens of thousands of files - so none of it runs on the
//! UI thread. The git-poll thread captures, one [`Rewinder`] thread restores, and the UI only says
//! where the working tree should be.
//!
//! Each repo's store has a lock of its own, held across its `git` work, so a capture of one repo
//! never holds up a rewind of another. The poll only ever `try_lock`s a store (a busy one skips
//! that cycle); the worker waits, because a rewind must happen - and must not race a capture of
//! the same repo for its private index.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::mpsc::{self, Receiver, Sender};
use std::sync::{Arc, Mutex, PoisonError};
use std::thread::JoinHandle;

use skelly_session::SnapshotStore;

/// Why a rewind could not use a repo's store (no temp dir, no `git`).
const UNAVAILABLE: &str = "the snapshot store is unavailable";

/// One repo's snapshot store, opened on first use.
type Slot = Arc<Mutex<Option<SnapshotStore>>>;

/// The snapshot stores shared by the git-poll thread and the rewind worker.
pub(crate) struct Snapshots {
    /// The directory the stores live under (this process's session root; a temp dir in tests).
    store_root: PathBuf,
    /// One store per repo root. This lock is held only to find or add a slot, never across `git`.
    slots: Mutex<HashMap<PathBuf, Slot>>,
    /// The repo whose working tree is rewound, if any. It shows a past state, so the poll must not
    /// snapshot it - that would record the rewind itself as a moment. Only changed while that
    /// repo's store is locked, so the poll's check after locking a store cannot race a rewind.
    frozen: Mutex<Option<PathBuf>>,
}

impl Snapshots {
    /// The stores for a session under `store_root`, none opened yet.
    pub(crate) fn new(store_root: PathBuf) -> Self {
        Self {
            store_root,
            slots: Mutex::default(),
            frozen: Mutex::default(),
        }
    }

    /// Snapshot `root`'s working tree from the poll thread, returning its tree id.
    ///
    /// `None` when the repo is rewound, when the worker is restoring it right now, or when `git`
    /// fails: all cases where the honest answer is "no new moment observed this cycle".
    pub(crate) fn capture(&self, root: &Path) -> Option<String> {
        let slot = self.slot(root);
        let mut store = slot.try_lock().ok()?;
        if self.frozen().as_deref() == Some(root) {
            return None;
        }
        match self.open(&mut store, root)?.capture() {
            Ok(tree) => Some(tree),
            Err(err) => {
                tracing::warn!(%err, root = %root.display(), "snapshot capture failed");
                None
            }
        }
    }

    /// Restore `root`'s working tree to `tree` and freeze the repo against captures, returning the
    /// snapshot of the state it replaced. Waits out a capture of the same repo already under way.
    fn rewind(&self, root: &Path, tree: &str) -> Result<String, String> {
        let slot = self.slot(root);
        let mut store = slot.lock().map_err(|_| UNAVAILABLE.to_owned())?;
        let store = self
            .open(&mut store, root)
            .ok_or_else(|| UNAVAILABLE.to_owned())?;
        let live = store.restore(tree).map_err(|err| err.to_string())?;
        *self.frozen() = Some(root.to_path_buf());
        Ok(live)
    }

    /// Put the live state `rewind` parked back, and let the poll snapshot its repo again.
    fn give_back(&self, rewind: &Rewind) -> Result<(), String> {
        let slot = self.slot(&rewind.root);
        let mut store = slot.lock().map_err(|_| UNAVAILABLE.to_owned())?;
        let restored = self
            .open(&mut store, &rewind.root)
            .ok_or_else(|| UNAVAILABLE.to_owned())
            .and_then(|store| store.restore(&rewind.live).map_err(|err| err.to_string()));
        *self.frozen() = None;
        restored.map(drop)
    }

    /// The slot for `root`'s store, adding an empty one on first sight.
    fn slot(&self, root: &Path) -> Slot {
        let mut slots = self.slots.lock().unwrap_or_else(PoisonError::into_inner);
        Arc::clone(slots.entry(root.to_path_buf()).or_default())
    }

    /// The rewound repo, if any.
    fn frozen(&self) -> std::sync::MutexGuard<'_, Option<PathBuf>> {
        self.frozen.lock().unwrap_or_else(PoisonError::into_inner)
    }

    /// The store in `slot`, opening it on first use. `None` when it cannot be created (no temp
    /// dir, no `git`), which degrades the timeline to a log without rewind.
    ///
    /// A store whose directory has gone (the OS reaping the temp dir under a long-running session)
    /// is re-opened rather than used, so a vanished store costs one re-init instead of failing
    /// every capture for the rest of the session.
    fn open<'a>(
        &self,
        slot: &'a mut Option<SnapshotStore>,
        root: &Path,
    ) -> Option<&'a SnapshotStore> {
        if slot.as_ref().is_none_or(|s| !s.git_dir().is_dir()) {
            *slot = SnapshotStore::open_in(&self.store_root, root)
                .inspect_err(|err| {
                    tracing::warn!(%err, root = %root.display(), "no snapshot store for repo");
                })
                .ok();
        }
        slot.as_ref()
    }
}

/// Where the working tree should be: as it is now, or a past snapshot of one repo.
#[derive(Debug, Clone)]
pub(crate) enum Target {
    /// The live state - nothing rewound.
    Now,
    /// The snapshot `tree` of the repo at `root`: the moment the timeline calls `label`.
    Past {
        root: PathBuf,
        tree: String,
        label: String,
    },
}

/// What a request came to, for the UI to tell the user. Later steps of a rewind that went as asked
/// and a return to now that worked say nothing.
#[derive(Debug, PartialEq, Eq)]
pub(crate) enum Outcome {
    /// The first step away from now: the live state is parked and the files show moment `label`.
    Rewound(String),
    /// A rewind step failed, so the files are where they were.
    Failed(String),
    /// Putting the live state back failed, so the files still show a past moment.
    NotReturned(String),
}

/// A live rewind: a repo's working tree is showing a past snapshot.
///
/// Holding `live` is what makes the rewind non-destructive in the way that matters to the user -
/// the state the first restore replaced is parked in Skelly's store, so returning to now puts it
/// back byte for byte. HEAD, refs, and the repository's index are never in play at all.
struct Rewind {
    /// The repo whose working tree is rewound (only ever one at a time).
    root: PathBuf,
    /// The snapshot of the live state the rewind replaced, restored on return to now.
    live: String,
    /// The snapshot currently materialized in the working tree.
    viewing: String,
}

/// The rewind worker: the one thread that writes snapshots into a working tree.
///
/// [`request`](Self::request) hands it where the working tree should be and returns at once. The
/// worker applies only the newest request (a fast scrub restores where the user stopped, not every
/// moment on the way) and tells `report` how it went. Dropping it returns the working tree to now
/// before the thread ends, so quitting mid-scrub can never be how work goes missing.
pub(crate) struct Rewinder {
    /// The worker's inbox; `None` once it has been told to finish.
    requests: Option<Sender<Target>>,
    /// The worker thread, joined on [`finish`](Self::finish).
    worker: Option<JoinHandle<()>>,
}

impl Rewinder {
    /// Start the worker over `snapshots`, reporting each [`Outcome`] to `report`.
    pub(crate) fn spawn(
        snapshots: Arc<Snapshots>,
        report: impl Fn(Outcome) + Send + 'static,
    ) -> Self {
        let (requests, inbox) = mpsc::channel();
        let worker = std::thread::spawn(move || run(&snapshots, &inbox, &report));
        Self {
            requests: Some(requests),
            worker: Some(worker),
        }
    }

    /// Ask for the working tree to show `target`. Never waits: the worker applies it, and asking
    /// for where the working tree already is costs it nothing.
    pub(crate) fn request(&self, target: Target) {
        if let Some(requests) = &self.requests {
            let _ = requests.send(target);
        }
    }

    /// Return to now and wait for the worker to finish. Idempotent.
    pub(crate) fn finish(&mut self) {
        drop(self.requests.take());
        if let Some(worker) = self.worker.take() {
            let _ = worker.join();
        }
    }
}

impl Drop for Rewinder {
    fn drop(&mut self) {
        self.finish();
    }
}

/// The worker loop: apply the newest request until the UI hangs up, then return to now.
fn run(snapshots: &Snapshots, inbox: &Receiver<Target>, report: &dyn Fn(Outcome)) {
    let mut rewind = None;
    while let Ok(first) = inbox.recv() {
        let target = inbox.try_iter().last().unwrap_or(first);
        step(snapshots, &mut rewind, target, report);
    }
    step(snapshots, &mut rewind, Target::Now, report);
}

/// Move the working tree to `target`, keeping `rewind` true to what is on disk.
fn step(
    snapshots: &Snapshots,
    rewind: &mut Option<Rewind>,
    target: Target,
    report: &dyn Fn(Outcome),
) {
    let Target::Past { root, tree, label } = target else {
        if let Some(parked) = rewind.take() {
            if let Err(err) = snapshots.give_back(&parked) {
                tracing::warn!(%err, live = %parked.live, "failed to return to now");
                report(Outcome::NotReturned(err));
            }
        }
        return;
    };
    if rewind
        .as_ref()
        .is_some_and(|r| r.root == root && r.viewing == tree)
    {
        return;
    }
    // Only one repo is ever rewound: stepping into another repo's past puts the first one back.
    if rewind.as_ref().is_some_and(|r| r.root != root) {
        step(snapshots, rewind, Target::Now, report);
    }
    match snapshots.rewind(&root, &tree) {
        // A later step moves between past moments and keeps pointing at the live state the first
        // one parked; the past state it replaced is not worth keeping.
        Ok(_) if rewind.is_some() => {
            if let Some(r) = rewind {
                r.viewing = tree;
            }
        }
        Ok(live) => {
            *rewind = Some(Rewind {
                root,
                live,
                viewing: tree,
            });
            report(Outcome::Rewound(label));
        }
        Err(err) => {
            tracing::warn!(%err, %tree, "rewind failed");
            report(Outcome::Failed(err));
        }
    }
}

#[cfg(test)]
mod tests {
    use std::process::Command;
    use std::time::Duration;

    use super::{Outcome, Rewinder, Snapshots, Target};
    use std::path::{Path, PathBuf};
    use std::sync::mpsc::{self, Receiver};
    use std::sync::Arc;

    /// Longer than any rewind of a two-file repo, short enough that a wait shows as a failure.
    const PATIENCE: Duration = Duration::from_secs(10);

    /// Run `git -C <root> <args>` isolated from the host's git config, asserting success.
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

    /// A committed repo `name` under `dir` whose `notes.txt` reads `then`.
    fn repo(dir: &Path, name: &str) -> PathBuf {
        let root = dir.join(name);
        std::fs::create_dir_all(&root).expect("repo dir");
        git(&root, &["init", "-q", "-b", "main"]);
        std::fs::write(root.join("notes.txt"), "then\n").expect("write");
        git(&root, &["add", "."]);
        git(&root, &["commit", "-q", "-m", "init", "--no-gpg-sign"]);
        root
    }

    fn notes(root: &Path) -> String {
        std::fs::read_to_string(root.join("notes.txt")).expect("read notes")
    }

    fn past(root: &Path, tree: &str) -> Target {
        Target::Past {
            root: root.to_path_buf(),
            tree: tree.to_owned(),
            label: format!("moment {tree}"),
        }
    }

    /// A worker over `snapshots` whose outcomes arrive on the returned channel.
    fn rewinder(snapshots: &Arc<Snapshots>) -> (Rewinder, Receiver<Outcome>) {
        let (tx, rx) = mpsc::channel();
        let rewinder = Rewinder::spawn(Arc::clone(snapshots), move |outcome| {
            let _ = tx.send(outcome);
        });
        (rewinder, rx)
    }

    /// `root` captured as it is (its "then" tree), then edited so the live state differs.
    fn then_and_now(snapshots: &Snapshots, root: &Path) -> String {
        let then = snapshots.capture(root).expect("capture then");
        std::fs::write(root.join("notes.txt"), "now\n").expect("edit");
        std::fs::write(root.join("new.txt"), "made since\n").expect("create");
        then
    }

    #[test]
    fn asking_never_waits_for_a_capture() {
        let dir = tempfile::tempdir().expect("temp dir");
        let snapshots = Arc::new(Snapshots::new(dir.path().join("stores")));
        let root = repo(dir.path(), "repo");
        let then = then_and_now(&snapshots, &root);
        let (rewinder, _outcomes) = rewinder(&snapshots);
        // Every workspace, tab or focus switch to another repo asks for now, and the poll thread
        // holds a store for a whole capture - seconds in a large repository.
        let store = snapshots.slot(&root);
        let capturing = store.lock().expect("hold the store");
        let (done, finished) = mpsc::channel();
        std::thread::scope(|scope| {
            scope.spawn(|| {
                rewinder.request(past(&root, &then));
                rewinder.request(Target::Now);
                let _ = done.send(());
            });
            let waited = finished.recv_timeout(Duration::from_secs(2)).is_err();
            drop(capturing);
            assert!(!waited, "asking for a rewind waited for a capture");
        });
    }

    #[test]
    fn a_rewind_never_waits_for_another_repos_capture() {
        let dir = tempfile::tempdir().expect("temp dir");
        let snapshots = Arc::new(Snapshots::new(dir.path().join("stores")));
        let small = repo(dir.path(), "small");
        let large = repo(dir.path(), "large");
        let then = then_and_now(&snapshots, &small);
        // The poll thread holds a store for a whole capture - tens of seconds for a large repo's
        // first one.
        let large_store = snapshots.slot(&large);
        let capturing = large_store.lock().expect("hold the large repo's store");

        let (rewinder, outcomes) = rewinder(&snapshots);
        rewinder.request(past(&small, &then));
        let outcome = outcomes.recv_timeout(PATIENCE);
        drop(capturing);

        assert_eq!(outcome, Ok(Outcome::Rewound(format!("moment {then}"))));
        assert_eq!(notes(&small), "then\n");
    }

    #[test]
    fn a_rewind_waits_out_a_capture_of_its_own_repo() {
        let dir = tempfile::tempdir().expect("temp dir");
        let snapshots = Arc::new(Snapshots::new(dir.path().join("stores")));
        let root = repo(dir.path(), "repo");
        let then = then_and_now(&snapshots, &root);
        let store = snapshots.slot(&root);
        let capturing = store.lock().expect("hold the store");

        let (rewinder, outcomes) = rewinder(&snapshots);
        rewinder.request(past(&root, &then));
        // Both drive git through the same private index, so the restore must not start yet.
        assert!(outcomes.recv_timeout(Duration::from_millis(300)).is_err());
        assert_eq!(notes(&root), "now\n");
        drop(capturing);

        assert_eq!(
            outcomes.recv_timeout(PATIENCE),
            Ok(Outcome::Rewound(format!("moment {then}")))
        );
        assert_eq!(notes(&root), "then\n");
    }

    #[test]
    fn quitting_mid_rewind_puts_the_live_state_back() {
        let dir = tempfile::tempdir().expect("temp dir");
        let snapshots = Arc::new(Snapshots::new(dir.path().join("stores")));
        let root = repo(dir.path(), "repo");
        let index = std::fs::read(root.join(".git/index")).expect("read index");
        let then = then_and_now(&snapshots, &root);

        let (mut rewinder, outcomes) = rewinder(&snapshots);
        rewinder.request(past(&root, &then));
        assert!(outcomes.recv_timeout(PATIENCE).is_ok());
        assert_eq!(notes(&root), "then\n");
        assert!(!root.join("new.txt").exists(), "a file made since is gone");
        assert_eq!(snapshots.capture(&root), None, "a rewound repo is frozen");

        rewinder.finish();
        assert_eq!(notes(&root), "now\n");
        assert!(root.join("new.txt").exists(), "the file made since is back");
        assert_eq!(
            std::fs::read(root.join(".git/index")).expect("read index"),
            index,
            "the repository's own index never moves"
        );
        assert!(snapshots.capture(&root).is_some(), "the poll resumes");
    }

    #[test]
    fn a_failed_step_leaves_the_rewind_in_place() {
        let dir = tempfile::tempdir().expect("temp dir");
        let snapshots = Arc::new(Snapshots::new(dir.path().join("stores")));
        let root = repo(dir.path(), "repo");
        let then = then_and_now(&snapshots, &root);

        let (mut rewinder, outcomes) = rewinder(&snapshots);
        rewinder.request(past(&root, &then));
        assert!(outcomes.recv_timeout(PATIENCE).is_ok());
        rewinder.request(past(&root, "0000000000000000000000000000000000000000"));
        assert!(matches!(
            outcomes.recv_timeout(PATIENCE),
            Ok(Outcome::Failed(_))
        ));
        // Still rewound, so still frozen - the poll must not record the past state as a moment.
        assert_eq!(snapshots.capture(&root), None);
        assert_eq!(notes(&root), "then\n");

        rewinder.finish();
        assert_eq!(notes(&root), "now\n");
    }

    #[test]
    fn stepping_into_another_repos_past_puts_the_first_back() {
        let dir = tempfile::tempdir().expect("temp dir");
        let snapshots = Arc::new(Snapshots::new(dir.path().join("stores")));
        let first = repo(dir.path(), "first");
        let second = repo(dir.path(), "second");
        let first_then = then_and_now(&snapshots, &first);
        let second_then = then_and_now(&snapshots, &second);

        let (mut rewinder, outcomes) = rewinder(&snapshots);
        rewinder.request(past(&first, &first_then));
        assert!(outcomes.recv_timeout(PATIENCE).is_ok());
        rewinder.request(past(&second, &second_then));
        assert!(outcomes.recv_timeout(PATIENCE).is_ok());

        assert_eq!(notes(&first), "now\n");
        assert_eq!(notes(&second), "then\n");
        assert!(
            snapshots.capture(&first).is_some(),
            "only one repo is frozen"
        );
        rewinder.finish();
        assert_eq!(notes(&second), "now\n");
    }
}
