//! The text layer: a `glyphon` pipeline that draws shaped text into a render target
//! (surface frame or offscreen texture).
//!
//! The background/cursor quad pass ([`QuadLayer`](crate::cells::QuadLayer)) clears
//! the target and fills cell backgrounds first, so this layer **loads** that result
//! and draws the glyphs on top. Extracted so the windowed renderer and the headless
//! capture share the exact drawing code - no drift between what ships and what we
//! verify.

use std::collections::hash_map::DefaultHasher;
use std::hash::{Hash, Hasher};

use glyphon::{
    Attrs, Buffer, Cache, Color, Family, FontSystem, Metrics, Resolution, Shaping, Style,
    SwashCache, TextArea, TextAtlas, TextBounds, TextRenderer, Viewport, Weight, Wrap,
};
use skelly_config::Appearance;

use crate::error::RenderError;
use crate::theme::{Srgb, Theme};
use crate::GridCell;

/// Logical padding (px) from a pane's top-left, matching the design's content pad.
const CONTENT_PAD: f32 = 12.0;

/// One pane's shaped text buffer plus where it draws: the pixel position of its
/// top-left cell (`left`, `top`) and its clip rectangle `(x, y, w, h)` (so a row
/// wider than the pane is clipped rather than spilling into a neighbor).
struct PaneBuf {
    buffer: Buffer,
    left: f32,
    top: f32,
    clip: (f32, f32, f32, f32),
    /// A fingerprint of the last grid + size actually shaped into `buffer`, so an
    /// identical grid on a later frame skips the (expensive) re-shape - see the note
    /// on [`TextLayer::set_panes`]. `None` forces the next call to shape.
    shaped: Option<u64>,
}

/// One pane's text input to [`TextLayer::set_panes`]: the cell grid plus the pixel
/// origin of cell `(0, 0)` and the clip rectangle.
pub(crate) struct PaneTextInput<'a> {
    pub rows: &'a [Vec<GridCell>],
    pub left: f32,
    pub top: f32,
    pub clip: (f32, f32, f32, f32),
}

/// A `glyphon` text pipeline bound to a texture format. Holds one shaped buffer per
/// visible pane, all drawn in a single prepare/render pass.
pub struct TextLayer {
    scale: f32,
    default_fg: Srgb,
    cell_w: f32,
    cell_h: f32,
    /// Font size + line height, for building each pane's buffer.
    metrics: Metrics,
    /// Current render-target size (physical px), used as the single-pane clip.
    target_w: f32,
    target_h: f32,
    font_system: FontSystem,
    swash_cache: SwashCache,
    viewport: Viewport,
    atlas: TextAtlas,
    renderer: TextRenderer,
    panes: Vec<PaneBuf>,
    /// The configured font family when installed; `None` falls back to monospace.
    family_name: Option<String>,
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
        let mut font_system = crate::fonts::new_font_system();
        let swash_cache = SwashCache::new();
        let cache = Cache::new(device);
        let viewport = Viewport::new(device, &cache);
        let mut atlas = TextAtlas::new(device, queue, &cache, format);
        let renderer =
            TextRenderer::new(&mut atlas, device, wgpu::MultisampleState::default(), None);

        let scale = scale_to_f32(scale_factor);
        let font_px = f32::from(appearance.font_size) * scale;
        let line_px = font_px * appearance.line_height;

        // Honor the configured font (Nerd Fonts included) when it is installed;
        // otherwise fall back to a system monospace face so columns still align.
        let family_name = installed_family(&font_system, &appearance.font_family);
        let cell_w = measure_cell_width(
            &mut font_system,
            font_px,
            line_px,
            family_of(family_name.as_deref()),
        );

