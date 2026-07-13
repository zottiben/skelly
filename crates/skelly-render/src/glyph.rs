//! A fixed-metric **glyph atlas** renderer for the terminal cell grid.
//!
//! The old path shaped the whole grid through `cosmic-text` every frame - fine for a static
//! prompt, but ~50ms/frame under a full-screen repaint (vim scroll/edit/syntax), which shaping
//! makes O(grid). A terminal is a monospace grid, so shaping per frame is unnecessary: rasterize
//! each glyph **once** into a GPU texture atlas and draw cells as instanced textured quads at fixed
//! `col*cell_w, row*cell_h` positions. A full repaint then costs "build ~N quad instances + one
//! draw" - sub-millisecond regardless of content (the alacritty/kitty/wezterm approach). Chrome /
//! proportional text keeps `glyphon` (it shapes rarely).
//!
//! A glyph is keyed by `(char, bold, italic)` and shaped **once** (which resolves font fallback for
//! emoji / Nerd glyphs) to get its `cosmic-text` cache key, then rasterized via `SwashCache`. Mask
//! (coverage) glyphs are stored white-with-alpha and tinted by the cell's fg in the shader; color
//! glyphs (emoji) are stored RGBA and drawn untinted.
//!
//! The atlas is **dynamic**: it starts small and *grows* (up to the device's max 2D texture size) by
//! copying itself into a larger texture when a glyph no longer fits, and *evicts* (clears its whole
//! history) if even a max-size atlas overflows. Instance UVs are stored in **atlas pixels** and
//! normalized in the shader by the current atlas size, so a grow - which preserves every glyph's
//! pixel position - never invalidates an instance already built this frame; the one eviction path
//! runs at frame start, before any instance is built. See [`Atlas::grow`] / [`GlyphLayer::set_panes`].

#![allow(
    clippy::cast_precision_loss,
    clippy::cast_possible_truncation,
    clippy::cast_sign_loss,
    reason = "atlas/surface dimensions, glyph placements, and instance counts are small, non-negative pixel values"
)]

use std::collections::HashMap;

use glyphon::cosmic_text::{
    Attrs, Buffer, Family, FontSystem, Shaping, Style, SwashCache, SwashContent, Weight,
};

use crate::GridCell;

/// The atlas texture's initial square side (px). A coding session's Latin glyph set fits here; the
/// atlas [`grow`](Atlas::grow)s from this on demand (doubling, capped at the device max) as a
/// multilingual / emoji-heavy session accumulates more distinct glyphs, so this is a starting size,
/// not a ceiling.
const INITIAL_ATLAS_SIZE: u32 = 1024;
/// Padding (px) between packed glyphs, so linear sampling never bleeds a neighbor.
const GLYPH_PAD: u32 = 1;

/// One glyph quad instance handed to the GPU (all physical px / normalized uv).
#[repr(C)]
#[derive(Clone, Copy, bytemuck::Pod, bytemuck::Zeroable)]
struct GlyphInstance {
    /// Destination rect: `x, y, w, h` (physical px).
    rect: [f32; 4],
    /// Atlas sub-rect in **atlas pixels**: `u0, v0, u1, v1`. The shader normalizes by the current
    /// atlas size (a uniform), so a grow that preserves pixel positions never restates this.
    uv: [f32; 4],
    /// Tint color for a mask glyph (linear rgba); ignored for a color glyph.
    color: [f32; 4],
    /// `x` = 1.0 for a color glyph (sample the atlas as-is), 0.0 for a mask (tint by `color`).
    flags: [f32; 4],
}

/// The four `vec4<f32>` instance attributes (rect, uv, color, flags), matching `GlyphInstance`.
const INSTANCE_ATTRS: [wgpu::VertexAttribute; 4] =
    wgpu::vertex_attr_array![0 => Float32x4, 1 => Float32x4, 2 => Float32x4, 3 => Float32x4];

