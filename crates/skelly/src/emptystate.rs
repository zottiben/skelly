//! The empty-state overlay content (the design §10.2 "Empty state" screen).
//!
//! A fresh tab with no history shows a faint vertebra brand mark and a row of hint chips,
//! centered over the (blank) terminal, until the user runs their first command. The mark
//! itself is a vector overlay the renderer paints (the guide's §02 logo, via
//! `skelly_render`'s `PaneView::logo`); this module owns the part that lives in the cell
//! grid - the hint chips - plus the shared layout (see [`chip_row`] / [`MARK_SIZE`]) the
//! binary uses to seat the vector mark above them. The binary gates the whole overlay (a
//! pristine single-pane tab, see `Tab::is_empty_state`) and the "fade on first command"
//! flip; kept here so the layout is unit-testable without a GPU.

use skelly_render::{GridCell, Srgb, Theme};

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

/// Logical size (px) of the empty-state vertebra brand mark - the guide's §10.2 mark is a
/// 56px square. The renderer paints the mark (from `PaneView::logo`); the binary sizes it
/// with this and seats it above [`chip_row`].
pub(crate) const MARK_SIZE: f32 = 56.0;
/// Logical gap (px) between the brand mark's bottom edge and the hint-chip row.
pub(crate) const MARK_GAP: f32 = 14.0;

/// The grid row of the hint chips (a touch below center), or `None` when `height` is too
/// small to seat the mark and chips. Shared by [`overlay_onto`] and the binary's mark
/// placement so the vector mark and the chip text stay aligned as one lockup.
pub(crate) fn chip_row(height: usize) -> Option<usize> {
    let chip_row = height * 9 / 20 + 2; // the nominal mark row, two rows down
    (chip_row < height).then_some(chip_row)
}

/// Bake the empty-state hint chips into a fresh tab's grid `rows`, in UI tokens (Hard rule
/// 2). The faint vertebra mark above them is a vector overlay the renderer paints (see
/// [`MARK_SIZE`]); this module owns only the chip text. A no-op on a grid too small to hold
/// the layout.
pub(crate) fn overlay_onto(rows: &mut [Vec<GridCell>], theme: &Theme) {
    let width = rows.first().map_or(0, Vec::len);
    let Some(chip_row) = chip_row(rows.len()) else {
        return;
    };
    if width == 0 {
        return;
    }
    write_chips(&mut rows[chip_row], width, theme);
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
    use super::{chip_row, overlay_onto, CHIPS};
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
    fn overlays_the_hint_chip_labels() {
        let theme = Theme::resolve("ossein-dark");
        let mut grid = blank(60, 24);
        overlay_onto(&mut grid, &theme);
        let text = joined(&grid);
        for (_, label) in CHIPS {
            assert!(text.contains(label), "chip label {label:?}: {text}");
        }
    }

    #[test]
    fn chip_row_seats_below_center_and_bails_on_a_tiny_grid() {
        // A roomy grid places the chips a touch below center (with headroom for the mark).
        assert_eq!(chip_row(24), Some(24 * 9 / 20 + 2));
        // Too short to seat the mark + chips: no overlay row.
        assert_eq!(chip_row(3), None);
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
    fn tiny_grid_is_a_no_op() {
        let theme = Theme::resolve("ossein-dark");
        let mut grid = blank(10, 3); // too short for mark + chips
        overlay_onto(&mut grid, &theme);
        assert_eq!(joined(&grid).trim(), "", "no overlay on a tiny grid");
    }
}
