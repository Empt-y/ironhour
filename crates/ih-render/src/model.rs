//! Turning W3D render objects into draw batches.
//!
//! Follows how WW3D2 prepares a mesh for drawing:
//! - `hlod.cpp`: an HLOD draws its highest-detail LOD plus its aggregates; each
//!   sub-object is attached to a bone. Rigid mesh vertices are in that bone's space;
//!   skin vertices are in the space of their per-vertex bone (`meshmdl.cpp`,
//!   `get_deformed_vertices`).
//! - `meshmatdesc.cpp` (`Post_Load_Process`): per-vertex colours (DCG) are
//!   pre-multiplied by the material channels in use and decide which material colours
//!   come from the vertex; DIG is multiplied into DCG; prelit-vertex meshes are unlit.
//! - `W3DAssetManager.cpp`: meshes named `HOUSECOLOR...` take the house colour as their
//!   material ambient and diffuse.

use bytemuck::{Pod, Zeroable};
use ih_formats::w3d::{self, Hierarchy, Mesh, Shader, VertexMaterial, mesh_flags};

use crate::assets::Assets;

/// One vertex as the shaders see it. Colours are 0..=1.
#[repr(C)]
#[derive(Debug, Clone, Copy, Pod, Zeroable)]
pub struct Vertex {
    pub position: [f32; 3],
    pub normal: [f32; 3],
    pub uv0: [f32; 2],
    pub uv1: [f32; 2],
    /// Diffuse colour and alpha (material or vertex colour).
    pub diffuse: [f32; 4],
    pub ambient: [f32; 3],
    pub emissive: [f32; 3],
    pub bone: u32,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TextureSlot {
    pub name: String,
    pub clamp_u: bool,
    pub clamp_v: bool,
}

/// Texture coordinate generation for one stage (`WW3D2/mapper.cpp`). Time-based
/// mappers are stateless functions of time here; the original advanced state each
/// frame, which gives the same result.
#[derive(Debug, Clone, PartialEq)]
pub enum Mapper {
    /// The mesh's UVs unchanged.
    Uv,
    /// UV * scale (`ScaleTextureMapperClass`).
    Scale { scale: [f32; 2] },
    /// Scrolling UVs (`LinearOffsetTextureMapperClass`): scale, starting offset, offset
    /// change per millisecond, and whether to clamp instead of wrap.
    LinearOffset { scale: [f32; 2], start: [f32; 2], per_ms: [f32; 2], clamp_fix: bool },
    /// Flipbook cells (`GridTextureMapperClass`).
    Grid { log2_width: u32, last: u32, offset: u32, ms_per_frame: u32, sign: i32 },
    /// Rotation about a centre (`RotateTextureMapperClass`).
    Rotate { scale: [f32; 2], center: [f32; 2], rad_per_ms: f32 },
    /// Camera-space reflection vector (`EnvironmentMapperClass`).
    Environment,
    /// Camera-space normal (`ClassicEnvironmentMapperClass`).
    ClassicEnvironment,
    /// World-space reflection (`WSEnvironmentMapperClass`), projected along an axis
    /// (0 = X, 1 = Y, 2 = Z).
    WsEnvironment { axis: u32 },
    /// World-space normal (`WSClassicEnvironmentMapperClass`).
    WsClassicEnvironment { axis: u32 },
    /// Not implemented yet; drawn with plain UVs. Holds the `W3DVERTMAT` mapping type.
    Unsupported(u32),
}

impl Mapper {
    /// The mapper for `stage` from a vertex material's attribute bits and arguments
    /// (`VertexMaterialClass::Parse_Mapping_Args`).
    pub fn from_material(vm: &VertexMaterial, stage: usize) -> Mapper {
        let kind = (vm.info.attributes >> if stage == 0 { 16 } else { 8 }) & 0xFF;
        let args = parse_args(&vm.mapper_args[stage]);
        let f = |k: &str, d: f32| -> f32 {
            args.iter().find(|(n, _)| n.eq_ignore_ascii_case(k)).and_then(|(_, v)| v.trim().parse().ok()).unwrap_or(d)
        };
        let i = |k: &str, d: i64| -> i64 {
            args.iter()
                .find(|(n, _)| n.eq_ignore_ascii_case(k))
                .and_then(|(_, v)| v.trim().parse::<f64>().ok())
                .map_or(d, |v| v as i64)
        };
        let axis = || -> u32 {
            match args.iter().find(|(n, _)| n.eq_ignore_ascii_case("Axis")).and_then(|(_, v)| v.trim().chars().next()) {
                Some('X' | 'x') => 0,
                Some('Y' | 'y') => 1,
                _ => 2,
            }
        };
        let scale = || [f("UScale", 1.0), f("VScale", 1.0)];
        match kind {
            0 => Mapper::Uv,
            1 => Mapper::Environment,
            2 => Mapper::ClassicEnvironment,
            4 => Mapper::LinearOffset {
                scale: scale(),
                start: [f("UOffset", 0.0), f("VOffset", 0.0)],
                per_ms: [f("UPerSec", 0.0) * -0.001, f("VPerSec", 0.0) * -0.001],
                clamp_fix: args.iter().any(|(n, v)| n.eq_ignore_ascii_case("ClampFix") && is_true(v)),
            },
            6 => Mapper::Scale { scale: scale() },
            7 => {
                let fps = f("FPS", 1.0);
                let log2_width = i("Log2Width", 1).clamp(0, 15) as u32;
                let mut last = i("Last", 0).max(0) as u32;
                if last == 0 {
                    last = (1 << log2_width) * (1 << log2_width);
                }
                let offset = i("Offset", 0).max(0) as u32 % last.max(1);
                let (sign, ms_per_frame) =
                    if fps == 0.0 { (0, 1) } else { (if fps < 0.0 { -1 } else { 1 }, (1000.0 / fps.abs()) as u32) };
                Mapper::Grid { log2_width, last, offset, ms_per_frame: ms_per_frame.max(1), sign }
            }
            8 => Mapper::Rotate {
                scale: scale(),
                center: [f("UCenter", 0.0), f("VCenter", 0.0)],
                rad_per_ms: 2.0 * std::f32::consts::PI * f("Speed", 0.1) / 1000.0,
            },
            12 => Mapper::WsClassicEnvironment { axis: axis() },
            13 => Mapper::WsEnvironment { axis: axis() },
            other => Mapper::Unsupported(other),
        }
    }

