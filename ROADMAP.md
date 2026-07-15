# Roadmap

Delivery is sequenced in milestones, each a set of thin vertical slices with a
demoable outcome. We never start a later milestone's polish before the current one
runs end-to-end. Rationale and detail are in the engineering playbook.

## Current: M5 (hardening & release)

M0, M1, the core of M2 (core terminal), and all the big M3 slices (sidebar + tabs,
pane tree, command palette, settings view, live theming) are in place and run
end-to-end. M2 has a handful of tracked carry-overs - the unchecked boxes below
(bespoke cell renderer, cargo-fuzz, reflow/theme polish, SGR fidelity follow-ups) -
plus a few M3 follow-ups, which we finish opportunistically rather than block on.
**M4 (signature features) is complete**: the **per-repo git diff dock** (`⇧⌘G`) - the
diff model, the dock UI, per-file + hunk-level staging, and the commit box - and the
**session timeline + non-destructive rewind** (`⇧⌘H`, ADR-0007) both land end-to-end.
**M5 (hardening & release) is now in progress**: six edge states have landed - the
**shell-exit / crash overlay** (design §12; a dim scrim + exit message + `↵ restart` over a
pane whose shell ended), the **empty state + never-quit close cascade** (design §10.2;
closing the last pane closes the tab, closing the last tab resets to a fresh empty-state
tab), the **sidebar collapse rail** (design §08; `⇧⌘B` cycles the full panel <-> a slim
56px icon rail, the mode persisting to `config.sidebar.mode`), the **not-a-git-repo
Init button** (design §12; the git dock's empty state offers `Init repo`, running
`git init`), the **process-running-on-close confirm** (design §12; closing a pane/tab
with a running foreground job asks first, naming the process), and the **tab overflow
scroll** (design §12; the tab list windows into the available height, keeping the active
tab in view and marking hidden tabs with `↑`/`↓` counts). Remaining M5: the other edge
states, perf budgets, packaging, and the first tagged release - plus the tracked M2-M4
follow-ups, finished opportunistically.

- [x] **M0 - Foundation.** Workspace, quality gates, CI, docs, ADR log, and the
  `skelly-config` slice (schema + load + validate + tests), with a runnable binary
  that reports the resolved config. All gates green on macOS + Linux.
- [x] **M1 - Walking skeleton** (done). Window opens -> spawns the login shell in a
  PTY -> shell output paints on the GPU -> keystrokes reach the shell -> clean quit.
  Single pane, one shell, ugly but real. The hardest integration risks (GPU surface,
  PTY plumbing, event loop, glyph upload) are retired.
  - [x] **M1a** - native window (`winit`) + `wgpu` surface clearing to the theme
    background; clean quit. GPU-surface + event-loop risk retired.
  - [x] **M1b** - render shaped text via `glyphon`/`cosmic-text` in the cell font +
    `fg.primary` token; reusable `TextLayer` + headless PNG capture. Text-rendering
    risk retired.
  - [x] **M1c** - PTY (`portable-pty`) + terminal core (`alacritty_terminal`): pipe
    shell output through the parser into the grid, render it live, forward
    keystrokes. Proven end-to-end with an e2e shell test + a session-capture PNG.
