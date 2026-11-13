//! The wgpu side: device, pipelines per render state, textures, offscreen rendering.

use std::collections::HashMap;
use std::sync::Arc;

use bytemuck::{Pod, Zeroable};
use glam::{Mat4, Vec3};
use ih_formats::texture::Image;
use ih_formats::w3d::Shader;
use wgpu::util::DeviceExt;

use crate::assets::Assets;
use crate::model::{Model, TextureSlot, Vertex};

#[derive(Debug, thiserror::Error)]
pub enum GpuError {
    #[error("no graphics adapter: {0}")]
    Adapter(String),
    #[error("could not create device: {0}")]
    Device(String),
}

/// Scene lighting: ambient plus up to three directional lights (GameData's
/// `TerrainObjectsLighting*` values). Directions point the way the light travels.
#[derive(Debug, Clone, Copy)]
pub struct Lights {
    pub ambient: Vec3,
    pub lights: [(Vec3, Vec3); 3],
}

impl Lights {
    /// GameData's afternoon object lighting from stock Zero Hour.
    pub fn afternoon() -> Self {
        let c = |r: f32, g: f32, b: f32| Vec3::new(r, g, b) / 255.0;
        Self {
            ambient: c(56.0, 52.0, 44.0),
            lights: [
                (Vec3::new(-0.81, 0.38, -0.45).normalize(), c(255.0, 255.0, 255.0)),
                (Vec3::new(0.79, 0.62, 0.0).normalize(), c(60.0, 60.0, 80.0)),
                (Vec3::new(0.0, 0.0, -1.0), Vec3::ZERO),
            ],
        }
    }
}

#[derive(Debug, Clone, Copy)]
pub struct Camera {
    pub eye: Vec3,
    pub target: Vec3,
    pub fov_y: f32,
    pub near: f32,
    pub far: f32,
}

impl Camera {
    fn view(&self) -> Mat4 {
        glam::camera::rh::view::look_at_mat4(self.eye, self.target, Vec3::Z)
    }

    fn view_proj(&self, aspect: f32) -> Mat4 {
        // WebGPU clip space: right-handed, depth 0..1, Y up (glam's "directx" convention).
        let proj = glam::camera::rh::proj::directx::perspective(self.fov_y, aspect, self.near, self.far);
        proj * self.view()
    }
}

#[repr(C)]
#[derive(Clone, Copy, Pod, Zeroable)]
struct GlobalsU {
    view_proj: [[f32; 4]; 4],
    view: [[f32; 4]; 4],
    eye: [f32; 4],
    ambient: [f32; 4],
    light_dir: [[f32; 4]; 3],
    light_color: [[f32; 4]; 3],
}

#[repr(C)]
#[derive(Clone, Copy, Pod, Zeroable)]
struct DrawU {
    modes: [u32; 4],
    flags: [u32; 4],
    params: [f32; 4],
    uvgen: [u32; 4],
    tm: [[f32; 4]; 4],
}

/// Render state that needs its own pipeline.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
struct PipeKey {
    src_blend: u8,
    dest_blend: u8,
    depth_compare: u8,
    depth_write: bool,
    two_sided: bool,
}

const COLOR_FORMAT: wgpu::TextureFormat = wgpu::TextureFormat::Rgba8Unorm;
const DEPTH_FORMAT: wgpu::TextureFormat = wgpu::TextureFormat::Depth32Float;
const SAMPLES: u32 = 4;
/// `alphareference = 0x60` in `ShaderClass::Apply`.
const ALPHA_REF: f32 = 96.0 / 255.0;

pub struct Renderer {
    device: wgpu::Device,
    queue: wgpu::Queue,
    module: wgpu::ShaderModule,
    layout: wgpu::PipelineLayout,
    globals_layout: wgpu::BindGroupLayout,
    bones_layout: wgpu::BindGroupLayout,
    draw_layout: wgpu::BindGroupLayout,
    pipelines: HashMap<PipeKey, wgpu::RenderPipeline>,
    textures: HashMap<String, Arc<wgpu::TextureView>>,
    white: Arc<wgpu::TextureView>,
    samplers: HashMap<(bool, bool), Arc<wgpu::Sampler>>,
    pub adapter_name: String,
}

