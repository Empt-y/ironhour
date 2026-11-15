//! The wgpu side: device, pipelines per render state, textures, and scenes that are
//! uploaded once and drawn every frame (offscreen or to a window).

use std::collections::HashMap;
use std::sync::Arc;

use bytemuck::{Pod, Zeroable};
use glam::{Mat4, Vec3};
use ih_formats::texture::Image;
use ih_formats::w3d::Shader;
use wgpu::util::DeviceExt;

use crate::assets::Assets;
use crate::model::{Mapper, Model, TextureSlot, Vertex};
use crate::roads::RoadMesh;
use crate::terrain::{Atlas, TerrainMesh, TerrainVertex};
use crate::water::{WaterMesh, WaterSettings, WaterVertex};

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

    /// A map's object lights for its time of day. As in `W3DScene`, only the first
    /// light's ambient is used.
    pub fn from_map(map: &ih_formats::map::Map) -> Option<Self> {
        let l = map.lighting.as_ref()?.current();
        let dir = |p: [f32; 3]| {
            let v = Vec3::from(p);
            if v.length_squared() > 0.0 { v.normalize() } else { Vec3::NEG_Z }
        };
        Some(Self {
            ambient: Vec3::from(l.objects[0].ambient),
            lights: [0, 1, 2].map(|i| (dir(l.objects[i].position), Vec3::from(l.objects[i].diffuse))),
        })
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
    pub fn view(&self) -> Mat4 {
        glam::camera::rh::view::look_at_mat4(self.eye, self.target, Vec3::Z)
    }

    pub fn view_proj(&self, aspect: f32) -> Mat4 {
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

#[repr(C)]
#[derive(Clone, Copy, Pod, Zeroable)]
struct TerrainU {
    cloud: [f32; 4],
    mode: [f32; 4],
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

/// Offscreen colour format; a window uses its surface's format instead.
pub const COLOR_FORMAT: wgpu::TextureFormat = wgpu::TextureFormat::Rgba8Unorm;
pub const DEPTH_FORMAT: wgpu::TextureFormat = wgpu::TextureFormat::Depth32Float;
pub const SAMPLES: u32 = 4;
/// `alphareference = 0x60` in `ShaderClass::Apply`.
const ALPHA_REF: f32 = 96.0 / 255.0;

/// One draw of a prepared model.
struct Draw {
    key: PipeKey,
    sub_object: usize,
    vb: wgpu::Buffer,
    ib: wgpu::Buffer,
    count: u32,
    bg: wgpu::BindGroup,
    ub: wgpu::Buffer,
    uniforms: DrawU,
    mappers: [Mapper; 2],
    translucent: bool,
    sort_level: i32,
}

/// A model's geometry, textures and states on the GPU, shared by its instances.
pub struct PreparedModel {
    draws: Vec<Draw>,
    animated: bool,
}

/// A placed model: shared geometry plus its own pose.
pub struct Instance {
    model: Arc<PreparedModel>,
    bones: wgpu::BindGroup,
    /// Per sub-object: hidden.
    hidden: Option<Arc<[bool]>>,
}

pub struct PreparedTerrain {
    vb: wgpu::Buffer,
    ib: wgpu::Buffer,
    count: u32,
    extra: Option<(wgpu::Buffer, wgpu::Buffer, u32)>,
    ub: wgpu::Buffer,
    extra_ub: wgpu::Buffer,
    bg: wgpu::BindGroup,
    extra_bg: wgpu::BindGroup,
    use_cloud: bool,
    use_noise: bool,
}

/// A textured terrain surface: road ribbons or a bridge mesh. The scene chooses
/// the road's no-depth-write state or the bridge's alpha-test/depth-write state.
pub struct PreparedRoad {
    vb: wgpu::Buffer,
    ib: wgpu::Buffer,
    count: u32,
    ub: wgpu::Buffer,
    bg: wgpu::BindGroup,
    use_cloud: bool,
    use_noise: bool,
}

#[repr(C)]
#[derive(Clone, Copy, Pod, Zeroable)]
struct WaterU {
    color: [f32; 4],
    params: [f32; 4],
    map: [f32; 4],
}

pub struct PreparedWater {
    vb: wgpu::Buffer,
    ib: wgpu::Buffer,
    count: u32,
    ub: wgpu::Buffer,
    uniforms: WaterU,
    bg: wgpu::BindGroup,
    additive: bool,
}

/// What to draw.
#[derive(Default)]
pub struct Scene {
    pub terrain: Option<PreparedTerrain>,
    pub roads: Vec<PreparedRoad>,
    pub bridges: Vec<PreparedRoad>,
    pub water: Option<PreparedWater>,
    pub instances: Vec<Instance>,
}

/// Render targets of one size: multisampled colour and depth.
pub struct Targets {
    pub size: (u32, u32),
    msaa: wgpu::TextureView,
    depth: wgpu::TextureView,
}

pub struct Renderer {
    pub device: wgpu::Device,
    pub queue: wgpu::Queue,
    pub format: wgpu::TextureFormat,
    module: wgpu::ShaderModule,
    layout: wgpu::PipelineLayout,
    bones_layout: wgpu::BindGroupLayout,
    draw_layout: wgpu::BindGroupLayout,
    terrain_layout: wgpu::BindGroupLayout,
    terrain_pipelines: [wgpu::RenderPipeline; 4],
    water_layout: wgpu::BindGroupLayout,
    water_pipelines: [wgpu::RenderPipeline; 2],
    pipelines: HashMap<PipeKey, wgpu::RenderPipeline>,
    textures: HashMap<String, Arc<wgpu::TextureView>>,
    white: Arc<wgpu::TextureView>,
    samplers: HashMap<(bool, bool), Arc<wgpu::Sampler>>,
    globals: wgpu::Buffer,
    globals_bg: wgpu::BindGroup,
    identity_bones: wgpu::BindGroup,
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
        Self::with_adapter(&adapter, COLOR_FORMAT)
    }

    /// Creates a renderer on `adapter` drawing to `format` (a window surface's).
    pub fn with_adapter(adapter: &wgpu::Adapter, format: wgpu::TextureFormat) -> Result<Self, GpuError> {
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
        let terrain_layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("terrain"),
            entries: &[
                entry(0, vf, uniform),
                entry(1, fs, tex),
                entry(2, fs, samp),
                entry(3, fs, tex),
                entry(4, fs, tex),
                entry(5, fs, samp),
            ],
        });
        let terrain_pipelines = terrain_pipelines(&device, format, &globals_layout, &terrain_layout);
        let water_layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("water"),
            entries: &[
                entry(0, vf, uniform),
                entry(1, fs, tex),
                entry(2, fs, tex),
                entry(3, fs, tex),
                entry(4, fs, tex),
                entry(5, fs, samp),
                entry(6, fs, samp),
                entry(7, fs, tex),
            ],
        });
        let water_pipelines = water_pipelines(&device, format, &globals_layout, &water_layout);
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
        let globals = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("globals"),
            size: std::mem::size_of::<GlobalsU>() as u64,
            usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });
        let globals_bg = device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: None,
            layout: &globals_layout,
            entries: &[wgpu::BindGroupEntry { binding: 0, resource: globals.as_entire_binding() }],
        });
        let identity_bones = bones_bind_group(&device, &bones_layout, &[]);
        Ok(Self {
            device,
            queue,
            format,
            module,
            layout,
            bones_layout,
            draw_layout,
            terrain_layout,
            terrain_pipelines,
            water_layout,
            water_pipelines,
            pipelines: HashMap::new(),
            textures: HashMap::new(),
            white,
            samplers: HashMap::new(),
            globals,
            globals_bg,
            identity_bones,
            adapter_name,
        })
    }

    fn pipeline(&mut self, key: PipeKey) {
        let (device, module, layout, format) = (&self.device, &self.module, &self.layout, self.format);
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
                    targets: &[Some(wgpu::ColorTargetState { format, blend, write_mask: wgpu::ColorWrites::ALL })],
                }),
                multiview_mask: None,
                cache: None,
            })
        });
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

    fn named_texture(&mut self, assets: &mut Assets, name: &str) -> Arc<wgpu::TextureView> {
        let key = name.to_ascii_lowercase();
        if let Some(t) = self.textures.get(&key) {
            return t.clone();
        }
        let view = match assets.texture(name) {
            Some(img) => Arc::new(upload(&self.device, &self.queue, &img)),
            None => self.white.clone(),
        };
        self.textures.insert(key, view.clone());
        view
    }

    fn texture(&mut self, assets: &mut Assets, slot: &TextureSlot, house: Option<u32>) -> Arc<wgpu::TextureView> {
        let house = house.filter(|_| crate::house_color::applies(&slot.name));
        let Some(c) = house else { return self.named_texture(assets, &slot.name) };
        let key = format!("{}#{c:06x}", slot.name.to_ascii_lowercase());
        if let Some(t) = self.textures.get(&key) {
            return t.clone();
        }
        let view = match assets.texture(&slot.name) {
            Some(img) => {
                let recolored = crate::house_color::recolor(&slot.name, &img, c);
                Arc::new(upload(&self.device, &self.queue, recolored.as_ref().unwrap_or(&img)))
            }
            None => self.white.clone(),
        };
        self.textures.insert(key, view.clone());
        view
    }

    /// Uploads a model's batches. Instances of it share the result.
    pub fn prepare_model(&mut self, assets: &mut Assets, model: &Model) -> Arc<PreparedModel> {
        let mut draws = Vec::new();
        let mut animated = false;
        for b in &model.batches {
            let s: &Shader = &b.shader;
            let uniforms = DrawU {
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
                tm: tm_rows(&b.mappers, 0.0),
            };
            animated |= b.mappers.iter().any(Mapper::is_animated);
            let ub = self.device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
                label: None,
                contents: bytemuck::bytes_of(&uniforms),
                usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
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
            draws.push(Draw {
                key,
                sub_object: b.sub_object,
                vb,
                ib,
                count: b.indices.len() as u32,
                bg,
                ub,
                uniforms,
                mappers: b.mappers.clone(),
                translucent: b.is_translucent(),
                sort_level: b.sort_level,
            });
        }
        Arc::new(PreparedModel { draws, animated })
    }

    /// Places a prepared model with one world matrix per hierarchy pivot.
    pub fn instance(&self, model: &Arc<PreparedModel>, bones: &[Mat4]) -> Instance {
        Instance {
            model: model.clone(),
            bones: bones_bind_group(&self.device, &self.bones_layout, bones),
            hidden: None,
        }
    }

    /// Places a prepared model with some sub-objects hidden (`hidden[i]` for
    /// `Model::sub_objects[i]`).
    pub fn instance_hiding(&self, model: &Arc<PreparedModel>, bones: &[Mat4], hidden: Arc<[bool]>) -> Instance {
        Instance { hidden: Some(hidden), ..self.instance(model, bones) }
    }

    /// Uploads terrain geometry and its atlas. `cloud` and `noise` are the
    /// `TSCloudMed.tga` and macro (`TSNoiseUrb.tga`) textures when enabled.
    pub fn prepare_terrain(
        &mut self,
        assets: &mut Assets,
        atlas: &Atlas,
        mesh: &TerrainMesh,
        cloud: Option<&str>,
        noise: Option<&str>,
    ) -> PreparedTerrain {
        let img =
            Image { width: atlas.width, height: atlas.height, levels: vec![atlas.pixels.clone()], has_alpha: false };
        // MIP_LEVELS_3, box filtered.
        let atlas_view = upload_levels(&self.device, &self.queue, &img, 3);
        let cloud_view = cloud.map(|n| self.named_texture(assets, n)).unwrap_or_else(|| self.white.clone());
        let noise_view = noise.map(|n| self.named_texture(assets, n)).unwrap_or_else(|| self.white.clone());
        let clamp = self.device.create_sampler(&wgpu::SamplerDescriptor {
            address_mode_u: wgpu::AddressMode::ClampToEdge,
            address_mode_v: wgpu::AddressMode::ClampToEdge,
            mag_filter: wgpu::FilterMode::Linear,
            min_filter: wgpu::FilterMode::Linear,
            mipmap_filter: wgpu::MipmapFilterMode::Linear,
            ..Default::default()
        });
        let wrap = self.sampler(false, false);
        let mk_ub = |extra: bool| {
            let u = TerrainU { cloud: [0.0; 4], mode: [if extra { 1.0 } else { 0.0 }, 0.0, 0.0, 0.0] };
            self.device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
                label: None,
                contents: bytemuck::bytes_of(&u),
                usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
            })
        };
        let (ub, extra_ub) = (mk_ub(false), mk_ub(true));
        let mk_bg = |ub: &wgpu::Buffer| {
            self.device.create_bind_group(&wgpu::BindGroupDescriptor {
                label: Some("terrain"),
                layout: &self.terrain_layout,
                entries: &[
                    wgpu::BindGroupEntry { binding: 0, resource: ub.as_entire_binding() },
                    wgpu::BindGroupEntry { binding: 1, resource: wgpu::BindingResource::TextureView(&atlas_view) },
                    wgpu::BindGroupEntry { binding: 2, resource: wgpu::BindingResource::Sampler(&clamp) },
                    wgpu::BindGroupEntry { binding: 3, resource: wgpu::BindingResource::TextureView(&cloud_view) },
                    wgpu::BindGroupEntry { binding: 4, resource: wgpu::BindingResource::TextureView(&noise_view) },
                    wgpu::BindGroupEntry { binding: 5, resource: wgpu::BindingResource::Sampler(&wrap) },
                ],
            })
        };
        let (bg, extra_bg) = (mk_bg(&ub), mk_bg(&extra_ub));
        let buffers = |v: &[TerrainVertex], i: &[u32]| {
            let vb = self.device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
                label: Some("terrain"),
                contents: bytemuck::cast_slice(v),
                usage: wgpu::BufferUsages::VERTEX,
            });
            let ib = self.device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
                label: Some("terrain"),
                contents: bytemuck::cast_slice(i),
                usage: wgpu::BufferUsages::INDEX,
            });
            (vb, ib, i.len() as u32)
        };
        let (vb, ib, count) = buffers(&mesh.vertices, &mesh.indices);
        let extra = (!mesh.extra_indices.is_empty()).then(|| buffers(&mesh.extra_vertices, &mesh.extra_indices));
        PreparedTerrain {
            vb,
            ib,
            count,
            extra,
            ub,
            extra_ub,
            bg,
            extra_bg,
            use_cloud: cloud.is_some(),
            use_noise: noise.is_some(),
        }
    }

    /// Uploads road ribbons in draw order. As in `RoadShaderPixelShader::set`
    /// (`W3DShaderManager.cpp`), road colour is modulated by terrain lighting,
    /// cloud and noise; texture alpha blends its edges into the terrain.
    pub fn prepare_roads(
        &mut self,
        assets: &mut Assets,
        meshes: &[RoadMesh],
        cloud: Option<&str>,
        noise: Option<&str>,
    ) -> Vec<PreparedRoad> {
        if meshes.is_empty() {
            return Vec::new();
        }
        let cloud_view = cloud.map(|n| self.named_texture(assets, n)).unwrap_or_else(|| self.white.clone());
        let noise_view = noise.map(|n| self.named_texture(assets, n)).unwrap_or_else(|| self.white.clone());
        let wrap = self.sampler(false, false);
        let mut roads = Vec::new();
        for mesh in meshes.iter().filter(|m| !m.indices.is_empty()) {
            let texture = self.named_texture(assets, &mesh.texture);
            let u = TerrainU { cloud: [0.0; 4], mode: [2.0, 0.0, 0.0, 0.0] };
            let ub = self.device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
                label: Some("road"),
                contents: bytemuck::bytes_of(&u),
                usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
            });
            let bg = self.device.create_bind_group(&wgpu::BindGroupDescriptor {
                label: Some("road"),
                layout: &self.terrain_layout,
                entries: &[
                    wgpu::BindGroupEntry { binding: 0, resource: ub.as_entire_binding() },
                    wgpu::BindGroupEntry { binding: 1, resource: wgpu::BindingResource::TextureView(&texture) },
                    wgpu::BindGroupEntry { binding: 2, resource: wgpu::BindingResource::Sampler(&wrap) },
                    wgpu::BindGroupEntry { binding: 3, resource: wgpu::BindingResource::TextureView(&cloud_view) },
                    wgpu::BindGroupEntry { binding: 4, resource: wgpu::BindingResource::TextureView(&noise_view) },
                    wgpu::BindGroupEntry { binding: 5, resource: wgpu::BindingResource::Sampler(&wrap) },
                ],
            });
            let vb = self.device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
                label: Some("road"),
                contents: bytemuck::cast_slice(&mesh.vertices),
                usage: wgpu::BufferUsages::VERTEX,
            });
            let ib = self.device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
                label: Some("road"),
                contents: bytemuck::cast_slice(&mesh.indices),
                usage: wgpu::BufferUsages::INDEX,
            });
            roads.push(PreparedRoad {
                vb,
                ib,
                count: mesh.indices.len() as u32,
                ub,
                bg,
                use_cloud: cloud.is_some(),
                use_noise: noise.is_some(),
            });
        }
        roads
    }

    /// Uploads the map's standing water. `heights` is the height map (width, height,
    /// samples) for the shoreline opacity.
    #[allow(clippy::too_many_arguments)]
    pub fn prepare_water(
        &mut self,
        assets: &mut Assets,
        mesh: &WaterMesh,
        settings: &WaterSettings,
        color: [f32; 4],
        heights: (u32, u32, &[u8]),
        border: i32,
    ) -> Option<PreparedWater> {
        if mesh.indices.is_empty() {
            return None;
        }
        let water = self.named_texture(assets, &settings.texture);
        let sparkles = self.named_texture(assets, "WaterSurfaceBubbles.tga");
        let noise = self.named_texture(assets, "Noise0000.tga");
        let edge = if mesh.vertices.iter().any(|v| v.river_uv[0] >= 0.0) {
            self.named_texture(assets, "TWAlphaEdge.tga")
        } else {
            self.white.clone()
        };
        let (hw, hh, hdata) = heights;
        let height_tex = self.device.create_texture_with_data(
            &self.queue,
            &wgpu::TextureDescriptor {
                label: Some("heights"),
                size: wgpu::Extent3d { width: hw, height: hh, depth_or_array_layers: 1 },
                mip_level_count: 1,
                sample_count: 1,
                dimension: wgpu::TextureDimension::D2,
                format: wgpu::TextureFormat::R8Unorm,
                usage: wgpu::TextureUsages::TEXTURE_BINDING,
                view_formats: &[],
            },
            wgpu::util::TextureDataOrder::LayerMajor,
            hdata,
        );
        let height_view = height_tex.create_view(&Default::default());
        let wrap = self.sampler(false, false);
        let clamp = self.sampler(true, true);
        let uniforms = WaterU {
            color,
            params: [0.0, settings.transparent_depth * settings.min_opacity, settings.min_opacity, 1.0],
            map: [border as f32, hw as f32, hh as f32, 0.0],
        };
        let ub = self.device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
            label: Some("water"),
            contents: bytemuck::bytes_of(&uniforms),
            usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
        });
        fn tv(v: &wgpu::TextureView) -> wgpu::BindingResource<'_> {
            wgpu::BindingResource::TextureView(v)
        }
        let bg = self.device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("water"),
            layout: &self.water_layout,
            entries: &[
                wgpu::BindGroupEntry { binding: 0, resource: ub.as_entire_binding() },
                wgpu::BindGroupEntry { binding: 1, resource: tv(&water) },
                wgpu::BindGroupEntry { binding: 2, resource: tv(&sparkles) },
                wgpu::BindGroupEntry { binding: 3, resource: tv(&noise) },
                wgpu::BindGroupEntry { binding: 4, resource: tv(&height_view) },
                wgpu::BindGroupEntry { binding: 5, resource: wgpu::BindingResource::Sampler(&wrap) },
                wgpu::BindGroupEntry { binding: 6, resource: wgpu::BindingResource::Sampler(&clamp) },
                wgpu::BindGroupEntry { binding: 7, resource: tv(&edge) },
            ],
        });
        let vb = self.device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
            label: Some("water"),
            contents: bytemuck::cast_slice(&mesh.vertices),
            usage: wgpu::BufferUsages::VERTEX,
        });
        let ib = self.device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
            label: Some("water"),
            contents: bytemuck::cast_slice(&mesh.indices),
            usage: wgpu::BufferUsages::INDEX,
        });
        Some(PreparedWater { vb, ib, count: mesh.indices.len() as u32, ub, uniforms, bg, additive: settings.additive })
    }

    /// Render targets for `size`.
    pub fn targets(&self, size: (u32, u32)) -> Targets {
        let extent = wgpu::Extent3d { width: size.0.max(1), height: size.1.max(1), depth_or_array_layers: 1 };
        let make = |format| {
            self.device
                .create_texture(&wgpu::TextureDescriptor {
                    label: None,
                    size: extent,
                    mip_level_count: 1,
                    sample_count: SAMPLES,
                    dimension: wgpu::TextureDimension::D2,
                    format,
                    usage: wgpu::TextureUsages::RENDER_ATTACHMENT,
                    view_formats: &[],
                })
                .create_view(&Default::default())
        };
        Targets { size, msaa: make(self.format), depth: make(DEPTH_FORMAT) }
    }

    /// Records a frame of `scene` into `enc`, resolving into `output`.
    #[allow(clippy::too_many_arguments)]
    pub fn draw(
        &mut self,
        enc: &mut wgpu::CommandEncoder,
        scene: &Scene,
        camera: &Camera,
        lights: &Lights,
        targets: &Targets,
        output: &wgpu::TextureView,
        clear: [f64; 4],
        time_ms: f32,
    ) {
        let (w, h) = targets.size;
        let mut gu = GlobalsU {
            view_proj: camera.view_proj(w as f32 / h.max(1) as f32).to_cols_array_2d(),
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
        self.queue.write_buffer(&self.globals, 0, bytemuck::bytes_of(&gu));

        // Time-based texture mappers.
        let mut seen = std::collections::HashSet::new();
        for inst in &scene.instances {
            if !inst.model.animated || !seen.insert(Arc::as_ptr(&inst.model)) {
                continue;
            }
            for d in &inst.model.draws {
                if d.mappers.iter().any(Mapper::is_animated) {
                    let mut u = d.uniforms;
                    u.tm = tm_rows(&d.mappers, time_ms);
                    self.queue.write_buffer(&d.ub, 0, bytemuck::bytes_of(&u));
                }
            }
        }
        if let Some(t) = &scene.terrain {
            // TerrainShader2Stage::updateNoise1: -0.02 and -0.03 per second, wrapped.
            let wrap = |v: f32| v - v.trunc();
            let secs = time_ms / 1000.0;
            let cloud = [wrap(-0.02 * secs), wrap(-0.03 * secs)];
            for (ub, extra) in [(&t.ub, 0.0), (&t.extra_ub, 1.0)] {
                let u = TerrainU {
                    cloud: [cloud[0], cloud[1], f32::from(u8::from(t.use_cloud)), f32::from(u8::from(t.use_noise))],
                    mode: [extra, 0.0, 0.0, 0.0],
                };
                self.queue.write_buffer(ub, 0, bytemuck::bytes_of(&u));
            }
        }
        if let Some(wt) = &scene.water {
            // WaterRenderObjClass::update adds 0.002 to m_riverVOrigin each logic frame.
            let mut u = wt.uniforms;
            u.params[0] = time_ms / 1000.0 * 30.0 * 0.002;
            self.queue.write_buffer(&wt.ub, 0, bytemuck::bytes_of(&u));
        }
        for (road, mode) in scene.roads.iter().map(|r| (r, 2.0)).chain(scene.bridges.iter().map(|r| (r, 3.0))) {
            let secs = time_ms / 1000.0;
            let u = TerrainU {
                cloud: [
                    (-0.02 * secs).fract(),
                    (-0.03 * secs).fract(),
                    f32::from(u8::from(road.use_cloud)),
                    f32::from(u8::from(road.use_noise)),
                ],
                mode: [mode, 0.0, 0.0, 0.0],
            };
            self.queue.write_buffer(&road.ub, 0, bytemuck::bytes_of(&u));
        }

        // Opaque before translucent, each in sort-level order; stable otherwise.
        let mut order: Vec<(&Instance, &Draw)> = scene
            .instances
            .iter()
            .flat_map(|i| i.model.draws.iter().map(move |d| (i, d)))
            .filter(|(i, d)| !i.hidden.as_ref().is_some_and(|h| h.get(d.sub_object).copied().unwrap_or(false)))
            .collect();
        order.sort_by_key(|(_, d)| (d.translucent, d.sort_level));
        let split = order.iter().position(|(_, d)| d.translucent).unwrap_or(order.len());

        let mut pass = enc.begin_render_pass(&wgpu::RenderPassDescriptor {
            label: Some("scene"),
            color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                view: &targets.msaa,
                depth_slice: None,
                resolve_target: Some(output),
                ops: wgpu::Operations {
                    load: wgpu::LoadOp::Clear(wgpu::Color { r: clear[0], g: clear[1], b: clear[2], a: clear[3] }),
                    store: wgpu::StoreOp::Store,
                },
            })],
            depth_stencil_attachment: Some(wgpu::RenderPassDepthStencilAttachment {
                view: &targets.depth,
                depth_ops: Some(wgpu::Operations { load: wgpu::LoadOp::Clear(1.0), store: wgpu::StoreOp::Store }),
                stencil_ops: None,
            }),
            timestamp_writes: None,
            occlusion_query_set: None,
            multiview_mask: None,
        });
        pass.set_bind_group(0, &self.globals_bg, &[]);
        let draw_model = |pass: &mut wgpu::RenderPass, list: &[(&Instance, &Draw)]| {
            for (inst, d) in list {
                pass.set_pipeline(&self.pipelines[&d.key]);
                pass.set_bind_group(1, &inst.bones, &[]);
                pass.set_bind_group(2, &d.bg, &[]);
                pass.set_vertex_buffer(0, d.vb.slice(..));
                pass.set_index_buffer(d.ib.slice(..), wgpu::IndexFormat::Uint32);
                pass.draw_indexed(0..d.count, 0, 0..1);
            }
        };
        if let Some(t) = &scene.terrain {
            pass.set_pipeline(&self.terrain_pipelines[0]);
            pass.set_bind_group(1, &t.bg, &[]);
            pass.set_vertex_buffer(0, t.vb.slice(..));
            pass.set_index_buffer(t.ib.slice(..), wgpu::IndexFormat::Uint32);
            pass.draw_indexed(0..t.count, 0, 0..1);
            if let Some((vb, ib, n)) = &t.extra {
                pass.set_pipeline(&self.terrain_pipelines[1]);
                pass.set_bind_group(1, &t.extra_bg, &[]);
                pass.set_vertex_buffer(0, vb.slice(..));
                pass.set_index_buffer(ib.slice(..), wgpu::IndexFormat::Uint32);
                pass.draw_indexed(0..*n, 0, 0..1);
            }
        }
        for road in &scene.roads {
            pass.set_pipeline(&self.terrain_pipelines[2]);
            pass.set_bind_group(1, &road.bg, &[]);
            pass.set_vertex_buffer(0, road.vb.slice(..));
            pass.set_index_buffer(road.ib.slice(..), wgpu::IndexFormat::Uint32);
            pass.draw_indexed(0..road.count, 0, 0..1);
        }
        draw_model(&mut pass, &order[..split]);
        for bridge in &scene.bridges {
            pass.set_pipeline(&self.terrain_pipelines[3]);
            pass.set_bind_group(1, &bridge.bg, &[]);
            pass.set_vertex_buffer(0, bridge.vb.slice(..));
            pass.set_index_buffer(bridge.ib.slice(..), wgpu::IndexFormat::Uint32);
            pass.draw_indexed(0..bridge.count, 0, 0..1);
        }
        if let Some(wt) = &scene.water {
            pass.set_pipeline(&self.water_pipelines[usize::from(wt.additive)]);
            pass.set_bind_group(1, &wt.bg, &[]);
            pass.set_vertex_buffer(0, wt.vb.slice(..));
            pass.set_index_buffer(wt.ib.slice(..), wgpu::IndexFormat::Uint32);
            pass.draw_indexed(0..wt.count, 0, 0..1);
        }
        draw_model(&mut pass, &order[split..]);
    }

    /// Renders `scene` offscreen and returns tightly packed RGBA8 rows.
    pub fn render_scene(
        &mut self,
        scene: &Scene,
        camera: &Camera,
        lights: &Lights,
        size: (u32, u32),
        clear: [f64; 4],
        time_ms: f32,
    ) -> Vec<u8> {
        let (w, h) = size;
        let extent = wgpu::Extent3d { width: w, height: h, depth_or_array_layers: 1 };
        let resolve = self.device.create_texture(&wgpu::TextureDescriptor {
            label: None,
            size: extent,
            mip_level_count: 1,
            sample_count: 1,
            dimension: wgpu::TextureDimension::D2,
            format: self.format,
            usage: wgpu::TextureUsages::RENDER_ATTACHMENT | wgpu::TextureUsages::COPY_SRC,
            view_formats: &[],
        });
        let resolve_view = resolve.create_view(&Default::default());
        let targets = self.targets(size);
        let mut enc = self.device.create_command_encoder(&Default::default());
        self.draw(&mut enc, scene, camera, lights, &targets, &resolve_view, clear, time_ms);

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

    /// Renders one model posed by `bones` (one world matrix per hierarchy pivot) at
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
        let prepared = self.prepare_model(assets, model);
        let instance = if bones.is_empty() {
            Instance { model: prepared, bones: self.identity_bones.clone(), hidden: None }
        } else {
            self.instance(&prepared, bones)
        };
        let scene = Scene { instances: vec![instance], ..Default::default() };
        self.render_scene(&scene, camera, lights, size, clear, time_ms)
    }
}

