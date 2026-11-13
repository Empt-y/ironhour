//! W3D model files: meshes, hierarchies, animations and HLODs.
//!
//! Ported from `Libraries/Source/WWVegas/WW3D2/`: `w3d_file.h` (layouts),
//! `meshmdlio.cpp` (meshes), `htree.cpp` (hierarchies), `hanim.cpp`/`hrawanim.cpp`,
//! `hcanim.cpp`/`motchan.cpp` (animations), `hlod.cpp` (HLODs), and
//! `WWLib/chunkio.cpp` (the chunk container).
//!
//! A W3D file is a sequence of chunks: `u32 type`, `u32 size` (bit 31 set when the
//! chunk contains sub-chunks), then `size` bytes. Everything is little-endian. Fixed
//! names are 16-byte NUL-padded strings (`W3D_NAME_LEN`).

use std::collections::BTreeMap;

#[derive(Debug, Clone, thiserror::Error, PartialEq, Eq)]
pub enum W3dError {
    #[error("chunk {chunk:#x} at {offset}: {why}")]
    Bad { chunk: u32, offset: usize, why: String },
}

type R<T> = Result<T, W3dError>;

pub const NAME_LEN: usize = 16;

// Chunk ids (`w3d_file.h`, `w3d_obsolete.h`).
pub mod id {
    pub const MESH: u32 = 0x0000;
    pub const VERTICES: u32 = 0x0002;
    pub const VERTEX_NORMALS: u32 = 0x0003;
    pub const MESH_USER_TEXT: u32 = 0x000C;
    pub const VERTEX_INFLUENCES: u32 = 0x000E;
    pub const MESH_HEADER3: u32 = 0x001F;
    pub const TRIANGLES: u32 = 0x0020;
    pub const VERTEX_SHADE_INDICES: u32 = 0x0022;
    pub const PRELIT_UNLIT: u32 = 0x0023;
    pub const PRELIT_VERTEX: u32 = 0x0024;
    pub const PRELIT_LIGHTMAP_MULTI_PASS: u32 = 0x0025;
    pub const PRELIT_LIGHTMAP_MULTI_TEXTURE: u32 = 0x0026;
    pub const MATERIAL_INFO: u32 = 0x0028;
    pub const SHADERS: u32 = 0x0029;
    pub const VERTEX_MATERIALS: u32 = 0x002A;
    pub const VERTEX_MATERIAL: u32 = 0x002B;
    pub const VERTEX_MATERIAL_NAME: u32 = 0x002C;
    pub const VERTEX_MATERIAL_INFO: u32 = 0x002D;
    pub const VERTEX_MAPPER_ARGS0: u32 = 0x002E;
    pub const VERTEX_MAPPER_ARGS1: u32 = 0x002F;
    pub const TEXTURES: u32 = 0x0030;
    pub const TEXTURE: u32 = 0x0031;
    pub const TEXTURE_NAME: u32 = 0x0032;
    pub const TEXTURE_INFO: u32 = 0x0033;
    pub const MATERIAL_PASS: u32 = 0x0038;
    pub const VERTEX_MATERIAL_IDS: u32 = 0x0039;
    pub const SHADER_IDS: u32 = 0x003A;
    pub const DCG: u32 = 0x003B;
    pub const DIG: u32 = 0x003C;
    pub const SCG: u32 = 0x003E;
    pub const TEXTURE_STAGE: u32 = 0x0048;
    pub const TEXTURE_IDS: u32 = 0x0049;
    pub const STAGE_TEXCOORDS: u32 = 0x004A;
    pub const PER_FACE_TEXCOORD_IDS: u32 = 0x004B;
    pub const DEFORM: u32 = 0x0058;
    pub const PS2_SHADERS: u32 = 0x0080;
    pub const AABTREE: u32 = 0x0090;
    pub const HIERARCHY: u32 = 0x0100;
    pub const HIERARCHY_HEADER: u32 = 0x0101;
    pub const PIVOTS: u32 = 0x0102;
    pub const PIVOT_FIXUPS: u32 = 0x0103;
    pub const ANIMATION: u32 = 0x0200;
    pub const ANIMATION_HEADER: u32 = 0x0201;
    pub const ANIMATION_CHANNEL: u32 = 0x0202;
    pub const BIT_CHANNEL: u32 = 0x0203;
    pub const COMPRESSED_ANIMATION: u32 = 0x0280;
    pub const COMPRESSED_ANIMATION_HEADER: u32 = 0x0281;
    pub const COMPRESSED_ANIMATION_CHANNEL: u32 = 0x0282;
    pub const COMPRESSED_BIT_CHANNEL: u32 = 0x0283;
    pub const MORPH_ANIMATION: u32 = 0x02C0;
    pub const HMODEL: u32 = 0x0300;
    pub const LODMODEL: u32 = 0x0400;
    pub const COLLECTION: u32 = 0x0420;
    pub const POINTS: u32 = 0x0440;
    pub const LIGHT: u32 = 0x0460;
    pub const EMITTER: u32 = 0x0500;
    pub const AGGREGATE: u32 = 0x0600;
    pub const HLOD: u32 = 0x0700;
    pub const HLOD_HEADER: u32 = 0x0701;
    pub const HLOD_LOD_ARRAY: u32 = 0x0702;
    pub const HLOD_SUB_OBJECT_ARRAY_HEADER: u32 = 0x0703;
    pub const HLOD_SUB_OBJECT: u32 = 0x0704;
    pub const HLOD_AGGREGATE_ARRAY: u32 = 0x0705;
    pub const HLOD_PROXY_ARRAY: u32 = 0x0706;
    pub const BOX: u32 = 0x0740;
    pub const SPHERE: u32 = 0x0741;
    pub const RING: u32 = 0x0742;
    pub const NULL_OBJECT: u32 = 0x0750;
    pub const LIGHTSCAPE: u32 = 0x0800;
    pub const DAZZLE: u32 = 0x0900;
    pub const DAZZLE_NAME: u32 = 0x0901;
    pub const DAZZLE_TYPENAME: u32 = 0x0902;
    pub const SOUNDROBJ: u32 = 0x0A00;
    pub const SHDMESH: u32 = 0x0B00;
}

