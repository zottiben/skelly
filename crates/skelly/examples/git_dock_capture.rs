//! Headless proof of the M4 git diff dock: render a live-ish pane workspace with the
//! per-repo git diff dock docked on the right (its file list, the selected file's unified
//! diff with add/del/hunk backgrounds, and the left-edge divider) to a PNG, with no
//! window or screen recording needed.
//!
//! The live binary drives the dock from its real `gitdock` module over a
//! `skelly_session` repo; examples cannot import the binary crate, so this hand-builds a
//! representative grid (as `settings_capture` does) purely to exercise the `gitdock_quads`
//! render path. An optional second arg picks the theme (`ossein-light`).
//! Run: `cargo run -p skelly --example git_dock_capture -- git-dock.png [theme]`.
#![allow(
    clippy::cast_precision_loss,
    clippy::cast_possible_truncation,
    clippy::cast_sign_loss,
    reason = "example: surface dimensions and grid sizes are small, non-negative values"
)]

use skelly_config::Appearance;
use skelly_render::{
    CaptureGitDock, CapturePane, Chrome, ChromeQuad, FontRole, GridCell, ProseLabel, PxRect, Srgb,
    TextMeasure, Theme,
};

/// Logical dock width - mirrors the binary's `GIT_DOCK_WIDTH` (420, the guide's default).
const DOCK_WIDTH: f32 = 420.0;
/// Logical window margin - mirrors the binary's `WINDOW_PAD`.
const WINDOW_PAD: f32 = 12.0;
/// Logical inner pane inset - mirrors the binary's `PANE_INSET`.
const PANE_INSET: f32 = 6.0;

fn main() {
    let path = std::env::args()
        .nth(1)
        .unwrap_or_else(|| "skelly-git-dock.png".to_owned());
    let (width, height, scale) = (1360_u32, 680_u32, 2.0_f64);

    let theme_name = std::env::args()
        .nth(2)
        .unwrap_or_else(|| "ossein-dark".to_owned());
    let appearance = Appearance {
        font_family: "SauceCodePro Nerd Font Mono".to_owned(),
        theme: theme_name,
        ..Appearance::default()
    };
    let theme = Theme::resolve(&appearance.theme);
    let sc = scale as f32;

    // One pane, inset on the left by the window margin and on the right by the dock -
    // exactly as the binary's `viewport_rect` insets it.
    let pad = WINDOW_PAD * sc;
    let inset = PANE_INSET * sc;
    let dock_w = DOCK_WIDTH * sc;
    let pane_rect = PxRect {
        x: pad,
        y: pad,
        w: width as f32 - dock_w - 2.0 * pad,
        h: height as f32 - 2.0 * pad,
    };
    let pane = CapturePane {
        rect: pane_rect,
        origin: (pane_rect.x + inset, pane_rect.y + inset),
        rows: terminal_rows(&theme),
        cursor: (11, 2),
        focused: true,
        logo: None,
    };

    // An optional third arg `norepo` renders the "Not a git repo" empty state (the Init
    // button) instead of the populated diff dock.
    let norepo = std::env::args().nth(3).as_deref() == Some("norepo");
    let dock = build_dock(width, height, sc, norepo, &theme);
    let rgba = skelly_render::capture_panes_rgba(
        &appearance,
        width,
        height,
        scale,
        &[pane],
        &Chrome {
            git_dock: Some(&dock),
            ..Default::default()
        },
    );

    let file = std::fs::File::create(&path).expect("create png");
    let mut encoder = png::Encoder::new(std::io::BufWriter::new(file), width, height);
    encoder.set_color(png::ColorType::Rgba);
    encoder.set_depth(png::BitDepth::Eight);
    encoder
        .write_header()
        .expect("png header")
        .write_image_data(&rgba)
        .expect("png data");
    println!("wrote {path}");
}

