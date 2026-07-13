//! Headless offscreen rendering, for visual and (future) golden-image verification
//! without a window or OS screen-recording permission. Exercises the same
//! [`TextLayer`](crate::TextLayer) the windowed renderer uses.

#![allow(
    clippy::cast_possible_truncation,
    reason = "capture: dimensions are small; u32<->usize casts are exact on 64-bit targets"
)]

use skelly_config::Appearance;

use crate::cells::{
    card_quads, chrome_quad, dock_frame_quads, grid_quads, logo_quads, push_outline, scrim_quad,
    settings_frame_quads, Quad, QuadLayer, LOGO_WATERMARK_OPACITY,
};
use crate::prose::{ProseLabel, ProseLayer};
use crate::text::{PaneTextInput, TextLayer};
use crate::theme::{Rgba, Theme};
use crate::{ChromeQuad, GridCell, PxRect};

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
                crate::CursorShape::Block,
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
    /// The cursor's shape (block / bar / underline / hidden), honoring `DECSCUSR`.
    pub cursor_shape: crate::CursorShape,
    /// Whether this is the focused pane (accent ring + drawn cursor).
    pub focused: bool,
    /// The empty-state brand watermark's square bounding box (physical px), when this pane
    /// is a pristine empty-state tab (design §10.2); `None` for an ordinary pane.
    pub logo: Option<PxRect>,
}

/// The left sidebar for [`capture_panes_rgba`], mirroring
/// [`SidebarView`](crate::SidebarView) but owning its display list (proportional chrome).
pub struct CaptureSidebar {
    /// The sidebar rectangle (`x = 0`, full height), physical px.
    pub panel: PxRect,
    /// The decorative quads (surface, active pill + bar, chips, divider), in draw order.
    pub quads: Vec<ChromeQuad>,
    /// The positioned proportional text labels.
    pub labels: Vec<ProseLabel>,
}

/// The git diff dock for [`capture_panes_rgba`], mirroring
/// [`GitDockView`](crate::GitDockView) but owning its display list (proportional chrome).
pub struct CaptureGitDock {
    /// The dock rectangle (right edge, full height), physical px.
    pub panel: PxRect,
    /// The content quads over the dock frame (diff backgrounds, selection/hunk fills, caret).
    pub quads: Vec<ChromeQuad>,
    /// The positioned proportional text labels.
    pub labels: Vec<ProseLabel>,
}

/// The session-timeline dock for [`capture_panes_rgba`], mirroring
/// [`TimelineView`](crate::TimelineView) but owning its rows.
pub struct CaptureTimeline {
    /// The dock rectangle (right edge, full height), physical px.
    pub panel: PxRect,
    /// The content quads over the dock frame (selected fill, viewing bar).
    pub quads: Vec<ChromeQuad>,
    /// The positioned proportional text labels.
    pub labels: Vec<ProseLabel>,
}

/// A command-palette / modal overlay for [`capture_panes_rgba`], mirroring
/// [`OverlayView`](crate::OverlayView) but owning its display list (proportional chrome).
pub struct CaptureOverlay {
    /// The centered panel rectangle, physical px.
    pub panel: PxRect,
    /// The content quads over the card (selected pill, caret).
    pub quads: Vec<ChromeQuad>,
    /// The positioned proportional text labels.
    pub labels: Vec<ProseLabel>,
}

/// The pane-level overlay for [`capture_panes_rgba`], mirroring `Renderer::set_pane_overlays`:
/// a `bg.base` scrim over each exited pane, plus content quads (empty-state chip pills) and
/// proportional labels (the exit message, chip text).
#[derive(Default)]
pub struct PaneOverlay {
    /// The exited-pane rects to scrim, physical px.
    pub scrims: Vec<PxRect>,
    /// Content quads over the scrims (empty-state chip pills).
    pub quads: Vec<ChromeQuad>,
    /// Positioned proportional labels (the exit message, empty-state chip text).
    pub labels: Vec<ProseLabel>,
}

