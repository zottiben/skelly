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
    measure_cell, CapturePane, CaptureTimeline, Chrome, GridCell, PxRect, Srgb, Theme,
};

/// Logical dock width - mirrors the binary's `GIT_DOCK_WIDTH` (420, the guide's default).
const DOCK_WIDTH: f32 = 420.0;
/// Logical inset of the dock text - mirrors the binary's `GIT_DOCK_PAD`.
const DOCK_PAD: f32 = 14.0;
/// Logical window margin - mirrors the binary's `WINDOW_PAD`.
const WINDOW_PAD: f32 = 12.0;
/// Logical inner pane inset - mirrors the binary's `PANE_INSET`.
const PANE_INSET: f32 = 6.0;
/// Column where an event's title begins - mirrors the binary's `TITLE_COL`.
const TITLE_COL: usize = 7;

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
    let (cell_w, cell_h) = measure_cell(&appearance, scale);
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
        focused: true,
        logo: None,
    };

    let dock = build_dock(width, height, cell_w, cell_h, sc, &theme);
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
/// optional right badge (`now` / `view`).
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
fn build_dock(
    width: u32,
    height: u32,
    cell_w: f32,
    cell_h: f32,
    scale: f32,
    theme: &Theme,
) -> CaptureTimeline {
    let pad = DOCK_PAD * scale;
    let dock_w = DOCK_WIDTH * scale;
    let panel_x = width as f32 - dock_w;
    let cols = ((dock_w - 2.0 * pad) / cell_w).floor().max(1.0) as usize;
    let rows_n = ((height as f32 - 2.0 * pad) / cell_h).floor().max(1.0) as usize;

    let mut rows: Vec<Vec<GridCell>> = (0..rows_n)
        .map(|_| blank_row(cols, theme.fg_muted))
        .collect();

    // Status banner: viewing a past commit, plus the `esc` hint.
    write(&mut rows[0], 0, "Viewing 1:30 - 67b010a", theme.accent);
    write_right(&mut rows[0], cols, "esc", theme.fg_muted);

    // Section label + control hint.
    write(&mut rows[2], 0, "TIMELINE - 5 events", theme.fg_muted);
    write_right(&mut rows[2], cols, "up down move", theme.fg_muted);

    // The event list. The selection (index 2) is a past commit, so it is primary + carries
    // the viewing bar; the two newer events are the dimmed "future"; the newest is "now".
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
            badge: Some(("view", theme.accent)),
            title_fg: theme.fg_primary,
        },
        Event {
            time: "2:10",
            actor: "you",
            actor_fg: theme.accent,
            title: "Staged timeline.rs",
            badge: None,
            title_fg: theme.fg_muted,
        },
        Event {
            time: "3:20",
            actor: "you",
            actor_fg: theme.accent,
            title: "git commit - feat: timeline",
            badge: Some(("now", theme.diff_hunk)),
            title_fg: theme.fg_muted,
        },
    ];
    let event_start = 3usize;
    let selected = 2usize;
    for (i, e) in events.iter().enumerate() {
        event_row(&mut rows[event_start + i], cols, e);
    }

    // Foot band: a divider, the actor legend, and the session summary.
    let top = rows_n - 3;
    write(&mut rows[top], 0, &"-".repeat(cols), theme.border_strong);
    write(&mut rows[top + 1], 0, "you 4", theme.accent);
    write(&mut rows[top + 1], 7, "agent 0", theme.diff_hunk);
    write(&mut rows[top + 1], 16, "system 1", theme.fg_muted);
    write(
        &mut rows[top + 2],
        0,
        "Session - 3m 20s - 5 events - main",
        theme.fg_muted,
    );

    CaptureTimeline {
        panel: PxRect {
            x: panel_x,
            y: 0.0,
            w: dock_w,
            h: height as f32,
        },
        text_origin: (panel_x + pad, pad),
        rows,
        selected_row: Some(event_start + selected),
        viewing_row: Some(event_start + selected),
    }
}

/// Write one event row: the time, the title (clipped before the actor tag), a right-anchored
/// actor label, and an optional `now`/`view` badge - mirroring the binary's `event_row`.
fn event_row(row: &mut [GridCell], cols: usize, e: &Event) {
    write(row, 0, &format!("{:>5}", e.time), e.title_fg);
    let actor_start = cols.saturating_sub(e.actor.chars().count() + 1);
    write(row, actor_start, e.actor, e.actor_fg);
    let right = match e.badge {
        Some((text, fg)) => write_before(row, actor_start, text, fg),
        None => actor_start,
    };
    let title_max = right.saturating_sub(TITLE_COL + 1);
    let clipped: String = e.title.chars().take(title_max).collect();
    write(row, TITLE_COL, &clipped, e.title_fg);
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

/// Write `text` so its last cell sits just before `end`, returning its start column.
fn write_before(row: &mut [GridCell], end: usize, text: &str, fg: Srgb) -> usize {
    let start = end.saturating_sub(text.chars().count() + 1);
    write(row, start, text, fg);
    start
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
