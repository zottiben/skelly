//! Hover tooltips (design §09 "Primitives"): a small floating label that names an icon-only
//! affordance - the utility-bar icons (⚙ ◐ ⟲ ⑂), the slim rail's numbered tabs, the workspace
//! chips, the command well - after the pointer rests on it briefly. Pure layout: the binary owns
//! the hover-delay timer + mapping the hovered element to a label, and reuses the shared overlay
//! card (`bg.elevated` + shadow + `border.strong` ring) to draw it. Not modal - it never captures
//! input and vanishes as soon as the pointer moves off the element.

use skelly_render::{ChromeQuad, FontRole, ProseLabel, PxRect, TextMeasure, Theme};

/// Tooltip layout constants in **logical** px (multiplied by the DPI scale when placed).
const PAD_X: f32 = 9.0;
const PAD_Y: f32 = 5.0;
/// Gap between the pointer/anchor and the tooltip card.
const OFFSET: f32 = 14.0;

/// The tooltip's natural size in **physical** px (the label + insets).
pub(crate) fn natural_size(label: &str, scale: f32, measure: &mut TextMeasure) -> (f32, f32) {
    let w = measure.width(label, FontRole::Caption, None) + 2.0 * PAD_X * scale;
    let h = measure.line_height(FontRole::Caption) + 2.0 * PAD_Y * scale;
    (w, h)
}

/// Place the tooltip near `anchor` (physical px, the pointer), down-right by `OFFSET` and clamped
/// inside the `surface` so it never spills past an edge.
pub(crate) fn place(
    anchor: (f32, f32),
    size: (f32, f32),
    surface: (f32, f32),
    scale: f32,
) -> PxRect {
    let (w, h) = size;
    let off = OFFSET * scale;
    let x = (anchor.0 + off).min(surface.0 - w).max(0.0);
    let y = (anchor.1 + off).min(surface.1 - h).max(0.0);
    PxRect { x, y, w, h }
}

/// Build the tooltip content within `panel` (physical px; the renderer draws the card): the
/// single centered label.
pub(crate) fn build(
    label: &str,
    panel: PxRect,
    scale: f32,
    theme: &Theme,
    measure: &mut TextMeasure,
) -> (Vec<ChromeQuad>, Vec<ProseLabel>) {
    let line = measure.line_height(FontRole::Caption);
    let labels = vec![ProseLabel {
        text: label.to_owned(),
        x: panel.x + PAD_X * scale,
        y: panel.y + (panel.h - line) * 0.5,
        role: FontRole::Caption,
        color: theme.fg_primary,
        weight: None,
        max_w: f32::MAX,
    }];
    (Vec::new(), labels)
}

#[cfg(test)]
mod tests {
    use super::{build, natural_size, place};
    use skelly_render::{TextMeasure, Theme};

    #[test]
    fn builds_the_label_and_clamps_to_the_surface() {
        let theme = Theme::resolve("ossein-dark");
        let mut m = TextMeasure::new(2.0);
        let size = natural_size("Settings", 2.0, &mut m);
        // Anchored at the far bottom-right corner, the card shifts back fully on-screen.
        let panel = place((999.0, 699.0), size, (700.0, 700.0), 2.0);
        assert!(panel.x + panel.w <= 700.0 + 0.5);
        assert!(panel.y + panel.h <= 700.0 + 0.5);
        let (_, labels) = build("Settings", panel, 2.0, &theme, &mut m);
        assert_eq!(labels.len(), 1);
        assert_eq!(labels[0].text, "Settings");
    }
}
