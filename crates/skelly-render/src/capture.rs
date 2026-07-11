//! Headless offscreen rendering, for visual and (future) golden-image verification
//! without a window or OS screen-recording permission. Exercises the same
//! [`TextLayer`](crate::TextLayer) the windowed renderer uses.

#![allow(
    clippy::cast_possible_truncation,
    reason = "capture: dimensions are small; u32<->usize casts are exact on 64-bit targets"
)]

use skelly_config::Appearance;

use crate::cells::{grid_quads, push_outline, Quad, QuadLayer};
use crate::text::{PaneTextInput, TextLayer};
use crate::theme::{Rgba, Theme};
use crate::{GridCell, PxRect};

/// Render plain `content` in `appearance`'s theme and cell font to an offscreen
/// `width` x `height` sRGB target and return tight RGBA8 bytes (row-major, no row
/// padding). Blocks on GPU work.
///
/// # Panics
/// Panics if no GPU adapter/device is available or the readback fails - this is a
/// verification helper, not a shipping path.
#[must_use]
pub fn capture_rgba(
    appearance: &Appearance,
    width: u32,
    height: u32,
    scale: f64,
    content: &str,
) -> Vec<u8> {
    let clear = color(Theme::resolve(&appearance.theme).bg_base);
    pollster::block_on(capture_async(
        appearance,
        width,
        height,
        scale,
        clear,
        |text| {
            text.set_content(content);
            Vec::new()
        },
    ))
}

/// Like [`capture_rgba`], but renders a colored grid with per-cell backgrounds, a
/// cursor at `cursor` `(column, row)`, and `selection` highlighted cells.
///
/// # Panics
/// Panics if no GPU adapter/device is available or the readback fails.
#[must_use]
pub fn capture_cells_rgba(
    appearance: &Appearance,
    width: u32,
    height: u32,
    scale: f64,
    rows: &[Vec<GridCell>],
    cursor: (usize, usize),
    selection: &[(usize, usize)],
) -> Vec<u8> {
    let theme = Theme::resolve(&appearance.theme);
    pollster::block_on(capture_async(
        appearance,
        width,
        height,
        scale,
        color(theme.bg_base),
        |text| {
            text.set_cells(rows);
            let (cell_w, cell_h, pad) = text.cell_metrics();
            grid_quads(
                cell_w,
                cell_h,
                (pad, pad),
                rows,
                Some(cursor),
                theme.accent,
                selection,
            )
        },
    ))
}

/// One pane for [`capture_panes_rgba`]: its rectangle and cell origin (physical px),
/// its grid, cursor, and whether it is focused.
pub struct CapturePane {
    /// The pane's rectangle on the target (border + text clip).
    pub rect: PxRect,
    /// Pixel position of cell `(0, 0)`'s top-left corner.
    pub origin: (f32, f32),
    /// The pane's cell grid.
    pub rows: Vec<Vec<GridCell>>,
    /// Cursor position `(column, row)` - drawn only when `focused`.
    pub cursor: (usize, usize),
    /// Whether this is the focused pane (accent ring + drawn cursor).
    pub focused: bool,
}

/// Render a tiled set of `panes` headlessly - the exact multi-pane path the windowed
/// [`Renderer::set_panes`](crate::Renderer::set_panes) uses (dividers, focus ring,
/// and per-pane cursor) - to an offscreen `width` x `height` sRGB target, returning
/// tight RGBA8 bytes. The verification twin of the live pane workspace.
///
/// # Panics
/// Panics if no GPU adapter/device is available or the readback fails.
#[must_use]
pub fn capture_panes_rgba(
    appearance: &Appearance,
    width: u32,
    height: u32,
    scale: f64,
    panes: &[CapturePane],
) -> Vec<u8> {
    let theme = Theme::resolve(&appearance.theme);
    pollster::block_on(capture_async(
        appearance,
        width,
        height,
        scale,
        color(theme.bg_base),
        |text| {
            let (cell_w, cell_h, _) = text.cell_metrics();
            let stroke = text.scale();
            let mut quads = Vec::new();
            let mut inputs = Vec::with_capacity(panes.len());
            for pane in panes {
                let cursor = pane.focused.then_some(pane.cursor);
                quads.extend(grid_quads(
                    cell_w,
                    cell_h,
                    pane.origin,
                    &pane.rows,
                    cursor,
                    theme.accent,
                    &[],
                ));
                inputs.push(PaneTextInput {
                    rows: &pane.rows,
                    left: pane.origin.0,
                    top: pane.origin.1,
                    clip: (pane.rect.x, pane.rect.y, pane.rect.w, pane.rect.h),
                });
            }
            if panes.len() > 1 {
                let divider = theme.border.to_linear();
                for pane in panes {
                    push_outline(
                        &mut quads,
                        pane.rect.x,
                        pane.rect.y,
                        pane.rect.w,
                        pane.rect.h,
                        stroke,
                        divider,
                    );
                }
                if let Some(pane) = panes.iter().find(|p| p.focused) {
                    push_outline(
                        &mut quads,
                        pane.rect.x,
                        pane.rect.y,
                        pane.rect.w,
                        pane.rect.h,
                        2.0 * stroke,
                        theme.accent.to_linear(),
                    );
                }
            }
            text.set_panes(&inputs);
            quads
        },
    ))
}

fn color(c: Rgba) -> wgpu::Color {
    wgpu::Color {
        r: c.r,
        g: c.g,
        b: c.b,
        a: c.a,
    }
}

async fn capture_async<F>(
    appearance: &Appearance,
    width: u32,
    height: u32,
    scale: f64,
    clear: wgpu::Color,
    setup: F,
) -> Vec<u8>
where
    F: FnOnce(&mut TextLayer) -> Vec<Quad>,
{
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

    let mut quad_layer = QuadLayer::new(&device, format);
    let mut text = TextLayer::new(&device, &queue, format, width, height, scale, appearance);
    let quads = setup(&mut text);
    quad_layer.set(&device, &queue, width, height, &quads);
    quad_layer.draw(&device, &queue, &view, clear);
    text.draw(&device, &queue, &view, width, height)
        .expect("draw text");

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

    readback.slice(..).map_async(wgpu::MapMode::Read, |_| {});
    device
        .poll(wgpu::PollType::wait_indefinitely())
        .expect("poll");
    let mapped = readback
        .slice(..)
        .get_mapped_range()
        .expect("map readback buffer");

    let row = unpadded as usize;
    let stride = padded as usize;
    let mut rgba = Vec::with_capacity(row * height as usize);
    for y in 0..height as usize {
        let start = y * stride;
        rgba.extend_from_slice(&mapped[start..start + row]);
    }
    drop(mapped);
    readback.unmap();
    rgba
}
