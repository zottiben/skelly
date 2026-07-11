//! A minimal instanced colored-quad pipeline, used to paint per-cell backgrounds
//! and the cursor beneath the text. Positions are in physical pixels; the vertex
//! shader converts them to clip space using the surface size uniform, and colors
//! are linear (the surface is sRGB).

#![allow(
    clippy::cast_precision_loss,
    clippy::cast_possible_truncation,
    reason = "surface dimensions and instance counts are small; casts are exact"
)]

/// One instanced quad: a pixel rectangle and a linear RGBA fill.
#[repr(C)]
#[derive(Clone, Copy, bytemuck::Pod, bytemuck::Zeroable)]
pub(crate) struct Quad {
    /// `x, y, w, h` in physical pixels.
    rect: [f32; 4],
    /// Linear RGBA.
    color: [f32; 4],
}

impl Quad {
    /// A quad at pixel `(x, y)` of size `(w, h)` filled with linear `color`.
    pub(crate) fn new(x: f32, y: f32, w: f32, h: f32, color: [f32; 4]) -> Self {
        Self {
            rect: [x, y, w, h],
            color,
        }
    }
}

/// Alpha applied to the accent color for the (translucent) selection highlight.
const SELECTION_ALPHA: f32 = 0.30;

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
    let mut quads = vec![Quad::new(
        panel.x,
        panel.y,
        panel.w,
        panel.h,
        theme.bg_elevated.to_linear(),
    )];
    if let Some(row) = view.selected_row {
        let mut highlight = theme.accent.to_linear();
        highlight[3] = 0.16;
        let y = view.text_origin.1 + row as f32 * cell_h;
        quads.push(Quad::new(
            panel.x + stroke,
            y,
            panel.w - 2.0 * stroke,
            cell_h,
            highlight,
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
    push_outline(
        &mut quads,
        panel.x,
        panel.y,
        panel.w,
        panel.h,
        stroke,
        theme.border_strong.to_linear(),
    );
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

const INSTANCE_ATTRS: [wgpu::VertexAttribute; 2] =
    wgpu::vertex_attr_array![0 => Float32x4, 1 => Float32x4];

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
};
struct VOut {
    @builtin(position) pos: vec4<f32>,
    @location(0) color: vec4<f32>,
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
    return out;
}

@fragment
fn fs_main(in: VOut) -> @location(0) vec4<f32> {
    return in.color;
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
}
