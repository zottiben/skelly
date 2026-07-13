//! A minimal instanced colored-quad pipeline, used to paint per-cell backgrounds
//! and the cursor beneath the text. Positions are in physical pixels; the vertex
//! shader converts them to clip space using the surface size uniform, and colors
//! are linear (the surface is sRGB).

#![allow(
    clippy::cast_precision_loss,
    clippy::cast_possible_truncation,
    clippy::cast_sign_loss,
    reason = "surface dimensions and instance/step counts are small, non-negative, exact"
)]

/// One instanced quad: a pixel rectangle, a linear RGBA fill, and shape params.
///
/// `params` is `[radius, blur, diamond, _]` in physical pixels and drives the fragment
/// shader: all-zero is a plain sharp fill (every cell background, divider, cursor,
/// underline, and flush panel - identical to the old flat pipeline); `radius > 0` rounds
/// the corners with a signed-distance box and a 1px anti-aliased edge (floating panels);
/// `blur > 0` marks the quad a soft drop shadow, its coverage feathered over `blur` px
/// around the inset panel box (the guide's elevation tokens); `diamond > 0` marks a
/// rounded square rotated 45° (a "diamond" disc of the vertebra logo, §02).
#[repr(C)]
#[derive(Clone, Copy, bytemuck::Pod, bytemuck::Zeroable)]
pub(crate) struct Quad {
    /// `x, y, w, h` in physical pixels.
    rect: [f32; 4],
    /// Linear RGBA.
    color: [f32; 4],
    /// `[radius, blur, diamond, _]` in physical pixels (see the type docs).
    params: [f32; 4],
}

/// A drop-shadow spec in *logical* px, matching the guide's elevation tokens (§03).
/// Every guide shadow has a zero horizontal offset, so only the vertical offset `dy`,
/// the `blur` radius, and the black `alpha` vary.
#[derive(Clone, Copy)]
pub(crate) struct Shadow {
    dy: f32,
    blur: f32,
    alpha: f32,
}

/// `e4` - command palette, modals. (The lighter `e2`/`e3` tokens land with the dock /
/// tooltip surfaces that use them.)
pub(crate) const SHADOW_E4: Shadow = Shadow {
    dy: 16.0,
    blur: 48.0,
    alpha: 0.52,
};

/// Corner radius (logical px) for floating panels / the palette / panes (the guide's
/// `lg` radius token).
pub(crate) const RADIUS_LG: f32 = 10.0;

impl Quad {
    /// A sharp-cornered quad at pixel `(x, y)` of size `(w, h)` filled with linear
    /// `color` - the flat fill used for cell backgrounds, dividers, cursors, and
    /// flush surfaces (`params` all zero, so the shader returns the fill directly).
    pub(crate) fn new(x: f32, y: f32, w: f32, h: f32, color: [f32; 4]) -> Self {
        Self {
            rect: [x, y, w, h],
            color,
            params: [0.0, 0.0, 0.0, 0.0],
        }
    }

    /// A quad with anti-aliased rounded corners of `radius` physical px (clamped to
    /// half the shorter side so the corners never overlap). Used for floating panels
    /// and list-row pills.
    pub(crate) fn rounded(x: f32, y: f32, w: f32, h: f32, color: [f32; 4], radius: f32) -> Self {
        let radius = radius.clamp(0.0, w.min(h) * 0.5);
        Self {
            rect: [x, y, w, h],
            color,
            params: [radius, 0.0, 0.0, 0.0],
        }
    }

    /// A rounded-square "diamond" disc: a square of side `size` (physical px) rotated 45°
    /// about `(cx, cy)`, its corners rounded by `radius` px in the pre-rotation (axis-aligned)
    /// frame - the vertebra logo's discs (the guide's §02 mark). The quad's rect is the
    /// diamond's axis-aligned bounding box (side `size·√2`); the fragment shader rotates the
    /// sample point 45° back into that frame and evaluates the rounded-box SDF at half-size
    /// `size/2` (the AABB half divided by √2). `params.z == 1` selects that shader branch.
    pub(crate) fn diamond(cx: f32, cy: f32, size: f32, color: [f32; 4], radius: f32) -> Self {
        let aabb = size * std::f32::consts::SQRT_2;
        Self {
            rect: [cx - aabb * 0.5, cy - aabb * 0.5, aabb, aabb],
            color,
            params: [radius, 0.0, 1.0, 0.0],
        }
    }

    /// A soft drop shadow for the rounded `panel` (physical px), following the guide's
    /// elevation `spec` at DPI `scale`, with corners matching the panel's `radius`. The
    /// quad is the panel inflated by the blur on every side and shifted down by the
    /// shadow offset; the fragment shader feathers its coverage from the panel's edge
    /// outward, so it reads as a shadow cast behind the panel (which is drawn on top).
    pub(crate) fn shadow(panel: crate::PxRect, spec: Shadow, scale: f32, radius: f32) -> Self {
        let blur = spec.blur * scale;
        let dy = spec.dy * scale;
        Self {
            rect: [
                panel.x - blur,
                panel.y + dy - blur,
                panel.w + 2.0 * blur,
                panel.h + 2.0 * blur,
            ],
            color: [0.0, 0.0, 0.0, spec.alpha],
            params: [radius, blur, 0.0, 0.0],
        }
    }
}

