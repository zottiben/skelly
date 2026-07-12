//! Headless proof of the M3 pane workspace: build a real two-pane split with the
//! `skelly-pane` tree, spawn a live shell in each pane, and render the tiled result
//! (the left sidebar / tab list, pane dividers, the focused-pane ring, and a cursor
//! only in the focused pane, with a command palette on top) to a PNG, with no window
//! or screen-recording needed.
//! Run: `cargo run -p skelly --example pane_capture -- panes.png`.
#![allow(
    clippy::cast_precision_loss,
    clippy::cast_possible_truncation,
    clippy::cast_sign_loss,
    reason = "example: surface dimensions and grid sizes are small, non-negative values"
)]

use std::thread::sleep;
use std::time::{Duration, Instant};

use skelly_config::Appearance;
use skelly_pane::{Dir, PaneTree, Rect};
use skelly_render::{
    measure_cell, AnsiPalette, CaptureOverlay, CapturePane, CaptureSidebar, GridCell, PxRect, Srgb,
    Theme,
};
use skelly_term::{CellAttrs, CellColor, TermCell, Terminal};

/// Logical padding around the whole pane area - mirrors the binary's `WINDOW_PAD`.
const WINDOW_PAD: f32 = 12.0;
/// Logical inset inside each pane - mirrors the binary's `PANE_INSET`.
const PANE_INSET: f32 = 6.0;
/// Logical sidebar width - mirrors the config default (`[sidebar] width = 240`).
const SIDEBAR_WIDTH: f32 = 240.0;
/// Logical inset of the sidebar text - mirrors the binary's `SIDEBAR_PAD`.
const SIDEBAR_PAD: f32 = 12.0;
/// Logical width of the slim icon rail - mirrors the binary's `RAIL_WIDTH`.
const RAIL_WIDTH: f32 = 56.0;
/// Logical horizontal inset of the rail's centered content - mirrors `RAIL_PAD`.
const RAIL_PAD: f32 = 6.0;

