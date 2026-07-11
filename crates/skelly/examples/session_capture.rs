//! Headless proof of the live terminal: spawn a real shell, run commands, and
//! render the resulting grid to a PNG - no window or screen-recording needed.
//! Run: `cargo run -p skelly --example session_capture -- out.png`.

use std::thread::sleep;
use std::time::{Duration, Instant};

use skelly_config::Appearance;
use skelly_render::{AnsiPalette, GridCell, Srgb};
use skelly_term::{CellColor, Terminal};

fn main() {
    let path = std::env::args()
        .nth(1)
        .unwrap_or_else(|| "skelly-session.png".to_owned());
    let (cols, rows) = (80_u16, 24_u16);
    let (width, height) = (960_u32, 600_u32);

    let mut term = Terminal::spawn(cols, rows, || {}).expect("spawn shell");
    // Wait for the shell's first prompt before typing - heavy prompts (p10k etc.)
    // initialize asynchronously, and typing before the prompt is ready races.
    wait_until(&term, Duration::from_secs(6), |t| {
        t.snapshot().iter().any(|line| !line.is_empty())
    });
    sleep(Duration::from_millis(400));

    // Print 40 numbered lines (past the 24-row screen), each with a Nerd Font icon
    // and colored index, ending in the marker. The split quotes make the marker
    // appear only in the executed output.
    let cmd = format!(
        "clear; for i in $(seq 1 40); do printf '{}  \\033[36mline %02d\\033[0m  scrollback demo\\n' \"$i\"; done; printf 'COLORS''_LIVE\\n'\n",
        '\u{f07c}',
    );
    term.write(cmd.as_bytes());
    wait_until(&term, Duration::from_secs(15), |t| {
        snapshot_has(t, "COLORS_LIVE")
    });
    sleep(Duration::from_millis(400));

    // Scroll up into history so the capture shows the scrollback view, not the tail.
    term.scroll_lines(20);
    sleep(Duration::from_millis(100));

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
                .map(|cell| GridCell {
                    c: cell.c,
                    fg: resolve_fg(cell.fg, &palette),
                    bg: resolve_bg(cell.bg, &palette),
                })
                .collect()
        })
        .collect();
    let rgba =
        skelly_render::capture_cells_rgba(&appearance, width, height, 2.0, &rows, term.cursor());

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