        // Panes are created lazily by the first `set_panes` / `set_cells` call.
        Self {
            scale,
            default_fg: Theme::resolve(&appearance.theme).fg_primary,
            cell_w,
            cell_h: line_px,
            metrics: Metrics::new(font_px, line_px),
            target_w: dim_to_f32(width),
            target_h: dim_to_f32(height),
            font_system,
            swash_cache,
            viewport,
            atlas,
            renderer,
            panes: Vec::new(),
            family_name,
        }
    }

    /// Rebuild the cell metrics for a new `font_size` (px) / `line_height` - the live `⌘=/-/0`
    /// font-size bindings (design §11). Recomputes the cell width/height + shaping metrics and
    /// forces every pane to re-shape at the new size (the shape fingerprint keys on the clip
    /// size, not the font metrics, so it would not otherwise invalidate). The binary re-fits the
    /// PTY grids to the new cell size afterwards.
    pub fn set_font_size(&mut self, font_size: u16, line_height: f32) {
        let font_px = f32::from(font_size) * self.scale;
        let line_px = font_px * line_height;
        self.metrics = Metrics::new(font_px, line_px);
        self.cell_h = line_px;
        self.cell_w = measure_cell_width(
            &mut self.font_system,
            font_px,
            line_px,
            family_of(self.family_name.as_deref()),
        );
        // Store the new metrics on each buffer and clear its shape cache; the next `set_panes`
        // re-shapes it (with a fresh `font_system` borrow) at the new size.
        for pane in &mut self.panes {
            pane.buffer.set_metrics(self.metrics);
            pane.shaped = None;
        }
    }

    /// Cell metrics in physical px: `(width, height, top-left padding)`. Used to
    /// place cell backgrounds and the cursor so they align with the text.
    #[must_use]
    pub fn cell_metrics(&self) -> (f32, f32, f32) {
        (self.cell_w, self.cell_h, CONTENT_PAD * self.scale)
    }

    /// The DPI scale factor (physical px per logical px), for sizing pane dividers.
    #[must_use]
    pub(crate) fn scale(&self) -> f32 {
        self.scale
    }

    /// Update the fallback glyph color used for cells with no explicit color, when the
    /// active theme changes.
    pub(crate) fn set_default_fg(&mut self, fg: Srgb) {
        self.default_fg = fg;
    }

    /// Grow or shrink the pane-buffer pool to exactly `n` buffers, reusing existing
    /// ones. New buffers inherit the no-reflow policy (each grid row is one visual
    /// line - see the width note below).
    fn ensure_panes(&mut self, n: usize) {
        let Self {
            panes,
            font_system,
            metrics,
            ..
        } = self;
        while panes.len() < n {
            // A terminal grid never reflows: each row is exactly one visual line.
            // Without this, full-width rows (trailing spaces included) exceed the
            // buffer width and wrap, doubling the line pitch and desyncing the glyphs
            // from the cell-background / cursor / underline quads.
            let mut buffer = Buffer::new(font_system, *metrics);
            buffer.set_wrap(Wrap::None);
            panes.push(PaneBuf {
                buffer,
                left: 0.0,
                top: 0.0,
                clip: (0.0, 0.0, 0.0, 0.0),
                shaped: None,
            });
        }
        panes.truncate(n);
    }

    /// Shape each pane's grid into its own buffer, recording where it draws. One
    /// buffer per input; consecutive cells with the same color and attributes merge
    /// into runs to keep the span count down.
    ///
    /// Shaping a full grid with `cosmic-text` costs on the order of a millisecond (many
    /// more for a busy colored grid), and the windowed renderer calls this every redraw -
    /// including redraws driven by animation, selection, or an *unchanged* pane. So each
    /// pane skips the re-shape when its grid + size fingerprint matches what is already
    /// shaped in its buffer; only its draw position (which is cheap and applied below
    /// regardless) may have moved. This is the difference between a repaint costing
    /// microseconds and costing milliseconds. (A true fixed-metric atlas renderer would
    /// remove the shaping entirely - tracked as the M2c follow-up.)
    pub(crate) fn set_panes(&mut self, inputs: &[PaneTextInput]) {
        self.ensure_panes(inputs.len());
        let Self {
            panes,
            font_system,
            family_name,
            ..
        } = self;
        let family = family_of(family_name.as_deref());
        let default = Attrs::new().family(family);
        for (pane, input) in panes.iter_mut().zip(inputs) {
            // Position is cheap and may change without the content changing (e.g. a pane
            // slides on resize), so always refresh it; only the shaping is gated.
            pane.left = input.left;
            pane.top = input.top;
            pane.clip = input.clip;

            let fingerprint = shape_fingerprint(input.rows, input.clip.2, input.clip.3);
            if pane.shaped == Some(fingerprint) {
                continue;
            }

            pane.buffer
                .set_size(Some(input.clip.2.max(1.0)), Some(input.clip.3.max(1.0)));
            let runs = text_runs(input.rows);
            if runs.is_empty() {
                pane.buffer.set_text("", &default, Shaping::Advanced, None);
            } else {
                let spans = runs
                    .iter()
                    .map(|run| (run.text.as_str(), run.attrs(family)));
                pane.buffer
                    .set_rich_text(spans, &default, Shaping::Advanced, None);
            }
            pane.buffer.shape_until_scroll(font_system, false);
            pane.shaped = Some(fingerprint);
        }
    }

    /// Replace the display with a single pane of plain `text` at the content pad, in
    /// the configured cell font. (The plain-text capture path.)
    pub fn set_content(&mut self, text: &str) {
        self.ensure_panes(1);
        let pad = CONTENT_PAD * self.scale;
        let Self {
            panes,
            font_system,
            family_name,
            target_w,
            target_h,
            ..
        } = self;
        let pane = &mut panes[0];
        pane.left = pad;
        pane.top = pad;
        pane.clip = (0.0, 0.0, *target_w, *target_h);
        pane.buffer
            .set_size(Some(target_w.max(1.0)), Some(target_h.max(1.0)));
        let attrs = Attrs::new().family(family_of(family_name.as_deref()));
        pane.buffer.set_text(text, &attrs, Shaping::Advanced, None);
        pane.buffer.shape_until_scroll(font_system, false);
        // This path shapes unconditionally and isn't grid-fingerprinted; clear any stale
        // fingerprint so a later `set_panes` on the reused buffer still re-shapes.
        pane.shaped = None;
    }

    /// Replace the display with a single colored grid at the content pad, filling the
    /// whole target. (The headless cell-capture path; the windowed renderer drives
    /// multiple panes through `set_panes`.)
    pub fn set_cells(&mut self, rows: &[Vec<GridCell>]) {
        let pad = CONTENT_PAD * self.scale;
        let clip = (0.0, 0.0, self.target_w, self.target_h);
        self.set_panes(&[PaneTextInput {
            rows,
            left: pad,
            top: pad,
            clip,
        }]);
    }

    /// Record a new target size. The windowed renderer re-runs `set_panes` with
    /// fresh clips every frame, so this only keeps the single-pane capture/plain
    /// paths' clip in sync.
    pub fn resize(&mut self, width: u32, height: u32) {
        self.target_w = dim_to_f32(width);
        self.target_h = dim_to_f32(height);
    }

    /// Draw every pane's text into `view`, *loading* the existing contents (the quad
    /// pass has already cleared and filled backgrounds). Each pane clips to its own
    /// rectangle. Submits its own command buffer.
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
        let areas: Vec<TextArea> = self
            .panes
            .iter()
            .map(|pane| {
                let (cx, cy, cw, ch) = pane.clip;
                TextArea {
                    buffer: &pane.buffer,
                    left: pane.left,
                    top: pane.top,
                    scale: 1.0,
                    bounds: TextBounds {
                        left: px_to_i32(cx),
                        top: px_to_i32(cy),
                        right: px_to_i32(cx + cw),
                        bottom: px_to_i32(cy + ch),
                    },
                    default_color: Color::rgb(fg.r, fg.g, fg.b),
                    custom_glyphs: &[],
                }
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

/// A run of consecutive cells that share a color, weight, and style - the unit we
/// hand to `set_rich_text` as one span. Newlines are their own (color-irrelevant)
/// runs separating rows. `pub(crate)` only so the bench seam can observe `text_runs`'
/// output; its fields stay private (glyphon's `Color` never leaves this module).
pub(crate) struct Run {
    text: String,
    color: Color,
    bold: bool,
    italic: bool,
}

impl Run {
    /// The shaping attributes for this run in `family`: color plus bold/italic.
    fn attrs<'a>(&self, family: Family<'a>) -> Attrs<'a> {
        let mut attrs = Attrs::new().family(family).color(self.color);
        if self.bold {
            attrs = attrs.weight(Weight::BOLD);
        }
        if self.italic {
            attrs = attrs.style(Style::Italic);
        }
        attrs
    }
}

