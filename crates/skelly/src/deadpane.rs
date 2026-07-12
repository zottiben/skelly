//! The "shell exited" overlay content (the design "Shell exits / crashes" edge state).
//!
//! When a pane's shell ends, the renderer dims the pane and draws a small centered
//! message over its preserved scrollback. This module is the pure part: it turns an
//! [`ExitStatus`] into that message as a UI-token-colored [`GridCell`] grid, sized to its
//! widest line. The binary centers the grid in the pane rect and hands it to
//! `Renderer::set_pane_overlays`; the wiring (detecting the exit, restart on `↵`) lives in
//! `main.rs`. Kept here so the message layout is unit-testable without a GPU.

use skelly_render::{GridCell, Srgb, Theme};
use skelly_term::ExitStatus;

/// The restart / close hint shown beneath the exit line. `↵` restarts the shell in place;
/// `⌥w` closes the pane (this app's pane-close chord; the design's `⌘W` closes the tab).
const HINT_RESTART: &str = "\u{21b5} restart";
const HINT_CLOSE: &str = "\u{2325}w close";

/// One colored run within a message line.
struct Segment {
    text: String,
    fg: Srgb,
}

impl Segment {
    fn new(text: impl Into<String>, fg: Srgb) -> Self {
        Self {
            text: text.into(),
            fg,
        }
    }

    fn width(&self) -> usize {
        self.text.chars().count()
    }
}

/// Build the centered exit message for a pane whose shell ended, as a UI-token-colored
/// grid sized to its widest line (the caller centers it within the pane).
///
/// Reads only theme tokens (Hard rule 2): the title in `fg.primary`, the exit detail in
/// `diff.add` when clean or `diff.del` when it failed / was signalled, and the key hints
/// with `accent` chords over `fg.muted` words.
pub(crate) fn overlay_grid(status: &ExitStatus, theme: &Theme) -> Vec<Vec<GridCell>> {
    let detail = match &status.signal {
        Some(signal) => format!("killed by {signal}"),
        None => format!("exit code {}", status.code),
    };
    let detail_fg = if status.success() {
        theme.diff_add
    } else {
        theme.diff_del
    };
    let lines: Vec<Vec<Segment>> = vec![
        vec![Segment::new("shell exited", theme.fg_primary)],
        vec![Segment::new(detail, detail_fg)],
        Vec::new(), // spacer
        vec![
            Segment::new(HINT_RESTART, theme.accent),
            Segment::new("   ", theme.fg_muted),
            Segment::new(HINT_CLOSE, theme.accent),
        ],
    ];

    let width = lines
        .iter()
        .map(|segments| segments.iter().map(Segment::width).sum())
        .max()
        .unwrap_or(0);
    lines
        .iter()
        .map(|segments| render_line(segments, width, theme.fg_muted))
        .collect()
}

/// Render one line's segments centered in a `width`-cell row (blank cells `blank_fg`).
fn render_line(segments: &[Segment], width: usize, blank_fg: Srgb) -> Vec<GridCell> {
    let content: usize = segments.iter().map(Segment::width).sum();
    let mut row = vec![cell(' ', blank_fg); width];
    let mut col = width.saturating_sub(content) / 2;
    for segment in segments {
        for ch in segment.text.chars() {
            if let Some(slot) = row.get_mut(col) {
                *slot = cell(ch, segment.fg);
            }
            col += 1;
        }
    }
    row
}

/// A plain UI cell: a glyph in `fg`, no background or attributes.
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

#[cfg(test)]
mod tests {
    use super::overlay_grid;
    use skelly_render::Theme;
    use skelly_term::ExitStatus;

    fn joined(status: &ExitStatus) -> String {
        let theme = Theme::resolve("ossein-dark");
        overlay_grid(status, &theme)
            .iter()
            .map(|row| row.iter().map(|c| c.c).collect::<String>())
            .collect::<Vec<_>>()
            .join("\n")
    }

    #[test]
    fn clean_exit_shows_code_and_restart_hint() {
        let text = joined(&ExitStatus {
            code: 0,
            signal: None,
        });
        assert!(text.contains("shell exited"), "title: {text}");
        assert!(text.contains("exit code 0"), "detail: {text}");
        assert!(text.contains("restart"), "restart hint: {text}");
        assert!(text.contains("close"), "close hint: {text}");
    }

    #[test]
    fn nonzero_exit_reports_its_code() {
        let text = joined(&ExitStatus {
            code: 130,
            signal: None,
        });
        assert!(text.contains("exit code 130"), "{text}");
    }

    #[test]
    fn signalled_exit_names_the_signal() {
        let text = joined(&ExitStatus {
            code: 1,
            signal: Some("SIGTERM".to_owned()),
        });
        assert!(text.contains("killed by SIGTERM"), "{text}");
    }

    #[test]
    fn detail_color_reflects_success() {
        let theme = Theme::resolve("ossein-dark");
        let clean = overlay_grid(
            &ExitStatus {
                code: 0,
                signal: None,
            },
            &theme,
        );
        let failed = overlay_grid(
            &ExitStatus {
                code: 1,
                signal: None,
            },
            &theme,
        );
        // The exit-detail row (row 1) is green on success, red on failure.
        let color_of = |grid: &[Vec<skelly_render::GridCell>]| {
            grid[1].iter().find(|c| c.c == 'e').map(|c| c.fg)
        };
        assert_eq!(color_of(&clean), Some(theme.diff_add));
        assert_eq!(color_of(&failed), Some(theme.diff_del));
    }

    #[test]
    fn every_row_has_equal_width() {
        let grid = overlay_grid(
            &ExitStatus {
                code: 0,
                signal: None,
            },
            &Theme::resolve("ossein-dark"),
        );
        let width = grid[0].len();
        assert!(width > 0);
        assert!(grid.iter().all(|row| row.len() == width), "ragged grid");
    }
}
