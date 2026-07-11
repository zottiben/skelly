//! Headless capture: render the M1 scene to an offscreen texture and write a PNG.
//!
//! Needs no window or OS screen-recording permission - a deterministic visual and
//! (future) golden-image check that exercises the same [`TextLayer`] the windowed
//! renderer uses. Run: `cargo run -p skelly-render --example capture -- out.png`.

#![allow(
    clippy::cast_possible_truncation,
    reason = "capture tool: dimensions are small window sizes; u32<->usize casts are exact on 64-bit targets"
)]

use skelly_config::Appearance;
use skelly_render::TextLayer;

fn main() {
    let path = std::env::args()
        .nth(1)
        .unwrap_or_else(|| "skelly-capture.png".to_owned());
    let (width, height) = (960_u32, 600_u32);
    pollster::block_on(run(&path, width, height));
    println!("wrote {path} ({width}x{height})");
}

async fn run(path: &str, width: u32, height: u32) {
    let instance = wgpu::Instance::default();
    let adapter = instance
        .request_adapter(&wgpu::RequestAdapterOptions::default())
        .await
        .expect("no GPU adapter");
    let (device, queue) = adapter
        .request_device(&wgpu::DeviceDescriptor {
            label: Some("capture"),
            ..Default::default()
        })
        .await
        .expect("no GPU device");

    // Offscreen sRGB target we can render into and copy out of.
    let format = wgpu::TextureFormat::Rgba8UnormSrgb;
    let texture = device.create_texture(&wgpu::TextureDescriptor {
        label: Some("capture-target"),
        size: wgpu::Extent3d {
            width,
            height,
            depth_or_array_layers: 1,
        },
        mip_level_count: 1,
        sample_count: 1,
        dimension: wgpu::TextureDimension::D2,
        format,
        usage: wgpu::TextureUsages::RENDER_ATTACHMENT | wgpu::TextureUsages::COPY_SRC,
        view_formats: &[],
    });
    let view = texture.create_view(&wgpu::TextureViewDescriptor::default());

    // Same TextLayer the windowed renderer uses -> what we capture is what ships.
    let appearance = Appearance::default();
    let mut text = TextLayer::new(&device, &queue, format, width, height, 2.0, &appearance);
    text.draw(&device, &queue, &view, width, height)
        .expect("draw scene");

    // Copy the texture into a readback buffer, respecting the 256-byte row align.
    let bytes_per_pixel = 4_u32;
    let unpadded = width * bytes_per_pixel;
    let padded =
        unpadded.div_ceil(wgpu::COPY_BYTES_PER_ROW_ALIGNMENT) * wgpu::COPY_BYTES_PER_ROW_ALIGNMENT;
    let readback = device.create_buffer(&wgpu::BufferDescriptor {
        label: Some("capture-readback"),
        size: u64::from(padded) * u64::from(height),
        usage: wgpu::BufferUsages::COPY_DST | wgpu::BufferUsages::MAP_READ,
        mapped_at_creation: false,
    });
    let mut encoder = device.create_command_encoder(&wgpu::CommandEncoderDescriptor {
        label: Some("capture-copy"),
    });
    encoder.copy_texture_to_buffer(
        wgpu::TexelCopyTextureInfo {
            texture: &texture,
            mip_level: 0,
            origin: wgpu::Origin3d::ZERO,
            aspect: wgpu::TextureAspect::All,
        },
        wgpu::TexelCopyBufferInfo {
            buffer: &readback,
            layout: wgpu::TexelCopyBufferLayout {
                offset: 0,
                bytes_per_row: Some(padded),
                rows_per_image: Some(height),
            },
        },
        wgpu::Extent3d {
            width,
            height,
            depth_or_array_layers: 1,
        },
    );
    queue.submit(std::iter::once(encoder.finish()));

    // Map and block until the GPU has finished the copy.
    readback.slice(..).map_async(wgpu::MapMode::Read, |_| {});
    device
        .poll(wgpu::PollType::wait_indefinitely())
        .expect("poll");
    let mapped = readback
        .slice(..)
        .get_mapped_range()
        .expect("map readback buffer");

    // Strip row padding into tight RGBA.
    let row = unpadded as usize;
    let stride = padded as usize;
    let mut rgba = Vec::with_capacity(row * height as usize);
    for y in 0..height as usize {
        let start = y * stride;
        rgba.extend_from_slice(&mapped[start..start + row]);
    }
    drop(mapped);
    readback.unmap();

    let file = std::fs::File::create(path).expect("create png");
    let mut encoder = png::Encoder::new(std::io::BufWriter::new(file), width, height);
    encoder.set_color(png::ColorType::Rgba);
    encoder.set_depth(png::BitDepth::Eight);
    encoder
        .write_header()
        .expect("png header")
        .write_image_data(&rgba)
        .expect("png data");
}
