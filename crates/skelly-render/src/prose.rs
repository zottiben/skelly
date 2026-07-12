//! The proportional-text layer: positioned chrome labels drawn in the guide's fonts.
//!
//! Where [`TextLayer`](crate::text::TextLayer) shapes a *monospace cell grid* for the
//! terminal, this layer draws a flat list of independently positioned [`ProseLabel`]s -
//! each a run of proportional text at a pixel origin, in a [`FontRole`] from the §05
//! scale. It is what makes the sidebar, palette, settings, docks, and status line render
//! as the design guide draws them (IBM Plex Sans / Space Grotesk / `JetBrains` Mono) rather
//! than as oversized terminal cells.
//!
//! Each label owns a small `glyphon` buffer, and - like the terminal layer - re-shapes
//! only when its content fingerprint changes, so an unchanged chrome repaint (an
//! animation frame, an idle redraw) costs microseconds. The binary lays chrome out with
//! the matching GPU-free [`TextMeasure`], so hit-testing and rendering agree on widths.

use std::collections::hash_map::DefaultHasher;
use std::hash::{Hash, Hasher};

use glyphon::cosmic_text::LetterSpacing;
use glyphon::{
    Attrs, Buffer, Cache, Color, Family, FontSystem, Metrics, Resolution, Shaping, TextArea,
    TextAtlas, TextBounds, TextRenderer, Viewport, Weight, Wrap,
};

use crate::error::RenderError;
use crate::fonts::{load_bundled, FontRole};
use crate::theme::Srgb;
use crate::PxRect;

/// One positioned proportional label to draw. `x`/`y` are the physical-pixel top-left of
/// the text's line box; the label is clipped to the surface panel and to `max_w` (long
/// text is cut, never wrapped). Colors are the resolved UI token (already theme-correct).
#[derive(Clone, Debug, PartialEq)]
pub struct ProseLabel {
    /// The text to draw (a single line).
    pub text: String,
    /// Left edge of the line box, physical px.
    pub x: f32,
    /// Top edge of the line box, physical px.
    pub y: f32,
    /// The type role (family / size / line-height / tracking) from the §05 scale.
    pub role: FontRole,
    /// Glyph color (a resolved UI token).
    pub color: Srgb,
    /// Weight override (100..900); `None` uses the role's default weight.
    pub weight: Option<u16>,
    /// Maximum line width in physical px - the buffer clips here (no wrap).
    pub max_w: f32,
}

/// One label's shaped buffer plus where it draws and a fingerprint of what was shaped.
struct LabelBuf {
    buffer: Buffer,
    x: f32,
    y: f32,
    color: Srgb,
    shaped: Option<u64>,
}

/// A `glyphon` pipeline for positioned proportional chrome labels, parallel to
/// [`TextLayer`](crate::text::TextLayer) but keyed on labels rather than a cell grid.
pub(crate) struct ProseLayer {
    scale: f32,
    clip: PxRect,
    font_system: FontSystem,
    swash_cache: glyphon::SwashCache,
    viewport: Viewport,
    atlas: TextAtlas,
    renderer: TextRenderer,
    labels: Vec<LabelBuf>,
}

impl ProseLayer {
    /// Build the pipeline for `format` at DPI `scale`, sized `width` x `height` physical px.
    #[must_use]
    pub(crate) fn new(
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        format: wgpu::TextureFormat,
        scale: f32,
    ) -> Self {
        let mut font_system = FontSystem::new();
        load_bundled(font_system.db_mut());
        let swash_cache = glyphon::SwashCache::new();
        let cache = Cache::new(device);
        let viewport = Viewport::new(device, &cache);
        let mut atlas = TextAtlas::new(device, queue, &cache, format);
        let renderer =
            TextRenderer::new(&mut atlas, device, wgpu::MultisampleState::default(), None);
        Self {
            scale,
            clip: PxRect {
                x: 0.0,
                y: 0.0,
                w: 0.0,
                h: 0.0,
            },
            font_system,
            swash_cache,
            viewport,
            atlas,
            renderer,
            labels: Vec::new(),
        }
    }

    /// Grow/shrink the buffer pool to exactly `n`, reusing existing buffers.
    fn ensure(&mut self, n: usize) {
        let Self {
            labels,
            font_system,
            ..
        } = self;
        while labels.len() < n {
            let mut buffer = Buffer::new(font_system, Metrics::new(1.0, 1.0));
            buffer.set_wrap(Wrap::None);
            labels.push(LabelBuf {
                buffer,
                x: 0.0,
                y: 0.0,
                color: Srgb { r: 0, g: 0, b: 0 },
                shaped: None,
            });
        }
        labels.truncate(n);
    }

