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

/// The tab list windowed into the available height (design §12 "Many tabs overflow").
/// The header stays pinned at the top and the `+ New tab` action sits directly below the
/// visible window - so when the list overflows and the window fills the height, the action
/// lands at the bottom; only the tab rows between them scroll. Built once and shared by
/// [`view`] and [`hit`] so the rendered rows and the click map never disagree.
struct Layout {
    /// The first visible tab index (the scroll offset).
    first: usize,
    /// How many tab rows are visible (`<= count`; at least 1 while any tab exists).
    visible: usize,
    /// Tabs hidden above the window (drives the "more above" indicator).
    more_above: usize,
    /// Tabs hidden below the window (drives the "more below" indicator).
    more_below: usize,
}

impl Layout {
    /// Window `count` tabs into `max_rows` grid rows with `active` scrolled into view.
    fn compute(count: usize, active: usize, max_rows: usize) -> Self {
        // Rows the header + the gap + the new-tab action always claim.
        let reserved = HEADER_ROWS + GAP_ROWS + 1;
        let capacity = max_rows.saturating_sub(reserved).max(1);
        let visible = count.min(capacity);
        // Keep `active` inside `[first, first + visible)`, biased to the top so a tab that
        // already fits never scrolls; once it would fall off the bottom, scroll so it sits
        // on the last visible row (auto-scroll into view).
        let first = if count <= visible {
            0
        } else {
            active.saturating_sub(visible - 1).min(count - visible)
        };
        Self {
            first,
            visible,
            more_above: first,
            more_below: count - first - visible,
        }
    }

    /// The grid row of the `+ New tab` action = header + the visible window + the gap.
    fn new_tab_row(&self) -> usize {
        HEADER_ROWS + self.visible + GAP_ROWS
    }

    /// The grid row of the active tab, or `None` when it is scrolled out of view.
    fn active_row(&self, active: usize, count: usize) -> Option<usize> {
        let shown = active < count && (self.first..self.first + self.visible).contains(&active);
        shown.then_some(HEADER_ROWS + (active - self.first))
    }
}

