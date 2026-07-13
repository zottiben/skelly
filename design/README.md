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

- [~] **Timeline AI-actions contract** - _v1 resolved 2026-07-12: the timeline is
  an in-session event log Skelly records itself (see decision log); the model
  carries an `Agent` actor and a `record()` API, but the **transport** by which
  external AI actions reach the timeline stays open - a future additive hook (an
  explicit append-only event log the agent writes), never a shell heuristic._
- [x] **Windowing** - single OS window for v0.1 via `winit`; multi-window is a
  later additive decision. _Resolved 2026-07-11 (ADR-0004)._
- [x] **Rewind + edit** - _Resolved 2026-07-12: v1 rewind is **read-only
  inspection**. Restoring materializes the commit in a shadow worktree
  (HEAD/refs untouched, Hard rule 3) and shows the viewed state; Skelly never
  auto-forks or moves panes into it. Editable rewind / fork-on-edit is deferred._
- [x] **Persist scope** - _Resolved 2026-07-12: **layout only** (restore tabs +
  pinned on launch), never re-run processes, per the guide's own `[session]
  persist` comment. The launch-time restore itself is a follow-up, separate from
  the in-session timeline._
- [x] **Scrollback search scope** - _Resolved 2026-07-13: **active pane only** (`⌘F` searches the
  focused pane's scrollback via `Terminal::find`). An all-panes / cross-pane search is a later
  additive option._
- [~] **Per-workspace cwd/theme isolation** (§08 #2 "Each isolates tabs, cwd & theme") -
  _Partially resolved 2026-07-13. **Tabs**: isolated (stashed per workspace, shells keep
  running). **cwd**: already isolated - each workspace's tabs are separate shell processes that
  each own their cwd (shells spawn in `current_dir`, `[panes] split_inherits_cwd`, tabs
  `follow_cwd`), so a workspace's directory context is its shells' cwds; there is no single
  "workspace cwd" in the schema. **Theme**: left OPEN. A per-workspace theme override has **no
  config key** - the schema is a single global `[appearance] theme` with no `[[workspace]]`
  section - so storing it would violate Hard rule 1 (no UI state without a config key) and mean
  inventing schema the guide never defines (Hard rule 5). Settling it needs a guide-level config
  extension (e.g. a `[[workspace]]` array with an optional `theme`), not a code guess. Until
  then Skelly keeps one global UI theme; switching workspaces does not repaint._

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

- 2026-07-13 - **Hover tooltips (§09 primitive).** The guide's tooltip primitive is built and
  applied to the icon-only chrome, where a label isn't otherwise visible: the utility-bar icons
  (⚙ Settings ⌘, / ◐ Toggle theme / ⟲ Session timeline ⇧⌘H / ⑂ Git diff ⇧⌘G), the command well
  (Search or run ⌘K), the workspace chips (their name / New workspace), the slim rail's numbered
  tabs, and pinned icon tiles (the tab title). A tooltip reveals after a 450ms hover rest (the
  loop sleeps to the reveal via `WaitUntil`) and hides the moment the pointer moves off; it reuses
  the overlay card at the lowest priority and never captures input. Full-panel list tabs already
  show their titles, so they get no tooltip. Decided: tooltips attach to the obviously-icon-only
  affordances (not a guess - completing the defined primitive where a name is otherwise hidden).
- 2026-07-13 - **Sidebar drag-resize + snap-to-rail (§12 "Sidebar collapsed").** The full sidebar
  panel's right edge is now draggable (a 5px grab zone + `EwResize` cursor, mirroring the dock
  resize), applying the guide's snap thresholds live: dragging to >=180 logical keeps the full
  panel at that width (<=360), 90-180 snaps to the slim rail (Autohide), and <90 hides it (`⌘B`
  restores). The width/mode persist once on release (Hard rule 1; per-frame writes avoided). A new
  drag only starts from the full panel (the rail widens via `⇧⌘B`), but a single continuous drag
  crosses all thresholds both ways. The `[sidebar] width` valid range is tightened to the guide's
  **180-360** (config validation + the settings slider), since below 180 is the rail mode, not a
  narrow panel.
- 2026-07-13 - **Transient toasts (§09 primitive / §12 flows).** The guide's toast primitive (a
  bottom-anchored `bg.elevated` card - status dot + message) is built and auto-dismisses after
  4s (the loop sleeps to the deadline via `ControlFlow::WaitUntil`, so it costs nothing idle).
  Two triggers wired: (1) **pane cap** - a split at the 8-pane limit was a *silent* no-op; it now
  shows "Pane limit reached (8 max)" (design §12 flow 1). (2) **commit success** - an accent/green
  `Success` toast "Committed <sha>" (design §12 flow 3), complementing the git dock's existing
  inline `⌘U` Undo affordance (soft reset), which stays the Undo entry point. Non-modal: the toast
  reuses the overlay card at the lowest priority and never captures input. An action-bearing toast
  (a clickable Undo on the card itself) is a later addition; today Undo lives on the dock.
- 2026-07-13 - **Right-click tab action menu (§08).** The guide's "Right-click any tab for the
  full action menu" is built: right-clicking a sidebar tab focuses it and opens an anchored menu
  (reusing the shared overlay card - `bg.elevated` + shadow + `border.strong` ring), with
  keyboard (↑/↓/↵/esc) + mouse (hover/click, click-away dismiss) drive. Items: Pin/Unpin (⇧⌘P),
  Rename… (F2), Duplicate (new tab inheriting the group + name), New group (⇧⌘N), Move to
  "<group>" per other group, Remove from group (when grouped), and Close (⌘W, danger color =
  `diff.del` #F38BA8, matching the mockup). Decided: the mockup's "Move to group ›" **submenu** is
  realized as flat rows (the overlay slot is a single card; a nested submenu card is a later
  refinement) - this keeps the full move-into-existing-group behavior without a second overlay.
- 2026-07-13 - **Collapsible tab groups (§08 #5) + `⇧⌘N` fix.** Real named, collapsible groups
  now exist (was: a single decorative "repo · branch" header, and `⇧⌘N` wrongly created a
  workspace). Model: each workspace owns `Vec<TabGroup>`; each tab carries `group: Option<usize>`.
  `⇧⌘N` creates a group from the active tab (named after its repo·branch context, e.g.
  `skelly · main`, or `Group N`) - the guide's `⇧⌘N New group`, not a workspace (the guide has
  no new-workspace key; the `+` chip still makes those). The sidebar renders ungrouped tabs first,
  then each group as a header (▾/▸ chevron + mono name + member count); clicking a header
  collapses/expands it (members hide but their shells keep running). `⌘1…9` now jumps to the nth
  tab **in the active group** (guide §11), and closing a tab prunes any emptied group. Decided:
  the grouped list is not overflow-windowed (organization implies modest counts); the flat
  no-groups path keeps the exact §12 windowing. Groups are per-workspace (stashed on switch).
  Still deferred (needs a context menu / cross-group drag, no guide keybinding): right-click
  "Move to group" and dragging a tab between groups; drag-reorder today reorders the flat list.
- 2026-07-13 - **Palette `⌘↵` run in new pane (§10.8/§11).** The palette footer now shows
  `⌘↵ run in new pane` in files mode, and `⌘↵` on a file entry splits a fresh pane (Dir::Right,
  respecting the 8-pane cap) before typing the path there, so the command runs in a new pane
  rather than the focused one; plain `↵` still types into the focused pane. This was the last
  deferred §11 binding (previously left because the palette runs app actions, not shell
  commands - the file-entry path gives it a clean, faithful mapping).
- 2026-07-13 - **Sidebar deferred interactions built: drag-reorder tabs + rail hover-to-expand
  (§08).** (a) A sidebar tab can be **dragged to reorder** it - press a tab, drag over another,
  and the list reorders live (the dragged tab stays active); release ends the drag. (b) The
  guide's §08 rail spec ("56px · hover to expand") is now real: hovering the slim icon rail
  **expands it to the full panel as an overlay** and the pointer leaving collapses it. Decided
  overlay, not push: the pane viewport keeps the rail *footprint* (56px) while expanded, so the
  terminal never reflows on hover - only the paint/hit width grows to the full panel (opaque
  `bg.sidebar` fill occludes the panes beneath). The transient expand is pointer state, not a
  config key (Hard rule 1 unaffected); the persisted mode is still just Fixed/Autohide/Hidden.
- 2026-07-13 - **§11 keybindings implemented end-to-end** (feature-completeness pass). Every §11
  chord now works: pane nav on `⌥arrows` (focus) / `⌃⌥arrows` (resize) / `⌥⇧arrows` (swap - new
  `PaneTree::swap`), `⌥Space` cycle-layout (new `PaneTree::cycle_layout` - even cols/rows/main-
  vertical), the tmux **leader** (`[panes] leader`, default `ctrl+a`, previously inert), timeline
  `⌥⌘←/→/0` now global (open the dock so the rewind is visible), terminal `⌘=/-/0` **live font
  size** (new `Renderer::set_font_size`) + `⌘L` clear scrollback, tab `⇧⌘N` new group / `F2` rename
  (inline, `Tab.custom_title`) / `⇧⌘T` reopen (closed-title stack), `⌘/` **cheatsheet** overlay, and
  `⌘F` **find in scrollback** (alacritty `RegexSearch` + `scroll_to_point`, accent-highlighted, a
  bottom find bar). **`⌘↵` run-in-new-pane DEFERRED:** Skelly's palette runs app actions, not shell
  commands, so "run in new pane" has no clean mapping - it needs a shell-command palette mode
  (separate feature), and inventing a bogus behavior would violate Hard rule 5.
- 2026-07-13 - **§10.4 status line COMPLETE - editor MODE + FILETYPE, both from real signals.**
  The guide's §10.4 status line is `cwd · ⑂ branch · MODE · filetype · ●+A −R · shell … Ln, Col`;
  Skelly now shows every segment, each from a real signal (never fabricated):
  - **MODE** (NORMAL/INSERT/REPLACE): modal editors set the terminal cursor shape per mode via
    `DECSCUSR` (block/bar/underline), which alacritty parses - `Terminal::cursor_shape()` +
    `editor_mode(job, shape)`, gated on a known modal-editor foreground process (so a shell's block
    cursor never reads as "NORMAL").
  - **FILETYPE** (rust, python, …): editors name the open file in the OS window title (`OSC 0/2`,
    e.g. `main.rs (…) - NVIM`), which Skelly captured by replacing `VoidListener` with a
    `TitleListener` (`Terminal::title()`); `editor_filetype(job, title)` parses the title for a
    `name.ext` with a known code extension - the editor's ACTUAL file, gated on a modal editor, so
    an unrelated title never shows a bogus filetype. (Config-dependent: the editor must set a title
    with the filename; absent otherwise - honest, not faked.)
  This settles the earlier "filetype needs editor RPC" note: the title is a real, sufficient signal
  for the common case, so no fabrication and no RPC dependency.
- 2026-07-13 - **First-run onboarding (§10.1) BUILT** (was: deferred as a schema+flow slice).
  Shown once on a fresh install (no config file). New `[shell] program` config key (empty = login
  shell) backs the shell picker 1:1 (Hard rule 1); `skelly-term::spawn_shell` honors it;
  `Config::is_first_run()` detects the fresh install. New `ChromeQuad::diamond` + `logo_chrome_quads`
  let the overlay path draw the vertebra mark. The theme picker shows only the two real Ossein
  themes - the guide's third `kana / + 8 presets` card is omitted, not fabricated (Hard rule 5).
  Skip = defaults, Start = apply picks; both write config + respawn the first pane under the chosen
  shell; theme selection previews live via `apply_theme`. Modal bg uses `bg.elevated` (the shared
  overlay-card surface) rather than the mockup's one-off `#232338`.
