//! Headless proof of the first-run onboarding modal (design §10.1): the centered welcome card
//! with the vertebra mark, shell + theme pickers, key-chord hints, and Skip / Start buttons,
//! rendered over a blank terminal. Mirrors the binary's `onboarding` module (examples cannot
//! import the binary crate). Run: `cargo run -p skelly --example onboarding_capture -- out.png`.
#![allow(
    clippy::cast_precision_loss,
    clippy::cast_possible_truncation,
    clippy::cast_sign_loss,
    reason = "example: small non-negative surface + layout dimensions"
)]
#![allow(
    clippy::many_single_char_names,
    reason = "example: terse q/l/m builders mirroring the binary layout helpers"
)]

use skelly_config::Appearance;
use skelly_render::{
    logo_chrome_quads, CaptureOverlay, CapturePane, Chrome, ChromeQuad, FontRole, GridCell,
    ProseLabel, PxRect, TextMeasure, Theme,
};

// Mirrors `onboarding.rs` metrics (logical px).
const CARD_W: f32 = 460.0;
const PAD_X: f32 = 32.0;
const PAD_TOP: f32 = 30.0;
const PAD_BOTTOM: f32 = 26.0;
const MARK: f32 = 38.0;
const MARK_GAP: f32 = 13.0;
const HEADER_GAP: f32 = 22.0;
const LABEL_H: f32 = 14.0;
const LABEL_GAP: f32 = 8.0;
const SEG_H: f32 = 34.0;
const SEG_GAP: f32 = 8.0;
const SEG_RADIUS: f32 = 8.0;
const SHELL_GAP: f32 = 18.0;
const SWATCH_H: f32 = 32.0;
const STRIP_H: f32 = 24.0;
const THEME_GAP: f32 = 22.0;
const CHIP_H: f32 = 22.0;
const CHIP_GAP: f32 = 22.0;
const BTN_H: f32 = 36.0;
const BTN_GAP: f32 = 10.0;

const SHELLS: [&str; 3] = ["zsh", "bash", "fish"];
const THEMES: [(&str, &str); 2] = [
    ("ossein-dark", "Ossein Dark"),
    ("ossein-light", "Ossein Light"),
];
const HINTS: [(&str, &str); 3] = [
    ("\u{2318}K", "palette"),
    ("\u{2318}T", "new tab"),
    ("\u{2325}|", "split"),
];

fn main() {
    let path = std::env::args()
        .nth(1)
        .unwrap_or_else(|| "skelly-onboarding.png".to_owned());
    let theme_name = std::env::args()
        .nth(2)
        .unwrap_or_else(|| "ossein-dark".to_owned());
    let (width, height, scale) = (1360_u32, 760_u32, 2.0_f64);
    let sc = scale as f32;
    let appearance = Appearance {
        theme: theme_name.clone(),
        ..Appearance::default()
    };
    let theme = Theme::resolve(&theme_name);

    // A single blank pane behind the modal.
    let pane = CapturePane {
        rect: PxRect {
            x: 0.0,
            y: 0.0,
            w: width as f32,
            h: height as f32,
        },
        origin: (24.0, 24.0),
        rows: Vec::<Vec<GridCell>>::new(),
        cursor: (0, 0),
        cursor_shape: skelly_render::CursorShape::Block,
        focused: false,
        logo: None,
    };

    let card_h = (PAD_TOP
        + MARK
        + HEADER_GAP
        + LABEL_H
        + LABEL_GAP
        + SEG_H
        + SHELL_GAP
        + LABEL_H
        + LABEL_GAP
        + SWATCH_H
        + STRIP_H
        + THEME_GAP
        + CHIP_H
        + CHIP_GAP
        + BTN_H
        + PAD_BOTTOM)
        * sc;
    let card_w = CARD_W * sc;
    let panel = PxRect {
        x: (width as f32 - card_w) / 2.0,
        y: (height as f32 - card_h) / 2.0,
        w: card_w,
        h: card_h,
    };

    let mut measure = TextMeasure::new(sc);
    let (quads, labels) = build(&panel, sc, &theme, &mut measure, 0, 0);
    let overlay = CaptureOverlay {
        panel,
        quads,
        labels,
    };

    let rgba = skelly_render::capture_panes_rgba(
        &appearance,
        width,
        height,
        scale,
        &[pane],
        &Chrome {
            overlay: Some(&overlay),
            ..Default::default()
        },
    );
    write_png(&path, width, height, &rgba);
    println!("wrote {path}");
}