/// A cached glyph's atlas location + placement relative to the pen (baseline origin), in px.
#[derive(Clone, Copy)]
struct GlyphEntry {
    /// Atlas sub-rect in **atlas pixels**: `u0, v0, u1, v1`. Pixel-space (not normalized) so a
    /// grow - which keeps every glyph at the same pixel position - leaves it valid untouched.
    uv: [f32; 4],
    /// Left bearing: x offset from the pen to the bitmap's left edge.
    left: f32,
    /// Top bearing: y distance from the baseline **up** to the bitmap's top edge.
    top: f32,
    width: f32,
    height: f32,
    /// A color (emoji) glyph, drawn untinted.
    color: bool,
}

/// A dynamic glyph atlas: an RGBA texture packed with a simple shelf allocator, plus a
/// `(char, bold, italic) -> Option<GlyphEntry>` cache (`None` = a blank/whitespace glyph, cached so
/// it isn't re-probed). The texture [`grow`](Atlas::grow)s on demand up to `max_dim`.
struct Atlas {
    texture: wgpu::Texture,
    view: wgpu::TextureView,
    /// Current texture side (px). Starts at `INITIAL_ATLAS_SIZE`, doubles on `grow` up to `max_dim`.
    size: u32,
    /// The device's max 2D texture side - the growth ceiling.
    max_dim: u32,
    cursor_x: u32,
    cursor_y: u32,
    shelf_h: u32,
    cache: HashMap<(char, bool, bool), Option<GlyphEntry>>,
    /// Set when a glyph could not be placed even at `max_dim` (a genuine overflow of the whole
    /// distinct-glyph history). The next `set_panes` resets the atlas at frame start to reclaim it.
    overflowed: bool,
    /// Set when the atlas must be reset on the next `set_panes` but the caller had no device (a
    /// font-size / DPI change via `reset_atlas`); coalesced with `overflowed` at frame start.
    pending_reset: bool,
    /// Set when `reset`/`grow` replaced the texture, so `GlyphLayer` rebuilds its bind group (new
    /// view) before the next draw.
    view_dirty: bool,
}

impl Atlas {
    fn new(device: &wgpu::Device) -> Self {
        let max_dim = device
            .limits()
            .max_texture_dimension_2d
            .max(INITIAL_ATLAS_SIZE);
        let size = INITIAL_ATLAS_SIZE.min(max_dim);
        let texture = Self::create_texture(device, size);
        let view = texture.create_view(&wgpu::TextureViewDescriptor::default());
        Self {
            texture,
            view,
            size,
            max_dim,
            // Start one pad in from the top-left so the first glyph also has an empty margin on its
            // left/top (see the note on `pack`).
            cursor_x: GLYPH_PAD,
            cursor_y: GLYPH_PAD,
            shelf_h: 0,
            cache: HashMap::new(),
            overflowed: false,
            pending_reset: false,
            view_dirty: false,
        }
    }

    /// Create a `size`x`size` atlas texture. `COPY_SRC` lets a later [`grow`](Atlas::grow) copy this
    /// texture into a larger one.
    fn create_texture(device: &wgpu::Device, size: u32) -> wgpu::Texture {
        device.create_texture(&wgpu::TextureDescriptor {
            label: Some("glyph-atlas"),
            size: wgpu::Extent3d {
                width: size,
                height: size,
                depth_or_array_layers: 1,
            },
            mip_level_count: 1,
            sample_count: 1,
            dimension: wgpu::TextureDimension::D2,
            // sRGB so a *color* glyph's display-encoded RGB decodes to linear on sample and
            // round-trips cleanly to the sRGB target (no double-encode / washed-out emoji). A
            // *mask* glyph is unaffected: its RGB is an ignored constant white and its coverage
            // lives in the (always-linear) alpha channel, which the sRGB curve never touches.
            format: wgpu::TextureFormat::Rgba8UnormSrgb,
            usage: wgpu::TextureUsages::TEXTURE_BINDING
                | wgpu::TextureUsages::COPY_DST
                | wgpu::TextureUsages::COPY_SRC,
            view_formats: &[],
        })
    }

