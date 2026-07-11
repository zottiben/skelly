//! The text layer: a `glyphon` pipeline that draws shaped text into a render target
//! (surface frame or offscreen texture).
//!
//! The background/cursor quad pass ([`QuadLayer`](crate::cells::QuadLayer)) clears
//! the target and fills cell backgrounds first, so this layer **loads** that result
//! and draws the glyphs on top. Extracted so the windowed renderer and the headless
//! capture share the exact drawing code - no drift between what ships and what we
//! verify.

use glyphon::{
    Attrs, Buffer, Cache, Color, Family, FontSystem, Metrics, Resolution, Shaping, SwashCache,
    TextArea, TextAtlas, TextBounds, TextRenderer, Viewport,
};
use skelly_config::Appearance;

use crate::error::RenderError;
use crate::theme::{Srgb, Theme};
use crate::GridCell;

/// Placeholder content proving the shaping -> atlas -> GPU-draw path end-to-end,
/// used by [`TextLayer::set_content`] callers (the plain-text capture example).
const DEMO_TEXT: &str = "skelly\na barebones terminal, built in rust.\n\ntext rendering online: glyphon + cosmic-text on wgpu.";

/// Logical padding (px) from the top-left, matching the design's content pad.
const CONTENT_PAD: f32 = 12.0;

/// A `glyphon` text pipeline bound to a texture format.
pub struct TextLayer {
    scale: f32,
    default_fg: Srgb,
    cell_w: f32,
    cell_h: f32,
    font_system: FontSystem,
    swash_cache: SwashCache,
    viewport: Viewport,
    atlas: TextAtlas,
    renderer: TextRenderer,
    buffer: Buffer,
    family: String,
}

impl TextLayer {
    /// Build the pipeline for `format`, at `scale_factor`, sized for `width` x
    /// `height` physical px, using `appearance` for the font and default color.
    #[must_use]
    pub fn new(
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        format: wgpu::TextureFormat,
        width: u32,
        height: u32,
        scale_factor: f64,
        appearance: &Appearance,
    ) -> Self {
        let mut font_system = FontSystem::new();
        let swash_cache = SwashCache::new();
        let cache = Cache::new(device);
        let viewport = Viewport::new(device, &cache);
        let mut atlas = TextAtlas::new(device, queue, &cache, format);
        let renderer =
            TextRenderer::new(&mut atlas, device, wgpu::MultisampleState::default(), None);

        let scale = scale_to_f32(scale_factor);
        let font_px = f32::from(appearance.font_size) * scale;
        let line_px = font_px * appearance.line_height;
        let cell_w = measure_cell_width(&mut font_system, font_px, line_px);

        let family = appearance.font_family.clone();
        let mut buffer = Buffer::new(&mut font_system, Metrics::new(font_px, line_px));
        buffer.set_size(Some(dim_to_f32(width)), Some(dim_to_f32(height)));
        buffer.set_text(
            DEMO_TEXT,
            &Attrs::new().family(Family::Name(&family)),
            Shaping::Advanced,
            None,
        );
        buffer.shape_until_scroll(&mut font_system, false);

        Self {
            scale,
            default_fg: Theme::resolve(&appearance.theme).fg_primary,
            cell_w,
            cell_h: line_px,
            font_system,
            swash_cache,
            viewport,
            atlas,
            renderer,
            buffer,
            family,
        }
    }

    /// Cell metrics in physical px: `(width, height, top-left padding)`. Used to
    /// place cell backgrounds and the cursor so they align with the text.
    #[must_use]
    pub fn cell_metrics(&self) -> (f32, f32, f32) {
        (self.cell_w, self.cell_h, CONTENT_PAD * self.scale)
    }

    /// Replace the displayed text with a plain string in the configured cell font.
    pub fn set_content(&mut self, text: &str) {
        let attrs = Attrs::new().family(Family::Name(&self.family));
        self.buffer.set_text(text, &attrs, Shaping::Advanced, None);
        self.buffer.shape_until_scroll(&mut self.font_system, false);
    }

