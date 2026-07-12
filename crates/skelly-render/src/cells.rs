//! A minimal instanced colored-quad pipeline, used to paint per-cell backgrounds
//! and the cursor beneath the text. Positions are in physical pixels; the vertex
//! shader converts them to clip space using the surface size uniform, and colors
//! are linear (the surface is sRGB).

#![allow(
    clippy::cast_precision_loss,
    clippy::cast_possible_truncation,
    reason = "surface dimensions and instance counts are small; casts are exact"
)]

/// One instanced quad: a pixel rectangle, a linear RGBA fill, and shape params.
///
/// `params` is `[radius, blur, _, _]` in physical pixels and drives the fragment
/// shader: `radius == 0 && blur == 0` is a plain sharp fill (every cell background,
/// divider, cursor, underline, and flush panel - identical to the old flat pipeline);
/// `radius > 0` rounds the corners with a signed-distance box and a 1px anti-aliased
/// edge (floating panels); `blur > 0` marks the quad a soft drop shadow, its coverage
/// feathered over `blur` px around the inset panel box (the guide's elevation tokens).
#[repr(C)]
#[derive(Clone, Copy, bytemuck::Pod, bytemuck::Zeroable)]
pub(crate) struct Quad {
    /// `x, y, w, h` in physical pixels.
    rect: [f32; 4],
    /// Linear RGBA.
    color: [f32; 4],
    /// `[radius, blur, _, _]` in physical pixels (see the type docs).
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
/// Corner radius (logical px) for the selected-row pill inside a menu/palette (the
/// guide's `md` radius: cards, menus, list rows).
pub(crate) const RADIUS_MD: f32 = 8.0;

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
pub(crate) fn grid_quads(
    cell_w: f32,
    cell_h: f32,
    origin: (f32, f32),
    rows: &[Vec<crate::GridCell>],
    cursor: Option<(usize, usize)>,
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
        quads.push(cell_quad(cursor_col, cursor_row, accent.to_linear()));
    }
    quads
}

/// Build the decorative quads for a command-palette overlay: the `bg.elevated` panel
/// fill, the translucent `accent` selected-row highlight, the `accent` input caret,
/// and the `border.strong` outline (drawn last, on top). Shared by the windowed
/// [`Renderer`](crate::Renderer) and the headless capture so both draw identically.
#[allow(
    clippy::cast_precision_loss,
    reason = "row/column indices into a small overlay grid are exact as f32"
)]
pub(crate) fn overlay_quads(
    view: &crate::OverlayView,
    theme: &crate::theme::Theme,
    cell_w: f32,
    cell_h: f32,
    scale: f32,
) -> Vec<Quad> {
    let panel = view.panel;
    let stroke = scale.max(1.0);
    let radius = RADIUS_LG * scale;
    let mut quads = Vec::new();

    // The e4 drop shadow behind the floating card, then a rounded `border.strong`
    // card with the `bg.elevated` fill inset by the stroke - a 1px rounded border ring
    // (replacing the old four sharp edge bars, which would poke past rounded corners).
    quads.push(Quad::shadow(panel, SHADOW_E4, scale, radius));
    quads.push(Quad::rounded(
        panel.x,
        panel.y,
        panel.w,
        panel.h,
        theme.border_strong.to_linear(),
        radius,
    ));
    quads.push(Quad::rounded(
        panel.x + stroke,
        panel.y + stroke,
        (panel.w - 2.0 * stroke).max(0.0),
        (panel.h - 2.0 * stroke).max(0.0),
        theme.bg_elevated.to_linear(),
        (radius - stroke).max(0.0),
    ));

    // The selected command: a rounded `accent.subtle` pill inset from the panel edges
    // (the guide's rounded list rows).
    if let Some(row) = view.selected_row {
        let inset = 6.0 * scale;
        let y = view.text_origin.1 + row as f32 * cell_h;
        quads.push(Quad::rounded(
            panel.x + inset,
            y,
            (panel.w - 2.0 * inset).max(0.0),
            cell_h,
            theme.accent_subtle(),
            RADIUS_MD * scale,
        ));
    }
    if let Some((col, row)) = view.caret {
        let x = view.text_origin.0 + col as f32 * cell_w;
        let y = view.text_origin.1 + row as f32 * cell_h;
        quads.push(Quad::new(
            x,
            y,
            (2.0 * scale).max(1.0),
            cell_h,
            theme.accent.to_linear(),
        ));
    }
    quads
}

