//! The persistent left sidebar: the vertical tab list (AGENTS Hard rule 4 - chrome
//! that layers over the always-present pane tree, never a route). This module is pure
//! state + layout: it owns the show/hide flag and turns the tab list into a *proportional*
//! display list - decorative [`ChromeQuad`]s (the surface, the active tab's rounded
//! `accent.subtle` pill + `accent` bar, the right-edge divider) plus positioned
//! [`ProseLabel`]s in the guide's fonts (§08) - plus the pixel -> action hit-test the
//! binary uses for clicks. The binary owns toggling it, geometry, and switching tabs.
//!
//! Two display modes (design §08 "Sidebar modes"): the full-width panel listing tabs
//! (active highlighted) with a "+ New tab" action, and the slim 56px icon rail with
//! compact centered tab numbers. `⌘B` shows/hides; `⇧⌘B` cycles full <-> rail. The
//! chosen mode persists to `config.sidebar.mode` (Hard rule 1). Also built here: the workspace
//! switcher chips (§08 #2) at the top, the command-input well (§08 #3, opens the palette), the
//! pinned-tab grid (§08 #4), the collapsible group headers (§08 #5 - a chevron + mono name +
//! member count; a collapsed group hides its member rows), and the bottom-anchored utility bar
//! (§08 #7 - the ⚙ settings / ◐ theme / ⟲ timeline / ⑂ git toggles).

use skelly_config::SidebarMode;
use skelly_render::{
    logo_chrome_quads, ChromeQuad, FontRole, ProseLabel, PxRect, Srgb, TextMeasure, Theme,
};

/// Layout constants in **logical** px (multiplied by the DPI scale when placed). Tuned to
/// the guide's §08 sidebar: a compact group header, comfortable 13px `label` tab rows, and
/// a matching "+ New tab" action.
const PAD_TOP: f32 = 10.0;
/// Height (logical px) of the brand-lockup row seated *below* the macOS traffic-light strip -
/// the mark + "skelly" wordmark sit under the lights (left-aligned), not beside them.
const BRAND_BLOCK: f32 = 30.0;
/// Gap between the vertebra mark and the "skelly" wordmark.
const LOGO_GAP: f32 = 8.0;
/// Workspace-switcher chips (design §08 #2): 26px rounded tiles, `gap:7px`, inset `13px`.
const CHIP_SIZE: f32 = 26.0;
const CHIP_GAP: f32 = 7.0;
const CHIP_RADIUS: f32 = 7.0;
const CHIP_INSET: f32 = 13.0;
/// Gap below the workspace-chip row before the command well (the guide's `padding: … 10px`).
const CHIP_BLOCK_GAP: f32 = 10.0;
/// Height of the command-input well (design §08 #3), matching the guide's 30px search field.
const CMD_H: f32 = 30.0;
/// Gap below the command well before the tab list (the guide's `padding: … 12px`).
const CMD_GAP: f32 = 12.0;
/// Horizontal inset of the command well from the sidebar edges (content pad).
const CMD_INSET: f32 = 12.0;
/// Command-well corner radius (the guide's `md` 8px).
const CMD_RADIUS: f32 = 8.0;
/// The command well's search glyph + placeholder (design §08 #3).
const CMD_ICON: &str = "\u{2315}";
const CMD_PLACEHOLDER: &str = "Search or run\u{2026}";
/// Pinned-tab grid (design §08 #4): a "PINNED" label over a 3-column grid of square tiles.
/// Its `13px` horizontal inset + `6px` gap + `8px` tile radius are the guide's `padding:0 13px`,
/// grid `gap:6px`, and `border-radius:8px`; the label is 9px with a 7px margin below (~16px).
const PINNED_LABEL_H: f32 = 16.0;
const PINNED_INSET: f32 = 13.0;
const PINNED_GRID_GAP: f32 = 6.0;
const PINNED_RADIUS: f32 = 8.0;
const PINNED_COLS: usize = 3;
/// Height of a group header row (design §08 #5: a collapse chevron + mono name + member count),
/// with breathing room. The mockup's `padding:4px 6px` inside the `padding:0 9px` list.
const GROUP_H: f32 = 22.0;
/// Left inset of a group header's chevron from the sidebar edge (list pad + header pad).
const GROUP_INSET: f32 = 15.0;
/// Width reserved for the header chevron + its gap before the name (the mockup's `gap:6px`).
const GROUP_CHEVRON_SLOT: f32 = 14.0;
/// The group-header chevrons (design §08 #5): `▾` expanded (accent), `▸` collapsed (muted).
const CHEVRON_OPEN: &str = "\u{25be}";
const CHEVRON_CLOSED: &str = "\u{25b8}";
/// Height of an overflow indicator row (`↑ N more` / `↓ N more`).
const IND_H: f32 = 16.0;
/// Height of a tab row (and the "+ New tab" action), per the §09 "Sidebar tab item" (Height 30).
const TAB_H: f32 = 30.0;
/// Bottom padding beneath the new-tab action.
const PAD_BOTTOM: f32 = 10.0;
/// Height of the bottom-anchored utility bar (design §08 #7 - the icon-only settings /
/// theme / timeline / git toggles), matching the guide's 40px footer.
const UTIL_H: f32 = 40.0;
/// Horizontal inset (logical px) of a full-panel label from the sidebar edge (content pad).
const LABEL_INSET: f32 = 12.0;
/// Horizontal inset of the tab pill from the sidebar edges (the guide's tab container
/// `padding:0 9px`, consistent across the window-anatomy + empty-state mockups).
const PILL_INSET: f32 = 9.0;
/// Vertical gap between tab pills (the guide's per-tab `margin-bottom:3px`).
const TAB_GAP_V: f32 = 3.0;
/// The active tab's `accent` indicator bar (design §09 "Sidebar tab item": a 3x14 rounded bar
/// seated inside the pill, not a full-height rule at the sidebar edge).
const BAR_W: f32 = 3.0;
const BAR_H: f32 = 14.0;
const BAR_RADIUS: f32 = 2.0;
/// The tab pill's internal left padding + the gap after the indicator bar (§09 `padding:0 10px`
/// + `gap:8px`); the label is inset past both so active and inactive tabs align.
const TAB_PAD_X: f32 = 10.0;
const TAB_GAP: f32 = 8.0;
/// The tab prefix glyph (design §09/§10.3): a shell-prompt `❯` in `accent`, or a `●` running
/// dot when the tab has a live foreground job. A fixed slot keeps the labels aligned.
const TAB_PROMPT: &str = "\u{276f}";
const TAB_PROMPT_SLOT: f32 = 9.0;
/// Diameter of the running-job status dot (the guide's 6px `●`).
const TAB_DOT: f32 = 6.0;
/// Corner radius (logical px) of the active-tab pill (the guide's `sm` radius: tab items).
const PILL_RADIUS: f32 = 6.0;

/// What a click on a sidebar row targets.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub(crate) enum Hit {
    /// Switch to the workspace at this 0-based index (a chip, design §08 #2).
    Workspace(usize),
    /// Add a new workspace (the `+` chip).
    AddWorkspace,
    /// Switch to the pinned tab at this 0-based grid position (design §08 #4).
    Pinned(usize),
    /// Open the command palette (the command-input well, design §08 #3).
    CommandInput,
    /// Switch to the tab at this 0-based index.
    Tab(usize),
    /// Collapse / expand the group at this 0-based index (a group header, design §08 #5).
    GroupHeader(usize),
    /// Open a new tab.
    NewTab,
    /// Trigger a utility-bar toggle (design §08 #7).
    Util(UtilAction),
}

