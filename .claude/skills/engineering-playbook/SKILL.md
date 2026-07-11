---
name: engineering-playbook
description: How we take Skelly from greenfield to production the way a serious engineering org does - the binding delivery method, sequencing, quality gates, and Rust practices for this repo. Read at the START of any build/feature/refactor session and follow it throughout. Use whenever deciding what to build next, opening a PR, scaffolding a crate, wiring CI, writing tests, or making an architecture call.
---

# Skelly engineering playbook

The operating manual for building Skelly to production standard. This is *how* we
work; `design/` is *what* we build and `AGENTS.md` holds the project's hard rules.
When those conflict with this file, they win - this playbook fills the gaps they
leave. Follow it in every session, including fresh contexts.

The point of a playbook on a greenfield repo is to counter the four failure modes
that sink new projects: **drift** (code diverging from the spec), **sprawl**
(unbounded scope, no vertical slice ever finished), **breakage** (no gate, so main
rots), and **confident-wrong** (claims not backed by a run). Every rule below
exists to kill one of those.

---

## 0. The prime directive: always shippable, always end-to-end

We build in **thin vertical slices**, never horizontal layers. At every commit,
`main` builds, passes its gates, and does *something* a user could observe. We do
not build "all of the renderer" then "all of the PTY" then wire them up months
later. We build a **walking skeleton** first (Cockburn): the thinnest possible
path that runs end-to-end, then thicken it slice by slice.

For Skelly the walking skeleton is: *window opens -> spawns the login shell in a
PTY -> shell output paints in a GPU cell grid -> keystrokes reach the shell ->
Esc/quit works.* Everything else (splits, sidebar, git dock, timeline, palette,
themes) is a slice added onto a skeleton that already runs. Config, being pure
logic and the source of truth (Hard rule 1), is the natural first testable slice
while the skeleton's harder pieces (GPU, PTY) are still being wired.

**Rule:** never leave `main` in a state that does not build and pass gates. If a
slice is too big to land green, land it behind a feature flag or split it smaller.

---

## 1. Delivery lifecycle - the phases

Ship in milestones. Each milestone is a set of vertical slices with a demoable
outcome. Do not start milestone N+1's polish before N runs end-to-end.

- **M0 - Foundation (day-1 setup).** Workspace, CI, quality gates, docs, ADR log.
  No feature work merges until this exists. See the day-1 checklist (§8).
- **M1 - Walking skeleton.** Window + PTY + cell renderer + input, single pane,
  one shell. It is ugly but it runs a real shell end-to-end.
- **M2 - Core terminal.** VT/ANSI correctness, scrollback, selection/copy-paste,
  resize/reflow, font shaping + Nerd Font fallback, theme token resolution.
- **M3 - Skelly shell UX.** Sidebar + tabs/groups/pinning, pane tree (split/
  focus/resize/zoom, <=8), command palette, settings view, live theming.
- **M4 - Signature features.** Per-repo git diff dock with hunk staging; session
  timeline with non-destructive rewind (shadow worktree).
- **M5 - Hardening & release.** Edge/empty/error states, perf budgets, packaging
  (macOS `.app`, Linux), first tagged release.

Record the current milestone and the next 1-3 slices somewhere visible (an issue,
a `ROADMAP.md`, or the PR description). Scope each slice so it is reviewable in one
sitting.

---

## 2. Version control & change flow

- **Trunk-based.** `main` is always releasable. Work on **short-lived** branches
  (`feat/…`, `fix/…`, `chore/…`, `docs/…`) that merge back in days, not weeks.
- **Small PRs.** One slice per PR. If a diff sprawls past a few hundred lines of
  real change, split it. Big PRs hide bugs and stall review.
- **Conventional Commits.** `type(scope): summary` (`feat`, `fix`, `refactor`,
  `perf`, `test`, `docs`, `chore`, `build`, `ci`). Scope is usually the crate
  (`render`, `term`, `session`, `config`, `skelly`). This repo has a commit-msg
  hook that enforces it - do not fight it. Never add an agent name as co-author.
- **SemVer** for any released artifact; pre-1.0 we stay in `0.x` and treat minor
  as the breaking axis. **Changelog is generated** from commits (keep-a-changelog
  shape) via `git-cliff`; releases via `cargo-release` + `cargo-dist` (signed,
  notarized macOS `.app`/`.dmg` + Linux artifacts) - never hand-edit
  `CHANGELOG.md` (AGENTS/base charter rule).
