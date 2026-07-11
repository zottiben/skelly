# 0001. Terminal core engine

- Status: Accepted
- Date: 2026-07-11
- Deciders: maintainers
- Related: design/README.md (open foundation decision); ADR-0002

## Context

`skelly-term` must parse the ANSI/VT byte stream and maintain the cell grid,
scrollback, selection, and - hardest of all - reflow on resize. Terminal
correctness is a trust contract for the vim/neovim/tmux users Skelly targets
(AGENTS Hard rules; the charter values correctness and long-term maintainability
over dev cost). Reflow and VT conformance are the single most bug-prone areas in a
terminal, and getting them wrong corrupts exactly the TUI workflows we exist for.

The Rust ecosystem offers: `alacritty_terminal` (Alacritty's grid + parser +
reflow + scrollback, as a library, Apache-2.0, battle-tested); `vte` (Alacritty's
low-level parser only); and WezTerm's `termwiz`/`vtparse` (MIT). Ghostty's own path
was to hand-roll the core for maximum throughput.

## Decision

We will build `skelly-term` on **`alacritty_terminal`** as the VT/grid core, placed
**behind a Skelly-owned trait boundary** (our own `Terminal`/`Grid` interface) so no
other crate depends on it directly. We inherit proven parsing, reflow, and
scrollback now, and keep the option to hand-roll on `vte` later for throughput
without touching UI or render code.

## Consequences

- Fastest path to a *correct* terminal; we spend our effort on rendering and the
  Skelly-specific features (panes, timeline, git) rather than reinventing reflow.
- We take a dependency on Alacritty's model and update cadence; the trait boundary
  is the reversal path if we outgrow it.
- We still run vttest/esctest and fuzz the parser in CI - reusing a proven core
  does not remove our obligation to verify conformance headless.
- License (Apache-2.0) is compatible with Skelly's MIT-OR-Apache-2.0 and passes
  `cargo deny`; confirm at adoption time.

## Alternatives considered

- **Hand-roll on `vte`** (Ghostty's spirit) - maximum control and throughput, but
  reflow/scrollback correctness is months of the riskiest work before we have a
  usable terminal. Deferred, not rejected; the trait keeps it open.
- **WezTerm `termwiz`** - capable and MIT-licensed, but a larger surface than we
  need and less drop-in as a pure grid core.
