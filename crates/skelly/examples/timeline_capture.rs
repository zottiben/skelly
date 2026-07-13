//! Headless proof of the M4 session-timeline dock: render a live-ish pane workspace with
//! the timeline docked on the right (its event list, the viewing banner, the selected +
//! "viewing" event highlights, the dimmed "future" events, the actor legend, and the
//! left-edge divider) to a PNG, with no window or screen recording needed.
//!
//! The live binary drives the dock from its real `timeline` module over a
//! `skelly_session::Timeline`; examples cannot import the binary crate, so this hand-builds
//! a representative grid (as `git_dock_capture` does) purely to exercise the
//! `timeline_quads` render path. It shows a REWOUND state - the selection is on a past
//! commit, so the accent viewing bar, the viewing banner, and the dimmed future events are
//! all visible. An optional second arg picks the theme (`ossein-light`).
//! Run: `cargo run -p skelly --example timeline_capture -- timeline.png [theme]`.
#![allow(
    clippy::cast_precision_loss,
    clippy::cast_possible_truncation,
    clippy::cast_sign_loss,
    reason = "example: surface dimensions and grid sizes are small, non-negative values"
)]

use skelly_config::Appearance;
use skelly_render::{
    CapturePane, CaptureTimeline, Chrome, ChromeQuad, FontRole, GridCell, ProseLabel, PxRect, Srgb,
    TextMeasure, Theme,
};

/// Logical dock width - mirrors the binary's `GIT_DOCK_WIDTH` (420, the guide's default).
const DOCK_WIDTH: f32 = 420.0;
/// Logical window margin - mirrors the binary's `WINDOW_PAD`.
const WINDOW_PAD: f32 = 12.0;
/// Logical inner pane inset - mirrors the binary's `PANE_INSET`.
const PANE_INSET: f32 = 6.0;
// Timeline layout constants (logical px) - mirror the binary's `timeline` module (§10.5).
const T_PAD_X: f32 = 14.0;
const T_PAD_TOP: f32 = 12.0;
const T_STATUS_H: f32 = 26.0;
const T_LABEL_H: f32 = 24.0;
const T_EVENT_H: f32 = 30.0;
const T_FOOT_H: f32 = 60.0;
const T_RIGHT_GAP: f32 = 10.0;