/// The optional chrome layers to composite over the panes in [`capture_panes_rgba`],
/// mirroring the windowed renderer's base-chrome + overlay passes.
#[derive(Default)]
pub struct Chrome<'a> {
    /// The pane-level overlays (exited-pane scrims/messages + empty-state chips), drawn just
    /// above the panes.
    pub pane_overlay: PaneOverlay,
    /// The left sidebar (passes 3-4).
    pub sidebar: Option<&'a CaptureSidebar>,
    /// The git diff dock (passes 5-6).
    pub git_dock: Option<&'a CaptureGitDock>,
    /// The session-timeline dock (right strip; mutually exclusive with `git_dock`).
    pub timeline: Option<&'a CaptureTimeline>,
    /// The command-palette overlay (passes 7-8).
    pub overlay: Option<&'a CaptureOverlay>,
}

/// Render a tiled set of `panes` plus any `chrome` (sidebar, git dock, palette overlay)
/// headlessly - the exact path the windowed [`Renderer`](crate::Renderer) uses (dividers,
/// focus ring, per-pane cursor, and the chrome layers on top) - to an offscreen `width` x
/// `height` sRGB target, returning tight RGBA8 bytes. The verification twin of the live
/// workspace.
///
/// # Panics
/// Panics if no GPU adapter/device is available or the readback fails.
#[must_use]
#[allow(
    clippy::cast_possible_truncation,
    clippy::cast_precision_loss,
    reason = "DPI scale precision loss is irrelevant for stroke width; surface dims are exact in f32"
)]
pub fn capture_panes_rgba(
    appearance: &Appearance,
    width: u32,
    height: u32,
    scale: f64,
    panes: &[CapturePane],
    chrome: &Chrome,
) -> Vec<u8> {
    let theme = Theme::resolve(&appearance.theme);
    // The "shell exited" scrims + messages, drawn just above the panes (before the sidebar).
    let po = &chrome.pane_overlay;
    let pane_overlay_scene =
        (!po.scrims.is_empty() || !po.quads.is_empty() || !po.labels.is_empty()).then(|| {
            let mut quads: Vec<Quad> = po.scrims.iter().map(|r| scrim_quad(*r, &theme)).collect();
            quads.extend(po.quads.iter().map(chrome_quad));
            Scene {
                quads,
                rows: &[],
                left: 0.0,
                top: 0.0,
                clip: (0.0, 0.0, width as f32, height as f32),
                prose: po.labels.clone(),
            }
        });
    let dead_pane_scenes: Vec<Scene> = pane_overlay_scene.into_iter().collect();
    let sidebar_scene = chrome.sidebar.map(|sb| SidebarScene {
        quads: sb.quads.iter().map(chrome_quad).collect(),
        rows: &[],
        left: sb.panel.x,
        top: sb.panel.y,
        clip: (sb.panel.x, sb.panel.y, sb.panel.w, sb.panel.h),
        prose: sb.labels.clone(),
    });
    let gitdock_scene = chrome.git_dock.map(|gd| {
        let mut quads = dock_frame_quads(gd.panel, &theme, scale as f32);
        quads.extend(gd.quads.iter().map(chrome_quad));
        Scene {
            quads,
            rows: &[],
            left: gd.panel.x,
            top: gd.panel.y,
            clip: (gd.panel.x, gd.panel.y, gd.panel.w, gd.panel.h),
            prose: gd.labels.clone(),
        }
    });
    let timeline_scene = chrome.timeline.map(|tl| {
        let mut quads = dock_frame_quads(tl.panel, &theme, scale as f32);
        quads.extend(tl.quads.iter().map(chrome_quad));
        Scene {
            quads,
            rows: &[],
            left: tl.panel.x,
            top: tl.panel.y,
            clip: (tl.panel.x, tl.panel.y, tl.panel.w, tl.panel.h),
            prose: tl.labels.clone(),
        }
    });
    let overlay_scene = chrome.overlay.map(|ov| {
        let mut quads = card_quads(ov.panel, &theme, scale as f32);
        quads.extend(ov.quads.iter().map(chrome_quad));
        OverlayScene {
            quads,
            rows: &[],
            left: ov.panel.x,
            top: ov.panel.y,
            clip: (ov.panel.x, ov.panel.y, ov.panel.w, ov.panel.h),
            prose: ov.labels.clone(),
        }
    });
    pollster::block_on(capture_async(
        appearance,
        width,
        height,
        scale,
        color(theme.bg_base),
        |text| paint_panes(text, panes, &theme),
        // In the windowed renderer's order: the exited-pane overlays just above the panes,
        // then the sidebar (3-4), the git dock / timeline dock (5-6, mutually exclusive),
        // and the palette overlay (7-8) on top.
        dead_pane_scenes
            .into_iter()
            .chain(
                [sidebar_scene, gitdock_scene, timeline_scene, overlay_scene]
                    .into_iter()
                    .flatten(),
            )
            .collect(),
    ))
}