/// Build the modal content - mirrors `onboarding::build` for a `shell_sel`/`theme_sel`.
fn build(
    panel: &PxRect,
    sc: f32,
    theme: &Theme,
    m: &mut TextMeasure,
    shell_sel: usize,
    theme_sel: usize,
) -> (Vec<ChromeQuad>, Vec<ProseLabel>) {
    let pad = PAD_X * sc;
    let inner_x = panel.x + pad;
    let inner_w = panel.w - 2.0 * pad;
    let mut q = Vec::new();
    let mut l = Vec::new();
    let mut y = panel.y + PAD_TOP * sc;

    // Header: vertebra mark + title/subtitle.
    q.extend(logo_chrome_quads(inner_x, y, MARK * sc, theme, 1.0));
    let tx = inner_x + (MARK + MARK_GAP) * sc;
    let th = m.line_height(FontRole::H2);
    let sh = m.line_height(FontRole::Caption);
    let top = y + (MARK * sc - th - sh) * 0.5;
    push(
        &mut l,
        "Welcome to skelly",
        FontRole::H2,
        theme.fg_primary,
        tx,
        top,
    );
    push(
        &mut l,
        "Barebones. Let\u{2019}s set two things.",
        FontRole::Caption,
        theme.fg_muted,
        tx,
        top + th,
    );
    y += (MARK + HEADER_GAP) * sc;

    // SHELL.
    label(&mut l, m, "SHELL", inner_x, y, sc, theme);
    y += (LABEL_H + LABEL_GAP) * sc;
    let seg_w = (inner_w - 2.0 * SEG_GAP * sc) / 3.0;
    for (i, name) in SHELLS.iter().enumerate() {
        let r = PxRect {
            x: inner_x + i as f32 * (seg_w + SEG_GAP * sc),
            y,
            w: seg_w,
            h: SEG_H * sc,
        };
        segment(&mut q, &r, i == shell_sel, sc, theme);
        center(
            &mut l,
            m,
            name,
            r,
            FontRole::Mono,
            if i == shell_sel {
                theme.fg_primary
            } else {
                theme.fg_secondary
            },
        );
    }
    y += (SEG_H + SHELL_GAP) * sc;

    // THEME.
    label(&mut l, m, "THEME", inner_x, y, sc, theme);
    y += (LABEL_H + LABEL_GAP) * sc;
    let cw = (inner_w - SEG_GAP * sc) / 2.0;
    let ch = (SWATCH_H + STRIP_H) * sc;
    for (i, (name, disp)) in THEMES.iter().enumerate() {
        let r = PxRect {
            x: inner_x + i as f32 * (cw + SEG_GAP * sc),
            y,
            w: cw,
            h: ch,
        };
        theme_card(&mut q, &mut l, m, &r, name, disp, i == theme_sel, sc, theme);
    }
    y += ch + THEME_GAP * sc;

    // Hint chips (centered).
    hints(&mut q, &mut l, m, panel, y, sc, theme);
    y += (CHIP_H + CHIP_GAP) * sc;

    // Buttons.
    let stroke = sc.max(1.0);
    let skip_w = (inner_w - BTN_GAP * sc) / 3.0;
    let skip = PxRect {
        x: inner_x,
        y,
        w: skip_w,
        h: BTN_H * sc,
    };
    q.push(ChromeQuad::rounded(skip, theme.border, SEG_RADIUS * sc));
    q.push(ChromeQuad::rounded(
        inset(skip, stroke),
        theme.bg_elevated,
        SEG_RADIUS * sc - stroke,
    ));
    center(&mut l, m, "Skip", skip, FontRole::Body, theme.fg_secondary);
    let start = PxRect {
        x: inner_x + skip_w + BTN_GAP * sc,
        y,
        w: inner_w - skip_w - BTN_GAP * sc,
        h: BTN_H * sc,
    };
    q.push(ChromeQuad::rounded(start, theme.accent, SEG_RADIUS * sc));
    center(
        &mut l,
        m,
        "Start  \u{276f}",
        start,
        FontRole::Body,
        theme.bg_base.to_srgb(),
    );

    (q, l)
}

