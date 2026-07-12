//! The "shell exited" overlay content (the design "Shell exits / crashes" edge state).
//!
//! When a pane's shell ends, the renderer dims the pane (a `bg.base` scrim over its
//! preserved scrollback) and draws a small centered message over it. This module is the
//! pure part: it turns an [`ExitStatus`] into that message as centered proportional
//! [`ProseLabel`]s in the guide's fonts. The binary hands the labels (plus the scrim rect)
//! to `Renderer::set_pane_overlays`; the wiring (detecting the exit, restart on `↵`) lives
//! in `main.rs`. Kept here so the message layout is unit-testable without a GPU.

use skelly_render::{FontRole, ProseLabel, PxRect, Srgb, TextMeasure, Theme};
use skelly_term::ExitStatus;

/// The restart / close hint shown beneath the exit line. `↵` restarts the shell in place;
/// `⌥w` closes the pane (this app's pane-close chord; the design's `⌘W` closes the tab).
const HINT: &str = "\u{21b5} restart    \u{2325}w close";
/// Vertical gap (logical px) between message lines.
const LINE_GAP: f32 = 8.0;

/// Build the centered "shell exited" message for a pane whose shell ended, as proportional
/// labels centered within `rect` (physical px). Reads only theme tokens (Hard rule 2): the
/// title in `fg.primary`, the exit detail in `diff.add` when clean or `diff.del` when it
/// failed / was signalled, and the restart/close hint in `accent`.
pub(crate) fn message_labels(
    status: &ExitStatus,
    rect: PxRect,
    scale: f32,
    theme: &Theme,
    measure: &mut TextMeasure,
) -> Vec<ProseLabel> {
    let detail = match &status.signal {
        Some(signal) => format!("killed by {signal}"),
        None => format!("exit code {}", status.code),
    };
    let detail_fg = if status.success() {
        theme.diff_add
    } else {
        theme.fg_muted
    };
    let lines: [(&str, FontRole, Srgb); 3] = [
        ("shell exited", FontRole::Title, theme.fg_primary),
        (&detail, FontRole::Body, detail_fg),
        (HINT, FontRole::Caption, theme.accent),
    ];
    let gap = LINE_GAP * scale;
    let total: f32 = lines
        .iter()
        .map(|(_, role, _)| measure.line_height(*role))
        .sum::<f32>()
        + gap * 2.0;
    let mut y = rect.y + (rect.h - total) * 0.5;
    let mut labels = Vec::new();
    for (text, role, color) in lines {
        let w = measure.width(text, role, None);
        labels.push(ProseLabel {
            text: text.to_owned(),
            x: rect.x + (rect.w - w) * 0.5,
            y,
            role,
            color,
            weight: None,
            max_w: f32::MAX,
        });
        y += measure.line_height(role) + gap;
    }
    labels
}

#[cfg(test)]
mod tests {
    use super::message_labels;
    use skelly_render::{PxRect, TextMeasure, Theme};
    use skelly_term::ExitStatus;

    fn rect() -> PxRect {
        PxRect {
            x: 0.0,
            y: 0.0,
            w: 800.0,
            h: 400.0,
        }
    }

    fn texts(status: &ExitStatus) -> String {
        let theme = Theme::resolve("ossein-dark");
        let mut m = TextMeasure::new(2.0);
        message_labels(status, rect(), 2.0, &theme, &mut m)
            .iter()
            .map(|l| l.text.clone())
            .collect::<Vec<_>>()
            .join("\n")
    }

    #[test]
    fn clean_exit_shows_code_and_restart_hint() {
        let text = texts(&ExitStatus {
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
        assert!(texts(&ExitStatus {
            code: 130,
            signal: None,
        })
        .contains("exit code 130"));
    }

    #[test]
    fn signalled_exit_names_the_signal() {
        assert!(texts(&ExitStatus {
            code: 1,
            signal: Some("SIGTERM".to_owned()),
        })
        .contains("killed by SIGTERM"));
    }

    #[test]
    fn detail_color_reflects_success() {
        let theme = Theme::resolve("ossein-dark");
        let mut m = TextMeasure::new(2.0);
        let clean = message_labels(
            &ExitStatus {
                code: 0,
                signal: None,
            },
            rect(),
            2.0,
            &theme,
            &mut m,
        );
        // The exit-detail line (index 1) is green on a clean exit.
        assert_eq!(clean[1].color, theme.diff_add);
        let failed = message_labels(
            &ExitStatus {
                code: 1,
                signal: None,
            },
            rect(),
            2.0,
            &theme,
            &mut m,
        );
        assert_ne!(failed[1].color, theme.diff_add, "a failed exit isn't green");
    }
}