/// The inputs the sidebar lays out from: the tab list, the workspace chips, the display mode,
/// and the control-strip inset. Bundled so [`build`] + [`hit`] share one shape as the sidebar
/// grows (chips now; pinned grid + groups later).
pub(crate) struct View<'a> {
    /// Number of tabs in the active workspace.
    pub(crate) tab_count: usize,
    /// Index of the active tab.
    pub(crate) active_tab: usize,
    /// One chip glyph per workspace (its name's first letter); a trailing `+` is always added.
    pub(crate) chips: &'a [char],
    /// Index of the active workspace (highlighted chip).
    pub(crate) active_chip: usize,
    /// One glyph per pinned tab for the 3-up pinned grid (design §08 #4); empty when nothing is
    /// pinned (the grid is then hidden).
    pub(crate) pinned: &'a [char],
    /// The grid position of the active tab when it is pinned (highlights that tile).
    pub(crate) active_pinned: Option<usize>,
    /// The collapsible tab groups (design §08 #5), each spanning a contiguous range of the
    /// ordered tab list. Empty when no groups exist (the list is then a flat set of ungrouped
    /// tabs). Ungrouped tabs occupy the positions before the first group's `start`.
    pub(crate) groups: &'a [GroupSpan<'a>],
    /// Whether each tab has a live foreground job (a `●` running dot instead of the `❯` prompt),
    /// one flag per tab. Empty or short-of-`tab_count` means "not running".
    pub(crate) tab_running: &'a [bool],
    /// Each tab's title (design §10.3), one per tab; a missing entry falls back to `Tab N`.
    pub(crate) tab_titles: &'a [String],
    /// Whether the sidebar is the slim icon rail.
    pub(crate) rail: bool,
    /// The macOS control-strip inset in **logical** px (0 elsewhere); content clears it.
    pub(crate) top_inset: f32,
}

/// One collapsible group's span over the ordered tab list (design §08 #5): the header name +
/// collapsed flag and the `[start, start + len)` range of member tab positions.
pub(crate) struct GroupSpan<'a> {
    /// The header name (e.g. `skelly · main`), rendered in a mono micro label.
    pub(crate) name: &'a str,
    /// Whether the group is collapsed (its member tab rows are hidden - header only).
    pub(crate) collapsed: bool,
    /// The first tab position belonging to this group in the ordered list.
    pub(crate) start: usize,
    /// The number of member tabs.
    pub(crate) len: usize,
}

/// A utility-bar icon's action (design §08 #7: "Settings, theme, session timeline, git diff
/// toggles"). Each maps 1:1 to an existing command the binary already exposes, so the bar is
/// a second entry point, not new behavior. Left-to-right order matches the guide (⚙ ◐ ⟲ ⑂).
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub(crate) enum UtilAction {
    /// Open the settings view (`⌘,`).
    Settings,
    /// Toggle the UI theme (Ossein Dark <-> Light).
    Theme,
    /// Toggle the session-timeline dock.
    Timeline,
    /// Toggle the git-diff dock.
    Git,
}

/// The utility-bar actions in the guide's left-to-right order, paired with their glyphs
/// (⚙ settings · ◐ theme · ⟲ timeline · ⑂ git). Rendered as glyph labels - the same unicode
/// the mockup itself uses - so no bespoke icon subsystem is needed yet.
const UTIL_ICONS: [(UtilAction, &str); 4] = [
    (UtilAction::Settings, "\u{2699}"),
    (UtilAction::Theme, "\u{25D0}"),
    (UtilAction::Timeline, "\u{27F2}"),
    (UtilAction::Git, "\u{2442}"),
];
/// Horizontal padding of the full-panel utility row from the sidebar edges; the four icons
/// spread evenly across the inner width between these pads.
const UTIL_PAD_X: f32 = 15.0;

/// The persistent left sidebar's state: its display mode (a mirror of
/// `config.sidebar.mode`, the source of truth per Hard rule 1) plus the visible mode to
/// restore when `⌘B` recalls a hidden sidebar. Groups and pinning are later slices.
pub(crate) struct Sidebar {
    /// The current display mode. `Fixed` = full panel, `Autohide` = the slim icon rail,
    /// `Hidden` = collapsed. Kept in sync with `config.sidebar.mode` by the binary.
    mode: SidebarMode,
    /// The visible mode (`Fixed` or `Autohide`, never `Hidden`) to return to when the
    /// sidebar is recalled from hidden - so hide/show preserves the rail-vs-full choice.
    restore: SidebarMode,
}

impl Sidebar {
    /// Start from the configured mode.
    pub(crate) fn new(mode: SidebarMode) -> Self {
        Self {
            mode,
            restore: visible_or(mode, SidebarMode::Fixed),
        }
    }

    /// The current display mode (mirrors `config.sidebar.mode`).
    pub(crate) fn mode(&self) -> SidebarMode {
        self.mode
    }

    /// Whether the sidebar occupies any width (the full panel or the rail).
    pub(crate) fn visible(&self) -> bool {
        !matches!(self.mode, SidebarMode::Hidden)
    }

    /// Whether the sidebar is the slim icon rail (`Autohide`).
    pub(crate) fn is_rail(&self) -> bool {
        matches!(self.mode, SidebarMode::Autohide)
    }

    /// `⌘B` - show or hide. Hiding remembers the visible mode; showing restores it.
    pub(crate) fn toggle(&mut self) {
        if matches!(self.mode, SidebarMode::Hidden) {
            self.mode = self.restore;
        } else {
            self.restore = self.mode;
            self.mode = SidebarMode::Hidden;
        }
    }

    /// `⇧⌘B` - cycle between the full panel and the slim icon rail (design §08). Always
    /// leaves the sidebar visible; from hidden it comes back as the full panel.
    pub(crate) fn cycle_rail(&mut self) {
        self.mode = match self.mode {
            SidebarMode::Fixed => SidebarMode::Autohide,
            _ => SidebarMode::Fixed,
        };
        self.restore = self.mode;
    }

    /// Adopt a mode chosen elsewhere (the settings view writing `sidebar.mode`), keeping
    /// the recall target in step.
    pub(crate) fn set_mode(&mut self, mode: SidebarMode) {
        self.mode = mode;
        self.restore = visible_or(mode, self.restore);
    }
}

/// `mode` if it is a visible mode, else `fallback` - the recall target is never `Hidden`.
fn visible_or(mode: SidebarMode, fallback: SidebarMode) -> SidebarMode {
    if matches!(mode, SidebarMode::Hidden) {
        fallback
    } else {
        mode
    }
}

/// What one laid-out row is, so [`build`] and [`hit`] agree on where every tab sits.
#[derive(Clone, Copy)]
enum RowKind {
    /// The command-input well (design §08 #3), which opens the palette.
    Command,
    /// The pinned-tab grid (design §08 #4), drawn only when tabs are pinned.
    Pinned,
    /// A collapsible group header (design §08 #5) for the group at this 0-based index.
    GroupHeader(usize),
    /// A tab at this 0-based index.
    Tab(usize),
    /// The "N tabs hidden above" indicator (drawn only when `> 0`).
    OverflowUp(usize),
    /// The "N tabs hidden below" indicator (drawn only when `> 0`).
    OverflowDown(usize),
    /// The "+ New tab" action.
    NewTab,
}

/// A laid-out row: its top edge and height in **logical** px, and what it is.
#[derive(Clone, Copy)]
struct Row {
    top: f32,
    height: f32,
    kind: RowKind,
}