fn segment(q: &mut Vec<ChromeQuad>, r: &PxRect, selected: bool, sc: f32, theme: &Theme) {
    let radius = SEG_RADIUS * sc;
    let stroke = sc.max(1.0);
    if selected {
        q.push(ChromeQuad::rounded(
            *r,
            theme.accent.over(theme.bg_elevated, 0.4),
            radius,
        ));
        q.push(ChromeQuad::rounded(
            inset(*r, stroke),
            theme.accent_subtle_on(theme.bg_elevated),
            radius - stroke,
        ));
    } else {
        q.push(ChromeQuad::rounded(*r, theme.border_subtle, radius));
        q.push(ChromeQuad::rounded(
            inset(*r, stroke),
            theme.bg_surface,
            radius - stroke,
        ));
    }
}

#[allow(clippy::too_many_arguments, reason = "example theme-card mirror")]
fn theme_card(
    q: &mut Vec<ChromeQuad>,
    l: &mut Vec<ProseLabel>,
    m: &mut TextMeasure,
    r: &PxRect,
    name: &str,
    disp: &str,
    selected: bool,
    sc: f32,
    theme: &Theme,
) {
    let radius = SEG_RADIUS * sc;
    let stroke = sc.max(1.0);
    let border = if selected {
        theme.accent.over(theme.bg_elevated, 0.4)
    } else {
        theme.border_subtle
    };
    q.push(ChromeQuad::rounded(*r, border, radius));
    let inner = inset(*r, stroke);
    let card_theme = Theme::resolve(name);
    let swatch_h = SWATCH_H * sc - stroke;
    q.push(ChromeQuad::rounded(
        PxRect {
            x: inner.x,
            y: inner.y,
            w: inner.w,
            h: swatch_h,
        },
        card_theme.bg_base.to_srgb(),
        radius - stroke,
    ));
    q.push(ChromeQuad::rounded(
        PxRect {
            x: inner.x,
            y: inner.y + swatch_h,
            w: inner.w,
            h: inner.h - swatch_h,
        },
        theme.bg_surface,
        radius - stroke,
    ));
    let swl = m.line_height(FontRole::Mono);
    l.push(ProseLabel {
        text: "\u{276f} ossein".to_owned(),
        x: inner.x + 9.0 * sc,
        y: inner.y + (swatch_h - swl) * 0.5,
        role: FontRole::Mono,
        color: card_theme.accent,
        weight: None,
        max_w: f32::MAX,
    });
    let sl = m.line_height(FontRole::Caption);
    l.push(ProseLabel {
        text: disp.to_owned(),
        x: inner.x + 9.0 * sc,
        y: inner.y + swatch_h + (inner.h - swatch_h - sl) * 0.5,
        role: FontRole::Caption,
        color: if selected {
            theme.fg_primary
        } else {
            theme.fg_secondary
        },
        weight: None,
        max_w: f32::MAX,
    });
}

