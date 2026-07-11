# Roadmap

Delivery is sequenced in milestones, each a set of thin vertical slices with a
demoable outcome. We never start a later milestone's polish before the current one
runs end-to-end. Rationale and detail are in the engineering playbook.

## Current: M2 -> M3

M0, M1, and the core of M2 (core terminal) are in place and run end-to-end. M2 has
a handful of tracked carry-overs - the unchecked boxes below (bespoke cell renderer,
cargo-fuzz, reflow/theme polish, SGR fidelity follow-ups) - which we finish
opportunistically rather than block M3 on, since the terminal already works. M3
(Skelly shell UX) is the next milestone.

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
    - [ ] Merge user `[keys]` overrides + the configurable `panes.leader` (tmux-style
      `ctrl+a`); fuzzy match with accent-highlighted characters; surface tabs /
      themes / files and the `/` `?` mode prefixes.
- [ ] **M4 - Signature features.** Per-repo git diff dock with hunk staging;
  session timeline with non-destructive rewind (shadow worktree).
- [ ] **M5 - Hardening & release.** Edge/empty/error states, perf budgets,
  packaging (signed macOS `.app`, Linux artifacts), first tagged release.

## Open product decisions

Tracked in [`design/README.md`](design/README.md): timeline AI-actions contract,
windowing (single vs multi OS window), rewind + edit behavior, persist scope, and
scrollback search scope. Resolve each as the relevant slice lands.
