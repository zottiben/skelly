//! Headless proof of the M3 pane workspace: build a real two-pane split with the
//! `skelly-pane` tree, spawn a live shell in each pane, and render the tiled result
//! (dividers, focused-pane ring, and a cursor only in the focused pane) to a PNG,
//! with no window or screen-recording needed.
//! Run: `cargo run -p skelly --example pane_capture -- panes.png`.
#![allow(
    clippy::cast_precision_loss,
    clippy::cast_possible_truncation,
    clippy::cast_sign_loss,
    reason = "example: surface dimensions and grid sizes are small, non-negative values"
)]

use std::thread::sleep;
use std::time::{Duration, Instant};

use skelly_config::Appearance;
use skelly_pane::{Dir, PaneTree, Rect};
use skelly_render::{measure_cell, AnsiPalette, CapturePane, GridCell, PxRect, Srgb};
use skelly_term::{CellAttrs, CellColor, TermCell, Terminal};

/// Logical padding around the whole pane area - mirrors the binary's `WINDOW_PAD`.
const WINDOW_PAD: f32 = 12.0;
/// Logical inset inside each pane - mirrors the binary's `PANE_INSET`.
const PANE_INSET: f32 = 6.0;

fn main() {
    let path = std::env::args()
        .nth(1)
        .unwrap_or_else(|| "skelly-panes.png".to_owned());
    let (width, height, scale) = (1040_u32, 640_u32, 2.0_f64);

    // Use an installed Nerd Font so the configured-font path is exercised.
    let appearance = Appearance {
        font_family: "SauceCodePro Nerd Font Mono".to_owned(),
        ..Appearance::default()
    };
    let (cell_w, cell_h) = measure_cell(&appearance, scale);
    let sc = scale as f32;
    let pad = WINDOW_PAD * sc;
    let inset = PANE_INSET * sc;
    let viewport = Rect::new(
        pad,
        pad,
        width as f32 - 2.0 * pad,
        height as f32 - 2.0 * pad,
    );

    // Two panes, side by side; focus lands on the new (right) pane, matching the
    // binary's split behavior.
    let mut tree = PaneTree::new();
    let left = tree.focused();
    tree.split(Dir::Right).expect("under the pane cap");
    let focused = tree.focused();
    let layout = tree.layout(viewport);

    let palette = AnsiPalette::resolve(&appearance.theme);
    let mut panes = Vec::new();
    for (id, rect) in &layout {
        let cols = ((rect.w - 2.0 * inset) / cell_w).floor().max(2.0) as u16;
        let rows = ((rect.h - 2.0 * inset) / cell_h).floor().max(1.0) as u16;

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
        });
    }

    let rgba = skelly_render::capture_panes_rgba(&appearance, width, height, scale, &panes);

    let file = std::fs::File::create(&path).expect("create png");
    let mut encoder = png::Encoder::new(std::io::BufWriter::new(file), width, height);
    encoder.set_color(png::ColorType::Rgba);
    encoder.set_depth(png::BitDepth::Eight);
    encoder
        .write_header()
        .expect("png header")
        .write_image_data(&rgba)
        .expect("png data");
    println!("wrote {path} ({} panes)", panes.len());
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
