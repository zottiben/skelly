//! Headless proof of the M3 pane workspace: build a real two-pane split with the
//! `skelly-pane` tree, spawn a live shell in each pane, and render the tiled result
//! (the left sidebar / tab list, pane dividers, the focused-pane ring, and a cursor
//! only in the focused pane, with a command palette on top) to a PNG, with no window
//! or screen-recording needed.
//! Run: `cargo run -p skelly --example pane_capture -- panes.png`.
#![allow(
    clippy::cast_precision_loss,
    clippy::cast_possible_truncation,
    clippy::cast_sign_loss,
    reason = "example: surface dimensions and grid sizes are small, non-negative values"
)]
#![allow(
    clippy::too_many_lines,
    reason = "example: one straight-line scene builder mirroring the binary"
)]

use std::thread::sleep;
use std::time::{Duration, Instant};

use skelly_config::Appearance;
use skelly_pane::{Dir, PaneTree, Rect};
use skelly_render::{
    measure_cell, AnsiPalette, CaptureOverlay, CapturePane, CaptureSidebar, ChromeQuad, FontRole,
    GridCell, PaneOverlay, ProseLabel, PxRect, Srgb, TextMeasure, Theme,
};
use skelly_term::{CellAttrs, CellColor, TermCell, Terminal};

/// Logical padding around the whole pane area - mirrors the binary's `WINDOW_PAD`.
const WINDOW_PAD: f32 = 12.0;
/// Logical inset inside each pane - mirrors the binary's `PANE_INSET`.
const PANE_INSET: f32 = 6.0;
/// Logical height of the macOS control strip - mirrors the binary's `TITLE_STRIP`.
const TITLE_STRIP: f32 = 38.0;
/// Logical sidebar width - mirrors the config default (`[sidebar] width = 240`).
const SIDEBAR_WIDTH: f32 = 240.0;
/// Logical width of the slim icon rail - mirrors the binary's `RAIL_WIDTH`.
const RAIL_WIDTH: f32 = 56.0;
/// Logical height of the per-pane status line - mirrors the binary's `statusline::HEIGHT`.
const STATUS_H: f32 = 24.0;
/// Status-line padding/gap - mirror the binary's `statusline` module (the guide's 14px).
const STATUS_PAD_X: f32 = 14.0;
const STATUS_GAP: f32 = 14.0;

