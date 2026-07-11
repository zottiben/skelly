# Contributing to skelly

How we work is captured in the **engineering playbook**
(`.claude/skills/engineering-playbook/`); this is the human-facing summary. The
binding product spec is [`design/`](design/); the non-negotiable project rules are
in [`AGENTS.md`](AGENTS.md).

## The loop

1. Pick a thin **vertical slice** (see [`ROADMAP.md`](ROADMAP.md)). Every change
   keeps `main` building, green, and runnable. We build end-to-end, not
   layer-by-layer.
2. Branch off `main`: `feat/…`, `fix/…`, `chore/…`, `docs/…`. Keep branches
   short-lived and PRs small (one slice; aim for a few hundred lines of real
   change, split if larger).
3. For a bug, **reproduce first**: write a test that fails for the reported reason,
   then fix until green, and keep the test.
4. Match the surrounding code and the spec's tokens/dimensions exactly. UI reads
   semantic tokens - never raw hex.
5. Run the gates locally before pushing (below).
6. Record any architecture decision as an [ADR](docs/adr/); record undecided
   product calls in [`design/README.md`](design/README.md).
7. Open a PR with a [Conventional Commit](https://www.conventionalcommits.org)
   title. Hand back for the maintainer to ship - don't push tags.

## Commits

`type(scope): summary`, where `type` is one of
`feat|fix|docs|refactor|perf|test|build|ci|chore` and `scope` is usually the crate
(`render`, `term`, `session`, `config`, `skelly`). PRs are squash-merged, so the PR
title becomes the commit. The commit-msg hook enforces the format. Never add an
agent as co-author. `CHANGELOG.md` is generated from commits - never hand-edit it.

## Quality gates (must be green - CI enforces the same)

```sh
cargo fmt --all --check
cargo clippy --all-targets --all-features -- -D warnings
cargo test --workspace --all-features
RUSTDOCFLAGS="-D warnings" cargo doc --no-deps --workspace
cargo deny check
```

`/pre-pr` runs these on the current diff, plus native code review, before you push.
Warnings are errors: fix the root cause, never silence a gate (`#[allow]` to duck
clippy, `--no-verify`, ignoring a flaky test). A flaky test is a broken test.

## Definition of done

A slice is done only when: it builds and every gate is green; it has tests that
would fail without it; UI touches read semantic tokens and pass review in Ossein
Dark *and* Light; every new action has a remappable binding surfaced in the
palette; the relevant empty/first-run/error states are handled; any decision made
was recorded; and it has been **run**, not just compiled.

## Setup

`rustup` installs the pinned toolchain from `rust-toolchain.toml` automatically on
first `cargo` invocation. Formatting auto-runs on save via the repo hook. Install
`cargo-deny` (`cargo install cargo-deny --locked`) to run the supply-chain gate
locally.
