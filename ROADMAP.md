# Roadmap

Delivery is sequenced in milestones, each a set of thin vertical slices with a
demoable outcome. We never start a later milestone's polish before the current one
runs end-to-end. Rationale and detail are in the engineering playbook.

## Current: M0 -> M1

M0 (foundation) is in place. The next slice begins M1 once the foundation-stack
ADRs (0001-0004) are ratified.

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
  - [ ] **M2b** - per-cell background colors + cursor (a colored-quad pipeline);
    honor the configured font with proper monospace fallback.
  - [ ] **M2c** - the real fixed-metric cell renderer (glyph atlas + instanced
    quads) replacing reflowed text, so cells align exactly regardless of glyph.
  - [ ] **M2d** - VT/ANSI conformance (vttest / esctest in CI, fuzz the parser),
    scrollback, selection + copy/paste, resize/reflow, live theme-token resolution.
- [ ] **M3 - Skelly shell UX.** Sidebar + tabs/groups/pinning; pane tree
  (split/focus/resize/zoom, <=8); command palette; settings view; live theming.
- [ ] **M4 - Signature features.** Per-repo git diff dock with hunk staging;
  session timeline with non-destructive rewind (shadow worktree).
- [ ] **M5 - Hardening & release.** Edge/empty/error states, perf budgets,
  packaging (signed macOS `.app`, Linux artifacts), first tagged release.

## Open product decisions

Tracked in [`design/README.md`](design/README.md): timeline AI-actions contract,
windowing (single vs multi OS window), rewind + edit behavior, persist scope, and
scrollback search scope. Resolve each as the relevant slice lands.