    /// Reset the atlas to empty: every glyph is stale after a font-size / DPI change, and a full-atlas
    /// eviction reclaims the whole distinct-glyph history. Recreate a **fresh, zero-initialized**
    /// texture (rather than reusing the current one) so re-rasterized glyphs sample zero in their pad
    /// margins - reusing the old texture would leave stale texels there and corrupt the new glyphs'
    /// edge anti-aliasing. The new texture means a new view, so flag a bind-group rebuild.
    fn reset(&mut self, device: &wgpu::Device) {
        self.texture = Self::create_texture(device, self.size);
        self.view = self
            .texture
            .create_view(&wgpu::TextureViewDescriptor::default());
        self.cursor_x = GLYPH_PAD;
        self.cursor_y = GLYPH_PAD;
        self.shelf_h = 0;
        self.cache.clear();
        self.overflowed = false;
        self.pending_reset = false;
        self.view_dirty = true;
    }

    /// Grow the atlas to the next size (double, capped at `max_dim`) by copying the current texture
    /// into a larger one at the same pixel origin, so every packed glyph keeps its position (and
    /// thus every pixel-space UV stays valid). Returns `false` when already at `max_dim`.
    fn grow(&mut self, device: &wgpu::Device, queue: &wgpu::Queue) -> bool {
        let new_size = (self.size.saturating_mul(2)).min(self.max_dim);
        if new_size <= self.size {
            return false; // already at the device ceiling
        }
        let new_texture = Self::create_texture(device, new_size);
        let mut encoder = device.create_command_encoder(&wgpu::CommandEncoderDescriptor {
            label: Some("atlas-grow"),
        });
        encoder.copy_texture_to_texture(
            wgpu::TexelCopyTextureInfo {
                texture: &self.texture,
                mip_level: 0,
                origin: wgpu::Origin3d::ZERO,
                aspect: wgpu::TextureAspect::All,
            },
            wgpu::TexelCopyTextureInfo {
                texture: &new_texture,
                mip_level: 0,
                origin: wgpu::Origin3d::ZERO,
                aspect: wgpu::TextureAspect::All,
            },
            wgpu::Extent3d {
                width: self.size,
                height: self.size,
                depth_or_array_layers: 1,
            },
        );
        queue.submit(Some(encoder.finish()));
        self.view = new_texture.create_view(&wgpu::TextureViewDescriptor::default());
        self.texture = new_texture;
        self.size = new_size;
        self.view_dirty = true;
        true
    }

    /// Take the "the texture was replaced by a grow" flag (so `GlyphLayer` rebuilds its bind group).
    fn take_view_dirty(&mut self) -> bool {
        std::mem::take(&mut self.view_dirty)
    }

    /// Reserve a `w`x`h` slot via a shelf packer, leaving a `GLYPH_PAD` empty margin on **every**
    /// side of the glyph, or `None` when the current texture is full (the caller grows and retries).
    ///
    /// The all-sides margin is load-bearing, not just anti-bleed: the sampler clamps to edge, so a
    /// glyph flush against the atlas's `0` row/column would sample its own edge texel there instead
    /// of an empty one, giving position-dependent edge anti-aliasing. With a pad on every side, a
    /// glyph renders identically wherever it lands - which is also what makes a `grow` (that
    /// repacks nothing but relocates the usable region) pixel-for-pixel stable. Computes the slot
    /// locally and only commits the cursor on success, so a failed pack never advances state.
    fn pack(&mut self, w: u32, h: u32) -> Option<(u32, u32)> {
        // A glyph needs its own `w`x`h` plus a pad on all sides: pad + glyph + pad along each axis.
        if w + 2 * GLYPH_PAD > self.size || h + 2 * GLYPH_PAD > self.size {
            return None;
        }
        let (mut x, mut y, mut shelf_height) = (self.cursor_x, self.cursor_y, self.shelf_h);
        // The glyph plus its right/bottom pad must stay inside the texture.
        if x + w + GLYPH_PAD > self.size {
            // Next shelf: drop below the current one, back to the left pad column.
            y += shelf_height;
            x = GLYPH_PAD;
            shelf_height = 0;
        }
        if y + h + GLYPH_PAD > self.size {
            return None; // full - no state was mutated
        }
        self.cursor_x = x + w + GLYPH_PAD;
        self.cursor_y = y;
        self.shelf_h = shelf_height.max(h + GLYPH_PAD);
        Some((x, y))
    }

