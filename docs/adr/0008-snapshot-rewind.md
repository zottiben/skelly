# 0008. Timeline rewind restores the working tree from content snapshots

- Status: Accepted
- Date: 2026-08-06
- Deciders: maintainers
- Supersedes: ADR-0007's "only commits are restorable" and "rewind is read-only
  inspection" decisions. The rest of ADR-0007 (the in-session event log, the
  `Timeline`/`SessionEvent` model, the `Agent` transport staying open) stands.
- Related: AGENTS.md Hard rule 3 (rewind never moves HEAD/refs); ADR-0006 (git
  backend = the `git` CLI); design guide §10.7; `design/README.md` decision log

## Context

ADR-0007 shipped rewind as `git worktree add --detach <sha>` into a temp
directory, with only commit events restorable and the panes never repointed into
the checkout. In use, that means scrubbing the timeline does nothing observable:

1. **Almost nothing is restorable.** An ordinary session records a "session
   started" anchor and a series of edits, none of which are commits. Every event
   therefore inherited the same launch SHA, `Timeline::is_now` was true at every
   index, and selecting any entry was a no-op by construction.
2. **Even a hit is invisible.** When a commit *was* selected, the past state
   materialized in a temp directory the user never sees. The only change on screen
   was a banner.
3. **Edits after the first were not even recorded.** The poll keyed off which
   *paths* were dirty, so the second and every later edit to a file added no
   moment to scrub to.

The guide is unambiguous about the intent (§10.7): "Scrub the track (or click any
entry) to restore the codebase to that moment." A rewind the user cannot see is
not that feature.

The structural claim behind ADR-0007 - "non-destructive rewind can only target a
git object" - is true but was read too narrowly. It only follows that the target
must be a git *object*; it does not follow that the object must be a commit in the
user's repository, nor that the restore must land somewhere other than the working
tree.

## Decision

**Every recorded moment is a content snapshot in a Skelly-owned object store, and
restoring one puts those files back in the working tree.**

- **The store (`SnapshotStore`).** A bare git object store per repository, under a
  per-process directory in the OS temp dir - never inside the user's `.git`. Every
  command runs with `--git-dir` pointed at that store and `GIT_INDEX_FILE` at a
  private index, so no invocation has the user's `.git` in scope at all.
- **Capture.** `add -A` + `write-tree` against the user's working tree. Trees are
  content-addressed, so an idle poll yields the same id and records no moment;
  `.gitignore` applies, so build output is neither snapshotted nor disturbed.
  Capture runs on the git-poll thread, never the UI thread.
- **Restore.** `read-tree -u --reset <tree>`: content restored, files created
  since removed, files deleted since brought back, ignored files untouched.
  `SnapshotStore::restore` captures the state it replaces *first* and returns it,
  so the live working tree is parked in the store before anything is overwritten.
- **Return to now** restores that parked snapshot - byte for byte, including work
  the poll had not yet observed. It runs on `⌥⌘0`, on closing the dock, on a
  cross-repo `cd` or tab switch, and on quit.
- **`session.shadow_worktree`** gates rewind: false means the timeline is a log
  Skelly will not act on. There is no mode where Skelly rewinds without a way back.
- **`Repo::shadow_checkout`** stays as API for materializing a commit elsewhere;
  the timeline no longer uses it.

## Consequences

- **Hard rule 3 holds, and for a better reason.** The old argument was "git's
  `worktree add` can't move HEAD". The new one is stronger: Skelly never runs a
  git command with the user's `.git` as `GIT_DIR`, so HEAD, branches, refs, the
  reflog, and the repository's index are not merely unmoved but out of reach.
  Tests assert all five are byte-identical across a rewind cycle.
- **Rewind writes to the working tree.** This is the deliberate change, and the
  reason the "return to now" snapshot is taken before any write rather than
  assumed to equal the newest recorded moment. Nothing on disk is discarded by a
  rewind; it is moved into the store and restored on the way back.
- **Every moment is scrubbable**, so the timeline is as dense as the poll: one
  moment per cycle in which the codebase actually changed. Edit detection tracks
  each file's line counts, not just which paths are dirty, so repeated edits to
  one file each get their own entry.
- **A crash while rewound leaves the past state on disk.** The live snapshot
  survives in the temp store until the OS reaps it, so the work is recoverable,
  but not automatically. Restoring on next launch needs the timeline to persist
  (currently layout-only) - a follow-up, not a v0.1 blocker.
- **The store costs a `git add -A` per poll cycle** (~60ms warm on this repo, off
  the UI thread) and grows with the session's distinct content, deduplicated by
  git. It is dropped on exit.

## Alternatives considered

- **Keep read-only inspection, add a viewer.** Show the rewound tree's files in
  the dock instead of touching the working tree. Safe, but it makes the timeline a
  history browser rather than a rewind, and it cannot answer "does the build pass
  at that point", which is the reason to go back.
- **Snapshot into the user's repo** (`git stash create`, throwaway commits).
  Rejected in ADR-0007 and still rejected: it writes objects into the user's
  object database and risks their index. A separate `GIT_DIR` has neither problem
  and is no harder.
- **A filesystem watcher** instead of polling, for finer moments. Heavier (a watch
  over the whole tree) and orthogonal - the granularity limit is the snapshot
  cadence, not the change signal. Reconsider if the poll proves too coarse.
- **Confirm before each rewind.** A modal on every scrub step would make scrubbing
  unusable. The first restore of a session raises a toast naming the return-to-now
  binding instead, which is non-blocking and, with the live state parked, enough.
