//! 2D interface drawing: textured and coloured quads in screen pixels.
//!
//! Stands in for `W3DDisplay::drawImage` and `drawFillRect` (`W3DDisplay.cpp`),
//! which draw through WW3D's 2D renderer with alpha blending and no depth.
//! Quads that share a texture are batched in order, so overlap follows the
//! window system's back-to-front draw list.
use std::sync::Arc;

use wgpu::util::DeviceExt;

use crate::{Assets, Renderer};

/// One quad: `rect` is left, top, right, bottom in pixels; `uv` the
/// top-left, top-right, bottom-right and bottom-left texture corners.
#[derive(Clone, Debug, PartialEq)]
pub struct UiQuad {
    /// `None` draws a flat colour.
    pub texture: Option<String>,
    pub rect: [f32; 4],
    pub uv: [[f32; 2]; 4],
    pub color: [f32; 4],
}

#[repr(C)]
#[derive(Clone, Copy, bytemuck::Pod, bytemuck::Zeroable)]
struct Vertex {
    pos: [f32; 2],
    uv: [f32; 2],
    color: [f32; 4],
}

const SHADER: &str = r#"
struct Screen { size: vec2<f32>, pad: vec2<f32> };
@group(0) @binding(0) var<uniform> screen: Screen;
@group(0) @binding(1) var tex: texture_2d<f32>;
@group(0) @binding(2) var samp: sampler;
struct Out { @builtin(position) pos: vec4<f32>, @location(0) uv: vec2<f32>, @location(1) color: vec4<f32> };
@vertex fn vs(@location(0) pos: vec2<f32>, @location(1) uv: vec2<f32>, @location(2) color: vec4<f32>) -> Out {
    var o: Out;
    let ndc = pos / screen.size * 2.0 - vec2<f32>(1.0, 1.0);
    o.pos = vec4<f32>(ndc.x, -ndc.y, 0.0, 1.0);
    o.uv = uv;
    o.color = color;
    return o;
}
@fragment fn fs(i: Out) -> @location(0) vec4<f32> {
    return textureSample(tex, samp, i.uv) * i.color;
}
"#;

/// The UI pipeline, made on first use.
pub struct UiPass {
    pipeline: wgpu::RenderPipeline,
    layout: wgpu::BindGroupLayout,
    sampler: wgpu::Sampler,
}