fn tm_rows(mappers: &[Mapper; 2], time_ms: f32) -> [[f32; 4]; 4] {
    let (t0, t1) = (mappers[0].matrix(time_ms), mappers[1].matrix(time_ms));
    let row = |x: [f32; 3]| [x[0], x[1], x[2], 0.0];
    [row(t0[0]), row(t0[1]), row(t1[0]), row(t1[1])]
}

fn bones_bind_group(device: &wgpu::Device, layout: &wgpu::BindGroupLayout, bones: &[Mat4]) -> wgpu::BindGroup {
    let data: Vec<[[f32; 4]; 4]> = if bones.is_empty() {
        vec![Mat4::IDENTITY.to_cols_array_2d()]
    } else {
        bones.iter().map(|m| m.to_cols_array_2d()).collect()
    };
    let buf = device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
        label: Some("bones"),
        contents: bytemuck::cast_slice(&data),
        usage: wgpu::BufferUsages::STORAGE,
    });
    device.create_bind_group(&wgpu::BindGroupDescriptor {
        label: None,
        layout,
        entries: &[wgpu::BindGroupEntry { binding: 0, resource: buf.as_entire_binding() }],
    })
}

fn water_pipelines(
    device: &wgpu::Device,
    format: wgpu::TextureFormat,
    globals: &wgpu::BindGroupLayout,
    water: &wgpu::BindGroupLayout,
) -> [wgpu::RenderPipeline; 2] {
    let module = device.create_shader_module(wgpu::ShaderModuleDescriptor {
        label: Some("water"),
        source: wgpu::ShaderSource::Wgsl(include_str!("water.wgsl").into()),
    });
    let layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
        label: Some("water"),
        bind_group_layouts: &[Some(globals), Some(water)],
        immediate_size: 0,
    });
    let attrs = wgpu::vertex_attr_array![0 => Float32x3, 1 => Float32x2];
    let make = |additive: bool| {
        // Premultiplied output: alpha blending, or additive (`AdditiveBlending`).
        let dst = if additive { wgpu::BlendFactor::One } else { wgpu::BlendFactor::OneMinusSrcAlpha };
        let comp = wgpu::BlendComponent {
            src_factor: wgpu::BlendFactor::One,
            dst_factor: dst,
            operation: wgpu::BlendOperation::Add,
        };
        device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
            label: Some("water"),
            layout: Some(&layout),
            vertex: wgpu::VertexState {
                module: &module,
                entry_point: Some("vs_main"),
                compilation_options: Default::default(),
                buffers: &[Some(wgpu::VertexBufferLayout {
                    array_stride: std::mem::size_of::<WaterVertex>() as u64,
                    step_mode: wgpu::VertexStepMode::Vertex,
                    attributes: &attrs,
                })],
            },
            primitive: wgpu::PrimitiveState {
                topology: wgpu::PrimitiveTopology::TriangleList,
                // drawTrapezoidWater turns culling off.
                cull_mode: None,
                ..Default::default()
            },
            depth_stencil: Some(wgpu::DepthStencilState {
                format: DEPTH_FORMAT,
                depth_write_enabled: Some(false),
                depth_compare: Some(wgpu::CompareFunction::LessEqual),
                stencil: Default::default(),
                bias: Default::default(),
            }),
            multisample: wgpu::MultisampleState { count: SAMPLES, ..Default::default() },
            fragment: Some(wgpu::FragmentState {
                module: &module,
                entry_point: Some("fs_main"),
                compilation_options: Default::default(),
                targets: &[Some(wgpu::ColorTargetState {
                    format,
                    blend: Some(wgpu::BlendState { color: comp, alpha: comp }),
                    write_mask: wgpu::ColorWrites::ALL,
                })],
            }),
            multiview_mask: None,
            cache: None,
        })
    };
    [make(false), make(true)]
}

