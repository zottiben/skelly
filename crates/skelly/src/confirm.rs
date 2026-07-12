//! The "close with a running job" confirm modal (design §12 "Process running on close"):
//! a centered overlay that warns before a close (`⌥w` pane / `⌘W` tab) which would kill a
//! running foreground job, so a job is never destroyed silently. Confirmed with `Enter` or
//! a second press of the close chord, dismissed with `Esc`. This module is pure state +
//! layout (a proportional display list); the binary owns detecting the job (via
//! `Terminal::foreground_job_pid` plus the process name), routing keys, and closing.

use skelly_render::{FontRole, ProseLabel, PxRect, Srgb, TextMeasure, Theme};

/// Modal layout constants in **logical** px (multiplied by the DPI scale).
const PAD: f32 = 18.0;
/// The title line height (`"<proc>" is still running`).
const TITLE_H: f32 = 28.0;
/// Gap between the title and the action question.
const GAP: f32 = 6.0;
/// The action-question line height.
const ACTION_H: f32 = 24.0;
/// Gap between the action and the key hints.
const GAP2: f32 = 14.0;
/// The key-hint line height.
const HINT_H: f32 = 22.0;

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

impl Confirm {
    /// A pending confirm for closing `target`, which would kill the foreground job named
    /// `process`.
    pub(crate) fn new(target: CloseTarget, process: String) -> Self {
        Self { target, process }
    }

    /// The `"<proc>" is still running` title, split so the process name draws in `accent`.
    fn title_runs(&self, theme: &Theme) -> [(String, Srgb); 3] {
        [
            ("\"".to_owned(), theme.fg_primary),
            (self.process.clone(), theme.accent),
            ("\" is still running".to_owned(), theme.fg_primary),
        ]
    }

    /// The action question naming the target.
    fn action(&self) -> String {
        format!("Close this {} and end it?", self.target.noun())
    }

    /// The modal's natural panel size in **physical** px (including padding), for the binary
    /// to center + animate the card.
    pub(crate) fn natural_size(&self, scale: f32, measure: &mut TextMeasure) -> (f32, f32) {
        let theme = Theme::resolve("ossein-dark"); // color-independent; only widths are used
        let title_w: f32 = self
            .title_runs(&theme)
            .iter()
            .map(|(t, _)| measure.width(t, FontRole::Title, None))
            .sum();
        let width = title_w
            .max(measure.width(&self.action(), FontRole::Body, None))
            .max(measure.width(HINT, FontRole::Caption, None))
            + 2.0 * PAD * scale;
        let height = (PAD + TITLE_H + GAP + ACTION_H + GAP2 + HINT_H + PAD) * scale;
        (width, height)
    }

    /// Build the modal's centered content labels within `panel` (the renderer draws the card
    /// itself): the title (process name in `accent`), the action question, and the key hints.
    pub(crate) fn build(
        &self,
        panel: PxRect,
        scale: f32,
        theme: &Theme,
        measure: &mut TextMeasure,
    ) -> Vec<ProseLabel> {
        let mut labels = Vec::new();
        let mut y = panel.y + PAD * scale;

        // Title: centered as a whole, drawn as consecutive colored runs.
        let runs = self.title_runs(theme);
        let total: f32 = runs
            .iter()
            .map(|(t, _)| measure.width(t, FontRole::Title, None))
            .sum();
        let line_h = measure.line_height(FontRole::Title);
        let ty = y + (TITLE_H * scale - line_h) * 0.5;
        let mut x = panel.x + (panel.w - total) * 0.5;
        for (text, color) in runs {
            let w = measure.width(&text, FontRole::Title, None);
            labels.push(ProseLabel {
                text,
                x,
                y: ty,
                role: FontRole::Title,
                color,
                weight: None,
                max_w: f32::MAX,
            });
            x += w;
        }
        y += TITLE_H * scale + GAP * scale;

        push_centered(
            &mut labels,
            &self.action(),
            FontRole::Body,
            theme.fg_primary,
            panel,
            y,
            ACTION_H,
            scale,
            measure,
        );
        y += ACTION_H * scale + GAP2 * scale;
        push_centered(
            &mut labels,
            HINT,
            FontRole::Caption,
            theme.fg_muted,
            panel,
            y,
            HINT_H,
            scale,
            measure,
        );
        labels
    }
}

/// The dismiss hint line.
const HINT: &str = "\u{21b5} close    esc cancel";

/// Push a single label horizontally centered in `panel` and vertically centered in a row of
/// `row_h` logical px whose top is physical `top`.
#[allow(clippy::too_many_arguments, reason = "one focused placement helper")]
fn push_centered(
    labels: &mut Vec<ProseLabel>,
    text: &str,
    role: FontRole,
    color: Srgb,
    panel: PxRect,
    top: f32,
    row_h: f32,
    scale: f32,
    measure: &mut TextMeasure,
) {
    let w = measure.width(text, role, None);
    let line_h = measure.line_height(role);
    labels.push(ProseLabel {
        text: text.to_owned(),
        x: panel.x + (panel.w - w) * 0.5,
        y: top + (row_h * scale - line_h) * 0.5,
        role,
        color,
        weight: None,
        max_w: f32::MAX,
    });
}

#[cfg(test)]
mod tests {
    use super::{CloseTarget, Confirm, PxRect};
    use skelly_render::{TextMeasure, Theme};

    #[test]
    fn build_names_the_process_in_accent_and_names_the_target() {
        let theme = Theme::resolve("ossein-dark");
        let mut m = TextMeasure::new(2.0);
        let confirm = Confirm::new(CloseTarget::Pane, "vim".to_owned());
        let (w, h) = confirm.natural_size(2.0, &mut m);
        let panel = PxRect {
            x: 0.0,
            y: 0.0,
            w,
            h,
        };
        let labels = confirm.build(panel, 2.0, &theme, &mut m);
        let joined: String = labels.iter().map(|l| l.text.clone()).collect();
        assert!(joined.contains("vim"), "names the running process");
        assert!(joined.contains("Close this pane"), "names the target");
        assert!(joined.contains("esc cancel"), "shows the dismiss hint");
        // The process name draws in accent.
        assert!(labels
            .iter()
            .any(|l| l.text == "vim" && l.color == theme.accent));
    }

    #[test]
    fn the_tab_target_names_the_tab() {
        let theme = Theme::resolve("ossein-dark");
        let mut m = TextMeasure::new(2.0);
        let confirm = Confirm::new(CloseTarget::Tab, "cargo".to_owned());
        let (w, h) = confirm.natural_size(2.0, &mut m);
        let labels = confirm.build(
            PxRect {
                x: 0.0,
                y: 0.0,
                w,
                h,
            },
            2.0,
            &theme,
            &mut m,
        );
        let joined: String = labels.iter().map(|l| l.text.clone()).collect();
        assert!(joined.contains("Close this tab"));
    }
}