impl UiPass {
    pub fn new(r: &Renderer) -> Self {
        let device = &r.device;
        let module = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("ui"),
            source: wgpu::ShaderSource::Wgsl(SHADER.into()),
        });
        let entry = |binding, ty| wgpu::BindGroupLayoutEntry {
            binding,
            visibility: wgpu::ShaderStages::VERTEX_FRAGMENT,
            ty,
            count: None,
        };
        let layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("ui"),
            entries: &[
                entry(
                    0,
                    wgpu::BindingType::Buffer {
                        ty: wgpu::BufferBindingType::Uniform,
                        has_dynamic_offset: false,
                        min_binding_size: None,
                    },
                ),
                entry(
                    1,
                    wgpu::BindingType::Texture {
                        sample_type: wgpu::TextureSampleType::Float { filterable: true },
                        view_dimension: wgpu::TextureViewDimension::D2,
                        multisampled: false,
                    },
                ),
                entry(2, wgpu::BindingType::Sampler(wgpu::SamplerBindingType::Filtering)),
            ],
        });
        let pl = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
            label: Some("ui"),
            bind_group_layouts: &[Some(&layout)],
            immediate_size: 0,
        });
        let pipeline = device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
            label: Some("ui"),
            layout: Some(&pl),
            vertex: wgpu::VertexState {
                module: &module,
                entry_point: Some("vs"),
                compilation_options: Default::default(),
                buffers: &[Some(wgpu::VertexBufferLayout {
                    array_stride: std::mem::size_of::<Vertex>() as u64,
                    step_mode: wgpu::VertexStepMode::Vertex,
                    attributes: &wgpu::vertex_attr_array![0 => Float32x2, 1 => Float32x2, 2 => Float32x4],
                })],
            },
            fragment: Some(wgpu::FragmentState {
                module: &module,
                entry_point: Some("fs"),
                compilation_options: Default::default(),
                targets: &[Some(wgpu::ColorTargetState {
                    format: r.format,
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
        let sampler = device.create_sampler(&wgpu::SamplerDescriptor {
            mag_filter: wgpu::FilterMode::Linear,
            min_filter: wgpu::FilterMode::Linear,
            ..Default::default()
        });
        Self { pipeline, layout, sampler }
    }

    /// Records the quads into `target` (cleared to `clear` when given).
    pub fn draw(
        &self,
        r: &mut Renderer,
        assets: &mut Assets,
        enc: &mut wgpu::CommandEncoder,
        target: &wgpu::TextureView,
        size: (u32, u32),
        quads: &[UiQuad],
        clear: Option<[f64; 4]>,
    ) {
        let screen = r.device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
            label: Some("ui screen"),
            contents: bytemuck::cast_slice(&[size.0 as f32, size.1 as f32, 0.0, 0.0]),
            usage: wgpu::BufferUsages::UNIFORM,
        });
        // Batches of consecutive quads with one texture.
        let mut batches: Vec<(Arc<wgpu::TextureView>, std::ops::Range<u32>)> = vec![];
        let mut verts: Vec<Vertex> = vec![];
        let mut last: Option<Option<String>> = None;
        for q in quads {
            let view = match &q.texture {
                Some(t) => r.named_texture(assets, t),
                None => r.white_texture(),
            };
            let start = verts.len() as u32;
            let [x0, y0, x1, y1] = q.rect;
            let corners = [[x0, y0], [x1, y0], [x1, y1], [x0, y1]];
            for i in [0, 1, 2, 0, 2, 3] {
                verts.push(Vertex { pos: corners[i], uv: q.uv[i], color: q.color });
            }
            match batches.last_mut() {
                Some((_, range)) if last.as_ref() == Some(&q.texture) => range.end = verts.len() as u32,
                _ => batches.push((view, start..verts.len() as u32)),
            }
            last = Some(q.texture.clone());
        }
        let vbuf = (!verts.is_empty()).then(|| {
            r.device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
                label: Some("ui quads"),
                contents: bytemuck::cast_slice(&verts),
                usage: wgpu::BufferUsages::VERTEX,
            })
        });
        let groups: Vec<wgpu::BindGroup> = batches
            .iter()
            .map(|(view, _)| {
                r.device.create_bind_group(&wgpu::BindGroupDescriptor {
                    label: None,
                    layout: &self.layout,
                    entries: &[
                        wgpu::BindGroupEntry { binding: 0, resource: screen.as_entire_binding() },
                        wgpu::BindGroupEntry { binding: 1, resource: wgpu::BindingResource::TextureView(view) },
                        wgpu::BindGroupEntry { binding: 2, resource: wgpu::BindingResource::Sampler(&self.sampler) },
                    ],
                })
            })
            .collect();
        let load = match clear {
            Some(c) => wgpu::LoadOp::Clear(wgpu::Color { r: c[0], g: c[1], b: c[2], a: c[3] }),
            None => wgpu::LoadOp::Load,
        };
        let mut pass = enc.begin_render_pass(&wgpu::RenderPassDescriptor {
            label: Some("ui"),
            color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                view: target,
                depth_slice: None,
                resolve_target: None,
                ops: wgpu::Operations { load, store: wgpu::StoreOp::Store },
            })],
            depth_stencil_attachment: None,
            timestamp_writes: None,
            occlusion_query_set: None,
            multiview_mask: None,
        });
        let Some(vbuf) = &vbuf else { return };
        pass.set_pipeline(&self.pipeline);
        pass.set_vertex_buffer(0, vbuf.slice(..));
        for ((_, range), group) in batches.iter().zip(&groups) {
            pass.set_bind_group(0, group, &[]);
            pass.draw(range.clone(), 0..1);
        }
    }
}

impl Renderer {
    /// Draws the quads offscreen over black and returns tightly packed RGBA8 rows.
    pub fn render_ui(&mut self, assets: &mut Assets, quads: &[UiQuad], size: (u32, u32)) -> Vec<u8> {
        let pass = UiPass::new(self);
        let (w, h) = size;
        let extent = wgpu::Extent3d { width: w, height: h, depth_or_array_layers: 1 };
        let tex = self.device.create_texture(&wgpu::TextureDescriptor {
            label: Some("ui target"),
            size: extent,
            mip_level_count: 1,
            sample_count: 1,
            dimension: wgpu::TextureDimension::D2,
            format: self.format,
            usage: wgpu::TextureUsages::RENDER_ATTACHMENT | wgpu::TextureUsages::COPY_SRC,
            view_formats: &[],
        });
        let view = tex.create_view(&Default::default());
        let mut enc = self.device.create_command_encoder(&Default::default());
        pass.draw(self, assets, &mut enc, &view, size, quads, Some([0.0, 0.0, 0.0, 1.0]));
        self.read_back(enc, &tex, size)
    }
}