fn terrain_pipelines(
    device: &wgpu::Device,
    format: wgpu::TextureFormat,
    globals: &wgpu::BindGroupLayout,
    terrain: &wgpu::BindGroupLayout,
) -> [wgpu::RenderPipeline; 4] {
    let module = device.create_shader_module(wgpu::ShaderModuleDescriptor {
        label: Some("terrain"),
        source: wgpu::ShaderSource::Wgsl(include_str!("terrain.wgsl").into()),
    });
    let layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
        label: Some("terrain"),
        bind_group_layouts: &[Some(globals), Some(terrain)],
        immediate_size: 0,
    });
    let attrs = wgpu::vertex_attr_array![0 => Float32x3, 1 => Float32x4, 2 => Float32x2, 3 => Float32x2];
    let make = |layer: usize| {
        let blend = (layer != 0).then_some(wgpu::BlendState::ALPHA_BLENDING);
        device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
            label: Some(["terrain", "terrain extra blend", "road", "bridge"][layer]),
            layout: Some(&layout),
            vertex: wgpu::VertexState {
                module: &module,
                entry_point: Some("vs_main"),
                compilation_options: Default::default(),
                buffers: &[Some(wgpu::VertexBufferLayout {
                    array_stride: std::mem::size_of::<TerrainVertex>() as u64,
                    step_mode: wgpu::VertexStepMode::Vertex,
                    attributes: &attrs,
                })],
            },
            primitive: wgpu::PrimitiveState {
                topology: wgpu::PrimitiveTopology::TriangleList,
                front_face: wgpu::FrontFace::Ccw,
                cull_mode: if layer == 3 { None } else { Some(wgpu::Face::Back) },
                ..Default::default()
            },
            depth_stencil: Some(wgpu::DepthStencilState {
                format: DEPTH_FORMAT,
                // Extra blends/roads do not write depth; bridges do.
                depth_write_enabled: Some(layer == 0 || layer == 3),
                depth_compare: Some(wgpu::CompareFunction::LessEqual),
                stencil: Default::default(),
                bias: Default::default(),
            }),
            multisample: wgpu::MultisampleState { count: SAMPLES, ..Default::default() },
            fragment: Some(wgpu::FragmentState {
                module: &module,
                entry_point: Some("fs_main"),
                compilation_options: Default::default(),
                targets: &[Some(wgpu::ColorTargetState { format, blend, write_mask: wgpu::ColorWrites::ALL })],
            }),
            multiview_mask: None,
            cache: None,
        })
    };
    [make(0), make(1), make(2), make(3)]
}

/// Uploads an image with a full mip chain: the file's own mips (DDS) or box-filtered
/// ones (Targa), as `TextureLoader` generates them.
fn upload(device: &wgpu::Device, queue: &wgpu::Queue, img: &Image) -> wgpu::TextureView {
    let full = 32 - img.width.max(img.height).leading_zeros();
    upload_levels(device, queue, img, full)
}

/// Uploads an image with `count` mip levels, box-filtering the ones it lacks.
fn upload_levels(device: &wgpu::Device, queue: &wgpu::Queue, img: &Image, count: u32) -> wgpu::TextureView {
    let mut levels: Vec<Vec<u8>> = img.levels.clone();
    levels.truncate(count as usize);
    let (mut w, mut h) = (img.width, img.height);
    for _ in 0..levels.len().saturating_sub(1) {
        w = (w / 2).max(1);
        h = (h / 2).max(1);
    }
    while (levels.len() as u32) < count {
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
