# skelly - project knowledge

A barebones, keyboard-driven terminal emulator built natively in Rust. Minimal by
design, for vim / neovim / LazyVim development: multi-pane splits, per-repo git
diff, and a rewindable session timeline. Targets macOS and Linux (Windows is out
of scope for v0.1).

**Status:** design-driven; M0 foundation landed. The Cargo workspace, quality
gates, CI, docs, and ADR log exist, and `skelly-config` is implemented (schema +
load + validate + tests) with a runnable binary that reports the resolved config.
The **M1 walking skeleton is complete**: a real shell runs in a PTY (`portable-pty`),
its output is parsed by `alacritty_terminal` (`skelly-term`) and rendered live on the
GPU (`skelly-render`, `glyphon`/`cosmic-text`), with keystrokes forwarded. Verified
by an e2e shell test + a headless `session_capture` PNG. M2 (core terminal) is in progress: M2a (per-cell foreground colors via an
`AnsiPalette`, kept separate from UI tokens) and M2b (per-cell backgrounds + cursor
via an instanced colored-quad `wgpu` pipeline, two passes: quads then text) have
landed. M2c is partly done: the renderer now honors the configured font (Nerd Fonts) with a
monospace fallback, so Nerd glyphs render aligned. Still remaining in M2c: the real
fixed-metric cell renderer (own glyph atlas + instanced glyph quads) replacing
glyphon's reflowed text, for exact wide-char/fallback alignment. M2d is in progress: scrollback (10k-line history, mouse-wheel + Shift+PageUp/Down)
and selection + copy/paste (mouse-drag highlight, Cmd+C/V via `arboard`) have landed.
VT/ANSI conformance has landed too: `skelly-term` exposes a headless `Parser`
(`alacritty_terminal` grid + `Processor`, no PTY) that shares the live terminal's
exact parse path, covered by a deterministic conformance suite
(`tests/conformance.rs`) and a `proptest` byte-fuzz robustness guard
(`tests/robustness.rs`). The fuzzer found a reachable DoS - a single-column grid
wedges `alacritty_terminal`'s reflow for seconds - so the core now clamps every
grid to a 2-column floor (`MIN_COLS`). Remaining in M2d: coverage-guided
`cargo-fuzz` (nightly), reflow polish, live theme-token resolution. M2e (SGR text attributes) has landed: bold/italic via
cosmic-text weight+style, underline as a cell-width rule quad, reverse video + dim
resolved against the palette (`AnsiPalette::default_bg`); `skelly-term` exposes a
`CellAttrs` bitflags set. M2e also fixed a latent alignment bug - full-width grid
rows wrapped in the text buffer (doubling the line pitch and desyncing glyphs from
the bg/cursor/underline/selection quads); the buffer now uses `Wrap::None` so a grid
row is always one visual line. **M3 (Skelly shell UX) is essentially complete** (pane
tree, sidebar + tabs, command palette, settings view, live theming) and **M4 (signature
features) is COMPLETE**: the read-only git diff model (`skelly-session`, ADR-0006) and
the **git diff dock** landed. The dock is a right-edge layer (`⇧⌘G`, `Esc`; Hard rule 4)
over the live terminal - a status bar, the changed-file list, and the selected file's
unified diff (`diff.*` tokens, separate from the ANSI palette per Hard rule 2), driven by
the pure `gitdock` module over `skelly_session::Repo` (process cwd for now). Fixed at
420px (vertical stack; the wide side-by-side view + resizable width are follow-ups - see
`design/README.md`). The dock's git integration is now **complete**: `Repo::stage`/`unstage`/`stage_all`/
`commit`/`head_short`/`undo_commit`/`apply_hunk`, a `[x]`/`[ ]` checkbox per file row
(`Space` toggles the selected file, `a` stages all), a commit box at the dock foot (a
`Focus::{List,Commit}` model - `Tab` switches, typing edits the message, `Enter` commits
when >=1 file is staged, then a "committed <sha>" line offers `u` to undo), and hunk-level
staging (`[`/`]` move the focused hunk, `⌘↵` stages/unstages it via
`git apply --cached [--reverse]` of a reconstructed one-hunk patch). The **session timeline +
non-destructive rewind** (`⇧⌘H`, ADR-0007) completes M4: settling the three open design
decisions (in-session event log; read-only-inspection rewind; layout-only persist), it adds a
`skelly-session` model (`Timeline`/`SessionEvent`/`Actor` = an append-only, clock-free event
log; `Repo::shadow_checkout` -> `ShadowWorktree` = `git worktree add --detach` into a temp dir,
HEAD/refs untouched per Hard rule 3, backed by an adversarial trust-contract test) and a right
dock (the pure `timeline` module, mutually exclusive with the git dock) that lists the events
with a viewing banner + actor legend + session summary - `↑/↓` (or `⌥⌘←/→`) scrub, `⌥⌘0` returns
to now, and selecting a past **commit** rewinds to it read-only (staging events are recorded but
not restorable; the `Agent` actor's transport stays the open AI-actions contract). Event times
are session-relative (`M:SS`), avoiding a date dependency. Renderer chrome layers (sidebar / git
dock / timeline / palette / settings) now share a `ChromeLayer` (quads + text + active). **M5
(hardening & release) is in progress**: the first edge state - the **shell-exit / crash overlay**
(design §12) - has landed. When a pane's shell ends, `skelly-term` reaps the child + reports an
`ExitStatus` (and now kills the shell on drop so the reader thread reaps it, closing a latent
process/zombie leak); the renderer draws a translucent `bg.base` scrim (72%) over the pane's
preserved scrollback + a centered `shell exited` / exit-code-or-signal / `↵ restart   ⌥w close`
message (a 6th `ChromeLayer`, `set_pane_overlays`, above the terminal text but beneath the
docks/overlays). `↵` respawns the shell in place (drop the exited `Terminal`, `sync_layout`
respawns); a focused dead pane swallows other input. Pure `deadpane` module (unit-tested), an e2e
exit-detection test, and a `dead_pane_capture` PNG (both themes). Also landed: the **empty state +
never-quit close cascade** (design §10.2 + edge "Close last pane"): closing the only pane closes
the tab, and closing the only tab RESETS it to a fresh tab instead of quitting (`close_tab`; the
old tab drops so its shells are killed). A pristine single-pane tab (`Tab::activated` false) paints
a faint `skelly` wordmark + hint chips (`⌘K`/`⌘T`/`⌥|`, `bg.elevated` pills) centered over its
blank grid via the pure `emptystate` module (baked into the pane grid, no new render layer),
cleared on the first command (Enter) or split. Unit-tested + `empty_state_capture` PNG (both
themes). Also landed: the **sidebar collapse rail** (design §08 "Sidebar modes"). `⇧⌘B` cycles
the sidebar between the full panel and a slim 56px icon rail (compact centered tab numbers + a
`sk` brand mark; the active tab keeps its accent bar + `accent.subtle` fill); `⌘B` still
shows/hides. The mode round-trips `config.sidebar.mode` and persists per workspace (Hard rule 1 -
`⌘B`/`⇧⌘B` now write the config too, closing a latent gap where the sidebar's shown/hidden state
diverged from the file). The `sidebar` module now holds a `mode` (`Fixed`=full / `Autohide`=rail /
`Hidden`) + a `restore` target so hide/recall preserves the rail-vs-full choice; `Fixed` and the
rail share the same grid row layout, so `hit()`/`active_row` work unchanged for both. Surfaced in
the palette ("Cycle sidebar mode"); the `pane_capture` example takes a `rail` arg for the PNG.
Verified both themes + clean boot. (Follow-up: hover-to-expand the rail - the design's "hover to
expand" - which needs a transient-expand state and mouse-region tracking.) Also landed: the
**not-a-git-repo Init button** (design §12 "Not a git repo"). The git dock's empty state now shows
"No repository here" **+ an accent `Init repo ↩` button**; `Enter` in the no-repo state runs it.
`skelly_session::Repo::init` shells `git init` in the process cwd then rediscovers (integration-
tested); `init_repo` then calls `refresh_git` so the dock flips to the new empty repo. The button
reuses the file-list `selected_file_row` accent-highlight quad (the empty state has no file list)
and is keyboard-driven (dock mouse hit-testing is the same tracked follow-up as click-to-select-
file). Verified by the `git_dock_capture` `norepo` arg PNG in both themes + a `gitdock` unit test.
The build target is native Rust, not the mockup HTML. See `ROADMAP.md`.

**Stack:** Rust (pinned stable via `rust-toolchain.toml`, edition 2021), cargo
workspace. Foundation crates are *proposed* in ADR-0001..0004 (terminal core
`alacritty_terminal`, PTY `portable-pty`, renderer `wgpu` + `cosmic-text`,
windowing `winit`) - pending ratification, landed in M1. See `docs/adr/`.

**Layout** (Cargo workspace - scaffolded; keep crate boundaries aligned with the
design guide's modules)
- `Cargo.toml` - `[workspace]` root
- `crates/skelly/` - the binary: window, sidebar, pane widget/wiring, command palette
- `crates/skelly-render/` - GPU cell-grid renderer, fonts, theme token resolution
- `crates/skelly-term/` - PTY, shell I/O, ANSI parsing, scrollback
- `crates/skelly-pane/` - the pane-tree model (tiling splits/focus/resize/zoom, <=8);
  pure logic, no UI/GPU/PTY deps, a leaf like `skelly-config` (ADR-0005). The binary
  owns the wiring (pane -> terminal, rendering, keys).
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
