# 0007. Session timeline: an in-session event log with shadow-worktree rewind

> **Superseded in part by [ADR-0008](0008-snapshot-rewind.md) (2026-08-06).** The
> event-log model, `Timeline`/`SessionEvent`, and the still-open `Agent` transport
> stand. What 0008 replaces: "only commits are restorable" and "rewind is read-only
> inspection" - every recorded moment is now a content snapshot in a Skelly-owned
> object store, and restoring one puts those files back in the working tree.

- Status: Accepted (rewind mechanism superseded by ADR-0008)
- Date: 2026-07-12
- Deciders: maintainers
- Related: AGENTS.md Hard rule 3 (non-destructive rewind never moves HEAD/refs -
  the feature's whole trust contract); Hard rule 4 (git diff + timeline open as a
  right dock, a layer, one at a time); ADR-0006 (git backend = the `git` CLI);
  `design/README.md` decision log (2026-07-12 timeline v1 scope); the engineering
  playbook (§4 mandatory trust-contract tests, §7 the rewind touches the user's
  repo - treat any path that could mutate real HEAD/refs as security-grade)

## Context

M4's remaining signature feature is the **session timeline + non-destructive
rewind**. The design guide (§10.7) shows a clickable record of the session -
human, agent, and system actions - where scrubbing to an entry "restores the
codebase to that moment" in a shadow worktree, "never rewriting history," and
`⌥⌘0` returns to HEAD.

Three product questions were flagged "Confirm first" and had to be settled before
the code lands (recorded in `design/README.md`): what the timeline *records* (the
AI-actions contract), what rewind *permits* (rewind + edit), and persist scope.
The mechanism is hard to reverse once the model + UI are written against it, so it
warrants an ADR alongside those product decisions.

Two structural facts constrain the design:

1. **Non-destructive rewind can only target a git object.** Restoring the working
   tree to a past moment without touching HEAD means checking that object out
   *somewhere else*. Only committed states are addressable this way; a mid-session
   "staged 2 files" moment is not a restorable point.
2. **There is no AI-actions transport yet.** How an external agent's actions reach
   the timeline is explicitly undecided and must be an explicit hook, not a shell
   heuristic - so v1 cannot depend on it.

## Decision

**The timeline is an in-session event log that Skelly records itself, and rewind is
a read-only shadow-worktree checkout.**

- **Model (`skelly-session`).** A `Timeline` owns an append-only `Vec<SessionEvent>`.
  Each `SessionEvent` has an `Actor` (`Human` / `Agent` / `System`), a title, a
  detail line, and an optional **restore target** (a commit SHA). The model is
  pure and clock-free (deterministic per playbook §4); the binary attaches the
  display time when it records an event. `Timeline::record` is the single append
  point, so an agent-events transport can feed it later without a model change.
- **What v1 records.** Skelly records the events it genuinely witnesses: a `System`
  "session started" event, and the `Human` git events it drives through the diff
  dock - a commit (restore target = the new HEAD SHA) and stage / unstage /
  stage-all (no restore target). The `Agent` actor exists but has no transport in
  v1 (the open AI-actions contract).
- **Rewind = shadow worktree.** Restoring a restorable event runs `git worktree add
  --detach <sha> <dir>` into a Skelly-owned temp directory (`ShadowWorktree`). By
  construction this creates a *separate* checkout in detached HEAD and never moves
  the main worktree's HEAD, branch, or any ref (Hard rule 3). "Return to now"
  (`⌥⌘0`) runs `git worktree remove --force`. The dock shows "viewing state at
  <event>"; Skelly does not repoint panes into the shadow tree or auto-fork
  (read-only inspection; fork-on-edit deferred).

## Consequences

- **The trust contract is git's own guarantee, re-checked adversarially.** Because
  the mechanism is `worktree add --detach`, "HEAD/refs untouched" is structural,
  not hand-rolled. The mandatory tests drive a real `git` against a throwaway repo
  and assert HEAD, the symbolic-ref branch, and `for-each-ref` are byte-identical
  across a full rewind -> return cycle, including when the checkout fails.
- **Only commits are restorable.** Non-commit events (staging) render in the log
  but have no rewind target - honest about what the mechanism can do. The richer
  fully-scrubbable timeline the mockup implies arrives with the AI-actions
  transport and (later) fork-on-edit.
- **Shadow worktrees are cleaned up.** A live `ShadowWorktree` is removed on return
  -to-now, on closing the dock, and best-effort on exit; `git worktree prune`
  reclaims any that leak. The temp dir lives outside the repo (git refuses a path
  inside `.git`).
- **Backend stays behind `skelly-session`.** Consistent with ADR-0006; swappable
  for libgit2 later if a hot path ever needs it.

## Alternatives considered

- **A pure git-commit timeline** (list `git log`, no in-session event log). Simpler
  and every entry restorable, but it drops the human/agent/system session-record
  framing the guide centers on, and shows commits made before launch as "session"
  events. Rejected in favor of the event log; the restore mechanism is identical.
- **Record fine-grained edits via a filesystem watcher** to make more moments
  restorable. Heavy (a watcher over the repo), and still not non-destructively
  restorable without committing each moment. Rejected.
- **Snapshot via `git stash create` / throwaway commits** instead of a worktree.
  Either writes objects/refs into the user's repo or risks touching the index -
  more surface to get the trust contract wrong than a detached worktree. Rejected.
- **Editable rewind now** (repoint panes into the shadow tree, fork a branch on
  edit). Much larger surface (pane-cwd redirection, edit detection, branch
  creation) for one M4 slice, and it widens the trust-contract attack surface.
  Deferred to a later slice; v1 is read-only.
