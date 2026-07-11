//! Headless proof of the M3 settings view: render the full-window settings surface
//! (left category nav, right control list, the active-category and focused-control
//! highlights, and the nav/content divider) to a PNG, with no window or screen
//! recording needed.
//!
//! The live binary drives this from its real `settings` module; examples cannot import
//! the binary crate, so this hand-builds a representative grid (as `pane_capture` does
//! for the palette/sidebar) purely to exercise the `settings_quads` render path. An
//! optional second arg picks the theme (`ossein-light`).
//! Run: `cargo run -p skelly --example settings_capture -- settings.png [theme]`.
#![allow(
    clippy::cast_precision_loss,
    clippy::cast_possible_truncation,
    clippy::cast_sign_loss,
    reason = "example: surface dimensions and grid sizes are small, non-negative values"
)]

use skelly_config::Appearance;
use skelly_render::{measure_cell, CaptureSettings, GridCell, PxRect, Srgb, Theme};

/// Nav column width in cells - mirrors the binary's `settings::NAV_COLS`.
const NAV_COLS: usize = 20;
/// Column where content begins - mirrors the binary's `settings::CONTENT_INDENT`.
const CONTENT_INDENT: usize = NAV_COLS + 2;
/// Logical inset of the settings text - mirrors the binary's `SETTINGS_PAD`.
const SETTINGS_PAD: f32 = 20.0;

fn main() {
    let path = std::env::args()
        .nth(1)
        .unwrap_or_else(|| "skelly-settings.png".to_owned());
    let (width, height, scale) = (1200_u32, 720_u32, 2.0_f64);

    let theme_name = std::env::args()
        .nth(2)
        .unwrap_or_else(|| "ossein-dark".to_owned());
    let appearance = Appearance {
        font_family: "SauceCodePro Nerd Font Mono".to_owned(),
        theme: theme_name,
        ..Appearance::default()
    };
    let theme = Theme::resolve(&appearance.theme);
    let (cell_w, _) = measure_cell(&appearance, scale);
    let pad = SETTINGS_PAD * scale as f32;
    let cols = ((width as f32 - 2.0 * pad) / cell_w).floor().max(1.0) as usize;

    let settings = build_settings(cols, &theme, width, height, pad);

    let rgba = skelly_render::capture_settings_rgba(&appearance, width, height, scale, &settings);
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

/// A representative Appearance settings panel, mirroring the binary's `settings` module
/// layout so the capture verifies the render path.
fn build_settings(
    cols: usize,
    theme: &Theme,
    width: u32,
    height: u32,
    pad: f32,
) -> CaptureSettings {
    let categories = [
        ('#', "Appearance"),
        ('=', "Sidebar"),
        ('+', "Tabs"),
        ('|', "Panes"),
        ('@', "Session"),
        ('%', "Git"),
    ];
    let controls = [
        ("Theme", "Ossein Dark"),
        ("Font size", "14px"),
        ("Line height", "1.2"),
        ("Cursor style", "Block"),
        ("Font ligatures", "On"),
        ("Bold uses bright colors", "On"),
        ("Background blur", "18"),
        ("Window opacity", "0.98"),
    ];
    let active_category = 0usize;
    let selected_control = 3usize; // "Cursor style"

    let nav_start = 2usize;
    let content_start = 4usize;
    let footer_row = (nav_start + categories.len()).max(content_start + controls.len()) + 1;
    let total = footer_row + 1;

    let mut rows: Vec<Vec<GridCell>> = (0..total)
        .map(|_| blank_row(cols, theme.fg_muted))
        .collect();

    write(&mut rows[0], 2, "skelly", theme.fg_secondary);
    write(&mut rows[0], CONTENT_INDENT, "Appearance", theme.fg_primary);
    write_right(&mut rows[0], cols, "esc to close", theme.fg_muted);

    for (i, (icon, label)) in categories.iter().enumerate() {
        let fg = if i == active_category {
            theme.fg_primary
        } else {
            theme.fg_secondary
        };
        write(&mut rows[nav_start + i], 2, &format!("{icon} {label}"), fg);
    }

    for (i, (label, value)) in controls.iter().enumerate() {
        let row = content_start + i;
        let selected = i == selected_control;
        let label_fg = if selected {
            theme.fg_primary
        } else {
            theme.fg_secondary
        };
        write(&mut rows[row], CONTENT_INDENT, label, label_fg);
        if selected {
            let width_cells = value.chars().count() + 4;
            let start = cols.saturating_sub(2 + width_cells);
            write(&mut rows[row], start, "\u{2039} ", theme.fg_muted);
            write(&mut rows[row], start + 2, value, theme.accent);
            write(
                &mut rows[row],
                start + 2 + value.chars().count(),
                " \u{203a}",
                theme.fg_muted,
            );
        } else {
            let start = cols.saturating_sub(2 + value.chars().count());
            write(&mut rows[row], start, value, theme.fg_secondary);
        }
    }

    write(
        &mut rows[footer_row],
        2,
        "up/down move   left/right change   tab category   esc close",
        theme.fg_muted,
    );

    CaptureSettings {
        panel: PxRect {
            x: 0.0,
            y: 0.0,
            w: width as f32,
            h: height as f32,
        },
        text_origin: (pad, pad),
        rows,
        nav_cols: NAV_COLS,
        nav_active_row: Some(nav_start + active_category),
        selected_row: Some(content_start + selected_control),
    }
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

fn blank_row(cols: usize, fg: Srgb) -> Vec<GridCell> {
    vec![ui_cell(' ', fg); cols]
}

fn write(row: &mut [GridCell], col: usize, text: &str, fg: Srgb) {
    for (i, ch) in text.chars().enumerate() {
        if let Some(slot) = row.get_mut(col + i) {
            *slot = ui_cell(ch, fg);
        }
    }
}

fn write_right(row: &mut [GridCell], cols: usize, text: &str, fg: Srgb) {
    let start = cols.saturating_sub(text.chars().count() + 1);
    write(row, start, text, fg);
}