/// Lay the sidebar out top-to-bottom in **logical** px for `count` tabs with `active`
/// selected, windowing the tab list into `panel_h` (design §12 "Many tabs overflow"): the
/// header stays pinned, the active tab auto-scrolls into view, and the "+ New tab" action
/// follows the visible window. Measurer-free (row bands depend only on fixed heights), so
/// [`hit`] and [`build`] share the exact same geometry.
#[allow(
    clippy::cast_precision_loss,
    clippy::cast_possible_truncation,
    clippy::cast_sign_loss,
    reason = "the tab-row capacity is a small, non-negative count"
)]
fn rows_layout(
    count: usize,
    active: usize,
    panel_h: f32,
    top_inset: f32,
    groups: &[GroupSpan],
    pinned_h: f32,
) -> Vec<Row> {
    // Reserve the bottom-anchored utility bar so the top-down flow stops above it.
    let flow_h = panel_h - UTIL_H;
    let mut rows = Vec::new();
    // Start below the control strip (macOS traffic lights); zero elsewhere. The sidebar bg
    // still fills the strip - only the content clears it.
    let mut y = top_inset + PAD_TOP;
    rows.push(Row {
        top: y,
        height: CMD_H,
        kind: RowKind::Command,
    });
    y += CMD_H + CMD_GAP;

    // The pinned-tab grid (design §08 #4), above the tab list, when anything is pinned.
    if pinned_h > 0.0 {
        rows.push(Row {
            top: y,
            height: pinned_h,
            kind: RowKind::Pinned,
        });
        y += pinned_h;
    }

    if groups.is_empty() {
        // The common case: a flat tab list windowed into the panel (design §12 overflow), the
        // header pinned + the active tab auto-scrolled into view + "+ New tab" following.
        // Capacity for tab rows, reserving both overflow-indicator slots + the new-tab action.
        let reserved_below = IND_H + TAB_H + PAD_BOTTOM;
        let avail = flow_h - y - IND_H - reserved_below;
        let capacity = (avail / (TAB_H + TAB_GAP_V)).floor().max(1.0) as usize;
        let visible = count.min(capacity);
        let first = if count <= visible {
            0
        } else {
            active.saturating_sub(visible - 1).min(count - visible)
        };
        let more_above = first;
        let more_below = count - first - visible;

        // The overflow indicators only occupy a row when there is something hidden, so a short
        // tab list sits flush under the command well (no reserved gap).
        if more_above > 0 {
            rows.push(Row {
                top: y,
                height: IND_H,
                kind: RowKind::OverflowUp(more_above),
            });
            y += IND_H;
        }
        for index in first..first + visible {
            rows.push(Row {
                top: y,
                height: TAB_H,
                kind: RowKind::Tab(index),
            });
            y += TAB_H + TAB_GAP_V;
        }
        if more_below > 0 {
            rows.push(Row {
                top: y,
                height: IND_H,
                kind: RowKind::OverflowDown(more_below),
            });
            y += IND_H;
        }
    } else {
        // Grouped: the ungrouped tabs (positions before the first group) render in full, then
        // each group as a header + its member rows (design §08 #5); a collapsed group is just
        // its header. Group organization implies modest counts, so the grouped view is not
        // windowed.
        let ungrouped_end = groups[0].start;
        for index in 0..ungrouped_end {
            rows.push(Row {
                top: y,
                height: TAB_H,
                kind: RowKind::Tab(index),
            });
            y += TAB_H + TAB_GAP_V;
        }
        for (gi, group) in groups.iter().enumerate() {
            rows.push(Row {
                top: y,
                height: GROUP_H,
                kind: RowKind::GroupHeader(gi),
            });
            y += GROUP_H;
            if !group.collapsed {
                for index in group.start..group.start + group.len {
                    rows.push(Row {
                        top: y,
                        height: TAB_H,
                        kind: RowKind::Tab(index),
                    });
                    y += TAB_H + TAB_GAP_V;
                }
            }
        }
    }
    rows.push(Row {
        top: y,
        height: TAB_H,
        kind: RowKind::NewTab,
    });
    rows
}

/// Map a click at physical `(px, py)` (relative to the surface top-left) to a sidebar action
/// for `view`, filling `panel` (physical px) at DPI `scale`. The workspace chips are tested
/// first, then the bottom utility bar (full panel), then the tab rows + new-tab action; the
/// command well; the spacers/overflow indicators map to nothing. Shares [`rows_layout`] +
/// [`utility_slots`] + [`chip_slots`] with [`build`] so a click lands on exactly what is drawn.
pub(crate) fn hit(view: &View, panel: PxRect, scale: f32, px: f32, py: f32) -> Option<Hit> {
    // Workspace chips (full panel only).
    if !view.rail {
        for (i, slot) in chip_slots(view, panel, scale).into_iter().enumerate() {
            if px >= slot.x && px < slot.x + slot.w && py >= slot.y && py < slot.y + slot.h {
                return Some(if i < view.chips.len() {
                    Hit::Workspace(i)
                } else {
                    Hit::AddWorkspace
                });
            }
        }
    }
    // The utility bar occupies the bottom `UTIL_H` band (full panel only).
    let util_top = panel.y + panel.h - UTIL_H * scale;
    if !view.rail && py >= util_top {
        return utility_slots(panel, scale)
            .into_iter()
            .find(|(_, slot)| px >= slot.x && px < slot.x + slot.w)
            .map(|(action, _)| Hit::Util(action));
    }
    let top_inset = view.top_inset + brand_block_h(view) + chips_block_h(view);
    let y_logical = (py - panel.y) / scale;
    let layout_groups: &[GroupSpan] = if view.rail { &[] } else { view.groups };
    for row in rows_layout(
        view.tab_count,
        view.active_tab,
        panel.h / scale,
        top_inset,
        layout_groups,
        pinned_block_h(view, panel, scale),
    ) {
        if y_logical >= row.top && y_logical < row.top + row.height {
            return match row.kind {
                RowKind::Command => Some(Hit::CommandInput),
                RowKind::Pinned => {
                    // Map the click to the pinned tile under it (`block_top` is the row top).
                    let block_top = panel.y + row.top * scale;
                    pinned_slots(view.pinned.len(), panel, block_top, scale)
                        .into_iter()
                        .position(|s| px >= s.x && px < s.x + s.w && py >= s.y && py < s.y + s.h)
                        .map(Hit::Pinned)
                }
                RowKind::Tab(index) => Some(Hit::Tab(index)),
                RowKind::GroupHeader(gi) => Some(Hit::GroupHeader(gi)),
                RowKind::NewTab => Some(Hit::NewTab),
                _ => None,
            };
        }
    }
    None
}

/// The full-panel utility bar's per-icon hit slots (physical px), each returning its action +
/// click box: the four icons are spread evenly across the footer, each occupying (and centered
/// in) an equal slot spanning the sidebar's padded inner width, rather than left-clustered.
/// Shared by [`hit`] and [`build`] so the drawn glyph and its click target coincide.
#[allow(
    clippy::cast_precision_loss,
    reason = "the icon count/index is a tiny fixed range (0..4)"
)]
fn utility_slots(panel: PxRect, scale: f32) -> Vec<(UtilAction, PxRect)> {
    let top = panel.y + panel.h - UTIL_H * scale;
    let h = UTIL_H * scale;
    let pad = UTIL_PAD_X * scale;
    let inner = (panel.w - 2.0 * pad).max(0.0);
    let slot_w = inner / UTIL_ICONS.len() as f32;
    UTIL_ICONS
        .iter()
        .enumerate()
        .map(|(i, (action, _))| {
            (
                *action,
                PxRect {
                    x: panel.x + pad + i as f32 * slot_w,
                    y: top,
                    w: slot_w,
                    h,
                },
            )
        })
        .collect()
}

/// The sidebar's finished proportional display list plus the panel it clips to.
pub(crate) struct Paint {
    /// The sidebar rectangle (`x = 0`, full height), physical px.
    pub(crate) panel: PxRect,
    /// The decorative quads, in draw order.
    pub(crate) quads: Vec<ChromeQuad>,
    /// The positioned proportional text labels.
    pub(crate) labels: Vec<ProseLabel>,
}

