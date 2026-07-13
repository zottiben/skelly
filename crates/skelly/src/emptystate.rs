//! The empty-state overlay content (the design §10.2 "Empty state" screen).
//!
//! A fresh tab with no history shows a faint vertebra brand mark and a row of hint chips,
//! centered over the (blank) terminal, until the user runs their first command. The mark
//! itself is a vector overlay the renderer paints (the guide's §02 logo, via
//! `skelly_render`'s `PaneView::logo`); this module computes its bounds ([`logo_bounds`])
//! and builds the hint chips ([`chips_paint`]) as a proportional display list - rounded
//! `bg.elevated` pills with the key chord + label - seated below the mark. The binary gates
//! the whole overlay (a pristine single-pane tab) and draws the chips through the pane-
//! overlay layer; kept here so the layout is unit-testable without a GPU.

use skelly_render::{ChromeQuad, FontRole, ProseLabel, PxRect, TextMeasure, Theme};

/// The hint chips (key chord + label), verbatim from the guide's §10.2 empty-state mockup:
/// the palette, a split, the git diff dock, and settings - the four chords worth surfacing
/// first. They wrap to a second row on a pane too narrow to hold them all.
const CHIPS: [(&str, &str); 4] = [
    ("\u{2318}K", "commands"),         // ⌘K
    ("\u{2325}|", "split right"),      // ⌥|
    ("\u{21E7}\u{2318}G", "git diff"), // ⇧⌘G
    ("\u{2318},", "settings"),         // ⌘,
];

/// Logical size (px) of the empty-state vertebra brand mark - the guide's §10.2 56px square.
pub(crate) const MARK_SIZE: f32 = 56.0;
/// Logical gap (px) between the mark's bottom edge and the hint-chip row.
const MARK_GAP: f32 = 18.0;
/// Chip pill height, inner horizontal padding, inter-chip gap, and inter-row gap (logical px).
/// The pills are capsules (radius = half the height), per the guide's `border-radius:999px`.
const CHIP_H: f32 = 28.0;
const CHIP_PAD: f32 = 13.0;
const CHIP_GAP: f32 = 10.0;
const CHIP_KEY_GAP: f32 = 6.0;
const CHIP_ROW_GAP: f32 = 10.0;
/// Horizontal breathing room (logical px, each side) reserved before the chips wrap - so a wide
/// pane keeps them on one row and only a narrow pane wraps (as the guide's preview does).
const CHIP_MARGIN: f32 = 40.0;

/// The vertebra mark's square bounding box (physical px), centered horizontally and a touch
/// above the pane's vertical center, or `None` when `rect` is too small to seat the lockup.
/// The renderer paints the mark here (via `PaneView::logo`); [`chips_paint`] seats the chips
/// below it so the two read as one lockup.
pub(crate) fn logo_bounds(rect: PxRect, scale: f32) -> Option<PxRect> {
    let mark = MARK_SIZE * scale;
    // Need headroom for the mark + gap + a chip row, plus breathing room.
    if rect.w < mark * 2.0 || rect.h < mark + (MARK_GAP + CHIP_H) * scale + 60.0 * scale {
        return None;
    }
    let cx = rect.x + rect.w * 0.5;
    let cy = rect.y + rect.h * 0.42;
    Some(PxRect {
        x: cx - mark * 0.5,
        y: cy - mark * 0.5,
        w: mark,
        h: mark,
    })
}

