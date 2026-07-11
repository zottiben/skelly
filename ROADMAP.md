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
- [~] **M1 - Walking skeleton** (in progress). Window opens -> spawns the login
  shell in a PTY -> shell output paints in a GPU cell grid -> keystrokes reach the
  shell -> clean quit. Single pane, one shell, ugly but real. Retires the hardest
  integration risks (GPU surface, PTY plumbing, event loop, glyph upload).
  - [x] **M1a** - native window (`winit`) + `wgpu` surface clearing to the theme
    background; clean quit. GPU-surface + event-loop risk retired.
  - [ ] **M1b** - render text (a static line) via `glyphon`/`cosmic-text` in the
    cell font + colors. Text-rendering risk retired.
  - [ ] **M1c** - spawn the PTY (`portable-pty`) + terminal core
    (`alacritty_terminal`), pipe output through the parser into the grid, forward
    keystrokes to the shell. Full walking skeleton.
- [ ] **M2 - Core terminal.** VT/ANSI correctness (vttest / esctest in CI),
  scrollback, selection + copy/paste, resize/reflow, font shaping + Nerd Font
  fallback, live theme-token resolution.
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
