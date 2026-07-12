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
const S_CTRL_ROW_H: f32 = 40.0;
// §09 widget dimensions - mirror the binary's `settings` module.
const S_TOGGLE_W: f32 = 38.0;
const S_TOGGLE_H: f32 = 22.0;
const S_TOGGLE_KNOB: f32 = 18.0;
const S_TOGGLE_INSET: f32 = 2.0;
const S_SEG_PAD: f32 = 3.0;
const S_SEG_ITEM_PAD_X: f32 = 11.0;
const S_SEG_H: f32 = 24.0;
const S_SEG_RADIUS: f32 = 7.0;
const S_SEG_ITEM_RADIUS: f32 = 5.0;
const S_SLIDER_W: f32 = 116.0;
const S_SLIDER_TRACK_H: f32 = 6.0;
const S_SLIDER_KNOB: f32 = 14.0;
const S_SLIDER_VALUE_GAP: f32 = 12.0;

/// A representative control widget for the capture (mirrors the binary's `Kind` render).
enum Widget {
    Segmented(&'static [&'static str], usize),
    Toggle(bool),
    Slider(&'static str, f32),
}

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
        (
            "Theme",
            Widget::Segmented(&["Ossein Dark", "Ossein Light"], 0),
        ),
        ("Font size", Widget::Slider("14px", 0.25)),
        ("Line height", Widget::Slider("1.2", 0.18)),
        (
            "Cursor style",
            Widget::Segmented(&["Block", "Bar", "Underline"], 0),
        ),
        ("Font ligatures", Widget::Toggle(true)),
        ("Bold uses bright colors", Widget::Toggle(true)),
        ("Background blur", Widget::Slider("18", 0.18)),
        ("Window opacity", Widget::Slider("0.98", 0.98)),
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
            // accent.subtle active-row fill, sRGB-composited over the nav column (bg.base).
            quads.push(ChromeQuad::fill(
                PxRect {
                    x: panel.x,
                    y: ny,
                    w: nav_divider_x - panel.x,
                    h: S_NAV_ROW_H * scale,
                },
                theme.accent_subtle_on(theme.bg_base.to_srgb()),
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

    // Controls, each rendered as its §09 widget.
    let mut cy = hy + S_HEADER_H * scale;
    for (i, (label, widget)) in controls.iter().enumerate() {
        let focused = i == selected;
        if focused {
            // accent.subtle selected-row band, sRGB-composited over the content (bg.elevated).
            quads.push(ChromeQuad::fill(
                PxRect {
                    x: nav_divider_x,
                    y: cy,
                    w: content_right + S_PAD * scale - nav_divider_x,
                    h: S_CTRL_ROW_H * scale,
                },
                theme.accent_subtle_on(theme.bg_elevated),
            ));
        }
        s_row(
            &mut labels,
            &mut m,
            label,
            FontRole::Label,
            if focused {
                theme.fg_primary
            } else {
                theme.fg_secondary
            },
            content_x,
            cy,
            S_CTRL_ROW_H,
            scale,
        );
        match widget {
            Widget::Toggle(on) => {
                s_toggle(&mut quads, *on, content_right, cy, scale, theme);
            }
            Widget::Segmented(options, sel) => s_segmented(
                &mut quads,
                &mut labels,
                &mut m,
                options,
                *sel,
                content_right,
                cy,
                scale,
                theme,
            ),
            Widget::Slider(value, fraction) => s_slider(
                &mut quads,
                &mut labels,
                &mut m,
                value,
                *fraction,
                content_right,
                cy,
                scale,
                theme,
            ),
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

/// A §09 toggle switch, right-anchored - mirrors the binary's `push_toggle`.
fn s_toggle(
    quads: &mut Vec<ChromeQuad>,
    on: bool,
    content_right: f32,
    top: f32,
    scale: f32,
    theme: &Theme,
) {
    let w = S_TOGGLE_W * scale;
    let h = S_TOGGLE_H * scale;
    let x = content_right - w;
    let y = top + (S_CTRL_ROW_H * scale - h) * 0.5;
    quads.push(ChromeQuad::rounded(
        PxRect { x, y, w, h },
        if on { theme.accent } else { theme.border },
        h * 0.5,
    ));
    let knob = S_TOGGLE_KNOB * scale;
    let inset = S_TOGGLE_INSET * scale;
    let knob_x = if on { x + w - inset - knob } else { x + inset };
    quads.push(ChromeQuad::rounded(
        PxRect {
            x: knob_x,
            y: y + inset,
            w: knob,
            h: knob,
        },
        if on { theme.bg_inset } else { theme.fg_muted },
        knob * 0.5,
    ));
}

/// A §09 segmented control, right-anchored - mirrors the binary's `push_segmented`.
#[allow(clippy::too_many_arguments, reason = "one focused example helper")]
fn s_segmented(
    quads: &mut Vec<ChromeQuad>,
    labels: &mut Vec<ProseLabel>,
    m: &mut TextMeasure,
    options: &[&str],
    selected: usize,
    content_right: f32,
    top: f32,
    scale: f32,
    theme: &Theme,
) {
    let pad = S_SEG_PAD * scale;
    let item_pad = S_SEG_ITEM_PAD_X * scale;
    let seg_h = S_SEG_H * scale;
    let line_h = m.line_height(FontRole::Caption);
    let widths: Vec<f32> = options
        .iter()
        .map(|o| m.width(o, FontRole::Caption, None) + 2.0 * item_pad)
        .collect();
    let total: f32 = widths.iter().sum::<f32>() + 2.0 * pad;
    let cx = content_right - total;
    let cy = top + (S_CTRL_ROW_H * scale - (seg_h + 2.0 * pad)) * 0.5;
    quads.push(ChromeQuad::rounded(
        PxRect {
            x: cx,
            y: cy,
            w: total,
            h: seg_h + 2.0 * pad,
        },
        theme.bg_inset,
        S_SEG_RADIUS * scale,
    ));
    let mut x = cx + pad;
    for (i, option) in options.iter().enumerate() {
        let w = widths[i];
        let active = i == selected;
        if active {
            quads.push(ChromeQuad::rounded(
                PxRect {
                    x,
                    y: cy + pad,
                    w,
                    h: seg_h,
                },
                theme.bg_elevated,
                S_SEG_ITEM_RADIUS * scale,
            ));
        }
        labels.push(ProseLabel {
            text: (*option).to_owned(),
            x: x + item_pad,
            y: cy + pad + (seg_h - line_h) * 0.5,
            role: FontRole::Caption,
            color: if active {
                theme.fg_primary
            } else {
                theme.fg_secondary
            },
            weight: None,
            max_w: f32::MAX,
        });
        x += w;
    }
}

/// A §09 slider, right-anchored - mirrors the binary's `push_slider`.
#[allow(clippy::too_many_arguments, reason = "one focused example helper")]
fn s_slider(
    quads: &mut Vec<ChromeQuad>,
    labels: &mut Vec<ProseLabel>,
    m: &mut TextMeasure,
    value: &str,
    fraction: f32,
    content_right: f32,
    top: f32,
    scale: f32,
    theme: &Theme,
) {
    let value_w = m.width(value, FontRole::Mono, None);
    let vline = m.line_height(FontRole::Mono);
    labels.push(ProseLabel {
        text: value.to_owned(),
        x: content_right - value_w,
        y: top + (S_CTRL_ROW_H * scale - vline) * 0.5,
        role: FontRole::Mono,
        color: theme.accent,
        weight: None,
        max_w: f32::MAX,
    });
    let track_w = S_SLIDER_W * scale;
    let track_h = S_SLIDER_TRACK_H * scale;
    let track_x = content_right - value_w - S_SLIDER_VALUE_GAP * scale - track_w;
    let track_y = top + (S_CTRL_ROW_H * scale - track_h) * 0.5;
    quads.push(ChromeQuad::rounded(
        PxRect {
            x: track_x,
            y: track_y,
            w: track_w,
            h: track_h,
        },
        theme.border_subtle,
        track_h * 0.5,
    ));
    quads.push(ChromeQuad::rounded(
        PxRect {
            x: track_x,
            y: track_y,
            w: (track_w * fraction).max(track_h),
            h: track_h,
        },
        theme.accent,
        track_h * 0.5,
    ));
    let knob = S_SLIDER_KNOB * scale;
    quads.push(ChromeQuad::rounded(
        PxRect {
            x: track_x + track_w * fraction - knob * 0.5,
            y: top + (S_CTRL_ROW_H * scale - knob) * 0.5,
            w: knob,
            h: knob,
        },
        theme.fg_primary,
        knob * 0.5,
    ));
}