/// Build the hint chips as a proportional display list, centered horizontally in `rect` and
/// seated a gap below the `logo` mark: each chip is a capsule `bg.elevated` pill with its key
/// chord (`mono`, `fg.secondary`) and label (`caption`, `fg.muted`), wrapping across rows.
#[allow(
    clippy::cast_precision_loss,
    reason = "the per-row chip count is a tiny exact value"
)]
pub(crate) fn chips_paint(
    logo: PxRect,
    rect: PxRect,
    scale: f32,
    theme: &Theme,
    measure: &mut TextMeasure,
) -> (Vec<ChromeQuad>, Vec<ProseLabel>) {
    let (pad, gap, h, key_gap) = (
        CHIP_PAD * scale,
        CHIP_GAP * scale,
        CHIP_H * scale,
        CHIP_KEY_GAP * scale,
    );
    // Measure each chip: (key width, label width, total pill width).
    let sizes: Vec<(f32, f32, f32)> = CHIPS
        .iter()
        .map(|(key, label)| {
            let kw = measure.width(key, FontRole::Micro, None);
            let lw = measure.width(label, FontRole::Caption, None);
            (kw, lw, pad + kw + key_gap + lw + pad)
        })
        .collect();
    // Greedily pack chips into rows no wider than the pane's content span, so a wide pane keeps
    // them on one row and a narrow one wraps (as the guide's preview does).
    let max_row = (rect.w - 2.0 * CHIP_MARGIN * scale).max(sizes[0].2);
    let mut rows: Vec<Vec<usize>> = vec![Vec::new()];
    let mut row_w = 0.0;
    for (i, &(_, _, pw)) in sizes.iter().enumerate() {
        let extra = if rows.last().unwrap().is_empty() {
            pw
        } else {
            pw + gap
        };
        if !rows.last().unwrap().is_empty() && row_w + extra > max_row {
            rows.push(vec![i]);
            row_w = pw;
        } else {
            rows.last_mut().unwrap().push(i);
            row_w += extra;
        }
    }

    let key_line = measure.line_height(FontRole::Micro);
    let label_line = measure.line_height(FontRole::Caption);
    let mut quads = Vec::new();
    let mut labels = Vec::new();
    let mut y = logo.y + logo.h + MARK_GAP * scale;
    for row in &rows {
        let row_total: f32 = row.iter().map(|&i| sizes[i].2).sum::<f32>()
            + gap * (row.len().saturating_sub(1)) as f32;
        let mut x = rect.x + (rect.w - row_total) * 0.5;
        for &i in row {
            let (key, label) = CHIPS[i];
            let (kw, _, pw) = sizes[i];
            // Capsule pill (radius = half height, clamped by the renderer).
            quads.push(ChromeQuad::rounded(
                PxRect { x, y, w: pw, h },
                theme.bg_elevated,
                h * 0.5,
            ));
            labels.push(ProseLabel {
                text: key.to_owned(),
                x: x + pad,
                y: y + (h - key_line) * 0.5,
                role: FontRole::Micro,
                color: theme.fg_secondary,
                weight: None,
                max_w: f32::MAX,
            });
            labels.push(ProseLabel {
                text: label.to_owned(),
                x: x + pad + kw + key_gap,
                y: y + (h - label_line) * 0.5,
                role: FontRole::Caption,
                color: theme.fg_muted,
                weight: None,
                max_w: f32::MAX,
            });
            x += pw + gap;
        }
        y += h + CHIP_ROW_GAP * scale;
    }
    (quads, labels)
}

#[cfg(test)]
mod tests {
    use super::{chips_paint, logo_bounds, MARK_SIZE};
    use skelly_render::{PxRect, TextMeasure, Theme};

    fn pane() -> PxRect {
        PxRect {
            x: 0.0,
            y: 0.0,
            w: 1200.0,
            h: 800.0,
        }
    }

    #[test]
    fn logo_bounds_seats_a_square_mark_centered_horizontally() {
        let b = logo_bounds(pane(), 2.0).expect("roomy pane seats a mark");
        assert!((b.w - MARK_SIZE * 2.0).abs() < 1e-3);
        assert!((b.h - MARK_SIZE * 2.0).abs() < 1e-3);
        // Centered horizontally in the pane.
        assert!((b.x + b.w * 0.5 - (pane().x + pane().w * 0.5)).abs() < 1e-3);
    }

    #[test]
    fn logo_bounds_is_none_on_a_tiny_pane() {
        assert!(logo_bounds(
            PxRect {
                x: 0.0,
                y: 0.0,
                w: 40.0,
                h: 40.0,
            },
            2.0,
        )
        .is_none());
    }

    #[test]
    fn chips_paint_emits_a_pill_and_two_labels_per_chip() {
        let theme = Theme::resolve("ossein-dark");
        let mut m = TextMeasure::new(2.0);
        let logo = logo_bounds(pane(), 2.0).unwrap();
        let (quads, labels) = chips_paint(logo, pane(), 2.0, &theme, &mut m);
        // Four pills, eight labels (chord + label each) - the guide's §10.2 chip set.
        assert_eq!(quads.len(), 4, "one pill per chip");
        assert_eq!(labels.len(), 8, "chord + label per chip");
        let joined: String = labels.iter().map(|l| l.text.clone()).collect();
        assert!(joined.contains("commands"));
        assert!(joined.contains("split right"));
        assert!(joined.contains("git diff"));
        assert!(joined.contains("settings"));
        // The pills use the elevated surface and are capsules (radius = half the height).
        assert!(quads.iter().all(|q| q.color == theme.bg_elevated));
        assert!(
            (quads[0].radius - quads[0].rect.h * 0.5).abs() < 1e-3,
            "capsule pill"
        );
        // The chips sit below the mark.
        assert!(quads[0].rect.y > logo.y + logo.h);
    }

    #[test]
    fn chips_wrap_to_a_second_row_on_a_narrow_pane() {
        // A pane too narrow for all four chips on one row stacks a chip onto a second row,
        // seated below the first (the guide's 3-then-1 wrap).
        let theme = Theme::resolve("ossein-dark");
        let mut m = TextMeasure::new(2.0);
        let narrow = PxRect {
            x: 0.0,
            y: 0.0,
            w: 560.0,
            h: 800.0,
        };
        let logo = logo_bounds(narrow, 2.0).unwrap();
        let (quads, _) = chips_paint(logo, narrow, 2.0, &theme, &mut m);
        assert_eq!(quads.len(), 4, "still four pills");
        let top = quads[0].rect.y;
        assert!(
            quads.iter().any(|q| q.rect.y > top + 1.0),
            "at least one chip wrapped to a lower row"
        );
    }
}