impl Renderer {
    /// Creates a headless renderer on the default adapter.
    pub fn new() -> Result<Self, GpuError> {
        let instance = wgpu::Instance::new(wgpu::InstanceDescriptor::new_without_display_handle_from_env());
        let adapter = pollster::block_on(instance.request_adapter(&wgpu::RequestAdapterOptions {
            power_preference: wgpu::PowerPreference::HighPerformance,
            force_fallback_adapter: false,
            compatible_surface: None,
            apply_limit_buckets: false,
        }))
        .map_err(|e| GpuError::Adapter(e.to_string()))?;
        let adapter_name = adapter.get_info().name;
        let (device, queue) = pollster::block_on(adapter.request_device(&wgpu::DeviceDescriptor {
            label: Some("ih-render"),
            required_features: wgpu::Features::empty(),
            required_limits: wgpu::Limits::default(),
            ..Default::default()
        }))
        .map_err(|e| GpuError::Device(e.to_string()))?;

        let module = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("w3d"),
            source: wgpu::ShaderSource::Wgsl(include_str!("w3d.wgsl").into()),
        });
        let entry = |binding, visibility, ty| wgpu::BindGroupLayoutEntry { binding, visibility, ty, count: None };
        let uniform = wgpu::BindingType::Buffer {
            ty: wgpu::BufferBindingType::Uniform,
            has_dynamic_offset: false,
            min_binding_size: None,
        };
        let vf = wgpu::ShaderStages::VERTEX | wgpu::ShaderStages::FRAGMENT;
        let globals_layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("globals"),
            entries: &[entry(0, vf, uniform)],
        });
        let bones_layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("bones"),
            entries: &[entry(
                0,
                wgpu::ShaderStages::VERTEX,
                wgpu::BindingType::Buffer {
                    ty: wgpu::BufferBindingType::Storage { read_only: true },
                    has_dynamic_offset: false,
                    min_binding_size: None,
                },
            )],
        });
        let tex = wgpu::BindingType::Texture {
            sample_type: wgpu::TextureSampleType::Float { filterable: true },
            view_dimension: wgpu::TextureViewDimension::D2,
            multisampled: false,
        };
        let samp = wgpu::BindingType::Sampler(wgpu::SamplerBindingType::Filtering);
        let fs = wgpu::ShaderStages::FRAGMENT;
        let draw_layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("draw"),
            entries: &[
                entry(0, vf, uniform),
                entry(1, fs, tex),
                entry(2, fs, samp),
                entry(3, fs, tex),
                entry(4, fs, samp),
            ],
        });
        let layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
            label: Some("w3d"),
            bind_group_layouts: &[Some(&globals_layout), Some(&bones_layout), Some(&draw_layout)],
            immediate_size: 0,
        });
        let white_tex = device.create_texture_with_data(
            &queue,
            &wgpu::TextureDescriptor {
                label: Some("white"),
                size: wgpu::Extent3d { width: 1, height: 1, depth_or_array_layers: 1 },
                mip_level_count: 1,
                sample_count: 1,
                dimension: wgpu::TextureDimension::D2,
                format: COLOR_FORMAT,
                usage: wgpu::TextureUsages::TEXTURE_BINDING,
                view_formats: &[],
            },
            wgpu::util::TextureDataOrder::LayerMajor,
            &[255, 255, 255, 255],
        );
        let white = Arc::new(white_tex.create_view(&Default::default()));
        Ok(Self {
            device,
            queue,
            module,
            layout,
            globals_layout,
            bones_layout,
            draw_layout,
            pipelines: HashMap::new(),
            textures: HashMap::new(),
            white,
            samplers: HashMap::new(),
            adapter_name,
        })
    }

    fn pipeline(&mut self, key: PipeKey) -> &wgpu::RenderPipeline {
        let (device, module, layout) = (&self.device, &self.module, &self.layout);
        self.pipelines.entry(key).or_insert_with(|| {
            // srcBlendLUT / dstBlendLUT (shader.cpp).
            let src = match key.src_blend {
                0 => wgpu::BlendFactor::Zero,
                2 => wgpu::BlendFactor::SrcAlpha,
                3 => wgpu::BlendFactor::OneMinusSrcAlpha,
                _ => wgpu::BlendFactor::One,
            };
            let dst = match key.dest_blend {
                1 => wgpu::BlendFactor::One,
                2 | 6 => wgpu::BlendFactor::Src,
                3 => wgpu::BlendFactor::OneMinusSrc,
                4 => wgpu::BlendFactor::SrcAlpha,
                5 => wgpu::BlendFactor::OneMinusSrcAlpha,
                _ => wgpu::BlendFactor::Zero,
            };
            let blend = (src != wgpu::BlendFactor::One || dst != wgpu::BlendFactor::Zero).then_some(wgpu::BlendState {
                color: wgpu::BlendComponent { src_factor: src, dst_factor: dst, operation: wgpu::BlendOperation::Add },
                alpha: wgpu::BlendComponent { src_factor: src, dst_factor: dst, operation: wgpu::BlendOperation::Add },
            });
            let compare = match key.depth_compare {
                0 => wgpu::CompareFunction::Never,
                1 => wgpu::CompareFunction::Less,
                2 => wgpu::CompareFunction::Equal,
                4 => wgpu::CompareFunction::Greater,
                5 => wgpu::CompareFunction::NotEqual,
                6 => wgpu::CompareFunction::GreaterEqual,
                7 => wgpu::CompareFunction::Always,
                _ => wgpu::CompareFunction::LessEqual,
            };
            let attrs = wgpu::vertex_attr_array![
                0 => Float32x3, 1 => Float32x3, 2 => Float32x2, 3 => Float32x2,
                4 => Float32x4, 5 => Float32x3, 6 => Float32x3, 7 => Uint32
            ];
            device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
                label: Some("w3d"),
                layout: Some(layout),
                vertex: wgpu::VertexState {
                    module,
                    entry_point: Some("vs_main"),
                    compilation_options: Default::default(),
                    buffers: &[Some(wgpu::VertexBufferLayout {
                        array_stride: std::mem::size_of::<Vertex>() as u64,
                        step_mode: wgpu::VertexStepMode::Vertex,
                        attributes: &attrs,
                    })],
                },
                primitive: wgpu::PrimitiveState {
                    topology: wgpu::PrimitiveTopology::TriangleList,
                    front_face: wgpu::FrontFace::Ccw,
                    cull_mode: if key.two_sided { None } else { Some(wgpu::Face::Back) },
                    ..Default::default()
                },
                depth_stencil: Some(wgpu::DepthStencilState {
                    format: DEPTH_FORMAT,
                    depth_write_enabled: Some(key.depth_write),
                    depth_compare: Some(compare),
                    stencil: Default::default(),
                    bias: Default::default(),
                }),
                multisample: wgpu::MultisampleState { count: SAMPLES, ..Default::default() },
                fragment: Some(wgpu::FragmentState {
                    module,
                    entry_point: Some("fs_main"),
                    compilation_options: Default::default(),
                    targets: &[Some(wgpu::ColorTargetState {
                        format: COLOR_FORMAT,
                        blend,
                        write_mask: wgpu::ColorWrites::ALL,
                    })],
                }),
                multiview_mask: None,
                cache: None,
            })
        })
    }

    fn sampler(&mut self, clamp_u: bool, clamp_v: bool) -> Arc<wgpu::Sampler> {
        let device = &self.device;
        self.samplers
            .entry((clamp_u, clamp_v))
            .or_insert_with(|| {
                let mode = |c| if c { wgpu::AddressMode::ClampToEdge } else { wgpu::AddressMode::Repeat };
                Arc::new(device.create_sampler(&wgpu::SamplerDescriptor {
                    address_mode_u: mode(clamp_u),
                    address_mode_v: mode(clamp_v),
                    mag_filter: wgpu::FilterMode::Linear,
                    min_filter: wgpu::FilterMode::Linear,
                    mipmap_filter: wgpu::MipmapFilterMode::Linear,
                    ..Default::default()
                }))
            })
            .clone()
    }

    fn texture(&mut self, assets: &mut Assets, slot: &TextureSlot, house: Option<u32>) -> Arc<wgpu::TextureView> {
        let house = house.filter(|_| crate::house_color::applies(&slot.name));
        let key = match house {
            Some(c) => format!("{}#{c:06x}", slot.name.to_ascii_lowercase()),
            None => slot.name.to_ascii_lowercase(),
        };
        if let Some(t) = self.textures.get(&key) {
            return t.clone();
        }
        let view = match assets.texture(&slot.name) {
            Some(img) => {
                let recolored = house.and_then(|c| crate::house_color::recolor(&slot.name, &img, c));
                Arc::new(upload(&self.device, &self.queue, recolored.as_ref().unwrap_or(&img)))
            }
            None => self.white.clone(),
        };
        self.textures.insert(key, view.clone());
        view
    }

    /// Renders `model` posed by `bones` (one world matrix per hierarchy pivot) at
    /// `time_ms` (for time-based texture mappers) and returns tightly packed RGBA8 rows.
    #[allow(clippy::too_many_arguments)]
    pub fn render(
        &mut self,
        assets: &mut Assets,
        model: &Model,
        bones: &[Mat4],
        camera: &Camera,
        lights: &Lights,
        size: (u32, u32),
        clear: [f64; 4],
        time_ms: f32,
    ) -> Vec<u8> {
        let (w, h) = size;
        let extent = wgpu::Extent3d { width: w, height: h, depth_or_array_layers: 1 };
        let target = |format, samples, usage| {
            self.device.create_texture(&wgpu::TextureDescriptor {
                label: None,
                size: extent,
                mip_level_count: 1,
                sample_count: samples,
                dimension: wgpu::TextureDimension::D2,
                format,
                usage,
                view_formats: &[],
            })
        };
        let msaa = target(COLOR_FORMAT, SAMPLES, wgpu::TextureUsages::RENDER_ATTACHMENT);
        let resolve = target(COLOR_FORMAT, 1, wgpu::TextureUsages::RENDER_ATTACHMENT | wgpu::TextureUsages::COPY_SRC);
        let depth = target(DEPTH_FORMAT, SAMPLES, wgpu::TextureUsages::RENDER_ATTACHMENT);

        // Globals.
        let mut gu = GlobalsU {
            view_proj: camera.view_proj(w as f32 / h as f32).to_cols_array_2d(),
            view: camera.view().to_cols_array_2d(),
            eye: camera.eye.extend(1.0).to_array(),
            ambient: lights.ambient.extend(1.0).to_array(),
            light_dir: [[0.0; 4]; 3],
            light_color: [[0.0; 4]; 3],
        };
        for (i, (d, c)) in lights.lights.iter().enumerate() {
            gu.light_dir[i] = d.extend(0.0).to_array();
            gu.light_color[i] = c.extend(1.0).to_array();
        }
        let globals = self.device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
            label: None,
            contents: bytemuck::bytes_of(&gu),
            usage: wgpu::BufferUsages::UNIFORM,
        });
        let globals_bg = self.device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: None,
            layout: &self.globals_layout,
            entries: &[wgpu::BindGroupEntry { binding: 0, resource: globals.as_entire_binding() }],
        });
        let bone_data: Vec<[[f32; 4]; 4]> = if bones.is_empty() {
            vec![Mat4::IDENTITY.to_cols_array_2d()]
        } else {
            bones.iter().map(|m| m.to_cols_array_2d()).collect()
        };
        let bone_buf = self.device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
            label: None,
            contents: bytemuck::cast_slice(&bone_data),
            usage: wgpu::BufferUsages::STORAGE,
        });
        let bones_bg = self.device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: None,
            layout: &self.bones_layout,
            entries: &[wgpu::BindGroupEntry { binding: 0, resource: bone_buf.as_entire_binding() }],
        });

        // Opaque first, then translucent, each in sort-level order (stable).
        let mut order: Vec<usize> = (0..model.batches.len()).collect();
        order.sort_by_key(|&i| (model.batches[i].is_translucent(), model.batches[i].sort_level));

        struct Prepared {
            key: PipeKey,
            vb: wgpu::Buffer,
            ib: wgpu::Buffer,
            count: u32,
            bg: wgpu::BindGroup,
        }
        let mut prepared = Vec::new();
        for i in order {
            let b = &model.batches[i];
            let s: &Shader = &b.shader;
            let du = DrawU {
                modes: [
                    u32::from(s.pri_gradient),
                    u32::from(s.texturing != 0 && b.textures[0].is_some()),
                    u32::from(s.post_detail_color_func),
                    u32::from(s.post_detail_alpha_func),
                ],
                flags: [
                    if s.alpha_test == 0 {
                        0
                    } else if s.src_blend == 3 {
                        2
                    } else {
                        1
                    },
                    u32::from(b.lighting),
                    u32::from(b.textures[1].is_some()),
                    0,
                ],
                params: [if s.src_blend == 3 { 1.0 - ALPHA_REF } else { ALPHA_REF }, 0.0, 0.0, 0.0],
                uvgen: {
                    let (m0, a0) = b.mappers[0].mode();
                    let (m1, a1) = b.mappers[1].mode();
                    [m0, m1, a0, a1]
                },
                tm: {
                    let r = |m: &crate::model::Mapper| m.matrix(time_ms);
                    let (t0, t1) = (r(&b.mappers[0]), r(&b.mappers[1]));
                    let row = |x: [f32; 3]| [x[0], x[1], x[2], 0.0];
                    [row(t0[0]), row(t0[1]), row(t1[0]), row(t1[1])]
                },
            };
            let ub = self.device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
                label: None,
                contents: bytemuck::bytes_of(&du),
                usage: wgpu::BufferUsages::UNIFORM,
            });
            let views: Vec<(Arc<wgpu::TextureView>, Arc<wgpu::Sampler>)> = b
                .textures
                .iter()
                .map(|t| match t {
                    Some(slot) => {
                        (self.texture(assets, slot, model.house_color), self.sampler(slot.clamp_u, slot.clamp_v))
                    }
                    None => (self.white.clone(), self.sampler(false, false)),
                })
                .collect();
            let bg = self.device.create_bind_group(&wgpu::BindGroupDescriptor {
                label: None,
                layout: &self.draw_layout,
                entries: &[
                    wgpu::BindGroupEntry { binding: 0, resource: ub.as_entire_binding() },
                    wgpu::BindGroupEntry { binding: 1, resource: wgpu::BindingResource::TextureView(&views[0].0) },
                    wgpu::BindGroupEntry { binding: 2, resource: wgpu::BindingResource::Sampler(&views[0].1) },
                    wgpu::BindGroupEntry { binding: 3, resource: wgpu::BindingResource::TextureView(&views[1].0) },
                    wgpu::BindGroupEntry { binding: 4, resource: wgpu::BindingResource::Sampler(&views[1].1) },
                ],
            });
            let vb = self.device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
                label: None,
                contents: bytemuck::cast_slice(&b.vertices),
                usage: wgpu::BufferUsages::VERTEX,
            });
            let ib = self.device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
                label: None,
                contents: bytemuck::cast_slice(&b.indices),
                usage: wgpu::BufferUsages::INDEX,
            });
            let key = PipeKey {
                src_blend: s.src_blend,
                dest_blend: s.dest_blend,
                depth_compare: s.depth_compare,
                depth_write: s.depth_mask != 0,
                two_sided: b.two_sided,
            };
            self.pipeline(key);
            prepared.push(Prepared { key, vb, ib, count: b.indices.len() as u32, bg });
        }

        let msaa_view = msaa.create_view(&Default::default());
        let resolve_view = resolve.create_view(&Default::default());
        let depth_view = depth.create_view(&Default::default());
        let mut enc = self.device.create_command_encoder(&Default::default());
        {
            let mut pass = enc.begin_render_pass(&wgpu::RenderPassDescriptor {
                label: Some("model"),
                color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                    view: &msaa_view,
                    depth_slice: None,
                    resolve_target: Some(&resolve_view),
                    ops: wgpu::Operations {
                        load: wgpu::LoadOp::Clear(wgpu::Color { r: clear[0], g: clear[1], b: clear[2], a: clear[3] }),
                        store: wgpu::StoreOp::Store,
                    },
                })],
                depth_stencil_attachment: Some(wgpu::RenderPassDepthStencilAttachment {
                    view: &depth_view,
                    depth_ops: Some(wgpu::Operations { load: wgpu::LoadOp::Clear(1.0), store: wgpu::StoreOp::Store }),
                    stencil_ops: None,
                }),
                timestamp_writes: None,
                occlusion_query_set: None,
                multiview_mask: None,
            });
            pass.set_bind_group(0, &globals_bg, &[]);
            pass.set_bind_group(1, &bones_bg, &[]);
            for p in &prepared {
                pass.set_pipeline(&self.pipelines[&p.key]);
                pass.set_bind_group(2, &p.bg, &[]);
                pass.set_vertex_buffer(0, p.vb.slice(..));
                pass.set_index_buffer(p.ib.slice(..), wgpu::IndexFormat::Uint32);
                pass.draw_indexed(0..p.count, 0, 0..1);
            }
        }

        // Read back (rows padded to 256 bytes).
        let padded = (w * 4).div_ceil(256) * 256;
        let readback = self.device.create_buffer(&wgpu::BufferDescriptor {
            label: None,
            size: u64::from(padded * h),
            usage: wgpu::BufferUsages::COPY_DST | wgpu::BufferUsages::MAP_READ,
            mapped_at_creation: false,
        });
        enc.copy_texture_to_buffer(
            wgpu::TexelCopyTextureInfo {
                texture: &resolve,
                mip_level: 0,
                origin: wgpu::Origin3d::ZERO,
                aspect: wgpu::TextureAspect::All,
            },
            wgpu::TexelCopyBufferInfo {
                buffer: &readback,
                layout: wgpu::TexelCopyBufferLayout { offset: 0, bytes_per_row: Some(padded), rows_per_image: Some(h) },
            },
            extent,
        );
        self.queue.submit([enc.finish()]);
        readback.slice(..).map_async(wgpu::MapMode::Read, |_| {});
        let _ = self.device.poll(wgpu::PollType::wait_indefinitely());
        let data = readback.slice(..).get_mapped_range().expect("readback mapped");
        let mut out = Vec::with_capacity((w * h * 4) as usize);
        for row in 0..h {
            let start = (row * padded) as usize;
            out.extend_from_slice(&data[start..start + (w * 4) as usize]);
        }
        out
    }
}