    /// Replace the labels drawn next frame, clipped to `clip`. Re-shapes only labels whose
    /// content fingerprint changed; repositioning is always cheap.
    pub(crate) fn set_labels(&mut self, input: &[ProseLabel], clip: PxRect) {
        self.clip = clip;
        self.ensure(input.len());
        let scale = self.scale;
        let Self {
            labels,
            font_system,
            ..
        } = self;
        for (buf, label) in labels.iter_mut().zip(input) {
            buf.x = label.x;
            buf.y = label.y;
            buf.color = label.color;
            let fingerprint = label_fingerprint(label, scale);
            if buf.shaped == Some(fingerprint) {
                continue;
            }
            let role = label.role;
            let font_px = role.size_px() * scale;
            let line_px = role.line_height_px(scale);
            buf.buffer.set_metrics(Metrics::new(font_px, line_px));
            // No width bound: the line lays out at its natural advance (with `Wrap::None`)
            // and is clipped to the surface panel by the draw-time `TextBounds`. A finite
            // width here would let a short line justify/stretch to fill it (the `max_w` field
            // is kept for callers' layout math, not for constraining the buffer).
            buf.buffer.set_size(None, Some(line_px.max(1.0)));
            let weight = Weight(label.weight.unwrap_or_else(|| role.weight()));
            let mut attrs = Attrs::new()
                .family(Family::Name(role.family()))
                .weight(weight)
                .color(Color::rgb(label.color.r, label.color.g, label.color.b));
            // cosmic-text letter-spacing is in em (scaled by the font size at layout), so a
            // pixel tracking of `t` px is `t / font_size` em - which reduces to the logical
            // ratio, independent of DPI scale.
            if let Some(spacing) = letter_spacing(role) {
                attrs.letter_spacing_opt = Some(spacing);
            }
            buf.buffer
                .set_text(&label.text, &attrs, Shaping::Advanced, None);
            buf.buffer.shape_until_scroll(font_system, false);
            buf.shaped = Some(fingerprint);
        }
    }

    /// Hide every label (nothing drawn next frame) without freeing the buffers.
    pub(crate) fn clear(&mut self) {
        self.labels.clear();
    }

    /// Draw every label into `view`, loading the existing contents (chrome quads have
    /// already drawn beneath). Clips each to the surface panel. Submits its own command
    /// buffer.
    ///
    /// # Errors
    /// Returns [`RenderError::Text`] if shaping/preparing or drawing the glyphs fails.
    pub(crate) fn draw(
        &mut self,
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        view: &wgpu::TextureView,
        width: u32,
        height: u32,
    ) -> Result<(), RenderError> {
        if self.labels.is_empty() {
            return Ok(());
        }
        self.viewport.update(queue, Resolution { width, height });
        let (cx, cy, cw, ch) = (self.clip.x, self.clip.y, self.clip.w, self.clip.h);
        let bounds = TextBounds {
            left: clip_i32(cx),
            top: clip_i32(cy),
            right: clip_i32(cx + cw),
            bottom: clip_i32(cy + ch),
        };
        let areas: Vec<TextArea> = self
            .labels
            .iter()
            .map(|label| TextArea {
                buffer: &label.buffer,
                left: label.x,
                top: label.y,
                scale: 1.0,
                bounds,
                default_color: Color::rgb(label.color.r, label.color.g, label.color.b),
                custom_glyphs: &[],
            })
            .collect();
        self.renderer
            .prepare(
                device,
                queue,
                &mut self.font_system,
                &mut self.atlas,
                &self.viewport,
                areas,
                &mut self.swash_cache,
            )
            .map_err(|err| RenderError::Text(err.to_string()))?;

        let mut encoder = device.create_command_encoder(&wgpu::CommandEncoderDescriptor {
            label: Some("prose-encoder"),
        });
        {
            let mut pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
                label: Some("prose"),
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

/// The role's letter-spacing as a cosmic-text em value (its `tracking / size` ratio), or
/// `None` when the role has no tracking. cosmic-text adds letter-spacing in em space and
/// scales it by the font size at layout, so this em ratio yields the guide's pixel tracking
/// at any font size and DPI scale.
fn letter_spacing(role: FontRole) -> Option<LetterSpacing> {
    let track = role.tracking_px();
    (track.abs() > f32::EPSILON).then(|| LetterSpacing(track / role.size_px()))
}

/// A fingerprint of everything that affects a label's shaped glyphs, so an unchanged label
/// skips the (comparatively expensive) re-shape on a later frame - matching the terminal
/// layer's shape-skip. Position is *not* included (it is applied cheaply every frame).
fn label_fingerprint(label: &ProseLabel, scale: f32) -> u64 {
    let mut hasher = DefaultHasher::new();
    label.text.hash(&mut hasher);
    label.role.hash(&mut hasher);
    label.color.hash(&mut hasher);
    label.weight.hash(&mut hasher);
    label.max_w.to_bits().hash(&mut hasher);
    scale.to_bits().hash(&mut hasher);
    hasher.finish()
}

/// Round a clip coordinate to `i32` for a glyph clip bound (small, may be negative off the
/// top/left of a scrolled surface - clamped by glyphon).
#[allow(
    clippy::cast_possible_truncation,
    reason = "clip coordinates are small pixel values"
)]
fn clip_i32(value: f32) -> i32 {
    value.round() as i32
}

/// A GPU-free proportional-text measurer, owned by the binary to lay out and hit-test the
/// chrome. It loads the same bundled fonts the renderer uses, so a width measured here
/// equals the width drawn there - the invariant that keeps click targets aligned with what
/// the user sees.
pub struct TextMeasure {
    font_system: FontSystem,
    scratch: Buffer,
    scale: f32,
}

impl TextMeasure {
    /// Build a measurer at DPI `scale`.
    #[must_use]
    pub fn new(scale: f32) -> Self {
        let mut font_system = FontSystem::new();
        load_bundled(font_system.db_mut());
        let mut scratch = Buffer::new(&mut font_system, Metrics::new(1.0, 1.0));
        scratch.set_wrap(Wrap::None);
        Self {
            font_system,
            scratch,
            scale,
        }
    }