/// Merge a grid into runs of same-color, same-weight, same-style cells, with a
/// newline run between rows. Underline is *not* a run key - it is drawn as a quad,
/// not a shaping attribute (glyphon does not render text decorations).
pub(crate) fn text_runs(rows: &[Vec<GridCell>]) -> Vec<Run> {
    let mut runs: Vec<Run> = Vec::new();
    for (index, row) in rows.iter().enumerate() {
        if index > 0 {
            runs.push(Run {
                text: String::from("\n"),
                color: Color::rgb(0, 0, 0),
                bold: false,
                italic: false,
            });
        }
        let mut current: Option<Run> = None;
        for cell in row {
            let color = Color::rgb(cell.fg.r, cell.fg.g, cell.fg.b);
            match current.as_mut() {
                Some(run)
                    if run.color == color && run.bold == cell.bold && run.italic == cell.italic =>
                {
                    run.text.push(cell.c);
                }
                _ => {
                    if let Some(run) = current.take() {
                        runs.push(run);
                    }
                    current = Some(Run {
                        text: cell.c.to_string(),
                        color,
                        bold: cell.bold,
                        italic: cell.italic,
                    });
                }
            }
        }
        if let Some(run) = current.take() {
            runs.push(run);
        }
    }
    runs
}

/// Measure the cell size `(width, height)` in physical px for `appearance` at
/// `scale_factor`, with no GPU - the exact metrics the grid tiles to. Lets headless
/// callers (captures, tests) size a grid to the same cells the renderer draws.
#[must_use]
pub fn measure_cell(appearance: &Appearance, scale_factor: f64) -> (f32, f32) {
    let mut font_system = crate::fonts::new_font_system();
    let scale = scale_to_f32(scale_factor);
    let font_px = f32::from(appearance.font_size) * scale;
    let line_px = font_px * appearance.line_height;
    let family_name = installed_family(&font_system, &appearance.font_family);
    let cell_w = measure_cell_width(
        &mut font_system,
        font_px,
        line_px,
        family_of(family_name.as_deref()),
    );
    (cell_w, line_px)
}