fn main() {
    let path = std::env::args()
        .nth(1)
        .unwrap_or_else(|| "skelly-panes.png".to_owned());
    let (width, height, scale) = (1360_u32, 680_u32, 2.0_f64);

    // Use an installed Nerd Font so the configured-font path is exercised. An optional
    // second arg picks the theme (e.g. `ossein-light`), exercising live-theming tokens.
    let theme = std::env::args()
        .nth(2)
        .unwrap_or_else(|| "ossein-dark".to_owned());
    let appearance = Appearance {
        font_family: "SauceCodePro Nerd Font Mono".to_owned(),
        theme,
        ..Appearance::default()
    };
    // An optional third arg picks the sidebar mode: `rail` = the slim 56px icon rail,
    // anything else (default) = the full-width panel.
    let rail = std::env::args().nth(3).as_deref() == Some("rail");
    // `overflow` renders a many-tab full panel scrolled so the active tab stays in view,
    // exercising the tab-list windowing + the ↑/↓ overflow indicators (design §12).
    let overflow = std::env::args().nth(3).as_deref() == Some("overflow");

    let (cell_w, cell_h) = measure_cell(&appearance, scale);
    let sc = scale as f32;
    let pad = WINDOW_PAD * sc;
    let inset = PANE_INSET * sc;
    // The pane viewport starts to the right of the sidebar, as the binary insets it.
    let sidebar_w = if rail { RAIL_WIDTH } else { SIDEBAR_WIDTH } * sc;
    let viewport = Rect::new(
        sidebar_w + pad,
        pad,
        width as f32 - sidebar_w - 2.0 * pad,
        height as f32 - 2.0 * pad,
    );

    // Two panes, side by side; focus lands on the new (right) pane, matching the
    // binary's split behavior.
    let mut tree = PaneTree::new();
    let left = tree.focused();
    tree.split(Dir::Right).expect("under the pane cap");
    let focused = tree.focused();
    let layout = tree.layout(viewport);

    let palette = AnsiPalette::resolve(&appearance.theme);
    let mut panes = Vec::new();
    for (id, rect) in &layout {
        let cols = ((rect.w - 2.0 * inset) / cell_w).floor().max(2.0) as u16;
        let rows = ((rect.h - 2.0 * inset) / cell_h).floor().max(1.0) as u16;

        let mut term = Terminal::spawn(cols, rows, || {}).expect("spawn shell");
        wait_until(&term, Duration::from_secs(6), |t| {
            t.snapshot().iter().any(|line| !line.is_empty())
        });
        sleep(Duration::from_millis(300));

        // Distinct, colored content per pane so the split reads clearly.
        let cmd = if *id == left {
            "clear; \
             printf '\\033[35m\\033[0m left pane \\342\\200\\224 editor\\n\\n'; \
             printf '\\033[36m  1\\033[0m fn main() {\\n'; \
             printf '\\033[36m  2\\033[0m     println!(\"skelly\");\\n'; \
             printf '\\033[36m  3\\033[0m }\\n'; \
             printf 'PANE_READY\\n'\n"
        } else {
            "clear; \
             printf '\\033[32m\\033[0m right pane \\342\\200\\224 shell\\n\\n'; \
             printf '$ \\033[1mgit status\\033[0m\\n'; \
             printf '\\033[32m  modified:\\033[0m src/pane.rs\\n'; \
             printf '\\033[33m  branch:\\033[0m feat/m3\\n'; \
             printf 'PANE_READY\\n'\n"
        };
        term.write(cmd.as_bytes());
        wait_until(&term, Duration::from_secs(15), |t| {
            t.snapshot().iter().any(|line| line.contains("PANE_READY"))
        });
        sleep(Duration::from_millis(300));

        let grid: Vec<Vec<GridCell>> = term
            .cells()
            .iter()
            .map(|row| row.iter().map(|c| resolve_cell(c, &palette)).collect())
            .collect();
        panes.push(CapturePane {
            rect: PxRect {
                x: rect.x,
                y: rect.y,
                w: rect.w,
                h: rect.h,
            },
            origin: (rect.x + inset, rect.y + inset),
            rows: grid,
            cursor: term.cursor(),
            focused: *id == focused,
        });
    }

    // The left sidebar (a two-tab list, tab 1 active) + an overlay over the panes,
    // verifying the sidebar chrome and the overlay compositing together. The overlay is
    // the command palette by default, or the "running job" confirm modal for `confirm`.
    let theme = Theme::resolve(&appearance.theme);
    let sidebar = sidebar_panel(height, cell_w, sidebar_w, sc, rail, overflow, &theme);
    let overlay = if std::env::args().nth(3).as_deref() == Some("confirm") {
        confirm_overlay(width, height, cell_w, cell_h, sc, &theme)
    } else {
        palette_overlay(width, height, cell_w, cell_h, sc, &theme)
    };
    let rgba = skelly_render::capture_panes_rgba(
        &appearance,
        width,
        height,
        scale,
        &panes,
        &skelly_render::Chrome {
            sidebar: Some(&sidebar),
            overlay: Some(&overlay),
            ..Default::default()
        },
    );

    write_png(&path, width, height, &rgba);
    println!("wrote {path} ({} panes)", panes.len());
}

/// Encode tight RGBA8 bytes to a PNG at `path`.
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

/// Build a representative command-palette overlay (a prompt line, a few command
/// rows, and a footer) centered on the surface - to exercise the overlay compositing
/// path. The live binary drives this from the real `palette` module.
fn palette_overlay(
    width: u32,
    height: u32,
    cell_w: f32,
    cell_h: f32,
    scale: f32,
    theme: &Theme,
) -> CaptureOverlay {
    let cols = 44_usize; // fits the footer (the widest line), matching the real palette
    let rows = vec![
        prompt_row("> zoom", cols, theme),
        ui_row("  2 results", cols, theme.fg_muted),
        command_row("Zoom / unzoom pane", "opt Z", cols, theme),
        command_row("Even out splits", "opt =", cols, theme),
        ui_row("", cols, theme.fg_muted),
        ui_row(
            "  up/down navigate    enter run    esc close",
            cols,
            theme.fg_muted,
        ),
    ];
    let pad = 12.0 * scale;
    let panel_w = cols as f32 * cell_w + 2.0 * pad;
    let panel_h = rows.len() as f32 * cell_h + 2.0 * pad;
    let x = ((width as f32 - panel_w) / 2.0).max(0.0);
    let y = height as f32 * 0.16;
    CaptureOverlay {
        panel: PxRect {
            x,
            y,
            w: panel_w,
            h: panel_h,
        },
        text_origin: (x + pad, y + pad),
        rows,
        selected_row: Some(2),
        caret: Some(("> zoom".chars().count(), 0)),
    }
}

