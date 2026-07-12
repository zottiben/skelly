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
//! chosen mode persists to `config.sidebar.mode` (Hard rule 1). Deferred to later slices:
//! the workspace switcher, command input, pinned grid, collapsible groups, and the
//! utility bar; per-tab cwd/branch titling (tabs are numbered today).

use skelly_config::SidebarMode;
use skelly_render::{ChromeQuad, FontRole, ProseLabel, PxRect, Srgb, TextMeasure, Theme};

/// Layout constants in **logical** px (multiplied by the DPI scale when placed). Tuned to
/// the guide's §08 sidebar: a compact group header, comfortable 13px `label` tab rows, and
/// a matching "+ New tab" action.
const PAD_TOP: f32 = 10.0;
/// Height of the group-header block (a `micro` uppercase label + breathing room).
const HEADER_H: f32 = 24.0;
/// Height of an overflow indicator row (`↑ N more` / `↓ N more`).
const IND_H: f32 = 16.0;
/// Height of a tab row (and the "+ New tab" action).
const TAB_H: f32 = 28.0;
/// Bottom padding beneath the new-tab action.
const PAD_BOTTOM: f32 = 10.0;
/// Horizontal inset (logical px) of a full-panel label from the sidebar edge (content pad).
const LABEL_INSET: f32 = 12.0;
/// Horizontal inset of the active-tab pill from the sidebar edges.
const PILL_INSET: f32 = 6.0;
/// The active tab's `accent` bar width (logical px).
const BAR_W: f32 = 2.0;
/// Corner radius (logical px) of the active-tab pill (the guide's `sm` radius: tab items).
const PILL_RADIUS: f32 = 6.0;

/// What a click on a sidebar row targets.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub(crate) enum Hit {
    /// Switch to the tab at this 0-based index.
    Tab(usize),
    /// Open a new tab.
    NewTab,
}

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
    /// The group / brand header (the guide's group label).
    Header,
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
fn rows_layout(count: usize, active: usize, panel_h: f32) -> Vec<Row> {
    let mut rows = Vec::new();
    let mut y = PAD_TOP;
    rows.push(Row {
        top: y,
        height: HEADER_H,
        kind: RowKind::Header,
    });
    y += HEADER_H;

    // Capacity for tab rows, reserving both overflow-indicator slots + the new-tab action.
    let reserved_below = IND_H + TAB_H + PAD_BOTTOM;
    let avail = panel_h - y - IND_H - reserved_below;
    let capacity = (avail / TAB_H).floor().max(1.0) as usize;
    let visible = count.min(capacity);
    let first = if count <= visible {
        0
    } else {
        active.saturating_sub(visible - 1).min(count - visible)
    };
    let more_above = first;
    let more_below = count - first - visible;

    rows.push(Row {
        top: y,
        height: IND_H,
        kind: RowKind::OverflowUp(more_above),
    });
    y += IND_H;
    for index in first..first + visible {
        rows.push(Row {
            top: y,
            height: TAB_H,
            kind: RowKind::Tab(index),
        });
        y += TAB_H;
    }
    rows.push(Row {
        top: y,
        height: IND_H,
        kind: RowKind::OverflowDown(more_below),
    });
    y += IND_H;
    rows.push(Row {
        top: y,
        height: TAB_H,
        kind: RowKind::NewTab,
    });
    rows
}

