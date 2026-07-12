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
use skelly_render::{
    CaptureSettings, ChromeQuad, FontRole, ProseLabel, PxRect, Srgb, TextMeasure, Theme,
};

// Settings layout constants (logical px) - mirror the binary's `settings` module.
const S_NAV_WIDTH: f32 = 210.0;
const S_PAD: f32 = 20.0;
const S_NAV_PAD: f32 = 16.0;
const S_CONTENT_PAD: f32 = 24.0;
const S_HEADER_H: f32 = 44.0;
const S_NAV_ROW_H: f32 = 32.0;
const S_CTRL_ROW_H: f32 = 34.0;

fn main() {
    let path = std::env::args()
        .nth(1)
        .unwrap_or_else(|| "skelly-settings.png".to_owned());
    let (width, height, scale) = (1200_u32, 920_u32, 2.0_f64);

    let theme_name = std::env::args()
        .nth(2)
        .unwrap_or_else(|| "ossein-dark".to_owned());
    let appearance = Appearance {
        font_family: "SauceCodePro Nerd Font Mono".to_owned(),
        theme: theme_name,
        ..Appearance::default()
    };
    let theme = Theme::resolve(&appearance.theme);
    let settings = build_settings(&theme, width, height, scale as f32);

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

/// A representative Appearance settings panel as a proportional display list, mirroring the
/// binary's `settings` module so the capture verifies the render path.
#[allow(
    clippy::too_many_lines,
    reason = "one straight-line representative settings builder mirroring the binary"
)]
fn build_settings(theme: &Theme, width: u32, height: u32, scale: f32) -> CaptureSettings {
    let mut m = TextMeasure::new(scale);
    let panel = PxRect {
        x: 0.0,
        y: 0.0,
        w: width as f32,
        h: height as f32,
    };
    let nav_divider_x = panel.x + S_NAV_WIDTH * scale;
    let content_x = nav_divider_x + S_CONTENT_PAD * scale;
    let content_right = panel.x + panel.w - S_PAD * scale;
    let mut quads = Vec::new();
    let mut labels = Vec::new();

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
    let active = 0usize;
    let selected = 3usize; // "Cursor style"

    // Header.
    let hy = panel.y + S_PAD * scale;
    s_row(
        &mut labels,
        &mut m,
        "Appearance",
        FontRole::H2,
        theme.fg_primary,
        content_x,
        hy,
        S_HEADER_H,
        scale,
    );
    s_right(
        &mut labels,
        &mut m,
        "esc to close",
        FontRole::Caption,
        theme.fg_muted,
        content_right,
        hy,
        S_HEADER_H,
        scale,
    );

    // Nav.
    let mut ny = hy + S_HEADER_H * scale;
    for (i, (icon, label)) in categories.iter().enumerate() {
        if i == active {
            quads.push(ChromeQuad::tint(
                PxRect {
                    x: panel.x,
                    y: ny,
                    w: nav_divider_x - panel.x,
                    h: S_NAV_ROW_H * scale,
                },
                theme.accent,
                0.14,
                0.0,
            ));
            quads.push(ChromeQuad::fill(
                PxRect {
                    x: panel.x,
                    y: ny,
                    w: (2.0 * scale).max(1.0),
                    h: S_NAV_ROW_H * scale,
                },
                theme.accent,
            ));
        }
        let color = if i == active {
            theme.fg_primary
        } else {
            theme.fg_secondary
        };
        s_row(
            &mut labels,
            &mut m,
            &format!("{icon} {label}"),
            FontRole::Label,
            color,
            panel.x + S_NAV_PAD * scale,
            ny,
            S_NAV_ROW_H,
            scale,
        );
        ny += S_NAV_ROW_H * scale;
    }

    // Controls.
    let mut cy = hy + S_HEADER_H * scale;
    for (i, (label, value)) in controls.iter().enumerate() {
        let focused = i == selected;
        if focused {
            quads.push(ChromeQuad::tint(
                PxRect {
                    x: nav_divider_x,
                    y: cy,
                    w: content_right + S_PAD * scale - nav_divider_x,
                    h: S_CTRL_ROW_H * scale,
                },
                theme.accent,
                0.14,
                0.0,
            ));
        }
        let label_fg = if focused {
            theme.fg_primary
        } else {
            theme.fg_secondary
        };
        s_row(
            &mut labels,
            &mut m,
            label,
            FontRole::Label,
            label_fg,
            content_x,
            cy,
            S_CTRL_ROW_H,
            scale,
        );
        if focused {
            let close = " \u{203a}";
            let open = "\u{2039} ";
            let mut x = content_right - m.width(close, FontRole::Label, None);
            s_row(
                &mut labels,
                &mut m,
                close,
                FontRole::Label,
                theme.fg_muted,
                x,
                cy,
                S_CTRL_ROW_H,
                scale,
            );
            x -= m.width(value, FontRole::Label, None);
            s_row(
                &mut labels,
                &mut m,
                value,
                FontRole::Label,
                theme.accent,
                x,
                cy,
                S_CTRL_ROW_H,
                scale,
            );
            x -= m.width(open, FontRole::Label, None);
            s_row(
                &mut labels,
                &mut m,
                open,
                FontRole::Label,
                theme.fg_muted,
                x,
                cy,
                S_CTRL_ROW_H,
                scale,
            );
        } else {
            let x = content_right - m.width(value, FontRole::Label, None);
            s_row(
                &mut labels,
                &mut m,
                value,
                FontRole::Label,
                theme.fg_secondary,
                x,
                cy,
                S_CTRL_ROW_H,
                scale,
            );
        }
        cy += S_CTRL_ROW_H * scale;
    }

    // Footer.
    let fy = panel.y + panel.h - S_PAD * scale - FontRole::Caption.line_height_px(scale);
    s_row(
        &mut labels,
        &mut m,
        "up/down move   left/right change   tab category   esc close",
        FontRole::Caption,
        theme.fg_muted,
        content_x,
        fy,
        0.0,
        scale,
    );

    CaptureSettings {
        panel,
        nav_divider_x,
        quads,
        labels,
    }
}

/// Push a left-anchored label vertically centered in a `row_h` row (`row_h = 0` = top at `top`).
#[allow(clippy::too_many_arguments, reason = "one focused example helper")]
fn s_row(
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

/// Push a right-anchored label ending at `right`.
#[allow(clippy::too_many_arguments, reason = "one focused example helper")]
fn s_right(
    labels: &mut Vec<ProseLabel>,
    m: &mut TextMeasure,
    text: &str,
    role: FontRole,
    color: Srgb,
    right: f32,
    top: f32,
    row_h: f32,
    scale: f32,
) {
    let x = right - m.width(text, role, None);
    s_row(labels, m, text, role, color, x, top, row_h, scale);
}