fn hints(
    q: &mut Vec<ChromeQuad>,
    l: &mut Vec<ProseLabel>,
    m: &mut TextMeasure,
    panel: &PxRect,
    y: f32,
    sc: f32,
    theme: &Theme,
) {
    let gap = 16.0 * sc;
    let kp = 6.0 * sc;
    let kg = 6.0 * sc;
    let widths: Vec<(f32, f32, f32)> = HINTS
        .iter()
        .map(|(c, lbl)| {
            let kw = m.width(c, FontRole::Micro, None) + 2.0 * kp;
            let lw = m.width(lbl, FontRole::Caption, None);
            (kw, lw, kw + kg + lw)
        })
        .collect();
    let total: f32 = widths.iter().map(|(_, _, w)| w).sum::<f32>() + gap * (HINTS.len() - 1) as f32;
    let mut x = panel.x + (panel.w - total) * 0.5;
    let h = CHIP_H * sc;
    let kl = m.line_height(FontRole::Micro);
    let ll = m.line_height(FontRole::Caption);
    for (i, (c, lbl)) in HINTS.iter().enumerate() {
        let (kw, _, w) = widths[i];
        q.push(ChromeQuad::rounded(
            PxRect { x, y, w: kw, h },
            theme.bg_surface,
            4.0 * sc,
        ));
        l.push(ProseLabel {
            text: (*c).to_owned(),
            x: x + kp,
            y: y + (h - kl) * 0.5,
            role: FontRole::Micro,
            color: theme.fg_primary,
            weight: None,
            max_w: f32::MAX,
        });
        l.push(ProseLabel {
            text: (*lbl).to_owned(),
            x: x + kw + kg,
            y: y + (h - ll) * 0.5,
            role: FontRole::Caption,
            color: theme.fg_muted,
            weight: None,
            max_w: f32::MAX,
        });
        x += w + gap;
    }
}

fn label(
    l: &mut Vec<ProseLabel>,
    m: &mut TextMeasure,
    text: &str,
    x: f32,
    y: f32,
    sc: f32,
    theme: &Theme,
) {
    let h = m.line_height(FontRole::Micro);
    l.push(ProseLabel {
        text: text.to_owned(),
        x,
        y: y + (LABEL_H * sc - h) * 0.5,
        role: FontRole::Micro,
        color: theme.fg_muted,
        weight: None,
        max_w: f32::MAX,
    });
}

fn center(
    l: &mut Vec<ProseLabel>,
    m: &mut TextMeasure,
    text: &str,
    r: PxRect,
    role: FontRole,
    color: skelly_render::Srgb,
) {
    let w = m.width(text, role, None);
    let line = m.line_height(role);
    l.push(ProseLabel {
        text: text.to_owned(),
        x: r.x + (r.w - w) * 0.5,
        y: r.y + (r.h - line) * 0.5,
        role,
        color,
        weight: None,
        max_w: f32::MAX,
    });
}

fn push(
    l: &mut Vec<ProseLabel>,
    text: &str,
    role: FontRole,
    color: skelly_render::Srgb,
    x: f32,
    y: f32,
) {
    l.push(ProseLabel {
        text: text.to_owned(),
        x,
        y,
        role,
        color,
        weight: None,
        max_w: f32::MAX,
    });
}

fn inset(r: PxRect, d: f32) -> PxRect {
    PxRect {
        x: r.x + d,
        y: r.y + d,
        w: (r.w - 2.0 * d).max(0.0),
        h: (r.h - 2.0 * d).max(0.0),
    }
}

fn write_png(path: &str, width: u32, height: u32, rgba: &[u8]) {
    let file = std::fs::File::create(path).expect("create png");
    let mut enc = png::Encoder::new(std::io::BufWriter::new(file), width, height);
    enc.set_color(png::ColorType::Rgba);
    enc.set_depth(png::BitDepth::Eight);
    enc.write_header()
        .expect("png header")
        .write_image_data(rgba)
        .expect("png data");
}