/// Paint every pane's cells into `text` and return its quads (backgrounds/cursor, plus
/// the dividers + focused ring when tiled) - the pane half of [`capture_panes_rgba`]'s
/// setup, matching the windowed [`Renderer`](crate::Renderer)'s pane pass.
fn paint_panes(text: &mut TextLayer, panes: &[CapturePane], theme: &Theme) -> Vec<Quad> {
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
            pane.cursor_shape,
            theme.accent,
            &[],
        ));
        if let Some(bounds) = pane.logo {
            quads.extend(logo_quads(bounds, theme, LOGO_WATERMARK_OPACITY));
        }
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
}

/// The full-window settings view for [`capture_settings_rgba`], mirroring
/// [`SettingsView`](crate::SettingsView) but owning its display list (proportional chrome).
pub struct CaptureSettings {
    /// The settings panel rectangle (usually the whole surface), physical px.
    pub panel: PxRect,
    /// The x of the nav/content divider (physical px).
    pub nav_divider_x: f32,
    /// The content quads over the frame (active-category + focused-control highlights).
    pub quads: Vec<ChromeQuad>,
    /// The positioned proportional text labels.
    pub labels: Vec<ProseLabel>,
}

/// Render the full-window `settings` view over the theme background headlessly - the
/// exact path the windowed [`Renderer`](crate::Renderer) uses for its settings pass -
/// to an offscreen `width` x `height` sRGB target, returning tight RGBA8 bytes. The
/// settings panel is opaque and full-bleed, so (as in the app) it fully replaces the
/// terminal view; capturing it over the plain background is representative.
///
/// # Panics
/// Panics if no GPU adapter/device is available or the readback fails.
#[must_use]
#[allow(
    clippy::cast_possible_truncation,
    reason = "DPI scale precision loss is irrelevant for the frame stroke width"
)]
pub fn capture_settings_rgba(
    appearance: &Appearance,
    width: u32,
    height: u32,
    scale: f64,
    settings: &CaptureSettings,
) -> Vec<u8> {
    let theme = Theme::resolve(&appearance.theme);
    let mut quads =
        settings_frame_quads(settings.panel, settings.nav_divider_x, &theme, scale as f32);
    quads.extend(settings.quads.iter().map(chrome_quad));
    let scene = Scene {
        quads,
        rows: &[],
        left: settings.panel.x,
        top: settings.panel.y,
        clip: (
            settings.panel.x,
            settings.panel.y,
            settings.panel.w,
            settings.panel.h,
        ),
        prose: settings.labels.clone(),
    };
    pollster::block_on(capture_async(
        appearance,
        width,
        height,
        scale,
        color(theme.bg_base),
        |_text| Vec::new(),
        vec![scene],
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
    /// Proportional prose labels for a migrated surface (the sidebar); empty for
    /// monospace-grid surfaces, which draw only `rows`.
    prose: Vec<ProseLabel>,
}

/// The sidebar chrome scene (passes 3-4 in the windowed renderer).
type SidebarScene<'a> = Scene<'a>;
/// The command-palette overlay scene (passes 7-8, after the git dock's 5-6).
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
    #[allow(
        clippy::cast_possible_truncation,
        reason = "DPI scale precision loss is irrelevant for glyph sizing"
    )]
    let scale_f32 = scale as f32;
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
        // Proportional chrome (the sidebar): draw its positioned labels on top, clipped to
        // the surface panel - the same ProseLayer pass the windowed renderer uses.
        if !scene.prose.is_empty() {
            let (cx, cy, cw, ch) = scene.clip;
            let mut prose = ProseLayer::new(&device, &queue, format, scale_f32);
            prose.set_labels(
                &scene.prose,
                PxRect {
                    x: cx,
                    y: cy,
                    w: cw,
                    h: ch,
                },
            );
            prose
                .draw(&device, &queue, &view, width, height)
                .expect("draw chrome prose");
        }
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