/// Convert a binary-built [`ChromeQuad`](crate::ChromeQuad) (physical-px rect, a UI-token
/// color at an alpha, an optional corner radius) into a GPU [`Quad`]. This is the paint
/// primitive proportional chrome is drawn from: the sidebar (and, as they migrate, the
/// other surfaces) hand the renderer a display list of these plus positioned prose labels.
pub(crate) fn chrome_quad(q: &crate::ChromeQuad) -> Quad {
    let mut color = q.color.to_linear();
    color[3] = q.alpha;
    let r = q.rect;
    if q.diamond {
        // The rect is the disc's square bounding box; recover its center + side for the
        // rotated-square SDF (the vertebra-logo disc path).
        Quad::diamond(r.x + r.w * 0.5, r.y + r.h * 0.5, r.w, color, q.radius)
    } else if q.radius > 0.0 {
        Quad::rounded(r.x, r.y, r.w, r.h, color, q.radius)
    } else {
        Quad::new(r.x, r.y, r.w, r.h, color)
    }
}

/// Alpha applied to the accent color for the (translucent) selection highlight.
const SELECTION_ALPHA: f32 = 0.30;

/// Alpha of the dim scrim drawn over an exited pane. High enough to read as "inactive"
/// yet translucent, so the preserved scrollback stays faintly visible beneath.
const SCRIM_ALPHA: f32 = 0.72;

/// A translucent `bg.base` scrim filling `rect`, dimming an exited pane's preserved grid.
/// Shared by the windowed [`Renderer`](crate::Renderer) and the headless capture.
pub(crate) fn scrim_quad(rect: crate::PxRect, theme: &crate::theme::Theme) -> Quad {
    let mut color = theme.bg_base.to_array();
    color[3] = SCRIM_ALPHA;
    Quad::new(rect.x, rect.y, rect.w, rect.h, color)
}

/// Build the quads for one pane's grid, given the cell metrics (physical px) and the
/// pixel position of cell `(0, 0)`'s top-left corner (`origin`), in draw order:
/// opaque cell backgrounds, then per-cell underline rules, then
/// translucent selection fills, then the accent cursor block (only when `cursor` is
/// `Some`, i.e. the focused pane). All sit *beneath* the glyphs (the text pass loads
/// over them). `selection` is the list of selected `(column, row)` cells.
#[allow(
    clippy::too_many_arguments,
    reason = "one focused per-pane quad builder; grouping the args would only obscure it"
)]
pub(crate) fn grid_quads(
    cell_w: f32,
    cell_h: f32,
    origin: (f32, f32),
    rows: &[Vec<crate::GridCell>],
    cursor: Option<(usize, usize)>,
    cursor_shape: crate::CursorShape,
    accent: crate::theme::Srgb,
    selection: &[(usize, usize)],
) -> Vec<Quad> {
    let (origin_x, origin_y) = origin;
    let cell_quad = |col: usize, row: usize, color: [f32; 4]| {
        Quad::new(
            origin_x + col as f32 * cell_w,
            origin_y + row as f32 * cell_h,
            cell_w,
            cell_h,
            color,
        )
    };

    // An underline rule: a thin bar near the cell's baseline, in the glyph color.
    let underline_thickness = (cell_h * 0.07).round().max(1.0);
    let underline_top = (cell_h - underline_thickness * 2.0).max(0.0);
    let underline_quad = |col: usize, row: usize, color: [f32; 4]| {
        Quad::new(
            origin_x + col as f32 * cell_w,
            origin_y + row as f32 * cell_h + underline_top,
            cell_w,
            underline_thickness,
            color,
        )
    };

    let mut quads = Vec::new();
    for (row_index, row) in rows.iter().enumerate() {
        for (col_index, cell) in row.iter().enumerate() {
            if let Some(bg) = cell.bg {
                quads.push(cell_quad(col_index, row_index, bg.to_linear()));
            }
        }
    }
    for (row_index, row) in rows.iter().enumerate() {
        for (col_index, cell) in row.iter().enumerate() {
            if cell.underline {
                quads.push(underline_quad(col_index, row_index, cell.fg.to_linear()));
            }
        }
    }

    let mut selection_color = accent.to_linear();
    selection_color[3] = SELECTION_ALPHA;
    for &(col, row) in selection {
        quads.push(cell_quad(col, row, selection_color));
    }

    if let Some((cursor_col, cursor_row)) = cursor {
        let x = origin_x + cursor_col as f32 * cell_w;
        let y = origin_y + cursor_row as f32 * cell_h;
        let accent = accent.to_linear();
        // Honor the program's requested cursor shape (design: match vim's per-mode cursor).
        match cursor_shape {
            crate::CursorShape::Block => quads.push(Quad::new(x, y, cell_w, cell_h, accent)),
            crate::CursorShape::Bar => {
                let bar_w = (cell_w * 0.15).max(2.0);
                quads.push(Quad::new(x, y, bar_w, cell_h, accent));
            }
            crate::CursorShape::Underline => {
                let bar_h = underline_thickness.max(2.0);
                quads.push(Quad::new(x, y + cell_h - bar_h, cell_w, bar_h, accent));
            }
            crate::CursorShape::Hidden => {}
        }
    }
    quads
}

