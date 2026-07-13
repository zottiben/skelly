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
    measure_cell, AnsiPalette, CapturePane, FontRole, GridCell, PaneOverlay, ProseLabel, PxRect,
    Srgb, TextMeasure, Theme,
};
use skelly_term::{CellAttrs, CellColor, TermCell, Terminal};

/// Logical padding around the whole pane area - mirrors the binary's `WINDOW_PAD`.
const WINDOW_PAD: f32 = 12.0;
/// Logical inset inside each pane - mirrors the binary's `PANE_INSET`.
const PANE_INSET: f32 = 6.0;

#[allow(
    clippy::too_many_lines,
    reason = "example: one straight-line capture scene"
)]
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
    let mut measure = TextMeasure::new(sc);
    let mut panes = Vec::new();
    let mut overlay = PaneOverlay::default();
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
            cursor_shape: skelly_render::CursorShape::Block,
            focused: false, // an exited pane draws no cursor
            logo: None,
        });
        if *id == right {
            overlay.scrims.push(px);
            overlay
                .labels
                .extend(exit_message(px, sc, &theme, &mut measure));
        }
    }

    let dead = overlay.scrims.len();
    let rgba = skelly_render::capture_panes_rgba(
        &appearance,
        width,
        height,
        scale,
        &panes,
        &skelly_render::Chrome {
            pane_overlay: overlay,
            ..Default::default()
        },
    );

    write_png(&path, width, height, &rgba);
    println!("wrote {path} ({} panes, {dead} exited)", panes.len());
}

/// The exited-pane message centered in `rect` - the same layout the binary's `deadpane`
/// module produces: "shell exited" (title) / "exit code 0" (green body) / restart hint.
fn exit_message(rect: PxRect, scale: f32, theme: &Theme, m: &mut TextMeasure) -> Vec<ProseLabel> {
    let lines: [(&str, FontRole, Srgb); 3] = [
        ("shell exited", FontRole::Title, theme.fg_primary),
        ("exit code 0", FontRole::Body, theme.diff_add),
        (
            "\u{21b5} restart    \u{2325}w close",
            FontRole::Caption,
            theme.accent,
        ),
    ];
    let gap = 8.0 * scale;
    let total: f32 = lines.iter().map(|(_, r, _)| m.line_height(*r)).sum::<f32>() + gap * 2.0;
    let mut y = rect.y + (rect.h - total) * 0.5;
    let mut labels = Vec::new();
    for (text, role, color) in lines {
        let w = m.width(text, role, None);
        labels.push(ProseLabel {
            text: text.to_owned(),
            x: rect.x + (rect.w - w) * 0.5,
            y,
            role,
            color,
            weight: None,
            max_w: f32::MAX,
        });
        y += m.line_height(role) + gap;
    }
    labels
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
