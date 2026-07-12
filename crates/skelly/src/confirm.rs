//! The "close with a running job" confirm modal (design §12 "Process running on close"):
//! a centered overlay that warns before a close (`⌥w` pane / `⌘W` tab) which would kill a
//! running foreground job, so a job is never destroyed silently. Confirmed with `Enter` or
//! a second press of the close chord, dismissed with `Esc`. This module is pure state +
//! view-building; the binary owns detecting the job (via `Terminal::foreground_job_pid`
//! plus the process name), routing keys, and performing the actual close.

use skelly_render::{GridCell, Srgb, Theme};

/// What a pending close would destroy.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub(crate) enum CloseTarget {
    /// The focused pane (`⌥w`).
    Pane,
    /// The active tab and every pane in it (`⌘W`).
    Tab,
}

impl CloseTarget {
    /// The noun for the confirm message.
    fn noun(self) -> &'static str {
        match self {
            CloseTarget::Pane => "pane",
            CloseTarget::Tab => "tab",
        }
    }
}

/// A pending confirm: the close it guards and the running process it would kill.
pub(crate) struct Confirm {
    /// The close this modal guards (so the binary knows what to close on confirm).
    pub(crate) target: CloseTarget,
    /// The name of the foreground job the close would end (shown in the message).
    process: String,
}

/// The rendered modal grid plus the row to highlight (none - the process name carries the
/// emphasis via `accent`).
pub(crate) struct View {
    pub(crate) rows: Vec<Vec<GridCell>>,
    pub(crate) selected_row: Option<usize>,
}

impl Confirm {
    /// A pending confirm for closing `target`, which would kill the foreground job named
    /// `process`.
    pub(crate) fn new(target: CloseTarget, process: String) -> Self {
        Self { target, process }
    }

    /// Render the modal to a grid in `theme`'s UI tokens: the running process (its name in
    /// `accent`), what the close would do, and the key hints. The grid is sized to its
    /// widest line so nothing clips.
    pub(crate) fn view(&self, theme: &Theme) -> View {
        // The lines, built as colored cell runs; line 0 mixes primary + accent.
        let mut title = text_cells("\"", theme.fg_primary);
        title.extend(text_cells(&self.process, theme.accent));
        title.extend(text_cells("\" is still running", theme.fg_primary));

        let action = format!("Close this {} and end it?", self.target.noun());
        let hint = "\u{21b5} close   esc cancel";

        let content = [
            title,
            Vec::new(),
            text_cells(&action, theme.fg_primary),
            Vec::new(),
            text_cells(hint, theme.fg_muted),
        ];

        // Width = the widest line + a one-cell margin each side, floored so short messages
        // still read as a dialog.
        let widest = content.iter().map(Vec::len).max().unwrap_or(0);
        let cols = (widest + 2 * MARGIN).max(MIN_COLS);
        let rows = content
            .into_iter()
            .map(|line| indent_to(line, cols, theme.fg_muted))
            .collect();

        View {
            rows,
            selected_row: None,
        }
    }
}

/// Left margin (cells) before each line, matching the palette's indent feel.
const MARGIN: usize = 2;
/// Floor width so a short message still reads as a panel, not a sliver.
const MIN_COLS: usize = 30;

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

/// Prefix `line` with the left margin and pad it to exactly `cols` cells with spaces.
fn indent_to(line: Vec<GridCell>, cols: usize, space_fg: Srgb) -> Vec<GridCell> {
    let mut row: Vec<GridCell> = (0..MARGIN).map(|_| cell(' ', space_fg)).collect();
    row.extend(line);
    row.truncate(cols);
    while row.len() < cols {
        row.push(cell(' ', space_fg));
    }
    row
}

#[cfg(test)]
mod tests {
    use super::{CloseTarget, Confirm};
    use skelly_render::Theme;

    fn row_text(row: &[super::GridCell]) -> String {
        row.iter().map(|c| c.c).collect()
    }

    #[test]
    fn view_names_the_process_and_the_target() {
        let theme = Theme::resolve("ossein-dark");
        let confirm = Confirm::new(CloseTarget::Pane, "vim".to_owned());
        let view = confirm.view(&theme);
        let joined = view
            .rows
            .iter()
            .map(|r| row_text(r))
            .collect::<Vec<_>>()
            .join("\n");
        assert!(joined.contains("vim"), "names the running process");
        assert!(joined.contains("Close this pane"), "names the target");
        assert!(joined.contains("esc cancel"), "shows the dismiss hint");
        assert!(view.selected_row.is_none());
    }

    #[test]
    fn the_process_name_is_drawn_in_accent() {
        let theme = Theme::resolve("ossein-dark");
        let confirm = Confirm::new(CloseTarget::Tab, "cargo".to_owned());
        let view = confirm.view(&theme);
        // The title row holds the accent-colored process name.
        let title = &view.rows[0];
        assert!(
            title.iter().any(|c| c.c == 'c' && c.fg == theme.accent),
            "the process name is accent-colored"
        );
        // The tab noun appears in the action line.
        assert!(row_text(&view.rows[2]).contains("Close this tab"));
    }
}