/// Build the floating-card decoration for an overlay (the command palette or a modal): the
/// `e4` drop shadow, a rounded `border.strong` ring, and the `bg.elevated` fill inset by the
/// stroke. The overlay's content (selected-row pill, caret, text) is supplied by the binary
/// as a proportional display list drawn on top. Shared by the windowed
/// [`Renderer`](crate::Renderer) and the headless capture so both draw identically.
pub(crate) fn card_quads(
    panel: crate::PxRect,
    theme: &crate::theme::Theme,
    scale: f32,
) -> Vec<Quad> {
    let stroke = scale.max(1.0);
    let radius = RADIUS_LG * scale;
    vec![
        Quad::shadow(panel, SHADOW_E4, scale, radius),
        Quad::rounded(
            panel.x,
            panel.y,
            panel.w,
            panel.h,
            theme.border_strong.to_linear(),
            radius,
        ),
        Quad::rounded(
            panel.x + stroke,
            panel.y + stroke,
            (panel.w - 2.0 * stroke).max(0.0),
            (panel.h - 2.0 * stroke).max(0.0),
            theme.bg_elevated.to_linear(),
            (radius - stroke).max(0.0),
        ),
    ]
}

/// The vertebra logo mark's own spine opacity (the guide draws the spine at `accent`
/// opacity 0.3, beneath the mark's container opacity).
const LOGO_SPINE_ALPHA: f32 = 0.3;

/// Container opacity for the faint empty-state brand watermark (the guide's §10.2 mark,
/// `opacity:0.32`). The one place a pane paints the vertebra mark, so the value lives here
/// with the mark's other transcribed geometry.
pub(crate) const LOGO_WATERMARK_OPACITY: f32 = 0.32;

/// Build the vertebra logo mark (the guide's §02 brand mark): a faint `accent` spine (a
/// vertical rounded pill) threading three rounded-diamond discs - two small `fg.primary`
/// discs at top and bottom and a large `accent` disc at the center. `bounds` is the mark's
/// square bounding box (physical px); every quad's alpha is scaled by `opacity` (the guide's
/// container opacity, e.g. 0.32 for the faint empty-state watermark). Resolving the two-tone
/// colors from theme tokens makes the mark correct in both themes for free - on light the
/// guide's "on-light" variant falls out (accent = deep mauve, `fg.primary` = slate). The
/// disc geometry (fractions of the box) is transcribed from the mockup: spine `top 9% h 82%
/// w 6%`; discs centered horizontally at `cy` 16% / 50% / 84%, sides 26% / 42% / 26%, corner
/// radii 26% / 22% / 26%. Shared by the windowed [`Renderer`](crate::Renderer) and the
/// headless capture.
pub(crate) fn logo_quads(
    bounds: crate::PxRect,
    theme: &crate::theme::Theme,
    opacity: f32,
) -> Vec<Quad> {
    // The mark is drawn square, centered horizontally in `bounds`.
    let mark_px = bounds.w.min(bounds.h);
    let cx = bounds.x + bounds.w * 0.5;
    let tint = |c: crate::theme::Srgb, alpha: f32| {
        let mut linear = c.to_linear();
        linear[3] = alpha;
        linear
    };
    // A rounded-diamond disc: center `(cx, bounds.y + cy_frac·mark)`, square side
    // `size_frac·mark`, corners rounded by `radius_frac` of that side.
    let disc = |cy_frac: f32, size_frac: f32, radius_frac: f32, color: [f32; 4]| {
        let size = size_frac * mark_px;
        Quad::diamond(
            cx,
            bounds.y + cy_frac * mark_px,
            size,
            color,
            radius_frac * size,
        )
    };

    let spine_w = 0.06 * mark_px;
    vec![
        Quad::rounded(
            cx - spine_w * 0.5,
            bounds.y + 0.09 * mark_px,
            spine_w,
            0.82 * mark_px,
            tint(theme.accent, LOGO_SPINE_ALPHA * opacity),
            spine_w * 0.5,
        ),
        disc(0.16, 0.26, 0.26, tint(theme.fg_primary, opacity)),
        disc(0.50, 0.42, 0.22, tint(theme.accent, opacity)),
        disc(0.84, 0.26, 0.26, tint(theme.fg_primary, opacity)),
    ]
}