- **Tags** mark releases (`v0.1.0`). Never push a `v*` tag without an explicit
  request from the user.

---

## 3. Quality gates - non-negotiable, enforced not suggested

Every one of these runs in CI and blocks merge. Run them locally before pushing
(`/pre-pr` composes them). Green means *actually* green - never silence a gate to
pass it (no `#[allow]` to duck clippy, no `--no-verify`, no ignoring a flaky test).

1. **Format** - `cargo fmt --all --check`. Auto-runs on save via the repo hook.
2. **Lint** - `cargo clippy --all-targets --all-features -- -D warnings`. Warnings
   are errors. Fix the cause; do not blanket-allow.
3. **Test** - `cargo test --all` green, deterministic, no ignored-without-reason.
4. **Docs** - `cargo doc --no-deps` builds clean; doctests pass.
5. **Supply chain** - `cargo deny check` (licenses, advisories, bans, sources).
6. **Build** - release build succeeds on the target matrix (macOS + Linux).

A gate that cannot yet run (e.g. a GPU test needs a display) is documented as
such, not faked green. See `references/rust-quality-gates.md` for the concrete
configs (`deny.toml`, `rust-toolchain.toml`, `rustfmt.toml`, clippy lint policy,
the CI workflow).

---

## 4. Testing strategy

- **Test pyramid.** Many fast unit tests (pure logic: config parse/validate, pane
  tree math, ANSI state machine, token resolution, timeline model). Fewer
  integration tests across crate boundaries. A thin top of end-to-end/behavioral
  checks (spawn a PTY, feed bytes, assert grid state).
- **Reproduce before you fix.** Every bug starts with a failing test that fails
  *for the reported reason*; then fix until green; keep the test (base charter).
- **Determinism.** No wall-clock, no network, no ordering assumptions, no shared
  mutable global. Flaky == broken; quarantine + fix, never retry-to-green.
- **Right tool per shape:** property-based tests (`proptest`) for parsers and the
  pane-tree invariants; **golden/snapshot** tests (`insta`) for rendered grid
  output and theme token tables; `criterion` benches for the hot paths (parser,
  renderer) with a tracked budget.
- **Coverage is a smell detector, not a target.** Cover logic and invariants;
  don't chase coverage on glue, generated code, or GPU/platform shims that only a
  human-in-the-loop can meaningfully exercise.
- **Skelly's trust-contract tests are mandatory** (from Hard rules): timeline
  rewind never moves HEAD/refs; every settings control round-trips exactly one
  config key; no raw hex in UI code; docks/overlays never unmount the terminal.

---

## 5. Architecture discipline

- **The spec is the source of truth.** `design/` binds. When it is silent, decide,
  then record the decision in `design/README.md` (its "Open decisions" + decision
  log). Do not guess undecided *product* behavior - surface it (Hard rule 5/6).
- **ADRs for anything hard to reverse.** Crate/foundation choices (GPU renderer,
  PTY layer, font shaping), cross-crate contracts, data formats, threading model,
  the timeline/rewind mechanism. One ADR per decision in `docs/adr/NNNN-title.md`
  using the Nygard format in `references/adr-template.md`: Context / Decision /
  Consequences / Alternatives, status `Proposed|Accepted|Superseded`. An ADR is
  cheap and permanent; a silent decision is expensive and invisible.
- **Crate boundaries are contracts.** Keep them aligned with the design modules
  (`skelly`, `skelly-render`, `skelly-term`, `skelly-session`, `skelly-config`).
  Dependencies flow one way: the binary depends on the libraries; libraries do not
  depend on the binary and avoid depending on each other except through small,
  intentional interfaces. `skelly-config` is a leaf everyone can read.
- **Error handling boundary.** Libraries return typed errors (`thiserror`); the
  binary is where errors get contextualized/reported (`anyhow` at the edges).
- **Tech debt is explicit.** A shortcut gets a `// TODO(owner): …` or an issue,
  never a silent landmine. No broken windows.

---

## 6. Rust practices (enterprise-grade)

Full configs live in `references/rust-quality-gates.md`. The essentials:

- **Cargo workspace**, one `Cargo.toml` root with `[workspace]` + shared
  `[workspace.dependencies]` and `[workspace.lints]` so versions and lint policy
  are set once. Crates under `crates/`.
- **Pinned toolchain** via `rust-toolchain.toml` (stable channel, explicit
  version) so every machine + CI build with the same compiler. State an **MSRV**
  and check it in CI.
