//! The persistent left sidebar: the vertical tab list (AGENTS Hard rule 4 - chrome
//! that layers over the always-present pane tree, never a route). This module is pure
//! state + view-building: it owns the show/hide flag and turns the tab list into a
//! monospace grid of UI-token-colored cells, plus the row -> action hit-test the
//! binary uses for clicks. The binary owns toggling it, geometry, and switching tabs.
//!
//! Two display modes (design §08 "Sidebar modes"): the full-width panel listing tabs
//! (active highlighted) with a "+ New tab" action, and the slim 56px icon rail with
//! compact centered tab numbers. `⌘B` shows/hides; `⇧⌘B` cycles full <-> rail. The
//! chosen mode persists to `config.sidebar.mode` (Hard rule 1). Deferred: hover-to-expand
//! the rail, the pinned grid, collapsible groups, drag-reorder, and the footer icons.

use skelly_config::SidebarMode;
use skelly_render::{GridCell, Srgb, Theme};

/// Grid rows above the tab list: a brand header and a blank spacer.
const HEADER_ROWS: usize = 2;
/// Blank rows between the tab list and the "+ New tab" action.
const GAP_ROWS: usize = 1;
/// Left indent (cells) for every sidebar line, leaving room for the active accent bar.
const INDENT: usize = 2;

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

/// The grid row of the "+ New tab" action, given `count` tabs.
fn new_tab_row(count: usize) -> usize {
    HEADER_ROWS + count + GAP_ROWS
}

/// Map a clicked grid `row` to a sidebar action, given `count` open tabs. Rows outside
/// the tab list and the new-tab action (the header, spacers) hit nothing.
pub(crate) fn hit(count: usize, row: usize) -> Option<Hit> {
    if (HEADER_ROWS..HEADER_ROWS + count).contains(&row) {
        Some(Hit::Tab(row - HEADER_ROWS))
    } else if row == new_tab_row(count) {
        Some(Hit::NewTab)
    } else {
        None
    }
}

/// The rendered sidebar grid plus the active tab's grid row (for the highlight quad).
pub(crate) struct View {
    /// The sidebar's lines as a grid of UI-colored cells.
    pub(crate) rows: Vec<Vec<GridCell>>,
    /// The grid row of the active tab, to draw the accent bar + subtle fill behind.
    pub(crate) active_row: Option<usize>,
}

/// Build the sidebar grid `cols` cells wide for `count` tabs with `active` selected. The
/// `rail` flag picks the compact 56px icon rail (centered tab numbers) over the full
/// panel. Both layouts share the same row structure (header, spacer, one row per tab,
/// spacer, new-tab), so [`hit`] and `active_row` are identical. Tabs are labeled by
/// position (`Tab 1..`); cwd / command titling is a later slice.
pub(crate) fn view(count: usize, active: usize, cols: usize, rail: bool, theme: &Theme) -> View {
    if rail {
        rail_view(count, active, cols, theme)
    } else {
        full_view(count, active, cols, theme)
    }
}

/// The full-width panel: a `skelly` brand header, one `Tab N` row per tab, and a
/// `+ New tab` action, all left-indented past the active-tab accent bar.
fn full_view(count: usize, active: usize, cols: usize, theme: &Theme) -> View {
    let cols = cols.max(INDENT + 1);
    let mut rows: Vec<Vec<GridCell>> = Vec::new();

    // Header (a quiet brand mark), then a blank spacer = `HEADER_ROWS` rows.
    rows.push(indented("skelly", cols, theme.fg_secondary));
    rows.push(pad_to(Vec::new(), cols, theme.fg_muted));

    // One row per tab; the active tab's label is primary, the rest secondary.
    for index in 0..count {
        let fg = if index == active {
            theme.fg_primary
        } else {
            theme.fg_secondary
        };
        rows.push(indented(&format!("Tab {}", index + 1), cols, fg));
    }

    rows.push(pad_to(Vec::new(), cols, theme.fg_muted)); // GAP_ROWS spacer
    rows.push(indented("+ New tab", cols, theme.fg_muted));

    let active_row = (active < count).then_some(HEADER_ROWS + active);
    View { rows, active_row }
}

/// The slim 56px icon rail: a compact `sk` brand mark, the 1-based tab numbers centered
/// (the active number in primary, the rest secondary, with the renderer's full-width
/// accent highlight behind it), and a centered `+` new-tab action.
fn rail_view(count: usize, active: usize, cols: usize, theme: &Theme) -> View {
    let cols = cols.max(1);
    let mut rows: Vec<Vec<GridCell>> = Vec::new();

    rows.push(centered("sk", cols, theme.fg_secondary));
    rows.push(pad_to(Vec::new(), cols, theme.fg_muted));

    for index in 0..count {
        let fg = if index == active {
            theme.fg_primary
        } else {
            theme.fg_secondary
        };
        rows.push(centered(&(index + 1).to_string(), cols, fg));
    }

    rows.push(pad_to(Vec::new(), cols, theme.fg_muted)); // GAP_ROWS spacer
    rows.push(centered("+", cols, theme.fg_muted));

    let active_row = (active < count).then_some(HEADER_ROWS + active);
    View { rows, active_row }
}

