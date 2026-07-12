//! Headless proof of the M4 git diff dock: render a live-ish pane workspace with the
//! per-repo git diff dock docked on the right (its file list, the selected file's unified
//! diff with add/del/hunk backgrounds, and the left-edge divider) to a PNG, with no
//! window or screen recording needed.
//!
//! The live binary drives the dock from its real `gitdock` module over a
//! `skelly_session` repo; examples cannot import the binary crate, so this hand-builds a
//! representative grid (as `settings_capture` does) purely to exercise the `gitdock_quads`
//! render path. An optional second arg picks the theme (`ossein-light`).
//! Run: `cargo run -p skelly --example git_dock_capture -- git-dock.png [theme]`.
#![allow(
    clippy::cast_precision_loss,
    clippy::cast_possible_truncation,
    clippy::cast_sign_loss,
    reason = "example: surface dimensions and grid sizes are small, non-negative values"
)]

use skelly_config::Appearance;
use skelly_render::{
    measure_cell, CaptureGitDock, CapturePane, Chrome, GridCell, PxRect, Srgb, Theme,
};

/// Logical dock width - mirrors the binary's `GIT_DOCK_WIDTH` (420, the guide's default).
const DOCK_WIDTH: f32 = 420.0;
/// Logical inset of the dock text - mirrors the binary's `GIT_DOCK_PAD`.
const DOCK_PAD: f32 = 14.0;
/// Logical window margin - mirrors the binary's `WINDOW_PAD`.
const WINDOW_PAD: f32 = 12.0;
/// Logical inner pane inset - mirrors the binary's `PANE_INSET`.
const PANE_INSET: f32 = 6.0;

