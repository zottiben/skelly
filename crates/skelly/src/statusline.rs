//! The per-pane status line (design §08 anatomy #9 / §10.3): a 24px strip at the bottom of
//! each pane showing the working directory, the git branch, the shell, and the cursor
//! position. Pure layout: it turns the [`Info`] the binary gathers into a proportional
//! display list (a `bg.inset` strip + a `border.subtle` top hairline + `mono` labels), which
//! the binary draws through the pane-overlay pass. Kept here so the layout is unit-testable
//! without a GPU.
//!
//! v1 shows the data Skelly actually has - `cwd · ⑂ branch · shell … Ln, Col`; the guide's
//! editor `mode` (NORMAL/INSERT) and `filetype` segments wait on shell/editor integration.

use skelly_render::{ChromeQuad, FontRole, ProseLabel, PxRect, Theme};

/// Logical height (px) of the status strip (the guide's 24px status line).
pub(crate) const HEIGHT: f32 = 24.0;
/// Logical horizontal padding inside the strip (the guide's `padding:0 14px`).
const PAD_X: f32 = 14.0;
/// Logical gap between segments (the guide's `gap:14px`).
const GAP: f32 = 14.0;

/// The status-line data for one pane. `cwd`/`branch`/`shell` are process-level; `cursor` is
/// the pane's terminal cursor `(column, row)`, 0-based.
pub(crate) struct Info<'a> {
    pub(crate) cwd: &'a str,
    pub(crate) branch: Option<&'a str>,
    pub(crate) shell: &'a str,
    pub(crate) cursor: (usize, usize),
}

/// Build the pane's status-line display list within `rect` (the pane rectangle, physical px):
/// a `bg.inset` strip along the bottom `HEIGHT` with a `border.subtle` top hairline, the cwd
/// (`status.success`) · `⑂ branch` (`status.info`) · shell (muted) on the left, and `Ln, Col`
/// (muted) right-anchored.
///
/// A pane can be narrow (up to 8 splits), so the segments never overlap: the right-anchored
/// `Ln, Col` is placed first, then the left segments fill the space that remains, in priority
/// order (cwd, then branch, then shell). The cwd is truncated with a leading `…` if it alone
/// would collide; lower-priority segments are dropped whole when they no longer fit.
#[allow(
    clippy::cast_possible_truncation,
    clippy::cast_sign_loss,
    reason = "avail/char_w is a small positive cell count (guarded avail >= char_w > 0)"
)]
pub(crate) fn paint(
    info: &Info,
    rect: PxRect,
    scale: f32,
    theme: &Theme,
    measure: &mut skelly_render::TextMeasure,
) -> (Vec<ChromeQuad>, Vec<ProseLabel>) {
    let h = HEIGHT * scale;
    let top = rect.y + rect.h - h;
    let pad = PAD_X * scale;
    let gap = GAP * scale;
    let line = measure.line_height(FontRole::Mono);
    let cy = top + (h - line) * 0.5;
    let quads = vec![
        ChromeQuad::fill(
            PxRect {
                x: rect.x,
                y: top,
                w: rect.w,
                h,
            },
            theme.bg_inset,
        ),
        // The top hairline separating the strip from the terminal grid.
        ChromeQuad::fill(
            PxRect {
                x: rect.x,
                y: top,
                w: rect.w,
                h: scale.max(1.0),
            },
            theme.border_subtle,
        ),
    ];
    let mut labels = Vec::new();
    let label = |text: String, x: f32, color| ProseLabel {
        text,
        x,
        y: cy,
        role: FontRole::Mono,
        color,
        weight: None,
        max_w: f32::MAX,
    };

    // Right-anchored cursor position, placed first so the left segments can avoid it. Dropped
    // only if it cannot fit the pad-to-pad span at all (a vanishingly narrow pane).
    let pos = format!("Ln {}, Col {}", info.cursor.1 + 1, info.cursor.0 + 1);
    let pos_w = measure.width(&pos, FontRole::Mono, None);
    // The x past which left content would touch the right column (or the right pad if there is
    // no room for the cursor readout).
    let left_limit = if pos_w + 2.0 * pad <= rect.w {
        labels.push(label(pos, rect.x + rect.w - pad - pos_w, theme.fg_muted));
        rect.x + rect.w - pad - pos_w - gap
    } else {
        rect.x + rect.w - pad
    };

    // Left segments, in priority order: cwd (truncated to fit), then branch, then shell (each
    // dropped whole once it no longer fits before `left_limit`).
    let char_w = measure.width("M", FontRole::Mono, None).max(f32::EPSILON);
    let mut x = rect.x + pad;
    let avail = left_limit - x;
    if avail >= char_w {
        let cwd = fit_lead(info.cwd, (avail / char_w) as usize);
        let w = measure.width(&cwd, FontRole::Mono, None);
        labels.push(label(cwd, x, theme.diff_add));
        x += w + gap;
    }
    if let Some(branch) = info.branch {
        let seg = format!("\u{2442} {branch}");
        let w = measure.width(&seg, FontRole::Mono, None);
        if x + w <= left_limit {
            labels.push(label(seg, x, theme.diff_hunk));
            x += w + gap;
        }
    }
    if !info.shell.is_empty() {
        let w = measure.width(info.shell, FontRole::Mono, None);
        if x + w <= left_limit {
            labels.push(label(info.shell.to_owned(), x, theme.fg_muted));
        }
    }
    (quads, labels)
}

