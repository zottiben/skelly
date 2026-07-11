# design/ - source of truth for skelly

This directory is the **binding design spec**. Build the app against it. When a
decision isn't covered by the guide, decide it, then record the decision here so
the next contributor (human or agent) inherits it.

## The guide

- **`Skelly Design Guide.dc.html`** - the full v0.1 spec: foundations, brand,
  color/token system, ANSI palette, type, scale/motion, icons, layout/window
  anatomy, the component library with states, every core screen mockup,
  keybindings, behavior/edge states, and the `config.toml` schema. Open it in a
  browser to view. `support.js`, `.thumbnail`, `screenshots/`, and `uploads/` are
  assets for that export.

**Generated file - do not hand-edit.** The `.dc.html` is a Claude Design export.
To change the design, edit it in the design tool and re-export; don't patch the
HTML by hand. (See AGENTS.md Hard rule 6.)

## Open decisions

The guide flags these as "Confirm first" - genuine product/architecture calls to
settle before or as the relevant code lands. Resolve each, then replace its line
with the decision + date.

- [ ] **Timeline AI-actions contract** - how the session timeline receives AI
  actions. Needs an explicit integration hook/contract, not shell heuristics.
- [x] **Windowing** - single OS window for v0.1 via `winit`; multi-window is a
  later additive decision. _Resolved 2026-07-11 (ADR-0004)._
- [ ] **Rewind + edit** - when viewing a past state, fork a branch on edit, or
  block edits while detached?
- [ ] **Persist scope** - restore layout only, or attempt to re-run processes?
- [ ] **Scrollback search scope** - active pane only, or all panes?

Deferred stack/foundation choices (from init; keep TBD until crates are picked):

- [x] **GPU cell renderer** - `wgpu` + `cosmic-text`/`glyphon`. _Resolved
  2026-07-11 (ADR-0003)._
- [x] **PTY / shell I/O** - `portable-pty`. _Resolved 2026-07-11 (ADR-0002)._
- [x] **Font shaping + Nerd Font fallback** - `cosmic-text` (shaping + fallback +
  atlas). _Resolved 2026-07-11 (ADR-0003)._
- [x] **Terminal core** - `alacritty_terminal` behind a Skelly-owned trait.
  _Resolved 2026-07-11 (ADR-0001)._

## Decision log

Record settled decisions here, newest first: `YYYY-MM-DD - <decision> (was: <the
open question>)`.

- 2026-07-12 - **Command palette (first slice).** `⌘K` opens a centered overlay over
  the live terminal (Hard rule 4), `Esc` closes; typing filters a built-in command
  set (case-insensitive substring for now), up/down navigates, Enter runs. The panel
  uses `bg.elevated`, a `border.strong` outline, a translucent `accent` selected-row
  highlight, and an `accent` caret; hints are `fg.muted`. The panel sizes to its
  widest line. Deferred (guide describes these, but they're a later slice): fuzzy
  matching with accent-highlighted matched characters, the mode prefixes (`>`
  commands / `/` files / `?` help / plain = scrollback search), surfacing tabs /
  themes / files, `⌘↵` run-in-new-pane, and merging user `[keys]` + the configurable
  `panes.leader`. Also corrected the focused-pane ring from `accent` to
  `border.strong` (the guide's token table: `border.strong` #6C6F93 dark / #ACB0BE
  light) - the `accent` focus ring belongs to interactive UI elements, not panes.
- 2026-07-11 - **Pane keybindings (M3 wiring).** Pane control uses `Alt` (`⌥`) as a
  direct, leader-less modifier, matched on the *physical* key (so macOS Option-key
  glyph remapping doesn't interfere). The guide shows `⌥|` split-right, `⌥-`
  split-down, `⌥Z` zoom, and `⌥1..⌥8` focus-by-number; these are honored exactly.
  The guide is silent on the rest, so decided here: `⌥h/j/k/l` directional focus,
  `⌥⇧h/j/k/l` resize the enclosing divider, `⌥w` close (the guide's `⌘W` is
  close-*tab*), and `⌥=` even-out. The configurable tmux-style leader
  (`panes.leader`, default `ctrl+a`) and the full remappable `[keys]` registry +
  command-palette surfacing are deferred to the command-palette slice; the built-in
  `⌥` chords ship first.
- 2026-07-11 - **Pane dividers + focus ring.** With more than one pane, each pane
  gets a subtle 1px `border` divider and the focused pane a 2px `accent` ring
  (`border.strong` in the guide == accent `#BD93F9`). A lone pane stays borderless.
  Added a `border` UI token: Ossein Dark `#313244` (a surface color present in the
  guide), Ossein Light `#BCC0CC` (the matching Catppuccin-Latte surface the Ossein
  Light palette derives from - the guide's light tokens are sparse). Geometry: a
  12px logical window margin around the pane area, a 6px logical inset inside each
  pane between its border and cells.
- 2026-07-11 - Architecture decisions are recorded as ADRs in `docs/adr/`
  (ADR-0000). Foundation-stack choices are proposed in ADR-0001..0004, pending
  maintainer ratification before the M1 walking skeleton lands them.