/// Build the sidebar's display list for `view`, filling `panel` (physical px) at DPI `scale`,
/// in the guide's fonts + `theme` tokens. The full panel shows workspace chips, the command
/// well, the tab list, and the utility bar; the slim rail shows compact tab numbers. Shares
/// [`rows_layout`] with [`hit`] so clicks land on exactly what is drawn.
pub(crate) fn build(
    view: &View,
    panel: PxRect,
    scale: f32,
    theme: &Theme,
    measure: &mut TextMeasure,
) -> Paint {
    let mut quads = vec![ChromeQuad::fill(panel, theme.bg_sidebar)];
    let mut labels = Vec::new();

    // The brand lockup (design §02): the vertebra mark + "skelly" wordmark, seated in its own row
    // just *below* the macOS traffic-light strip (left-aligned under the lights, not beside them).
    // Full panel only, and only where a strip is reserved (macOS) - the slim rail and non-macOS
    // builds have no room for it.
    if brand_block_h(view) > 0.0 {
        let block_top = panel.y + view.top_inset * scale;
        let block_h = BRAND_BLOCK * scale;
        // The vertebra mark is a tall vertical spine; keep it close to the 12px wordmark's height.
        let mark = 16.0 * scale;
        let mark_x = panel.x + CHIP_INSET * scale;
        let mark_y = block_top + (block_h - mark) * 0.5;
        quads.extend(logo_chrome_quads(mark_x, mark_y, mark, theme, 1.0));
        let line = measure.line_height(FontRole::Mono);
        labels.push(ProseLabel {
            text: "skelly".to_owned(),
            x: mark_x + mark + LOGO_GAP * scale,
            y: block_top + (block_h - line) * 0.5,
            role: FontRole::Mono,
            color: theme.fg_primary,
            weight: None,
            max_w: f32::MAX,
        });
    }

    // The workspace chips sit below the control strip; the tab flow starts below them.
    let chips_block = chips_block_h(view);
    if !view.rail {
        push_chips(&mut quads, &mut labels, view, panel, scale, theme, measure);
    }

    let ctx = RowCtx {
        panel,
        active: view.active_tab,
        rail: view.rail,
        groups: view.groups,
        tab_running: view.tab_running,
        tab_titles: view.tab_titles,
        pinned: view.pinned,
        active_pinned: view.active_pinned,
        scale,
        theme,
    };
    // The slim rail lays the tab list out flat (numbered), so groups only shape the full panel.
    let layout_groups: &[GroupSpan] = if view.rail { &[] } else { view.groups };
    for row in rows_layout(
        view.tab_count,
        view.active_tab,
        panel.h / scale,
        view.top_inset + brand_block_h(view) + chips_block,
        layout_groups,
        pinned_block_h(view, panel, scale),
    ) {
        push_row(&mut quads, &mut labels, row, &ctx, measure);
    }

    // The bottom-anchored utility bar (design §08 #7) - full panel only; the slim rail has no
    // room for it (its actions stay reachable via keys / the palette).
    if !view.rail {
        push_utility_bar(&mut quads, &mut labels, panel, scale, theme, measure);
    }

    // The right-edge divider separating the sidebar from the pane area (drawn last).
    let stroke = scale.max(1.0);
    quads.push(ChromeQuad::fill(
        PxRect {
            x: panel.x + panel.w - stroke,
            y: panel.y,
            w: stroke,
            h: panel.h,
        },
        theme.border,
    ));

    Paint {
        panel,
        quads,
        labels,
    }
}

/// The shared per-row context for [`push_row`] - everything but the row itself.
struct RowCtx<'a> {
    panel: PxRect,
    active: usize,
    rail: bool,
    groups: &'a [GroupSpan<'a>],
    tab_running: &'a [bool],
    tab_titles: &'a [String],
    pinned: &'a [char],
    active_pinned: Option<usize>,
    scale: f32,
    theme: &'a Theme,
}

/// Render one laid-out row into `quads` + `labels`: the group header, a tab (with its active
/// marks + prompt/dot + label), an overflow indicator, or the new-tab action.
#[allow(clippy::too_many_lines, reason = "one straight-line per-row renderer")]
fn push_row(
    quads: &mut Vec<ChromeQuad>,
    labels: &mut Vec<ProseLabel>,
    row: Row,
    ctx: &RowCtx,
    measure: &mut TextMeasure,
) {
    let top = ctx.panel.y + row.top * ctx.scale;
    let height = row.height * ctx.scale;
    let place = |labels: &mut Vec<ProseLabel>,
                 measure: &mut TextMeasure,
                 text: &str,
                 role: FontRole,
                 color: Srgb| {
        push_label(
            labels, measure, text, role, color, ctx.panel, top, height, ctx.rail, ctx.scale,
        );
    };
    match row.kind {
        RowKind::Command => push_command_well(quads, labels, top, height, ctx, measure),
        RowKind::Pinned => {
            if !ctx.rail {
                push_pinned(
                    quads,
                    labels,
                    ctx.pinned,
                    ctx.active_pinned,
                    ctx.panel,
                    top,
                    ctx.scale,
                    ctx.theme,
                    measure,
                );
            }
        }
        RowKind::GroupHeader(gi) => {
            // A collapsible group header (design §08 #5): a chevron (▾ open / ▸ collapsed), the
            // mono name, and a right-aligned member count. Full panel only (the rail lays the
            // tab list out flat). Grouped tabs never reach the rail path, so this is defensive.
            if let Some(group) = ctx.groups.get(gi) {
                if !ctx.rail {
                    let inset = GROUP_INSET * ctx.scale;
                    let x = ctx.panel.x + inset;
                    let (chevron, chev_color) = if group.collapsed {
                        (CHEVRON_CLOSED, ctx.theme.fg_muted)
                    } else {
                        (CHEVRON_OPEN, ctx.theme.accent)
                    };
                    let chev_line = measure.line_height(FontRole::Caption);
                    labels.push(ProseLabel {
                        text: chevron.to_owned(),
                        x,
                        y: top + (height - chev_line) * 0.5,
                        role: FontRole::Caption,
                        color: chev_color,
                        weight: None,
                        max_w: f32::MAX,
                    });
                    // The count, right-aligned; the name fills the space between chevron + count.
                    let count = group.len.to_string();
                    let count_w = measure.width(&count, FontRole::Micro, None);
                    let count_x = ctx.panel.x + ctx.panel.w - inset - count_w;
                    let name_x = x + GROUP_CHEVRON_SLOT * ctx.scale;
                    let name_line = measure.line_height(FontRole::Micro);
                    labels.push(ProseLabel {
                        text: group.name.to_owned(),
                        x: name_x,
                        y: top + (height - name_line) * 0.5,
                        role: FontRole::Micro,
                        color: ctx.theme.fg_muted,
                        weight: None,
                        max_w: (count_x - name_x - GROUP_CHEVRON_SLOT * ctx.scale).max(1.0),
                    });
                    labels.push(ProseLabel {
                        text: count,
                        x: count_x,
                        y: top + (height - name_line) * 0.5,
                        role: FontRole::Micro,
                        color: ctx.theme.fg_faint,
                        weight: None,
                        max_w: f32::MAX,
                    });
                }
            }
        }
        RowKind::Tab(index) => {
            let is_active = index == ctx.active;
            if is_active {
                push_active_marks(quads, ctx.panel, top, height, ctx.scale, ctx.theme);
            }
            let color = if is_active {
                ctx.theme.fg_primary
            } else {
                ctx.theme.fg_secondary
            };
            if ctx.rail {
                place(
                    labels,
                    measure,
                    &(index + 1).to_string(),
                    FontRole::Label,
                    color,
                );
            } else {
                // After the pill padding + indicator bar + gap: a `●` running dot (design
                // §10.3) when the tab has a live job, else a `❯` shell-prompt glyph in accent.
                let prefix_x = ctx.panel.x + (PILL_INSET + TAB_PAD_X + BAR_W + TAB_GAP) * ctx.scale;
                if ctx.tab_running.get(index).copied().unwrap_or(false) {
                    let dot = TAB_DOT * ctx.scale;
                    quads.push(ChromeQuad::rounded(
                        PxRect {
                            x: prefix_x + (TAB_PROMPT_SLOT * ctx.scale - dot) * 0.5,
                            y: top + (height - dot) * 0.5,
                            w: dot,
                            h: dot,
                        },
                        ctx.theme.diff_add,
                        dot * 0.5,
                    ));
                } else {
                    let line = measure.line_height(FontRole::Mono);
                    labels.push(ProseLabel {
                        text: TAB_PROMPT.to_owned(),
                        x: prefix_x,
                        y: top + (height - line) * 0.5,
                        role: FontRole::Mono,
                        color: ctx.theme.accent,
                        weight: None,
                        max_w: f32::MAX,
                    });
                }
                // The label (the tab title, §10.3), inset past the prefix slot + gap so all
                // tabs align.
                let x = prefix_x + (TAB_PROMPT_SLOT + TAB_GAP) * ctx.scale;
                let line = measure.line_height(FontRole::Label);
                let title = ctx
                    .tab_titles
                    .get(index)
                    .cloned()
                    .unwrap_or_else(|| format!("Tab {}", index + 1));
                labels.push(ProseLabel {
                    text: title,
                    x,
                    y: top + (height - line) * 0.5,
                    role: FontRole::Label,
                    color,
                    weight: None,
                    max_w: (ctx.panel.x + ctx.panel.w - x - LABEL_INSET * ctx.scale * 0.5).max(1.0),
                });
            }
        }
        RowKind::OverflowUp(hidden) | RowKind::OverflowDown(hidden) if hidden > 0 => {
            let arrow = if matches!(row.kind, RowKind::OverflowUp(_)) {
                '↑'
            } else {
                '↓'
            };
            let text = if ctx.rail {
                arrow.to_string()
            } else {
                format!("{arrow} {hidden} more")
            };
            place(
                labels,
                measure,
                &text,
                FontRole::Caption,
                ctx.theme.fg_muted,
            );
        }
        RowKind::OverflowUp(_) | RowKind::OverflowDown(_) => {}
        RowKind::NewTab => {
            let text = if ctx.rail { "+" } else { "+ New tab" };
            place(labels, measure, text, FontRole::Label, ctx.theme.fg_muted);
        }
    }
}

