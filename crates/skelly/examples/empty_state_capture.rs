//! Headless proof of the empty-state screen (design §10.2 / the "close last tab" edge
//! state): a fresh single-pane tab paints a faint brand mark and hint chips over its blank
//! terminal until the first command runs. Renders to a PNG with no window needed.
//! Run: `cargo run -p skelly --example empty_state_capture -- empty.png [ossein-dark|ossein-light]`.
#![allow(
    clippy::cast_precision_loss,
    clippy::cast_possible_truncation,
    clippy::cast_sign_loss,
    reason = "example: surface dimensions and grid sizes are small, non-negative values"
)]

use std::thread::sleep;
use std::time::{Duration, Instant};

use skelly_config::Appearance;
use skelly_pane::{PaneTree, Rect};
use skelly_render::{measure_cell, AnsiPalette, CapturePane, GridCell, PxRect, Srgb, Theme};
use skelly_term::{CellAttrs, CellColor, TermCell, Terminal};

/// Logical padding around the whole pane area - mirrors the binary's `WINDOW_PAD`.
const WINDOW_PAD: f32 = 12.0;
/// Logical inset inside each pane - mirrors the binary's `PANE_INSET`.
const PANE_INSET: f32 = 6.0;

// Mirrors the binary's `emptystate` module (examples cannot import the binary crate).
const MARK: &str = "skelly";
const CHIPS: [(&str, &str); 3] = [
    ("\u{2318}K", "palette"),
    ("\u{2318}T", "new tab"),
    ("\u{2325}|", "split"),
];
const CHIP_PAD: usize = 1;
const CHIP_GAP: usize = 2;

fn main() {
    let path = std::env::args()
        .nth(1)
        .unwrap_or_else(|| "skelly-empty-state.png".to_owned());
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

    // A single fresh pane, full viewport.
    let tree = PaneTree::new();
    let (_id, rect) = tree.layout(viewport).into_iter().next().expect("one pane");
    let cols = ((rect.w - 2.0 * inset) / cell_w).floor().max(2.0) as u16;
    let rows_n = ((rect.h - 2.0 * inset) / cell_h).floor().max(1.0) as u16;

    let term = Terminal::spawn(cols, rows_n, || {}).expect("spawn shell");
    // Let the shell print its prompt so the tab is a realistic "fresh" grid.
    wait_until(&term, Duration::from_secs(6), |t| {
        t.snapshot().iter().any(|line| !line.is_empty())
    });
    sleep(Duration::from_millis(300));

    let palette = AnsiPalette::resolve(&appearance.theme);
    let theme = Theme::resolve(&appearance.theme);
    let mut grid: Vec<Vec<GridCell>> = term
        .cells()
        .iter()
        .map(|row| row.iter().map(|c| resolve_cell(c, &palette)).collect())
        .collect();
    overlay_empty_state(&mut grid, &theme);

    let panes = vec![CapturePane {
        rect: PxRect {
            x: rect.x,
            y: rect.y,
            w: rect.w,
            h: rect.h,
        },
        origin: (rect.x + inset, rect.y + inset),
        rows: grid,
        cursor: term.cursor(),
        focused: true,
    }];

    let rgba = skelly_render::capture_panes_rgba(
        &appearance,
        width,
        height,
        scale,
        &panes,
        &skelly_render::Chrome::default(),
    );
    write_png(&path, width, height, &rgba);
    println!("wrote {path}");
}

/// Bake the empty-state mark + chips into the center of `grid` - a faithful copy of the
/// binary's `emptystate::overlay_onto`.
fn overlay_empty_state(rows: &mut [Vec<GridCell>], theme: &Theme) {
    let height = rows.len();
    let width = rows.first().map_or(0, Vec::len);
    let mark_row = height * 9 / 20;
    let chip_row = mark_row + 2;
    if width == 0 || chip_row >= height {
        return;
    }
    let start = width.saturating_sub(MARK.chars().count()) / 2;
    for (i, ch) in MARK.chars().enumerate() {
        if let Some(slot) = rows[mark_row].get_mut(start + i) {
            *slot = ui_cell(ch, theme.fg_muted, None);
        }
    }
    write_chips(&mut rows[chip_row], width, theme);
}

fn write_chips(row: &mut [GridCell], width: usize, theme: &Theme) {
    let chip_width = |key: &str, label: &str| {
        CHIP_PAD + key.chars().count() + 1 + label.chars().count() + CHIP_PAD
    };
    let total: usize =
        CHIPS.iter().map(|(k, l)| chip_width(k, l)).sum::<usize>() + CHIP_GAP * (CHIPS.len() - 1);
    let mut col = width.saturating_sub(total) / 2;
    for (key, label) in CHIPS {
        let pill = chip_width(key, label);
        for c in col..col + pill {
            if let Some(slot) = row.get_mut(c) {
                *slot = ui_cell(' ', theme.fg_muted, Some(theme.bg_elevated));
            }
        }
        let mut inner = col + CHIP_PAD;
        for ch in key.chars() {
            if let Some(slot) = row.get_mut(inner) {
                *slot = ui_cell(ch, theme.fg_secondary, Some(theme.bg_elevated));
            }
            inner += 1;
        }
        inner += 1; // the space between key and label
        for ch in label.chars() {
            if let Some(slot) = row.get_mut(inner) {
                *slot = ui_cell(ch, theme.fg_muted, Some(theme.bg_elevated));
            }
            inner += 1;
        }
        col += pill + CHIP_GAP;
    }
}

fn ui_cell(c: char, fg: Srgb, bg: Option<Srgb>) -> GridCell {
    GridCell {
        c,
        fg,
        bg,
        bold: false,
        italic: false,
        underline: false,
    }
}

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
