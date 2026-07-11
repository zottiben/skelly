//! The command palette: a centered overlay listing runnable commands, filtered by a
//! typed query and navigated by keyboard (AGENTS Hard rule 4 - an overlay over the
//! live terminal, never a route). This module is pure state + view-building: it owns
//! the query, the filtered selection, and how the palette renders as a monospace grid
//! of UI-token-colored cells. The binary owns opening it, routing keys, and executing
//! the chosen command.
//!
//! The built-in [`COMMANDS`] set is the seed of the keybinding registry; merging user
//! `[keys]` overrides + surfacing tabs/themes/files is a later slice.

use skelly_render::{GridCell, Srgb, Theme};

/// A runnable command surfaced in the palette.
pub(crate) struct Command {
    /// The human label, shown left-aligned.
    pub(crate) label: &'static str,
    /// The default key-chord hint, shown right-aligned in muted text.
    pub(crate) hint: &'static str,
    /// What running it does.
    pub(crate) action: Action,
}

/// What a command does when run. The binary maps each to its existing handlers.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub(crate) enum Action {
    /// Split the focused pane to the right.
    SplitRight,
    /// Split the focused pane downward.
    SplitDown,
    /// Toggle zoom on the focused pane.
    Zoom,
    /// Reset every split to 50/50.
    EvenOut,
    /// Close the focused pane.
    ClosePane,
    /// Move focus left / down / up / right.
    FocusLeft,
    /// Move focus down.
    FocusDown,
    /// Move focus up.
    FocusUp,
    /// Move focus right.
    FocusRight,
    /// Switch the UI theme to Ossein Dark.
    ThemeDark,
    /// Switch the UI theme to Ossein Light.
    ThemeLight,
    /// Quit the application.
    Quit,
}

/// The built-in command set. Order is the display order.
pub(crate) const COMMANDS: &[Command] = &[
    Command {
        label: "Split pane right",
        hint: "opt |",
        action: Action::SplitRight,
    },
    Command {
        label: "Split pane down",
        hint: "opt -",
        action: Action::SplitDown,
    },
    Command {
        label: "Zoom / unzoom pane",
        hint: "opt Z",
        action: Action::Zoom,
    },
    Command {
        label: "Even out splits",
        hint: "opt =",
        action: Action::EvenOut,
    },
    Command {
        label: "Close pane",
        hint: "opt W",
        action: Action::ClosePane,
    },
    Command {
        label: "Focus pane left",
        hint: "opt H",
        action: Action::FocusLeft,
    },
    Command {
        label: "Focus pane down",
        hint: "opt J",
        action: Action::FocusDown,
    },
    Command {
        label: "Focus pane up",
        hint: "opt K",
        action: Action::FocusUp,
    },
    Command {
        label: "Focus pane right",
        hint: "opt L",
        action: Action::FocusRight,
    },
    Command {
        label: "Theme: Ossein Dark",
        hint: "",
        action: Action::ThemeDark,
    },
    Command {
        label: "Theme: Ossein Light",
        hint: "",
        action: Action::ThemeLight,
    },
    Command {
        label: "Quit skelly",
        hint: "cmd Q",
        action: Action::Quit,
    },
];

/// The rendered palette: the monospace text grid plus where the selection highlight
/// and input caret go, for a [`skelly_render::OverlayView`].
pub(crate) struct View {
    /// The palette's lines as a grid of UI-colored cells.
    pub(crate) rows: Vec<Vec<GridCell>>,
    /// The grid row to highlight (the selected command), if any results.
    pub(crate) selected_row: Option<usize>,
    /// The input caret's `(column, row)` cell.
    pub(crate) caret: (usize, usize),
}

/// Palette state: whether it is open, the query, and the selected match.
pub(crate) struct Palette {
    /// Whether the palette overlay is showing.
    pub(crate) open: bool,
    query: String,
    selected: usize,
}

impl Palette {
    /// A closed, empty palette.
    pub(crate) fn new() -> Self {
        Self {
            open: false,
            query: String::new(),
            selected: 0,
        }
    }

    /// Open the palette fresh (empty query, first match selected).
    pub(crate) fn open(&mut self) {
        self.open = true;
        self.query.clear();
        self.selected = 0;
    }