// Git-dock layout constants (logical px) - mirror the binary's `gitdock` module (§10.6).
const G_PAD_X: f32 = 12.0;
const G_PAD_TOP: f32 = 12.0;
const G_STATUS_H: f32 = 26.0;
const G_LABEL_H: f32 = 24.0;
const G_FILE_ROW_H: f32 = 26.0;
const G_DIFF_HEADER_H: f32 = 26.0;
const G_DIFF_ROW_H: f32 = 20.0;
const G_COMMIT_ROW_H: f32 = 24.0;
const G_COMMIT_BAND_H: f32 = 78.0;
const G_GUTTER_CHARS: usize = 4;
const G_DIFF_BG_ALPHA: f32 = 0.14;
const G_HUNK_BG_ALPHA: f32 = 0.08;

/// A representative git diff dock as a proportional display list, mirroring the binary's
/// `gitdock` layout: a status bar, a changed-file list (the selected file filled), the
/// selected file's diff (add/del/hunk backgrounds + mono code), and a commit box.
#[allow(
    clippy::too_many_lines,
    reason = "one straight-line representative dock builder mirroring the binary"
)]
fn build_dock(width: u32, height: u32, scale: f32, norepo: bool, theme: &Theme) -> CaptureGitDock {
    let mut m = TextMeasure::new(scale);
    let dock_w = DOCK_WIDTH * scale;
    let panel = PxRect {
        x: width as f32 - dock_w,
        y: 0.0,
        w: dock_w,
        h: height as f32,
    };
    let cx = panel.x + G_PAD_X * scale;
    let cr = panel.x + panel.w - G_PAD_X * scale;
    let mut quads = Vec::new();
    let mut labels = Vec::new();

    if norepo {
        let mid = panel.y + panel.h * 0.5;
        gpush_center(
            &mut labels,
            &mut m,
            "No repository here",
            FontRole::Body,
            theme.fg_muted,
            panel,
            mid,
        );
        let by = mid + 34.0 * scale;
        let w = m.width("Init repo  \u{21a9}", FontRole::Body, None);
        let bx = panel.x + (panel.w - w) * 0.5;
        quads.push(ChromeQuad::rounded(
            PxRect {
                x: bx - 10.0 * scale,
                y: by,
                w: w + 20.0 * scale,
                h: 28.0 * scale,
            },
            theme.accent_subtle_on(theme.bg_base.to_srgb()),
            6.0 * scale,
        ));
        gpush(
            &mut labels,
            &mut m,
            "Init repo  \u{21a9}",
            FontRole::Body,
            theme.accent,
            bx,
            by,
            28.0,
            scale,
        );
        return CaptureGitDock {
            panel,
            quads,
            labels,
        };
    }

    // Status bar.
    let sy = panel.y + G_PAD_TOP * scale;
    let mut x = cx;
    gpush(
        &mut labels,
        &mut m,
        "\u{2442} main",
        FontRole::Mono,
        theme.diff_hunk,
        x,
        sy,
        G_STATUS_H,
        scale,
    );
    x += m.width("\u{2442} main", FontRole::Mono, None) + 10.0 * scale;
    gpush(
        &mut labels,
        &mut m,
        "\u{2191}2 \u{2193}1",
        FontRole::Mono,
        theme.fg_muted,
        x,
        sy,
        G_STATUS_H,
        scale,
    );
    let mut end = gpush_right(
        &mut labels,
        &mut m,
        "esc",
        FontRole::Caption,
        theme.fg_muted,
        cr,
        sy,
        G_STATUS_H,
        scale,
    ) - 10.0 * scale;
    end = gpush_right(
        &mut labels,
        &mut m,
        "-45",
        FontRole::Mono,
        theme.diff_del,
        end,
        sy,
        G_STATUS_H,
        scale,
    ) - 8.0 * scale;
    gpush_right(
        &mut labels,
        &mut m,
        "+122",
        FontRole::Mono,
        theme.diff_add,
        end,
        sy,
        G_STATUS_H,
        scale,
    );

    // Section label.
    let mut y = sy + G_STATUS_H * scale;
    gpush(
        &mut labels,
        &mut m,
        "CHANGED - 3",
        FontRole::Micro,
        theme.fg_muted,
        cx,
        y,
        G_LABEL_H,
        scale,
    );
    gpush_right(
        &mut labels,
        &mut m,
        "space stage  a all",
        FontRole::Caption,
        theme.fg_muted,
        cr,
        y,
        G_LABEL_H,
        scale,
    );
    y += G_LABEL_H * scale;

    // File rows.
    let files = [
        (
            "[x]",
            'M',
            "src/pane/tree.rs",
            42u32,
            11u32,
            theme.diff_hunk,
            true,
        ),
        (
            "[x]",
            'A',
            "src/session/timeline.rs",
            80,
            0,
            theme.diff_add,
            false,
        ),
        ("[ ]", 'D', "old/legacy.rs", 0, 34, theme.diff_del, false),
    ];
    for (i, (check, letter, path, add, del, lfg, selected)) in files.iter().enumerate() {
        let top = y + i as f32 * G_FILE_ROW_H * scale;
        if *selected {
            // accent.subtle selected-row fill, sRGB-composited over bg.base (mirrors the binary).
            quads.push(ChromeQuad::fill(
                PxRect {
                    x: panel.x,
                    y: top,
                    w: panel.w,
                    h: G_FILE_ROW_H * scale,
                },
                theme.accent_subtle_on(theme.bg_base.to_srgb()),
            ));
        }
        let mut fx = cx;
        let check_fg = if *check == "[x]" {
            theme.diff_add
        } else {
            theme.fg_muted
        };
        gpush(
            &mut labels,
            &mut m,
            check,
            FontRole::Mono,
            check_fg,
            fx,
            top,
            G_FILE_ROW_H,
            scale,
        );
        fx += m.width("[x] ", FontRole::Mono, None);
        gpush(
            &mut labels,
            &mut m,
            &letter.to_string(),
            FontRole::Mono,
            *lfg,
            fx,
            top,
            G_FILE_ROW_H,
            scale,
        );
        fx += m.width("M  ", FontRole::Mono, None);
        let counts_left = gpush_counts(
            &mut labels,
            &mut m,
            cr,
            *add,
            *del,
            top,
            G_FILE_ROW_H,
            scale,
            theme,
        );
        let name_fg = if *selected {
            theme.fg_primary
        } else {
            theme.fg_secondary
        };
        labels.push(ProseLabel {
            text: (*path).to_owned(),
            x: fx,
            y: top + (G_FILE_ROW_H * scale - m.line_height(FontRole::Body)) * 0.5,
            role: FontRole::Body,
            color: name_fg,
            weight: None,
            max_w: (counts_left - 8.0 * scale - fx).max(1.0),
        });
    }
    y += files.len() as f32 * G_FILE_ROW_H * scale;

    // Diff header.
    y += 6.0 * scale;
    gpush(
        &mut labels,
        &mut m,
        "src/pane/tree.rs",
        FontRole::Label,
        theme.fg_secondary,
        cx,
        y,
        G_DIFF_HEADER_H,
        scale,
    );
    gpush_counts(
        &mut labels,
        &mut m,
        cr,
        42,
        11,
        y,
        G_DIFF_HEADER_H,
        scale,
        theme,
    );
    y += G_DIFF_HEADER_H * scale;

    // Diff body.
    let gutter_w = m.width(&"0".repeat(G_GUTTER_CHARS), FontRole::Mono, None);
    let content_bottom = panel.y + panel.h - G_COMMIT_BAND_H * scale;
    let lines: &[(char, &str, u32, &str)] = &[
        ('@', "@@ -18,7 +18,9 @@ impl PaneTree", 0, ""),
        (' ', "", 18, "fn split(&mut self, dir: Dir) {"),
        (' ', "", 19, "    let node = self.focused();"),
        ('-', "", 20, "    node.grow(dir);"),
        ('+', "", 20, "    if self.count() >= 8 { return; }"),
        ('+', "", 21, "    node.grow(dir);"),
        (' ', "", 22, "    self.rebalance();"),
        (' ', "", 23, "}"),
    ];
    let row_h = G_DIFF_ROW_H * scale;
    for (i, (sign, hunk, gutter, text)) in lines.iter().enumerate() {
        let ry = y + i as f32 * row_h;
        if ry + row_h > content_bottom {
            break;
        }
        let full = PxRect {
            x: panel.x,
            y: ry,
            w: panel.w,
            h: row_h,
        };
        // Diff-row backgrounds pre-composite in sRGB over bg.base (mirrors the binary).
        let base = theme.bg_base.to_srgb();
        if *sign == '@' {
            // A focused hunk header: an accent wash over a diff.hunk wash over bg.base.
            let hunk_bg = theme
                .accent
                .over(theme.diff_hunk.over(base, G_HUNK_BG_ALPHA), G_DIFF_BG_ALPHA);
            quads.push(ChromeQuad::fill(full, hunk_bg));
            gpush(
                &mut labels,
                &mut m,
                hunk,
                FontRole::Mono,
                theme.diff_hunk,
                cx,
                ry,
                G_DIFF_ROW_H,
                scale,
            );
            gpush_right(
                &mut labels,
                &mut m,
                "stage \u{2318}\u{21a9}",
                FontRole::Micro,
                theme.accent,
                cr,
                ry,
                G_DIFF_ROW_H,
                scale,
            );
        } else {
            let (fg, bg) = match sign {
                '+' => (theme.diff_add, Some(theme.diff_add)),
                '-' => (theme.diff_del, Some(theme.diff_del)),
                _ => (theme.fg_secondary, None),
            };
            if let Some(b) = bg {
                quads.push(ChromeQuad::fill(full, b.over(base, G_DIFF_BG_ALPHA)));
            }
            let gutter_fg = if *sign == ' ' { theme.fg_muted } else { fg };
            gpush_right(
                &mut labels,
                &mut m,
                &gutter.to_string(),
                FontRole::Mono,
                gutter_fg,
                cx + gutter_w,
                ry,
                G_DIFF_ROW_H,
                scale,
            );
            let code_x = cx + gutter_w + m.width("  ", FontRole::Mono, None);
            labels.push(ProseLabel {
                text: format!("{sign} {text}"),
                x: code_x,
                y: ry + (row_h - m.line_height(FontRole::Mono)) * 0.5,
                role: FontRole::Mono,
                color: fg,
                weight: None,
                max_w: (cr - code_x).max(1.0),
            });
        }
    }

    // Commit box.
    let ctop = content_bottom;
    quads.push(ChromeQuad::fill(
        PxRect {
            x: panel.x,
            y: ctop,
            w: panel.w,
            h: scale.max(1.0),
        },
        theme.border,
    ));
    let iy = ctop + 10.0 * scale;
    gpush(
        &mut labels,
        &mut m,
        "\u{203a}",
        FontRole::Mono,
        theme.accent,
        cx,
        iy,
        G_COMMIT_ROW_H,
        scale,
    );
    let msg_x = cx + m.width("\u{203a} ", FontRole::Mono, None);
    let msg = "feat: rewindable session timeline";
    gpush(
        &mut labels,
        &mut m,
        msg,
        FontRole::Body,
        theme.fg_primary,
        msg_x,
        iy,
        G_COMMIT_ROW_H,
        scale,
    );
    let caret_x = msg_x + m.width(msg, FontRole::Body, None);
    let line_h = m.line_height(FontRole::Body);
    quads.push(ChromeQuad::fill(
        PxRect {
            x: caret_x,
            y: iy + (G_COMMIT_ROW_H * scale - line_h) * 0.5,
            w: (2.0 * scale).max(1.0),
            h: line_h,
        },
        theme.accent,
    ));
    let stat_y = iy + G_COMMIT_ROW_H * scale;
    gpush(
        &mut labels,
        &mut m,
        "2 staged",
        FontRole::Caption,
        theme.fg_muted,
        cx,
        stat_y,
        G_COMMIT_ROW_H,
        scale,
    );
    gpush_right(
        &mut labels,
        &mut m,
        "enter commit  esc back",
        FontRole::Caption,
        theme.fg_muted,
        cr,
        stat_y,
        G_COMMIT_ROW_H,
        scale,
    );

    CaptureGitDock {
        panel,
        quads,
        labels,
    }
}

