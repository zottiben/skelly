//! The empty-state overlay content (the design §10.2 "Empty state" screen).
//!
//! A fresh tab with no history shows a faint brand mark and a row of hint chips, centered
//! over the (blank) terminal, until the user runs their first command. This module is the
//! pure part: it bakes that content - in UI tokens - into a pane's cell grid. The binary
//! gates it (a pristine single-pane tab, see `Tab::is_empty_state`) and the "fade on first
//! command" flip; kept here so the layout is unit-testable without a GPU.
//!
//! It writes into the grid rather than a separate layer because a fresh tab's grid is
//! essentially blank (just the shell prompt at the top), so the centered mark and chips sit
//! on empty cells and ride the existing pane text + background passes - no new render path.

use skelly_render::{GridCell, Srgb, Theme};

/// The faint brand mark shown centered (a wordmark; the bespoke big-logo waits on the
/// fixed-metric cell renderer, like the other Nerd-glyph placeholders).
const MARK: &str = "skelly";

/// The hint chips (key chord + label), from the guide's empty-state mockup: the palette,
/// a new tab, and a split - the three keys that matter first.
const CHIPS: [(&str, &str); 3] = [
    ("\u{2318}K", "palette"), // ⌘K
    ("\u{2318}T", "new tab"), // ⌘T
    ("\u{2325}|", "split"),   // ⌥|
];

/// Cells of blank padding inside each chip pill, and between adjacent chips.
const CHIP_PAD: usize = 1;
const CHIP_GAP: usize = 2;

/// Bake the empty-state content (a faint mark + hint chips) into the center of a fresh
/// tab's grid `rows`, in UI tokens (Hard rule 2). A no-op on a grid too small to hold it.
pub(crate) fn overlay_onto(rows: &mut [Vec<GridCell>], theme: &Theme) {
    let height = rows.len();
    let width = rows.first().map_or(0, Vec::len);
    // Need room for the mark and, two rows below, the chips.
    let mark_row = height * 9 / 20; // a touch above center
    let chip_row = mark_row + 2;
    if width == 0 || chip_row >= height {
        return;
    }
    write_centered(&mut rows[mark_row], MARK, theme.fg_muted);
    write_chips(&mut rows[chip_row], width, theme);
}

/// Write `text` centered in `row`, in `fg`, leaving other cells untouched.
fn write_centered(row: &mut [GridCell], text: &str, fg: Srgb) {
    let start = row.len().saturating_sub(text.chars().count()) / 2;
    for (i, ch) in text.chars().enumerate() {
        if let Some(slot) = row.get_mut(start + i) {
            *slot = cell(ch, fg, None);
        }
    }
}

/// Paint the hint chips centered in `row`: each chip is a `bg.elevated` pill with its key
/// chord in `fg.secondary` and its label in `fg.muted`, separated by a gap.
fn write_chips(row: &mut [GridCell], width: usize, theme: &Theme) {
    let chip_width = |key: &str, label: &str| {
        // " <key> <label> " - a pad, the key, a space, the label, a pad.
        CHIP_PAD + key.chars().count() + 1 + label.chars().count() + CHIP_PAD
    };
    let total: usize =
        CHIPS.iter().map(|(k, l)| chip_width(k, l)).sum::<usize>() + CHIP_GAP * (CHIPS.len() - 1);
    let mut col = width.saturating_sub(total) / 2;
    for (key, label) in CHIPS {
        let pill = chip_width(key, label);
        // Segments within the pill: pad (muted), key (secondary), space, label (muted), pad.
        let mut inner = col + CHIP_PAD;
        paint(
            row,
            inner..inner + key.chars().count(),
            key,
            theme.fg_secondary,
            theme,
        );
        inner += key.chars().count();
        paint(row, inner..inner + 1, " ", theme.fg_muted, theme);
        inner += 1;
        paint(
            row,
            inner..inner + label.chars().count(),
            label,
            theme.fg_muted,
            theme,
        );
        // Fill the two pad cells (leading + trailing) with the pill background too.
        pill_bg(row, col, col + pill, theme);
        col += pill + CHIP_GAP;
    }
}

