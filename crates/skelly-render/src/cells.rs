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

/// Build the quads for a grid, given the cell metrics (physical px), in draw order:
/// opaque cell backgrounds, then translucent selection fills over them, then the
/// accent cursor block. `selection` is the list of selected `(column, row)` cells.
pub(crate) fn grid_quads(
    cell_w: f32,
    cell_h: f32,
    pad: f32,
    rows: &[Vec<crate::GridCell>],
    cursor: (usize, usize),
    accent: crate::theme::Srgb,
    selection: &[(usize, usize)],
) -> Vec<Quad> {
    let cell_quad = |col: usize, row: usize, color: [f32; 4]| {
        Quad::new(
            pad + col as f32 * cell_w,
            pad + row as f32 * cell_h,
            cell_w,
            cell_h,
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

    let mut selection_color = accent.to_linear();
    selection_color[3] = SELECTION_ALPHA;
    for &(col, row) in selection {
        quads.push(cell_quad(col, row, selection_color));
    }

    let (cursor_col, cursor_row) = cursor;
    quads.push(cell_quad(cursor_col, cursor_row, accent.to_linear()));
    quads
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

    /// Clear `view` to `clear` and draw the uploaded quads over it. Submits its own
    /// command buffer; run before the text pass (which loads this result).
    pub(crate) fn draw(
        &self,
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        view: &wgpu::TextureView,
        clear: wgpu::Color,
    ) {
        let mut encoder = device.create_command_encoder(&wgpu::CommandEncoderDescriptor {
            label: Some("quad-encoder"),
        });
        {
            let mut pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
                label: Some("cells"),
                color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                    view,
                    depth_slice: None,
                    resolve_target: None,
                    ops: wgpu::Operations {
                        load: wgpu::LoadOp::Clear(clear),
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