/// The command-input well (design §08 #3): a rounded `bg.surface` field with a `border.subtle`
/// ring, holding a `⌕` glyph + a "Search or run…" placeholder (both `fg.muted`), that opens the
/// palette. The rail is too narrow for the field, so it shows just a centered `⌕` button.
fn push_command_well(
    quads: &mut Vec<ChromeQuad>,
    labels: &mut Vec<ProseLabel>,
    top: f32,
    height: f32,
    ctx: &RowCtx,
    measure: &mut TextMeasure,
) {
    let cy = top + (height - measure.line_height(FontRole::Caption)) * 0.5;
    if ctx.rail {
        // A centered search glyph standing in for the field.
        let w = measure.width(CMD_ICON, FontRole::Caption, None);
        labels.push(ProseLabel {
            text: CMD_ICON.to_owned(),
            x: ctx.panel.x + (ctx.panel.w - w) * 0.5,
            y: cy,
            role: FontRole::Caption,
            color: ctx.theme.fg_muted,
            weight: None,
            max_w: f32::MAX,
        });
        return;
    }
    let inset = CMD_INSET * ctx.scale;
    let well = PxRect {
        x: ctx.panel.x + inset,
        y: top,
        w: (ctx.panel.w - 2.0 * inset).max(0.0),
        h: height,
    };
    // A `border.subtle` ring drawn behind the `bg.surface` fill (inset by the 1px stroke).
    let stroke = ctx.scale.max(1.0);
    quads.push(ChromeQuad::rounded(
        well,
        ctx.theme.border_subtle,
        CMD_RADIUS * ctx.scale,
    ));
    quads.push(ChromeQuad::rounded(
        PxRect {
            x: well.x + stroke,
            y: well.y + stroke,
            w: (well.w - 2.0 * stroke).max(0.0),
            h: (well.h - 2.0 * stroke).max(0.0),
        },
        ctx.theme.bg_surface,
        (CMD_RADIUS * ctx.scale - stroke).max(0.0),
    ));
    // The search glyph + placeholder, left-aligned inside the field (guide `padding:0 10px;
    // gap:8px`).
    let pad = 10.0 * ctx.scale;
    let mut x = well.x + pad;
    for (text, gap) in [(CMD_ICON, 8.0), (CMD_PLACEHOLDER, 0.0)] {
        labels.push(ProseLabel {
            text: text.to_owned(),
            x,
            y: cy,
            role: FontRole::Caption,
            color: ctx.theme.fg_muted,
            weight: None,
            max_w: (well.x + well.w - pad - x).max(1.0),
        });
        x += measure.width(text, FontRole::Caption, None) + gap * ctx.scale;
    }
}

/// The logical height reserved for the brand-lockup row below the traffic-light strip, or 0
/// where no brand is drawn (the slim rail, or a build with no title strip). Everything below the
/// strip - chips, command well, tab list - shifts down by this so the lockup gets its own row.
fn brand_block_h(view: &View) -> f32 {
    if !view.rail && view.top_inset > 1.0 {
        BRAND_BLOCK
    } else {
        0.0
    }
}

/// The logical height the workspace-chip block occupies above the command well (its 26px row
/// plus the gap), or 0 when there are no chips or in the rail.
fn chips_block_h(view: &View) -> f32 {
    if view.rail || view.chips.is_empty() {
        0.0
    } else {
        CHIP_SIZE + CHIP_BLOCK_GAP
    }
}

/// The workspace chips' hit/draw rectangles (physical px): one 26px tile per workspace plus a
/// trailing `+` tile, left-clustered from `CHIP_INSET` with a `CHIP_GAP` between them, just
/// below the control strip. Shared by [`hit`] and [`push_chips`].
#[allow(
    clippy::cast_precision_loss,
    reason = "the chip index is a tiny count (workspaces are few)"
)]
fn chip_slots(view: &View, panel: PxRect, scale: f32) -> Vec<PxRect> {
    if view.chips.is_empty() {
        return Vec::new();
    }
    let y = panel.y + (view.top_inset + brand_block_h(view) + PAD_TOP) * scale;
    let size = CHIP_SIZE * scale;
    let step = (CHIP_SIZE + CHIP_GAP) * scale;
    let x0 = panel.x + CHIP_INSET * scale;
    (0..=view.chips.len())
        .map(|i| PxRect {
            x: x0 + i as f32 * step,
            y,
            w: size,
            h: size,
        })
        .collect()
}

/// The workspace-switcher chips (design §08 #2): a rounded tile per workspace (the active one
/// `accent`@0.16 filled + `accent`@0.4 bordered, its glyph in `accent`; the rest `bg.surface`
/// with a `fg.muted` glyph) plus a trailing `+` tile. Full panel only.
fn push_chips(
    quads: &mut Vec<ChromeQuad>,
    labels: &mut Vec<ProseLabel>,
    view: &View,
    panel: PxRect,
    scale: f32,
    theme: &Theme,
    measure: &mut TextMeasure,
) {
    let slots = chip_slots(view, panel, scale);
    let radius = CHIP_RADIUS * scale;
    let stroke = scale.max(1.0);
    let line = measure.line_height(FontRole::Mono);
    for (i, slot) in slots.iter().enumerate() {
        let is_add = i >= view.chips.len();
        let active = !is_add && i == view.active_chip;
        if active {
            // accent@0.4 border ring over an accent.subtle (§03) fill, both composited in sRGB
            // over the sidebar bg so they read at the guide's weight (not the brighter linear-
            // space GPU blend).
            quads.push(ChromeQuad::rounded(
                *slot,
                theme.accent.over(theme.bg_sidebar, 0.4),
                radius,
            ));
            let inner = PxRect {
                x: slot.x + stroke,
                y: slot.y + stroke,
                w: (slot.w - 2.0 * stroke).max(0.0),
                h: (slot.h - 2.0 * stroke).max(0.0),
            };
            let inner_r = (radius - stroke).max(0.0);
            quads.push(ChromeQuad::rounded(
                inner,
                theme.accent_subtle_on(theme.bg_sidebar),
                inner_r,
            ));
        } else {
            quads.push(ChromeQuad::rounded(*slot, theme.bg_surface, radius));
        }
        let glyph = if is_add {
            "+".to_owned()
        } else {
            view.chips[i].to_string()
        };
        let gw = measure.width(&glyph, FontRole::Mono, None);
        labels.push(ProseLabel {
            text: glyph,
            x: slot.x + (slot.w - gw) * 0.5,
            y: slot.y + (slot.h - line) * 0.5,
            role: FontRole::Mono,
            color: if active { theme.accent } else { theme.fg_muted },
            weight: None,
            max_w: f32::MAX,
        });
    }
}

