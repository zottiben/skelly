//! Headless proof of the live terminal: spawn a real shell, run commands, and
//! render the resulting grid to a PNG - no window or screen-recording needed.
//! Run: `cargo run -p skelly --example session_capture -- out.png`.

use std::thread::sleep;
use std::time::{Duration, Instant};

use skelly_config::Appearance;
use skelly_render::{AnsiPalette, Srgb};
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

    // `ls -G` colorizes to a tty (the PTY is one), so the output shows real ANSI
    // colors. The adjacent quotes concatenate only when the shell *executes* the
    // command, so the marker appears in the output but not the echoed input.
    term.write(b"printf 'COLORS''_LIVE\\n'; ls -G -1\n");
    wait_until(&term, Duration::from_secs(10), |t| {
        snapshot_has(t, "COLORS_LIVE")
    });
    sleep(Duration::from_millis(500));

    println!("--- captured grid ---");
    for line in &term.snapshot() {
        println!("{line}");
    }
    println!("---------------------");

    let appearance = Appearance::default();
    let palette = AnsiPalette::resolve(&appearance.theme);
    let rows: Vec<Vec<(char, Srgb)>> = term
        .cells()
        .iter()
        .map(|row| {
            row.iter()
                .map(|cell| (cell.c, resolve_fg(cell.fg, &palette)))
                .collect()
        })
        .collect();
    let rgba = skelly_render::capture_cells_rgba(&appearance, width, height, 2.0, &rows);

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

fn resolve_fg(color: CellColor, palette: &AnsiPalette) -> Srgb {
    match color {
        CellColor::Default => palette.default_fg(),
        CellColor::Indexed(index) => palette.indexed(index),
        CellColor::Rgb(r, g, b) => Srgb { r, g, b },
    }
}