/// Mesh attribute flags (`W3D_MESH_FLAG_*`).
pub mod mesh_flags {
    pub const HIDDEN: u32 = 0x0000_1000;
    pub const TWO_SIDED: u32 = 0x0000_2000;
    pub const CAST_SHADOW: u32 = 0x0000_8000;
    pub const GEOMETRY_TYPE_MASK: u32 = 0x00FF_0000;
    pub const GEOMETRY_TYPE_CAMERA_ALIGNED: u32 = 0x0001_0000;
    pub const GEOMETRY_TYPE_SKIN: u32 = 0x0002_0000;
    pub const GEOMETRY_TYPE_CAMERA_ORIENTED: u32 = 0x0006_0000;
    pub const PRELIT_UNLIT: u32 = 0x0100_0000;
    pub const PRELIT_VERTEX: u32 = 0x0200_0000;
    pub const PRELIT_LIGHTMAP_MULTI_PASS: u32 = 0x0400_0000;
    pub const PRELIT_LIGHTMAP_MULTI_TEXTURE: u32 = 0x0800_0000;
}

// ---------------------------------------------------------------------------
// Chunk walking

#[derive(Debug, Clone, Copy)]
pub struct Chunk<'a> {
    pub id: u32,
    pub has_children: bool,
    pub data: &'a [u8],
    /// Offset of the chunk's data in the file, for error messages.
    pub offset: usize,
}

/// Iterates the chunks in `data` (`base` is its offset in the file).
pub fn chunks(data: &[u8], base: usize) -> impl Iterator<Item = R<Chunk<'_>>> {
    let mut pos = 0usize;
    std::iter::from_fn(move || {
        if pos >= data.len() {
            return None;
        }
        if data.len() - pos < 8 {
            pos = data.len();
            return Some(Err(W3dError::Bad { chunk: 0, offset: base + pos, why: "truncated chunk header".into() }));
        }
        let id = u32::from_le_bytes(data[pos..pos + 4].try_into().unwrap());
        let raw = u32::from_le_bytes(data[pos + 4..pos + 8].try_into().unwrap());
        let size = (raw & 0x7FFF_FFFF) as usize;
        let start = pos + 8;
        let Some(end) = start.checked_add(size).filter(|&e| e <= data.len()) else {
            pos = data.len();
            return Some(Err(W3dError::Bad {
                chunk: id,
                offset: base + start,
                why: "chunk runs past its parent".into(),
            }));
        };
        pos = end;
        Some(Ok(Chunk { id, has_children: raw & 0x8000_0000 != 0, data: &data[start..end], offset: base + start }))
    })
}

impl<'a> Chunk<'a> {
    fn bad<T>(&self, why: impl Into<String>) -> R<T> {
        Err(W3dError::Bad { chunk: self.id, offset: self.offset, why: why.into() })
    }
    fn children(&self) -> impl Iterator<Item = R<Chunk<'a>>> {
        chunks(self.data, self.offset)
    }
    fn reader(&self) -> Rd<'a> {
        Rd { b: self.data, pos: 0, chunk: self.id, offset: self.offset }
    }
    /// A chunk that holds a NUL-terminated string.
    fn string(&self) -> String {
        cstr(self.data)
    }
}

/// Little-endian reader over one chunk's data.
struct Rd<'a> {
    b: &'a [u8],
    pos: usize,
    chunk: u32,
    offset: usize,
}

impl Rd<'_> {
    fn take(&mut self, n: usize) -> R<&[u8]> {
        if self.b.len() - self.pos < n {
            return Err(W3dError::Bad {
                chunk: self.chunk,
                offset: self.offset + self.pos,
                why: "truncated record".into(),
            });
        }
        let s = &self.b[self.pos..self.pos + n];
        self.pos += n;
        Ok(s)
    }
    fn u8(&mut self) -> R<u8> {
        Ok(self.take(1)?[0])
    }
    fn u16(&mut self) -> R<u16> {
        Ok(u16::from_le_bytes(self.take(2)?.try_into().unwrap()))
    }
    fn u32(&mut self) -> R<u32> {
        Ok(u32::from_le_bytes(self.take(4)?.try_into().unwrap()))
    }
    fn i32(&mut self) -> R<i32> {
        Ok(self.u32()? as i32)
    }
    fn f32(&mut self) -> R<f32> {
        Ok(f32::from_le_bytes(self.take(4)?.try_into().unwrap()))
    }
    fn vec3(&mut self) -> R<[f32; 3]> {
        Ok([self.f32()?, self.f32()?, self.f32()?])
    }
    fn quat(&mut self) -> R<[f32; 4]> {
        Ok([self.f32()?, self.f32()?, self.f32()?, self.f32()?])
    }
    fn name(&mut self, len: usize) -> R<String> {
        Ok(cstr(self.take(len)?))
    }
    fn skip(&mut self, n: usize) -> R<()> {
        self.take(n).map(|_| ())
    }
    fn remaining(&self) -> usize {
        self.b.len() - self.pos
    }
}

/// A NUL-terminated (or padded) string, decoded byte-for-byte.
fn cstr(b: &[u8]) -> String {
    let end = b.iter().position(|&c| c == 0).unwrap_or(b.len());
    b[..end].iter().map(|&c| c as char).collect()
}

// ---------------------------------------------------------------------------
// Data model

/// Everything in one `.w3d` file.
#[derive(Debug, Default, Clone)]
pub struct W3dFile {
    pub meshes: Vec<Mesh>,
    pub hierarchies: Vec<Hierarchy>,
    pub animations: Vec<Animation>,
    pub hlods: Vec<Hlod>,
    pub boxes: Vec<BoxObject>,
    /// Names of other top-level render objects (emitters, aggregates, dazzles, ...).
    pub others: Vec<(u32, String)>,
    /// Chunk ids this reader skipped, with counts (for coverage).
    pub skipped: BTreeMap<u32, u32>,
    /// Objects that could not be read and were left out. The original's chunk reader
    /// does not check that a child fits inside its parent, so for malformed files it reads
    /// garbage and usually ends up with a partial or missing object too.
    pub errors: Vec<W3dError>,
}

