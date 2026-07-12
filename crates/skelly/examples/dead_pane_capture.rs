//! Headless proof of the "shell exits / crashes" edge state (M5): a two-pane split
//! where the right pane's shell has exited, so it draws the dim scrim over its preserved
//! scrollback plus the centered exit message ("shell exited" / exit code / restart hint).
//! Renders to a PNG with no window needed.
//! Run: `cargo run -p skelly --example dead_pane_capture -- dead.png [ossein-dark|ossein-light]`.
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
use skelly_render::{
    measure_cell, AnsiPalette, CaptureDeadPane, CapturePane, GridCell, PxRect, Srgb, Theme,
};
use skelly_term::{CellAttrs, CellColor, TermCell, Terminal};

/// Logical padding around the whole pane area - mirrors the binary's `WINDOW_PAD`.
const WINDOW_PAD: f32 = 12.0;
/// Logical inset inside each pane - mirrors the binary's `PANE_INSET`.
const PANE_INSET: f32 = 6.0;

fn main() {
    let path = std::env::args()
        .nth(1)
        .unwrap_or_else(|| "skelly-dead-pane.png".to_owned());
    let (width, height, scale) = (1360_u32, 680_u32, 2.0_f64);

    let theme_name = std::env::args()
        .nth(2)
        .unwrap_or_else(|| "ossein-dark".to_owned());
    let appearance = Appearance {
        font_family: "SauceCodePro Nerd Font Mono".to_owned(),
        theme: theme_name,
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

    // Two panes side by side; the right one is the "exited" pane.
    let mut tree = PaneTree::new();
    let left = tree.focused();
    tree.split(Dir::Right).expect("under the pane cap");
    let right = tree.focused();
    let layout = tree.layout(viewport);

    let palette = AnsiPalette::resolve(&appearance.theme);
    let theme = Theme::resolve(&appearance.theme);
    let mut panes = Vec::new();
    let mut dead_panes = Vec::new();
    for (id, rect) in &layout {
        let cols = ((rect.w - 2.0 * inset) / cell_w).floor().max(2.0) as u16;
        let rows = ((rect.h - 2.0 * inset) / cell_h).floor().max(1.0) as u16;

        let mut term = Terminal::spawn(cols, rows, || {}).expect("spawn shell");
        wait_until(&term, Duration::from_secs(6), |t| {
            t.snapshot().iter().any(|line| !line.is_empty())
        });
        sleep(Duration::from_millis(300));

        let cmd = if *id == left {
            "clear; \
             printf '\\033[35m\\033[0m left pane \\342\\200\\224 editor\\n\\n'; \
             printf '\\033[36m  1\\033[0m fn main() {\\n'; \
             printf '\\033[36m  2\\033[0m     println!(\"skelly\");\\n'; \
             printf '\\033[36m  3\\033[0m }\\n'; \
             printf 'PANE_READY\\n'\n"
        } else {
            // The right pane ran a build that ended, so its scrollback shows beneath the scrim.
            "clear; \
             printf '$ cargo build\\n'; \
             printf '\\033[32m   Compiling\\033[0m skelly v0.1.0\\n'; \
             printf '\\033[32m    Finished\\033[0m in 3.7s\\n'; \
             printf '$ exit\\n'; \
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
        let px = PxRect {
            x: rect.x,
            y: rect.y,
            w: rect.w,
            h: rect.h,
        };
        panes.push(CapturePane {
            rect: px,
            origin: (rect.x + inset, rect.y + inset),
            rows: grid,
            cursor: term.cursor(),
            focused: false, // an exited pane draws no cursor
        });
        if *id == right {
            dead_panes.push(exit_overlay(px, cell_w, cell_h, &theme));
        }
    }

    let rgba = skelly_render::capture_panes_rgba(
        &appearance,
        width,
        height,
        scale,
        &panes,
        &skelly_render::Chrome {
            dead_panes: &dead_panes,
            ..Default::default()
        },
    );

    write_png(&path, width, height, &rgba);
    println!(
        "wrote {path} ({} panes, {} exited)",
        panes.len(),
        dead_panes.len()
    );
}

/// Build the exited-pane overlay - the same layout the binary's `deadpane` module
/// produces - centered in `rect`: "shell exited" / "exit code 0" (green) / restart hint.
fn exit_overlay(rect: PxRect, cell_w: f32, cell_h: f32, theme: &Theme) -> CaptureDeadPane {
    let lines: [Vec<(String, Srgb)>; 4] = [
        vec![("shell exited".to_owned(), theme.fg_primary)],
        vec![("exit code 0".to_owned(), theme.diff_add)],
        Vec::new(),
        vec![
            ("\u{21b5} restart".to_owned(), theme.accent),
            ("   ".to_owned(), theme.fg_muted),
            ("\u{2325}w close".to_owned(), theme.accent),
        ],
    ];
    let grid_cols = lines
        .iter()
        .map(|segs| segs.iter().map(|(t, _)| t.chars().count()).sum())
        .max()
        .unwrap_or(0);
    let rows: Vec<Vec<GridCell>> = lines
        .iter()
        .map(|segs| centered_row(segs, grid_cols, theme.fg_muted))
        .collect();
    let grid_w = grid_cols as f32 * cell_w;
    let grid_h = rows.len() as f32 * cell_h;
    CaptureDeadPane {
        rect,
        text_origin: (
            rect.x + ((rect.w - grid_w) / 2.0).max(0.0),
            rect.y + ((rect.h - grid_h) / 2.0).max(0.0),
        ),
        rows,
    }
}

/// A `width`-cell row with `segments` (text + color) laid out centered.
fn centered_row(segments: &[(String, Srgb)], width: usize, blank_fg: Srgb) -> Vec<GridCell> {
    let content: usize = segments.iter().map(|(t, _)| t.chars().count()).sum();
    let mut row = vec![ui_cell(' ', blank_fg); width];
    let mut col = width.saturating_sub(content) / 2;
    for (text, fg) in segments {
        for ch in text.chars() {
            if let Some(slot) = row.get_mut(col) {
                *slot = ui_cell(ch, *fg);
            }
            col += 1;
        }
    }
    row
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

fn wait_until<F: Fn(&Terminal) -> bool>(term: &Terminal, timeout: Duration, ready: F) {
    let deadline = Instant::now() + timeout;
    while Instant::now() < deadline && !ready(term) {
        sleep(Duration::from_millis(50));
    }
}

// Mirrors `resolve_cell` in the binary (examples cannot import the binary crate).
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