/// The width (physical px) of one square pinned tile: the content span split into
/// `PINNED_COLS` columns with a `PINNED_GRID_GAP` between them.
#[allow(
    clippy::cast_precision_loss,
    reason = "PINNED_COLS is a tiny fixed count (3)"
)]
fn pinned_tile_w(panel: PxRect, scale: f32) -> f32 {
    let gaps = (PINNED_COLS - 1) as f32 * PINNED_GRID_GAP;
    ((panel.w / scale - 2.0 * PINNED_INSET - gaps) / PINNED_COLS as f32).max(1.0) * scale
}

/// The logical height the pinned grid block occupies (its "PINNED" label + the tile rows), or
/// 0 when nothing is pinned or in the rail.
#[allow(
    clippy::cast_precision_loss,
    reason = "the pinned-row count is a tiny value (few pinned tabs)"
)]
fn pinned_block_h(view: &View, panel: PxRect, scale: f32) -> f32 {
    if view.rail || view.pinned.is_empty() {
        return 0.0;
    }
    let rows = view.pinned.len().div_ceil(PINNED_COLS) as f32;
    let tile = pinned_tile_w(panel, scale) / scale; // logical
    PINNED_LABEL_H + rows * tile + rows * PINNED_GRID_GAP
}

/// The pinned tiles' hit/draw rectangles (physical px), a 3-column grid below the "PINNED"
/// label whose top is `block_top`. Shared by [`hit`] and [`push_pinned`].
#[allow(
    clippy::cast_precision_loss,
    reason = "the tile col/row indices are tiny values"
)]
fn pinned_slots(count: usize, panel: PxRect, block_top: f32, scale: f32) -> Vec<PxRect> {
    let tile = pinned_tile_w(panel, scale);
    let gap = PINNED_GRID_GAP * scale;
    let x0 = panel.x + PINNED_INSET * scale;
    let y0 = block_top + PINNED_LABEL_H * scale;
    (0..count)
        .map(|i| {
            let col = (i % PINNED_COLS) as f32;
            let row = (i / PINNED_COLS) as f32;
            PxRect {
                x: x0 + col * (tile + gap),
                y: y0 + row * (tile + gap),
                w: tile,
                h: tile,
            }
        })
        .collect()
}

/// The pinned-tab grid (design §08 #4): a `fg.faint` "PINNED" label over a 3-up grid of rounded
/// `bg.surface` tiles (the active one `accent`-bordered), each holding its glyph in `fg.muted`.
#[allow(clippy::too_many_arguments, reason = "one focused pinned-grid builder")]
fn push_pinned(
    quads: &mut Vec<ChromeQuad>,
    labels: &mut Vec<ProseLabel>,
    pinned: &[char],
    active_pinned: Option<usize>,
    panel: PxRect,
    block_top: f32,
    scale: f32,
    theme: &Theme,
    measure: &mut TextMeasure,
) {
    // The "PINNED" label.
    let lh = measure.line_height(FontRole::Micro);
    labels.push(ProseLabel {
        text: "PINNED".to_owned(),
        x: panel.x + PINNED_INSET * scale,
        y: block_top + (PINNED_LABEL_H * scale - lh) * 0.5,
        role: FontRole::Micro,
        color: theme.fg_faint,
        weight: None,
        max_w: f32::MAX,
    });
    let slots = pinned_slots(pinned.len(), panel, block_top, scale);
    let radius = PINNED_RADIUS * scale;
    let stroke = scale.max(1.0);
    let gline = measure.line_height(FontRole::Mono);
    for (i, slot) in slots.iter().enumerate() {
        let active = active_pinned == Some(i);
        quads.push(ChromeQuad::rounded(*slot, theme.bg_surface, radius));
        if active {
            // A 1px accent@0.4 ring (composited in sRGB over the surface) for the active pinned
            // tab, its interior left as the surface fill.
            quads.push(ChromeQuad::rounded(
                *slot,
                theme.accent.over(theme.bg_surface, 0.4),
                radius,
            ));
            let inner = PxRect {
                x: slot.x + stroke,
                y: slot.y + stroke,
                w: (slot.w - 2.0 * stroke).max(0.0),
                h: (slot.h - 2.0 * stroke).max(0.0),
            };
            quads.push(ChromeQuad::rounded(
                inner,
                theme.bg_surface,
                (radius - stroke).max(0.0),
            ));
        }
        let glyph = pinned[i].to_string();
        let gw = measure.width(&glyph, FontRole::Mono, None);
        labels.push(ProseLabel {
            text: glyph,
            x: slot.x + (slot.w - gw) * 0.5,
            y: slot.y + (slot.h - gline) * 0.5,
            role: FontRole::Mono,
            color: if active { theme.accent } else { theme.fg_muted },
            weight: None,
            max_w: f32::MAX,
        });
    }
}

/// The active tab's marks, per the §09 "Sidebar tab item": a rounded pill inset from the
/// sidebar edges with an `accent`@0.14 fill and a 1px `accent`@0.28 border, plus a short 3x14
/// rounded `accent` indicator bar seated inside the pill's left padding (not a full-height rule
/// at the sidebar edge). The border is drawn as an `accent`@0.28 rounded rect with the interior
/// reset to `bg.sidebar` before the fill, so the 1px edge stays stronger than the fill.
fn push_active_marks(
    quads: &mut Vec<ChromeQuad>,
    panel: PxRect,
    top: f32,
    height: f32,
    scale: f32,
    theme: &Theme,
) {
    let inset = PILL_INSET * scale;
    let radius = PILL_RADIUS * scale;
    let stroke = scale.max(1.0);
    let pill = PxRect {
        x: panel.x + inset,
        y: top,
        w: (panel.w - 2.0 * inset).max(0.0),
        h: height,
    };
    // The 1px accent@0.28 border ring over an accent.subtle (§03) fill, both composited in sRGB
    // over the sidebar bg so the border reads stronger than the fill at the guide's weight.
    quads.push(ChromeQuad::rounded(
        pill,
        theme.accent.over(theme.bg_sidebar, 0.28),
        radius,
    ));
    let inner = PxRect {
        x: pill.x + stroke,
        y: pill.y + stroke,
        w: (pill.w - 2.0 * stroke).max(0.0),
        h: (pill.h - 2.0 * stroke).max(0.0),
    };
    let inner_r = (radius - stroke).max(0.0);
    quads.push(ChromeQuad::rounded(
        inner,
        theme.accent_subtle_on(theme.bg_sidebar),
        inner_r,
    ));
    // The 3x14 rounded indicator bar inside the pill's left padding, vertically centered.
    let bar_h = BAR_H * scale;
    quads.push(ChromeQuad::rounded(
        PxRect {
            x: pill.x + TAB_PAD_X * scale,
            y: top + (height - bar_h) * 0.5,
            w: BAR_W * scale,
            h: bar_h,
        },
        theme.accent,
        BAR_RADIUS * scale,
    ));
}

/// The bottom-anchored utility bar (design §08 #7): a `border.subtle` top hairline over the
/// sidebar surface, then the four toggle glyphs (⚙ ◐ ⟲ ⑂) in `fg.muted`, left-clustered per
/// the guide. Full panel only. Icon-only; the tooltip-on-hover is a follow-up (no tooltip
/// layer yet).
fn push_utility_bar(
    quads: &mut Vec<ChromeQuad>,
    labels: &mut Vec<ProseLabel>,
    panel: PxRect,
    scale: f32,
    theme: &Theme,
    measure: &mut TextMeasure,
) {
    let top = panel.y + panel.h - UTIL_H * scale;
    // Top hairline separating the footer from the tab list.
    quads.push(ChromeQuad::fill(
        PxRect {
            x: panel.x,
            y: top,
            w: panel.w,
            h: scale.max(1.0),
        },
        theme.border_subtle,
    ));
    // Larger than default UI text (§05 `h2`, 20px) so the toggles read as real touch targets,
    // each centered in its evenly-spread slot.
    let role = FontRole::H2;
    let line_h = measure.line_height(role);
    let cy = top + (UTIL_H * scale - line_h) * 0.5;
    for ((_, glyph), (_, slot)) in UTIL_ICONS.iter().zip(utility_slots(panel, scale)) {
        let gw = measure.width(glyph, role, None);
        labels.push(ProseLabel {
            text: (*glyph).to_owned(),
            x: slot.x + (slot.w - gw) * 0.5,
            y: cy,
            role,
            color: theme.fg_muted,
            weight: None,
            max_w: f32::MAX,
        });
    }
}