fn main() {
    let path = std::env::args()
        .nth(1)
        .unwrap_or_else(|| "skelly-panes.png".to_owned());
    let (width, height, scale) = (1360_u32, 680_u32, 2.0_f64);

    // Use an installed Nerd Font so the configured-font path is exercised. An optional
    // second arg picks the theme (e.g. `ossein-light`), exercising live-theming tokens.
    let theme = std::env::args()
        .nth(2)
        .unwrap_or_else(|| "ossein-dark".to_owned());
    let appearance = Appearance {
        font_family: "SauceCodePro Nerd Font Mono".to_owned(),
        theme,
        ..Appearance::default()
    };
    // An optional third arg picks the sidebar mode: `rail` = the slim 56px icon rail,
    // anything else (default) = the full-width panel.
    let rail = std::env::args().nth(3).as_deref() == Some("rail");
    // `overflow` renders a many-tab full panel scrolled so the active tab stays in view,
    // exercising the tab-list windowing + the ↑/↓ overflow indicators (design §12).
    let overflow = std::env::args().nth(3).as_deref() == Some("overflow");
    // `solo` renders a single full-width pane with no palette overlay - the wide context the
    // guide's §10.3 mockup shows, so the whole status line (cwd · ⑂ branch · shell … Ln, Col)
    // is visible for parity comparison.
    let solo = std::env::args().nth(3).as_deref() == Some("solo");

    let (cell_w, cell_h) = measure_cell(&appearance, scale);
    let sc = scale as f32;
    let pad = WINDOW_PAD * sc;
    let inset = PANE_INSET * sc;
    // The macOS control strip (§08 anatomy #1) is a SIDEBAR concern: the traffic lights sit
    // top-left over the sidebar, so only the sidebar reserves this band (its content clears it,
    // its bg fills it). The pane zone fills to the window top like the guide's content zone.
    // The OS-drawn lights cannot appear in a headless capture.
    let strip = TITLE_STRIP * sc;
    let sidebar_w = if rail { RAIL_WIDTH } else { SIDEBAR_WIDTH } * sc;
    let viewport = Rect::new(
        sidebar_w + pad,
        pad,
        width as f32 - sidebar_w - 2.0 * pad,
        height as f32 - 2.0 * pad,
    );

    // Two panes, side by side; focus lands on the new (right) pane, matching the binary's
    // split behavior. `solo` keeps a single full-width pane instead.
    let mut tree = PaneTree::new();
    let left = tree.focused();
    if !solo {
        tree.split(Dir::Right).expect("under the pane cap");
    }
    let focused = tree.focused();
    let layout = tree.layout(viewport);

    let palette = AnsiPalette::resolve(&appearance.theme);
    let mut panes = Vec::new();
    for (id, rect) in &layout {
        let cols = ((rect.w - 2.0 * inset) / cell_w).floor().max(2.0) as u16;
        // Reserve the status-line strip at the bottom, as the binary's `pane_dims` does.
        let rows = ((rect.h - 2.0 * inset - STATUS_H * sc) / cell_h)
            .floor()
            .max(1.0) as u16;

        let mut term = Terminal::spawn(cols, rows, || {}).expect("spawn shell");
        wait_until(&term, Duration::from_secs(6), |t| {
            t.snapshot().iter().any(|line| !line.is_empty())
        });
        sleep(Duration::from_millis(300));

        // Distinct, colored content per pane so the split reads clearly.
        let cmd = if *id == left {
            "clear; \
             printf '\\033[35m\\033[0m left pane \\342\\200\\224 editor\\n\\n'; \
             printf '\\033[36m  1\\033[0m fn main() {\\n'; \
             printf '\\033[36m  2\\033[0m     println!(\"skelly\");\\n'; \
             printf '\\033[36m  3\\033[0m }\\n'; \
             printf 'PANE_READY\\n'\n"
        } else {
            "clear; \
             printf '\\033[32m\\033[0m right pane \\342\\200\\224 shell\\n\\n'; \
             printf '$ \\033[1mgit status\\033[0m\\n'; \
             printf '\\033[32m  modified:\\033[0m src/pane.rs\\n'; \
             printf '\\033[33m  branch:\\033[0m feat/m3\\n'; \
             printf 'PANE_READY\\n'\n"
        };
        term.write(cmd.as_bytes());
        wait_until(&term, Duration::from_secs(15), |t| {
            t.snapshot().iter().any(|line| line.contains("PANE_READY"))
        });
        sleep(Duration::from_millis(300));

        let grid: Vec<Vec<GridCell>> = term
            .cells()
            .iter()
            .map(|row| row.iter().map(|c| resolve_cell(c, &palette)).collect())
            .collect();
        panes.push(CapturePane {
            rect: PxRect {
                x: rect.x,
                y: rect.y,
                w: rect.w,
                h: rect.h,
            },
            origin: (rect.x + inset, rect.y + inset),
            rows: grid,
            cursor: term.cursor(),
            focused: *id == focused,
            logo: None,
        });
    }

    // The left sidebar (a two-tab list, tab 1 active) + an overlay over the panes,
    // verifying the sidebar chrome and the overlay compositing together. The overlay is
    // the command palette by default, or the "running job" confirm modal for `confirm`.
    let theme = Theme::resolve(&appearance.theme);
    // The per-pane status line at each pane's bottom (§08 anatomy #9) - mirrors the binary's
    // `pane_overlay_paint`: the same process cwd/branch/shell, each pane's own cursor.
    let mut measure = TextMeasure::new(sc);
    let mut pane_overlay = PaneOverlay::default();
    for pane in &panes {
        let (q, l) = status_line(
            pane.rect,
            "~/skelly",
            Some("main"),
            "zsh",
            pane.cursor,
            sc,
            &theme,
            &mut measure,
        );
        pane_overlay.quads.extend(q);
        pane_overlay.labels.extend(l);
    }
    let sidebar = sidebar_panel(height, sidebar_w, strip, sc, rail, overflow, &theme);
    // `solo` shows the bare pane + status line (no overlay); otherwise the palette (or the
    // confirm modal) composites on top.
    let overlay = if solo {
        None
    } else if std::env::args().nth(3).as_deref() == Some("confirm") {
        Some(confirm_overlay(width, height, sc, &theme))
    } else {
        Some(palette_overlay(width, height, sc, &theme))
    };
    let rgba = skelly_render::capture_panes_rgba(
        &appearance,
        width,
        height,
        scale,
        &panes,
        &skelly_render::Chrome {
            pane_overlay,
            sidebar: Some(&sidebar),
            overlay: overlay.as_ref(),
            ..Default::default()
        },
    );

    write_png(&path, width, height, &rgba);
    println!("wrote {path} ({} panes)", panes.len());
}