#[derive(Debug, Clone, Copy, Default, PartialEq)]
pub struct Rgb {
    pub r: u8,
    pub g: u8,
    pub b: u8,
}

#[derive(Debug, Clone, Default)]
pub struct VertexMaterialInfo {
    pub attributes: u32,
    pub ambient: Rgb,
    pub diffuse: Rgb,
    pub specular: Rgb,
    pub emissive: Rgb,
    pub shininess: f32,
    pub opacity: f32,
    pub translucency: f32,
}

#[derive(Debug, Clone, Default)]
pub struct VertexMaterial {
    pub name: String,
    pub info: VertexMaterialInfo,
    /// Texture-mapper arguments (INI-like text) for stages 0 and 1.
    pub mapper_args: [String; 2],
}

/// Fixed-function render state (`W3dShaderStruct`, `shader.h`).
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Hash)]
pub struct Shader {
    pub depth_compare: u8,
    pub depth_mask: u8,
    pub dest_blend: u8,
    pub pri_gradient: u8,
    pub sec_gradient: u8,
    pub src_blend: u8,
    pub texturing: u8,
    pub detail_color_func: u8,
    pub detail_alpha_func: u8,
    pub alpha_test: u8,
    pub post_detail_color_func: u8,
    pub post_detail_alpha_func: u8,
}

#[derive(Debug, Clone, Copy, Default)]
pub struct TextureInfo {
    pub attributes: u16,
    pub anim_type: u16,
    pub frame_count: u32,
    pub frame_rate: f32,
}

#[derive(Debug, Clone, Default)]
pub struct TextureRef {
    pub name: String,
    pub info: Option<TextureInfo>,
}

/// One id for the whole mesh, or one per vertex / per triangle.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Ids {
    Single(u32),
    Each(Vec<u32>),
}

impl Ids {
    fn read(c: &Chunk) -> R<Ids> {
        let mut r = c.reader();
        let n = r.remaining() / 4;
        let v: Vec<u32> = (0..n).map(|_| r.u32()).collect::<R<_>>()?;
        Ok(if v.len() == 1 { Ids::Single(v[0]) } else { Ids::Each(v) })
    }
    /// Id for element `i` (`0xffffffff` in per-element arrays means "none").
    pub fn get(&self, i: usize) -> Option<u32> {
        let v = match self {
            Ids::Single(v) => *v,
            Ids::Each(v) => *v.get(i)?,
        };
        (v != u32::MAX).then_some(v)
    }
}

#[derive(Debug, Clone, Default)]
pub struct TextureStage {
    pub texture_ids: Option<Ids>,
    pub texcoords: Option<Vec<[f32; 2]>>,
    pub per_face_texcoord_ids: Option<Vec<[u32; 3]>>,
}

#[derive(Debug, Clone, Default)]
pub struct MaterialPass {
    pub vertex_material_ids: Option<Ids>,
    pub shader_ids: Option<Ids>,
    /// Per-vertex diffuse colour (RGBA).
    pub dcg: Option<Vec<[u8; 4]>>,
    /// Per-vertex diffuse illumination (RGBA).
    pub dig: Option<Vec<[u8; 4]>>,
    /// Per-vertex specular colour (RGBA).
    pub scg: Option<Vec<[u8; 4]>>,
    pub stages: Vec<TextureStage>,
}

/// Materials, shaders, textures and passes. A mesh may carry several of these, one per
/// prelighting mode.
#[derive(Debug, Clone, Default)]
pub struct MaterialSet {
    pub vertex_materials: Vec<VertexMaterial>,
    pub shaders: Vec<Shader>,
    pub textures: Vec<TextureRef>,
    pub passes: Vec<MaterialPass>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum Prelit {
    /// Materials stored directly in the mesh (no prelighting).
    None,
    Unlit,
    Vertex,
    LightmapMultiPass,
    LightmapMultiTexture,
}

#[derive(Debug, Clone)]
pub struct Triangle {
    pub indices: [u32; 3],
    pub attributes: u32,
    pub normal: [f32; 3],
    pub dist: f32,
}

#[derive(Debug, Clone, Default)]
pub struct Mesh {
    /// Mesh name and container (HLOD/model) name; render objects are named
    /// `CONTAINER.MESH`.
    pub name: String,
    pub container: String,
    pub version: u32,
    pub attributes: u32,
    pub sort_level: i32,
    pub bounds_min: [f32; 3],
    pub bounds_max: [f32; 3],
    pub sphere_center: [f32; 3],
    pub sphere_radius: f32,
    pub vertices: Vec<[f32; 3]>,
    pub normals: Vec<[f32; 3]>,
    /// Bone index per vertex (skins).
    pub influences: Vec<u16>,
    pub shade_indices: Vec<u32>,
    pub triangles: Vec<Triangle>,
    pub user_text: String,
    /// Material sets by prelighting mode.
    pub materials: BTreeMap<Prelit, MaterialSet>,
}

impl Mesh {
    pub fn full_name(&self) -> String {
        if self.container.is_empty() { self.name.clone() } else { format!("{}.{}", self.container, self.name) }
    }

    pub fn is_skin(&self) -> bool {
        self.attributes & mesh_flags::GEOMETRY_TYPE_MASK == mesh_flags::GEOMETRY_TYPE_SKIN
    }

