//! Headless proof of the live terminal: spawn a real shell, run commands, and
//! render the resulting grid to a PNG - no window or screen-recording needed.
//! Run: `cargo run -p skelly --example session_capture -- out.png`.

use std::thread::sleep;
use std::time::{Duration, Instant};

use skelly_config::Appearance;
use skelly_render::{AnsiPalette, GridCell, Srgb};
use skelly_term::{CellAttrs, CellColor, TermCell, Terminal};

fn main() {
    let path = std::env::args()
        .nth(1)
        .unwrap_or_else(|| "skelly-session.png".to_owned());
    let (cols, rows) = (80_u16, 24_u16);
    // Size the surface to fit all 24 rows at the default metrics (14px * 1.2 line *
    // 2.0 scale = ~34px/row): 24 rows need ~854px, so a 600px surface would clip the
    // lower rows (where the attribute showcase lands).
    let (width, height) = (960_u32, 860_u32);

    let mut term = Terminal::spawn(cols, rows, || {}).expect("spawn shell");
    // Wait for the shell's first prompt before typing - heavy prompts (p10k etc.)
    // initialize asynchronously, and typing before the prompt is ready races.
    wait_until(&term, Duration::from_secs(6), |t| {
        t.snapshot().iter().any(|line| !line.is_empty())
    });
    sleep(Duration::from_millis(400));

    // Print 30 numbered lines (past the 24-row screen, so earlier ones roll into
    // scrollback), then an SGR text-attribute showcase (bold / italic / underline /
    // reverse / dim), ending in the marker. Showing the live tail keeps the showcase
    // in view and the multi-line command echo scrolled off. Split quotes make the
    // marker appear only in the executed output.
    let icon = '\u{f07c}';
    let cmd = format!(
        "clear; \
         for i in $(seq 1 30); do printf '{icon}  \\033[36mline %02d\\033[0m  scrollback demo\\n' \"$i\"; done; \
         printf 'plain  \\033[1mbold\\033[0m  \\033[3mitalic\\033[0m  \\033[4munderline\\033[0m  \\033[7mreverse\\033[0m  \\033[2mdim\\033[0m\\n'; \
         printf '\\033[1;4mbold+underline\\033[0m  \\033[3;33myellow italic\\033[0m  \\033[4;36mcyan underline\\033[0m\\n'; \
         printf 'COLORS''_LIVE\\n'\n",
    );
    term.write(cmd.as_bytes());
    wait_until(&term, Duration::from_secs(15), |t| {
        snapshot_has(t, "COLORS_LIVE")
    });
    sleep(Duration::from_millis(400));

    println!("--- captured grid ---");
    for line in &term.snapshot() {
        println!("{line}");
    }
    println!("---------------------");

    // Use an installed Nerd Font so the configured-font path is exercised and the
    // Nerd glyphs render (default "JetBrainsMono Nerd Font" is not installed here).
    let appearance = Appearance {
        font_family: "SauceCodePro Nerd Font Mono".to_owned(),
        ..Appearance::default()
    };
    let palette = AnsiPalette::resolve(&appearance.theme);
    let rows: Vec<Vec<GridCell>> = term
        .cells()
        .iter()
        .map(|row| {
            row.iter()
                .map(|cell| resolve_cell(cell, &palette))
                .collect()
        })
        .collect();
    // A demo selection (rows 2-4) to show the translucent highlight.
    let mut selection = Vec::new();
    for row in 2..=4 {
        for col in 0..30 {
            selection.push((col, row));
        }
    }
    let rgba = skelly_render::capture_cells_rgba(
        &appearance,
        width,
        height,
        2.0,
        &rows,
        term.cursor(),
        &selection,
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

fn snapshot_has(term: &Terminal, marker: &str) -> bool {
    term.snapshot().iter().any(|line| line.contains(marker))
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