/// The vertebra brand mark (§02) as [`ChromeQuad`](crate::ChromeQuad)s, so an overlay (the
/// first-run modal, §10.1) can draw it in its display list. Same geometry as `logo_quads`: a spine
/// threading three rounded-diamond discs (two `fg.primary`, one `accent`), `mark` px square,
/// its top-left at `(x, y)`, every layer scaled by `opacity`.
#[must_use]
pub fn logo_chrome_quads(
    x: f32,
    y: f32,
    mark: f32,
    theme: &crate::theme::Theme,
    opacity: f32,
) -> Vec<crate::ChromeQuad> {
    use crate::ChromeQuad;
    let cx = x + mark * 0.5;
    let disc = |cy_frac: f32, size_frac: f32, radius_frac: f32, color, alpha: f32| {
        let size = size_frac * mark;
        ChromeQuad::diamond(
            cx,
            y + cy_frac * mark,
            size,
            color,
            alpha,
            radius_frac * size,
        )
    };
    let spine_w = 0.06 * mark;
    vec![
        ChromeQuad::tint(
            crate::PxRect {
                x: cx - spine_w * 0.5,
                y: y + 0.09 * mark,
                w: spine_w,
                h: 0.82 * mark,
            },
            theme.accent,
            LOGO_SPINE_ALPHA * opacity,
            spine_w * 0.5,
        ),
        disc(0.16, 0.26, 0.26, theme.fg_primary, opacity),
        disc(0.50, 0.42, 0.22, theme.accent, opacity),
        disc(0.84, 0.26, 0.26, theme.fg_primary, opacity),
    ]
}

/// Build the full-window settings frame: the `bg.elevated` content panel fill, a `bg.base`
/// fill over the left category-nav strip up to `nav_divider_x`, and the `border` divider
/// between nav and content. The active-category highlight, the focused-control highlight, and
/// all text are supplied by the binary as a proportional display list drawn on top. Shared by
/// the windowed [`Renderer`](crate::Renderer) and the headless capture.
pub(crate) fn settings_frame_quads(
    panel: crate::PxRect,
    nav_divider_x: f32,
    theme: &crate::theme::Theme,
    scale: f32,
) -> Vec<Quad> {
    let stroke = scale.max(1.0);
    vec![
        Quad::new(
            panel.x,
            panel.y,
            panel.w,
            panel.h,
            theme.bg_elevated.to_linear(),
        ),
        Quad::new(
            panel.x,
            panel.y,
            (nav_divider_x - panel.x).max(0.0),
            panel.h,
            theme.bg_base.to_array(),
        ),
        Quad::new(
            nav_divider_x,
            panel.y,
            stroke,
            panel.h,
            theme.border.to_linear(),
        ),
    ]
}

/// Build the generic right-dock frame chrome, shared by the migrated proportional docks:
/// the soft shadow the dock casts leftward onto the terminal as it slides over it, and a
/// `border` divider down the dock's left edge. The dock's content (row fills, bars, text) is
/// supplied by the binary as a proportional display list drawn on top. Shared by the
/// windowed [`Renderer`](crate::Renderer) and the headless capture.
pub(crate) fn dock_frame_quads(
    panel: crate::PxRect,
    theme: &crate::theme::Theme,
    scale: f32,
) -> Vec<Quad> {
    let stroke = scale.max(1.0);
    let mut quads = Vec::new();
    push_left_edge_shadow(&mut quads, panel.x, panel.y, panel.h, scale);
    quads.push(Quad::new(
        panel.x,
        panel.y,
        stroke,
        panel.h,
        theme.border.to_linear(),
    ));
    quads
}

/// Width (logical px) of a right-dock's left-edge shadow, matching the guide's ~6px
/// slide-over handle gradient (§07 hero).
const DOCK_EDGE_SHADOW: f32 = 8.0;
/// Peak alpha of the dock edge shadow, at the dock's edge (fading to 0 outward).
const DOCK_EDGE_SHADOW_ALPHA: f32 = 0.34;

/// Append a soft shadow cast *leftward* onto the terminal from a right-dock's left edge
/// at `edge_x` (the guide's dock "slides over the terminal" handle, §07). Unlike the SDF
/// [`Quad::shadow`] - which needs an opaque panel drawn over its interior - this is a thin
/// stack of 1px translucent-black steps sitting entirely in the terminal region just left
/// of the dock, with a quadratic falloff, so the dock itself needs no opaque fill. Drawn
/// first (beneath the dock's row fills and divider).
fn push_left_edge_shadow(quads: &mut Vec<Quad>, edge_x: f32, top: f32, height: f32, scale: f32) {
    let steps = (DOCK_EDGE_SHADOW * scale).round().max(1.0) as usize;
    for i in 0..steps {
        // i = 0 sits against the edge (darkest); farther left fades to nothing.
        let t = 1.0 - (i as f32 + 0.5) / steps as f32;
        let alpha = DOCK_EDGE_SHADOW_ALPHA * t * t;
        quads.push(Quad::new(
            edge_x - (i as f32 + 1.0),
            top,
            1.0,
            height,
            [0.0, 0.0, 0.0, alpha],
        ));
    }
}