/// Build the decorative quads for the left sidebar: the `bg.sidebar` panel fill (one step
/// off `bg.base`, per the guide's token table), the active tab's `accent.subtle` fill + a
/// 2px `accent` bar on its left edge, and a `border` divider down the right edge separating
/// the sidebar from the pane area. Shared by the windowed [`Renderer`](crate::Renderer) and
/// the headless capture.
#[allow(
    clippy::cast_precision_loss,
    reason = "the active-row index into a short tab list is exact as f32"
)]
pub(crate) fn sidebar_quads(
    view: &crate::SidebarView,
    theme: &crate::theme::Theme,
    _cell_w: f32,
    cell_h: f32,
    scale: f32,
) -> Vec<Quad> {
    let panel = view.panel;
    let stroke = scale.max(1.0);
    // The sidebar surface, distinct from the terminal's `bg.base` behind it.
    let mut quads = vec![Quad::new(
        panel.x,
        panel.y,
        panel.w,
        panel.h,
        theme.bg_sidebar.to_linear(),
    )];
    if let Some(row) = view.active_row {
        let y = view.text_origin.1 + row as f32 * cell_h;
        quads.push(Quad::new(
            panel.x,
            y,
            panel.w,
            cell_h,
            theme.accent_subtle(),
        ));
        quads.push(Quad::new(
            panel.x,
            y,
            (2.0 * scale).max(1.0),
            cell_h,
            theme.accent.to_linear(),
        ));
    }
    // The divider on the sidebar's right edge (drawn last, over the active fill).
    quads.push(Quad::new(
        panel.x + panel.w - stroke,
        panel.y,
        stroke,
        panel.h,
        theme.border.to_linear(),
    ));
    quads
}

/// Build the decorative quads for the full-window settings view: the `bg.elevated`
/// panel fill, a `bg.base` fill over the left category-nav strip, the active
/// category's `accent.subtle` fill + `accent` bar, the focused control's translucent
/// `accent` highlight (over the content strip only), and the `border` divider between
/// nav and content. Shared by the windowed [`Renderer`](crate::Renderer) and the
/// headless capture so both draw identically.
#[allow(
    clippy::cast_precision_loss,
    reason = "row/column indices into a small settings grid are exact as f32"
)]
pub(crate) fn settings_quads(
    view: &crate::SettingsView,
    theme: &crate::theme::Theme,
    cell_w: f32,
    cell_h: f32,
    scale: f32,
) -> Vec<Quad> {
    let panel = view.panel;
    let stroke = scale.max(1.0);
    let divider_x = view.text_origin.0 + view.nav_cols as f32 * cell_w;
    let row_y = |row: usize| view.text_origin.1 + row as f32 * cell_h;

    // The content panel, then the nav strip painted over its left portion.
    let mut quads = vec![
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
            (divider_x - panel.x).max(0.0),
            panel.h,
            theme.bg_base.to_array(),
        ),
    ];

    // The active category: a subtle fill across the nav strip + an accent bar.
    if let Some(row) = view.nav_active_row {
        let y = row_y(row);
        quads.push(Quad::new(
            panel.x,
            y,
            (divider_x - panel.x).max(0.0),
            cell_h,
            theme.accent_subtle(),
        ));
        quads.push(Quad::new(
            panel.x,
            y,
            (2.0 * scale).max(1.0),
            cell_h,
            theme.accent.to_linear(),
        ));
    }

    // The focused control: a translucent highlight over the content strip only.
    if let Some(row) = view.selected_row {
        let y = row_y(row);
        let highlight = theme.accent_subtle();
        let content_x = divider_x + stroke;
        quads.push(Quad::new(
            content_x,
            y,
            (panel.x + panel.w - content_x).max(0.0),
            cell_h,
            highlight,
        ));
    }

    // The nav/content divider, drawn last so it sits over both fills.
    quads.push(Quad::new(
        divider_x,
        panel.y,
        stroke,
        panel.h,
        theme.border.to_linear(),
    ));
    quads
}

/// Alpha for a diff line's translucent background: additions and deletions use the
/// heavier `.14`, hunk headers the lighter `.08` (the guide's `diff.*.bg` tokens).
const DIFF_LINE_ALPHA: f32 = 0.14;
/// Alpha for a hunk header's translucent background.
const HUNK_LINE_ALPHA: f32 = 0.08;