/// `s` shortened to at most `max_chars` monospace cells, keeping the tail (the most specific
/// part of a path) behind a leading `…`. Returns `s` unchanged when it already fits.
fn fit_lead(s: &str, max_chars: usize) -> String {
    let count = s.chars().count();
    if count <= max_chars {
        return s.to_owned();
    }
    if max_chars <= 1 {
        return "\u{2026}".to_owned();
    }
    let tail: String = s.chars().skip(count - (max_chars - 1)).collect();
    format!("\u{2026}{tail}")
}

#[cfg(test)]
mod tests {
    use super::{fit_lead, paint, Info};
    use skelly_render::{FontRole, PxRect, TextMeasure, Theme};

    fn rect() -> PxRect {
        PxRect {
            x: 0.0,
            y: 0.0,
            w: 800.0,
            h: 400.0,
        }
    }

    #[test]
    fn paint_shows_cwd_branch_shell_and_cursor() {
        let theme = Theme::resolve("ossein-dark");
        let mut m = TextMeasure::new(2.0);
        let info = Info {
            cwd: "~/skelly",
            branch: Some("main"),
            shell: "zsh",
            cursor: (2, 3),
        };
        let (quads, labels) = paint(&info, rect(), 2.0, &theme, &mut m);
        let joined = labels
            .iter()
            .map(|l| l.text.as_str())
            .collect::<Vec<_>>()
            .join("\n");
        assert!(joined.contains("~/skelly"), "cwd: {joined}");
        assert!(joined.contains("\u{2442} main"), "branch");
        assert!(joined.contains("zsh"), "shell");
        // Cursor is 1-based: (col 2, row 3) -> Ln 4, Col 3.
        assert!(joined.contains("Ln 4, Col 3"), "cursor: {joined}");
        // The strip fill + its top hairline.
        assert_eq!(quads.len(), 2);
        // The cwd draws in the success color, the branch in info.
        assert!(labels.iter().any(|l| l.color == theme.diff_add));
        assert!(labels.iter().any(|l| l.color == theme.diff_hunk));
    }

    #[test]
    fn a_detached_head_omits_the_branch_segment() {
        let theme = Theme::resolve("ossein-dark");
        let mut m = TextMeasure::new(2.0);
        let info = Info {
            cwd: "~/tmp",
            branch: None,
            shell: "bash",
            cursor: (0, 0),
        };
        let (_, labels) = paint(&info, rect(), 2.0, &theme, &mut m);
        assert!(
            labels.iter().all(|l| !l.text.contains('\u{2442}')),
            "no branch glyph"
        );
    }

    #[test]
    fn narrow_pane_fits_without_overlap() {
        // A ~220px-wide pane cannot hold every segment; the layout drops the low-priority ones
        // and never lets a left label run into the right-anchored cursor readout.
        let theme = Theme::resolve("ossein-dark");
        let mut m = TextMeasure::new(2.0);
        let info = Info {
            cwd: "~/work/skelly/crates/skelly-render",
            branch: Some("feat/m5-hardening"),
            shell: "zsh",
            cursor: (12, 340),
        };
        let narrow = PxRect {
            x: 0.0,
            y: 0.0,
            w: 460.0,
            h: 200.0,
        };
        let (_, labels) = paint(&info, narrow, 2.0, &theme, &mut m);
        // The cursor readout is right-anchored; every other label must end before it starts.
        let cursor = labels
            .iter()
            .find(|l| l.text.starts_with("Ln "))
            .expect("cursor readout present");
        for l in &labels {
            if std::ptr::eq(l, cursor) {
                continue;
            }
            let end = l.x + m.width(&l.text, FontRole::Mono, None);
            assert!(
                end <= cursor.x + 0.5,
                "label {:?} (ends {end}) overlaps cursor at {}",
                l.text,
                cursor.x
            );
        }
    }

    #[test]
    fn fit_lead_keeps_the_path_tail_behind_an_ellipsis() {
        assert_eq!(fit_lead("~/skelly", 20), "~/skelly");
        assert_eq!(fit_lead("~/work/skelly-render", 8), "\u{2026}-render");
        assert_eq!(fit_lead("anything", 1), "\u{2026}");
    }
}
