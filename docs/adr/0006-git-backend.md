# 0006. Git backend: the `git` CLI, not libgit2

- Status: Accepted
- Date: 2026-07-12
- Deciders: maintainers
- Related: AGENTS.md Hard rule 3 (non-destructive rewind never moves HEAD/refs);
  Hard rule 4 (git diff + timeline open as a right dock, a layer); the engineering
  playbook (§5 "backends go behind Skelly-owned traits so they stay swappable"; §7
  threat model - the opened repo may be hostile); `skelly-session` crate

## Context

M4 needs git integration: the per-repo diff dock (changed-file list, hunk-level
staging, commit) and the session timeline's non-destructive rewind (a shadow
worktree checkout that never rewrites history or moves HEAD - the feature's whole
trust contract). This all lives in `skelly-session`, which today is a stub.

There are two ways to talk to git from Rust: link **libgit2** (via the `git2`
crate) or shell out to the **`git` CLI** (`std::process::Command`). The choice is
hard to reverse once the diff/staging/rewind code is written against one API, so it
warrants an ADR.

## Decision

Talk to git by **invoking the `git` CLI**, wrapped behind a Skelly-owned type in
`skelly-session` (a thin `git(root, args)` runner plus typed parsers), not by
linking libgit2.

Rationale, weighted for this project's stated priorities (robustness, simplicity,
lean dependencies, long-term maintainability over dev cost):

- **Leanest dependency graph.** No new C dependency, no `libgit2-sys` build/link
  step, and no third-party-license question in `cargo deny`. "A minimal terminal
  earns its minimalism."
- **`git` is guaranteed present.** Skelly targets developers using vim/neovim in a
  repo; the `git` binary is the very thing being diffed. A terminal emulator whose
  core competency is already spawning and managing subprocesses shelling out to
  `git` is philosophically aligned, not a workaround.
- **Stable, machine-readable contracts.** `status --porcelain=v2 --branch`,
  `diff`/`--numstat`, `apply --cached`, and `worktree` are explicitly stable
  plumbing interfaces designed to be scripted.
- **The trust contract falls out of the tool.** The shadow-worktree rewind maps to
  `git worktree add --detach <path> <commit>`, which by construction creates a
  separate checkout and never touches the main worktree's HEAD or refs (Hard rule
  3). Enforcing "HEAD untouched" is then git's own guarantee, re-checked by our
  adversarial tests, rather than something we hand-roll against a mutable index API.

The backend stays behind a Skelly-owned boundary so it is swappable for libgit2
later if a hot path ever needs it (per the playbook's "backends behind traits").

## Consequences

- `skelly-session` parses git's textual output. We separate **invocation** (the
  thin `git` runner) from **parsing** (pure functions: porcelain-v2 status,
  numstat, unified diff), so the tricky parsing is fully unit-tested from sample
  strings with no git process, plus a small integration test that drives a real
  `git` against a throwaway temp repo.
- A subprocess per git query (status/diff). These run off the UI thread when wired,
  and are cheap relative to a human opening a dock; not a hot path.
- Untrusted-repo hygiene (playbook §7): we pass explicit `-C <root>` and never
  interpolate repo content into a shell (no shell; `Command` args are passed
  directly), and we treat all output as data. Paths with embedded newlines are a
  known parser edge (porcelain without `-z`); hardening to `-z` is a tracked
  follow-up.

## Alternatives considered

- **libgit2 via `git2`** - typed, in-process, no output parsing, and a real
  worktree API. But it adds a vendored C dependency (build + link surface), a
  `cargo deny` license review for the bundled libgit2, and a heavier graph, to
  replace parsing of interfaces that are already stable contracts. For the
  operations M4 needs, the CLI is simpler and leaner; libgit2's advantages (speed,
  transactional index ops) do not yet pay for their cost. Rejected for now, kept
  swappable.
- **A hybrid (CLI for diff, libgit2 for staging)** - two git models to keep
  consistent, the worst of both. Rejected.