/// Map a click at physical `py` (relative to the surface top) to a sidebar action, for
/// `count` tabs with `active` selected in a panel `panel_h` tall (physical px) at DPI
/// `scale`. Only tab rows and the new-tab action are hittable; the header, spacers, and
/// overflow indicators map to nothing. Shares [`rows_layout`] with [`build`] so a click
/// lands on exactly the tab drawn there, scroll offset included.
pub(crate) fn hit(count: usize, active: usize, panel_h: f32, scale: f32, py: f32) -> Option<Hit> {
    let y_logical = py / scale;
    for row in rows_layout(count, active, panel_h / scale) {
        if y_logical >= row.top && y_logical < row.top + row.height {
            return match row.kind {
                RowKind::Tab(index) => Some(Hit::Tab(index)),
                RowKind::NewTab => Some(Hit::NewTab),
                _ => None,
            };
        }
    }
    None
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

/// Build the sidebar's display list for `count` tabs with `active` selected, filling
/// `panel` (physical px) at DPI `scale`, in the guide's fonts + `theme` tokens. `rail`
/// picks the slim 56px icon rail (a brand mark, centered tab numbers, a centered `+`) over
/// the full panel; both share [`rows_layout`] so [`hit`] stays valid. `measure` is used for
/// horizontal placement (centering rail glyphs).
pub(crate) fn build(
    count: usize,
    active: usize,
    panel: PxRect,
    rail: bool,
    scale: f32,
    theme: &Theme,
    measure: &mut TextMeasure,
) -> Paint {
    let mut quads = vec![ChromeQuad::fill(panel, theme.bg_sidebar)];
    let mut labels = Vec::new();

    let ctx = RowCtx {
        panel,
        active,
        rail,
        scale,
        theme,
    };
    for row in rows_layout(count, active, panel.h / scale) {
        push_row(&mut quads, &mut labels, row, &ctx, measure);
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
    scale: f32,
    theme: &'a Theme,
}

/// Render one laid-out row into `quads` + `labels`: the group header, a tab (with its active
/// marks + label), an overflow indicator, or the new-tab action.
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
        RowKind::Header => {
            // A micro group label (uppercase, per the guide's §08 group header); the real
            // "cwd · branch" title arrives with per-tab shell tracking.
            let text = if ctx.rail { "SK" } else { "SKELLY" };
            place(labels, measure, text, FontRole::Micro, ctx.theme.fg_muted);
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
            let text = if ctx.rail {
                (index + 1).to_string()
            } else {
                format!("Tab {}", index + 1)
            };
            place(labels, measure, &text, FontRole::Label, color);
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

/// The active tab's marks: a rounded `accent.subtle` pill inset from the sidebar edges and a
/// 2px `accent` bar down the sidebar's left edge (design §08 "Active tab").
fn push_active_marks(
    quads: &mut Vec<ChromeQuad>,
    panel: PxRect,
    top: f32,
    height: f32,
    scale: f32,
    theme: &Theme,
) {
    let inset = PILL_INSET * scale;
    let pill = PxRect {
        x: panel.x + inset,
        y: top,
        w: (panel.w - 2.0 * inset).max(0.0),
        h: height,
    };
    quads.push(ChromeQuad::tint(
        pill,
        theme.accent,
        0.14,
        PILL_RADIUS * scale,
    ));
    quads.push(ChromeQuad::fill(
        PxRect {
            x: panel.x,
            y: top,
            w: (BAR_W * scale).max(1.0),
            h: height,
        },
        theme.accent,
    ));
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
    use super::{build, hit, Hit, Sidebar};
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
        // rebuilding the same layout the renderer uses.
        let p = panel();
        // A y inside the header maps to nothing.
        assert_eq!(hit(3, 0, p.h, 2.0, 12.0 * 2.0), None);
        // The first tab band sits just below the header + overflow slot: PAD_TOP(10) +
        // HEADER_H(24) + IND_H(16) = 50 logical, first tab spans 50..78. Center ~64 logical.
        assert_eq!(hit(3, 0, p.h, 2.0, 64.0 * 2.0), Some(Hit::Tab(0)));
        assert_eq!(hit(3, 0, p.h, 2.0, (64.0 + 28.0) * 2.0), Some(Hit::Tab(1)));
    }

    #[test]
    fn build_lists_every_tab_and_marks_the_active_one() {
        let theme = Theme::resolve("ossein-dark");
        let mut m = TextMeasure::new(2.0);
        let paint = build(3, 1, panel(), false, 2.0, &theme, &mut m);
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
    fn rail_centers_the_tab_number() {
        let theme = Theme::resolve("ossein-dark");
        let mut m = TextMeasure::new(2.0);
        let p = PxRect {
            x: 0.0,
            y: 0.0,
            w: 56.0 * 2.0,
            h: 800.0 * 2.0,
        };
        let paint = build(3, 1, p, true, 2.0, &theme, &mut m);
        // Rail tab labels are the bare numbers, centered (x offset from the left edge > 0).
        let two = paint.labels.iter().find(|l| l.text == "2").unwrap();
        assert!(two.x > 0.0 && two.x < p.w);
    }
}
