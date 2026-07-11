//! The text layer: a `glyphon` pipeline that clears to the theme background and
//! draws shaped text into any render target (surface frame or offscreen texture).
//!
//! Extracted so the windowed [`Renderer`](crate::Renderer) and the headless capture
//! path share the exact same drawing code - no drift between what ships and what we
//! verify. The real cell grid replaces the demo buffer in M1c/M2.

use glyphon::{
    Attrs, Buffer, Cache, Color, Family, FontSystem, Metrics, Resolution, Shaping, SwashCache,
    TextArea, TextAtlas, TextBounds, TextRenderer, Viewport,
};
use skelly_config::Appearance;

use crate::error::RenderError;
use crate::theme::Theme;

/// Placeholder content proving the shaping -> atlas -> GPU-draw path end-to-end.
/// Replaced by the live terminal grid in M1c/M2.
const DEMO_TEXT: &str = "skelly\na barebones terminal, built in rust.\n\nM1b - text rendering online: glyphon + cosmic-text on wgpu.\nnext: PTY + terminal core (M1c).";

/// Logical padding (px) from the top-left, matching the design's content pad.
const CONTENT_PAD: f32 = 12.0;

/// A `glyphon` text pipeline bound to a texture format, drawing the resolved theme.
pub struct TextLayer {
    theme: Theme,
    scale: f32,
    font_system: FontSystem,
    swash_cache: SwashCache,
    viewport: Viewport,
    atlas: TextAtlas,
    renderer: TextRenderer,
    buffer: Buffer,
}

impl TextLayer {
    /// Build the pipeline for `format`, laying out the demo text in the configured
    /// cell font at `scale_factor`, sized for `width` x `height` physical px.
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
        let mut buffer = Buffer::new(&mut font_system, Metrics::new(font_px, line_px));
        buffer.set_size(Some(dim_to_f32(width)), Some(dim_to_f32(height)));
        buffer.set_text(
            DEMO_TEXT,
            &Attrs::new().family(Family::Name(&appearance.font_family)),
            Shaping::Advanced,
            None,
        );
        buffer.shape_until_scroll(&mut font_system, false);

        Self {
            theme: Theme::resolve(&appearance.theme),
            scale,
            font_system,
            swash_cache,
            viewport,
            atlas,
            renderer,
            buffer,
        }
    }

    /// Re-layout the text for a new target size.
    pub fn resize(&mut self, width: u32, height: u32) {
        self.buffer
            .set_size(Some(dim_to_f32(width)), Some(dim_to_f32(height)));
        self.buffer.shape_until_scroll(&mut self.font_system, false);
    }

    /// Clear `view` to the theme background and draw the text into it. Submits its
    /// own command buffer; the caller presents (surface) or reads back (offscreen).
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

        let fg = self.theme.fg_primary;
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

        let bg = self.theme.bg_base;
        let mut encoder = device.create_command_encoder(&wgpu::CommandEncoderDescriptor {
            label: Some("skelly-frame"),
        });
        {
            let mut pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
                label: Some("clear+text"),
                color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                    view,
                    depth_slice: None,
                    resolve_target: None,
                    ops: wgpu::Operations {
                        load: wgpu::LoadOp::Clear(wgpu::Color {
                            r: bg.r,
                            g: bg.g,
                            b: bg.b,
                            a: bg.a,
                        }),
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