/// Place one label vertically centered in its row: left-inset for the full panel, or
/// horizontally centered for the rail (measuring the glyph run's width). `top`/`height` are
/// the row's physical box; the label's line box is centered within it.
#[allow(clippy::too_many_arguments, reason = "one focused placement helper")]
fn push_label(
    labels: &mut Vec<ProseLabel>,
    measure: &mut TextMeasure,
    text: &str,
    role: FontRole,
    color: Srgb,
    panel: PxRect,
    top: f32,
    height: f32,
    rail: bool,
    scale: f32,
) {
    let line_h = measure.line_height(role);
    let y = top + (height - line_h) * 0.5;
    let x = if rail {
        let w = measure.width(text, role, None);
        panel.x + (panel.w - w) * 0.5
    } else {
        panel.x + LABEL_INSET * scale
    };
    let max_w = (panel.x + panel.w - x - LABEL_INSET * scale * 0.5).max(1.0);
    labels.push(ProseLabel {
        text: text.to_owned(),
        x,
        y,
        role,
        color,
        weight: None,
        max_w,
    });
}

#[cfg(test)]
mod tests {
    use super::{build, hit, GroupSpan, Hit, Sidebar, UtilAction, View, UTIL_ICONS};
    use skelly_config::SidebarMode;
    use skelly_render::{PxRect, TextMeasure, Theme};

    /// A tall panel so a small tab list never scrolls, at 2x DPI.
    fn panel() -> PxRect {
        PxRect {
            x: 0.0,
            y: 0.0,
            w: 240.0 * 2.0,
            h: 800.0 * 2.0,
        }
    }