- [~] **M2 - Core terminal** (in progress). VT/ANSI correctness (vttest / esctest in
  CI), scrollback, selection + copy/paste, resize/reflow, font shaping + Nerd Font
  fallback, live theme-token resolution.
  - [x] **M2a** - per-cell foreground colors (ANSI 16 + 256-color resolution, kept
    separate from UI tokens) and monospace alignment. `skelly-term` exposes a colored
    cell snapshot; `skelly-render` gains an `AnsiPalette` + colored `set_cells`.
  - [x] **M2b** - per-cell background colors + a cursor block, via an instanced
    colored-quad `wgpu` pipeline aligned to the measured monospace cell grid (two
    passes: quads then text). `skelly-term` exposes per-cell `bg` + cursor position.
  - [~] **M2c** - honor the configured font + exact per-cell placement.
    - [x] Honor the configured font (Nerd Fonts) with a monospace fallback; the
      primary neovim / monospace-Nerd-Font case renders correctly and aligned, with
      Nerd glyphs. `skelly-render` uses the configured family when installed.
    - [ ] The real fixed-metric cell renderer (own glyph atlas + instanced glyph
      quads) replacing glyphon's reflowed text, so cells align exactly for wide
      chars / fallback glyphs regardless of natural advance.
  - [~] **M2d** - terminal capabilities.
    - [x] Scrollback: 10k-line history with mouse-wheel + Shift+PageUp/Down
      scrolling and scroll-to-bottom on input; renders the scrolled view via the
      display offset. (Engine support from `alacritty_terminal`.)
    - [x] Selection + copy/paste: mouse-drag selection with a translucent highlight,
      Cmd/Super+C copies the selection, Cmd/Super+V pastes into the shell (`arboard`).
    - [~] VT/ANSI conformance + parser fuzzing.
      - [x] Headless `Parser` (`alacritty_terminal` grid + `Processor`, no PTY)
        sharing the live terminal's exact parse path, plus a deterministic
        conformance suite (SGR named/256/truecolor + bg, bold/italic/underline/
        inverse/dim + reset, CUP/relative moves/CHA, EL/ED, wrap, tab, scrollback)
        and a `proptest` robustness guard that fuzzes arbitrary bytes in CI. The
        fuzzer caught a reachable reflow DoS (a single-column grid wedged
        `alacritty_terminal`'s reflow for seconds); the core now clamps to a
        2-column floor.
      - [ ] Coverage-guided `cargo-fuzz` target (nightly) for deeper fuzzing.
    - [ ] Resize/reflow polish, live theme-token resolution.
  - [~] **M2e** - SGR text attributes: bold / italic (cosmic-text weight+style),
    underline (a cell-width rule quad), reverse video and dim (resolved against the
    palette: swap fg/bg using a new `default_bg`, reduce fg intensity). `skelly-term`
    exposes a `CellAttrs` bitflags set read from the engine cell flags. Also fixed a
    latent cell-grid alignment bug: full-width rows wrapped in the text buffer,
    doubling the line pitch and desyncing glyphs from the background / cursor /
    underline / selection quads - the buffer now uses `Wrap::None` so each grid row is
    exactly one visual line. Core done; SGR fidelity follow-ups remain:
    - [ ] `bold_is_bright` config key - bold is weight-only today; when set, bold
      should also brighten ANSI 0-7 foregrounds to their 8-15 bright variants.
    - [ ] Distinguish underline styles (double / curly / dotted / dashed); all
      collapse to a single underline in `skelly-term`'s `map_attrs` today.
    - [ ] Resolve the dim-named ANSI colors and the explicit default-bg / cursor
      named colors in `map_color` instead of falling back to the default foreground.
- [~] **M3 - Skelly shell UX.** Sidebar + tabs/groups/pinning; pane tree
  (split/focus/resize/zoom, <=8); command palette; settings view; live theming.
  - [~] Sidebar + tabs.
    - [x] The **tab model** - `App` holds `tabs: Vec<Tab>` + `active: usize`; a `Tab`
      bundles an independent pane tree, one live shell per pane, the grid-size cache,
      and its own selection. Tabs are fully isolated; switching swaps the whole
      terminal workspace and background tabs keep running. Driven by the keyboard and
      command palette: `⌘T` new tab, `⌘W` close tab (keeps the last one), `⌘1..9`
      go-to, `⌥⇧[` / `⌥⇧]` cycle prev/next. Unit-tested (chord decode + cycle/close
      index math); rendering regression-checked (the active tab renders exactly as a
      single workspace did) and the binary boots clean. Deferred to the sidebar-chrome
      slice: closing the last tab -> empty state, and the close-confirm on a running
      process.
    - [x] The left-dock **sidebar chrome** - a fixed-width left panel (config
      `[sidebar] width`) listing the open tabs (active = `accent` bar + `accent.subtle`
      fill, per the guide), a brand header, and a "+ New tab" action, with a `border`
      divider on its right edge. The pane viewport insets by its width; `⌘B` shows/hides
      it (re-fitting the shells); clicking a tab switches, clicking "+ New tab" opens
      one. Renders as base chrome (a dedicated quad+text load-pass pair beneath the
      palette overlay), shared by the headless `pane_capture` PNG - verified in Ossein
      Dark + Light. Pure `sidebar` module (view + hit-test), unit-tested. Deferred: the
      `⇧⌘B` slim rail, the pinned grid + `⇧⌘P`, collapsible groups, drag-reorder, the
      footer action icons, cwd/command tab titles (tabs are numbered), and the
      last-tab-close empty state.
  - [x] Pane tree (split/focus/resize/zoom, <=8).
    - [x] The pane-tree **model** - a leaf crate `skelly-pane` (ADR-0005): nested
      uneven binary splits, directional focus, keyboard resize, zoom, even-out, and
      exact viewport tiling, capped at 8 panes. Pure logic, unit + `proptest`
      tiling-invariant tested; no UI/GPU/PTY deps.
    - [x] Wired into the window: a live terminal per pane (`HashMap<PaneId,
      Terminal>` reconciled by `sync_layout`: spawn / resize / prune), the renderer
      drawing every pane via `Renderer::set_panes` at its rect with a `border`
      divider + the focused `accent` ring and a cursor only in the focused pane,
      input routed to the focused pane (pointer->pane->cell for clicks/selection/
      wheel), and the `⌥` pane keybindings (split `⌥|`/`⌥-`, focus `⌥h/j/k/l` +
      `⌥1..8`, resize `⌥⇧h/j/k/l`, zoom `⌥Z`, close `⌥w`, even-out `⌥=`). Verified
      by the `pane_capture` headless PNG (real 2-pane split, two live shells) + unit
      tests for the geometry and chord decode. (The focused-pane ring is
      `border.strong`, corrected from accent.) Follow-ups: draggable dividers.
  - [~] Command palette.
    - [x] A centered overlay (Hard rule 4) over the live terminal, opened with `⌘K`
      / closed with `Esc`, that filters a built-in command set by typed query,
      navigates with up/down, and runs the selection with Enter. The renderer gained
      an overlay pass (`Renderer::set_overlay`, drawn on top with `LoadOp::Load`) and
      the UI tokens `bg.elevated` / `fg.muted` / `fg.secondary` / `border.strong`.
      Palette state + view-building is a pure `palette` module (unit tested);
      verified rendering by the `pane_capture` headless PNG (overlay over two panes).
    - [x] Fuzzy subsequence matching (ranked: earliest first-match + fewest gaps win),
      with the matched characters drawn in `accent`.
    - [ ] Merge user `[keys]` overrides + the configurable `panes.leader` (tmux-style
      `ctrl+a`); surface tabs / themes / files and the `/` `?` mode prefixes; `⌘↵`
      run-in-new-pane.
  - [~] Settings view.
    - [x] A full in-window view over `config.toml` (Hard rule 4), opened with `⌘,` /
      closed with `Esc`, drawn on top of the live terminal. A left category nav
      (Appearance, Sidebar, Tabs, Panes, Session, Git) and a right control list; `↑/↓`
      move between controls, `←/→` (or Enter) change the focused value, `Tab` switches
      category. Every control round-trips **exactly one** `config.toml` key (Hard rule
      1) - enforced by a test that diffs the serialized config and asserts a single
      leaf, the control's declared key, changed. Changes persist immediately
      (`Config::save_default`, atomic write) and apply live where cheap (theme +
      sidebar mode/width); font / cursor / opacity persist and take effect next launch.
      The renderer gained a settings pass (`Renderer::set_settings`, nav/content fills +
      the active-category and focused-control highlights + divider). Pure `settings`
      module (control model + view, unit tested); verified by the `settings_capture`
      headless PNG in Ossein Dark + Light. Surfaced in the palette ("Open settings").
    - [ ] Live font re-shaping (font size / family / line-height applied without a
      relaunch); the mockup's theme cards + real slider/toggle widgets and mouse
      hit-testing; the Keybindings / Shell & env / Advanced categories (they need the
      `[keys]` registry or config keys we do not have yet); Nerd-Font category icons
      (ASCII markers today, pending the fixed-metric cell renderer).
  - [x] Live theming. `Renderer::set_theme` re-resolves the UI tokens and the binary
    re-resolves the ANSI palette + updates `config.appearance.theme` (Hard rule 1),
    then the next frame repaints every surface in the new theme (Hard rule 2). Driven
    by the palette's "Theme: Ossein Dark / Light" commands; verified by rendering the
    workspace in both themes. (Follow-up: a separate ANSI-scheme vs UI-theme config
    key so the two are independently selectable per Hard rule 2; today one name drives
    both, and there is no theme-file watch yet.)
- [~] **M4 - Signature features.** Per-repo git diff dock with hunk staging;
  session timeline with non-destructive rewind (shadow worktree).
  - [x] Per-repo git diff dock.
    - [x] The read-only git diff **model** in `skelly-session` (ADR-0006: shell out
      to the `git` CLI behind a Skelly-owned type, not libgit2). `Repo::discover`
      finds the working tree; `Repo::status` reports the branch, ahead/behind, and
      the changed files (staged / unstaged / untracked, with status kind + line
      counts); `Repo::diff` returns a per-file unified diff parsed into hunks and
      line-numbered add/del/context lines. Invocation is split from parsing so the
      porcelain-v2 / numstat / unified-diff parsers are unit-tested from sample
      strings (10 tests), plus an integration test driving a real `git` against a
      throwaway repo (2 tests).
    - [x] The right-dock render surface + `⇧⌘G` wiring (a layer, Hard rule 4; fixed at
      the guide's 420px default - the resizable 360-560 range is a follow-up), showing
      the status bar (branch, ahead/behind, totals), the changed-file list, and the
      selected file's unified diff with the `diff.add` / `diff.del` / `diff.hunk`
      tokens. Opened with `⇧⌘G` (and the palette "Show git diff"), dismissed with
      `Esc`; `↑/↓` move between files (re-diffing), `PageUp/PageDown` scroll the diff.
      Base chrome on the right edge (its own quad+text load pass, like the sidebar);
      the pane viewport insets to its left. The dock's data comes from
      `skelly_session::Repo` on the process cwd (real per-pane cwd is a follow-up, the
      same blocker as cwd tab titles); the pure `gitdock` module (state + view, unit
      tested) turns a `Status` + `FileDiff` into the grid + row metadata; verified by
      the `git_dock_capture` headless PNG in both themes. A `bespoke-cell`-safe
      vertical stack replaces the guide's wide side-by-side layout at 420px (design
      decision, recorded in `design/README.md`). Deferred: the resizable width, the
      wide split view, click-to-select / mouse, and moving git calls off the UI thread.
    - [x] Staging + commit box.
      - [x] Per-file staging: `skelly_session::Repo` gained `stage` / `unstage` /
        `stage_all` (`git add` / `git reset`, integration-tested on a temp repo); the
        dock shows a per-file `[x]`/`[ ]` checkbox, `Space` toggles the selected file
        (stage <-> unstage), `a` stages everything, and the status + diff reload after.
      - [x] The commit box: `Repo` gained `commit` / `head_short` / `undo_commit`
        (`git commit -m` / `rev-parse --short HEAD` / `reset --soft HEAD^`,
        integration-tested). A message input band at the dock foot (a `Focus::{List,
        Commit}` model - `Tab` moves focus, typing edits the message, `Enter` commits
        when >=1 file is staged and the message is non-blank, `Esc` returns to the list),
        with an accent caret and a `N staged` status line; after a commit a "committed
        <sha>" line offers Undo (`u`, a soft reset).
      - [x] Hunk-level staging: `Repo::apply_hunk` reconstructs a one-hunk patch
        (`diff::hunk_patch`) and pipes it to `git apply --cached [--reverse]`
        (integration-tested staging one hunk of a two-hunk file). In the dock, `[` / `]`
        move the focused hunk (highlighted, with a `stage`/`unstage ⌘↵` affordance) and
        `⌘↵` stages it (or unstages when viewing the staged diff).
  - [x] Session timeline + non-destructive rewind (shadow worktree via
    `git worktree add --detach`; Hard rule 3 - HEAD/refs untouched, adversarially
    tested). The three open decisions are settled (ADR-0007 + `design/README.md`):
    the timeline is an **in-session event log** Skelly records itself (a `System`
    session-start anchor + the `Human` git events it witnesses - commits, which are
    restorable, and staging, which is not); rewind is **read-only inspection** (a
    shadow worktree, HEAD/refs untouched); persist is **layout only**. The
    `skelly-session` model (`Timeline` / `SessionEvent` / `Actor`,
    `Repo::shadow_checkout` -> `ShadowWorktree`) has a mandatory trust-contract
    integration suite; the right-dock UI (`⇧⌘H`, mutually exclusive with the git
    dock) lists the events with a viewing banner + actor legend, `↑/↓` (or `⌥⌘←/→`)
    scrub, `⌥⌘0` returns to now, and selecting a past commit rewinds to it. Verified
    by the `timeline_capture` headless PNG in both themes + a clean boot. The
    **launch-time layout restore** (`session.persist`) has since landed: on quit the
    full layout (all workspaces, their tabs + groups, each tab's pane tiling + per-pane
    cwd) saves to `~/.local/state/skelly/session.json` and restores on next launch,
    re-spawning each pane's shell in its saved cwd (layout only, processes never
    re-run). Remaining follow-ups: the `Agent` actor's transport (the still-open
    AI-actions contract), fork-on-edit, off-thread git, and global `⌥⌘←/→/0` while the dock is closed.
