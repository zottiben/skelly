//! The persistent left sidebar: the vertical tab list (AGENTS Hard rule 4 - chrome
//! that layers over the always-present pane tree, never a route). This module is pure
//! state + view-building: it owns the show/hide flag and turns the tab list into a
//! monospace grid of UI-token-colored cells, plus the row -> action hit-test the
//! binary uses for clicks. The binary owns toggling it, geometry, and switching tabs.
//!
//! This is the first sidebar slice: a fixed-width panel listing tabs (active
//! highlighted) and a "+ New tab" action. Deferred: the `⇧⌘B` slim rail, the pinned
//! grid, collapsible groups, drag-reorder, and the footer action icons.

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

/// The persistent left sidebar's state. For now just a show/hide flag; the rail mode,
/// groups, and pinning are later slices.
pub(crate) struct Sidebar {
    /// Whether the sidebar is currently shown (`⌘B` toggles it).
    pub(crate) visible: bool,
}

impl Sidebar {
    /// Start from the configured mode: shown unless the mode is `hidden`.
    pub(crate) fn new(mode: SidebarMode) -> Self {
        Self {
            visible: !matches!(mode, SidebarMode::Hidden),
        }
    }

    /// Toggle the sidebar's visibility (`⌘B`).
    pub(crate) fn toggle(&mut self) {
        self.visible = !self.visible;
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

/// Build the sidebar grid `cols` cells wide for `count` tabs with `active` selected.
/// Tabs are labeled by position (`Tab 1..`); cwd / command titling is a later slice.
pub(crate) fn view(count: usize, active: usize, cols: usize, theme: &Theme) -> View {
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
    use super::{hit, view, Hit, HEADER_ROWS};
    use skelly_config::SidebarMode;
    use skelly_render::Theme;

    #[test]
    fn new_respects_the_configured_mode() {
        assert!(super::Sidebar::new(SidebarMode::Fixed).visible);
        assert!(super::Sidebar::new(SidebarMode::Autohide).visible);
        assert!(!super::Sidebar::new(SidebarMode::Hidden).visible);
    }

    #[test]
    fn hit_maps_rows_to_tabs_then_the_new_tab_action() {
        // Three tabs: header rows hit nothing; the three tab rows map in order; the
        // spacer after them hits nothing; then the "+ New tab" row.
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
        let v = view(3, 1, 20, &theme);
        // header + spacer + 3 tabs + spacer + new-tab = 7 rows.
        assert_eq!(v.rows.len(), 7);
        assert_eq!(v.active_row, Some(HEADER_ROWS + 1));
        // The active tab (index 1) is drawn in the primary color; an inactive one isn't.
        let active_glyph = v.rows[HEADER_ROWS + 1][2].fg; // first label cell after indent
        let inactive_glyph = v.rows[HEADER_ROWS][2].fg;
        assert_eq!(active_glyph, theme.fg_primary);
        assert_eq!(inactive_glyph, theme.fg_secondary);
    }
}