/// Write `text` into `row[range]` in `fg`, each cell backed by the chip's `bg.elevated` pill.
fn paint(row: &mut [GridCell], range: std::ops::Range<usize>, text: &str, fg: Srgb, theme: &Theme) {
    let mut chars = text.chars();
    for col in range {
        let ch = chars.next().unwrap_or(' ');
        if let Some(slot) = row.get_mut(col) {
            *slot = cell(ch, fg, Some(theme.bg_elevated));
        }
    }
}

/// Give every cell in `start..end` the chip's `bg.elevated` pill background (a blank cell
/// stays a space; the text cells keep their glyphs). Fills the pad cells left by `paint`.
fn pill_bg(row: &mut [GridCell], start: usize, end: usize, theme: &Theme) {
    for col in start..end {
        if let Some(slot) = row.get_mut(col) {
            if slot.bg.is_none() {
                *slot = cell(' ', theme.fg_muted, Some(theme.bg_elevated));
            }
        }
    }
}

/// A UI cell: a glyph in `fg`, an optional background fill, no attributes.
fn cell(c: char, fg: Srgb, bg: Option<Srgb>) -> GridCell {
    GridCell {
        c,
        fg,
        bg,
        bold: false,
        italic: false,
        underline: false,
    }
}

#[cfg(test)]
mod tests {
    use super::{overlay_onto, CHIPS, MARK};
    use skelly_render::{GridCell, Srgb, Theme};

    fn blank(cols: usize, rows: usize) -> Vec<Vec<GridCell>> {
        let space = GridCell {
            c: ' ',
            fg: Srgb { r: 0, g: 0, b: 0 },
            bg: None,
            bold: false,
            italic: false,
            underline: false,
        };
        vec![vec![space; cols]; rows]
    }

    fn joined(grid: &[Vec<GridCell>]) -> String {
        grid.iter()
            .map(|row| row.iter().map(|c| c.c).collect::<String>())
            .collect::<Vec<_>>()
            .join("\n")
    }

    #[test]
    fn overlays_the_mark_and_chip_labels() {
        let theme = Theme::resolve("ossein-dark");
        let mut grid = blank(60, 24);
        overlay_onto(&mut grid, &theme);
        let text = joined(&grid);
        assert!(text.contains(MARK), "mark: {text}");
        for (_, label) in CHIPS {
            assert!(text.contains(label), "chip label {label:?}: {text}");
        }
    }

    #[test]
    fn chips_have_an_elevated_pill_background() {
        let theme = Theme::resolve("ossein-dark");
        let mut grid = blank(60, 24);
        overlay_onto(&mut grid, &theme);
        // Some cell carries the chip pill background (bg.elevated).
        let pill = grid
            .iter()
            .flatten()
            .any(|c| c.bg == Some(theme.bg_elevated));
        assert!(pill, "expected a bg.elevated chip pill");
    }

    #[test]
    fn mark_uses_the_muted_ui_token() {
        let theme = Theme::resolve("ossein-dark");
        let mut grid = blank(60, 24);
        overlay_onto(&mut grid, &theme);
        // The mark's first glyph is drawn in fg.muted (a quiet wordmark).
        let mark_glyph = grid
            .iter()
            .flatten()
            .find(|c| c.c == MARK.chars().next().unwrap());
        assert_eq!(mark_glyph.map(|c| c.fg), Some(theme.fg_muted));
    }

    #[test]
    fn tiny_grid_is_a_no_op() {
        let theme = Theme::resolve("ossein-dark");
        let mut grid = blank(10, 3); // too short for mark + chips
        overlay_onto(&mut grid, &theme);
        assert_eq!(joined(&grid).trim(), "", "no overlay on a tiny grid");
    }
}