- [~] **M5 - Hardening & release.** Edge/empty/error states, perf budgets,
  packaging (signed macOS `.app`, Linux artifacts), first tagged release.
  - [x] **Shell-exit / crash overlay** (design §12 "Shell exits / crashes"). A pane
    whose shell ends no longer dies silently: `skelly-term` reaps the child and reports
    its `ExitStatus` (and kills the shell on drop, so the reader thread reaps it - closing
    a latent process/zombie leak); the renderer dims the pane with a translucent `bg.base`
    scrim over its preserved scrollback and centers an exit message + `↵ restart` / `⌥w
    close` hint (a layer above the terminal text, beneath the docks/overlays). `↵` respawns
    the shell in place via `sync_layout`; a focused dead pane swallows other input. Verified
    by an e2e exit-detection test, the pure `deadpane` message unit tests, and the
    `dead_pane_capture` PNG in both themes.
  - [x] **Empty state + never-quit close cascade** (design §10.2 + edge "Close last pane").
    Closing the only pane closes the tab; closing the only tab resets it to a fresh tab
    (never quits). A pristine single-pane tab paints a faint `skelly` wordmark + hint chips
    (`⌘K` / `⌘T` / `⌥|`, subtle `bg.elevated` pills) over its blank terminal, cleared on the
    first command (or split) via a per-tab `activated` flag. Pure `emptystate` module baked
    into the pane grid (unit-tested); verified by the `empty_state_capture` PNG in both
    themes. (Follow-up: an animated fade rather than an instant clear.)
  - [x] **Sidebar collapse rail** (design §08 "Sidebar modes"). `⇧⌘B` cycles the sidebar
    between the full panel and a slim 56px icon rail (compact centered tab numbers + a
    `sk` brand mark, the active tab keeping its accent bar + subtle fill); `⌘B` still
    shows/hides. The chosen mode round-trips `config.sidebar.mode` and persists per
    workspace (Hard rule 1). Pure `sidebar` module (`Fixed`/`Autohide`/`Hidden` state +
    the shared row layout so hit-testing works in both modes), unit-tested; surfaced in the
    palette ("Cycle sidebar mode"); verified by the `pane_capture` PNG (rail arg) in both
    themes. (Follow-up: hover-to-expand the rail.)
  - [x] **Not-a-git-repo Init button** (design §12 "Not a git repo"). The git dock's
    empty state now shows "No repository here" **+ an accent "Init repo ↩" button**; `Enter`
    runs it. `skelly_session::Repo::init` shells `git init` in the process cwd and
    rediscovers (integration-tested); the dock then refreshes to the new, empty repo. The
    button is keyboard-driven (mouse hit-testing over the dock is the same tracked
    follow-up as click-to-select-file). Verified by the `git_dock_capture` PNG (`norepo`
    arg) in both themes + a `gitdock` unit test.
  - [x] **Process-running-on-close confirm** (design §12 "Process running on close"). Closing
    a pane (`⌥w`) or tab (`⌘W`) that has a running foreground job no longer kills it silently:
    a centered confirm modal names the process ("`vim` is still running") and asks before
    closing. `Enter` or a second press of the close chord confirms; `Esc` cancels.
    `skelly_term::Terminal::foreground_job_pid` reads the PTY's foreground process group
    (`portable-pty`'s `process_group_leader`) and compares it to the shell's own pid
    (e2e-tested); the binary looks up the name via `ps -o comm=`. Pure `confirm` module (unit-
    tested); verified by the `pane_capture` `confirm` arg PNG in both themes.
  - [x] **Tab overflow scroll** (design §12 "Many tabs overflow"). The sidebar tab list
    windows into the available window height instead of clipping: the header stays pinned,
    the tab rows between it and the `+ New tab` action scroll, and the active tab always
    auto-scrolls into view. Hidden tabs are marked with `↑ N more` / `↓ N more` indicators
    on the spacer rows (single-width arrows, alignment-safe). A shared `Layout` drives both
    the rendered rows and the click hit-test so a click lands on exactly the tab drawn there,
    scroll offset included. Pure `sidebar` module (unit-tested: windowing, scroll-into-view,
    hit-through-offset); verified by the `pane_capture` `overflow` arg PNG in both themes.
  - [ ] Remaining edge/empty states: detached/rewound "warns before forking",
    theme-with-no-light-variant fallback.
  - [x] **Panic hook + file logging** (playbook §7). Logs now also stream (non-blocking) to
    a daily-rotating `skelly.log` in the XDG state dir (`$XDG_STATE_HOME/skelly`, else
    `$HOME/.local/state/skelly`), teed with stderr under the same `SKELLY_LOG` filter, and a
    panic hook logs the message + location + thread + a captured backtrace at `ERROR` before
    chaining to the default hook - so a crash is persisted for a bug report instead of
    vanishing (the appender's worker guard is held for the whole run so it flushes even on an
    abrupt exit). Tests: `panic_message` payload extraction + a hook-fires-and-logs test;
    verified the log file is created + populated on a real boot. (Follow-up: recover a single
    panicking pane in-window - a dead-pane state - without tearing down the window.)
  - [x] **Perf budgets** (`criterion` on the parser/renderer hot paths, playbook §4).
    `skelly-term/benches/parser.rs` measures `Parser::advance` throughput on representative
    streams - plain text (~100 MiB/s), SGR-heavy color (~120 MiB/s), a full-screen TUI
    repaint (~170 MiB/s) - plus the per-frame grid read (`cells`, ~5us for 80x24).
    `skelly-render/benches/render.rs` measures the pure per-frame CPU builders `grid_quads`
    and `text_runs` on a plain vs. an adversarial all-distinct-color grid (via a
    `#[doc(hidden)] bench_support` seam so no GPU or window is needed and no internal types
    leak). criterion is lean (no plotters/rayon); soft budgets are documented in each bench
    (a CI regression gate is a follow-up). (Surfaced a future optimization: the busy-grid
    `grid_quads` spends ~60us mostly in per-cell sRGB->linear conversion.)
  - [x] Packaging + distribution. A curl installer served from GitHub Pages
    (`curl -fsSL https://zottiben.github.io/skelly/install.sh | sh`) and a
    tag-triggered release workflow (`.github/workflows/release.yml`) that builds a
    universal macOS `Skelly.app` (ad-hoc signed by default; Developer ID +
    notarization wired behind repo secrets) plus Linux `x86_64`/`aarch64` binaries,
    with SHA-256 `checksums.txt`. The `.app` bundle + icon are built from
    `packaging/`. First tagged release: `v0.1.0`.

## Open product decisions

Tracked in [`design/README.md`](design/README.md): timeline AI-actions contract,
windowing (single vs multi OS window), rewind + edit behavior, persist scope, and
scrollback search scope. Resolve each as the relevant slice lands.