    /// A view with the given tab count / active tab / rail flag, no workspace chips (so the
    /// existing tab-geometry tests are unaffected) and no control-strip inset.
    fn view(count: usize, active: usize, rail: bool) -> View<'static> {
        View {
            tab_count: count,
            active_tab: active,
            chips: &[],
            active_chip: 0,
            pinned: &[],
            active_pinned: None,
            groups: &[],
            tab_running: &[],
            tab_titles: &[],
            rail,
            top_inset: 0.0,
        }
    }

    #[test]
    fn new_respects_the_configured_mode() {
        assert!(Sidebar::new(SidebarMode::Fixed).visible());
        assert!(Sidebar::new(SidebarMode::Autohide).visible());
        assert!(!Sidebar::new(SidebarMode::Hidden).visible());
        assert!(Sidebar::new(SidebarMode::Autohide).is_rail());
        assert!(!Sidebar::new(SidebarMode::Fixed).is_rail());
    }

    #[test]
    fn toggle_hides_then_recalls_the_prior_visible_mode() {
        let mut s = Sidebar::new(SidebarMode::Autohide);
        s.toggle();
        assert_eq!(s.mode(), SidebarMode::Hidden);
        assert!(!s.visible());
        s.toggle();
        assert_eq!(s.mode(), SidebarMode::Autohide);
        assert!(s.is_rail());
    }

    #[test]
    fn cycle_rail_flips_full_and_rail_and_unhides() {
        let mut s = Sidebar::new(SidebarMode::Fixed);
        s.cycle_rail();
        assert_eq!(s.mode(), SidebarMode::Autohide);
        s.cycle_rail();
        assert_eq!(s.mode(), SidebarMode::Fixed);
        s.toggle();
        assert_eq!(s.mode(), SidebarMode::Hidden);
        s.cycle_rail();
        assert_eq!(s.mode(), SidebarMode::Fixed);
    }

    #[test]
    fn set_mode_tracks_the_config_and_keeps_a_visible_recall_target() {
        let mut s = Sidebar::new(SidebarMode::Fixed);
        s.set_mode(SidebarMode::Hidden);
        assert!(!s.visible());
        s.toggle();
        assert_eq!(s.mode(), SidebarMode::Fixed);
    }

    #[test]
    fn hit_maps_a_click_in_a_tab_band_to_that_tab() {
        // Three tabs, ample height, 2x DPI. Probe the vertical center of each row band by
        // rebuilding the same layout the renderer uses. `px` is in the panel's left area (it
        // only matters for the utility bar).
        let p = panel();
        let x = 20.0 * 2.0;
        // A y inside the command well (PAD_TOP 10 .. 40) opens the palette.
        assert_eq!(
            hit(&view(3, 0, false), p, 2.0, x, 20.0 * 2.0),
            Some(Hit::CommandInput)
        );
        // The gap below the well (40 .. 52) maps to nothing.
        assert_eq!(hit(&view(3, 0, false), p, 2.0, x, 46.0 * 2.0), None);
        // With no overflow, the first tab sits flush under the well: PAD_TOP(10) + CMD_H(30) +
        // CMD_GAP(12) = 52 logical, first tab (TAB_H 30) spans 52..82; probe its center (67).
        assert_eq!(
            hit(&view(3, 0, false), p, 2.0, x, 67.0 * 2.0),
            Some(Hit::Tab(0))
        );
        // The second tab: 52 + TAB_H(30) + TAB_GAP_V(3) = 85 .. 115; probe its center (100).
        assert_eq!(
            hit(&view(3, 0, false), p, 2.0, x, 100.0 * 2.0),
            Some(Hit::Tab(1))
        );
    }

    #[test]
    fn hit_maps_the_footer_icons_to_their_utility_actions() {
        // The full-panel footer spreads four equal slots across the padded inner width: with a
        // 240-logical sidebar and UTIL_PAD_X(15), inner = 210 and each slot is 52.5 logical, so
        // icon i is centered at 15 + i·52.5 + 26.25. Probe each center inside the 40px footer.
        let p = panel();
        let y = p.h - 20.0 * 2.0; // 20 logical up from the bottom
        let center = |i: f32| (15.0 + i * 52.5 + 26.25) * 2.0;
        assert_eq!(
            hit(&view(3, 0, false), p, 2.0, center(0.0), y),
            Some(Hit::Util(UtilAction::Settings))
        );
        assert_eq!(
            hit(&view(3, 0, false), p, 2.0, center(1.0), y),
            Some(Hit::Util(UtilAction::Theme))
        );
        assert_eq!(
            hit(&view(3, 0, false), p, 2.0, center(2.0), y),
            Some(Hit::Util(UtilAction::Timeline))
        );
        assert_eq!(
            hit(&view(3, 0, false), p, 2.0, center(3.0), y),
            Some(Hit::Util(UtilAction::Git))
        );
    }

    #[test]
    fn build_lists_every_tab_and_marks_the_active_one() {
        let theme = Theme::resolve("ossein-dark");
        let mut m = TextMeasure::new(2.0);
        let paint = build(&view(3, 1, false), panel(), 2.0, &theme, &mut m);
        // A label for the header, three tabs, and the new-tab action (overflow indicators
        // are hidden with no overflow) = 5 labels.
        let tab_labels: Vec<_> = paint
            .labels
            .iter()
            .filter(|l| l.text.starts_with("Tab "))
            .collect();
        assert_eq!(tab_labels.len(), 3);
        assert!(paint.labels.iter().any(|l| l.text == "+ New tab"));
        // The active tab (index 1) is primary-colored; an inactive one is secondary.
        let active = tab_labels.iter().find(|l| l.text == "Tab 2").unwrap();
        let inactive = tab_labels.iter().find(|l| l.text == "Tab 1").unwrap();
        assert_eq!(active.color, theme.fg_primary);
        assert_eq!(inactive.color, theme.fg_secondary);
        // The active tab contributes an accent-subtle pill + an accent bar (2 extra quads
        // over the surface fill + right divider).
        assert!(paint.quads.len() >= 4);
    }

    /// A full-panel view with `count` ordered tabs, single-char titles, and the given groups.
    fn grouped_view<'a>(count: usize, active: usize, groups: &'a [GroupSpan<'a>]) -> View<'a> {
        View {
            tab_count: count,
            active_tab: active,
            chips: &[],
            active_chip: 0,
            pinned: &[],
            active_pinned: None,
            groups,
            tab_running: &[],
            tab_titles: &[],
            rail: false,
            top_inset: 0.0,
        }
    }

    #[test]
    fn grouped_layout_renders_headers_and_collapse_hides_children() {
        let theme = Theme::resolve("ossein-dark");
        let mut m = TextMeasure::new(2.0);
        // Positions 0,1 are ungrouped; 2,3 belong to group "proj".
        let expanded = [GroupSpan {
            name: "proj",
            collapsed: false,
            start: 2,
            len: 2,
        }];
        let paint = build(&grouped_view(4, 0, &expanded), panel(), 2.0, &theme, &mut m);
        let texts: Vec<&str> = paint.labels.iter().map(|l| l.text.as_str()).collect();
        assert!(texts.contains(&"proj"), "group name header missing");
        assert!(texts.contains(&"2"), "group member count missing");
        // All four tabs (2 ungrouped + 2 members) show as "Tab N".
        assert_eq!(
            texts.iter().filter(|t| t.starts_with("Tab ")).count(),
            4,
            "expanded group should list its members"
        );

        // Collapsing the group hides its two member rows (header + count still shown).
        let collapsed = [GroupSpan {
            name: "proj",
            collapsed: true,
            start: 2,
            len: 2,
        }];
        let paint = build(
            &grouped_view(4, 0, &collapsed),
            panel(),
            2.0,
            &theme,
            &mut m,
        );
        let texts: Vec<&str> = paint.labels.iter().map(|l| l.text.as_str()).collect();
        assert!(
            texts.contains(&"proj"),
            "collapsed header still shows the name"
        );
        assert_eq!(
            texts.iter().filter(|t| t.starts_with("Tab ")).count(),
            2,
            "a collapsed group hides its members, leaving the 2 ungrouped tabs"
        );
    }

    #[test]
    fn hit_maps_group_headers_and_members_in_order() {
        let groups = [GroupSpan {
            name: "proj",
            collapsed: false,
            start: 2,
            len: 2,
        }];
        let v = grouped_view(4, 0, &groups);
        let p = panel();
        // Scan down the panel collecting the distinct row hits in order.
        let mut seq: Vec<Hit> = Vec::new();
        let mut y = 0.0;
        while y < p.h {
            if let Some(h) = hit(&v, p, 2.0, 120.0, y) {
                if seq.last() != Some(&h) {
                    seq.push(h);
                }
            }
            y += 2.0;
        }
        let pos = |h: Hit| {
            seq.iter()
                .position(|x| *x == h)
                .unwrap_or_else(|| panic!("{h:?} not hit"))
        };
        // The two ungrouped tabs precede the header, whose members follow it.
        assert!(pos(Hit::Tab(1)) < pos(Hit::GroupHeader(0)));
        assert!(pos(Hit::GroupHeader(0)) < pos(Hit::Tab(2)));
        assert!(pos(Hit::Tab(2)) < pos(Hit::Tab(3)));
        assert!(pos(Hit::Tab(3)) < pos(Hit::NewTab));

        // A collapsed group's member rows are not hittable at all.
        let collapsed = [GroupSpan {
            name: "proj",
            collapsed: true,
            start: 2,
            len: 2,
        }];
        let vc = grouped_view(4, 0, &collapsed);
        let mut y = 0.0;
        let mut saw_member = false;
        while y < p.h {
            if matches!(hit(&vc, p, 2.0, 120.0, y), Some(Hit::Tab(2 | 3))) {
                saw_member = true;
            }
            y += 2.0;
        }
        assert!(!saw_member, "collapsed group members must not be hittable");
    }

    #[test]
    fn workspace_chips_render_and_map_clicks() {
        let theme = Theme::resolve("ossein-dark");
        let mut m = TextMeasure::new(2.0);
        let chips = ['P', 'W'];
        let v = View {
            tab_count: 2,
            active_tab: 0,
            chips: &chips,
            active_chip: 0,
            pinned: &[],
            active_pinned: None,
            groups: &[],
            tab_running: &[],
            tab_titles: &[],
            rail: false,
            top_inset: 0.0,
        };
        let paint = build(&v, panel(), 2.0, &theme, &mut m);
        // The two workspace glyphs + the trailing "+" tile are drawn.
        assert!(paint.labels.iter().any(|l| l.text == "P"));
        assert!(paint.labels.iter().any(|l| l.text == "W"));
        assert!(paint.labels.iter().any(|l| l.text == "+"));
        // Chips sit at y in [10, 36] (logical) from CHIP_INSET(13) with a 33px step; probe each
        // tile's center. Chip 0 -> Workspace(0), chip 1 -> Workspace(1), the "+" -> AddWorkspace.
        let cy = 23.0 * 2.0;
        assert_eq!(
            hit(&v, panel(), 2.0, 26.0 * 2.0, cy),
            Some(Hit::Workspace(0))
        );
        assert_eq!(
            hit(&v, panel(), 2.0, 59.0 * 2.0, cy),
            Some(Hit::Workspace(1))
        );
        assert_eq!(
            hit(&v, panel(), 2.0, 92.0 * 2.0, cy),
            Some(Hit::AddWorkspace)
        );
    }

    #[test]
    fn build_draws_the_command_well() {
        let theme = Theme::resolve("ossein-dark");
        let mut m = TextMeasure::new(2.0);
        // Full panel: the search glyph + the placeholder text.
        let full = build(&view(2, 0, false), panel(), 2.0, &theme, &mut m);
        assert!(full.labels.iter().any(|l| l.text == "\u{2315}"));
        assert!(full.labels.iter().any(|l| l.text.contains("Search or run")));
        // Rail: just the centered glyph, no placeholder text.
        let rail = build(&view(2, 0, true), panel(), 2.0, &theme, &mut m);
        assert!(rail.labels.iter().any(|l| l.text == "\u{2315}"));
        assert!(rail.labels.iter().all(|l| !l.text.contains("Search")));
    }

    #[test]
    fn build_draws_the_utility_bar_glyphs() {
        let theme = Theme::resolve("ossein-dark");
        let mut m = TextMeasure::new(2.0);
        let paint = build(&view(2, 0, false), panel(), 2.0, &theme, &mut m);
        // Each utility glyph is drawn once, in the footer (below the tab list).
        for (_, glyph) in UTIL_ICONS {
            assert!(
                paint.labels.iter().any(|l| l.text == glyph),
                "utility glyph {glyph:?} should be drawn"
            );
        }
        // The slim rail has no room for the footer, so it omits the glyphs entirely.
        let rail = build(&view(2, 0, true), panel(), 2.0, &theme, &mut m);
        for (_, glyph) in UTIL_ICONS {
            assert!(
                rail.labels.iter().all(|l| l.text != glyph),
                "rail should omit utility glyph {glyph:?}"
            );
        }
    }

    #[test]
    fn rail_centers_the_tab_number() {
        let theme = Theme::resolve("ossein-dark");
        let mut m = TextMeasure::new(2.0);
        let p = PxRect {
            x: 0.0,
            y: 0.0,
            w: 56.0 * 2.0,
            h: 800.0 * 2.0,
        };
        let paint = build(&view(3, 1, true), p, 2.0, &theme, &mut m);
        // Rail tab labels are the bare numbers, centered (x offset from the left edge > 0).
        let two = paint.labels.iter().find(|l| l.text == "2").unwrap();
        assert!(two.x > 0.0 && two.x < p.w);
    }
}