    /// Generator mode for the shader: 0 UV (with matrix), 1 camera reflection,
    /// 2 camera normal, 3 world reflection, 4 world normal; and the axis.
    pub fn mode(&self) -> (u32, u32) {
        match self {
            Mapper::Environment => (1, 2),
            Mapper::ClassicEnvironment => (2, 2),
            Mapper::WsEnvironment { axis } => (3, *axis),
            Mapper::WsClassicEnvironment { axis } => (4, *axis),
            _ => (0, 2),
        }
    }

    /// The UV transform at `time_ms` as two rows `(a, b, c)`: u' = a*u + b*v + c.
    pub fn matrix(&self, time_ms: f32) -> [[f32; 3]; 2] {
        let id = [[1.0, 0.0, 0.0], [0.0, 1.0, 0.0]];
        match *self {
            Mapper::Scale { scale } => [[scale[0], 0.0, 0.0], [0.0, scale[1], 0.0]],
            Mapper::LinearOffset { scale, start, per_ms, clamp_fix } => {
                let mut o = [start[0] + per_ms[0] * time_ms, start[1] + per_ms[1] * time_ms];
                for k in 0..2 {
                    o[k] = if clamp_fix { o[k].clamp(-scale[k], scale[k]) } else { o[k] - o[k].floor() };
                }
                [[scale[0], 0.0, o[0]], [0.0, scale[1], o[1]]]
            }
            Mapper::Grid { log2_width, last, offset, ms_per_frame, sign } => {
                let steps = (time_ms.max(0.0) as u64 / u64::from(ms_per_frame)) as i64;
                let start = if sign >= 0 { offset as i64 } else { last as i64 - 1 - offset as i64 };
                let frame = (start + steps * i64::from(sign)).rem_euclid(i64::from(last.max(1))) as u32;
                let mask = !(u32::MAX << log2_width);
                let inv = 1.0 / (1u32 << log2_width) as f32;
                let (x, y) = (frame & mask, (frame >> log2_width) & mask);
                [[1.0, 0.0, x as f32 * inv], [0.0, 1.0, y as f32 * inv]]
            }
            Mapper::Rotate { scale, center, rad_per_ms } => {
                let a = (rad_per_ms * time_ms).rem_euclid(2.0 * std::f32::consts::PI);
                let (s, c) = a.sin_cos();
                let (cu, cv) = (center[0], center[1]);
                [
                    [scale[0] * c, -scale[0] * s, -scale[0] * (c * cu - s * cv - cu)],
                    [scale[1] * s, scale[1] * c, -scale[1] * (s * cu + c * cv - cv)],
                ]
            }
            _ => id,
        }
    }
}

fn is_true(v: &str) -> bool {
    matches!(v.trim().to_ascii_lowercase().as_str(), "true" | "yes" | "1")
}

/// Mapper arguments: `Key=Value` lines (an INI `[Args]` section in the original).
fn parse_args(text: &str) -> Vec<(String, String)> {
    text.lines()
        .filter_map(|l| {
            let l = l.split(';').next()?.trim();
            let (k, v) = l.split_once('=')?;
            Some((k.trim().to_string(), v.trim().to_string()))
        })
        .collect()
}

/// A run of triangles drawn with one shader and texture set.
#[derive(Debug, Clone)]
pub struct Batch {
    pub mesh: String,
    pub pass: usize,
    pub vertices: Vec<Vertex>,
    pub indices: Vec<u32>,
    pub shader: Shader,
    pub lighting: bool,
    pub textures: [Option<TextureSlot>; 2],
    /// How each texture stage gets its coordinates.
    pub mappers: [Mapper; 2],
    pub two_sided: bool,
    pub sort_level: i32,
}

impl Batch {
    /// Drawn after opaque batches (any blending).
    pub fn is_translucent(&self) -> bool {
        !(self.shader.src_blend == 1 && self.shader.dest_blend == 0)
    }
}

#[derive(Debug, Clone)]
pub struct Model {
    pub name: String,
    pub hierarchy: Option<Hierarchy>,
    pub batches: Vec<Batch>,
    /// House colour for `ZHC` textures (see [`crate::house_color`]).
    pub house_color: Option<u32>,
}

#[derive(Debug, thiserror::Error)]
pub enum ModelError {
    #[error("{0}: no model file")]
    NoFile(String),
    #[error("{0}: no HLOD or mesh of that name")]
    NoObject(String),
}

/// Options for building a model.
#[derive(Debug, Clone, Default)]
pub struct BuildOptions {
    /// House colour for `HOUSECOLOR` meshes, 0xRRGGBB.
    pub house_color: Option<u32>,
    /// Sub-object names to leave out (case-insensitive, matched on the part after the
    /// container name, by prefix), e.g. muzzle flashes.
    pub hide: Vec<String>,
}

pub fn build(assets: &mut Assets, name: &str, opts: &BuildOptions) -> Result<Model, ModelError> {
    let file = assets.w3d(name).ok_or_else(|| ModelError::NoFile(name.into()))?;
    let mut model =
        Model { name: name.to_string(), hierarchy: None, batches: Vec::new(), house_color: opts.house_color };

    if let Some(hlod) = file.hlods.iter().find(|h| h.name.eq_ignore_ascii_case(name)) {
        model.hierarchy = assets.hierarchy(&hlod.hierarchy, Some(&file));
        // The highest-detail LOD is the one with the largest max screen size.
        let best = hlod.lods.iter().max_by(|a, b| a.max_screen_size.total_cmp(&b.max_screen_size));
        let subs = best.into_iter().flat_map(|l| &l.objects).chain(&hlod.aggregates);
        for sub in subs {
            let short = sub.name.split_once('.').map_or(sub.name.as_str(), |(_, m)| m);
            if opts.hide.iter().any(|h| short.to_ascii_lowercase().starts_with(&h.to_ascii_lowercase())) {
                continue;
            }
            match assets.mesh(&sub.name, &file) {
                Some((f, i)) => add_mesh(&mut model, &f.meshes[i], sub.bone, opts),
                None => {
                    // Boxes and other helpers are not drawn; anything else is missing.
                    let helper = file.boxes.iter().any(|b| b.name.eq_ignore_ascii_case(&sub.name));
                    if !helper {
                        assets.warnings.push(format!("{}: sub-object {} not found", name, sub.name));
                    }
                }
            }
        }
        return Ok(model);
    }

    // A plain mesh model.
    let mesh = file
        .meshes
        .iter()
        .find(|m| m.full_name().eq_ignore_ascii_case(name) || m.name.eq_ignore_ascii_case(name))
        .ok_or_else(|| ModelError::NoObject(name.into()))?;
    add_mesh(&mut model, mesh, 0, opts);
    Ok(model)
}

fn rgb(c: w3d::Rgb) -> [f32; 3] {
    [c.r as f32 / 255.0, c.g as f32 / 255.0, c.b as f32 / 255.0]
}

fn nonzero(c: w3d::Rgb) -> bool {
    c.r != 0 || c.g != 0 || c.b != 0
}

/// `VertexMaterialClass` defaults: white ambient and diffuse, opaque, no emissive.
fn default_material() -> VertexMaterial {
    let mut m = VertexMaterial::default();
    m.info.ambient = w3d::Rgb { r: 255, g: 255, b: 255 };
    m.info.diffuse = w3d::Rgb { r: 255, g: 255, b: 255 };
    m.info.opacity = 1.0;
    m
}

/// `ShaderClass` defaults (`W3DSHADER_*_DEFAULT`).
pub fn default_shader() -> Shader {
    Shader { depth_compare: 3, depth_mask: 1, src_blend: 1, pri_gradient: 1, ..Shader::default() }
}

#[derive(Clone, Copy, PartialEq)]
enum Src {
    Material,
    Vertex,
}

fn add_mesh(model: &mut Model, mesh: &Mesh, bone: u32, opts: &BuildOptions) {
    if mesh.attributes & mesh_flags::HIDDEN != 0 {
        return;
    }
    let Some(mats) = mesh.active_materials() else { return };
    let house = {
        let short = mesh.name.to_ascii_uppercase();
        short.starts_with("HOUSECOLOR")
    };
    let skin = mesh.is_skin();
    let prelit_vertex = mesh.attributes & mesh_flags::PRELIT_VERTEX != 0
        && mesh.attributes & mesh_flags::PRELIT_LIGHTMAP_MULTI_PASS == 0;
    let nverts = mesh.vertices.len();
    let default_vm = default_material();

    for (pi, pass) in mats.passes.iter().enumerate() {
        // Material per vertex for this pass.
        let vmat = |v: usize| -> VertexMaterial {
            let mut m = pass
                .vertex_material_ids
                .as_ref()
                .and_then(|ids| ids.get(v))
                .and_then(|id| mats.vertex_materials.get(id as usize))
                .cloned()
                .unwrap_or_else(|| default_vm.clone());
            if house && let Some(c) = opts.house_color {
                let hc = w3d::Rgb { r: (c >> 16) as u8, g: (c >> 8) as u8, b: c as u8 };
                m.info.ambient = hc;
                m.info.diffuse = hc;
            }
            m
        };

        // Which channels the materials use (Post_Load_Process).
        let (mut diffuse_used, mut ambient_used, mut emissive_used) = (false, false, false);
        for v in 0..nverts.max(1) {
            let m = vmat(v);
            diffuse_used |= nonzero(m.info.diffuse);
            ambient_used |= nonzero(m.info.ambient);
            emissive_used |= nonzero(m.info.emissive);
        }

        // Vertex colours: DIG is multiplied into DCG, then DCG is pre-multiplied by
        // the channel it stands in for.
        let mut dcg: Option<Vec<[f32; 4]>> = pass.dcg.as_ref().map(|c| {
            c.iter()
                .map(|p| [p[0] as f32 / 255.0, p[1] as f32 / 255.0, p[2] as f32 / 255.0, p[3] as f32 / 255.0])
                .collect()
        });
        if let (Some(d), Some(dig)) = (dcg.as_mut(), pass.dig.as_ref()) {
            for (c, e) in d.iter_mut().zip(dig) {
                for k in 0..3 {
                    c[k] *= e[k] as f32 / 255.0;
                }
            }
        }
        let (mut amb_src, mut diff_src) = (Src::Material, if dcg.is_some() { Src::Vertex } else { Src::Material });
        let mut lighting = !prelit_vertex;
        if let Some(d) = dcg.as_mut() {
            let mult = |d: &mut Vec<[f32; 4]>, f: &dyn Fn(&VertexMaterial) -> [f32; 3]| {
                for (v, c) in d.iter_mut().enumerate() {
                    let m = vmat(v);
                    let k = f(&m);
                    c[0] *= k[0];
                    c[1] *= k[1];
                    c[2] *= k[2];
                    c[3] *= m.info.opacity;
                }
            };
            match (diffuse_used, ambient_used, emissive_used) {
                (true, false, false) => mult(d, &|m| rgb(m.info.diffuse)),
                (true, true, false) => {
                    mult(d, &|m| rgb(m.info.diffuse));
                    amb_src = Src::Vertex;
                }
                (false, true, false) => {
                    mult(d, &|m| rgb(m.info.ambient));
                    amb_src = Src::Vertex;
                    diff_src = Src::Material;
                }
                (false, false, true) => {
                    mult(d, &|m| rgb(m.info.emissive));
                    // Emissive-only with vertex colours: drawn unlit.
                    lighting = false;
                }
                _ => {}
            }
        }

        let uv = |stage: usize, v: usize| -> [f32; 2] {
            let s = pass.stages.get(stage).and_then(|s| s.texcoords.as_ref());
            let s = s.or_else(|| pass.stages.first().and_then(|s| s.texcoords.as_ref()));
            s.and_then(|t| t.get(v)).copied().unwrap_or([0.0, 0.0])
        };

        let vertices: Vec<Vertex> = (0..nverts)
            .map(|v| {
                let m = vmat(v);
                let diffuse = match (diff_src, &dcg) {
                    (Src::Vertex, Some(d)) => d[v],
                    _ => {
                        let c = rgb(m.info.diffuse);
                        [c[0], c[1], c[2], m.info.opacity]
                    }
                };
                let ambient = match (amb_src, &dcg) {
                    (Src::Vertex, Some(d)) => [d[v][0], d[v][1], d[v][2]],
                    _ => rgb(m.info.ambient),
                };
                Vertex {
                    position: mesh.vertices[v],
                    normal: mesh.normals.get(v).copied().unwrap_or([0.0, 0.0, 1.0]),
                    uv0: uv(0, v),
                    uv1: uv(1, v),
                    diffuse: if lighting || dcg.is_some() { diffuse } else { [1.0, 1.0, 1.0, diffuse[3]] },
                    ambient,
                    emissive: rgb(m.info.emissive),
                    bone: if skin { mesh.influences.get(v).copied().unwrap_or(0) as u32 } else { bone },
                }
            })
            .collect();

        // Group triangles by shader and textures (per-triangle ids are allowed).
        let tex_id = |stage: usize, t: usize| -> Option<u32> {
            pass.stages.get(stage).and_then(|s| s.texture_ids.as_ref()).and_then(|ids| ids.get(t))
        };
        // The vertex material (and so its texture mappers) of a triangle's first vertex.
        let tri_mat = |ti: usize| -> Option<u32> {
            let v = mesh.triangles[ti].indices[0] as usize;
            pass.vertex_material_ids.as_ref().and_then(|ids| ids.get(v))
        };
        type Key = (Option<u32>, Option<u32>, Option<u32>, Option<u32>);
        let mut groups: Vec<(Key, Vec<u32>)> = Vec::new();
        for (ti, tri) in mesh.triangles.iter().enumerate() {
            let key = (pass.shader_ids.as_ref().and_then(|s| s.get(ti)), tex_id(0, ti), tex_id(1, ti), tri_mat(ti));
            let idx = match groups.iter().position(|(k, _)| *k == key) {
                Some(i) => i,
                None => {
                    groups.push((key, Vec::new()));
                    groups.len() - 1
                }
            };
            if tri.indices.iter().all(|&i| (i as usize) < nverts) {
                groups[idx].1.extend_from_slice(&tri.indices);
            }
        }

        let slot = |id: Option<u32>| -> Option<TextureSlot> {
            let t = mats.textures.get(id? as usize)?;
            let attr = t.info.map_or(0, |i| i.attributes);
            Some(TextureSlot { name: t.name.clone(), clamp_u: attr & 0x0008 != 0, clamp_v: attr & 0x0010 != 0 })
        };
        for ((shader_id, t0, t1, mat_id), indices) in groups {
            if indices.is_empty() {
                continue;
            }
            let shader = shader_id.and_then(|i| mats.shaders.get(i as usize)).copied().unwrap_or_else(default_shader);
            let textures = if shader.texturing == 1 { [slot(t0), slot(t1)] } else { [None, None] };
            let mappers = match mat_id.and_then(|i| mats.vertex_materials.get(i as usize)) {
                Some(vm) => [Mapper::from_material(vm, 0), Mapper::from_material(vm, 1)],
                None => [Mapper::Uv, Mapper::Uv],
            };
            model.batches.push(Batch {
                mesh: mesh.full_name(),
                pass: pi,
                vertices: vertices.clone(),
                indices,
                shader,
                lighting,
                textures,
                mappers,
                two_sided: mesh.attributes & mesh_flags::TWO_SIDED != 0,
                sort_level: mesh.sort_level,
            });
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn vm(attributes: u32, args: &str) -> VertexMaterial {
        let mut m = VertexMaterial::default();
        m.info.attributes = attributes;
        m.mapper_args[0] = args.to_string();
        m
    }

    #[test]
    fn linear_offset_scrolls_and_wraps() {
        // UPerSec = 0.5: the offset moves by -0.5 per second and wraps into 0..1.
        let m = Mapper::from_material(&vm(4 << 16, "UPerSec=0.5\r\nVPerSec=0\r\n"), 0);
        let t = m.matrix(1000.0);
        assert!((t[0][2] - 0.5).abs() < 1e-6, "{t:?}");
        assert_eq!(t[0][0], 1.0);
    }

    #[test]
    fn grid_steps_through_cells() {
        // 2x2 grid at 10 fps: frame 3 after 300 ms is cell (1, 1).
        let m = Mapper::from_material(&vm(7 << 16, "FPS=10\nLog2Width=1\n"), 0);
        let t = m.matrix(300.0);
        assert_eq!((t[0][2], t[1][2]), (0.5, 0.5));
        assert_eq!(m.matrix(400.0)[0][2], 0.0, "wraps after the last cell");
    }

    #[test]
    fn environment_modes() {
        assert_eq!(Mapper::from_material(&vm(1 << 16, ""), 0).mode(), (1, 2));
        assert_eq!(Mapper::from_material(&vm(13 << 8, "Axis=X"), 1), Mapper::WsEnvironment { axis: 2 });
    }
}