/// Append a rectangular outline (four `thickness`-px bars) around the pixel rect
/// `(x, y, w, h)` in linear `color`, drawn *inside* the rect's edges so it never
/// escapes the pane. Used for the per-pane divider and the focused pane's ring.
pub(crate) fn push_outline(
    quads: &mut Vec<Quad>,
    x: f32,
    y: f32,
    w: f32,
    h: f32,
    thickness: f32,
    color: [f32; 4],
) {
    let stroke = thickness.min(w).min(h);
    quads.push(Quad::new(x, y, w, stroke, color)); // top
    quads.push(Quad::new(x, y + h - stroke, w, stroke, color)); // bottom
    quads.push(Quad::new(x, y, stroke, h, color)); // left
    quads.push(Quad::new(x + w - stroke, y, stroke, h, color)); // right
}

#[repr(C)]
#[derive(Clone, Copy, bytemuck::Pod, bytemuck::Zeroable)]
struct Uniforms {
    size: [f32; 2],
    _pad: [f32; 2],
}

const INSTANCE_ATTRS: [wgpu::VertexAttribute; 3] =
    wgpu::vertex_attr_array![0 => Float32x4, 1 => Float32x4, 2 => Float32x4];

/// The colored-quad render pipeline plus its dynamic instance buffer.
pub(crate) struct QuadLayer {
    pipeline: wgpu::RenderPipeline,
    uniforms: wgpu::Buffer,
    bind_group: wgpu::BindGroup,
    instances: wgpu::Buffer,
    capacity: u64,
    count: u32,
}

impl QuadLayer {
    /// Build the pipeline targeting `format`.
    pub(crate) fn new(device: &wgpu::Device, format: wgpu::TextureFormat) -> Self {
        let shader = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("quad-shader"),
            source: wgpu::ShaderSource::Wgsl(SHADER.into()),
        });

        let bind_group_layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("quad-bgl"),
            entries: &[wgpu::BindGroupLayoutEntry {
                binding: 0,
                visibility: wgpu::ShaderStages::VERTEX,
                ty: wgpu::BindingType::Buffer {
                    ty: wgpu::BufferBindingType::Uniform,
                    has_dynamic_offset: false,
                    min_binding_size: None,
                },
                count: None,
            }],
        });

        let uniforms = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("quad-uniforms"),
            size: std::mem::size_of::<Uniforms>() as u64,
            usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });

        let bind_group = device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("quad-bg"),
            layout: &bind_group_layout,
            entries: &[wgpu::BindGroupEntry {
                binding: 0,
                resource: uniforms.as_entire_binding(),
            }],
        });

        let layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
            label: Some("quad-layout"),
            bind_group_layouts: &[Some(&bind_group_layout)],
            immediate_size: 0,
        });

        let pipeline = device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
            label: Some("quad-pipeline"),
            layout: Some(&layout),
            vertex: wgpu::VertexState {
                module: &shader,
                entry_point: Some("vs_main"),
                compilation_options: wgpu::PipelineCompilationOptions::default(),
                buffers: &[Some(wgpu::VertexBufferLayout {
                    array_stride: std::mem::size_of::<Quad>() as u64,
                    step_mode: wgpu::VertexStepMode::Instance,
                    attributes: &INSTANCE_ATTRS,
                })],
            },
            fragment: Some(wgpu::FragmentState {
                module: &shader,
                entry_point: Some("fs_main"),
                compilation_options: wgpu::PipelineCompilationOptions::default(),
                targets: &[Some(wgpu::ColorTargetState {
                    format,
                    blend: Some(wgpu::BlendState::ALPHA_BLENDING),
                    write_mask: wgpu::ColorWrites::ALL,
                })],
            }),
            primitive: wgpu::PrimitiveState::default(),
            depth_stencil: None,
            multisample: wgpu::MultisampleState::default(),
            multiview_mask: None,
            cache: None,
        });

        let instances = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("quad-instances"),
            size: 0,
            usage: wgpu::BufferUsages::VERTEX | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });

        Self {
            pipeline,
            uniforms,
            bind_group,
            instances,
            capacity: 0,
            count: 0,
        }
    }

    /// Upload the quads to draw, sized for a `surface_width` x `surface_height`
    /// target. Grows the instance buffer as needed.
    pub(crate) fn set(
        &mut self,
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        surface_width: u32,
        surface_height: u32,
        quads: &[Quad],
    ) {
        queue.write_buffer(
            &self.uniforms,
            0,
            bytemuck::bytes_of(&Uniforms {
                size: [surface_width as f32, surface_height as f32],
                _pad: [0.0, 0.0],
            }),
        );

        self.count = quads.len() as u32;
        if quads.is_empty() {
            return;
        }
        let bytes = bytemuck::cast_slice(quads);
        let needed = bytes.len() as u64;
        if needed > self.capacity {
            self.instances = device.create_buffer(&wgpu::BufferDescriptor {
                label: Some("quad-instances"),
                size: needed,
                usage: wgpu::BufferUsages::VERTEX | wgpu::BufferUsages::COPY_DST,
                mapped_at_creation: false,
            });
            self.capacity = needed;
        }
        queue.write_buffer(&self.instances, 0, bytes);
    }

    /// Draw the uploaded quads onto `view`. When `clear` is `Some`, the pass first
    /// clears to that color (the terminal quad pass); when `None`, it loads the
    /// existing contents and draws over them (the overlay quad pass). Submits its own
    /// command buffer.
    pub(crate) fn draw(
        &self,
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        view: &wgpu::TextureView,
        clear: Option<wgpu::Color>,
    ) {
        let mut encoder = device.create_command_encoder(&wgpu::CommandEncoderDescriptor {
            label: Some("quad-encoder"),
        });
        {
            let load = match clear {
                Some(color) => wgpu::LoadOp::Clear(color),
                None => wgpu::LoadOp::Load,
            };
            let mut pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
                label: Some("cells"),
                color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                    view,
                    depth_slice: None,
                    resolve_target: None,
                    ops: wgpu::Operations {
                        load,
                        store: wgpu::StoreOp::Store,
                    },
                })],
                depth_stencil_attachment: None,
                timestamp_writes: None,
                occlusion_query_set: None,
                multiview_mask: None,
            });
            if self.count > 0 {
                pass.set_pipeline(&self.pipeline);
                pass.set_bind_group(0, &self.bind_group, &[]);
                pass.set_vertex_buffer(0, self.instances.slice(..));
                pass.draw(0..6, 0..self.count);
            }
        }
        queue.submit(std::iter::once(encoder.finish()));
    }
}