fn main() {
    let path = std::env::args()
        .nth(1)
        .unwrap_or_else(|| "skelly-git-dock.png".to_owned());
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
    };

    let dock = build_dock(width, height, cell_w, cell_h, sc, &theme);
    let rgba = skelly_render::capture_panes_rgba(
        &appearance,
        width,
        height,
        scale,
        &[pane],
        &Chrome {
            git_dock: Some(&dock),
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

/// A representative git diff dock, mirroring the binary's `gitdock` layout: a status bar,
/// a changed-file list (the selected file highlighted), and the selected file's unified
/// diff with hunk headers and add/del line backgrounds.
fn build_dock(
    width: u32,
    height: u32,
    cell_w: f32,
    cell_h: f32,
    scale: f32,
    theme: &Theme,
) -> CaptureGitDock {
    let pad = DOCK_PAD * scale;
    let dock_w = DOCK_WIDTH * scale;
    let panel_x = width as f32 - dock_w;
    let cols = ((dock_w - 2.0 * pad) / cell_w).floor().max(1.0) as usize;
    let rows_n = ((height as f32 - 2.0 * pad) / cell_h).floor().max(1.0) as usize;

    let mut rows: Vec<Vec<GridCell>> = (0..rows_n)
        .map(|_| blank_row(cols, theme.fg_muted))
        .collect();

    // Status bar: branch (diff.hunk), ahead/behind (muted), totals (diff.add/del), esc.
    write(&mut rows[0], 0, "main", theme.diff_hunk);
    write(&mut rows[0], 6, "ahead 2 behind 1", theme.fg_muted);
    write(&mut rows[0], 24, "+42", theme.diff_add);
    write(&mut rows[0], 28, "-11", theme.diff_del);
    write_right(&mut rows[0], cols, "esc", theme.fg_muted);

    // File-list section label (+ key hint) and the changed files. Each row: a stage
    // checkbox, the status letter, the path, and its counts (staged.rs is staged: [x]).
    write(&mut rows[2], 0, "CHANGED - 3", theme.fg_muted);
    write_right(&mut rows[2], cols, "space stage  a all", theme.fg_muted);
    let files = [
        ('M', "src/pane/tree.rs", "+42", "-11", theme.diff_hunk, true),
        (
            'A',
            "src/session/timeline.rs",
            "+80",
            "-0",
            theme.diff_add,
            true,
        ),
        ('D', "old/legacy.rs", "+0", "-34", theme.diff_del, false),
    ];
    let file_start = 3usize;
    let selected = 0usize;
    for (i, (letter, path, add, del, letter_fg, staged)) in files.iter().enumerate() {
        let row = file_start + i;
        let name_fg = if i == selected {
            theme.fg_primary
        } else {
            theme.fg_secondary
        };
        write(&mut rows[row], 0, "[", theme.fg_muted);
        write(
            &mut rows[row],
            1,
            if *staged { "x" } else { " " },
            theme.diff_add,
        );
        write(&mut rows[row], 2, "]", theme.fg_muted);
        write(&mut rows[row], 4, &letter.to_string(), *letter_fg);
        write(&mut rows[row], 6, path, name_fg);
        write_pair(&mut rows[row], cols, add, del, theme);
    }

    // Diff header (the selected file) + its unified diff.
    let diff_header = file_start + files.len() + 1;
    write(
        &mut rows[diff_header],
        0,
        "src/pane/tree.rs",
        theme.fg_secondary,
    );
    write_pair(&mut rows[diff_header], cols, "+42", "-11", theme);

    // Reserve the bottom band for the commit box; the diff fills the rows above it.
    let commit_rows = 3usize;
    let content_rows = rows_n.saturating_sub(commit_rows);
    let mut kinds = DiffRows::default();
    write_diff_rows(&mut rows, diff_header + 1, content_rows, theme, &mut kinds);

    // The commit box: a divider, a message input with a caret, and a status line.
    let caret = Some(write_commit_band(&mut rows, content_rows, cols, theme));

    CaptureGitDock {
        panel: PxRect {
            x: panel_x,
            y: 0.0,
            w: dock_w,
            h: height as f32,
        },
        text_origin: (panel_x + pad, pad),
        rows,
        selected_file_row: Some(file_start + selected),
        add_rows: kinds.add,
        del_rows: kinds.del,
        hunk_rows: kinds.hunk,
        caret,
    }
}

/// A representative commit box at the foot (divider, `> message` input, status line),
/// returning the caret cell. Mirrors the binary's `gitdock::write_commit_band`.
fn write_commit_band(
    rows: &mut [Vec<GridCell>],
    top: usize,
    cols: usize,
    theme: &Theme,
) -> (usize, usize) {
    if let Some(row) = rows.get_mut(top) {
        write(row, 0, &"-".repeat(cols), theme.border_strong);
    }
    let message = "feat: rewindable session timeline";
    let input_row = top + 1;
    if let Some(row) = rows.get_mut(input_row) {
        write(row, 0, "> ", theme.accent);
        write(row, 2, message, theme.fg_primary);
    }
    if let Some(row) = rows.get_mut(top + 2) {
        write(row, 0, "2 staged", theme.fg_muted);
        write_right(row, cols, "enter commit  esc back", theme.fg_muted);
    }
    (2 + message.chars().count(), input_row)
}

/// The add/del/hunk grid rows a diff render produced (for the background quads).
#[derive(Default)]
struct DiffRows {
    add: Vec<usize>,
    del: Vec<usize>,
    hunk: Vec<usize>,
}

/// Render a representative unified diff into `rows[body..]`, recording the add/del/hunk
/// grid rows into `kinds`.
fn write_diff_rows(
    rows: &mut [Vec<GridCell>],
    body: usize,
    rows_n: usize,
    theme: &Theme,
    kinds: &mut DiffRows,
) {
    let lines: &[(char, &str, &str)] = &[
        ('@', "@@ -18,7 +18,9 @@ impl PaneTree", ""),
        (' ', "18", "fn split(&mut self, dir: Dir) {"),
        (' ', "19", "    let node = self.focused();"),
        ('-', "20", "    node.grow(dir);"),
        ('+', "20", "    if self.count() >= 8 { return; }"),
        ('+', "21", "    node.grow(dir);"),
        (' ', "22", "    self.rebalance();"),
        (' ', "23", "}"),
    ];
    for (i, (kind, gutter, text)) in lines.iter().enumerate() {
        let row = body + i;
        if row >= rows_n {
            break;
        }
        match kind {
            '@' => {
                kinds.hunk.push(row);
                write(&mut rows[row], 0, gutter, theme.diff_hunk);
            }
            '+' => {
                kinds.add.push(row);
                write_diff_line(&mut rows[row], gutter, '+', text, theme.diff_add, theme);
            }
            '-' => {
                kinds.del.push(row);
                write_diff_line(&mut rows[row], gutter, '-', text, theme.diff_del, theme);
            }
            _ => write_diff_line(&mut rows[row], gutter, ' ', text, theme.fg_secondary, theme),
        }
    }
}

/// A right-aligned `+add -del` count pair (add in `diff.add`, del in `diff.del`).
fn write_pair(row: &mut [GridCell], cols: usize, add: &str, del: &str, theme: &Theme) {
    let del_start = cols.saturating_sub(1 + del.chars().count());
    write(row, del_start, del, theme.diff_del);
    let add_start = del_start.saturating_sub(1 + add.chars().count());
    write(row, add_start, add, theme.diff_add);
}

/// A diff body line: a right-aligned line-number gutter, the `+`/`-`/` ` sign, and the
/// code text, all in `fg` (context uses a muted gutter).
fn write_diff_line(
    row: &mut [GridCell],
    gutter: &str,
    sign: char,
    text: &str,
    fg: Srgb,
    theme: &Theme,
) {
    let gutter_fg = if sign == ' ' { theme.fg_muted } else { fg };
    let g = format!("{gutter:>4}");
    write(row, 0, &g, gutter_fg);
    write(row, 5, &sign.to_string(), fg);
    write(row, 7, text, fg);
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

/// A few plausible terminal lines behind the dock, so the capture shows the dock over a
/// live pane (not a blank one). Colored with the UI foreground for simplicity.
fn terminal_rows(theme: &Theme) -> Vec<Vec<GridCell>> {
    let lines = [
        "~/skelly main > git status",
        "On branch main",
        "Changes not staged for commit:",
        "  modified: src/pane/tree.rs",
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