    /// Close the palette.
    pub(crate) fn close(&mut self) {
        self.open = false;
    }

    /// The indices into [`COMMANDS`] whose label matches the query (case-insensitive
    /// substring). An empty query matches everything.
    pub(crate) fn matches(&self) -> Vec<usize> {
        let q = self.query.trim().to_lowercase();
        COMMANDS
            .iter()
            .enumerate()
            .filter(|(_, cmd)| q.is_empty() || cmd.label.to_lowercase().contains(&q))
            .map(|(index, _)| index)
            .collect()
    }

    /// Move the selection by `delta`, clamped to the current match count.
    pub(crate) fn move_selection(&mut self, delta: i32) {
        let count = self.matches().len();
        if count == 0 {
            self.selected = 0;
            return;
        }
        let current = i32::try_from(self.selected).unwrap_or(i32::MAX);
        let last = i32::try_from(count - 1).unwrap_or(i32::MAX);
        let next = (current + delta).clamp(0, last);
        self.selected = usize::try_from(next).unwrap_or(0);
    }

    /// Append a typed character to the query and reset the selection.
    pub(crate) fn push_char(&mut self, c: char) {
        self.query.push(c);
        self.selected = 0;
    }

    /// Delete the last query character and reset the selection.
    pub(crate) fn backspace(&mut self) {
        self.query.pop();
        self.selected = 0;
    }

    /// The action of the currently selected match, if any.
    pub(crate) fn selected_action(&self) -> Option<Action> {
        self.matches()
            .get(self.selected)
            .map(|&i| COMMANDS[i].action)
    }

    /// Render the palette to a grid in `theme`'s UI tokens: a prompt line, a result
    /// count, one line per match, a spacer, and a footer of key hints. The grid width
    /// is the widest line (so nothing clips), floored at a comfortable minimum and
    /// capped at `max_cols` (the panel must fit the window).
    pub(crate) fn view(&self, max_cols: usize, theme: &Theme) -> View {
        let cols = self.natural_cols().clamp(28, max_cols.max(28));
        let mut rows: Vec<Vec<GridCell>> = Vec::new();

        // Prompt line: "> " then the query (or a placeholder when empty).
        let mut prompt = vec![cell('>', theme.accent), cell(' ', theme.fg_primary)];
        let caret_col = prompt.len() + self.query.chars().count();
        if self.query.is_empty() {
            prompt.extend(text_cells("search commands", theme.fg_muted));
        } else {
            prompt.extend(text_cells(&self.query, theme.fg_primary));
        }
        rows.push(pad_to(prompt, cols, theme.fg_muted));

        let matches = self.matches();
        rows.push(count_row(matches.len(), cols, theme.fg_muted));

        let first_command_row = rows.len();
        for &index in &matches {
            let cmd = &COMMANDS[index];
            rows.push(command_row(
                cmd.label,
                cmd.hint,
                cols,
                theme.fg_primary,
                theme.fg_muted,
            ));
        }

        rows.push(pad_to(Vec::new(), cols, theme.fg_muted)); // spacer
        rows.push(pad_to(
            text_cells(FOOTER, theme.fg_muted),
            cols,
            theme.fg_muted,
        ));

        let selected_row = (!matches.is_empty()).then_some(first_command_row + self.selected);
        View {
            rows,
            selected_row,
            caret: (caret_col, 0),
        }
    }

    /// The natural grid width: the widest line the palette wants to draw (the footer,
    /// or the longest matched `label + hint` command row), plus side margins.
    fn natural_cols(&self) -> usize {
        let mut widest = FOOTER.chars().count();
        for &index in &self.matches() {
            let cmd = &COMMANDS[index];
            // 2 indent + label + 1 gap + hint + 1 right margin.
            let width = 2 + cmd.label.chars().count() + 1 + cmd.hint.chars().count() + 1;
            widest = widest.max(width);
        }
        widest
    }
}

/// The footer hint line - also the palette's minimum width.
const FOOTER: &str = "up/down navigate    enter run    esc close";