    /// The cached glyph for `(ch, bold, italic)`, rasterizing + packing it on first sight. `None`
    /// for a blank glyph (space, zero-size, or a genuine max-atlas overflow).
    #[allow(clippy::too_many_arguments, reason = "one glyph-cache hot path")]
    fn glyph(
        &mut self,
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        font_system: &mut FontSystem,
        swash: &mut SwashCache,
        probe: &mut Buffer,
        family: Family,
        key: (char, bool, bool),
    ) -> Option<GlyphEntry> {
        if let Some(entry) = self.cache.get(&key) {
            return *entry;
        }
        let entry = self.rasterize(device, queue, font_system, swash, probe, family, key);
        self.cache.insert(key, entry);
        entry
    }

    #[allow(clippy::too_many_arguments, reason = "one glyph rasterizer")]
    fn rasterize(
        &mut self,
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        font_system: &mut FontSystem,
        swash: &mut SwashCache,
        probe: &mut Buffer,
        family: Family,
        key: (char, bool, bool),
    ) -> Option<GlyphEntry> {
        let (ch, bold, italic) = key;
        if ch == ' ' || ch.is_whitespace() || ch == '\0' {
            return None;
        }
        // Shape the single char (resolves fallback for emoji / Nerd glyphs) to get its cache key.
        let mut attrs = Attrs::new().family(family);
        if bold {
            attrs = attrs.weight(Weight::BOLD);
        }
        if italic {
            attrs = attrs.style(Style::Italic);
        }
        let mut buf = [0u8; 4];
        probe.set_text(ch.encode_utf8(&mut buf), &attrs, Shaping::Advanced, None);
        probe.shape_until_scroll(font_system, false);
        let run = probe.layout_runs().next()?;
        let glyph = run.glyphs.first()?;
        // Integer pen -> subpixel bin 0, so a glyph rasterizes at exactly one size (stable cache).
        let physical = glyph.physical((0.0, 0.0), 1.0);
        let image = swash.get_image_uncached(font_system, physical.cache_key)?;
        let w = image.placement.width;
        let h = image.placement.height;
        if w == 0 || h == 0 {
            return None;
        }
        // Convert to RGBA: mask coverage -> white with alpha; color -> as-is.
        let (rgba, color) = match image.content {
            SwashContent::Mask => {
                let mut px = Vec::with_capacity((w * h * 4) as usize);
                for &a in &image.data {
                    px.extend_from_slice(&[255, 255, 255, a]);
                }
                (px, false)
            }
            SwashContent::SubpixelMask => {
                // 3 bytes/px subpixel coverage; average to a grayscale alpha.
                let mut px = Vec::with_capacity((w * h * 4) as usize);
                for chunk in image.data.chunks_exact(3) {
                    let a = ((u16::from(chunk[0]) + u16::from(chunk[1]) + u16::from(chunk[2])) / 3)
                        as u8;
                    px.extend_from_slice(&[255, 255, 255, a]);
                }
                (px, false)
            }
            SwashContent::Color => (image.data.clone(), true),
        };
        // Reserve a slot, growing the atlas as needed. A genuine overflow (no fit even at the device
        // max) flags the atlas for a frame-start eviction and renders this glyph blank for now.
        let (ax, ay) = loop {
            if let Some(slot) = self.pack(w, h) {
                break slot;
            }
            if !self.grow(device, queue) {
                self.overflowed = true;
                return None;
            }
        };
        queue.write_texture(
            wgpu::TexelCopyTextureInfo {
                texture: &self.texture,
                mip_level: 0,
                origin: wgpu::Origin3d { x: ax, y: ay, z: 0 },
                aspect: wgpu::TextureAspect::All,
            },
            &rgba,
            wgpu::TexelCopyBufferLayout {
                offset: 0,
                bytes_per_row: Some(w * 4),
                rows_per_image: Some(h),
            },
            wgpu::Extent3d {
                width: w,
                height: h,
                depth_or_array_layers: 1,
            },
        );
        // Pixel-space UV: the shader normalizes by the live atlas size, so a later grow (which keeps
        // this glyph's pixel position) leaves the stored + already-instanced UV valid.
        Some(GlyphEntry {
            uv: [ax as f32, ay as f32, (ax + w) as f32, (ay + h) as f32],
            left: image.placement.left as f32,
            top: image.placement.top as f32,
            width: w as f32,
            height: h as f32,
            color,
        })
    }
}