fn main() {
    let path = std::env::args()
        .nth(1)
        .unwrap_or_else(|| "skelly-timeline.png".to_owned());
    let (width, height, scale) = (1360_u32, 680_u32, 2.0_f64);

    let theme_name = std::env::args()
        .nth(2)
        .unwrap_or_else(|| "ossein-dark".to_owned());
    let appearance = Appearance {
        font_family: "SauceCodePro Nerd Font Mono".to_owned(),
        theme: theme_name,
        ..Appearance::default()
    };
    let theme = Theme::resolve(&appearance.theme);
    let sc = scale as f32;

    // One pane, inset on the left by the window margin and on the right by the dock -
    // exactly as the binary's `viewport_rect` insets it.
    let pad = WINDOW_PAD * sc;
    let inset = PANE_INSET * sc;
    let dock_w = DOCK_WIDTH * sc;
    let pane_rect = PxRect {
        x: pad,
        y: pad,
        w: width as f32 - dock_w - 2.0 * pad,
        h: height as f32 - 2.0 * pad,
    };
    let pane = CapturePane {
        rect: pane_rect,
        origin: (pane_rect.x + inset, pane_rect.y + inset),
        rows: terminal_rows(&theme),
        cursor: (11, 2),
        cursor_shape: skelly_render::CursorShape::Block,
        focused: true,
        logo: None,
    };

    let dock = build_dock(width, height, sc, &theme);
    let rgba = skelly_render::capture_panes_rgba(
        &appearance,
        width,
        height,
        scale,
        &[pane],
        &Chrome {
            timeline: Some(&dock),
            ..Default::default()
        },
    );

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

/// One representative event: session-relative time, actor label + color, title, and an
/// optional right badge (`HEAD` / `VIEWING` / `FUTURE`).
struct Event {
    time: &'static str,
    actor: &'static str,
    actor_fg: Srgb,
    title: &'static str,
    badge: Option<(&'static str, Srgb)>,
    /// The foreground for the title (primary = selected, secondary = past, muted = future).
    title_fg: Srgb,
}

/// A representative session-timeline dock rewound to a past commit, mirroring the binary's
/// `timeline` layout: the viewing banner, the event list (selected + viewing highlighted,
/// future events dimmed), and the foot band (divider, legend, session summary).
#[allow(
    clippy::too_many_lines,
    reason = "one straight-line representative dock builder mirroring the binary"
)]
fn build_dock(width: u32, height: u32, scale: f32, theme: &Theme) -> CaptureTimeline {
    let mut m = TextMeasure::new(scale);
    let dock_w = DOCK_WIDTH * scale;
    let panel = PxRect {
        x: width as f32 - dock_w,
        y: 0.0,
        w: dock_w,
        h: height as f32,
    };
    let cx = panel.x + T_PAD_X * scale;
    let cr = panel.x + panel.w - T_PAD_X * scale;
    let mut quads = Vec::new();
    let mut labels = Vec::new();
    let mut y = panel.y + T_PAD_TOP * scale;

    // Status banner (viewing a past commit) + esc hint.
    t_row(
        &mut labels,
        &mut m,
        "Viewing 1:30 - 67b010a",
        FontRole::Label,
        theme.accent,
        cx,
        y,
        T_STATUS_H,
        scale,
    );
    t_right(
        &mut labels,
        &mut m,
        "esc",
        FontRole::Caption,
        theme.fg_muted,
        cr,
        y,
        T_STATUS_H,
        scale,
    );
    y += T_STATUS_H * scale;
    // Section label + hint.
    t_row(
        &mut labels,
        &mut m,
        "TIMELINE - 5 EVENTS",
        FontRole::Micro,
        theme.fg_muted,
        cx,
        y,
        T_LABEL_H,
        scale,
    );
    t_right(
        &mut labels,
        &mut m,
        "up down move",
        FontRole::Caption,
        theme.fg_muted,
        cr,
        y,
        T_LABEL_H,
        scale,
    );
    y += T_LABEL_H * scale;

    // The event list; index 2 is the selected past commit (viewing bar), 3-4 are dimmed future.
    let events = [
        Event {
            time: "0:00",
            actor: "system",
            actor_fg: theme.fg_muted,
            title: "Session started",
            badge: None,
            title_fg: theme.fg_secondary,
        },
        Event {
            time: "1:05",
            actor: "you",
            actor_fg: theme.accent,
            title: "Staged src/pane/tree.rs",
            badge: None,
            title_fg: theme.fg_secondary,
        },
        Event {
            time: "1:30",
            actor: "you",
            actor_fg: theme.accent,
            title: "git commit - feat: pane tree",
            badge: Some(("VIEWING", theme.accent)),
            title_fg: theme.fg_primary,
        },
        Event {
            time: "2:10",
            actor: "you",
            actor_fg: theme.accent,
            title: "Staged timeline.rs",
            badge: Some(("FUTURE", theme.accent)),
            title_fg: theme.fg_muted,
        },
        Event {
            time: "3:20",
            actor: "you",
            actor_fg: theme.accent,
            title: "git commit - feat: timeline",
            badge: Some(("HEAD", theme.diff_hunk)),
            title_fg: theme.fg_muted,
        },
    ];
    let selected = 2usize;
    let time_w = m.width("00:00", FontRole::Micro, None) + 8.0 * scale;
    let row_h = T_EVENT_H * scale;
    for (i, e) in events.iter().enumerate() {
        let row_top = y + i as f32 * row_h;
        if i == selected {
            // accent.subtle selected-row fill, sRGB-composited over bg.base (mirrors the binary).
            quads.push(ChromeQuad::fill(
                PxRect {
                    x: panel.x,
                    y: row_top,
                    w: panel.w,
                    h: row_h,
                },
                theme.accent_subtle_on(theme.bg_base.to_srgb()),
            ));
            quads.push(ChromeQuad::fill(
                PxRect {
                    x: panel.x,
                    y: row_top,
                    w: (2.0 * scale).max(1.0),
                    h: row_h,
                },
                theme.accent,
            ));
        }
        t_row(
            &mut labels,
            &mut m,
            e.time,
            FontRole::Micro,
            theme.fg_muted,
            cx,
            row_top,
            T_EVENT_H,
            scale,
        );
        let actor_x = t_right(
            &mut labels,
            &mut m,
            e.actor,
            FontRole::Micro,
            e.actor_fg,
            cr,
            row_top,
            T_EVENT_H,
            scale,
        );
        let mut right = actor_x;
        if let Some((text, fg)) = e.badge {
            right = t_right(
                &mut labels,
                &mut m,
                text,
                FontRole::Micro,
                fg,
                actor_x - T_RIGHT_GAP * scale,
                row_top,
                T_EVENT_H,
                scale,
            );
        }
        let title_x = cx + time_w;
        let max_w = (right - T_RIGHT_GAP * scale - title_x).max(1.0);
        labels.push(ProseLabel {
            text: e.title.to_owned(),
            x: title_x,
            y: row_top + (T_EVENT_H * scale - m.line_height(FontRole::Body)) * 0.5,
            role: FontRole::Body,
            color: e.title_fg,
            weight: None,
            max_w,
        });
    }

    // Foot band: divider, legend, summary.
    let foot_top = panel.y + panel.h - T_FOOT_H * scale;
    quads.push(ChromeQuad::fill(
        PxRect {
            x: panel.x,
            y: foot_top,
            w: panel.w,
            h: scale.max(1.0),
        },
        theme.border,
    ));
    let legend_y = foot_top + 8.0 * scale;
    let mut lx = cx;
    for (label, color) in [
        ("you 4", theme.accent),
        ("agent 0", theme.diff_hunk),
        ("system 1", theme.fg_muted),
    ] {
        let w = m.width(label, FontRole::Caption, None);
        t_row(
            &mut labels,
            &mut m,
            label,
            FontRole::Caption,
            color,
            lx,
            legend_y,
            20.0,
            scale,
        );
        lx += w + 12.0 * scale;
    }
    t_row(
        &mut labels,
        &mut m,
        "Session - 3m 20s - 5 events - main",
        FontRole::Caption,
        theme.fg_muted,
        cx,
        legend_y + 22.0 * scale,
        20.0,
        scale,
    );

    CaptureTimeline {
        panel,
        quads,
        labels,
    }
}

/// Push a left-anchored label vertically centered in a `row_h` row.
#[allow(clippy::too_many_arguments, reason = "one focused example helper")]
fn t_row(
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

/// Push a right-anchored label ending at `right`; returns its left edge.
#[allow(clippy::too_many_arguments, reason = "one focused example helper")]
fn t_right(
    labels: &mut Vec<ProseLabel>,
    m: &mut TextMeasure,
    text: &str,
    role: FontRole,
    color: Srgb,
    right: f32,
    top: f32,
    row_h: f32,
    scale: f32,
) -> f32 {
    let x = right - m.width(text, role, None);
    t_row(labels, m, text, role, color, x, top, row_h, scale);
    x
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

/// A few plausible terminal lines behind the dock, so the capture shows the dock over a
/// live pane (not a blank one).
fn terminal_rows(theme: &Theme) -> Vec<Vec<GridCell>> {
    let lines = [
        "~/skelly main > nvim src/session/timeline.rs",
        "-- session timeline + rewind --",
        "impl Timeline {",
        "    pub fn record(&mut self, event: SessionEvent) {",
    ];
    lines
        .iter()
        .map(|line| {
            line.chars()
                .map(|c| ui_cell(c, theme.fg_secondary))
                .collect()
        })
        .collect()
}