/// Measure the advance width of a glyph in `family` at `font_px`, in physical px.
fn measure_cell_width(
    font_system: &mut FontSystem,
    font_px: f32,
    line_px: f32,
    family: Family,
) -> f32 {
    let mut probe = Buffer::new(font_system, Metrics::new(font_px, line_px));
    probe.set_text("M", &Attrs::new().family(family), Shaping::Advanced, None);
    probe.shape_until_scroll(font_system, false);
    probe
        .layout_runs()
        .next()
        .and_then(|run| run.glyphs.first().map(|glyph| glyph.w))
        .filter(|width| *width > 0.0)
        .unwrap_or(font_px * 0.6)
}

/// Return `Some(name)` if a font family with that name is installed, else `None`.
fn installed_family(font_system: &FontSystem, name: &str) -> Option<String> {
    let installed = font_system.db().faces().any(|face| {
        face.families
            .iter()
            .any(|(family, _)| family.eq_ignore_ascii_case(name))
    });
    installed.then(|| name.to_owned())
}

/// Resolve the family to shape with: the configured name when installed, else a
/// generic monospace face so columns still align.
fn family_of(name: Option<&str>) -> Family<'_> {
    name.map_or(Family::Monospace, Family::Name)
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

/// Convert a pixel coordinate to a non-negative `i32` for a text-clip bound.
#[allow(
    clippy::cast_possible_truncation,
    reason = "clip coordinates are small, non-negative pixel values"
)]
fn px_to_i32(value: f32) -> i32 {
    value.max(0.0).round() as i32
}

