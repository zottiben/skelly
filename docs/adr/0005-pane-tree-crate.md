# 0005. Pane-tree model as a leaf crate (`skelly-pane`)

- Status: Accepted
- Date: 2026-07-11
- Deciders: maintainers
- Related: AGENTS.md Hard rule 4 (docks/overlays are layers; <=8 panes); the
  engineering playbook (§5 "crate boundaries are contracts"; §4 property tests for
  "pane-tree invariants"); ADR-0004 (single window)

## Context

M3 needs a tab's pane workspace: nested (uneven) splits, directional focus, resize,
zoom, and a hard cap of 8 panes. The `AGENTS.md` layout originally listed the "pane
tree" under the `skelly` binary alongside the window, sidebar, and wiring.

The pane tree is really two separable things: a **model** (pure tiling geometry +
state - how splits nest, how focus moves, how a viewport tiles) and its **wiring**
(mapping a pane id to a live terminal, rendering each pane at its rectangle, routing
input, binding keys). The model is pure logic the playbook explicitly wants covered
by property tests. Building it first, before the heavier rendering/terminal wiring,
is the same "pure logic first" sequencing that made `skelly-config` the natural first
slice. But pure model code placed *inside* the binary is unreachable from `main`
until the wiring lands, so it trips `dead_code` under `-D warnings` - and silencing
that lint is against the charter.

## Decision

Extract the pane-tree **model** into its own leaf crate, **`skelly-pane`**, mirroring
`skelly-config`: no UI, GPU, or terminal dependency; a clean, fully unit- and
property-tested contract (`PaneTree` + `PaneId`/`Dir`/`Rect` + `layout`). The
`skelly` binary depends on it and owns the **wiring** (pane-id -> terminal,
rendering at each rect, dividers, focus ring, keybindings).

## Consequences

- The model lands and ships green as its own reviewable slice, fully tested
  (tiling invariants as `proptest` properties), with no `dead_code` allow and no
  half-rendered UI - the multi-pane rendering wiring is a clean follow-up slice.
- One more crate in the workspace (six). The `AGENTS.md` layout is updated: the
  model is `skelly-pane`; the binary owns the pane *widget*/wiring.
- The boundary stays honest and one-way: `skelly-pane` is a leaf everyone can read
  (like `skelly-config`); it never depends on render/term/session.

## Alternatives considered

- **Keep it in the binary, unwired** - trips `dead_code` under `-D warnings`;
  fixable only by an `#[allow]` the charter forbids. Rejected.
- **Keep it in the binary, fully wired in one slice** - bundles model + terminals-
  per-pane + renderer-rects + dividers + input routing into one sprawling,
  hard-to-verify PR; the playbook says split slices that large. Rejected in favor of
  model-first.
- **Fold it into an existing crate** (`skelly-render`/`skelly-session`) - violates
  crate boundaries; the tiling model is neither rendering nor session state.
  Rejected.