    /// The material set the game uses. Generals sets
    /// `PRELIT_MODE_LIGHTMAP_MULTI_PASS` (`W3DDisplay.cpp`), and `Load_W3D` falls back
    /// from there to vertex, then unlit (`meshmdlio.cpp`).
    pub fn active_materials(&self) -> Option<&MaterialSet> {
        let f = self.attributes;
        let pick = if f & mesh_flags::PRELIT_LIGHTMAP_MULTI_PASS != 0 {
            Prelit::LightmapMultiPass
        } else if f & mesh_flags::PRELIT_VERTEX != 0 {
            Prelit::Vertex
        } else if f & mesh_flags::PRELIT_UNLIT != 0 {
            Prelit::Unlit
        } else {
            Prelit::None
        };
        self.materials.get(&pick)
    }
}

#[derive(Debug, Clone)]
pub struct Pivot {
    pub name: String,
    /// `None` for the root.
    pub parent: Option<u32>,
    pub translation: [f32; 3],
    pub euler: [f32; 3],
    /// Quaternion (x, y, z, w).
    pub rotation: [f32; 4],
}

#[derive(Debug, Clone)]
pub struct Hierarchy {
    pub name: String,
    pub version: u32,
    pub center: [f32; 3],
    pub pivots: Vec<Pivot>,
}

/// Which component an animation channel drives (`ANIM_CHANNEL_*`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ChannelKind {
    X,
    Y,
    Z,
    /// Euler channels exist in the format but the game does not use them.
    XR,
    YR,
    ZR,
    Q,
}

impl ChannelKind {
    /// Maps both raw (0..=6) and compressed (time-coded 7..=10, adaptive 11..=14) ids.
    fn from_flags(f: u32) -> Option<Self> {
        Some(match f {
            0 | 7 | 11 => Self::X,
            1 | 8 | 12 => Self::Y,
            2 | 9 | 13 => Self::Z,
            3 => Self::XR,
            4 => Self::YR,
            5 => Self::ZR,
            6 | 10 | 14 => Self::Q,
            _ => return None,
        })
    }
}

/// Decoded channel data.
#[derive(Debug, Clone)]
pub enum ChannelData {
    /// Raw: one vector per frame from `first` to `last`; identity outside.
    Raw { first: u32, last: u32, values: Vec<f32> },
    /// Time-coded keys: (frame, step-to-this-key flag, vector). Linear between keys
    /// (slerp for quaternions) unless the next key is a step.
    TimeCoded { keys: Vec<(u32, bool, Vec<f32>)> },
    /// Adaptive delta, decoded to one vector per frame.
    Dense { values: Vec<f32> },
}

#[derive(Debug, Clone)]
pub struct Channel {
    pub pivot: u16,
    pub kind: ChannelKind,
    pub vector_len: u8,
    pub data: ChannelData,
}

/// Visibility per frame.
#[derive(Debug, Clone)]
pub enum BitChannel {
    Raw { pivot: u16, first: u32, last: u32, default: bool, bits: Vec<u8> },
    TimeCoded { pivot: u16, default: bool, keys: Vec<(u32, bool)> },
}

#[derive(Debug, Clone)]
pub struct Animation {
    pub name: String,
    pub hierarchy: String,
    pub num_frames: u32,
    pub frame_rate: u32,
    pub compressed: bool,
    pub channels: Vec<Channel>,
    pub bit_channels: Vec<BitChannel>,
}

#[derive(Debug, Clone)]
pub struct HlodSubObject {
    pub bone: u32,
    /// Render object name (`CONTAINER.MESH` or another model).
    pub name: String,
}

#[derive(Debug, Clone)]
pub struct HlodLod {
    pub max_screen_size: f32,
    pub objects: Vec<HlodSubObject>,
}

#[derive(Debug, Clone)]
pub struct Hlod {
    pub name: String,
    pub hierarchy: String,
    /// Lowest detail first, as stored.
    pub lods: Vec<HlodLod>,
    /// Always drawn, regardless of LOD.
    pub aggregates: Vec<HlodSubObject>,
    /// Named bone placeholders for game use.
    pub proxies: Vec<HlodSubObject>,
}

#[derive(Debug, Clone)]
pub struct BoxObject {
    pub name: String,
    pub attributes: u32,
    pub color: Rgb,
    pub center: [f32; 3],
    pub extent: [f32; 3],
}

// ---------------------------------------------------------------------------
// Parsing

/// Parses a whole `.w3d` file.
pub fn parse(data: &[u8]) -> R<W3dFile> {
    let mut f = W3dFile::default();
    for c in chunks(data, 0) {
        // Trailing bytes that do not form a chunk end the file, as reading would.
        let c = match c {
            Ok(c) => c,
            Err(e) => {
                f.errors.push(e);
                break;
            }
        };
        match c.id {
            id::MESH => match read_mesh(&c, &mut f.skipped) {
                Ok(m) => f.meshes.push(m),
                Err(e) => f.errors.push(e),
            },
            id::HIERARCHY => keep(read_hierarchy(&c), &mut f.hierarchies, &mut f.errors),
            id::ANIMATION => keep(read_raw_animation(&c), &mut f.animations, &mut f.errors),
            id::COMPRESSED_ANIMATION => keep(read_compressed_animation(&c), &mut f.animations, &mut f.errors),
            id::HLOD => keep(read_hlod(&c), &mut f.hlods, &mut f.errors),
            id::BOX => keep(read_box(&c), &mut f.boxes, &mut f.errors),
            id::DAZZLE => {
                let name = c.children().filter_map(Result::ok).find(|k| k.id == id::DAZZLE_NAME);
                f.others.push((c.id, name.map(|k| k.string()).unwrap_or_default()));
            }
            id::AGGREGATE
            | id::EMITTER
            | id::NULL_OBJECT
            | id::SPHERE
            | id::RING
            | id::LIGHT
            | id::HMODEL
            | id::LODMODEL
            | id::COLLECTION
            | id::MORPH_ANIMATION
            | id::SOUNDROBJ
            | id::SHDMESH => {
                f.others.push((c.id, first_name(&c)));
            }
            other => *f.skipped.entry(other).or_default() += 1,
        }
    }
    Ok(f)
}

fn keep<T>(r: R<T>, into: &mut Vec<T>, errors: &mut Vec<W3dError>) {
    match r {
        Ok(v) => into.push(v),
        Err(e) => errors.push(e),
    }
}

