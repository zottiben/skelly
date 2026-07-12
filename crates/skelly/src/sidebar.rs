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
//! switcher chips (§08 #2) at the top, the command-input well (§08 #3, opens the palette), and
//! the bottom-anchored utility bar (§08 #7 - the ⚙ settings / ◐ theme / ⟲ timeline / ⑂ git
//! toggles). Deferred to later slices: the pinned grid and collapsible groups; per-tab
//! cwd/branch titling (tabs are numbered today).

use skelly_config::SidebarMode;
use skelly_render::{ChromeQuad, FontRole, ProseLabel, PxRect, Srgb, TextMeasure, Theme};

/// Layout constants in **logical** px (multiplied by the DPI scale when placed). Tuned to
/// the guide's §08 sidebar: a compact group header, comfortable 13px `label` tab rows, and
/// a matching "+ New tab" action.
const PAD_TOP: f32 = 10.0;
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
/// Height of the group header row (design §08 #5: the "repo · branch" context label above the
/// tab list), an uppercase micro label with breathing room.
const GROUP_H: f32 = 22.0;
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
    /// Open the command palette (the command-input well, design §08 #3).
    CommandInput,
    /// Switch to the tab at this 0-based index.
    Tab(usize),
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
    /// The group header above the tab list (design §08 #5: the "repo · branch" context), or
    /// `None` outside a git repo. Shown uppercase.
    pub(crate) group_label: Option<&'a str>,
    /// Whether each tab has a live foreground job (a `●` running dot instead of the `❯` prompt),
    /// one flag per tab. Empty or short-of-`tab_count` means "not running".
    pub(crate) tab_running: &'a [bool],
    /// Whether the sidebar is the slim icon rail.
    pub(crate) rail: bool,
    /// The macOS control-strip inset in **logical** px (0 elsewhere); content clears it.
    pub(crate) top_inset: f32,
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
/// Left padding of the full-panel utility row (the guide's `padding:0 15px`).
const UTIL_PAD_X: f32 = 15.0;
/// Per-icon step in the full-panel utility row (icon box + the guide's `gap:16px`); also the
/// icon's click box, so draw and hit coincide.
const UTIL_STEP: f32 = 34.0;

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
    /// The group header (design §08 #5), drawn only when there is a group label.
    Group,
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
    has_group: bool,
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

    // The group header (design §08 #5), above the tab list, when there is one.
    if has_group {
        rows.push(Row {
            top: y,
            height: GROUP_H,
            kind: RowKind::Group,
        });
        y += GROUP_H;
    }

    // Capacity for tab rows, reserving both overflow-indicator slots + the new-tab action.
    // Each tab occupies its pill height plus the inter-tab gap.
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

    // The overflow indicators only occupy a row when there is something hidden, so a short tab
    // list sits flush under the group header / command well (no reserved gap).
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
    let top_inset = view.top_inset + chips_block_h(view);
    let y_logical = (py - panel.y) / scale;
    for row in rows_layout(
        view.tab_count,
        view.active_tab,
        panel.h / scale,
        top_inset,
        view.group_label.is_some(),
    ) {
        if y_logical >= row.top && y_logical < row.top + row.height {
            return match row.kind {
                RowKind::Command => Some(Hit::CommandInput),
                RowKind::Tab(index) => Some(Hit::Tab(index)),
                RowKind::NewTab => Some(Hit::NewTab),
                _ => None,
            };
        }
    }
    None
}

