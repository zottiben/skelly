//! Headless proof of the live terminal: spawn a real shell, run commands, and
//! render the resulting grid to a PNG - no window or screen-recording needed.
//! Run: `cargo run -p skelly --example session_capture -- out.png`.

use std::thread::sleep;
use std::time::{Duration, Instant};

use skelly_config::Appearance;
use skelly_term::Terminal;

fn main() {
    let path = std::env::args()
        .nth(1)
        .unwrap_or_else(|| "skelly-session.png".to_owned());
    let (cols, rows) = (80_u16, 24_u16);
    let (width, height) = (960_u32, 600_u32);

    let mut term = Terminal::spawn(cols, rows, || {}).expect("spawn shell");
    // Let the shell print its first prompt before we type.
    sleep(Duration::from_millis(700));

    // The adjacent quotes concatenate only when the shell *executes* the command,
    // so the marker appears in the output but not the echoed input line.
    term.write(b"echo 'skelly-M1C''-live'; echo; ls -1\n");

    let marker = "skelly-M1C-live";
    let deadline = Instant::now() + Duration::from_secs(10);
    while Instant::now() < deadline && !snapshot_has(&term, marker) {
        sleep(Duration::from_millis(50));
    }
    // A beat for the rest of the output.
    sleep(Duration::from_millis(300));

    let lines = term.snapshot();
    println!("--- captured grid ---");
    for line in &lines {
        println!("{line}");
    }
    println!("---------------------");

    let content = lines.join("\n");
    let rgba = skelly_render::capture_rgba(&Appearance::default(), width, height, 2.0, &content);

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