/// Build the "running job" confirm modal (design §12) - a centered panel warning that a
/// close would kill a foreground job. Mirrors the binary's `confirm` module view. The live
/// binary drives this from the real module.
fn confirm_overlay(
    width: u32,
    height: u32,
    cell_w: f32,
    cell_h: f32,
    scale: f32,
    theme: &Theme,
) -> CaptureOverlay {
    // Title: the process name (accent) inside straight quotes (primary).
    let mut title = vec![ui_cell('"', theme.fg_primary)];
    title.extend("vim".chars().map(|c| ui_cell(c, theme.accent)));
    title.extend(
        "\" is still running"
            .chars()
            .map(|c| ui_cell(c, theme.fg_primary)),
    );

    let lines = [
        title,
        Vec::new(),
        "Close this pane and end it?"
            .chars()
            .map(|c| ui_cell(c, theme.fg_primary))
            .collect(),
        Vec::new(),
        "\u{21b5} close   esc cancel"
            .chars()
            .map(|c| ui_cell(c, theme.fg_muted))
            .collect(),
    ];
    let widest = lines.iter().map(Vec::len).max().unwrap_or(0);
    let cols = (widest + 4).max(30);
    let rows: Vec<Vec<GridCell>> = lines
        .into_iter()
        .map(|mut line| {
            let mut row = vec![ui_cell(' ', theme.fg_muted), ui_cell(' ', theme.fg_muted)];
            row.append(&mut line);
            while row.len() < cols {
                row.push(ui_cell(' ', theme.fg_muted));
            }
            row
        })
        .collect();

    let pad = 12.0 * scale;
    let panel_w = cols as f32 * cell_w + 2.0 * pad;
    let panel_h = rows.len() as f32 * cell_h + 2.0 * pad;
    let x = ((width as f32 - panel_w) / 2.0).max(0.0);
    let y = height as f32 * 0.16;
    CaptureOverlay {
        panel: PxRect {
            x,
            y,
            w: panel_w,
            h: panel_h,
        },
        text_origin: (x + pad, y + pad),
        rows,
        selected_row: None,
        caret: None,
    }
}