/// A 64-bit fingerprint of the grid a pane buffer was shaped from, plus its clip size, so an
/// identical grid on a later frame is detected and the re-shape skipped (see [`TextLayer::
/// set_panes`]). Every field that changes the shaped glyphs - character, color, and the
/// bold/italic face - lives in the hashed `GridCell`; the clip size is hashed by its bit
/// pattern. A hash collision is astronomically unlikely and would at worst leave one frame's
/// glyphs stale until the next change repaints - never a persistent artifact.
fn shape_fingerprint(rows: &[Vec<GridCell>], clip_w: f32, clip_h: f32) -> u64 {
    let mut hasher = DefaultHasher::new();
    clip_w.to_bits().hash(&mut hasher);
    clip_h.to_bits().hash(&mut hasher);
    rows.len().hash(&mut hasher);
    for row in rows {
        row.len().hash(&mut hasher);
        for cell in row {
            cell.hash(&mut hasher);
        }
    }
    hasher.finish()
}

#[cfg(test)]
mod tests {
    use super::{shape_fingerprint, text_runs};
    use crate::{GridCell, Srgb};

    fn cell(c: char, fg: Srgb, bold: bool, italic: bool) -> GridCell {
        GridCell {
            c,
            fg,
            bg: None,
            bold,
            italic,
            underline: false,
        }
    }

    #[test]
    fn shape_fingerprint_changes_with_anything_that_alters_the_shaped_glyphs() {
        // The fingerprint gates whether `set_panes` re-shapes; if it missed a field, an
        // identical fingerprint would skip a needed re-shape and leave stale glyphs. So every
        // glyph-affecting change must move it, and an identical grid must keep it stable.
        let white = Srgb {
            r: 255,
            g: 255,
            b: 255,
        };
        let red = Srgb { r: 255, g: 0, b: 0 };
        let base = vec![vec![cell('a', white, false, false)]];
        let fp = shape_fingerprint(&base, 100.0, 50.0);

        // Identical grid + size -> identical fingerprint (the skip fires).
        assert_eq!(fp, shape_fingerprint(&base, 100.0, 50.0));
        // A different character, color, or face all move it (a re-shape is needed).
        assert_ne!(
            fp,
            shape_fingerprint(&[vec![cell('b', white, false, false)]], 100.0, 50.0)
        );
        assert_ne!(
            fp,
            shape_fingerprint(&[vec![cell('a', red, false, false)]], 100.0, 50.0)
        );
        assert_ne!(
            fp,
            shape_fingerprint(&[vec![cell('a', white, true, false)]], 100.0, 50.0)
        );
        assert_ne!(
            fp,
            shape_fingerprint(&[vec![cell('a', white, false, true)]], 100.0, 50.0)
        );
        // A resize (clip size change) re-shapes, so it must move too.
        assert_ne!(fp, shape_fingerprint(&base, 120.0, 50.0));
        assert_ne!(fp, shape_fingerprint(&base, 100.0, 60.0));
    }

    #[test]
    fn same_color_and_attrs_merge_into_one_run() {
        let white = Srgb {
            r: 255,
            g: 255,
            b: 255,
        };
        let rows = vec![vec![
            cell('a', white, false, false),
            cell('b', white, false, false),
        ]];
        let runs = text_runs(&rows);
        assert_eq!(runs.len(), 1);
        assert_eq!(runs[0].text, "ab");
    }

    #[test]
    fn a_weight_change_splits_the_run() {
        let white = Srgb {
            r: 255,
            g: 255,
            b: 255,
        };
        let rows = vec![vec![
            cell('a', white, false, false),
            cell('b', white, true, false),
        ]];
        let runs = text_runs(&rows);
        assert_eq!(runs.len(), 2);
        assert_eq!(runs[0].text, "a");
        assert!(!runs[0].bold);
        assert_eq!(runs[1].text, "b");
        assert!(runs[1].bold);
    }

    #[test]
    fn rows_are_separated_by_a_newline_run() {
        let white = Srgb {
            r: 255,
            g: 255,
            b: 255,
        };
        let rows = vec![
            vec![cell('a', white, false, false)],
            vec![cell('b', white, false, false)],
        ];
        let runs = text_runs(&rows);
        // "a", "\n", "b"
        assert_eq!(runs.len(), 3);
        assert_eq!(runs[1].text, "\n");
    }
}