/// The full-panel utility bar's per-icon hit slots (physical px), each returning its action +
/// click box: the icons left-cluster (the guide's `padding:0 15px; gap:16px`) in fixed
/// `UTIL_STEP` boxes from `UTIL_PAD_X`. Shared by [`hit`] and [`build`] so the drawn glyph and
/// its click target coincide.
#[allow(
    clippy::cast_precision_loss,
    reason = "the icon index is a tiny fixed range (0..4)"
)]
fn utility_slots(panel: PxRect, scale: f32) -> Vec<(UtilAction, PxRect)> {
    let top = panel.y + panel.h - UTIL_H * scale;
    let h = UTIL_H * scale;
    UTIL_ICONS
        .iter()
        .enumerate()
        .map(|(i, (action, _))| {
            (
                *action,
                PxRect {
                    x: panel.x + (UTIL_PAD_X + i as f32 * UTIL_STEP) * scale,
                    y: top,
                    w: UTIL_STEP * scale,
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

    // The workspace chips sit below the control strip; the tab flow starts below them.
    let chips_block = chips_block_h(view);
    if !view.rail {
        push_chips(&mut quads, &mut labels, view, panel, scale, theme, measure);
    }

    let ctx = RowCtx {
        panel,
        active: view.active_tab,
        rail: view.rail,
        group_label: view.group_label,
        tab_running: view.tab_running,
        scale,
        theme,
    };
    for row in rows_layout(
        view.tab_count,
        view.active_tab,
        panel.h / scale,
        view.top_inset + chips_block,
        view.group_label.is_some(),
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
    group_label: Option<&'a str>,
    tab_running: &'a [bool],
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
        RowKind::Group => {
            // The "repo · branch" group header (design §08 #5): an uppercase micro label,
            // quiet (fg.faint), inset like the tab labels.
            if let Some(label) = ctx.group_label {
                if !ctx.rail {
                    push_label(
                        labels,
                        measure,
                        &label.to_uppercase(),
                        FontRole::Micro,
                        ctx.theme.fg_faint,
                        ctx.panel,
                        top,
                        height,
                        false,
                        ctx.scale,
                    );
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
                // The label, inset past the prefix slot + gap so all tabs align.
                let x = prefix_x + (TAB_PROMPT_SLOT + TAB_GAP) * ctx.scale;
                let line = measure.line_height(FontRole::Label);
                labels.push(ProseLabel {
                    text: format!("Tab {}", index + 1),
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
    let y = panel.y + (view.top_inset + PAD_TOP) * scale;
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
            // accent@0.4 border ring, interior reset to the sidebar bg, then accent@0.16 fill.
            quads.push(ChromeQuad::tint(*slot, theme.accent, 0.4, radius));
            let inner = PxRect {
                x: slot.x + stroke,
                y: slot.y + stroke,
                w: (slot.w - 2.0 * stroke).max(0.0),
                h: (slot.h - 2.0 * stroke).max(0.0),
            };
            let inner_r = (radius - stroke).max(0.0);
            quads.push(ChromeQuad::rounded(inner, theme.bg_sidebar, inner_r));
            quads.push(ChromeQuad::tint(inner, theme.accent, 0.16, inner_r));
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
    // The 1px accent@0.28 border ring.
    quads.push(ChromeQuad::tint(pill, theme.accent, 0.28, radius));
    // Reset the interior to the sidebar surface, then lay the accent@0.14 fill over it, so the
    // border reads stronger than the fill (translucent-over-translucent would only add up).
    let inner = PxRect {
        x: pill.x + stroke,
        y: pill.y + stroke,
        w: (pill.w - 2.0 * stroke).max(0.0),
        h: (pill.h - 2.0 * stroke).max(0.0),
    };
    let inner_r = (radius - stroke).max(0.0);
    quads.push(ChromeQuad::rounded(inner, theme.bg_sidebar, inner_r));
    quads.push(ChromeQuad::tint(inner, theme.accent, 0.14, inner_r));
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
    let line_h = measure.line_height(FontRole::Body);
    let cy = top + (UTIL_H * scale - line_h) * 0.5;
    for ((_, glyph), (_, slot)) in UTIL_ICONS.iter().zip(utility_slots(panel, scale)) {
        labels.push(ProseLabel {
            text: (*glyph).to_owned(),
            x: slot.x,
            y: cy,
            role: FontRole::Body,
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
    use super::{build, hit, Hit, Sidebar, UtilAction, View, UTIL_ICONS};
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
            group_label: None,
            tab_running: &[],
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
        // The full-panel footer left-clusters four fixed UTIL_STEP(34) boxes from
        // UTIL_PAD_X(15): icon i spans [15 + i·34, +34] logical. Probe each box center at a y
        // inside the 40px footer.
        let p = panel();
        let y = p.h - 20.0 * 2.0; // 20 logical up from the bottom
        let center = |i: f32| (15.0 + i * 34.0 + 17.0) * 2.0;
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
            group_label: None,
            tab_running: &[],
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
