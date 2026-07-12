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

/// The hint chips (key chord + label), from the guide's empty-state mockup: the palette,
/// a new tab, and a split - the three keys that matter first.
const CHIPS: [(&str, &str); 3] = [
    ("\u{2318}K", "palette"), // ⌘K
    ("\u{2318}T", "new tab"), // ⌘T
    ("\u{2325}|", "split"),   // ⌥|
];

/// Logical size (px) of the empty-state vertebra brand mark - the guide's §10.2 56px square.
pub(crate) const MARK_SIZE: f32 = 56.0;
/// Logical gap (px) between the mark's bottom edge and the hint-chip row.
const MARK_GAP: f32 = 18.0;
/// Chip pill height, inner horizontal padding, inter-chip gap, and corner radius (logical px).
const CHIP_H: f32 = 28.0;
const CHIP_PAD: f32 = 11.0;
const CHIP_GAP: f32 = 10.0;
const CHIP_KEY_GAP: f32 = 6.0;
const CHIP_RADIUS: f32 = 6.0;

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
/// seated a gap below the `logo` mark: each chip is a rounded `bg.elevated` pill with its key
/// chord (`mono`, `fg.secondary`) and label (`caption`, `fg.muted`).
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
    #[allow(
        clippy::cast_precision_loss,
        reason = "the chip count is a tiny exact value"
    )]
    let total: f32 =
        sizes.iter().map(|(_, _, pw)| pw).sum::<f32>() + gap * (CHIPS.len() - 1) as f32;
    let mut x = rect.x + (rect.w - total) * 0.5;
    let y = logo.y + logo.h + MARK_GAP * scale;
    let mut quads = Vec::new();
    let mut labels = Vec::new();
    for (i, (key, label)) in CHIPS.iter().enumerate() {
        let (kw, _, pw) = sizes[i];
        quads.push(ChromeQuad::rounded(
            PxRect { x, y, w: pw, h },
            theme.bg_elevated,
            CHIP_RADIUS * scale,
        ));
        let key_line = measure.line_height(FontRole::Micro);
        labels.push(ProseLabel {
            text: (*key).to_owned(),
            x: x + pad,
            y: y + (h - key_line) * 0.5,
            role: FontRole::Micro,
            color: theme.fg_secondary,
            weight: None,
            max_w: f32::MAX,
        });
        let label_line = measure.line_height(FontRole::Caption);
        labels.push(ProseLabel {
            text: (*label).to_owned(),
            x: x + pad + kw + key_gap,
            y: y + (h - label_line) * 0.5,
            role: FontRole::Caption,
            color: theme.fg_muted,
            weight: None,
            max_w: f32::MAX,
        });
        x += pw + gap;
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
        // Three pills, six labels (chord + label each).
        assert_eq!(quads.len(), 3, "one pill per chip");
        assert_eq!(labels.len(), 6, "chord + label per chip");
        let joined: String = labels.iter().map(|l| l.text.clone()).collect();
        assert!(joined.contains("palette"));
        assert!(joined.contains("new tab"));
        assert!(joined.contains("split"));
        // The pills use the elevated surface.
        assert!(quads.iter().all(|q| q.color == theme.bg_elevated));
        // The chips sit below the mark.
        assert!(quads[0].rect.y > logo.y + logo.h);
    }
}