- 2026-07-13 - **Accent tints composite in sRGB space (§03 `accent.subtle`).** The guide's CSS
  `rgba()` tints (selected-row / active-chip / active-tab / diff washes) composite in **sRGB
  (gamma)** space, but the GPU alpha-blends chrome quads in **linear** space, which over a dark
  surface read noticeably brighter/more saturated than the guide. Decision: chrome tints on a
  known solid background pre-composite on the CPU (`Srgb::over` / `Theme::accent_subtle_on`,
  §03 alpha 0.14 dark / 0.12 light) and fill **opaque**, exactly matching the guide. Applied to
  the workspace chips, active-tab pill, palette selected-row, settings nav/control rows, timeline
  selected row, and the git-dock selection + diff-line washes (all over their `bg.base`/`bg.sidebar`/
  `bg.elevated` backing). `Rgba::to_srgb` recovers `bg.base`'s hex as a composite base. Terminal
  selection keeps GPU alpha (dynamic per-cell background).
- 2026-07-13 - **Pinned-tab grid (§08 #4) + `⇧⌘P`.** Tabs split into the unpinned list + a pinned
  3-up capsule grid (glyph = title's first letter); pin/unpin via the palette "Pin / unpin tab"
  command and `⇧⌘P` (the guide's binding). Grid inset corrected to the sidebar's 13px content
  inset, `gap:6px`, `radius:8px` per the guide markup.
- 2026-07-13 - **Palette footer + empty-state chips.** Palette footer uses key glyphs
  (`↕ navigate  ⏎ run`) with `esc close` right-anchored (§10.8); the "run in new pane" hint is
  omitted (unimplemented, not fabricated). Empty-state hint chips match §10.2 verbatim -
  `⌘K commands · ⌥| split right · ⇧⌘G git diff · ⌘, settings` as capsule pills that flex-wrap
  (was: palette / new tab / split as rounded rects).
- 2026-07-13 - **Tab prompt glyph + running dot (§09/§10.3).** Each sidebar tab now shows a `❯`
  shell-prompt glyph before its label, or a green `●` running dot when the tab has a live
  foreground job (real - from `Terminal::foreground_job_pid`, one flag per tab via the new
  `View.tab_running`). **Token decision:** the guide colors the inactive `❯` with a bright
  non-`§03` color that even varies (teal `#94E2D5` in §09, pink `#F5C2E7` in §10.3); rather than
  cross into a raw/ANSI colour (Hard rule 2), the `❯` is normalized to `accent` for all tabs -
  which matches the active state + the shell prompt in the pane. The dot uses `diff.add` (the
  guide's `#A6E3A1` green). A fixed prompt slot keeps labels aligned whether a tab shows the
  glyph or the dot. The per-tab colour variety (each tab its own hue) is a decorative flourish
  left out (needs a per-tab colour feature).

- 2026-07-13 - **Sidebar group header (§08 #5): the "repo · branch" context.** The guide's
  "SKELLY · MAIN" header above the tab list is real data, not a fabricated group: it renders the
  active repo's `name · branch` (from the cached status context - the cwd basename + the git
  branch), uppercase in `fg.faint` micro, and only inside a git repo (else no header). It sits as
  a `RowKind::Group` row between the command well and the tabs, shifting the tab flow down. This
  is the section-header half of §08 #5; the collapsible-groups *interaction* (collapse/expand,
  multiple named groups, drag-between) is the deferred real feature. With this + the workspace
  chips, the sidebar now reads top-to-bottom exactly like the guide: control strip -> chips ->
  command input -> group header -> tabs -> new tab -> utility bar. Only the pinned grid (#4)
  remains, and it is empty by default (nothing pinned) so it adds no default chrome.

- 2026-07-13 - **Workspace switcher (§08 #2), built as a real feature.** The sidebar now shows
  the `P W +` chips - not fabricated UI, a genuine workspaces feature (Hard rule 5). A `Workspace`
  is a named, isolated tab set; the active one's tabs live in `App.tabs`/`active`, the others are
  stashed (their shells keep running) and swapped in on switch, so the whole existing tab code is
  unchanged (low-churn). Clicking a chip switches; the `+` adds one (first two named
  Personal/Work -> chips P/W, then "Space N"). The chip is the name's first letter (accent-filled +
  bordered when active, else `bg.surface`). `sidebar::View` now bundles the sidebar inputs
  (tab list + chips + rail + control-strip inset) so `build`/`hit` share one shape as the sidebar
  grows (pinned grid + groups next). **Deferred (real follow-ups, not fabrication):** per-workspace
  cwd + theme isolation (today a workspace isolates its *tabs* only); Mod+1…9 switching (chip
  clicks work now); renaming; persistence across restarts.

- 2026-07-13 - **Command palette: mode prefixes + surfaced tabs (§10.8).** The palette's query
  now takes a leading mode prefix: `>` = commands only, `?` = the keybinding help (the command
  list as a reference), `/` = file search (deferred - shows a "coming soon" hint), and **no
  prefix = the universal mode** that lists commands *and* surfaces the open tabs under a "Tabs"
  group (each runs `Action::GotoTab`). Themes were already commands, so "surface themes" needed
  no new entries. The input renders the mode's prompt glyph + the term (query minus the prefix).
  **Interpretation decisions:** (a) the guide's "plain text searches scrollback" is read as the
  *universal* command/tab/theme browse (scrollback search is a separate feature - it needs
  scrollback access + result navigation - deferred); (b) `/` file search is deferred (needs a
  filesystem lister + a defined action, e.g. insert-path); (c) tabs are surfaced by their current
  titles ("Tab N" today; per-tab cwd titling is the blocked feature). The palette result model
  moved from a static `COMMANDS`-index list to a dynamic `Row`/`Entry` list so tabs (and later
  files) compose in.

- 2026-07-13 - **Settings view: real §09 widgets (§10.9).** The settings controls now render the
  guide's actual widgets instead of the placeholder `‹ value ›` text: `Kind::Toggle` -> a 38x22
  toggle switch (accent on / border off, sprung knob), `Kind::Choice` -> a segmented control
  (`bg.inset` container, selected segment `bg.elevated`), `Kind::Range` -> a slider (6px
  `border.subtle` track, `accent` fill, `fg.primary` knob, mono `accent` value readout). CTRL_ROW_H
  30 -> 40 to seat them. **Kept keyboard-driven:** `←/→` still drive edits and the focused row's
  `accent.subtle` fill is the focus cue - mouse hit-testing on the widgets is a deliberate
  follow-up (the widgets are the visual layer; the control model is unchanged, so the round-trip
  contract + Hard rule 1 hold). **Substitutions (not fabrication):** every `Choice` renders as a
  segmented control (all our choices are <=3 options), not the guide's `Select` dropdown or 4-up
  theme *cards* - cards need per-theme swatch colors, a data addition, deferred. The toggle
  knob-on reads `bg.base`; `bg.inset` stands in as the `Srgb` near-black (bg.base is stored linear
  `Rgba`). settings_capture mirrors all three widgets for verification.

- 2026-07-13 - **Command palette: per-command icons + category grouping (§10.8).** The palette
  now renders the guide's grouped list: each command carries a §07 reference-glyph `icon` (drawn
  left of the label, `accent` when selected else `fg.muted`) and a `category`; a small uppercase
  `fg.faint` header (FontRole::Micro) precedes each group. **Ordering decision:** the result list
  is now returned in `COMMANDS` (category) order rather than fuzzy-**score** order, because the
  §10.8 layout is a grouped browse - score-sorting would let a command jump out of its category
  and scatter the headers. Fuzzy matching still *filters* which commands show and *highlights*
  matched chars in accent; only the global relevance ranking is dropped in favour of stable
  grouping. Icons are the same unicode glyphs the guide's mockups use (no bespoke vector icon
  subsystem yet); all verified to render via cosmic-text font fallback (▯▭⤢⊞▤←↓↑→‹›±✕⟲◐⚙⏻).
  Still deferred: the palette mode prefixes (`>`/`/`/`?`) and surfacing tabs/themes/files, which
  need real input-parsing + data sources.

- 2026-07-13 - **Design-fidelity: pixel audit of the sidebar tab item against §09.** A close
  read of the guide (browser + HTML source) against the built sidebar found the active-tab
  styling had drifted from the authoritative §09 "Sidebar tab item" component. Corrected to
  match it exactly: tab **height 30** (was 28); the active **indicator bar is a 3x14 rounded
  `accent` bar seated inside the pill's left padding** (was a full-height 2px rule at the sidebar
  edge); the active pill gains a **1px `accent`@0.28 border** around its `accent`@0.14 fill (drawn
  as a 0.28 rounded rect with the interior reset to `bg.sidebar` before the 0.14 fill, so the edge
  reads stronger than the fill - translucent-over-translucent would only add up); tab labels inset
  past the bar + gap so active and inactive align. **Deferred** (not fabricated): the per-tab `❯`
  prompt glyph the component shows uses a teal `#94E2D5` that is **not** a §03 semantic UI token
  (the table has only accent + status.success/warning/danger/info), so adding it would violate
  Hard rule 2; it also pairs with the blocked content-titling. Left until a teal token is defined
  or titling lands.

- 2026-07-13 - **Sidebar apparatus + per-pane status line (design-fidelity slices 12-14, §08
  anatomy #3, #7, #9).** Several of the guide's window-anatomy pieces were still missing;
  decided how each maps to real Skelly capabilities rather than fabricated UI (Hard rule 5):
  (0) the **command input** (§08 #3, the "Search or run…" well at the sidebar top) is an
  *affordance that opens the existing centered palette overlay* on click (matching the guide's
  "opens the command palette. Focus with Mod+K") - NOT a separate inline text input; it replaces
  the earlier non-guide "SKELLY" text header, and resolves the canonical `bg.surface` token
  (which equals the mockup's `#313244` exactly) with a `border.subtle` ring. In the 56px rail it
  collapses to a centered `⌕` button. (a) the **utility bar** (§08 #7 - the ⚙ ◐ ⟲ ⑂ footer) is a
  *second entry point* to existing commands,
  not new behavior - ⚙ opens settings, ◐ toggles Ossein Dark<->Light, ⟲ toggles the timeline
  dock, ⑂ toggles the git dock. Rendered as the same unicode glyphs the mockup itself uses (no
  bespoke icon subsystem yet), left-clustered per the guide (`padding:0 15px; gap:16px`), full
  panel only - the 56px rail has no room, so it omits the bar (actions stay reachable via keys /
  palette). (b) the **per-pane status line** (§08 #9) shows the data Skelly actually has -
  `cwd · ⑂ branch · shell … Ln, Col`; the guide's editor `mode` (NORMAL/INSERT) + `filetype`
  segments are omitted, not faked, until shell/editor integration exists, and `shell` fills the
  slot they'd occupy. It resolves the canonical `bg.inset`/`Mono` tokens, not the mockup's
  one-off inline `#1A1826`/10.5px (Hard rule 2). Still deferred as genuinely blocked on absent
  features (would be fabrication): the workspace switcher (#2), pinned grid (#4), collapsible
  groups (#5), per-tab cwd titling, and editor mode/filetype - each needs a real feature first.
  (c) the **macOS control strip** (§08 #1, slice 15) is now built: the window uses a
  transparent, full-size-content-view title bar with the title hidden and the traffic-light
  buttons kept visible + functional (the standard native-terminal look, same as Alacritty/
  WezTerm), and app content reserves a 38px `TITLE_STRIP` band at the top (`content_top`, macOS
  only). **CORRECTED 2026-07-13:** the strip is a SIDEBAR concern, not full-width. The guide's
  content zone (panes) is a *sibling* of the sidebar that fills to the window top with just its
  own padding; only the sidebar holds the control strip (top-left, where the lights sit). The
  first cut wrongly reserved `content_top` on the pane viewport + docks too, which created a
  tall empty band across the top (user-reported). Fixed: `viewport_rect` + both docks fill to
  the top; only the sidebar reserves the strip - and it does so as a `top_inset` on its *content*
  while its bg fills the whole column (so the lights sit on the sidebar bg, per the guide).
  Settings keeps its own 38px title strip. The OS-drawn lights still can't appear in captures,
  but the reservation (panes to the top, sidebar strip) is now capture-verified. Linux keeps
  native decorations for v1 (`content_top` = 0); the guide's Linux top-right CSD is a follow-up.

- 2026-07-12 - **Design-fidelity: rounded corners + drop shadows (campaign slice 2).**
  The guide renders every chrome surface as a rounded, shadowed card, but the two
  surface *kinds* are treated differently (mockup §07 hero vs the palette/modal
  frames), so decided here how each maps to the renderer: (a) **floating overlays** -
  the command palette and the process-close confirm modal - are centered *cards*: they
  get rounded `lg` (10px) corners, a soft `e4` drop shadow, a 1px rounded `border.strong`
  ring, and rounded `md` (8px) `accent.subtle` selected-row pills (guide's rounded list
  rows). (b) **Right docks** (git diff, timeline) are **flush slide-overs**, not floating
  cards: they stay pinned to the right/top/bottom window edges with **square** corners
  (matching the §07 hero, where the dock is `right:0; top:0; bottom:0` with a thin left
  divider + a `transparent->surface` handle gradient), and get a soft shadow cast
  **leftward onto the terminal** from their left edge - the "slides over the terminal"
  depth - implemented as a short quadratic-falloff gradient strip, not the box `e*`
  shadow (which would need an opaque full-dock fill; the dock is a composite of
  sub-surfaces, not one flat `bg.surface`). Renderer mechanism: the shared instanced quad
  gained a `[radius, blur]` param driving an SDF rounded-box + soft-shadow in the shader,
  with a zero-param flat fast-path so every existing sharp fill (cells, dividers, cursor,
  sidebar, settings, dock rows) is byte-identical. The full-window **settings** view is
  *not* a floating card (it fills the content area, Hard rule 4) - its inner control
  cards/rows are a later slice. On dark themes a black shadow over the near-black terminal
  is honestly faint; the 1px divider carries dock separation there and the shadow reads on
  light. Deferred: proportional chrome (slice 3), the vertebra logo (slice 4), motion
  (slice 5), tooltip/chip `e2`/`e1` elevation, and routing `bg.surface`/`bg.inset` per
  dock sub-region.
- 2026-07-12 - **Empty state + never-quit close cascade (M5 edge states "Close last
  pane" + guide §10.2).** Two settled behaviors. **(1) Close cascade:** closing the only
  pane in a tab (`⌥w`) closes the whole tab; closing the only tab does **not** quit the app
  - it resets that tab to a fresh, pristine one (the old tab drops, so its shells are
  killed) and shows the empty state. So the window always holds >=1 tab. **(2) Empty
  state:** a fresh tab (no command run yet, single pane) paints a faint `skelly` wordmark +
  three hint chips (`⌘K palette`, `⌘T new tab`, `⌥| split`, each a subtle `bg.elevated`
  pill) centered over its blank terminal, in UI tokens (Hard rule 2). It shows on launch
  and on every new/reset tab, and clears the first time the user runs a command (submits
  with Enter) or splits - a per-tab `activated` flag. Decided here (the guide's §10.2 is a
  static mockup): the mark is a **wordmark** (the bespoke big-logo waits on the fixed-metric
  cell renderer, like the other Nerd-glyph placeholders); the content is **baked into the
  pane grid** rather than a separate render layer (a fresh grid is blank, so it rides the
  existing pane text + background passes with no new render path); and the clear is an
  **instant hide, not an animated fade** (no animation loop yet - a follow-up). The chips'
  keys use this app's real chords (`⌥|` split, not the guide's leader). _(was: the guide
  specifies the empty state + close-last-pane/tab edge cases but not the mechanism, the
  freshness trigger, or how they map onto this app's tab/pane model.)_
- 2026-07-12 - **Shell-exit overlay (M5 edge state "Shell exits / crashes").** When a
  pane's shell ends (`exit`, Ctrl-D, a kill, or a crash), the pane does **not** silently
  die: `skelly-term` reports the exit (its reader thread reaps the child and records an
  `ExitStatus`; the `Terminal` also holds a `clone_killer()` so drop kills the shell and
  the thread reaps it - fixing a latent leak). The renderer draws a translucent `bg.base`
  scrim (72% alpha) over the pane's **preserved** grid - the scrollback stays faintly
  visible - plus a centered message: `shell exited` / the exit code (green) or signal
  (red) / a `↵ restart   ⌥w close` hint (accent chords, `fg.muted` words), drawn as a
  layer above the terminal text but beneath every dock/overlay (Hard rule 4). Decided here
  (the guide's §12 line leaves the keys/mechanism implied): **`↵` restarts** the shell in
  place (drop the exited terminal, `sync_layout` respawns a fresh one for the same
  still-in-tree pane; scrollback makes way for a new prompt), and while a focused pane is
  dead it swallows all other input; **close is `⌥w`** (this app's pane-close chord - the
  design's `⌘W` is tab-close here, per the 2026-07-11 pane-keybindings decision). A lone
  exited shell shows the overlay rather than quitting the app (the "close last pane ->
  empty state" behavior is its own M5 slice). Fork-on-edit / editable rewind stays out of
  scope. _(was: the guide specifies the edge state but not the restart/close keys or the
  read-only mechanism for this app's tab/pane model.)_
- 2026-07-12 - **Session timeline + non-destructive rewind (v1 scope).** The three
  "Confirm first" timeline questions are settled for v1: **(1) what the timeline
  records** - an in-session **event log** Skelly records itself: a System event at
  session start, and the Human git events it witnesses through the diff dock
  (commits, and stage / unstage / stage-all). Each event may carry a **restore
  target** (a commit SHA); only commit events are restorable, because the
  non-destructive mechanism can only check out a real git object. The model has an
  `Agent` actor and a `Timeline::record` API so agent events slot in later, but the
  transport (how external AI actions arrive) is the still-open AI-actions contract -
  a future additive hook, not a shell heuristic. **(2) rewind + edit** - v1 rewind
  is **read-only inspection**: selecting a restorable event runs `git worktree add
  --detach <sha>` into a Skelly-owned shadow worktree (a temp dir), so HEAD, the
  branch, and every ref are untouched (Hard rule 3, adversarially tested); the dock
  shows "viewing state at <event>" and the past state's files; Skelly never
  auto-forks or repoints panes. `⌥⌘0` / "Return to now" runs `git worktree remove
  --force` and snaps back to HEAD. Fork-on-edit is deferred. **(3) persist scope** -
  **layout only** (restore tabs + pinned on launch), never re-run processes, per the
  guide's `[session] persist` comment; the launch-restore implementation is a
  separate follow-up. The timeline opens as a right dock with `⇧⌘H` (Hard rule 4 -
  mutually exclusive with the git dock, only one right-dock surface at a time);
  `↑/↓` select an event, `⌥⌘←`/`⌥⌘→` step, `⌥⌘0` return to now, `Esc` closes. _(was:
  the timeline AI-actions contract, rewind+edit, and persist-scope open questions.)_
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