/// Best-effort name of a render object we do not fully parse: the first header's
/// name field after a version number.
fn first_name(c: &Chunk) -> String {
    let header = if c.has_children { c.children().filter_map(Result::ok).next().map(|k| k.data) } else { Some(c.data) };
    header.filter(|h| h.len() >= 4 + NAME_LEN).map(|h| cstr(&h[4..4 + NAME_LEN])).unwrap_or_default()
}

fn read_mesh(c: &Chunk, skipped: &mut BTreeMap<u32, u32>) -> R<Mesh> {
    let mut m = Mesh::default();
    let mut direct = MaterialSet::default();
    for k in c.children() {
        let k = k?;
        match k.id {
            id::MESH_HEADER3 => {
                let mut r = k.reader();
                m.version = r.u32()?;
                m.attributes = r.u32()?;
                m.name = r.name(NAME_LEN)?;
                m.container = r.name(NAME_LEN)?;
                let _tris = r.u32()?;
                let _verts = r.u32()?;
                let _mats = r.u32()?;
                let _damage = r.u32()?;
                m.sort_level = r.i32()?;
                let _prelit_version = r.u32()?;
                let _future = r.u32()?;
                let _vchan = r.u32()?;
                let _fchan = r.u32()?;
                m.bounds_min = r.vec3()?;
                m.bounds_max = r.vec3()?;
                m.sphere_center = r.vec3()?;
                m.sphere_radius = r.f32()?;
            }
            id::VERTICES => m.vertices = vec3s(&k)?,
            id::VERTEX_NORMALS => m.normals = vec3s(&k)?,
            id::VERTEX_INFLUENCES => {
                let mut r = k.reader();
                while r.remaining() >= 8 {
                    m.influences.push(r.u16()?);
                    r.skip(6)?;
                }
            }
            id::VERTEX_SHADE_INDICES => {
                let mut r = k.reader();
                while r.remaining() >= 4 {
                    m.shade_indices.push(r.u32()?);
                }
            }
            id::TRIANGLES => {
                let mut r = k.reader();
                while r.remaining() >= 32 {
                    let indices = [r.u32()?, r.u32()?, r.u32()?];
                    let attributes = r.u32()?;
                    let normal = r.vec3()?;
                    let dist = r.f32()?;
                    m.triangles.push(Triangle { indices, attributes, normal, dist });
                }
            }
            id::MESH_USER_TEXT => m.user_text = k.string(),
            id::MATERIAL_INFO | id::SHADERS | id::VERTEX_MATERIALS | id::TEXTURES | id::MATERIAL_PASS => {
                read_material_chunk(&k, &mut direct)?;
            }
            id::PRELIT_UNLIT
            | id::PRELIT_VERTEX
            | id::PRELIT_LIGHTMAP_MULTI_PASS
            | id::PRELIT_LIGHTMAP_MULTI_TEXTURE => {
                let kind = match k.id {
                    id::PRELIT_UNLIT => Prelit::Unlit,
                    id::PRELIT_VERTEX => Prelit::Vertex,
                    id::PRELIT_LIGHTMAP_MULTI_PASS => Prelit::LightmapMultiPass,
                    _ => Prelit::LightmapMultiTexture,
                };
                let mut set = MaterialSet::default();
                for p in k.children() {
                    read_material_chunk(&p?, &mut set)?;
                }
                m.materials.insert(kind, set);
            }
            // Collision trees and deformation data are not needed for drawing.
            id::AABTREE | id::DEFORM | id::PS2_SHADERS => {}
            other => *skipped.entry(other).or_default() += 1,
        }
    }
    if !direct.vertex_materials.is_empty() || !direct.passes.is_empty() || !direct.shaders.is_empty() {
        m.materials.insert(Prelit::None, direct);
    }
    Ok(m)
}

fn vec3s(c: &Chunk) -> R<Vec<[f32; 3]>> {
    let mut r = c.reader();
    let mut v = Vec::with_capacity(r.remaining() / 12);
    while r.remaining() >= 12 {
        v.push(r.vec3()?);
    }
    Ok(v)
}

fn rgbas(c: &Chunk) -> R<Vec<[u8; 4]>> {
    Ok(c.data.as_chunks::<4>().0.to_vec())
}

