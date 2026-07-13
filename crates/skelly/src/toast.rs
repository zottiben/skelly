//! Transient toast notifications (design §09 "Primitives" / §12 flows: "At 8 panes the command
//! no-ops and a toast explains the cap"). A small floating card - a status dot + a message - that
//! rises at the bottom of the window and auto-dismisses. Pure state + layout: the binary owns the
//! expiry timer (it wakes the loop at the deadline) and reuses the shared overlay card
//! (`bg.elevated` + shadow + `border.strong` ring) to draw it. Not modal - it never captures
//! input; it just fades on its own or is replaced by the next one.

use skelly_render::{ChromeQuad, FontRole, ProseLabel, PxRect, TextMeasure, Theme};

/// Toast layout constants in **logical** px (multiplied by the DPI scale when placed).
const PAD_X: f32 = 12.0;
const PAD_Y: f32 = 10.0;
/// The leading status dot's diameter (the mockup's 6px `●`) and its gap before the message.
const DOT: f32 = 6.0;
const DOT_GAP: f32 = 10.0;
/// Gap from the window's bottom edge to the toast card.
const MARGIN_BOTTOM: f32 = 24.0;

/// A toast's severity, which colors its leading dot: `Info` (accent) for a neutral notice like
/// the pane cap, `Success` (green) for a completed action.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub(crate) enum ToastKind {
    Info,
    Success,
}

/// A transient notification: a short message + its severity. The binary pairs it with an expiry.
pub(crate) struct Toast {
    message: String,
    kind: ToastKind,
}

impl Toast {
    /// A toast with `message` at severity `kind`.
    pub(crate) fn new(message: impl Into<String>, kind: ToastKind) -> Self {
        Self {
            message: message.into(),
            kind,
        }
    }

    /// The toast's natural size in **physical** px (the dot + gap + message + insets), for the
    /// binary to place + draw the card.
    pub(crate) fn natural_size(&self, scale: f32, measure: &mut TextMeasure) -> (f32, f32) {
        let text_w = measure.width(&self.message, FontRole::Caption, None);
        let w = (DOT + DOT_GAP) * scale + text_w + 2.0 * PAD_X * scale;
        let h = 2.0 * PAD_Y * scale + measure.line_height(FontRole::Caption);
        (w, h)
    }

    /// Build the toast's content within `panel` (physical px; the renderer draws the card): the
    /// leading status dot + the message label.
    pub(crate) fn build(
        &self,
        panel: PxRect,
        scale: f32,
        theme: &Theme,
        measure: &mut TextMeasure,
    ) -> (Vec<ChromeQuad>, Vec<ProseLabel>) {
        let dot = DOT * scale;
        let dot_color = match self.kind {
            ToastKind::Info => theme.accent,
            ToastKind::Success => theme.diff_add,
        };
        let quads = vec![ChromeQuad::rounded(
            PxRect {
                x: panel.x + PAD_X * scale,
                y: panel.y + (panel.h - dot) * 0.5,
                w: dot,
                h: dot,
            },
            dot_color,
            dot * 0.5,
        )];
        let line = measure.line_height(FontRole::Caption);
        let labels = vec![ProseLabel {
            text: self.message.clone(),
            x: panel.x + (PAD_X + DOT + DOT_GAP) * scale,
            y: panel.y + (panel.h - line) * 0.5,
            role: FontRole::Caption,
            color: theme.fg_primary,
            weight: None,
            max_w: f32::MAX,
        }];
        (quads, labels)
    }
}

/// Center a toast of `size` horizontally and anchor it near the bottom edge of the `surface`
/// (physical px). Free-standing: the placement depends only on the sizes.
pub(crate) fn place(size: (f32, f32), surface: (f32, f32), scale: f32) -> PxRect {
    let (w, h) = size;
    PxRect {
        x: ((surface.0 - w) * 0.5).max(0.0),
        y: (surface.1 - h - MARGIN_BOTTOM * scale).max(0.0),
        w,
        h,
    }
}

#[cfg(test)]
mod tests {
    use super::{place, Toast, ToastKind};
    use skelly_render::{TextMeasure, Theme};

    #[test]
    fn builds_a_dot_and_message_and_places_above_the_bottom_edge() {
        let theme = Theme::resolve("ossein-dark");
        let mut m = TextMeasure::new(2.0);
        let toast = Toast::new("Pane limit reached (8 max)", ToastKind::Info);
        let size = toast.natural_size(2.0, &mut m);
        let surface = (1920.0, 1200.0);
        let panel = place(size, surface, 2.0);
        // Centered horizontally, and sitting above the bottom edge.
        assert!((panel.x + panel.w / 2.0 - surface.0 / 2.0).abs() < 1.0);
        assert!(panel.y + panel.h < surface.1);
        let (quads, labels) = toast.build(panel, 2.0, &theme, &mut m);
        assert_eq!(quads.len(), 1, "one status dot");
        assert!(labels.iter().any(|l| l.text.contains("Pane limit")));
        // The Info dot uses the accent color; Success uses the add-green.
        assert_eq!(quads[0].color, theme.accent);
        let (sq, _) =
            Toast::new("Committed abc123", ToastKind::Success).build(panel, 2.0, &theme, &mut m);
        assert_eq!(sq[0].color, theme.diff_add);
    }
}
