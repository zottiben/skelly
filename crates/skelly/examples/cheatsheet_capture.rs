//! Headless proof of the keybinding cheatsheet overlay (design §11 `⌘/`): the two-column
//! reference card over a blank terminal. Mirrors the binary's `cheatsheet` module (examples
//! cannot import the binary crate). Run: `cargo run -p skelly --example cheatsheet_capture -- out.png`.
#![allow(
    clippy::cast_precision_loss,
    clippy::cast_possible_truncation,
    clippy::cast_sign_loss,
    reason = "example: small non-negative layout dimensions"
)]
#![allow(clippy::too_many_arguments, reason = "example column builder")]
#![allow(
    clippy::many_single_char_names,
    reason = "example: terse q/l/m/x builders mirroring the module"
)]

use skelly_config::Appearance;
use skelly_render::{
    CaptureOverlay, CapturePane, Chrome, ChromeQuad, FontRole, GridCell, ProseLabel, PxRect,
    TextMeasure, Theme,
};

struct Bind(&'static str, &'static str);
struct Group(&'static str, &'static str, &'static [Bind]);

const GLOBAL: Group = Group(
    "Global",
    "",
    &[
        Bind("Command palette", "\u{2318}K"),
        Bind("Settings", "\u{2318},"),
        Bind("Show / hide sidebar", "\u{2318}B"),
        Bind("Cycle sidebar / rail", "\u{21e7}\u{2318}B"),
        Bind("Keybinding cheatsheet", "\u{2318}/"),
        Bind("Find in scrollback", "\u{2318}F"),
        Bind("Quit", "\u{2318}Q"),
    ],
);
const PANES: Group = Group(
    "Panes",
    "Leader = \u{2303}A (tmux-style).",
    &[
        Bind("Split right / down", "\u{2325}| \u{2325}-"),
        Bind("Move focus", "\u{2325}\u{2190}\u{2193}\u{2191}\u{2192}"),
        Bind("Resize pane", "\u{2303}\u{2325} arrows"),
        Bind("Swap pane", "\u{2325}\u{21e7} arrows"),
        Bind("Zoom / unzoom", "\u{2325}Z"),
        Bind("Close pane", "\u{2325}W"),
        Bind("Cycle layout preset", "\u{2325}Space"),
        Bind("Even out splits", "\u{2325}="),
    ],
);
const TERMINAL: Group = Group(
    "Terminal",
    "",
    &[
        Bind("Copy / paste", "\u{2318}C / V"),
        Bind("Clear scrollback", "\u{2318}L"),
        Bind("Font larger / smaller", "\u{2318}= / -"),
        Bind("Reset font size", "\u{2318}0"),
    ],
);
const TABS: Group = Group(
    "Tabs",
    "",
    &[
        Bind("New tab", "\u{2318}T"),
        Bind("Close tab", "\u{2318}W"),
        Bind("Next / prev tab", "\u{2325}\u{21e7} ] / ["),
        Bind("Go to tab 1-9", "\u{2318}1\u{2026}9"),
        Bind("Pin / unpin", "\u{21e7}\u{2318}P"),
        Bind("New group", "\u{21e7}\u{2318}N"),
        Bind("Rename tab", "F2"),
        Bind("Reopen closed", "\u{21e7}\u{2318}T"),
    ],
);
const SESSION: Group = Group(
    "Session & Git",
    "",
    &[
        Bind("Session timeline", "\u{21e7}\u{2318}H"),
        Bind("Git diff panel", "\u{21e7}\u{2318}G"),
        Bind("Rewind one step", "\u{2325}\u{2318}\u{2190}"),
        Bind("Fast-forward one step", "\u{2325}\u{2318}\u{2192}"),
        Bind("Return to now (HEAD)", "\u{2325}\u{2318}0"),
        Bind("Stage hunk (in diff)", "\u{2318}\u{21a9}"),
    ],
);
const COLUMNS: [&[Group]; 3] = [&[GLOBAL, SESSION], &[TABS, TERMINAL], &[PANES]];

const PAD: f32 = 28.0;
const COL_GAP: f32 = 36.0;
const TITLE_H: f32 = 24.0;
const NOTE_H: f32 = 16.0;
const ROW_H: f32 = 26.0;
const GROUP_GAP: f32 = 18.0;
const HEADER_H: f32 = 40.0;

fn main() {
    let path = std::env::args()
        .nth(1)
        .unwrap_or_else(|| "skelly-cheatsheet.png".to_owned());
    let theme_name = std::env::args()
        .nth(2)
        .unwrap_or_else(|| "ossein-dark".to_owned());
    let (width, height, scale) = (1920_u32, 1200_u32, 2.0_f64);
    let sc = scale as f32;
    let appearance = Appearance {
        theme: theme_name.clone(),
        ..Appearance::default()
    };
    let theme = Theme::resolve(&theme_name);
    let mut m = TextMeasure::new(sc);

    let col_w = column_width(sc, &mut m);
    let ncol = COLUMNS.len() as f32;
    let w = ncol * col_w + ((ncol - 1.0) * COL_GAP + 2.0 * PAD) * sc;
    let tallest = COLUMNS
        .iter()
        .map(|c| column_height(c, sc))
        .fold(0.0_f32, f32::max);
    let h = (HEADER_H + PAD) * sc + tallest + PAD * sc;
    let panel = PxRect {
        x: (width as f32 - w) / 2.0,
        y: (height as f32 - h) / 2.0,
        w,
        h,
    };
    let (quads, labels) = build(panel, sc, &theme, &mut m);
    let overlay = CaptureOverlay {
        panel,
        quads,
        labels,
    };
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
        focused: false,
        logo: None,
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

fn column_height(groups: &[Group], sc: f32) -> f32 {
    let mut hh = 0.0;
    for (i, g) in groups.iter().enumerate() {
        if i > 0 {
            hh += GROUP_GAP * sc;
        }
        hh += TITLE_H * sc;
        if !g.1.is_empty() {
            hh += NOTE_H * sc;
        }
        hh += g.2.len() as f32 * ROW_H * sc;
    }
    hh
}

fn column_width(sc: f32, m: &mut TextMeasure) -> f32 {
    let mut w = 210.0 * sc;
    for g in COLUMNS.iter().copied().flatten() {
        if !g.1.is_empty() {
            w = w.max(m.width(g.1, FontRole::Caption, None) + 8.0 * sc);
        }
        for b in g.2 {
            let aw = m.width(b.0, FontRole::Body, None);
            let cw = m.width(b.1, FontRole::Mono, None);
            w = w.max(aw + cw + 32.0 * sc);
        }
    }
    w
}

fn build(
    panel: PxRect,
    sc: f32,
    theme: &Theme,
    m: &mut TextMeasure,
) -> (Vec<ChromeQuad>, Vec<ProseLabel>) {
    let mut q = Vec::new();
    let mut l = Vec::new();
    let pad = PAD * sc;
    let x0 = panel.x + pad;
    l.push(label(
        "Keybindings",
        FontRole::H2,
        theme.fg_primary,
        x0,
        panel.y + pad,
    ));
    let hint_w = m.width("esc to close", FontRole::Caption, None);
    l.push(label(
        "esc to close",
        FontRole::Caption,
        theme.fg_muted,
        panel.x + panel.w - pad - hint_w,
        panel.y + pad + 6.0 * sc,
    ));
    let body_top = panel.y + (HEADER_H + PAD) * sc;
    let ncol = COLUMNS.len() as f32;
    let col_w = (panel.w - 2.0 * pad - (ncol - 1.0) * COL_GAP * sc) / ncol;
    for (i, groups) in COLUMNS.iter().enumerate() {
        let x = x0 + i as f32 * (col_w + COL_GAP * sc);
        column(&mut q, &mut l, m, groups, x, body_top, col_w, sc, theme);
    }
    (q, l)
}

fn column(
    q: &mut Vec<ChromeQuad>,
    l: &mut Vec<ProseLabel>,
    m: &mut TextMeasure,
    groups: &[Group],
    x: f32,
    top: f32,
    col_w: f32,
    sc: f32,
    theme: &Theme,
) {
    let mut y = top;
    for (i, g) in groups.iter().enumerate() {
        if i > 0 {
            y += GROUP_GAP * sc;
        }
        l.push(label(g.0, FontRole::Title, theme.fg_primary, x, y));
        y += TITLE_H * sc;
        q.push(ChromeQuad::fill(
            PxRect {
                x,
                y: y - 6.0 * sc,
                w: col_w,
                h: sc.max(1.0),
            },
            theme.border_subtle,
        ));
        if !g.1.is_empty() {
            l.push(label(g.1, FontRole::Caption, theme.fg_muted, x, y));
            y += NOTE_H * sc;
        }
        let line = m.line_height(FontRole::Body);
        for b in g.2 {
            let cy = y + (ROW_H * sc - line) * 0.5;
            l.push(label(b.0, FontRole::Body, theme.fg_secondary, x, cy));
            let cw = m.width(b.1, FontRole::Mono, None);
            l.push(label(
                b.1,
                FontRole::Mono,
                theme.fg_primary,
                x + col_w - cw,
                cy,
            ));
            y += ROW_H * sc;
        }
    }
}

fn label(text: &str, role: FontRole, color: skelly_render::Srgb, x: f32, y: f32) -> ProseLabel {
    ProseLabel {
        text: text.to_owned(),
        x,
        y,
        role,
        color,
        weight: None,
        max_w: f32::MAX,
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