- **`unsafe` is reviewed, justified, and localized.** Every `unsafe` block carries
  a `// SAFETY:` comment. Forbid it by default at the workspace level and opt in
  per-crate only where FFI/GPU genuinely needs it.
- **Supply chain:** commit `Cargo.lock`; `cargo deny` for licenses/advisories/
  bans; keep the dep graph lean (a minimal terminal earns its minimalism).
- **Docs:** crate-level `//!` docs, public items documented, examples as doctests.
- **Follow the Rust API Guidelines** for public interfaces (naming, error types,
  `Debug`/`Display`, must-use).

---

## 7. Observability, privacy, security & operability

- **Structured logging from day one** via `tracing` with env-controlled levels;
  no `println!` debugging left in. Spans around PTY I/O, render frames, git ops.
- **Privacy is a feature, not an afterthought.** Skelly is a local dev tool that
  sees the user's shell, code, and keystrokes. **No telemetry, no phone-home, no
  analytics** without explicit, off-by-default, documented opt-in. Never log
  command contents or file contents at default levels. This is a trust contract on
  par with non-destructive rewind - treat a silent network call as a bug.
- **Fail loud in dev, gracefully in prod.** A panicking pane must not take down
  the window; surface the shell-crash overlay (design edge state) instead. Install
  a panic hook that logs the backtrace to the log file for bug reports.
- **Threat model (a terminal has a real one).** Untrusted bytes flow straight into
  the ANSI parser - **fuzz it** (`cargo-fuzz`) and honor only safe control
  sequences (never blind clipboard-write or command-execution escapes; gate
  anything sensitive on the user). Config/theme files and the opened repo may be
  hostile - parse defensively, never execute config. The rewind touches the user's
  git repo: treat any path that could mutate real HEAD/refs as a security-grade
  bug and test it adversarially against a throwaway repo. Signing secrets live only
  as CI secrets, never in the repo or logs.

---

## 8. Day-1 setup checklist (M0 - do before feature work)

- [ ] `cargo init` the workspace; root `Cargo.toml` `[workspace]` + resolver 2 +
      shared deps/lints; stub the five crates with `//!` docs and a smoke test.
- [ ] `rust-toolchain.toml`, `rustfmt.toml`, `deny.toml`, `.editorconfig`,
      `.gitignore` (Rust) committed.
- [ ] CI workflow: fmt + clippy(-D warnings) + test + doc + deny, on a macOS +
      Linux matrix, with caching; required status checks on `main`.
- [ ] `CONTRIBUTING.md` (branch/commit/PR/gate rules), `ARCHITECTURE.md` (crate
      map + dependency direction), `docs/adr/` with ADR-0000 (record-decisions),
      `CHANGELOG.md` (generated), `LICENSE`.
- [ ] `Cargo.lock` committed; `cargo build`, `cargo test`, `cargo clippy`,
      `cargo fmt --check`, `cargo deny check` all green locally and in CI.
- [ ] First vertical slice picked and scoped (config load/validate is the natural
      first testable one).

---

## 9. Definition of done (per slice)

A slice is done only when: it builds and all gates are green; it has tests that
would fail without it (reproduce-before-fix for bugs); it reads semantic tokens
(no raw hex) and passes visual review in Ossein Dark *and* Light where UI is
touched; every new action has a remappable binding surfaced in the palette; the
relevant design edge/empty/error states are handled; any decision made was
recorded (ADR or `design/README.md`); docs/changelog reflect it; and it has been
*run*, not just compiled - proof, not assertion (base charter). The design guide's
own "Definition of done" list (§13) is the acceptance bar for UI slices.

---

## 10. How to actually work a session

1. Re-read the relevant `design/` section for the slice; check `design/README.md`
   open decisions.
2. State the slice and its done-criteria. Keep it thin.
3. Reproduce-first if it's a bug; write the test.
4. Implement to match surrounding code and the spec's tokens/dimensions exactly.
5. Run the gates locally (`/pre-pr`). Fix red at the root.
6. Record any decision (ADR / `design/README.md`); update docs/changelog inputs.
7. Verify by running the app/flow, not just tests, for anything user-facing.
8. Small, conventional-commit PR. Hand back for the user to ship.

> When in doubt, prefer quality, simplicity, robustness, and long-term
> maintainability over speed - and prefer doing the work over deferring it.