/// Build one pane's status-line display list - mirrors the binary's `statusline::paint`
/// (a `bg.inset` strip + a `border.subtle` top hairline; the cwd in `diff.add`, `⑂ branch`
/// in `diff.hunk`, the shell muted, and `Ln, Col` right-anchored, all in `FontRole::Mono`).
#[allow(
    clippy::too_many_arguments,
    reason = "one focused mirror of the binary helper"
)]
fn status_line(
    rect: PxRect,
    cwd: &str,
    branch: Option<&str>,
    shell: &str,
    cursor: (usize, usize),
    scale: f32,
    theme: &Theme,
    m: &mut TextMeasure,
) -> (Vec<ChromeQuad>, Vec<ProseLabel>) {
    let h = STATUS_H * scale;
    let top = rect.y + rect.h - h;
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
    let line = m.line_height(FontRole::Mono);
    let cy = top + (h - line) * 0.5;
    let pad = STATUS_PAD_X * scale;
    let gap = STATUS_GAP * scale;
    let label = |text: String, x: f32, color| ProseLabel {
        text,
        x,
        y: cy,
        role: FontRole::Mono,
        color,
        weight: None,
        max_w: f32::MAX,
    };

    // Right-anchored cursor readout first, then fit the left segments before it (mirrors the
    // binary's `statusline::paint`: never overlap in a narrow split).
    let pos = format!("Ln {}, Col {}", cursor.1 + 1, cursor.0 + 1);
    let pos_w = m.width(&pos, FontRole::Mono, None);
    let left_limit = if pos_w + 2.0 * pad <= rect.w {
        labels.push(label(pos, rect.x + rect.w - pad - pos_w, theme.fg_muted));
        rect.x + rect.w - pad - pos_w - gap
    } else {
        rect.x + rect.w - pad
    };

    let char_w = m.width("M", FontRole::Mono, None).max(f32::EPSILON);
    let mut x = rect.x + pad;
    let avail = left_limit - x;
    if avail >= char_w {
        let cwd = fit_lead(cwd, (avail / char_w) as usize);
        let w = m.width(&cwd, FontRole::Mono, None);
        labels.push(label(cwd, x, theme.diff_add));
        x += w + gap;
    }
    if let Some(branch) = branch {
        let seg = format!("\u{2442} {branch}");
        let w = m.width(&seg, FontRole::Mono, None);
        if x + w <= left_limit {
            labels.push(label(seg, x, theme.diff_hunk));
            x += w + gap;
        }
    }
    // The dirty indicator `●+A −R` (accent), mirroring the binary (representative counts).
    let dirty = "\u{25cf}+2 \u{2212}1";
    let dw = m.width(dirty, FontRole::Mono, None);
    if x + dw <= left_limit {
        labels.push(label(dirty.to_owned(), x, theme.accent));
        x += dw + gap;
    }
    let w = m.width(shell, FontRole::Mono, None);
    if x + w <= left_limit {
        labels.push(label(shell.to_owned(), x, theme.fg_muted));
    }
    (quads, labels)
}

/// `s` shortened to at most `max_chars` monospace cells behind a leading `…` - mirrors the
/// binary's `statusline::fit_lead`.
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

/// Encode tight RGBA8 bytes to a PNG at `path`.
fn write_png(path: &str, width: u32, height: u32, rgba: &[u8]) {
    let file = std::fs::File::create(path).expect("create png");
    let mut encoder = png::Encoder::new(std::io::BufWriter::new(file), width, height);
    encoder.set_color(png::ColorType::Rgba);
    encoder.set_depth(png::BitDepth::Eight);
    encoder
        .write_header()
        .expect("png header")
        .write_image_data(rgba)
        .expect("png data");
}

// Palette layout constants (logical px) - mirror the binary's `palette` module (§09).
const PL_PAD: f32 = 14.0;
const PL_ROW_INSET: f32 = 10.0;
const PL_INPUT_H: f32 = 34.0;
const PL_COUNT_H: f32 = 22.0;
const PL_CMD_H: f32 = 30.0;
const PL_SPACER_H: f32 = 8.0;
const PL_FOOTER_H: f32 = 24.0;
const PL_HINT_GAP: f32 = 24.0;
const PL_PILL_RADIUS: f32 = 8.0;
const PL_CAT_H: f32 = 20.0;
const PL_ICON_GAP: f32 = 12.0;
const PL_FOOTER: &str = "up/down navigate    enter run    esc close";