/// Push a left-anchored label vertically centered in a `row_h` row.
#[allow(clippy::too_many_arguments, reason = "one focused example helper")]
fn gpush(
    labels: &mut Vec<ProseLabel>,
    m: &mut TextMeasure,
    text: &str,
    role: FontRole,
    color: Srgb,
    x: f32,
    top: f32,
    row_h: f32,
    scale: f32,
) {
    let line_h = m.line_height(role);
    labels.push(ProseLabel {
        text: text.to_owned(),
        x,
        y: top + (row_h * scale - line_h) * 0.5,
        role,
        color,
        weight: None,
        max_w: f32::MAX,
    });
}

/// Push a right-anchored label ending at `right`; returns its left edge.
#[allow(clippy::too_many_arguments, reason = "one focused example helper")]
fn gpush_right(
    labels: &mut Vec<ProseLabel>,
    m: &mut TextMeasure,
    text: &str,
    role: FontRole,
    color: Srgb,
    right: f32,
    top: f32,
    row_h: f32,
    scale: f32,
) -> f32 {
    let x = right - m.width(text, role, None);
    gpush(labels, m, text, role, color, x, top, row_h, scale);
    x
}

/// Push a horizontally-centered label at physical `top`.
fn gpush_center(
    labels: &mut Vec<ProseLabel>,
    m: &mut TextMeasure,
    text: &str,
    role: FontRole,
    color: Srgb,
    panel: PxRect,
    top: f32,
) {
    let w = m.width(text, role, None);
    labels.push(ProseLabel {
        text: text.to_owned(),
        x: panel.x + (panel.w - w) * 0.5,
        y: top,
        role,
        color,
        weight: None,
        max_w: f32::MAX,
    });
}

