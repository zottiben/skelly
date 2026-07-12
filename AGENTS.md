# skelly - project knowledge

A barebones, keyboard-driven terminal emulator built natively in Rust. Minimal by
design, for vim / neovim / LazyVim development: multi-pane splits, per-repo git
diff, and a rewindable session timeline. Targets macOS and Linux (Windows is out
of scope for v0.1).

**Status:** design-driven, delivered in milestones. M0-M4 are complete (foundation,
walking skeleton, core terminal, the Skelly shell UX, and the signature features -
the git diff dock and the session timeline + non-destructive rewind); **M5 (hardening
& release) is in progress.** `ROADMAP.md` is the milestone tracker (what's done /
next), `docs/adr/` records the architecture decisions, and the `skelly-build-state`
memory holds the live working narrative. Don't restate milestone status here - keep
this file to durable facts and the Hard rules.

**Stack:** Rust (pinned stable via `rust-toolchain.toml`, edition 2021), cargo
workspace. Foundation backends sit behind Skelly-owned traits so they stay swappable
(ADR-0001..0004, Accepted): terminal core `alacritty_terminal`, PTY `portable-pty`,
renderer `wgpu` + `cosmic-text`/`glyphon`, windowing `winit` (single-window). Keep
the direct `wgpu` dep aligned to `glyphon`'s pin.

**Layout** (Cargo workspace - keep crate boundaries aligned with the design guide's modules)
- `crates/skelly/` - the binary: window, sidebar, pane widget/wiring, command palette
- `crates/skelly-render/` - GPU cell-grid renderer, fonts, theme token resolution
- `crates/skelly-term/` - PTY, shell I/O, ANSI parsing, scrollback
- `crates/skelly-pane/` - pane-tree model (tiling splits/focus/resize/zoom, <=8); pure
  logic, no UI/GPU/PTY deps - a leaf like `skelly-config` (ADR-0005). The binary owns
  the wiring (pane -> terminal, rendering, keys).
- `crates/skelly-session/` - session timeline, non-destructive rewind (shadow worktree), git diff
- `crates/skelly-config/` - `config.toml` load/watch/schema (the source of truth; Hard rule 1)

**Commands** (from repo root - standard cargo)
- Build: `cargo build` (release: `cargo build --release`); run: `cargo run -p skelly`
- Lint gate: `cargo clippy --all-targets --all-features -- -D warnings`
- Format: `cargo fmt` (also auto-runs on save via `.claude/hooks/format-on-edit.sh`)
- Test: `cargo test`; package/release: `TODO` (decide macOS `.app` + Linux packaging)

## Design

The design guide in `design/` is the **source of truth** - build against it, never
ship the mockup (Hard rule 5). `design/Skelly Design Guide.dc.html` (a generated,
static export - open in a browser) specifies tokens, component states, layout
dimensions, keybindings, flows, empty/error states, the `config.toml` schema, and
v0.1 scope. `design/README.md` is the index + running decision log for the guide's
"Confirm first" open questions - read it before making a product call, and record
new decisions there.

## Hard rules

Only the non-obvious binding facts an AI would otherwise get wrong. Most are enforced
by review against the design guide today; they graduate to tests/lints as the code
that makes them checkable lands.

### 1. `config.toml` is the single source of truth
`~/.config/skelly/config.toml` owns every setting; the UI is a *view* over it, every
setting mapping 1:1 to a config key. Don't store UI state with no config key, and
don't let the settings view diverge from the file.
*Enforced:* a settings round-trip test - every control reads and writes exactly one config key.

### 2. UI reads semantic tokens, never raw hex
Every surface resolves a semantic token (`category.role.state`) per the active theme;
switching theme repaints everything live. UI tokens are separate from the terminal
ANSI palette - never cross them (any ANSI scheme pairs with any UI theme).
*Enforced:* review + a visual check in both Ossein Dark and Light; later, a lint rejecting raw hex in UI code.

### 3. Session-timeline rewind is non-destructive
Restoring to a past moment checks out a shadow worktree and never rewrites history or
moves HEAD - this is the feature's whole trust contract. Treat any path that could
mutate the user's real branch/HEAD during rewind as a bug.
*Enforced:* tests asserting HEAD and refs are untouched across a rewind/fast-forward cycle.

### 4. Docks and overlays are layers, not routes
The terminal pane tree is the always-present base layer and never unmounts. Git diff
and the timeline open as a right dock (only one at a time); the command palette is a
centered overlay; settings is a full in-window view - all over a live terminal, all
dismissed with Esc. Opening any must not tear down the terminal, and focus returns to
the exact pane on close. Max 8 panes per tab.
*Enforced:* review + interaction tests.

### 5. Build in Rust from the spec - never ship the mockup
The design guide is a mockup describing intent. Don't reuse or scrape its HTML/CSS/JS
as app code, and don't infer behavior that isn't written down - if a behavior is
genuinely undecided, treat it as an open decision in `design/README.md`, not a guess.
*Enforced:* review.

### 6. Don't hand-edit the design guide, and record deferred decisions
`design/Skelly Design Guide.dc.html` (and `support.js`, `.thumbnail`) are a generated
Claude Design export - never hand-edit them; regenerate from the design source.
Product/architecture decisions the guide flags under "Confirm first" (and the still-open
crate choices) live in `design/README.md` until they're settled.
*Enforced:* review.