    /// Update the DPI scale (widths returned are in physical px at this scale).
    pub fn set_scale(&mut self, scale: f32) {
        self.scale = scale;
    }

    /// The current DPI scale (physical px per logical px).
    #[must_use]
    pub fn scale(&self) -> f32 {
        self.scale
    }

    /// The physical-px advance width of `text` in `role` (with optional weight override).
    #[must_use]
    pub fn width(&mut self, text: &str, role: FontRole, weight: Option<u16>) -> f32 {
        let font_px = role.size_px() * self.scale;
        let line_px = role.line_height_px(self.scale);
        self.scratch.set_metrics(Metrics::new(font_px, line_px));
        self.scratch.set_size(None, Some(line_px.max(1.0)));
        let weight = Weight(weight.unwrap_or_else(|| role.weight()));
        let mut attrs = Attrs::new()
            .family(Family::Name(role.family()))
            .weight(weight);
        if let Some(spacing) = letter_spacing(role) {
            attrs.letter_spacing_opt = Some(spacing);
        }
        self.scratch.set_text(text, &attrs, Shaping::Advanced, None);
        self.scratch
            .shape_until_scroll(&mut self.font_system, false);
        self.scratch
            .layout_runs()
            .next()
            .map_or(0.0, |run| run.line_w)
    }

    /// The line-box height in physical px for `role` at the current scale.
    #[must_use]
    pub fn line_height(&self, role: FontRole) -> f32 {
        role.line_height_px(self.scale)
    }
}

#[cfg(test)]
mod tests {
    use super::TextMeasure;
    use crate::fonts::FontRole;

    #[test]
    fn measures_proportional_widths_that_scale_and_grow_with_text() {
        let mut m = TextMeasure::new(2.0);
        let one = m.width("W", FontRole::Label, None);
        let many = m.width("WWWWW", FontRole::Label, None);
        assert!(one > 0.0, "a glyph has a positive advance");
        assert!(many > one * 3.0, "five glyphs are much wider than one");
        // Proportional, not monospace: "il" is far narrower than "WW".
        assert!(m.width("il", FontRole::Body, None) < m.width("WW", FontRole::Body, None));
        // Widths scale with DPI.
        m.set_scale(1.0);
        let small = m.width("W", FontRole::Label, None);
        assert!(one > small * 1.5, "2x is wider than 1x");
    }

    #[test]
    fn line_height_follows_the_role_and_scale() {
        let m = TextMeasure::new(2.0);
        assert!((m.line_height(FontRole::Body) - 14.0 * 1.6 * 2.0).abs() < 1e-3);
        assert!((m.line_height(FontRole::Label) - 13.0 * 1.4 * 2.0).abs() < 1e-3);
    }
}
