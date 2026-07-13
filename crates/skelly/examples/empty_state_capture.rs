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
use skelly_render::{
    measure_cell, AnsiPalette, CapturePane, ChromeQuad, FontRole, GridCell, PaneOverlay,
    ProseLabel, PxRect, Srgb, TextMeasure, Theme,
};
use skelly_term::{CellAttrs, CellColor, TermCell, Terminal};

/// Logical padding around the whole pane area - mirrors the binary's `WINDOW_PAD`.
const WINDOW_PAD: f32 = 12.0;
/// Logical inset inside each pane - mirrors the binary's `PANE_INSET`.
const PANE_INSET: f32 = 6.0;

// Mirrors the binary's `emptystate` module (examples cannot import the binary crate).
const CHIPS: [(&str, &str); 4] = [
    ("\u{2318}K", "commands"),
    ("\u{2325}|", "split right"),
    ("\u{21E7}\u{2318}G", "git diff"),
    ("\u{2318},", "settings"),
];
const MARK_SIZE: f32 = 56.0;
const MARK_GAP: f32 = 18.0;
const CHIP_H: f32 = 28.0;
const CHIP_PAD: f32 = 13.0;
const CHIP_GAP: f32 = 10.0;
const CHIP_KEY_GAP: f32 = 6.0;
const CHIP_ROW_GAP: f32 = 10.0;
const CHIP_MARGIN: f32 = 40.0;

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
    let grid: Vec<Vec<GridCell>> = term
        .cells()
        .iter()
        .map(|row| row.iter().map(|c| resolve_cell(c, &palette)).collect())
        .collect();
    let px_rect = PxRect {
        x: rect.x,
        y: rect.y,
        w: rect.w,
        h: rect.h,
    };
    // The vertebra mark (a vector overlay via CapturePane::logo) + the proportional hint
    // chips (through the pane-overlay), mirroring the binary's `emptystate` module.
    let mut measure = TextMeasure::new(sc);
    let logo = logo_bounds(px_rect, sc);
    let mut overlay = PaneOverlay::default();
    if let Some(logo) = logo {
        let (q, l) = chips_paint(logo, px_rect, sc, &theme, &mut measure);
        overlay.quads = q;
        overlay.labels = l;
    }

    let panes = vec![CapturePane {
        rect: px_rect,
        origin: (rect.x + inset, rect.y + inset),
        rows: grid,
        cursor: term.cursor(),
        focused: true,
        logo,
    }];

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
    println!("wrote {path}");
}

/// The vertebra mark's square bounding box, centered in `rect` (mirrors the binary's
/// `emptystate::logo_bounds`).
fn logo_bounds(rect: PxRect, scale: f32) -> Option<PxRect> {
    let mark = MARK_SIZE * scale;
    if rect.w < mark * 2.0 || rect.h < mark + (MARK_GAP + CHIP_H) * scale + 60.0 * scale {
        return None;
    }
    let cx = rect.x + rect.w * 0.5;
    let cy = rect.y + rect.h * 0.42;
    Some(PxRect {
        x: cx - mark * 0.5,
        y: cy - mark * 0.5,
        w: mark,
        h: mark,
    })
}

/// The proportional hint chips seated below the mark (mirrors `emptystate::chips_paint`):
/// rounded `bg.elevated` pills with the mono key chord + caption label.
fn chips_paint(
    logo: PxRect,
    rect: PxRect,
    scale: f32,
    theme: &Theme,
    m: &mut TextMeasure,
) -> (Vec<ChromeQuad>, Vec<ProseLabel>) {
    let (pad, gap, h, key_gap) = (
        CHIP_PAD * scale,
        CHIP_GAP * scale,
        CHIP_H * scale,
        CHIP_KEY_GAP * scale,
    );
    let sizes: Vec<(f32, f32, f32)> = CHIPS
        .iter()
        .map(|(k, l)| {
            let kw = m.width(k, FontRole::Micro, None);
            let lw = m.width(l, FontRole::Caption, None);
            (kw, lw, pad + kw + key_gap + lw + pad)
        })
        .collect();
    // Greedily wrap chips into rows no wider than the pane span (mirrors the binary).
    let max_row = (rect.w - 2.0 * CHIP_MARGIN * scale).max(sizes[0].2);
    let mut rows: Vec<Vec<usize>> = vec![Vec::new()];
    let mut row_w = 0.0;
    for (i, &(_, _, pw)) in sizes.iter().enumerate() {
        let extra = if rows.last().unwrap().is_empty() {
            pw
        } else {
            pw + gap
        };
        if !rows.last().unwrap().is_empty() && row_w + extra > max_row {
            rows.push(vec![i]);
            row_w = pw;
        } else {
            rows.last_mut().unwrap().push(i);
            row_w += extra;
        }
    }
    let mut quads = Vec::new();
    let mut labels = Vec::new();
    let mut y = logo.y + logo.h + MARK_GAP * scale;
    for row in &rows {
        let row_total: f32 = row.iter().map(|&i| sizes[i].2).sum::<f32>()
            + gap * (row.len().saturating_sub(1)) as f32;
        let mut x = rect.x + (rect.w - row_total) * 0.5;
        for &i in row {
            let (key, label) = CHIPS[i];
            let (kw, _, pw) = sizes[i];
            quads.push(ChromeQuad::rounded(
                PxRect { x, y, w: pw, h },
                theme.bg_elevated,
                h * 0.5,
            ));
            push_chip(
                &mut labels,
                m,
                key,
                FontRole::Micro,
                theme.fg_secondary,
                x + pad,
                y,
                h,
            );
            push_chip(
                &mut labels,
                m,
                label,
                FontRole::Caption,
                theme.fg_muted,
                x + pad + kw + key_gap,
                y,
                h,
            );
            x += pw + gap;
        }
        y += h + CHIP_ROW_GAP * scale;
    }
    (quads, labels)
}

#[allow(clippy::too_many_arguments, reason = "one focused example helper")]
fn push_chip(
    labels: &mut Vec<ProseLabel>,
    m: &mut TextMeasure,
    text: &str,
    role: FontRole,
    color: Srgb,
    x: f32,
    y: f32,
    h: f32,
) {
    let line = m.line_height(role);
    labels.push(ProseLabel {
        text: text.to_owned(),
        x,
        y: y + (h - line) * 0.5,
        role,
        color,
        weight: None,
        max_w: f32::MAX,
    });
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