fn read_material_chunk(k: &Chunk, set: &mut MaterialSet) -> R<()> {
    match k.id {
        id::MATERIAL_INFO => {}
        id::SHADERS => {
            for s in k.data.as_chunks::<16>().0 {
                set.shaders.push(Shader {
                    depth_compare: s[0],
                    depth_mask: s[1],
                    dest_blend: s[3],
                    pri_gradient: s[5],
                    sec_gradient: s[6],
                    src_blend: s[7],
                    texturing: s[8],
                    detail_color_func: s[9],
                    detail_alpha_func: s[10],
                    alpha_test: s[12],
                    post_detail_color_func: s[13],
                    post_detail_alpha_func: s[14],
                });
            }
        }
        id::VERTEX_MATERIALS => {
            for vm in k.children() {
                let vm = vm?;
                if vm.id != id::VERTEX_MATERIAL {
                    continue;
                }
                let mut mat = VertexMaterial::default();
                for p in vm.children() {
                    let p = p?;
                    match p.id {
                        id::VERTEX_MATERIAL_NAME => mat.name = p.string(),
                        id::VERTEX_MAPPER_ARGS0 => mat.mapper_args[0] = p.string(),
                        id::VERTEX_MAPPER_ARGS1 => mat.mapper_args[1] = p.string(),
                        id::VERTEX_MATERIAL_INFO => {
                            let mut r = p.reader();
                            let rgb = |r: &mut Rd| -> R<Rgb> {
                                let c = Rgb { r: r.u8()?, g: r.u8()?, b: r.u8()? };
                                r.skip(1)?;
                                Ok(c)
                            };
                            mat.info.attributes = r.u32()?;
                            mat.info.ambient = rgb(&mut r)?;
                            mat.info.diffuse = rgb(&mut r)?;
                            mat.info.specular = rgb(&mut r)?;
                            mat.info.emissive = rgb(&mut r)?;
                            mat.info.shininess = r.f32()?;
                            mat.info.opacity = r.f32()?;
                            mat.info.translucency = r.f32()?;
                        }
                        _ => {}
                    }
                }
                set.vertex_materials.push(mat);
            }
        }
        id::TEXTURES => {
            for t in k.children() {
                let t = t?;
                if t.id != id::TEXTURE {
                    continue;
                }
                let mut tex = TextureRef::default();
                for p in t.children() {
                    let p = p?;
                    match p.id {
                        id::TEXTURE_NAME => tex.name = p.string(),
                        id::TEXTURE_INFO => {
                            let mut r = p.reader();
                            tex.info = Some(TextureInfo {
                                attributes: r.u16()?,
                                anim_type: r.u16()?,
                                frame_count: r.u32()?,
                                frame_rate: r.f32()?,
                            });
                        }
                        _ => {}
                    }
                }
                set.textures.push(tex);
            }
        }
        id::MATERIAL_PASS => {
            let mut pass = MaterialPass::default();
            for p in k.children() {
                let p = p?;
                match p.id {
                    id::VERTEX_MATERIAL_IDS => pass.vertex_material_ids = Some(Ids::read(&p)?),
                    id::SHADER_IDS => pass.shader_ids = Some(Ids::read(&p)?),
                    id::DCG => pass.dcg = Some(rgbas(&p)?),
                    id::DIG => pass.dig = Some(rgbas(&p)?),
                    id::SCG => pass.scg = Some(rgbas(&p)?),
                    id::TEXTURE_STAGE => {
                        let mut stage = TextureStage::default();
                        for s in p.children() {
                            let s = s?;
                            match s.id {
                                id::TEXTURE_IDS => stage.texture_ids = Some(Ids::read(&s)?),
                                // W3D_CHUNK_TEXCOORDS (0x05, obsolete) is accepted too.
                                id::STAGE_TEXCOORDS | 0x0005 => {
                                    let mut r = s.reader();
                                    let mut uv = Vec::with_capacity(r.remaining() / 8);
                                    while r.remaining() >= 8 {
                                        uv.push([r.f32()?, r.f32()?]);
                                    }
                                    stage.texcoords = Some(uv);
                                }
                                id::PER_FACE_TEXCOORD_IDS => {
                                    let mut r = s.reader();
                                    let mut v = Vec::new();
                                    while r.remaining() >= 12 {
                                        v.push([r.u32()?, r.u32()?, r.u32()?]);
                                    }
                                    stage.per_face_texcoord_ids = Some(v);
                                }
                                _ => {}
                            }
                        }
                        pass.stages.push(stage);
                    }
                    _ => {}
                }
            }
            set.passes.push(pass);
        }
        _ => {}
    }
    Ok(())
}

fn read_hierarchy(c: &Chunk) -> R<Hierarchy> {
    let mut h = Hierarchy { name: String::new(), version: 0, center: [0.0; 3], pivots: Vec::new() };
    for k in c.children() {
        let k = k?;
        match k.id {
            id::HIERARCHY_HEADER => {
                let mut r = k.reader();
                h.version = r.u32()?;
                h.name = r.name(NAME_LEN)?;
                let _n = r.u32()?;
                h.center = r.vec3()?;
            }
            id::PIVOTS => {
                let mut r = k.reader();
                while r.remaining() >= 60 {
                    let name = r.name(NAME_LEN)?;
                    let parent = r.u32()?;
                    let translation = r.vec3()?;
                    let euler = r.vec3()?;
                    let rotation = r.quat()?;
                    h.pivots.push(Pivot {
                        name,
                        parent: (parent != u32::MAX).then_some(parent),
                        translation,
                        euler,
                        rotation,
                    });
                }
            }
            _ => {}
        }
    }
    Ok(h)
}

fn read_raw_animation(c: &Chunk) -> R<Animation> {
    let mut a = Animation {
        name: String::new(),
        hierarchy: String::new(),
        num_frames: 0,
        frame_rate: 0,
        compressed: false,
        channels: Vec::new(),
        bit_channels: Vec::new(),
    };
    for k in c.children() {
        let k = k?;
        match k.id {
            id::ANIMATION_HEADER => {
                let mut r = k.reader();
                let _v = r.u32()?;
                a.name = r.name(NAME_LEN)?;
                a.hierarchy = r.name(NAME_LEN)?;
                a.num_frames = r.u32()?;
                a.frame_rate = r.u32()?;
            }
            id::ANIMATION_CHANNEL => {
                let mut r = k.reader();
                let first = u32::from(r.u16()?);
                let last = u32::from(r.u16()?);
                let vector_len = r.u16()? as u8;
                let flags = u32::from(r.u16()?);
                let pivot = r.u16()?;
                let _pad = r.u16()?;
                let n = (last.saturating_sub(first) + 1) as usize * vector_len as usize;
                let values: Vec<f32> = (0..n).map(|_| r.f32()).collect::<R<_>>()?;
                // Unknown channel types are ignored by HRawAnimClass::add_channel.
                let Some(kind) = ChannelKind::from_flags(flags) else { continue };
                a.channels.push(Channel { pivot, kind, vector_len, data: ChannelData::Raw { first, last, values } });
            }
            id::BIT_CHANNEL => {
                let mut r = k.reader();
                let first = u32::from(r.u16()?);
                let last = u32::from(r.u16()?);
                let _flags = r.u16()?;
                let pivot = r.u16()?;
                let default = r.u8()? != 0;
                let n = ((last.saturating_sub(first) + 1) as usize).div_ceil(8);
                let bits = r.take(n.min(r.remaining()))?.to_vec();
                a.bit_channels.push(BitChannel::Raw { pivot, first, last, default, bits });
            }
            _ => {}
        }
    }
    Ok(a)
}