impl Default for Palette {
    fn default() -> Self {
        Self::new()
    }
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

/// A string as a run of same-colored cells.
fn text_cells(s: &str, fg: Srgb) -> Vec<GridCell> {
    s.chars().map(|c| cell(c, fg)).collect()
}

/// Pad (or truncate) `row` to exactly `cols` cells with spaces.
fn pad_to(mut row: Vec<GridCell>, cols: usize, space_fg: Srgb) -> Vec<GridCell> {
    row.truncate(cols);
    while row.len() < cols {
        row.push(cell(' ', space_fg));
    }
    row
}

/// The "N result(s)" line, indented and muted.
fn count_row(n: usize, cols: usize, fg: Srgb) -> Vec<GridCell> {
    let text = if n == 1 {
        "  1 result".to_owned()
    } else {
        format!("  {n} results")
    };
    pad_to(text_cells(&text, fg), cols, fg)
}

/// A command line: an indented `label` on the left and a right-aligned `hint`.
fn command_row(
    label: &str,
    hint: &str,
    cols: usize,
    label_fg: Srgb,
    hint_fg: Srgb,
) -> Vec<GridCell> {
    let indent = 2;
    let label_chars: Vec<char> = label.chars().collect();
    let hint_chars: Vec<char> = hint.chars().collect();
    let mut row: Vec<GridCell> = Vec::with_capacity(cols);
    for _ in 0..indent {
        row.push(cell(' ', label_fg));
    }
    for &c in &label_chars {
        row.push(cell(c, label_fg));
    }
    // Pad so the hint ends one cell from the right edge.
    let hint_start = cols.saturating_sub(hint_chars.len() + 1);
    while row.len() < hint_start {
        row.push(cell(' ', label_fg));
    }
    for &c in &hint_chars {
        row.push(cell(c, hint_fg));
    }
    pad_to(row, cols, label_fg)
}

#[cfg(test)]
mod tests {
    use super::{Action, Palette, COMMANDS};

    #[test]
    fn empty_query_matches_every_command() {
        let p = Palette::new();
        assert_eq!(p.matches().len(), COMMANDS.len());
    }

    #[test]
    fn query_filters_by_label_substring_case_insensitively() {
        let mut p = Palette::new();
        p.open();
        for c in "zoom".chars() {
            p.push_char(c);
        }
        let matches = p.matches();
        assert_eq!(matches.len(), 1);
        assert_eq!(COMMANDS[matches[0]].action, Action::Zoom);
        assert_eq!(p.selected_action(), Some(Action::Zoom));
    }

    #[test]
    fn selection_clamps_within_matches() {
        let mut p = Palette::new();
        p.open();
        p.move_selection(-1); // cannot go below 0
        assert_eq!(p.selected_action(), Some(COMMANDS[0].action));
        p.move_selection(1000); // clamps to the last match
        assert_eq!(
            p.selected_action(),
            Some(COMMANDS[COMMANDS.len() - 1].action)
        );
    }

    #[test]
    fn filtering_resets_selection_and_narrows() {
        let mut p = Palette::new();
        p.open();
        p.move_selection(3);
        for c in "focus".chars() {
            p.push_char(c);
        }
        // "focus" matches the four focus commands; selection reset to the first.
        assert_eq!(p.matches().len(), 4);
        assert_eq!(p.selected_action(), Some(Action::FocusLeft));
    }

    #[test]
    fn a_no_match_query_has_no_action() {
        let mut p = Palette::new();
        p.open();
        for c in "zzzz".chars() {
            p.push_char(c);
        }
        assert!(p.matches().is_empty());
        assert_eq!(p.selected_action(), None);
    }

    #[test]
    fn theme_query_surfaces_both_theme_commands() {
        let mut p = Palette::new();
        p.open();
        for c in "theme".chars() {
            p.push_char(c);
        }
        let actions: Vec<Action> = p.matches().iter().map(|&i| COMMANDS[i].action).collect();
        assert_eq!(actions, vec![Action::ThemeDark, Action::ThemeLight]);
    }

    #[test]
    fn backspace_widens_the_match_set_again() {
        let mut p = Palette::new();
        p.open();
        for c in "zoomx".chars() {
            p.push_char(c);
        }
        assert!(p.matches().is_empty());
        p.backspace();
        assert_eq!(p.matches().len(), 1);
    }
}