/// Build a representative command-palette overlay (input, count, a couple of command rows
/// with the first selected, footer) as a proportional display list - mirroring the binary's
/// `palette` module so the capture verifies the overlay. The live binary drives the real one.
#[allow(
    clippy::too_many_lines,
    reason = "one straight-line representative overlay builder mirroring the binary"
)]
fn palette_overlay(width: u32, height: u32, scale: f32, theme: &Theme) -> CaptureOverlay {
    let mut m = TextMeasure::new(scale);
    // (icon, label, hint) - two rows under one "PANES" category header (§10.8).
    let cmds = [
        ("\u{2922}", "Zoom / unzoom pane", "opt Z"),
        ("\u{229E}", "Even out splits", "opt ="),
    ];
    let inset = (PL_PAD + PL_ROW_INSET) * scale;
    let mut content_w = m
        .width(PL_FOOTER, FontRole::Caption, None)
        .max(m.width("> ", FontRole::Body, None) + m.width("zoom", FontRole::Body, None));
    for (ic, l, h) in cmds {
        content_w = content_w.max(
            m.width(ic, FontRole::Body, None)
                + PL_ICON_GAP * scale
                + m.width(l, FontRole::Body, None)
                + PL_HINT_GAP * scale
                + m.width(h, FontRole::Micro, None),
        );
    }
    let panel_w = content_w + 2.0 * inset;
    let panel_h = (PL_INPUT_H
        + PL_COUNT_H
        + PL_CAT_H
        + 2.0 * PL_CMD_H
        + PL_SPACER_H
        + PL_FOOTER_H
        + 2.0 * PL_PAD)
        * scale;
    let x = ((width as f32 - panel_w) / 2.0).max(0.0);
    let y = height as f32 * 0.16;
    let panel = PxRect {
        x,
        y,
        w: panel_w,
        h: panel_h,
    };
    let cx = x + PL_PAD * scale;
    let cw = panel_w - 2.0 * PL_PAD * scale;
    let px = cx + PL_ROW_INSET * scale;
    let mut quads = Vec::new();
    let mut labels = Vec::new();
    let mut yy = y + PL_PAD * scale;

    // Input line "> zoom" + caret.
    push_pl(
        &mut labels,
        &mut m,
        ">",
        FontRole::Body,
        theme.accent,
        px,
        yy,
        PL_INPUT_H,
        scale,
    );
    let pw = m.width("> ", FontRole::Body, None);
    push_pl(
        &mut labels,
        &mut m,
        "zoom",
        FontRole::Body,
        theme.fg_primary,
        px + pw,
        yy,
        PL_INPUT_H,
        scale,
    );
    let lh = m.line_height(FontRole::Body);
    quads.push(ChromeQuad::fill(
        PxRect {
            x: px + pw + m.width("zoom", FontRole::Body, None),
            y: yy + (PL_INPUT_H * scale - lh) * 0.5,
            w: (2.0 * scale).max(1.0),
            h: lh,
        },
        theme.accent,
    ));
    yy += PL_INPUT_H * scale;

    push_pl(
        &mut labels,
        &mut m,
        "2 results",
        FontRole::Caption,
        theme.fg_muted,
        px,
        yy,
        PL_COUNT_H,
        scale,
    );
    yy += PL_COUNT_H * scale;

    // The "PANES" category header above the group.
    push_pl(
        &mut labels,
        &mut m,
        "PANES",
        FontRole::Micro,
        theme.fg_faint,
        px,
        yy,
        PL_CAT_H,
        scale,
    );
    yy += PL_CAT_H * scale;

    for (i, (icon, label, hint)) in cmds.iter().enumerate() {
        let selected = i == 0;
        if selected {
            let pi = PL_ROW_INSET * 0.5 * scale;
            quads.push(ChromeQuad::tint(
                PxRect {
                    x: cx + pi,
                    y: yy,
                    w: (cw - 2.0 * pi).max(0.0),
                    h: PL_CMD_H * scale,
                },
                theme.accent,
                0.14,
                PL_PILL_RADIUS * scale,
            ));
        }
        // Icon (accent when selected, else muted), then the label after a gap.
        push_pl(
            &mut labels,
            &mut m,
            icon,
            FontRole::Body,
            if selected {
                theme.accent
            } else {
                theme.fg_muted
            },
            px,
            yy,
            PL_CMD_H,
            scale,
        );
        let label_x = px + m.width(icon, FontRole::Body, None) + PL_ICON_GAP * scale;
        push_pl(
            &mut labels,
            &mut m,
            label,
            FontRole::Body,
            theme.fg_primary,
            label_x,
            yy,
            PL_CMD_H,
            scale,
        );
        let hw = m.width(hint, FontRole::Micro, None);
        push_pl(
            &mut labels,
            &mut m,
            hint,
            FontRole::Micro,
            theme.fg_muted,
            cx + cw - PL_ROW_INSET * scale - hw,
            yy,
            PL_CMD_H,
            scale,
        );
        yy += PL_CMD_H * scale;
    }
    yy += PL_SPACER_H * scale;
    push_pl(
        &mut labels,
        &mut m,
        PL_FOOTER,
        FontRole::Caption,
        theme.fg_muted,
        px,
        yy,
        PL_FOOTER_H,
        scale,
    );

    CaptureOverlay {
        panel,
        quads,
        labels,
    }
}

/// Build the "running job" confirm modal (design §12) as a proportional centered card -
/// mirroring the binary's `confirm` module. The live binary drives the real one.
fn confirm_overlay(width: u32, height: u32, scale: f32, theme: &Theme) -> CaptureOverlay {
    let mut m = TextMeasure::new(scale);
    let title_runs: [(&str, Srgb); 3] = [
        ("\"", theme.fg_primary),
        ("vim", theme.accent),
        ("\" is still running", theme.fg_primary),
    ];
    let action = "Close this pane and end it?";
    let hint = "\u{21b5} close    esc cancel";
    let title_w: f32 = title_runs
        .iter()
        .map(|(t, _)| m.width(t, FontRole::Title, None))
        .sum();
    let content_w = title_w
        .max(m.width(action, FontRole::Body, None))
        .max(m.width(hint, FontRole::Caption, None));
    let panel_w = content_w + 2.0 * 18.0 * scale;
    let panel_h = (18.0 + 28.0 + 6.0 + 24.0 + 14.0 + 22.0 + 18.0) * scale;
    let x = ((width as f32 - panel_w) / 2.0).max(0.0);
    let y = height as f32 * 0.16;
    let panel = PxRect {
        x,
        y,
        w: panel_w,
        h: panel_h,
    };
    let mut labels = Vec::new();
    let mut yy = y + 18.0 * scale;

    // Title: centered, drawn as colored runs.
    let lh = m.line_height(FontRole::Title);
    let ty = yy + (28.0 * scale - lh) * 0.5;
    let mut tx = x + (panel_w - title_w) * 0.5;
    for (text, color) in title_runs {
        labels.push(ProseLabel {
            text: text.to_owned(),
            x: tx,
            y: ty,
            role: FontRole::Title,
            color,
            weight: None,
            max_w: f32::MAX,
        });
        tx += m.width(text, FontRole::Title, None);
    }
    yy += 28.0 * scale + 6.0 * scale;
    push_centered_pl(
        &mut labels,
        &mut m,
        action,
        FontRole::Body,
        theme.fg_primary,
        panel,
        yy,
        24.0,
        scale,
    );
    yy += 24.0 * scale + 14.0 * scale;
    push_centered_pl(
        &mut labels,
        &mut m,
        hint,
        FontRole::Caption,
        theme.fg_muted,
        panel,
        yy,
        22.0,
        scale,
    );

    CaptureOverlay {
        panel,
        quads: Vec::new(),
        labels,
    }
}

