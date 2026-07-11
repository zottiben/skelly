---
name: handoff
description: Prepare a clean context handoff before the window is cleared, so a fresh Skelly session resumes exactly where we left off. Commits/pushes in-progress work, certifies the repo is green, refreshes the build-state memory's RESUME HERE block, and confirms how to resume. Use whenever the context is getting full, or you're about to /clear, checkpoint, wrap up a session, or the user says "clear context" / "hand off" / "pick this up later".
---

# Skelly handoff - checkpoint before clearing context

Make it safe to clear the context window mid-build and pick up seamlessly in a fresh
session. Two things must be true when you finish: the **repo** is the durable source of
truth (committed + pushed + green), and the **build-state memory** tells the next
context where we are and what to do first. The fresh context only sees what is in git
plus that memory - anything else is lost.

Run the steps in order.

## 1. Commit and push in-progress work
Check `git status --short`. Uncommitted changes vanish on handoff. If there are any:
- Bring them to a coherent state. Never leave `main` broken; if on the default branch,
  branch first.
- Commit with a Conventional Commit (the commit-msg hook enforces it; never add an
  agent co-author) and `git push`. If genuinely mid-slice, commit a clearly-labelled
  WIP and call that out in the memory (step 3).
Do NOT open a PR or push a `v*` tag unless the user asked.

## 2. Certify the green baseline
Give the next context a known-good point to trust. Run this repo's real gates:
`cargo fmt --all --check` -> `cargo clippy --all-targets --all-features -- -D warnings`
-> `cargo test --workspace` -> `cargo deny check`. Record the short HEAD sha and each
PASS/FAIL. If anything is red, fix it - or record precisely what is red and why. Never
certify green over a failure.

## 3. Refresh the build-state memory
Update `skelly-build-state.md` (the project memory that auto-loads into every session)
so it matches reality:
- Update the **RESUME HERE** block at the top: current branch + HEAD sha, clean/pushed
  status, whether a PR exists, and the **next 1-3 concrete work items** pulled from
  `ROADMAP.md`.
- Update the per-milestone status for anything finished or started this session.
- Add any new **gotchas** learned - API quirks, verification tricks, dep/license notes,
  environment facts - the things the code alone does not reveal. These are the highest
  value part.
- Keep it lean. It is a living "you are here + how to work + gotchas" doc, not a
  changelog. The repo (`CHANGELOG.md`, `ROADMAP.md`, `docs/adr/`, git log) holds the
  full history; trim stale detail from the memory rather than appending forever.
- Ensure `MEMORY.md` still lists it.

## 4. Confirm, then it is safe to clear
Tell the user briefly: the HEAD sha, that it is pushed and green, that RESUME HERE is
current, and how to resume - start a fresh session and say "continue the skelly build";
the memory auto-loads and drives the rest.

## What resume looks like (for the next context)
The RESUME HERE block loads automatically and is the script: `git pull`, read the
`engineering-playbook` skill + `ROADMAP.md` + `AGENTS.md` + this memory, re-run the
gates to confirm the certified-green baseline still holds, then continue the next
slice in small, verified, committed increments.
