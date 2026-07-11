# skelly - project knowledge

A barebones, keyboard-driven terminal emulator built natively in Rust. Minimal by
design, for vim / neovim / LazyVim development: multi-pane splits, per-repo git
diff, and a rewindable session timeline. Targets macOS and Linux (Windows is out
of scope for v0.1).

**Status:** design-driven; M0 foundation landed. The Cargo workspace, quality
gates, CI, docs, and ADR log exist, and `skelly-config` is implemented (schema +
load + validate + tests) with a runnable binary that reports the resolved config.
Next is the M1 walking skeleton (window + PTY + cell renderer). The build target is
native Rust, not the mockup HTML. See `ROADMAP.md`.

**Stack:** Rust (pinned stable via `rust-toolchain.toml`, edition 2021), cargo
workspace. Foundation crates are *proposed* in ADR-0001..0004 (terminal core
`alacritty_terminal`, PTY `portable-pty`, renderer `wgpu` + `cosmic-text`,
windowing `winit`) - pending ratification, landed in M1. See `docs/adr/`.

**Layout** (Cargo workspace - scaffolded; keep crate boundaries aligned with the
design guide's modules)
- `Cargo.toml` - `[workspace]` root
- `crates/skelly/` - the binary: window, sidebar, pane tree, command palette, wiring
- `crates/skelly-render/` - GPU cell-grid renderer, fonts, theme token resolution
- `crates/skelly-term/` - PTY, shell I/O, ANSI parsing, scrollback
- `crates/skelly-session/` - session timeline, non-destructive rewind (shadow worktree), git diff
- `crates/skelly-config/` - `config.toml` load/watch/schema (the source of truth; see Hard rule 1)
- `design/` - the binding design spec (see Design below)

**Commands** (from repo root - the standard cargo toolchain)
- Build: `cargo build` (release: `cargo build --release`)
- Lint: `cargo clippy --all-targets --all-features -- -D warnings`
- Format: `cargo fmt` (rustfmt; also auto-runs on save via `.claude/hooks/format-on-edit.sh`)
- Test: `cargo test`
- Run: `cargo run -p skelly`
- Package / release: `TODO` - decide macOS `.app` bundling + Linux packaging when the app can build

## Design

The design guide in `design/` is the **source of truth**. Build against it; when a
decision isn't covered, decide it, then record it in `design/README.md`. As code
lands, graduate recurring conventions into Hard rules and design tokens into a
conformance check.

- Guide: `design/Skelly Design Guide.dc.html` - a static, exported mockup that
  specifies tokens, component anatomy/states, layout dimensions, keybindings,
  interaction flows, empty/first-run/error states, the `config.toml` schema, and
  v0.1 scope. Open in a browser to view.
- `design/README.md` - index + running decision log for the guide's "Confirm
  first" open questions.

## Hard rules

Only the non-obvious, binding facts an AI would otherwise get wrong. For a
greenfield repo most are enforced by review against the design guide today, and
should graduate to tests/lints as the code that makes them checkable appears.

### 1. `config.toml` is the single source of truth
`~/.config/skelly/config.toml` owns every setting; the UI is a *view* over it, and
every setting maps 1:1 to a config key. Don't store UI state that has no config
key, and don't let the settings view diverge from the file.

*Enforced:* review now; later, a round-trip test that every settings control reads
and writes exactly one config key.

### 2. UI reads semantic tokens, never raw hex
Every surface resolves a semantic token (`category.role.state`) per the active
theme; switching theme repaints everything live. UI tokens are separate from the
terminal ANSI palette - never cross them (a user pairs any ANSI scheme with any UI
theme).

*Enforced:* review + visual check in both Ossein Dark and Ossein Light; later, a
grep/lint that rejects raw hex in UI code.

### 3. Session-timeline rewind is non-destructive
Restoring to a past moment checks out a shadow worktree and never rewrites history
or moves HEAD. This is the feature's whole trust contract - treat any code path
that could mutate the user's real branch/HEAD during rewind as a bug.

*Enforced:* review + tests asserting HEAD and refs are untouched across a
rewind/fast-forward cycle.

### 4. Docks and overlays are layers, not routes
The terminal pane tree is the always-present base layer and never unmounts. Git
diff and the timeline open as a right dock (only one at a time); the command
palette is a centered overlay; settings is a full in-window view - all over a live
terminal, all dismissed with Esc. Opening any of them must not tear down the
terminal, and focus returns to the exact pane on close. Max 8 panes per tab.

*Enforced:* review + interaction tests.

### 5. Build in Rust from the spec - never ship the mockup
The design guide is a mockup describing intent. Do not reuse or scrape its
HTML/CSS/JS as app code, and don't infer behavior that isn't written down - if a
behavior is genuinely undecided, treat it as an open decision (Design, above)
rather than guessing.

*Enforced:* review.

### 6. Don't hand-edit the design guide, and record deferred decisions
`design/Skelly Design Guide.dc.html` (and `support.js`, `.thumbnail`) are a
generated Claude Design export - never hand-edit them; regenerate from the design
source. Product/architecture decisions the guide flags under "Confirm first" (and
the still-open crate choices for renderer / PTY / font shaping) live in
`design/README.md` until they're settled.

*Enforced:* review.