/// Push a right-anchored `+add -del` count pair; returns the left edge of the pair.
#[allow(clippy::too_many_arguments, reason = "one focused example helper")]
fn gpush_counts(
    labels: &mut Vec<ProseLabel>,
    m: &mut TextMeasure,
    cr: f32,
    add: u32,
    del: u32,
    top: f32,
    row_h: f32,
    scale: f32,
    theme: &Theme,
) -> f32 {
    let mut end = gpush_right(
        labels,
        m,
        &format!("-{del}"),
        FontRole::Mono,
        theme.diff_del,
        cr,
        top,
        row_h,
        scale,
    );
    end = gpush_right(
        labels,
        m,
        &format!("+{add}"),
        FontRole::Mono,
        theme.diff_add,
        end - 8.0 * scale,
        top,
        row_h,
        scale,
    );
    end
}

fn ui_cell(c: char, fg: Srgb) -> GridCell {
    GridCell {
        c,
        fg,
        bg: None,
        bold: false,
        italic: false,
        underline: false,
    }
}

/// A few plausible terminal lines behind the dock, so the capture shows the dock over a
/// live pane (not a blank one). Colored with the UI foreground for simplicity.
fn terminal_rows(theme: &Theme) -> Vec<Vec<GridCell>> {
    let lines = [
        "~/skelly main > git status",
        "On branch main",
        "Changes not staged for commit:",
        "  modified: src/pane/tree.rs",
    ];
    lines
        .iter()
        .map(|line| {
            line.chars()
                .map(|c| ui_cell(c, theme.fg_secondary))
                .collect()
        })
        .collect()
}
