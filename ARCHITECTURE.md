# Architecture

A high-level orientation for reading the code cold. The binding *product* spec is
[`design/`](design/); the *why* of technical decisions is in [`docs/adr/`](docs/adr/).
This file is the map between them.

## Shape

Skelly follows the Ghostty pattern: a **correctness-critical terminal core with no
UI/GPU dependency**, a renderer that draws a snapshot of it, and a thin binary that
wires everything to the platform. That maps onto a Cargo workspace where
dependencies flow one way and there are no cycles (Cargo enforces the acyclicity;
we enforce the direction).

```
                 +-------------------+
                 |      skelly        |  binary: window, event loop, sidebar,
                 |     (bin crate)    |  pane tree, palette, settings, wiring
                 +---------+---------+
                           | depends on
      +--------------------+--------------------+
      v                    v                    v
+-------------+   +-----------------+   +------------------+
| skelly-     |   | skelly-render   |   | skelly-session   |
| term        |   | GPU cell grid,  |   | timeline, rewind |
| PTY, ANSI/  |   | fonts, theme    |   | (shadow worktree)|
| VT, grid,   |   | token resolve   |   | git diff         |
| scrollback  |   +--------+--------+   +---------+--------+
+------+------+            |                      |
       |                   v                      v
       |            +--------------------------------+
       +----------->|        skelly-config           |  the source of truth:
                    |   config.toml load / validate  |  schema, defaults
                    +--------------------------------+
```

- **`skelly-config`** is the leaf everyone may read - schema, defaults, validation,
  round-trip serialization (Hard rule 1). No dependency on anything app-specific.
- **`skelly-term`** is the terminal core (Skelly's `libghostty-vt` analog): PTY,
  the ANSI/VT state machine, the cell grid, scrollback, selection, reflow. It has
  **no window, GPU, or OS-UI dependency** so it stays fuzzable and
  conformance-testable (vttest / esctest) headless.
- **`skelly-render`** rasterizes the grid on the GPU and resolves semantic theme
  tokens (`category.role.state`) to colors. The UI token set is a distinct type
  from the terminal ANSI palette so they can never be crossed (Hard rule 2).
  Renders off-thread from a snapshot so I/O and input never block frames.
- **`skelly-session`** records the timeline and restores past states
  non-destructively via a shadow worktree - never moving HEAD/refs (Hard rule 3).
- **`skelly`** owns the window, event loop, focus model, and the layer stack:
  the pane tree is the permanent base layer; git diff / timeline are a right dock
  (one at a time), the palette is a centered overlay, settings a full in-window
  view - all over a live terminal, all Esc-dismissed (Hard rule 4).

## Data flow (once M1 lands)

`PTY bytes -> skelly-term (parse -> grid/scrollback update) -> snapshot ->
skelly-render (shape -> atlas -> GPU draw) -> window`. Input travels the reverse
way: `window key event -> skelly (keymap) -> action or skelly-term (write to PTY)`.

## Boundaries as contracts

- Dependencies point toward `skelly-config`; nothing depends on the `skelly`
  binary. New cross-crate coupling goes through a small, intentional interface (a
  trait), not a reach-through.
- Foundation backends (terminal core, PTY, renderer, windowing) sit behind
  Skelly-owned traits so they can be swapped without touching callers. See ADRs
  0001-0004.
- Libraries return typed errors (`thiserror`); the binary contextualizes at the
  edge (`anyhow`).

## Milestones

Delivery is sequenced M0-M5; see [`ROADMAP.md`](ROADMAP.md) for what each contains
and where we are.