/// `ANIM_FLAVOR_*`.
const FLAVOR_TIMECODED: u16 = 0;
const FLAVOR_ADAPTIVE_DELTA: u16 = 1;
/// `W3D_TIMECODED_BINARY_MOVEMENT_FLAG` / `W3D_TIMECODED_BIT_MASK`.
const TIMECODE_FLAG: u32 = 0x8000_0000;

fn read_compressed_animation(c: &Chunk) -> R<Animation> {
    let mut a = Animation {
        name: String::new(),
        hierarchy: String::new(),
        num_frames: 0,
        frame_rate: 0,
        compressed: true,
        channels: Vec::new(),
        bit_channels: Vec::new(),
    };
    let mut flavor = FLAVOR_TIMECODED;
    for k in c.children() {
        let k = k?;
        match k.id {
            id::COMPRESSED_ANIMATION_HEADER => {
                let mut r = k.reader();
                let _v = r.u32()?;
                a.name = r.name(NAME_LEN)?;
                a.hierarchy = r.name(NAME_LEN)?;
                a.num_frames = r.u32()?;
                a.frame_rate = u32::from(r.u16()?);
                flavor = r.u16()?;
            }
            id::COMPRESSED_ANIMATION_CHANNEL => {
                a.channels.push(match flavor {
                    FLAVOR_TIMECODED => read_timecoded_channel(&k)?,
                    FLAVOR_ADAPTIVE_DELTA => read_adaptive_delta_channel(&k)?,
                    other => return k.bad(format!("unknown animation flavour {other}")),
                });
            }
            id::COMPRESSED_BIT_CHANNEL => {
                let mut r = k.reader();
                let n = r.u32()?;
                let pivot = r.u16()?;
                let _flags = r.u8()?;
                let default = r.u8()? != 0;
                let keys =
                    (0..n).map(|_| r.u32().map(|v| (v & !TIMECODE_FLAG, v & TIMECODE_FLAG != 0))).collect::<R<_>>()?;
                a.bit_channels.push(BitChannel::TimeCoded { pivot, default, keys });
            }
            _ => {}
        }
    }
    Ok(a)
}

fn read_timecoded_channel(k: &Chunk) -> R<Channel> {
    let mut r = k.reader();
    let n = r.u32()?;
    let pivot = r.u16()?;
    let vector_len = r.u8()?;
    let flags = u32::from(r.u8()?);
    let Some(kind) = ChannelKind::from_flags(flags) else { return k.bad(format!("unknown channel type {flags}")) };
    let mut keys = Vec::with_capacity(n as usize);
    for _ in 0..n {
        let t = r.u32()?;
        let v: Vec<f32> = (0..vector_len).map(|_| r.f32()).collect::<R<_>>()?;
        keys.push((t & !TIMECODE_FLAG, t & TIMECODE_FLAG != 0, v));
    }
    Ok(Channel { pivot, kind, vector_len, data: ChannelData::TimeCoded { keys } })
}

/// `FILTER_TABLE_SIZE`/`FILTER_TABLE_GEN_START` (`motchan.cpp`): sixteen fixed scales,
/// then `1 - sin(90° * i / 240)` for the remaining 240 entries.
fn filter_table() -> [f32; 256] {
    let mut t = [0f32; 256];
    let fixed = [
        0.000_000_01,
        0.000_000_1,
        0.000_001,
        0.000_01,
        0.0001,
        0.001,
        0.01,
        0.1,
        1.0,
        10.0,
        100.0,
        1000.0,
        10000.0,
        100_000.0,
        1_000_000.0,
        10_000_000.0,
    ];
    t[..16].copy_from_slice(&fixed);
    for i in 0..240 {
        let ratio = i as f32 / 240.0;
        t[16 + i] = 1.0 - (90.0f32 * ratio).to_radians().sin();
    }
    t
}

/// Decodes an adaptive-delta channel (`AdaptiveDeltaMotionChannelClass::decompress`):
/// the first vector is stored as floats; each later frame adds a signed 4-bit delta
/// times `filter_table[packet header] * scale`. Packets hold 16 frames per component
/// and are interleaved per component (9 bytes each).
fn read_adaptive_delta_channel(k: &Chunk) -> R<Channel> {
    let mut r = k.reader();
    let num_frames = r.u32()?;
    let pivot = r.u16()?;
    let vector_len = r.u8()?;
    let flags = u32::from(r.u8()?);
    let scale = r.f32()?;
    let Some(kind) = ChannelKind::from_flags(flags) else { return k.bad(format!("unknown channel type {flags}")) };
    let data = &k.data[12..];
    let vl = vector_len as usize;
    let table = filter_table();
    let base: Vec<f32> = (0..vl)
        .map(|i| data.get(i * 4..i * 4 + 4).map(|b| f32::from_le_bytes(b.try_into().unwrap())))
        .collect::<Option<_>>()
        .ok_or_else(|| W3dError::Bad { chunk: k.id, offset: k.offset, why: "truncated delta header".into() })?;
    let packets = &data[vl * 4..];
    let mut values = vec![0f32; num_frames as usize * vl];
    for vi in 0..vl {
        let mut value = base[vi];
        if num_frames > 0 {
            values[vi] = value;
        }
        for frame in 1..num_frames as usize {
            let packet = (frame - 1) / 16;
            let within = (frame - 1) % 16;
            let at = packet * 9 * vl + vi * 9;
            let Some(p) = packets.get(at..at + 9) else {
                return k.bad("delta stream shorter than its frame count");
            };
            let filter = table[p[0] as usize] * scale;
            let byte = p[1 + within / 2];
            let mut nyb = if within & 1 == 1 { byte >> 4 } else { byte } as i32 & 0xF;
            if nyb & 0x8 != 0 {
                nyb |= !0xF;
            }
            value += nyb as f32 * filter;
            values[frame * vl + vi] = value;
        }
    }
    Ok(Channel { pivot, kind, vector_len, data: ChannelData::Dense { values } })
}