const SHADER: &str = r"
struct Uniforms { size: vec2<f32>, _pad: vec2<f32> };
@group(0) @binding(0) var<uniform> u: Uniforms;

struct Inst {
    @location(0) rect: vec4<f32>,
    @location(1) color: vec4<f32>,
    @location(2) params: vec4<f32>,
};
struct VOut {
    @builtin(position) pos: vec4<f32>,
    @location(0) color: vec4<f32>,
    @location(1) @interpolate(flat) rect: vec4<f32>,
    @location(2) @interpolate(flat) params: vec4<f32>,
};

@vertex
fn vs_main(@builtin(vertex_index) vi: u32, inst: Inst) -> VOut {
    var corners = array<vec2<f32>, 6>(
        vec2<f32>(0.0, 0.0), vec2<f32>(1.0, 0.0), vec2<f32>(0.0, 1.0),
        vec2<f32>(0.0, 1.0), vec2<f32>(1.0, 0.0), vec2<f32>(1.0, 1.0),
    );
    let corner = corners[vi];
    let px = inst.rect.xy + corner * inst.rect.zw;
    let ndc = vec2<f32>(px.x / u.size.x * 2.0 - 1.0, 1.0 - px.y / u.size.y * 2.0);
    var out: VOut;
    out.pos = vec4<f32>(ndc, 0.0, 1.0);
    out.color = inst.color;
    out.rect = inst.rect;
    out.params = inst.params;
    return out;
}

// Signed distance from point `p` to a box of half-size `b` with corner radius `r`
// (negative inside). The standard rounded-box SDF (Inigo Quilez).
fn sd_round_box(p: vec2<f32>, b: vec2<f32>, r: f32) -> f32 {
    let q = abs(p) - b + vec2<f32>(r, r);
    return min(max(q.x, q.y), 0.0) + length(max(q, vec2<f32>(0.0, 0.0))) - r;
}