/// Map a clicked grid `row` to a sidebar action for `count` tabs windowed into `max_rows`
/// rows with `active` selected. Rows outside the visible tab window and the new-tab action
/// (the header, spacers, overflow indicators) hit nothing. Shares [`Layout`] with [`view`]
/// so a click lands on exactly the tab drawn there, scroll offset included.
pub(crate) fn hit(count: usize, active: usize, max_rows: usize, row: usize) -> Option<Hit> {
    let layout = Layout::compute(count, active, max_rows);
    if (HEADER_ROWS..HEADER_ROWS + layout.visible).contains(&row) {
        Some(Hit::Tab(layout.first + (row - HEADER_ROWS)))
    } else if row == layout.new_tab_row() {
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

/// Build the sidebar grid `cols` cells wide for `count` tabs with `active` selected,
/// windowed into `max_rows` grid rows so the active tab is always on screen and overflow
/// tabs scroll (design §12 "Many tabs overflow"). The `rail` flag picks the compact 56px
/// icon rail (centered tab numbers) over the full panel; both share the same row structure
/// (header, spacer, the visible tab window, spacer, new-tab) so [`hit`] and `active_row`
/// stay valid. Tabs are labeled by position (`Tab 1..`); cwd / command titling is a later
/// slice.
pub(crate) fn view(
    count: usize,
    active: usize,
    cols: usize,
    max_rows: usize,
    rail: bool,
    theme: &Theme,
) -> View {
    let layout = Layout::compute(count, active, max_rows);
    let cols = cols.max(if rail { 1 } else { INDENT + 1 });
    let place: fn(&str, usize, Srgb) -> Vec<GridCell> = if rail { centered } else { indented };

    let mut rows: Vec<Vec<GridCell>> = Vec::new();

    // Header (a quiet brand mark) stays pinned; the row below it doubles as the
    // "more above" scroll indicator = `HEADER_ROWS` rows.
    rows.push(place(
        if rail { "sk" } else { "skelly" },
        cols,
        theme.fg_secondary,
    ));
    rows.push(overflow_row(
        layout.more_above,
        true,
        rail,
        cols,
        place,
        theme,
    ));

    // The visible window of tabs; the active tab's label is primary, the rest secondary.
    for index in layout.first..layout.first + layout.visible {
        let fg = if index == active {
            theme.fg_primary
        } else {
            theme.fg_secondary
        };
        let label = if rail {
            (index + 1).to_string()
        } else {
            format!("Tab {}", index + 1)
        };
        rows.push(place(&label, cols, fg));
    }

    // The gap spacer doubles as the "more below" indicator, then the new-tab action.
    rows.push(overflow_row(
        layout.more_below,
        false,
        rail,
        cols,
        place,
        theme,
    ));
    rows.push(place(
        if rail { "+" } else { "+ New tab" },
        cols,
        theme.fg_muted,
    ));

    View {
        rows,
        active_row: layout.active_row(active, count),
    }
}

/// A spacer row that shows a scroll indicator when `hidden` tabs lie past the window in
/// the `up` (else down) direction, and stays blank otherwise. The indicator rows are ones
/// [`hit`] maps to nothing, so they never intercept a click. The arrow (`↑`/`↓`) is a
/// single-width glyph in the terminal's Nerd Font, keeping the cell grid aligned.
fn overflow_row(
    hidden: usize,
    up: bool,
    rail: bool,
    cols: usize,
    place: fn(&str, usize, Srgb) -> Vec<GridCell>,
    theme: &Theme,
) -> Vec<GridCell> {
    if hidden == 0 {
        return pad_to(Vec::new(), cols, theme.fg_muted);
    }
    let arrow = if up { '↑' } else { '↓' };
    // The rail is too narrow for a count; the full panel spells it out.
    let text = if rail {
        arrow.to_string()
    } else {
        format!("{arrow} {hidden} more")
    };
    place(&text, cols, theme.fg_muted)
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
    use super::{hit, view, Hit, Sidebar, HEADER_ROWS, INDENT};
    use skelly_config::SidebarMode;
    use skelly_render::Theme;

    /// A generous height that fits any small tab list, so the list never scrolls.
    const AMPLE: usize = 40;

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
        // Three tabs, ample height (no scroll): header rows hit nothing; the three tab
        // rows map in order; the spacer after them hits nothing; then the "+ New tab" row.
        assert_eq!(hit(3, 0, AMPLE, 0), None);
        assert_eq!(hit(3, 0, AMPLE, HEADER_ROWS), Some(Hit::Tab(0)));
        assert_eq!(hit(3, 0, AMPLE, HEADER_ROWS + 2), Some(Hit::Tab(2)));
        assert_eq!(hit(3, 0, AMPLE, HEADER_ROWS + 3), None); // the gap spacer
        assert_eq!(hit(3, 0, AMPLE, HEADER_ROWS + 4), Some(Hit::NewTab));
        assert_eq!(hit(3, 0, AMPLE, 99), None);
    }

    #[test]
    fn view_lists_every_tab_and_marks_the_active_row() {
        let theme = Theme::resolve("ossein-dark");
        let v = view(3, 1, 20, AMPLE, false, &theme);
        // header + spacer + 3 tabs + spacer + new-tab = 7 rows.
        assert_eq!(v.rows.len(), 7);
        assert_eq!(v.active_row, Some(HEADER_ROWS + 1));
        // The active tab (index 1) is drawn in the primary color; an inactive one isn't.
        let active_glyph = v.rows[HEADER_ROWS + 1][INDENT].fg; // first label cell after indent
        let inactive_glyph = v.rows[HEADER_ROWS][INDENT].fg;
        assert_eq!(active_glyph, theme.fg_primary);
        assert_eq!(inactive_glyph, theme.fg_secondary);
    }

    #[test]
    fn rail_view_centers_tab_numbers_and_keeps_the_shared_row_layout() {
        let theme = Theme::resolve("ossein-dark");
        let v = view(3, 1, 5, AMPLE, true, &theme);
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

    #[test]
    fn view_windows_tabs_and_scrolls_the_active_into_view() {
        let theme = Theme::resolve("ossein-dark");
        // 10 tabs, active = index 8, only room for a 6-tab window (max_rows 10, reserving
        // header + gap + new-tab = 4). The window scrolls to first = 3 -> [3, 9).
        let v = view(10, 8, 20, 10, false, &theme);
        // header + spacer(indicator) + 6 tabs + spacer(indicator) + new-tab = 10 rows.
        assert_eq!(v.rows.len(), 10);
        // The active tab is visible, highlighted, and shows its label ("Tab 9", index 8).
        assert_eq!(v.active_row, Some(7));
        assert_eq!(v.rows[7][INDENT].fg, theme.fg_primary);
        let active_text: String = v.rows[7].iter().map(|c| c.c).collect();
        assert!(active_text.contains("Tab 9"), "got {active_text:?}");
        // Both overflow indicators appear: 3 tabs hidden above, 1 below.
        let above: String = v.rows[1].iter().map(|c| c.c).collect();
        let below: String = v.rows[8].iter().map(|c| c.c).collect();
        assert!(above.contains('↑') && above.contains('3'), "got {above:?}");
        assert!(below.contains('↓') && below.contains('1'), "got {below:?}");
    }

    #[test]
    fn hit_maps_through_the_scroll_offset() {
        // Same window as above (10 tabs, active 8, room for 6, scrolled to first = 3): the
        // first visible tab row is tab index 3, and clicks land on the scrolled indices.
        assert_eq!(hit(10, 8, 10, HEADER_ROWS), Some(Hit::Tab(3)));
        assert_eq!(hit(10, 8, 10, HEADER_ROWS + 5), Some(Hit::Tab(8)));
        assert_eq!(hit(10, 8, 10, HEADER_ROWS + 6), None); // the "more below" spacer
        assert_eq!(hit(10, 8, 10, 9), Some(Hit::NewTab)); // the pinned new-tab action
    }

    #[test]
    fn a_tab_that_already_fits_does_not_scroll() {
        let theme = Theme::resolve("ossein-dark");
        // active = index 1 fits in the first 6-tab window, so first = 0: no "more above"
        // indicator, but the remaining 4 tabs still show a "more below" indicator.
        let v = view(10, 1, 20, 10, false, &theme);
        assert_eq!(v.active_row, Some(HEADER_ROWS + 1));
        let above: String = v.rows[1].iter().map(|c| c.c).collect();
        let below: String = v.rows[8].iter().map(|c| c.c).collect();
        assert!(
            !above.contains('↑'),
            "no scroll-up indicator at the top: {above:?}"
        );
        assert!(below.contains('↓') && below.contains('4'), "got {below:?}");
    }
}
