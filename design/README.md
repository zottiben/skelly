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

- 2026-07-12 - **Git diff dock (read-only) layout.** The dock opens on the right edge
  (`⇧⌘G`, `Esc` to close) as base chrome over the live terminal (Hard rule 4; the pane
  viewport insets to its left, like the sidebar insets from the left). Fixed at the
  guide's **420px** default; the resizable 360-560 range is a follow-up. At 420px the
  guide's wide side-by-side file-list + diff mockup (§10.6) does not fit, so the dock
  uses a **vertical stack**: a status bar (branch / ahead-behind / totals), the
  changed-file list, then the selected file's unified diff. `↑/↓` move between files,
  `PageUp/PageDown` scroll the diff. The wide side-by-side "full view" is deferred to a
  later slice. Diff colors use the new `diff.add` / `diff.del` / `diff.hunk` tokens
  (separate from the ANSI palette, Hard rule 2); add/del/hunk line backgrounds are drawn
  as translucent quads. Scoped to the repo of the **process cwd** for now (real per-pane
  cwd tracking is a follow-up, the same blocker as cwd-based tab titles). _(was: the
  guide leaves the narrow-dock layout unspecified.)_
- 2026-07-12 - **Settings view.** A full in-window view over `config.toml` (Hard rule
  4), opened with `⌘,` and dismissed with `Esc`, drawn over the still-running terminal
  (never a route; focus returns to the exact pane on close). Left category nav
  (Appearance, Sidebar, Tabs, Panes, Session, Git), right control list; keyboard-driven
  (`↑/↓` move control, `←/→` or Enter change value, `Tab` switch category). Every
  control maps to **exactly one** `config.toml` key (Hard rule 1); a test diffs the
  serialized config and asserts a single leaf - the control's declared key - changes.
  Edits persist immediately (`Config::save_default`, atomic temp-file rename) since the
  file is the source of truth, and apply live where cheap (theme repaints everything,
  including the open settings view itself; sidebar mode/width re-fit the shells);
  font / cursor / opacity persist and take effect on next launch (live font re-shaping
  is a follow-up). Rendered via a dedicated renderer pass (`Renderer::set_settings`):
  a `bg.elevated` content panel over a `bg.base` nav strip, a `border` divider, the
  active category's `accent` bar + `accent.subtle` fill, and the focused control's
  translucent `accent` highlight - all UI tokens (Hard rule 2), verified in both
  themes. Decided here (the guide is silent on these): the in-settings nav keys above;
  the nav categories are the config sections we actually have (the mockup's General /
  Keybindings / Shell & env / Advanced wait on config keys or the `[keys]` registry we
  do not have yet); the rich widgets (theme cards, sliders, toggles) are represented
  textually; category markers are alignment-safe ASCII glyphs until the fixed-metric
  cell renderer (M2c) can place Nerd-Font icons. Deferred: mouse hit-testing in
  settings, and a debounce on the per-edit file write.
- 2026-07-12 - **Sidebar chrome (second half of "sidebar + tabs").** The persistent
  left dock now renders: a fixed-width panel (config `[sidebar] width`, default 240
  logical px) with a quiet brand header, the open-tab list, and a "+ New tab" action.
  The active tab is marked per the guide's "Sidebar tab item" component - an `accent`
  bar on its left edge plus an `accent.subtle` (accent at ~0.16 alpha) row fill - and a
  `border` divider runs down the right edge. The sidebar shares the app `bg.base` (no
  separate sidebar-surface token was invented; Hard rule 2), so it reads as quiet
  chrome. `⌘B` shows/hides it (the pane viewport insets by its width and the shells
  re-fit); clicking a tab switches to it, clicking "+ New tab" opens one. It draws as
  base chrome - a dedicated quad+text load-pass pair beneath the command-palette
  overlay (`Renderer::set_sidebar` / `SidebarView`), shared with the headless capture.
  Tabs are labeled by position (`Tab 1..`); cwd / command titling (`[tabs] title` /
  `follow_cwd`) needs shell-cwd tracking and is a later slice. Deferred: the `⇧⌘B` slim
  56px rail (config `mode` / `width < 180`), the pinned grid + `⇧⌘P`, collapsible
  groups, drag-reorder, and the footer action icons (`⚙ ◐ ⟲ ⑂`) - those wait on the
  settings / theme / timeline / git surfaces they trigger.
- 2026-07-12 - **Tab model (first half of "sidebar + tabs").** The window now holds
  multiple tabs, each an independent tiling workspace (its own pane tree + a live
  shell per pane + selection); switching a tab swaps the whole terminal workspace and
  background tabs keep their shells running. Keybindings follow the guide's Tab
  management table: `⌘T` new tab, `⌘W` close tab, `⌘1..9` jump to the nth tab, and the
  guide's `⌥⇧[` / `⌥⇧]` bracket chords to cycle prev/next. As with the other `⌘`
  bindings (`⌘K` / `⌘C` / `⌘V`), the command modifier is mapped to `Super` on both
  platforms for now; the guide's Linux `Ctrl+Shift+*` mapping waits on the full
  `[keys]` registry. `⌘W` is tab-close and `⌥w` stays pane-close (per the 2026-07-11
  pane-keybindings decision), so the guide's overloading of `⌘W` onto panes is not
  adopted. Deferred to the sidebar-chrome slice (no visual tab affordance yet): closing
  the last tab shows the guide's empty state instead of a no-op, and closing a tab with
  a running foreground process should confirm first. Also deferred: the sidebar that
  lists tabs, `⌘B`/`⇧⌘B` sidebar modes, groups, and pinning.
- 2026-07-12 - **Palette fuzzy matching.** The command palette now fuzzy-matches
  (query characters must appear in order in the label, ASCII case-insensitive) and
  ranks results by an earliest-first-match, fewest-gaps score (ties keep the
  registry order); the matched characters render in `accent`, per the guide. Still
  deferred: the `/` `?` mode prefixes, surfacing tabs/themes/files, and `⌘↵`
  run-in-new-pane.
- 2026-07-12 - **Live theming.** Switching the UI theme repaints every surface live
  (Hard rule 2). `Renderer::set_theme(name)` re-resolves the semantic tokens (the
  clear color + all quads read the theme each frame; text-layer fallback color
  updated), and the binary re-resolves the ANSI palette + rewrites
  `config.appearance.theme` (the source of truth, Hard rule 1). Surfaced as the
  palette commands "Theme: Ossein Dark / Light". Open follow-up: the UI theme and the
  ANSI color scheme are meant to be independently selectable (Hard rule 2), but the
  config has a single `appearance.theme` today, so one name drives both; a separate
  ANSI-scheme key (and live config-file watch) is a later slice.
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