@fragment
fn fs_main(in: VOut) -> @location(0) vec4<f32> {
    let radius = in.params.x;
    let blur = in.params.y;
    let diamond = in.params.z;
    // Fast path: a plain sharp fill (cell backgrounds, dividers, cursors, underlines,
    // selection, flush panels) - identical to the old flat pipeline.
    if (radius <= 0.0 && blur <= 0.0 && diamond <= 0.0) {
        return in.color;
    }
    let half = in.rect.zw * 0.5;
    let center = in.rect.xy + half;
    let p = in.pos.xy - center;
    if (diamond > 0.0) {
        // A rounded square rotated 45deg (a logo disc): rotate the sample point back into
        // the square's axis-aligned frame, where the inner half-size is the AABB half
        // divided by sqrt(2) (a square's diagonal bound).
        let c = 0.70710678;
        let pr = vec2<f32>(c * (p.x + p.y), c * (p.y - p.x));
        let d = sd_round_box(pr, half * c, radius);
        let cov = clamp(0.5 - d, 0.0, 1.0);
        return vec4<f32>(in.color.rgb, in.color.a * cov);
    }
    if (blur > 0.0) {
        // Soft drop shadow: the SDF of the inset panel box, feathered over `blur`.
        let inner = half - vec2<f32>(blur, blur);
        let d = sd_round_box(p, inner, radius);
        let cov = 1.0 - smoothstep(-blur * 0.5, blur * 0.5, d);
        return vec4<f32>(in.color.rgb, in.color.a * cov);
    }
    // Rounded fill with a ~1px anti-aliased edge.
    let d = sd_round_box(p, half, radius);
    let cov = clamp(0.5 - d, 0.0, 1.0);
    return vec4<f32>(in.color.rgb, in.color.a * cov);
}
";

#[cfg(test)]
mod tests {
    use super::{grid_quads, logo_quads, push_outline, Quad};
    use crate::{GridCell, PxRect, Srgb, Theme};

    fn plain(c: char) -> GridCell {
        GridCell {
            c,
            fg: Srgb {
                r: 255,
                g: 255,
                b: 255,
            },
            bg: None,
            bold: false,
            italic: false,
            underline: false,
        }
    }

    const ACCENT: Srgb = Srgb {
        r: 0xBD,
        g: 0x93,
        b: 0xF9,
    };

    /// Exact-match a quad rect (all values here are integer-valued in f32).
    fn rect_eq(rect: [f32; 4], expected: [f32; 4]) -> bool {
        rect.iter().zip(expected).all(|(a, b)| (a - b).abs() < 1e-3)
    }

    #[test]
    fn focused_pane_draws_a_cursor_quad_at_the_offset_origin() {
        let rows = vec![vec![plain('a'), plain('b')]];
        let quads = grid_quads(
            10.0,
            20.0,
            (100.0, 200.0),
            &rows,
            Some((1, 0)),
            crate::CursorShape::Block,
            ACCENT,
            &[],
        );
        // No backgrounds/underlines/selection here, so the only quad is the cursor.
        assert_eq!(quads.len(), 1);
        // Cursor at column 1, row 0: origin + (1 * cell_w, 0).
        assert!(rect_eq(quads[0].rect, [110.0, 200.0, 10.0, 20.0]));
    }

    #[test]
    fn cursor_shape_bar_and_underline_draw_thin_quads() {
        let rows = vec![vec![plain('a'), plain('b')]];
        // A bar cursor: a thin vertical rule at the cell's left edge (full height).
        let bar = grid_quads(
            10.0,
            20.0,
            (0.0, 0.0),
            &rows,
            Some((1, 0)),
            crate::CursorShape::Bar,
            ACCENT,
            &[],
        );
        assert_eq!(bar.len(), 1);
        assert!(bar[0].rect[2] < 10.0, "bar is narrower than a full cell");
        assert!(rect_eq(bar[0].rect, [10.0, 0.0, 2.0, 20.0]));
        // An underline cursor: a thin horizontal rule along the cell's bottom (full width).
        let under = grid_quads(
            10.0,
            20.0,
            (0.0, 0.0),
            &rows,
            Some((1, 0)),
            crate::CursorShape::Underline,
            ACCENT,
            &[],
        );
        assert_eq!(under.len(), 1);
        assert!(
            (under[0].rect[2] - 10.0).abs() < 1e-3,
            "underline spans the full cell width"
        );
        assert!(
            under[0].rect[3] < 20.0,
            "underline is shorter than a full cell"
        );
        assert!(
            under[0].rect[1] > 0.0,
            "underline sits near the cell bottom"
        );
        // A hidden cursor emits nothing.
        let hidden = grid_quads(
            10.0,
            20.0,
            (0.0, 0.0),
            &rows,
            Some((1, 0)),
            crate::CursorShape::Hidden,
            ACCENT,
            &[],
        );
        assert!(hidden.is_empty(), "a hidden cursor emits no quad");
    }

    #[test]
    fn unfocused_pane_draws_no_cursor() {
        let rows = vec![vec![plain('a')]];
        let quads = grid_quads(
            10.0,
            20.0,
            (0.0, 0.0),
            &rows,
            None,
            crate::CursorShape::Block,
            ACCENT,
            &[],
        );
        assert!(quads.is_empty(), "a None cursor emits no quad");
    }

