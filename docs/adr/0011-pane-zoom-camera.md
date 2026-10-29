# 0011. Pane zoom is an animated camera over the spiral

- Status: Accepted
- Date: 2026-10-29
- Deciders: maintainer (animated zoom in the style of TrellisWM, zoom back out to the
  same panes, keyboard + pinch + two-finger double-tap); implementation agent (camera
  model, motion tokens, pinch commit rule, clipping)
- Related: ADR-0010 (Fibonacci spiral panes); AGENTS.md Hard rule 4; design/README.md
  decision 2026-10-29 (pane zoom); ai-planner `pane-zoom-camera`

## Context

`⌥Z` zoomed the focused pane by re-tiling the tab: the pane jumped to fill the
viewport and the other panes disappeared until the next `⌥Z`. The maintainer wants
the zoom of [TrellisWM](https://trelliswm.app/): the view moves into a pane with
animation, and moves back out to the panes exactly as they were. Zoom must also work
from a trackpad: a pinch and a two-finger double-tap.

The design guide (§10.4) only says "`⌥Z` zooms the focused pane to full, again to
restore". It gives no zoom motion, so the motion is a product decision.

## Decision

We will draw the spiral through a camera, and animate the camera instead of the
layout.

- **The spiral never changes for a zoom.** `skelly-pane` gives each pane a fixed slot in
  the spiral, in units relative to the viewport (`f64`, so the camera stays exact deep
  in a large spiral and across window resizes). A `Camera` frames part of that unit
  square. At rest the camera shows the whole spiral, or the focused pane filling the
  viewport when the tab is zoomed. `PaneTree::view(viewport, camera)` places every pane
  the camera can see, and `layout()` is unchanged at rest.
- **The camera moves the pane rects, not the text.** Each pane keeps its cell size: its
  glyphs, cell backgrounds and cursor are drawn at the pane origin and clipped to the
  pane rect and to the pane zone (the viewport). Panes slide past the zone edge and are
  cut there. The dividers and the focus ring fade out as one pane comes to fill the
  view.
- **Terminals resize once, not per frame.** Each terminal is sized from
  `PaneTree::resting_layout`: the zoomed pane fills the viewport, and every other pane,
  including the panes the zoom hides, keeps its spiral slot. The zoomed pane resizes
  when the motion starts.
- **Motion.** `⌥Z`, and a focus change while zoomed, glide the camera over 260 ms with
  the standard easing (design §03, the pane split / close token). Zoom follows focus,
  so `⌥H/J/K/L` and `⌥1..9` pan the camera from pane to pane while zoomed. `⌘K` or `⌥W` while
  zoomed un-zooms: the camera glides out from the zoomed pane (or the pane that took
  its slot) to the new spiral. Without zoom, opening or closing a pane re-tiles at once,
  as before.
- **Pinch.** A trackpad pinch over a pane follows the fingers: spreading them zooms
  into the pane under the pointer, and pinching them zooms back out. A 2x finger spread
  completes the zoom, or less for a pane that needs less magnification. When the
  fingers lift, the view settles on the far view past halfway, or past 10% while still
  moving toward it; else it springs back. The settle decelerates over the part of the
  260 ms that is left (at least 120 ms). The pane tree changes only when the pinch
  commits, so a pinch the user lets go of never resizes a terminal.
- **Two-finger double-tap** (macOS smart zoom) toggles zoom on the pane under the
  pointer, with the same glide as `⌥Z`.
- Gestures are ignored over the sidebar, the right dock, the find bar and any modal,
  menu or settings view, like a click.
- Session persistence is unchanged: the zoom flag is saved, and a restored tab starts
  at rest.

## Consequences

- Zoom shows where a pane sits in the spiral and returns to the same layout, for every
  pane count.
- Mid-motion, a pane's text does not scale with its rect: a pane grows or shrinks
  around a grid of the same size. The text stays sharp and the shells see one resize
  per zoom.
- Pinch and two-finger double-tap work only on macOS: winit 0.30 delivers
  `PinchGesture` and `DoubleTapGesture` only on macOS and iOS. On Linux, zoom is `⌥Z`.
- The pane layer and its overlays are now clipped to the pane zone. The find bar is
  window chrome in the same pass and stays unclipped.
- There is no reduced-motion setting; Skelly has none yet for any of its motion.

## Alternatives considered

- **Keep the instant re-tile** - does not meet the request. Rejected.
- **Animate the layout** (tween every pane rect from the spiral to full screen) - the
  other panes would shrink to nothing instead of staying where they are, and the
  camera could not pan between panes while zoomed. Rejected.
- **Scale the pane content with the camera** (magnify the grid like an image) - text
  would blur or need re-rasterizing at fractional sizes every frame. Rejected.
- **Resize the terminals every frame** - floods the shells with resizes and reflows
  every line on every frame. Rejected.