/// Build the decorative quads for the git diff dock: a full-width translucent background
/// behind every add / del / hunk-header row (from the `diff.*` tokens), the selected
/// file's `accent.subtle` row fill, and a `border` divider down the dock's *left* edge
/// (separating it from the pane area). The dock shares `bg.base` (already the clear
/// color), so there is no panel fill; the text colors come baked into `view.rows`. Shared
/// by the windowed [`Renderer`](crate::Renderer) and the headless capture.
#[allow(
    clippy::cast_precision_loss,
    reason = "row indices into a short dock grid are exact as f32"
)]
pub(crate) fn gitdock_quads(
    view: &crate::GitDockView,
    theme: &crate::theme::Theme,
    cell_w: f32,
    cell_h: f32,
    scale: f32,
) -> Vec<Quad> {
    let panel = view.panel;
    let stroke = scale.max(1.0);
    let row_y = |row: usize| view.text_origin.1 + row as f32 * cell_h;
    let tint = |color: crate::theme::Srgb, alpha: f32| {
        let mut c = color.to_linear();
        c[3] = alpha;
        c
    };
    let mut quads = Vec::new();

    // Diff-line backgrounds first (beneath the glyphs, which load over this pass).
    for &row in view.hunk_rows {
        quads.push(Quad::new(
            panel.x,
            row_y(row),
            panel.w,
            cell_h,
            tint(theme.diff_hunk, HUNK_LINE_ALPHA),
        ));
    }
    for &row in view.add_rows {
        quads.push(Quad::new(
            panel.x,
            row_y(row),
            panel.w,
            cell_h,
            tint(theme.diff_add, DIFF_LINE_ALPHA),
        ));
    }
    for &row in view.del_rows {
        quads.push(Quad::new(
            panel.x,
            row_y(row),
            panel.w,
            cell_h,
            tint(theme.diff_del, DIFF_LINE_ALPHA),
        ));
    }

    // The selected file's row fill (a subtle accent tint, like the focused settings
    // control - no accent bar, so it never fights the left-edge divider).
    if let Some(row) = view.selected_file_row {
        quads.push(Quad::new(
            panel.x,
            row_y(row),
            panel.w,
            cell_h,
            theme.accent_subtle(),
        ));
    }

    // The focused hunk header's fill (accent over its existing `diff.hunk` tint), marking
    // the target of a hunk-stage.
    if let Some(row) = view.focused_hunk_row {
        quads.push(Quad::new(
            panel.x,
            row_y(row),
            panel.w,
            cell_h,
            theme.accent_subtle(),
        ));
    }

    // The commit-message caret (an accent bar), when the commit box has focus.
    if let Some((col, row)) = view.caret {
        quads.push(Quad::new(
            view.text_origin.0 + col as f32 * cell_w,
            row_y(row),
            (2.0 * scale).max(1.0),
            cell_h,
            theme.accent.to_linear(),
        ));
    }

    // The divider on the dock's left edge, drawn last so it sits over the row fills.
    quads.push(Quad::new(
        panel.x,
        panel.y,
        stroke,
        panel.h,
        theme.border.to_linear(),
    ));
    quads
}

/// Build the decorative quads for the session-timeline dock: the selected event's
/// `accent.subtle` row fill, an `accent` bar on the viewed event's row when rewound to a
/// past state, and a `border` divider down the dock's *left* edge. Like the git dock it
/// shares `bg.base`, so there is no panel fill; text colors come baked into `view.rows`.
/// Shared by the windowed [`Renderer`](crate::Renderer) and the headless capture.
#[allow(
    clippy::cast_precision_loss,
    reason = "row indices into a short dock grid are exact as f32"
)]
pub(crate) fn timeline_quads(
    view: &crate::TimelineView,
    theme: &crate::theme::Theme,
    _cell_w: f32,
    cell_h: f32,
    scale: f32,
) -> Vec<Quad> {
    let panel = view.panel;
    let stroke = scale.max(1.0);
    let row_y = |row: usize| view.text_origin.1 + row as f32 * cell_h;
    let mut quads = Vec::new();

    // The selected event's row fill (a subtle accent tint, like the git dock's selection).
    if let Some(row) = view.selected_row {
        quads.push(Quad::new(
            panel.x,
            row_y(row),
            panel.w,
            cell_h,
            theme.accent_subtle(),
        ));
    }

    // When rewound to the past, mark the viewed event with a solid `accent` bar on the
    // left edge (the guide's "VIEWING" marker).
    if let Some(row) = view.viewing_row {
        quads.push(Quad::new(
            panel.x,
            row_y(row),
            (2.0 * scale).max(1.0),
            cell_h,
            theme.accent.to_linear(),
        ));
    }

    // The divider on the dock's left edge, drawn last so it sits over the row fill.
    quads.push(Quad::new(
        panel.x,
        panel.y,
        stroke,
        panel.h,
        theme.border.to_linear(),
    ));
    quads
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
    // Fast path: a plain sharp fill (cell backgrounds, dividers, cursors, underlines,
    // selection, flush panels) - identical to the old flat pipeline.
    if (radius <= 0.0 && blur <= 0.0) {
        return in.color;
    }
    let half = in.rect.zw * 0.5;
    let center = in.rect.xy + half;
    let p = in.pos.xy - center;
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
    use super::{grid_quads, push_outline, Quad};
    use crate::{GridCell, Srgb};

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
        let quads = grid_quads(10.0, 20.0, (100.0, 200.0), &rows, Some((1, 0)), ACCENT, &[]);
        // No backgrounds/underlines/selection here, so the only quad is the cursor.
        assert_eq!(quads.len(), 1);
        // Cursor at column 1, row 0: origin + (1 * cell_w, 0).
        assert!(rect_eq(quads[0].rect, [110.0, 200.0, 10.0, 20.0]));
    }

    #[test]
    fn unfocused_pane_draws_no_cursor() {
        let rows = vec![vec![plain('a')]];
        let quads = grid_quads(10.0, 20.0, (0.0, 0.0), &rows, None, ACCENT, &[]);
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