/// Push a left-anchored proportional label centered vertically in a `row_h` row.
#[allow(clippy::too_many_arguments, reason = "one focused example helper")]
fn push_pl(
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

/// Push a label at an explicit `(x, y)` (no row centering).
fn push_pl_at(
    labels: &mut Vec<ProseLabel>,
    text: &str,
    role: FontRole,
    color: Srgb,
    x: f32,
    y: f32,
) {
    labels.push(ProseLabel {
        text: text.to_owned(),
        x,
        y,
        role,
        color,
        weight: None,
        max_w: f32::MAX,
    });
}

/// Push a horizontally-centered proportional label centered vertically in a `row_h` row.
#[allow(clippy::too_many_arguments, reason = "one focused example helper")]
fn push_centered_pl(
    labels: &mut Vec<ProseLabel>,
    m: &mut TextMeasure,
    text: &str,
    role: FontRole,
    color: Srgb,
    panel: PxRect,
    top: f32,
    row_h: f32,
    scale: f32,
) {
    let w = m.width(text, role, None);
    let line_h = m.line_height(role);
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

// Sidebar layout constants (logical px) - mirror the binary's `sidebar` module (§08) so the
// capture reproduces the real proportional tab list.
const SB_PAD_TOP: f32 = 10.0;
const SB_IND_H: f32 = 16.0;
const SB_GROUP_H: f32 = 22.0;
const SB_TAB_H: f32 = 30.0;
const SB_PAD_BOTTOM: f32 = 10.0;
const SB_LABEL_INSET: f32 = 12.0;
const SB_PILL_INSET: f32 = 9.0;
const SB_TAB_GAP_V: f32 = 3.0;
const SB_BAR_W: f32 = 3.0;
const SB_BAR_H: f32 = 14.0;
const SB_BAR_RADIUS: f32 = 2.0;
const SB_TAB_PAD_X: f32 = 10.0;
const SB_TAB_GAP: f32 = 8.0;
const SB_TAB_PROMPT: &str = "\u{276f}";
const SB_TAB_PROMPT_SLOT: f32 = 9.0;
const SB_TAB_DOT: f32 = 6.0;
const SB_PILL_RADIUS: f32 = 6.0;
/// Workspace-switcher chips (§08 #2) - mirror the binary's `sidebar` module.
const SB_CHIP_SIZE: f32 = 26.0;
const SB_CHIP_GAP: f32 = 7.0;
const SB_CHIP_RADIUS: f32 = 7.0;
const SB_CHIP_INSET: f32 = 13.0;
const SB_CHIP_BLOCK_GAP: f32 = 10.0;
/// Command-input well (§08 #3) - mirror the binary's `sidebar` module.
const SB_CMD_H: f32 = 30.0;
const SB_CMD_GAP: f32 = 12.0;
const SB_CMD_INSET: f32 = 12.0;
const SB_CMD_RADIUS: f32 = 8.0;
const SB_CMD_ICON: &str = "\u{2315}";
const SB_CMD_PLACEHOLDER: &str = "Search or run\u{2026}";
/// Utility-bar footer height + glyphs - mirror the binary's `sidebar` module (§08 #7).
const SB_UTIL_H: f32 = 40.0;
const SB_UTIL_ICONS: [&str; 4] = ["\u{2699}", "\u{25D0}", "\u{27F2}", "\u{2442}"];
/// Left cluster of the utility row (the guide's `padding:0 15px; gap:16px`).
const SB_UTIL_PAD_X: f32 = 15.0;
const SB_UTIL_STEP: f32 = 34.0;

/// Build a representative left sidebar as a proportional display list, mirroring the
/// binary's `sidebar` module layout (§08) so the capture verifies the tab-list chrome.
/// `rail` picks the slim 56px icon rail (centered tab numbers); `overflow` shows the
/// many-tab windowed state. The live binary drives this from the real module.
#[allow(clippy::too_many_arguments, reason = "one focused example builder")]
fn sidebar_panel(
    height: u32,
    sidebar_w: f32,
    strip: f32,
    scale: f32,
    rail: bool,
    overflow: bool,
    theme: &Theme,
) -> CaptureSidebar {
    let mut measure = TextMeasure::new(scale);
    // The sidebar bg fills the whole column (the macOS traffic lights sit on its top strip);
    // only the content clears the strip (mirrors the binary's `top_inset`).
    let panel = PxRect {
        x: 0.0,
        y: 0.0,
        w: sidebar_w,
        h: height as f32,
    };
    let (count, active): (usize, usize) = if overflow { (10, 8) } else { (2, 0) };
    let mut quads = vec![ChromeQuad::fill(panel, theme.bg_sidebar)];
    let mut labels = Vec::new();

    // Workspace-switcher chips (§08 #2), just below the control strip - two workspaces (P / W)
    // + a "+" tile, full panel only. Mirrors the binary's `push_chips`.
    let chip_block = if rail {
        0.0
    } else {
        SB_CHIP_SIZE + SB_CHIP_BLOCK_GAP
    };
    if !rail {
        let cy = panel.y + strip + SB_PAD_TOP * scale;
        let size = SB_CHIP_SIZE * scale;
        let step = (SB_CHIP_SIZE + SB_CHIP_GAP) * scale;
        let x0 = panel.x + SB_CHIP_INSET * scale;
        let radius = SB_CHIP_RADIUS * scale;
        let stroke = scale.max(1.0);
        let line = measure.line_height(FontRole::Mono);
        for (i, glyph) in ["P", "W", "+"].iter().enumerate() {
            let x = x0 + i as f32 * step;
            let active_ws = i == 0;
            if active_ws {
                // accent@0.4 ring over an accent.subtle fill, composited in sRGB over the
                // sidebar bg - mirrors the binary's `push_chips`.
                quads.push(ChromeQuad::rounded(
                    PxRect {
                        x,
                        y: cy,
                        w: size,
                        h: size,
                    },
                    theme.accent.over(theme.bg_sidebar, 0.4),
                    radius,
                ));
                quads.push(ChromeQuad::rounded(
                    PxRect {
                        x: x + stroke,
                        y: cy + stroke,
                        w: size - 2.0 * stroke,
                        h: size - 2.0 * stroke,
                    },
                    theme.accent_subtle_on(theme.bg_sidebar),
                    radius - stroke,
                ));
            } else {
                quads.push(ChromeQuad::rounded(
                    PxRect {
                        x,
                        y: cy,
                        w: size,
                        h: size,
                    },
                    theme.bg_surface,
                    radius,
                ));
            }
            let gw = measure.width(glyph, FontRole::Mono, None);
            push_pl_at(
                &mut labels,
                glyph,
                FontRole::Mono,
                if active_ws {
                    theme.accent
                } else {
                    theme.fg_muted
                },
                x + (size - gw) * 0.5,
                cy + (size - line) * 0.5,
            );
        }
    }

    // Command-input well (§08 #3), below the chips - a bg.surface field with ⌕ + placeholder
    // (full panel) or a centered ⌕ (rail). Mirrors the binary's `push_command_well`.
    let cmd_top = panel.y + strip + (SB_PAD_TOP + chip_block) * scale;
    if rail {
        let w = measure.width(SB_CMD_ICON, FontRole::Caption, None);
        push_pl(
            &mut labels,
            &mut measure,
            SB_CMD_ICON,
            FontRole::Caption,
            theme.fg_muted,
            panel.x + (panel.w - w) * 0.5,
            cmd_top,
            SB_CMD_H,
            scale,
        );
    } else {
        let inset = SB_CMD_INSET * scale;
        let well = PxRect {
            x: panel.x + inset,
            y: cmd_top,
            w: panel.w - 2.0 * inset,
            h: SB_CMD_H * scale,
        };
        let stroke = scale.max(1.0);
        quads.push(ChromeQuad::rounded(
            well,
            theme.border_subtle,
            SB_CMD_RADIUS * scale,
        ));
        quads.push(ChromeQuad::rounded(
            PxRect {
                x: well.x + stroke,
                y: well.y + stroke,
                w: well.w - 2.0 * stroke,
                h: well.h - 2.0 * stroke,
            },
            theme.bg_surface,
            SB_CMD_RADIUS * scale - stroke,
        ));
        let pad = 10.0 * scale;
        let icon_w = measure.width(SB_CMD_ICON, FontRole::Caption, None);
        push_pl(
            &mut labels,
            &mut measure,
            SB_CMD_ICON,
            FontRole::Caption,
            theme.fg_muted,
            well.x + pad,
            cmd_top,
            SB_CMD_H,
            scale,
        );
        push_pl(
            &mut labels,
            &mut measure,
            SB_CMD_PLACEHOLDER,
            FontRole::Caption,
            theme.fg_muted,
            well.x + pad + icon_w + 8.0 * scale,
            cmd_top,
            SB_CMD_H,
            scale,
        );
    }

    // Group header (§08 #5): the "repo · branch" context above the tab list, full panel only.
    if !rail {
        let group_top = cmd_top + (SB_CMD_H + SB_CMD_GAP) * scale;
        push_pl(
            &mut labels,
            &mut measure,
            "SKELLY \u{b7} MAIN",
            FontRole::Micro,
            theme.fg_faint,
            panel.x + SB_LABEL_INSET * scale,
            group_top,
            SB_GROUP_H,
            scale,
        );
    }

    push_sb_body(
        &mut quads,
        &mut labels,
        &mut measure,
        panel,
        (count, active),
        rail,
        strip,
        scale,
        theme,
    );

    // The bottom-anchored utility bar (§08 #7): a border.subtle top hairline + four
    // left-clustered glyphs. Full panel only (the rail has no room for it).
    if !rail {
        let util_top = panel.y + panel.h - SB_UTIL_H * scale;
        quads.push(ChromeQuad::fill(
            PxRect {
                x: panel.x,
                y: util_top,
                w: panel.w,
                h: scale.max(1.0),
            },
            theme.border_subtle,
        ));
        for (i, glyph) in SB_UTIL_ICONS.iter().enumerate() {
            push_pl(
                &mut labels,
                &mut measure,
                glyph,
                FontRole::Body,
                theme.fg_muted,
                panel.x + (SB_UTIL_PAD_X + i as f32 * SB_UTIL_STEP) * scale,
                util_top,
                SB_UTIL_H,
                scale,
            );
        }
    }

    // Right-edge divider.
    let stroke = scale.max(1.0);
    quads.push(ChromeQuad::fill(
        PxRect {
            x: panel.w - stroke,
            y: 0.0,
            w: stroke,
            h: panel.h,
        },
        theme.border,
    ));

    CaptureSidebar {
        panel,
        quads,
        labels,
    }
}

/// The windowed tab rows (overflow indicators + tabs + the new-tab action) - mirrors the
/// binary's `sidebar::build`.
#[allow(clippy::too_many_arguments, reason = "one focused example builder")]
fn push_sb_body(
    quads: &mut Vec<ChromeQuad>,
    labels: &mut Vec<ProseLabel>,
    measure: &mut TextMeasure,
    panel: PxRect,
    tabs: (usize, usize),
    rail: bool,
    strip: f32,
    scale: f32,
    theme: &Theme,
) {
    let (count, active) = tabs;
    // The content clears the control strip + the workspace-chip block + the group header
    // (logical), mirroring the binary's top_inset + chips_block + the group row.
    let (chip_block, group_h) = if rail {
        (0.0, 0.0)
    } else {
        (SB_CHIP_SIZE + SB_CHIP_BLOCK_GAP, SB_GROUP_H)
    };
    let top = strip / scale + SB_PAD_TOP + chip_block + group_h;
    let reserved_below = SB_IND_H + SB_TAB_H + SB_PAD_BOTTOM;
    let cmd_block = SB_CMD_H + SB_CMD_GAP;
    let avail = panel.h / scale - SB_UTIL_H - top - cmd_block - SB_IND_H - reserved_below;
    let capacity = (avail / (SB_TAB_H + SB_TAB_GAP_V)).floor().max(1.0) as usize;
    let visible = count.min(capacity);
    let first = if count <= visible {
        0
    } else {
        active.saturating_sub(visible - 1).min(count - visible)
    };
    let mut y = top + cmd_block;
    let place = |labels: &mut Vec<ProseLabel>, m: &mut TextMeasure, t: &str, r, c, yy, h| {
        push_sb_label(
            labels,
            m,
            t,
            r,
            c,
            panel,
            panel.y + yy * scale,
            h * scale,
            rail,
            scale,
        );
    };
    // The overflow-up indicator only takes a row when something is hidden above (mirrors the
    // binary), so a short tab list sits flush under the group header.
    if first > 0 {
        place(
            labels,
            measure,
            &format!("↑ {first} more"),
            FontRole::Caption,
            theme.fg_muted,
            y,
            SB_IND_H,
        );
        y += SB_IND_H;
    }
    for index in first..first + visible {
        let is_active = index == active;
        if is_active {
            push_sb_active(
                quads,
                panel,
                panel.y + y * scale,
                SB_TAB_H * scale,
                scale,
                theme,
            );
        }
        let color = if is_active {
            theme.fg_primary
        } else {
            theme.fg_secondary
        };
        if rail {
            place(
                labels,
                measure,
                &(index + 1).to_string(),
                FontRole::Label,
                color,
                y,
                SB_TAB_H,
            );
        } else {
            // The ❯ prompt (accent), or a ● running dot for the second tab (representative);
            // then the label inset past the prefix slot + gap (§09/§10.3).
            let row_top = panel.y + y * scale;
            let prefix_x = panel.x + (SB_PILL_INSET + SB_TAB_PAD_X + SB_BAR_W + SB_TAB_GAP) * scale;
            if index == 1 {
                let dot = SB_TAB_DOT * scale;
                quads.push(ChromeQuad::rounded(
                    PxRect {
                        x: prefix_x + (SB_TAB_PROMPT_SLOT * scale - dot) * 0.5,
                        y: row_top + (SB_TAB_H * scale - dot) * 0.5,
                        w: dot,
                        h: dot,
                    },
                    theme.diff_add,
                    dot * 0.5,
                ));
            } else {
                let pline = measure.line_height(FontRole::Mono);
                labels.push(ProseLabel {
                    text: SB_TAB_PROMPT.to_owned(),
                    x: prefix_x,
                    y: row_top + (SB_TAB_H * scale - pline) * 0.5,
                    role: FontRole::Mono,
                    color: theme.accent,
                    weight: None,
                    max_w: f32::MAX,
                });
            }
            let x = prefix_x + (SB_TAB_PROMPT_SLOT + SB_TAB_GAP) * scale;
            let line = measure.line_height(FontRole::Label);
            // Representative §10.3 titles: the active tab shows a command, the running tab a
            // long-lived process (in the binary these come from each tab's foreground job).
            let title = match index {
                0 => "git status".to_owned(),
                1 => "dev server".to_owned(),
                n => format!("Tab {}", n + 1),
            };
            labels.push(ProseLabel {
                text: title,
                x,
                y: row_top + (SB_TAB_H * scale - line) * 0.5,
                role: FontRole::Label,
                color,
                weight: None,
                max_w: f32::MAX,
            });
        }
        y += SB_TAB_H + SB_TAB_GAP_V;
    }
    let more_below = count - first - visible;
    if more_below > 0 {
        place(
            labels,
            measure,
            &format!("↓ {more_below} more"),
            FontRole::Caption,
            theme.fg_muted,
            y,
            SB_IND_H,
        );
        y += SB_IND_H;
    }
    let newtab = if rail { "+" } else { "+ New tab" };
    place(
        labels,
        measure,
        newtab,
        FontRole::Label,
        theme.fg_muted,
        y,
        SB_TAB_H,
    );
}

/// The active tab's bordered `accent`@0.14 pill + a 3x14 rounded `accent` indicator bar inside
/// it (§09 "Sidebar tab item") - mirrors the binary's `push_active_marks`.
fn push_sb_active(
    quads: &mut Vec<ChromeQuad>,
    panel: PxRect,
    top: f32,
    height: f32,
    scale: f32,
    theme: &Theme,
) {
    let inset = SB_PILL_INSET * scale;
    let radius = SB_PILL_RADIUS * scale;
    let stroke = scale.max(1.0);
    let pill = PxRect {
        x: panel.x + inset,
        y: top,
        w: (panel.w - 2.0 * inset).max(0.0),
        h: height,
    };
    // accent@0.28 border ring over an accent.subtle fill, both composited in sRGB over the
    // sidebar bg - mirrors the binary's `push_active_marks`.
    quads.push(ChromeQuad::rounded(
        pill,
        theme.accent.over(theme.bg_sidebar, 0.28),
        radius,
    ));
    let inner = PxRect {
        x: pill.x + stroke,
        y: pill.y + stroke,
        w: (pill.w - 2.0 * stroke).max(0.0),
        h: (pill.h - 2.0 * stroke).max(0.0),
    };
    let inner_r = (radius - stroke).max(0.0);
    quads.push(ChromeQuad::rounded(
        inner,
        theme.accent_subtle_on(theme.bg_sidebar),
        inner_r,
    ));
    let bar_h = SB_BAR_H * scale;
    quads.push(ChromeQuad::rounded(
        PxRect {
            x: pill.x + SB_TAB_PAD_X * scale,
            y: top + (height - bar_h) * 0.5,
            w: SB_BAR_W * scale,
            h: bar_h,
        },
        theme.accent,
        SB_BAR_RADIUS * scale,
    ));
}

/// Place one sidebar label vertically centered (left-inset for the panel, centered for the
/// rail) - mirrors the binary's `push_label`.
#[allow(clippy::too_many_arguments, reason = "one focused placement helper")]
fn push_sb_label(
    labels: &mut Vec<ProseLabel>,
    measure: &mut TextMeasure,
    text: &str,
    role: FontRole,
    color: Srgb,
    panel: PxRect,
    top: f32,
    height: f32,
    rail: bool,
    scale: f32,
) {
    let line_h = measure.line_height(role);
    let y = top + (height - line_h) * 0.5;
    let x = if rail {
        let w = measure.width(text, role, None);
        panel.x + (panel.w - w) * 0.5
    } else {
        panel.x + SB_LABEL_INSET * scale
    };
    let max_w = (panel.x + panel.w - x - SB_LABEL_INSET * scale * 0.5).max(1.0);
    labels.push(ProseLabel {
        text: text.to_owned(),
        x,
        y,
        role,
        color,
        weight: None,
        max_w,
    });
}

fn wait_until<F: Fn(&Terminal) -> bool>(term: &Terminal, timeout: Duration, ready: F) {
    let deadline = Instant::now() + timeout;
    while Instant::now() < deadline && !ready(term) {
        sleep(Duration::from_millis(50));
    }
}

// Mirrors `resolve_cell` in the binary (examples cannot import the binary crate):
// fold dim + reverse video into concrete colors, pass bold/italic/underline through.
fn resolve_cell(cell: &TermCell, palette: &AnsiPalette) -> GridCell {
    let mut fg = resolve_fg(cell.fg, palette);
    let mut bg = resolve_bg(cell.bg, palette);
    if cell.attrs.contains(CellAttrs::DIM) {
        fg = dim(fg);
    }
    if cell.attrs.contains(CellAttrs::INVERSE) {
        let fill = fg;
        fg = bg.unwrap_or_else(|| palette.default_bg());
        bg = Some(fill);
    }
    GridCell {
        c: cell.c,
        fg,
        bg,
        bold: cell.attrs.contains(CellAttrs::BOLD),
        italic: cell.attrs.contains(CellAttrs::ITALIC),
        underline: cell.attrs.contains(CellAttrs::UNDERLINE),
    }
}

fn resolve_bg(color: CellColor, palette: &AnsiPalette) -> Option<Srgb> {
    match color {
        CellColor::Default => None,
        CellColor::Indexed(index) => Some(palette.indexed(index)),
        CellColor::Rgb(r, g, b) => Some(Srgb { r, g, b }),
    }
}

fn resolve_fg(color: CellColor, palette: &AnsiPalette) -> Srgb {
    match color {
        CellColor::Default => palette.default_fg(),
        CellColor::Indexed(index) => palette.indexed(index),
        CellColor::Rgb(r, g, b) => Srgb { r, g, b },
    }
}

fn dim(c: Srgb) -> Srgb {
    let faint = |v: u8| u8::try_from(u16::from(v) * 3 / 5).unwrap_or(v);
    Srgb {
        r: faint(c.r),
        g: faint(c.g),
        b: faint(c.b),
    }
}