    /// Replace the display with a colored grid, drawing each cell's glyph in its
    /// foreground color. Uses a monospace face so columns align; consecutive
    /// same-color cells merge into runs to keep the span count down.
    pub fn set_cells(&mut self, rows: &[Vec<GridCell>]) {
        let mut runs: Vec<(String, Color)> = Vec::new();
        for (index, row) in rows.iter().enumerate() {
            if index > 0 {
                runs.push((String::from("\n"), Color::rgb(0, 0, 0)));
            }
            let mut current: Option<(String, Color)> = None;
            for cell in row {
                let color = Color::rgb(cell.fg.r, cell.fg.g, cell.fg.b);
                match current.as_mut() {
                    Some((text, run_color)) if *run_color == color => text.push(cell.c),
                    _ => {
                        if let Some(run) = current.take() {
                            runs.push(run);
                        }
                        current = Some((cell.c.to_string(), color));
                    }
                }
            }
            if let Some(run) = current.take() {
                runs.push(run);
            }
        }

        let default = Attrs::new().family(Family::Monospace);
        let spans = runs.iter().map(|(text, color)| {
            (
                text.as_str(),
                Attrs::new().family(Family::Monospace).color(*color),
            )
        });
        self.buffer
            .set_rich_text(spans, &default, Shaping::Advanced, None);
        self.buffer.shape_until_scroll(&mut self.font_system, false);
    }

    /// Re-layout the text for a new target size.
    pub fn resize(&mut self, width: u32, height: u32) {
        self.buffer
            .set_size(Some(dim_to_f32(width)), Some(dim_to_f32(height)));
        self.buffer.shape_until_scroll(&mut self.font_system, false);
    }

    /// Draw the text into `view`, *loading* the existing contents (the quad pass has
    /// already cleared and filled backgrounds). Submits its own command buffer.
    ///
    /// # Errors
    /// Returns [`RenderError::Text`] if shaping/preparing or drawing the glyphs
    /// fails.
    pub fn draw(
        &mut self,
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        view: &wgpu::TextureView,
        width: u32,
        height: u32,
    ) -> Result<(), RenderError> {
        self.viewport.update(queue, Resolution { width, height });

        let fg = self.default_fg;
        let pad = CONTENT_PAD * self.scale;
        self.renderer
            .prepare(
                device,
                queue,
                &mut self.font_system,
                &mut self.atlas,
                &self.viewport,
                [TextArea {
                    buffer: &self.buffer,
                    left: pad,
                    top: pad,
                    scale: 1.0,
                    bounds: TextBounds {
                        left: 0,
                        top: 0,
                        right: dim_to_i32(width),
                        bottom: dim_to_i32(height),
                    },
                    default_color: Color::rgb(fg.r, fg.g, fg.b),
                    custom_glyphs: &[],
                }],
                &mut self.swash_cache,
            )
            .map_err(|err| RenderError::Text(err.to_string()))?;

        let mut encoder = device.create_command_encoder(&wgpu::CommandEncoderDescriptor {
            label: Some("text-encoder"),
        });
        {
            let mut pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
                label: Some("text"),
                color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                    view,
                    depth_slice: None,
                    resolve_target: None,
                    ops: wgpu::Operations {
                        load: wgpu::LoadOp::Load,
                        store: wgpu::StoreOp::Store,
                    },
                })],
                depth_stencil_attachment: None,
                timestamp_writes: None,
                occlusion_query_set: None,
                multiview_mask: None,
            });
            self.renderer
                .render(&self.atlas, &self.viewport, &mut pass)
                .map_err(|err| RenderError::Text(err.to_string()))?;
        }

        queue.submit(std::iter::once(encoder.finish()));
        self.atlas.trim();
        Ok(())
    }
}

/// Measure the advance width of a monospace glyph at `font_px`, in physical px.
fn measure_cell_width(font_system: &mut FontSystem, font_px: f32, line_px: f32) -> f32 {
    let mut probe = Buffer::new(font_system, Metrics::new(font_px, line_px));
    probe.set_text(
        "M",
        &Attrs::new().family(Family::Monospace),
        Shaping::Advanced,
        None,
    );
    probe.shape_until_scroll(font_system, false);
    probe
        .layout_runs()
        .next()
        .and_then(|run| run.glyphs.first().map(|glyph| glyph.w))
        .filter(|width| *width > 0.0)
        .unwrap_or(font_px * 0.6)
}

/// Cast a scale factor to `f32`. Sub-pixel precision loss is irrelevant for glyphs.
#[allow(
    clippy::cast_possible_truncation,
    reason = "scale-factor precision loss does not matter for glyph sizing"
)]
fn scale_to_f32(value: f64) -> f32 {
    value as f32
}

/// Cast a pixel dimension to `f32` for text layout.
#[allow(
    clippy::cast_precision_loss,
    reason = "window pixel dimensions are far within f32's exact-integer range"
)]
fn dim_to_f32(value: u32) -> f32 {
    value as f32
}

/// Cast a pixel dimension to `i32` for text-clip bounds.
#[allow(
    clippy::cast_possible_wrap,
    reason = "window pixel dimensions are far below i32::MAX"
)]
fn dim_to_i32(value: u32) -> i32 {
    value as i32
}
