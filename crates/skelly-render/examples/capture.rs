//! Render a sample scene to a PNG via the headless `capture_rgba` helper - a
//! window-free, permission-free visual/golden check of the text pipeline.
//! Run: `cargo run -p skelly-render --example capture -- out.png`.

use skelly_config::Appearance;

const SAMPLE: &str = "skelly\na barebones terminal, built in rust.\n\ntext rendering online: glyphon + cosmic-text on wgpu.";

fn main() {
    let path = std::env::args()
        .nth(1)
        .unwrap_or_else(|| "skelly-capture.png".to_owned());
    let (width, height) = (960_u32, 600_u32);

    let rgba = skelly_render::capture_rgba(&Appearance::default(), width, height, 2.0, SAMPLE);

    let file = std::fs::File::create(&path).expect("create png");
    let mut encoder = png::Encoder::new(std::io::BufWriter::new(file), width, height);
    encoder.set_color(png::ColorType::Rgba);
    encoder.set_depth(png::BitDepth::Eight);
    encoder
        .write_header()
        .expect("png header")
        .write_image_data(&rgba)
        .expect("png data");
    println!("wrote {path} ({width}x{height})");
}