/// One pane's instance range + scissor rect, so each pane clips to its own rectangle.
struct PaneRange {
    start: u32,
    end: u32,
    clip: (f32, f32, f32, f32),
}

/// The glyph-grid render layer: an instanced textured-quad pipeline over the atlas.
pub(crate) struct GlyphLayer {
    pipeline: wgpu::RenderPipeline,
    sampler: wgpu::Sampler,
    bind_layout: wgpu::BindGroupLayout,
    bind_group: wgpu::BindGroup,
    uniforms: wgpu::Buffer,
    instances: wgpu::Buffer,
    capacity: u64,
    atlas: Atlas,
    cpu: Vec<GlyphInstance>,
    ranges: Vec<PaneRange>,
    surface: (u32, u32),
}

/// One pane's grid + placement for [`GlyphLayer::set_panes`].
pub(crate) struct GlyphPaneInput<'a> {
    pub rows: &'a [Vec<GridCell>],
    /// Pixel top-left of cell `(0,0)` (physical px).
    pub left: f32,
    pub top: f32,
    /// Clip rect `(x, y, w, h)` (physical px).
    pub clip: (f32, f32, f32, f32),
}

impl GlyphLayer {
    pub(crate) fn new(device: &wgpu::Device, format: wgpu::TextureFormat) -> Self {
        let shader = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("glyph-shader"),
            source: wgpu::ShaderSource::Wgsl(SHADER.into()),
        });
        let atlas = Atlas::new(device);
        let bind_layout = Self::bind_group_layout(device);
        let uniforms = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("glyph-uniforms"),
            size: 16,
            usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });
        let sampler = device.create_sampler(&wgpu::SamplerDescriptor {
            label: Some("glyph-sampler"),
            mag_filter: wgpu::FilterMode::Linear,
            min_filter: wgpu::FilterMode::Linear,
            ..Default::default()
        });
        let bind_group =
            Self::make_bind_group(device, &bind_layout, &uniforms, &atlas.view, &sampler);
        let pipeline_layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
            label: Some("glyph-pipeline-layout"),
            bind_group_layouts: &[Some(&bind_layout)],
            immediate_size: 0,
        });
        let pipeline = device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
            label: Some("glyph-pipeline"),
            layout: Some(&pipeline_layout),
            vertex: wgpu::VertexState {
                module: &shader,
                entry_point: Some("vs_main"),
                buffers: &[Some(wgpu::VertexBufferLayout {
                    array_stride: std::mem::size_of::<GlyphInstance>() as u64,
                    step_mode: wgpu::VertexStepMode::Instance,
                    attributes: &INSTANCE_ATTRS,
                })],
                compilation_options: wgpu::PipelineCompilationOptions::default(),
            },
            fragment: Some(wgpu::FragmentState {
                module: &shader,
                entry_point: Some("fs_main"),
                targets: &[Some(wgpu::ColorTargetState {
                    format,
                    blend: Some(wgpu::BlendState::ALPHA_BLENDING),
                    write_mask: wgpu::ColorWrites::ALL,
                })],
                compilation_options: wgpu::PipelineCompilationOptions::default(),
            }),
            primitive: wgpu::PrimitiveState::default(),
            depth_stencil: None,
            multisample: wgpu::MultisampleState::default(),
            multiview_mask: None,
            cache: None,
        });
        let capacity = 4096;
        let instances = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("glyph-instances"),
            size: capacity * std::mem::size_of::<GlyphInstance>() as u64,
            usage: wgpu::BufferUsages::VERTEX | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });
        Self {
            pipeline,
            sampler,
            bind_layout,
            bind_group,
            uniforms,
            instances,
            capacity,
            atlas,
            cpu: Vec::new(),
            ranges: Vec::new(),
            surface: (0, 0),
        }
    }

    /// The layout for the three bindings: the size uniform (vertex), the atlas texture, and its
    /// sampler (both fragment).
    fn bind_group_layout(device: &wgpu::Device) -> wgpu::BindGroupLayout {
        device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("glyph-bind-layout"),
            entries: &[
                wgpu::BindGroupLayoutEntry {
                    binding: 0,
                    visibility: wgpu::ShaderStages::VERTEX,
                    ty: wgpu::BindingType::Buffer {
                        ty: wgpu::BufferBindingType::Uniform,
                        has_dynamic_offset: false,
                        min_binding_size: None,
                    },
                    count: None,
                },
                wgpu::BindGroupLayoutEntry {
                    binding: 1,
                    visibility: wgpu::ShaderStages::FRAGMENT,
                    ty: wgpu::BindingType::Texture {
                        sample_type: wgpu::TextureSampleType::Float { filterable: true },
                        view_dimension: wgpu::TextureViewDimension::D2,
                        multisampled: false,
                    },
                    count: None,
                },
                wgpu::BindGroupLayoutEntry {
                    binding: 2,
                    visibility: wgpu::ShaderStages::FRAGMENT,
                    ty: wgpu::BindingType::Sampler(wgpu::SamplerBindingType::Filtering),
                    count: None,
                },
            ],
        })
    }

    fn make_bind_group(
        device: &wgpu::Device,
        layout: &wgpu::BindGroupLayout,
        uniforms: &wgpu::Buffer,
        atlas_view: &wgpu::TextureView,
        sampler: &wgpu::Sampler,
    ) -> wgpu::BindGroup {
        device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("glyph-bind-group"),
            layout,
            entries: &[
                wgpu::BindGroupEntry {
                    binding: 0,
                    resource: uniforms.as_entire_binding(),
                },
                wgpu::BindGroupEntry {
                    binding: 1,
                    resource: wgpu::BindingResource::TextureView(atlas_view),
                },
                wgpu::BindGroupEntry {
                    binding: 2,
                    resource: wgpu::BindingResource::Sampler(sampler),
                },
            ],
        })
    }

    /// Reset the glyph atlas (after a font-size / DPI change invalidates every rasterized glyph).
    /// Deferred to the next `set_panes`, which has the device needed to recreate the texture.
    pub(crate) fn reset_atlas(&mut self) {
        self.atlas.pending_reset = true;
    }

    /// Build the glyph instances for every pane's grid, rasterizing any new glyphs into the atlas.
    /// `baseline` is the pen's y within a cell (distance from the cell top down to the baseline).
    /// Pure CPU + occasional atlas uploads - no shaping of already-cached glyphs, so a full-grid
    /// change stays cheap.
    #[allow(clippy::too_many_arguments, reason = "one grid-instance builder")]
    pub(crate) fn set_panes(
        &mut self,
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        font_system: &mut FontSystem,
        swash: &mut SwashCache,
        probe: &mut Buffer,
        family: Family,
        cell_w: f32,
        cell_h: f32,
        baseline: f32,
        panes: &[GlyphPaneInput],
    ) {
        self.cpu.clear();
        self.ranges.clear();
        // Frame-start eviction: if a previous frame overflowed even a max-size atlas, reclaim the
        // whole distinct-glyph history now - before any instance is built - so the clear can never
        // pull the atlas out from under a UV already placed this frame.
        if self.atlas.overflowed || self.atlas.pending_reset {
            self.atlas.reset(device);
        }
        for pane in panes {
            let start = self.cpu.len() as u32;
            for (row_index, row) in pane.rows.iter().enumerate() {
                let cell_top = pane.top + row_index as f32 * cell_h;
                let base_y = cell_top + baseline;
                for (col, cell) in row.iter().enumerate() {
                    if cell.c == ' ' || cell.c == '\0' {
                        continue;
                    }
                    let key = (cell.c, cell.bold, cell.italic);
                    let Some(g) =
                        self.atlas
                            .glyph(device, queue, font_system, swash, probe, family, key)
                    else {
                        continue;
                    };
                    let cell_left = pane.left + col as f32 * cell_w;
                    // Tint applies only to mask glyphs; a color glyph (emoji) ignores it.
                    self.cpu.push(GlyphInstance {
                        rect: [cell_left + g.left, base_y - g.top, g.width, g.height],
                        uv: g.uv,
                        color: cell.fg.to_linear(),
                        flags: [f32::from(u8::from(g.color)), 0.0, 0.0, 0.0],
                    });
                }
            }
            self.ranges.push(PaneRange {
                start,
                end: self.cpu.len() as u32,
                clip: pane.clip,
            });
        }
        // Grow the instance buffer if needed (rebuilds the bind group unaffected).
        let needed = self.cpu.len() as u64;
        if needed > self.capacity {
            self.capacity = needed.next_power_of_two();
            self.instances = device.create_buffer(&wgpu::BufferDescriptor {
                label: Some("glyph-instances"),
                size: self.capacity * std::mem::size_of::<GlyphInstance>() as u64,
                usage: wgpu::BufferUsages::VERTEX | wgpu::BufferUsages::COPY_DST,
                mapped_at_creation: false,
            });
        }
        if !self.cpu.is_empty() {
            queue.write_buffer(&self.instances, 0, bytemuck::cast_slice(&self.cpu));
        }
        // A grow during this build replaced the atlas texture (and its view); rebuild the bind group
        // so the draw samples the new texture. Safe to do here: the loop above never draws, and every
        // instance's pixel-space UV survived the grow (positions were preserved).
        if self.atlas.take_view_dirty() {
            self.bind_group = Self::make_bind_group(
                device,
                &self.bind_layout,
                &self.uniforms,
                &self.atlas.view,
                &self.sampler,
            );
        }
    }

    /// Record the surface size (for the uniform's clip-space conversion).
    pub(crate) fn resize(&mut self, width: u32, height: u32) {
        self.surface = (width, height);
    }

    /// Draw every pane's glyphs, clipping each to its own rect, loading over the cell backgrounds.
    /// Infallible (unlike the glyphon path, there is no per-frame shaping/prepare that can fail):
    /// the atlas is pre-populated and this only records + submits a draw.
    pub(crate) fn draw(
        &mut self,
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        view: &wgpu::TextureView,
    ) {
        if self.ranges.is_empty() || self.cpu.is_empty() {
            return;
        }
        let (w, h) = self.surface;
        // Uniform: surface size (clip-space) + atlas size (UV normalization). The atlas may have
        // grown during this frame's `set_panes`, so publish its current size here.
        let atlas = self.atlas.size as f32;
        queue.write_buffer(
            &self.uniforms,
            0,
            bytemuck::cast_slice(&[w as f32, h as f32, atlas, atlas]),
        );
        // Rebuild the bind group each draw is unnecessary; keep the persistent one.
        let mut encoder = device.create_command_encoder(&wgpu::CommandEncoderDescriptor {
            label: Some("glyph"),
        });
        {
            let mut pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
                label: Some("glyph-pass"),
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
            pass.set_pipeline(&self.pipeline);
            pass.set_bind_group(0, &self.bind_group, &[]);
            pass.set_vertex_buffer(0, self.instances.slice(..));
            for range in &self.ranges {
                if range.end <= range.start {
                    continue;
                }
                let (cx, cy, cw, ch) = range.clip;
                let (sx, sy) = (cx.max(0.0) as u32, cy.max(0.0) as u32);
                let sw = (cw.max(0.0) as u32).min(w.saturating_sub(sx));
                let sh = (ch.max(0.0) as u32).min(h.saturating_sub(sy));
                if sw == 0 || sh == 0 {
                    continue;
                }
                pass.set_scissor_rect(sx, sy, sw, sh);
                pass.draw(0..6, range.start..range.end);
            }
        }
        queue.submit(Some(encoder.finish()));
    }
}

