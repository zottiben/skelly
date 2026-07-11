//! Headless offscreen rendering, for visual and (future) golden-image verification
//! without a window or OS screen-recording permission. Exercises the same
//! [`TextLayer`](crate::TextLayer) the windowed renderer uses.

#![allow(
    clippy::cast_possible_truncation,
    reason = "capture: dimensions are small; u32<->usize casts are exact on 64-bit targets"
)]

use skelly_config::Appearance;

use crate::cells::{
    grid_quads, overlay_quads as build_overlay_quads, push_outline, sidebar_quads, Quad, QuadLayer,
};
use crate::text::{measure_cell, PaneTextInput, TextLayer};
use crate::theme::{Rgba, Theme};
use crate::{GridCell, OverlayView, PxRect, SidebarView};

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
        Vec::new(),
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
        Vec::new(),
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

/// The left sidebar for [`capture_panes_rgba`], mirroring
/// [`SidebarView`](crate::SidebarView) but owning its rows.
pub struct CaptureSidebar {
    /// The sidebar rectangle (`x = 0`, full height), physical px.
    pub panel: PxRect,
    /// Pixel position of the text grid's cell `(0, 0)` top-left.
    pub text_origin: (f32, f32),
    /// The sidebar text as a monospace grid (UI-token colored).
    pub rows: Vec<Vec<GridCell>>,
    /// Grid row of the active tab to highlight, if any.
    pub active_row: Option<usize>,
}

/// A command-palette overlay for [`capture_panes_rgba`], mirroring
/// [`OverlayView`](crate::OverlayView) but owning its rows.
pub struct CaptureOverlay {
    /// The centered panel rectangle, physical px.
    pub panel: PxRect,
    /// Pixel position of the text grid's cell `(0, 0)` top-left.
    pub text_origin: (f32, f32),
    /// The overlay text as a monospace grid (UI-token colored).
    pub rows: Vec<Vec<GridCell>>,
    /// The selected row to highlight, if any.
    pub selected_row: Option<usize>,
    /// The input caret's `(column, row)` cell, if any.
    pub caret: Option<(usize, usize)>,
}

/// Render a tiled set of `panes` (and an optional `overlay`) headlessly - the exact
/// path the windowed [`Renderer`](crate::Renderer) uses (dividers, focus ring,
/// per-pane cursor, and the palette overlay on top) - to an offscreen `width` x
/// `height` sRGB target, returning tight RGBA8 bytes. The verification twin of the
/// live workspace.
///
/// # Panics
/// Panics if no GPU adapter/device is available or the readback fails.
#[must_use]
#[allow(
    clippy::cast_possible_truncation,
    reason = "DPI scale precision loss is irrelevant for the overlay stroke width"
)]
pub fn capture_panes_rgba(
    appearance: &Appearance,
    width: u32,
    height: u32,
    scale: f64,
    panes: &[CapturePane],
    overlay: Option<&CaptureOverlay>,
    sidebar: Option<&CaptureSidebar>,
) -> Vec<u8> {
    let theme = Theme::resolve(&appearance.theme);
    let (cell_w, cell_h) = measure_cell(appearance, scale);
    let sidebar_scene = sidebar.map(|sb| {
        let view = SidebarView {
            panel: sb.panel,
            text_origin: sb.text_origin,
            rows: &sb.rows,
            active_row: sb.active_row,
        };
        SidebarScene {
            quads: sidebar_quads(&view, &theme, cell_w, cell_h, scale as f32),
            rows: &sb.rows,
            left: sb.text_origin.0,
            top: sb.text_origin.1,
            clip: (sb.panel.x, sb.panel.y, sb.panel.w, sb.panel.h),
        }
    });
    let overlay_scene = overlay.map(|ov| {
        let view = OverlayView {
            panel: ov.panel,
            text_origin: ov.text_origin,
            rows: &ov.rows,
            selected_row: ov.selected_row,
            caret: ov.caret,
        };
        OverlayScene {
            quads: build_overlay_quads(&view, &theme, cell_w, cell_h, scale as f32),
            rows: &ov.rows,
            left: ov.text_origin.0,
            top: ov.text_origin.1,
            clip: (ov.panel.x, ov.panel.y, ov.panel.w, ov.panel.h),
        }
    });
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
                        theme.border_strong.to_linear(),
                    );
                }
            }
            text.set_panes(&inputs);
            quads
        },
        // Sidebar first (drawn beneath), then the overlay on top.
        [sidebar_scene, overlay_scene]
            .into_iter()
            .flatten()
            .collect(),
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

/// A prepared chrome layer (sidebar or overlay) to draw over the terminal in the
/// headless capture, mirroring one of the windowed renderer's load-pass pairs.
struct Scene<'a> {
    quads: Vec<Quad>,
    rows: &'a [Vec<GridCell>],
    left: f32,
    top: f32,
    clip: (f32, f32, f32, f32),
}

/// The sidebar chrome scene (passes 3-4 in the windowed renderer).
type SidebarScene<'a> = Scene<'a>;
/// The command-palette overlay scene (passes 5-6).
type OverlayScene<'a> = Scene<'a>;

async fn capture_async<F>(
    appearance: &Appearance,
    width: u32,
    height: u32,
    scale: f64,
    clear: wgpu::Color,
    setup: F,
    chrome: Vec<Scene<'_>>,
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
    quad_layer.draw(&device, &queue, &view, Some(clear));
    text.draw(&device, &queue, &view, width, height)
        .expect("draw text");

    // The chrome load-pass pairs, in the windowed renderer's order: the sidebar
    // (passes 3-4) beneath the overlay (passes 5-6). Each loads over the terminal.
    for scene in chrome {
        let mut chrome_quads = QuadLayer::new(&device, format);
        chrome_quads.set(&device, &queue, width, height, &scene.quads);
        chrome_quads.draw(&device, &queue, &view, None);
        let mut chrome_text =
            TextLayer::new(&device, &queue, format, width, height, scale, appearance);
        chrome_text.set_panes(&[PaneTextInput {
            rows: scene.rows,
            left: scene.left,
            top: scene.top,
            clip: scene.clip,
        }]);
        chrome_text
            .draw(&device, &queue, &view, width, height)
            .expect("draw chrome text");
    }

    read_texture(&device, &queue, &texture, width, height)
}

/// Copy `texture` into a mapped buffer and return tight RGBA8 bytes (row-major, no
/// row padding), respecting the 256-byte copy row alignment.
fn read_texture(
    device: &wgpu::Device,
    queue: &wgpu::Queue,
    texture: &wgpu::Texture,
    width: u32,
    height: u32,
) -> Vec<u8> {
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
            texture,
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