/// One UI cell: a character in `fg`, no background or attributes.
fn cell(c: char, fg: Srgb) -> GridCell {
    GridCell {
        c,
        fg,
        bg: None,
        bold: false,
        italic: false,
        underline: false,
    }
}

/// An `INDENT`-indented line of `text` in `fg`, padded to `cols`.
fn indented(text: &str, cols: usize, fg: Srgb) -> Vec<GridCell> {
    let mut row: Vec<GridCell> = (0..INDENT).map(|_| cell(' ', fg)).collect();
    row.extend(text.chars().map(|c| cell(c, fg)));
    pad_to(row, cols, fg)
}

/// A line of `text` horizontally centered within `cols` cells in `fg` (for the rail).
fn centered(text: &str, cols: usize, fg: Srgb) -> Vec<GridCell> {
    let left = cols.saturating_sub(text.chars().count()) / 2;
    let mut row: Vec<GridCell> = (0..left).map(|_| cell(' ', fg)).collect();
    row.extend(text.chars().map(|c| cell(c, fg)));
    pad_to(row, cols, fg)
}

/// Pad (or truncate) `row` to exactly `cols` cells with spaces.
fn pad_to(mut row: Vec<GridCell>, cols: usize, space_fg: Srgb) -> Vec<GridCell> {
    row.truncate(cols);
    while row.len() < cols {
        row.push(cell(' ', space_fg));
    }
    row
}

#[cfg(test)]
mod tests {
    use super::{hit, view, Hit, Sidebar, HEADER_ROWS};
    use skelly_config::SidebarMode;
    use skelly_render::Theme;

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
        // From the rail, ⌘B hides it, and ⌘B again brings the rail back (not the full
        // panel) - hide/show preserves the rail-vs-full choice.
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
        assert_eq!(s.mode(), SidebarMode::Autohide); // full -> rail
        s.cycle_rail();
        assert_eq!(s.mode(), SidebarMode::Fixed); // rail -> full
                                                  // From hidden, ⇧⌘B brings the sidebar back as the full panel.
        s.toggle();
        assert_eq!(s.mode(), SidebarMode::Hidden);
        s.cycle_rail();
        assert_eq!(s.mode(), SidebarMode::Fixed);
    }

    #[test]
    fn set_mode_tracks_the_config_and_keeps_a_visible_recall_target() {
        // Settings writes hidden; a later ⌘B recall should return to the last visible
        // mode (Fixed by default here), never re-hide.
        let mut s = Sidebar::new(SidebarMode::Fixed);
        s.set_mode(SidebarMode::Hidden);
        assert!(!s.visible());
        s.toggle();
        assert_eq!(s.mode(), SidebarMode::Fixed);
    }

    #[test]
    fn hit_maps_rows_to_tabs_then_the_new_tab_action() {
        // Three tabs: header rows hit nothing; the three tab rows map in order; the
        // spacer after them hits nothing; then the "+ New tab" row. Shared by both modes.
        assert_eq!(hit(3, 0), None);
        assert_eq!(hit(3, HEADER_ROWS), Some(Hit::Tab(0)));
        assert_eq!(hit(3, HEADER_ROWS + 2), Some(Hit::Tab(2)));
        assert_eq!(hit(3, HEADER_ROWS + 3), None); // the gap spacer
        assert_eq!(hit(3, HEADER_ROWS + 4), Some(Hit::NewTab));
        assert_eq!(hit(3, 99), None);
    }

    #[test]
    fn view_lists_every_tab_and_marks_the_active_row() {
        let theme = Theme::resolve("ossein-dark");
        let v = view(3, 1, 20, false, &theme);
        // header + spacer + 3 tabs + spacer + new-tab = 7 rows.
        assert_eq!(v.rows.len(), 7);
        assert_eq!(v.active_row, Some(HEADER_ROWS + 1));
        // The active tab (index 1) is drawn in the primary color; an inactive one isn't.
        let active_glyph = v.rows[HEADER_ROWS + 1][2].fg; // first label cell after indent
        let inactive_glyph = v.rows[HEADER_ROWS][2].fg;
        assert_eq!(active_glyph, theme.fg_primary);
        assert_eq!(inactive_glyph, theme.fg_secondary);
    }

    #[test]
    fn rail_view_centers_tab_numbers_and_keeps_the_shared_row_layout() {
        let theme = Theme::resolve("ossein-dark");
        let v = view(3, 1, 5, true, &theme);
        // Same 7-row structure as the full panel, so `hit`/`active_row` stay valid.
        assert_eq!(v.rows.len(), 7);
        assert_eq!(v.active_row, Some(HEADER_ROWS + 1));
        // Tab 2 (index 1) is the number "2" centered in 5 cells => col 2.
        let text: String = v.rows[HEADER_ROWS + 1].iter().map(|c| c.c).collect();
        assert_eq!(text, "  2  ");
        assert_eq!(v.rows[HEADER_ROWS + 1][2].fg, theme.fg_primary);
        // The new-tab action is a centered "+".
        let last: String = v.rows[6].iter().map(|c| c.c).collect();
        assert_eq!(last, "  +  ");
    }
}