fn read_hlod(c: &Chunk) -> R<Hlod> {
    let mut h = Hlod {
        name: String::new(),
        hierarchy: String::new(),
        lods: Vec::new(),
        aggregates: Vec::new(),
        proxies: Vec::new(),
    };
    let sub_objects = |k: &Chunk| -> R<(f32, Vec<HlodSubObject>)> {
        let mut size = 0.0;
        let mut v = Vec::new();
        for s in k.children() {
            let s = s?;
            let mut r = s.reader();
            match s.id {
                id::HLOD_SUB_OBJECT_ARRAY_HEADER => {
                    let _count = r.u32()?;
                    size = r.f32()?;
                }
                id::HLOD_SUB_OBJECT => {
                    let bone = r.u32()?;
                    let name = r.name(NAME_LEN * 2)?;
                    v.push(HlodSubObject { bone, name });
                }
                _ => {}
            }
        }
        Ok((size, v))
    };
    for k in c.children() {
        let k = k?;
        match k.id {
            id::HLOD_HEADER => {
                let mut r = k.reader();
                let _v = r.u32()?;
                let _n = r.u32()?;
                h.name = r.name(NAME_LEN)?;
                h.hierarchy = r.name(NAME_LEN)?;
            }
            id::HLOD_LOD_ARRAY => {
                let (max_screen_size, objects) = sub_objects(&k)?;
                h.lods.push(HlodLod { max_screen_size, objects });
            }
            id::HLOD_AGGREGATE_ARRAY => h.aggregates = sub_objects(&k)?.1,
            id::HLOD_PROXY_ARRAY => h.proxies = sub_objects(&k)?.1,
            _ => {}
        }
    }
    Ok(h)
}

fn read_box(c: &Chunk) -> R<BoxObject> {
    let mut r = c.reader();
    let _v = r.u32()?;
    let attributes = r.u32()?;
    let name = r.name(NAME_LEN * 2)?;
    let color = Rgb { r: r.u8()?, g: r.u8()?, b: r.u8()? };
    r.skip(1)?;
    Ok(BoxObject { name, attributes, color, center: r.vec3()?, extent: r.vec3()? })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn chunk(id: u32, children: bool, data: &[u8]) -> Vec<u8> {
        let mut v = id.to_le_bytes().to_vec();
        let size = data.len() as u32 | if children { 0x8000_0000 } else { 0 };
        v.extend_from_slice(&size.to_le_bytes());
        v.extend_from_slice(data);
        v
    }

    fn name(s: &str, len: usize) -> Vec<u8> {
        let mut v = s.as_bytes().to_vec();
        v.resize(len, 0);
        v
    }

    #[test]
    fn hierarchy_and_hlod() {
        let mut hh = 0x0004_0001u32.to_le_bytes().to_vec();
        hh.extend(name("SKEL", 16));
        hh.extend(2u32.to_le_bytes());
        hh.extend([0u8; 12]);
        let mut piv = Vec::new();
        for (n, parent) in [("ROOTTRANSFORM", u32::MAX), ("TURRET", 0)] {
            piv.extend(name(n, 16));
            piv.extend(parent.to_le_bytes());
            piv.extend([0u8; 24]);
            piv.extend([0f32, 0.0, 0.0, 1.0].iter().flat_map(|f| f.to_le_bytes()));
        }
        let mut hier = chunk(id::HIERARCHY_HEADER, false, &hh);
        hier.extend(chunk(id::PIVOTS, false, &piv));
        let mut file = chunk(id::HIERARCHY, true, &hier);

        let mut hlh = 0x0001_0000u32.to_le_bytes().to_vec();
        hlh.extend(1u32.to_le_bytes());
        hlh.extend(name("TANK", 16));
        hlh.extend(name("SKEL", 16));
        let mut arr = chunk(id::HLOD_SUB_OBJECT_ARRAY_HEADER, false, &[1, 0, 0, 0, 0, 0, 0, 0]);
        let mut sub = 1u32.to_le_bytes().to_vec();
        sub.extend(name("TANK.TURRET", 32));
        arr.extend(chunk(id::HLOD_SUB_OBJECT, false, &sub));
        let mut hlod = chunk(id::HLOD_HEADER, false, &hlh);
        hlod.extend(chunk(id::HLOD_LOD_ARRAY, true, &arr));
        file.extend(chunk(id::HLOD, true, &hlod));

        let f = parse(&file).unwrap();
        assert_eq!(f.hierarchies[0].pivots.len(), 2);
        assert_eq!(f.hierarchies[0].pivots[1].parent, Some(0));
        assert_eq!(f.hlods[0].lods[0].objects[0].name, "TANK.TURRET");
        assert_eq!(f.hlods[0].lods[0].objects[0].bone, 1);
    }

    #[test]
    fn adaptive_delta_decoding() {
        // One component, 3 frames: base 1.0, then deltas +1 and -2 with filter index 8
        // (table 1.0) and scale 0.5.
        let mut d = 3u32.to_le_bytes().to_vec();
        d.extend(0u16.to_le_bytes()); // pivot
        d.push(1); // vector len
        d.push(11); // ADAPTIVEDELTA_X
        d.extend(0.5f32.to_le_bytes());
        d.extend(1.0f32.to_le_bytes());
        let mut packet = vec![8u8];
        packet.push(0x01 | (0x0E << 4)); // frame 1: +1, frame 2: -2
        packet.extend([0u8; 7]);
        d.extend(packet);
        let c = Chunk { id: id::COMPRESSED_ANIMATION_CHANNEL, has_children: false, data: &d, offset: 0 };
        let ch = read_adaptive_delta_channel(&c).unwrap();
        let ChannelData::Dense { values } = ch.data else { panic!() };
        assert_eq!(values, [1.0, 1.5, 0.5]);
    }

    #[test]
    fn filter_table_matches_original_shape() {
        let t = filter_table();
        assert_eq!(t[8], 1.0);
        assert_eq!(t[16], 1.0);
        assert!(t[255] > 0.0 && t[255] < 0.01);
    }
}