/// Uploads an image with a full mip chain: the file's own mips (DDS) or box-filtered
/// ones (Targa), as `TextureLoader` generates them.
fn upload(device: &wgpu::Device, queue: &wgpu::Queue, img: &Image) -> wgpu::TextureView {
    let mut levels: Vec<Vec<u8>> = img.levels.clone();
    let (mut w, mut h) = (img.width, img.height);
    let full = 32 - img.width.max(img.height).leading_zeros();
    for _ in 0..levels.len().saturating_sub(1) {
        w = (w / 2).max(1);
        h = (h / 2).max(1);
    }
    while (levels.len() as u32) < full {
        let prev = levels.last().unwrap();
        let (nw, nh) = ((w / 2).max(1), (h / 2).max(1));
        let mut next = vec![0u8; (nw * nh * 4) as usize];
        for y in 0..nh {
            for x in 0..nw {
                for c in 0..4 {
                    let mut sum = 0u32;
                    for (dx, dy) in [(0, 0), (1, 0), (0, 1), (1, 1)] {
                        let sx = (x * 2 + dx).min(w - 1);
                        let sy = (y * 2 + dy).min(h - 1);
                        sum += u32::from(prev[((sy * w + sx) * 4 + c) as usize]);
                    }
                    next[((y * nw + x) * 4 + c) as usize] = (sum / 4) as u8;
                }
            }
        }
        levels.push(next);
        w = nw;
        h = nh;
    }
    let tex = device.create_texture(&wgpu::TextureDescriptor {
        label: None,
        size: wgpu::Extent3d { width: img.width, height: img.height, depth_or_array_layers: 1 },
        mip_level_count: levels.len() as u32,
        sample_count: 1,
        dimension: wgpu::TextureDimension::D2,
        format: COLOR_FORMAT,
        usage: wgpu::TextureUsages::TEXTURE_BINDING | wgpu::TextureUsages::COPY_DST,
        view_formats: &[],
    });
    let (mut lw, mut lh) = (img.width, img.height);
    for (i, data) in levels.iter().enumerate() {
        queue.write_texture(
            wgpu::TexelCopyTextureInfo {
                texture: &tex,
                mip_level: i as u32,
                origin: wgpu::Origin3d::ZERO,
                aspect: wgpu::TextureAspect::All,
            },
            data,
            wgpu::TexelCopyBufferLayout { offset: 0, bytes_per_row: Some(lw * 4), rows_per_image: Some(lh) },
            wgpu::Extent3d { width: lw, height: lh, depth_or_array_layers: 1 },
        );
        lw = (lw / 2).max(1);
        lh = (lh / 2).max(1);
    }
    tex.create_view(&Default::default())
}
