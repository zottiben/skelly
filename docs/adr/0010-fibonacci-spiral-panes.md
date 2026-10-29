# 0010. Panes tile as a clockwise Fibonacci spiral, opened with `⌘K`

- Status: Accepted
- Date: 2026-10-29
- Deciders: maintainer (spiral layout, `⌘K` as the only way to open a pane, no cap,
  palette on `⌘P`); implementation agent (share formula, persistence shape)
- Related: ADR-0005 (amended in part); AGENTS.md Hard rule 4; design/README.md
  decision 2026-10-29; ai-planner `fibonacci-panes`

## Context

The M3 pane model (ADR-0005) was a binary split tree. The user picked a direction for
every split (`⌥|` / `⌥-`, or through a tmux-style leader), resized and swapped panes,
cycled layout presets, and was capped at 8 panes per tab. The maintainer wants one
gesture instead: `⌘K` opens a pane, the layout sizes itself along the Fibonacci
sequence in a clockwise spiral, and there is no pane limit. The old split behavior
does not need to be kept.

`⌘K` was the command palette (design guide §11), so the palette needs a new chord.
`⇧⌘P` is already pin; `⌘P` is free.

## Decision

We will replace the split tree with an ordered list of panes tiled as a clockwise
Fibonacci spiral, and open panes only with `⌘K`.

- With `n` panes, pane `i` takes `F(k) / F(k+1)` of the region left to it, where
  `k = n - i` panes are still to place (the last two share an even half). The cut
  side turns clockwise: left, top, right, bottom, then left again. This is the
  golden-rectangle tiling: the first pane is the largest (it approaches 61.8% of the
  viewport) and each later pane is smaller.
- `⌘K` appends a pane at the end of the spiral and focuses it. Closing a pane
  removes it from the list and the spiral re-tiles.
- There is no pane cap. A pane too small for its chrome draws no status line or exit
  message, and its cell backgrounds and cursor are clipped to its rect like its
  glyphs, so it never paints over a neighbor.
- Kept: directional focus (`⌥h/j/k/l`, `⌥↑/↓`), `⌥1..9` focus by number, `⌥Z` zoom,
  `⌥W` close, click to focus.
- Removed: directional splits, resize, swap, even-out, layout presets, the pane
  leader, the palette's split / even-out commands and its files-mode `⌘↵` new pane,
  and the `[panes] max` and `[panes] leader` keys. Old config files still load; the
  keys are ignored and dropped on the next save.
- The command palette opens and dismisses with `⌘P`.
- Session persistence stores only the focused index and the zoom flag; the pane
  count is the number of saved cwds. A session saved with the old split tree
  restores as a spiral with the same pane count.
- The crate boundary from ADR-0005 stands: `skelly-pane` stays a pure leaf crate,
  with `proptest` coverage of the tiling invariants.

## Consequences

- One chord and no layout decisions. The layout is fully set by the pane count, so
  users can no longer choose a split direction or size. Zoom (`⌥Z`) is the way to
  work in a small pane.
- Deep in the spiral, panes get very small: on a laptop screen the innermost panes of
  a 12-pane tab are shorter than one text row. This is the accepted cost of no cap.
  Each pane is a live shell, so only system resources bound the count.
- `⌘K` no longer opens the palette, which differs from the design guide's §11 and its
  empty-state chips. The guide is a generated export (Hard rule 6), so the divergence
  is recorded in `design/README.md` until the guide is regenerated.

## Alternatives considered

- **Halve the newest pane each time** (the dwm-style "fibonacci" layout) - simpler,
  but the sizes follow powers of two, not the Fibonacci sequence. Rejected: the
  request is a Fibonacci sequence.
- **Keep the split tree and add `⌘K` as one more split** - keeps the legacy surface
  the maintainer asked to remove. Rejected.
- **Keep a cap** - contradicts the requirement. Rejected.