const SHADER: &str = r"
// `surface` = target size (px), for clip-space; `atlas` = current atlas size (px), for UV
// normalization (the atlas can grow, so instance UVs are stored in atlas pixels).
struct Uniforms { surface: vec2<f32>, atlas: vec2<f32> };
@group(0) @binding(0) var<uniform> u: Uniforms;
@group(0) @binding(1) var atlas: texture_2d<f32>;
@group(0) @binding(2) var samp: sampler;

struct Inst {
    @location(0) rect: vec4<f32>,
    @location(1) uv: vec4<f32>,
    @location(2) color: vec4<f32>,
    @location(3) flags: vec4<f32>,
};
struct VOut {
    @builtin(position) pos: vec4<f32>,
    @location(0) uv: vec2<f32>,
    @location(1) color: vec4<f32>,
    @location(2) @interpolate(flat) is_color: f32,
};

@vertex
fn vs_main(@builtin(vertex_index) vi: u32, inst: Inst) -> VOut {
    var corners = array<vec2<f32>, 6>(
        vec2<f32>(0.0, 0.0), vec2<f32>(1.0, 0.0), vec2<f32>(0.0, 1.0),
        vec2<f32>(0.0, 1.0), vec2<f32>(1.0, 0.0), vec2<f32>(1.0, 1.0),
    );
    let corner = corners[vi];
    let px = inst.rect.xy + corner * inst.rect.zw;
    let ndc = vec2<f32>(px.x / u.surface.x * 2.0 - 1.0, 1.0 - px.y / u.surface.y * 2.0);
    // Instance UVs are in atlas pixels; normalize by the live atlas size.
    let uv_px = inst.uv.xy + corner * (inst.uv.zw - inst.uv.xy);
    let uv = uv_px / u.atlas;
    var out: VOut;
    out.pos = vec4<f32>(ndc, 0.0, 1.0);
    out.uv = uv;
    out.color = inst.color;
    out.is_color = inst.flags.x;
    return out;
}

@fragment
fn fs_main(in: VOut) -> @location(0) vec4<f32> {
    let texel = textureSample(atlas, samp, in.uv);
    if (in.is_color > 0.5) {
        return texel; // color glyph (emoji): premultiplied-ish RGBA as rasterized
    }
    // Mask glyph: the atlas holds coverage in alpha; tint by the cell's fg color.
    return vec4<f32>(in.color.rgb, in.color.a * texel.a);
}
";