    #[test]
    fn push_outline_emits_four_edge_bars() {
        let mut quads: Vec<Quad> = Vec::new();
        push_outline(&mut quads, 5.0, 6.0, 40.0, 30.0, 2.0, ACCENT.to_linear());
        assert_eq!(quads.len(), 4);
        // Bottom bar sits at y + h - thickness; right bar at x + w - thickness.
        assert!(rect_eq(quads[1].rect, [5.0, 6.0 + 30.0 - 2.0, 40.0, 2.0]));
        assert!(rect_eq(quads[3].rect, [5.0 + 40.0 - 2.0, 6.0, 2.0, 30.0]));
    }

    #[test]
    fn plain_quads_carry_no_shape_params_so_the_shader_takes_the_flat_fast_path() {
        let q = super::Quad::new(1.0, 2.0, 3.0, 4.0, ACCENT.to_linear());
        assert!(rect_eq(q.params, [0.0, 0.0, 0.0, 0.0]));
    }

    #[test]
    fn rounded_clamps_the_radius_to_half_the_shorter_side() {
        // radius fits: kept as-is.
        let ok = super::Quad::rounded(0.0, 0.0, 100.0, 40.0, ACCENT.to_linear(), 10.0);
        assert!((ok.params[0] - 10.0).abs() < 1e-3);
        assert!(ok.params[1].abs() < 1e-3, "a fill is not a shadow");
        // radius too large for a 40px-tall quad: clamped to 20 (half the height).
        let clamped = super::Quad::rounded(0.0, 0.0, 100.0, 40.0, ACCENT.to_linear(), 999.0);
        assert!((clamped.params[0] - 20.0).abs() < 1e-3);
    }

    #[test]
    fn diamond_rect_is_the_rotated_squares_bounding_box() {
        // A 10px square rotated 45° has an axis-aligned bounding box of side 10·√2, centered
        // on the disc center; params flag the shader's rotated-box branch.
        let q = super::Quad::diamond(100.0, 200.0, 10.0, ACCENT.to_linear(), 2.6);
        let aabb = 10.0 * std::f32::consts::SQRT_2;
        assert!(rect_eq(
            q.rect,
            [100.0 - aabb / 2.0, 200.0 - aabb / 2.0, aabb, aabb]
        ));
        assert!((q.params[0] - 2.6).abs() < 1e-3, "corner radius recorded");
        assert!((q.params[2] - 1.0).abs() < 1e-3, "diamond flag set");
    }

    #[test]
    fn logo_quads_is_a_spine_pill_and_three_two_tone_discs() {
        let theme = Theme::resolve("ossein-dark");
        let bounds = PxRect {
            x: 40.0,
            y: 60.0,
            w: 56.0,
            h: 56.0,
        };
        let quads = logo_quads(bounds, &theme, 0.32);
        // The spine (a rounded pill, not a diamond) then the three discs (rotated).
        assert_eq!(quads.len(), 4);
        assert!(quads[0].params[2].abs() < 1e-3, "spine is not a diamond");
        assert!(quads[1..].iter().all(|q| (q.params[2] - 1.0).abs() < 1e-3));
        // Two-tone: spine + center disc are `accent`; the small top/bottom discs `fg.primary`.
        let accent = theme.accent.to_linear();
        let primary = theme.fg_primary.to_linear();
        assert_eq!(quads[0].color[..3], accent[..3], "spine is accent");
        assert_eq!(quads[2].color[..3], accent[..3], "center disc is accent");
        assert_eq!(quads[1].color[..3], primary[..3], "top disc is fg.primary");
        assert_eq!(
            quads[3].color[..3],
            primary[..3],
            "bottom disc is fg.primary"
        );
        // The container opacity scales every quad; the spine carries its extra 0.3 factor.
        assert!((quads[2].color[3] - 0.32).abs() < 1e-3, "center disc alpha");
        assert!((quads[0].color[3] - 0.3 * 0.32).abs() < 1e-3, "spine alpha");
        // The discs are centered horizontally on the box's vertical axis.
        let cx = bounds.x + bounds.w / 2.0;
        for disc in &quads[1..] {
            assert!((disc.rect[0] + disc.rect[2] / 2.0 - cx).abs() < 1e-3);
        }
    }

    #[test]
    fn shadow_inflates_by_the_blur_and_offsets_down_by_the_spec() {
        // e4 at scale 1: dy 16, blur 48. A 100x40 panel at (200, 100).
        let panel = crate::PxRect {
            x: 200.0,
            y: 100.0,
            w: 100.0,
            h: 40.0,
        };
        let q = super::Quad::shadow(panel, super::SHADOW_E4, 1.0, 10.0);
        // Inflated by blur (48) on every side, shifted down by dy (16).
        assert!(rect_eq(
            q.rect,
            [200.0 - 48.0, 100.0 + 16.0 - 48.0, 100.0 + 96.0, 40.0 + 96.0]
        ));
        assert!(
            (q.params[1] - 48.0).abs() < 1e-3,
            "blur recorded for the shader"
        );
        assert!(rect_eq(q.color, [0.0, 0.0, 0.0, 0.52]), "e4 is 52% black");
    }
}