/// Build a representative left sidebar (a two-tab list with tab 1 active and a new-tab
/// action) - mirroring the binary's `sidebar` module layout so the capture verifies the
/// tab-list chrome. `rail` picks the slim 56px icon rail (centered tab numbers) over the
/// full panel. The live binary drives this from the real module.
fn sidebar_panel(
    height: u32,
    cell_w: f32,
    sidebar_w: f32,
    scale: f32,
    rail: bool,
    overflow: bool,
    theme: &Theme,
) -> CaptureSidebar {
    let pad = (if rail { RAIL_PAD } else { SIDEBAR_PAD }) * scale;
    let cols = ((sidebar_w - 2.0 * pad) / cell_w).max(1.0) as usize;
    let indent = "  ";
    let (rows, active_row) = if overflow {
        // Ten tabs windowed into a 6-tab list with the active tab (Tab 9) scrolled to the
        // bottom row: the ↑/↓ spacers carry the hidden-tab counts. Mirrors what the binary's
        // `sidebar` module produces for count=10, active=8, a 6-row window.
        (
            vec![
                ui_row(&format!("{indent}skelly"), cols, theme.fg_secondary), // header
                ui_row(&format!("{indent}↑ 3 more"), cols, theme.fg_muted),   // more above
                ui_row(&format!("{indent}Tab 4"), cols, theme.fg_secondary),
                ui_row(&format!("{indent}Tab 5"), cols, theme.fg_secondary),
                ui_row(&format!("{indent}Tab 6"), cols, theme.fg_secondary),
                ui_row(&format!("{indent}Tab 7"), cols, theme.fg_secondary),
                ui_row(&format!("{indent}Tab 8"), cols, theme.fg_secondary),
                ui_row(&format!("{indent}Tab 9"), cols, theme.fg_primary), // active
                ui_row(&format!("{indent}↓ 1 more"), cols, theme.fg_muted), // more below
                ui_row(&format!("{indent}+ New tab"), cols, theme.fg_muted),
            ],
            Some(7),
        )
    } else if rail {
        (
            vec![
                centered_row("sk", cols, theme.fg_secondary), // brand mark
                ui_row("", cols, theme.fg_muted),             // spacer
                centered_row("1", cols, theme.fg_primary),    // active
                centered_row("2", cols, theme.fg_secondary),
                ui_row("", cols, theme.fg_muted), // spacer
                centered_row("+", cols, theme.fg_muted),
            ],
            Some(2),
        )
    } else {
        (
            vec![
                ui_row(&format!("{indent}skelly"), cols, theme.fg_secondary), // header
                ui_row("", cols, theme.fg_muted),                             // spacer
                ui_row(&format!("{indent}Tab 1"), cols, theme.fg_primary),    // active
                ui_row(&format!("{indent}Tab 2"), cols, theme.fg_secondary),
                ui_row("", cols, theme.fg_muted), // spacer
                ui_row(&format!("{indent}+ New tab"), cols, theme.fg_muted),
            ],
            Some(2),
        )
    };
    CaptureSidebar {
        panel: PxRect {
            x: 0.0,
            y: 0.0,
            w: sidebar_w,
            h: height as f32,
        },
        text_origin: (pad, pad),
        rows,
        // The active tab's grid row (index 0 at HEADER_ROWS=2, or Tab 9 scrolled to row 7).
        active_row,
    }
}

/// A `text` centered within `cols` cells in `fg` - mirrors the `sidebar` module's rail
/// centering.
fn centered_row(text: &str, cols: usize, fg: Srgb) -> Vec<GridCell> {
    let left = cols.saturating_sub(text.chars().count()) / 2;
    let mut row: Vec<GridCell> = (0..left).map(|_| ui_cell(' ', fg)).collect();
    row.extend(text.chars().map(|c| ui_cell(c, fg)));
    row.truncate(cols);
    while row.len() < cols {
        row.push(ui_cell(' ', fg));
    }
    row
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

fn ui_row(text: &str, cols: usize, fg: Srgb) -> Vec<GridCell> {
    let mut row: Vec<GridCell> = text.chars().map(|c| ui_cell(c, fg)).collect();
    row.truncate(cols);
    while row.len() < cols {
        row.push(ui_cell(' ', fg));
    }
    row
}

fn prompt_row(text: &str, cols: usize, theme: &Theme) -> Vec<GridCell> {
    let mut row = vec![ui_cell('>', theme.accent)];
    row.extend(text.chars().skip(1).map(|c| ui_cell(c, theme.fg_primary)));
    row.truncate(cols);
    while row.len() < cols {
        row.push(ui_cell(' ', theme.fg_muted));
    }
    row
}

fn command_row(label: &str, hint: &str, cols: usize, theme: &Theme) -> Vec<GridCell> {
    let mut row: Vec<GridCell> = "  "
        .chars()
        .chain(label.chars())
        .map(|c| ui_cell(c, theme.fg_primary))
        .collect();
    let hint_len = hint.chars().count();
    let hint_start = cols.saturating_sub(hint_len + 1);
    while row.len() < hint_start {
        row.push(ui_cell(' ', theme.fg_primary));
    }
    row.extend(hint.chars().map(|c| ui_cell(c, theme.fg_muted)));
    row.truncate(cols);
    while row.len() < cols {
        row.push(ui_cell(' ', theme.fg_muted));
    }
    row
}

fn wait_until<F: Fn(&Terminal) -> bool>(term: &Terminal, timeout: Duration, ready: F) {
    let deadline = Instant::now() + timeout;
    while Instant::now() < deadline && !ready(term) {
        sleep(Duration::from_millis(50));
    }
}

// Mirrors `resolve_cell` in the binary (examples cannot import the binary crate):
// fold dim + reverse video into concrete colors, pass bold/italic/underline through.
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
