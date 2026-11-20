//! Map files (`.map`): the DataChunk container and the chunks the terrain, objects
//! and lighting come from.
//!
//! Ported from `Common/System/DataChunk.cpp` (container, dictionaries),
//! `W3DDevice/GameClient/WorldHeightMap.cpp` (`ParseHeightMapData`,
//! `ParseBlendTileData`, `ParseObjectData`, `ParseLightingDataChunk`,
//! `ParseWorldDictDataChunk`) and `GameLogic/Map/PolygonTrigger.cpp`.
//!
//! Map files are usually RefPack-compressed (`EAR\0`); [`Map::parse`] accepts both.
//! The container is a table of contents ("CkMp", then id/name pairs) followed by
//! chunks: `u32 id, u16 version, i32 size, data`. A chunk's data may hold nested
//! chunks after its own fields. Parsers are matched by label and parent label, and
//! chunks without a parser are skipped, as in `DataChunkInput::parse`.

use crate::compression::{self, DecompressError};
use std::collections::HashMap;
use thiserror::Error;

/// World units per height-map cell (`MAP_XY_FACTOR`).
pub const MAP_XY_FACTOR: f32 = 10.0;
/// World units per height step (`MAP_HEIGHT_SCALE`, `MAP_XY_FACTOR / 16`).
pub const MAP_HEIGHT_SCALE: f32 = MAP_XY_FACTOR / 16.0;

#[derive(Debug, Error)]
pub enum MapError {
    #[error(transparent)]
    Decompress(#[from] DecompressError),
    #[error("not a chunk file (no CkMp table of contents)")]
    NotChunky,
    #[error("unexpected end of data in {0}")]
    Truncated(String),
    #[error("corrupt {0}: {1}")]
    Corrupt(String, String),
}

/// A value in a map dictionary (`Dict`).
#[derive(Debug, Clone, PartialEq)]
pub enum DictValue {
    Bool(bool),
    Int(i32),
    Real(f32),
    Ascii(String),
    Unicode(String),
}

/// A map dictionary, in file order.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct Dict(pub Vec<(String, DictValue)>);

impl Dict {
    pub fn get(&self, key: &str) -> Option<&DictValue> {
        // Dict::setX replaces an existing key, so the last occurrence wins.
        self.0.iter().rev().find(|(k, _)| k == key).map(|(_, v)| v)
    }
    pub fn int(&self, key: &str) -> Option<i32> {
        match self.get(key)? {
            DictValue::Int(v) => Some(*v),
            _ => None,
        }
    }
    pub fn real(&self, key: &str) -> Option<f32> {
        match self.get(key)? {
            DictValue::Real(v) => Some(*v),
            _ => None,
        }
    }
    pub fn bool(&self, key: &str) -> Option<bool> {
        match self.get(key)? {
            DictValue::Bool(v) => Some(*v),
            _ => None,
        }
    }
    pub fn ascii(&self, key: &str) -> Option<&str> {
        match self.get(key)? {
            DictValue::Ascii(v) => Some(v),
            _ => None,
        }
    }
}

/// `TXTextureClass` as stored: a run of source tiles cut from one terrain texture.
#[derive(Debug, Clone, PartialEq)]
pub struct TextureClass {
    pub first_tile: i32,
    pub num_tiles: i32,
    /// Tiles per side of the class's square block.
    pub width: i32,
    /// The `TerrainType` name (looked up in `Terrain.ini`).
    pub name: String,
}

/// `TBlendTileInfo`: how a blend layer covers a cell.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct BlendTile {
    /// Tile index (in quarter tiles) of the texture blended over the base.
    pub blend_ndx: i32,
    pub horiz: bool,
    pub vert: bool,
    pub right_diagonal: bool,
    pub left_diagonal: bool,
    /// `INVERTED_MASK` (1) and `FLIPPED_MASK` (2).
    pub inverted: u8,
    pub long_diagonal: bool,
    pub custom_blend_edge_class: i32,
}

pub const INVERTED_MASK: u8 = 0x1;
pub const FLIPPED_MASK: u8 = 0x2;

/// `TCliffInfo`: explicit texture coordinates for a cliff cell.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct CliffInfo {
    pub tile_index: i32,
    pub u: [f32; 4],
    pub v: [f32; 4],
    pub flip: bool,
    pub mutant: bool,
}

/// The `BlendTileData` chunk.
#[derive(Debug, Clone, Default)]
pub struct BlendTiles {
    pub version: u16,
    /// Per height-map sample: base tile in quarter tiles (`tileNdx >> 2` is the source
    /// tile, the low two bits pick the quarter).
    pub tile_ndxes: Vec<i16>,
    pub blend_tile_ndxes: Vec<i16>,
    pub extra_blend_tile_ndxes: Vec<i16>,
    pub cliff_info_ndxes: Vec<i16>,
    /// Cliff flag bits, `(width + 7) / 8` bytes per row.
    pub cliff_state: Vec<u8>,
    /// Whether `cliff_state` came from the file (version 7+). Older maps derive it
    /// from heights (`initCliffFlagsFromHeights`).
    pub cliff_state_stored: bool,
    pub num_bitmap_tiles: i32,
    pub texture_classes: Vec<TextureClass>,
    pub num_edge_tiles: i32,
    pub edge_classes: Vec<TextureClass>,
    /// Index 0 is the "no blend" entry.
    pub blended: Vec<BlendTile>,
    /// Index 0 is the default entry.
    pub cliff_info: Vec<CliffInfo>,
}

/// One light of a time of day (`GlobalData::TerrainLighting`).
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Light {
    pub ambient: [f32; 3],
    pub diffuse: [f32; 3],
    pub position: [f32; 3],
}

impl Default for Light {
    fn default() -> Self {
        // The initial value ParseLightingDataChunk gives lights older maps lack.
        Self { ambient: [0.0; 3], diffuse: [0.0; 3], position: [0.0, 0.0, -1.0] }
    }
}

/// Lights for one time of day.
#[derive(Debug, Clone, Copy, Default, PartialEq)]
pub struct TimeOfDayLighting {
    pub terrain: [Light; 3],
    pub objects: [Light; 3],
}

/// The `GlobalLighting` chunk.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct Lighting {
    /// `TimeOfDay`: 1 morning, 2 afternoon, 3 evening, 4 night.
    pub time_of_day: i32,
    /// Morning, afternoon, evening, night.
    pub times: [TimeOfDayLighting; 4],
    pub shadow_color: Option<u32>,
}

impl Lighting {
    /// Lights for the map's own time of day.
    pub fn current(&self) -> &TimeOfDayLighting {
        let i = (self.time_of_day - 1).clamp(0, 3) as usize;
        &self.times[i]
    }
}

/// A placed object (`MapObject`).
#[derive(Debug, Clone, PartialEq)]
pub struct MapObject {
    pub position: [f32; 3],
    /// Radians.
    pub angle: f32,
    pub flags: i32,
    /// Thing template name.
    pub name: String,
    pub properties: Dict,
}

/// `MapObject` flags (`MapObject.h`).
pub mod object_flags {
    pub const DRAWS_IN_MIRROR: i32 = 0x001;
    pub const ROAD_POINT1: i32 = 0x002;
    pub const ROAD_POINT2: i32 = 0x004;
    pub const ROAD_CORNER_ANGLED: i32 = 0x008;
    pub const BRIDGE_POINT1: i32 = 0x010;
    pub const BRIDGE_POINT2: i32 = 0x020;
    pub const ROAD_CORNER_TIGHT: i32 = 0x040;
    pub const ROAD_JOIN: i32 = 0x080;
    pub const DONT_RENDER: i32 = 0x100;
}

/// A polygon area (`PolygonTrigger`), including water areas and rivers.
#[derive(Debug, Clone, PartialEq)]
pub struct PolygonTrigger {
    pub name: String,
    pub layer: String,
    pub id: i32,
    pub is_water: bool,
    pub is_river: bool,
    pub river_start: i32,
    pub points: Vec<[i32; 3]>,
}

/// A build-list entry of a side (`BuildListInfo`).
#[derive(Debug, Clone, PartialEq)]
pub struct BuildListEntry {
    pub building_name: String,
    pub template: String,
    /// z is forced to ground level on load.
    pub position: [f32; 3],
    pub angle: f32,
    pub initially_built: bool,
    pub rebuilds: i32,
    pub script: String,
    pub health: i32,
    pub whiner: bool,
    pub unsellable: bool,
    pub repairable: bool,
}

/// A side (`SidesList::addSide`): its player dictionary and build list.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct Side {
    pub dict: Dict,
    pub build_list: Vec<BuildListEntry>,
}

/// `SidesList` (`SidesList.cpp`), without the nested player scripts.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct SidesList {
    pub sides: Vec<Side>,
    /// Team dictionaries (`teamName`, `teamOwner`, ...).
    pub teams: Vec<Dict>,
}

/// A top-level chunk as found in the file.
#[derive(Debug, Clone, PartialEq)]
pub struct ChunkSummary {
    pub label: String,
    pub version: u16,
    pub size: u32,
}

#[derive(Debug, Clone, Default)]
pub struct Map {
    pub width: i32,
    pub height: i32,
    pub border: i32,
    /// Playable areas, in cells (`m_boundaries`).
    pub boundaries: Vec<[i32; 2]>,
    /// Heights, `width * height`, row 0 first (y increases with row).
    pub heights: Vec<u8>,
    pub height_map_version: u16,
    pub tiles: Option<BlendTiles>,
    pub world: Dict,
    pub objects: Vec<MapObject>,
    pub triggers: Vec<PolygonTrigger>,
    pub lighting: Option<Lighting>,
    pub sides: SidesList,
    /// Every top-level chunk, in order.
    pub chunks: Vec<ChunkSummary>,
    /// Problems the original tolerates (debug asserts, patched indexes).
    pub warnings: Vec<String>,
}

impl Map {
    pub fn parse(raw: &[u8]) -> Result<Map, MapError> {
        let data = compression::decompress(raw)?;
        let data: &[u8] = &data;
        let (toc, start) = read_toc(data)?;
        let mut map = Map::default();
        let mut r = Reader { data, pos: start, end: data.len(), label: String::new() };
        // DataChunkInput::parse at file level: open chunks until the data runs out.
        while r.end - r.pos >= CHUNK_HEADER {
            let chunk = r.open_chunk(&toc)?;
            map.chunks.push(ChunkSummary { label: chunk.label.clone(), version: chunk.version, size: chunk.size });
            let mut c = chunk.reader;
            match chunk.label.as_str() {
                "HeightMapData" => map.parse_height_map(&mut c, chunk.version)?,
                "BlendTileData" => map.parse_blend_tiles(&mut c, chunk.version)?,
                "WorldInfo" => map.world = c.dict(&toc)?,
                "ObjectsList" => map.parse_objects(&mut c, &toc)?,
                "PolygonTriggers" => map.parse_triggers(&mut c, chunk.version)?,
                "GlobalLighting" => map.parse_lighting(&mut c, chunk.version)?,
                "SidesList" => map.parse_sides(&mut c, &toc, chunk.version)?,
                _ => {}
            }
        }
        map.patch_indexes();
        Ok(map)
    }

    /// Height at sample (x, y), 0 outside the map.
    pub fn height_at(&self, x: i32, y: i32) -> u8 {
        if x < 0 || y < 0 || x >= self.width || y >= self.height {
            return 0;
        }
        self.heights[(y * self.width + x) as usize]
    }

    /// The smoothed normal `getHeightMapHeight` returns with the height: height
    /// differences over two samples, blended across the cell. `deltaZ_X3` is
    /// `d6 - d0`, as the original has it. Up off the map's interior.
    pub fn ground_normal(&self, x: f32, y: f32) -> [f32; 3] {
        if self.width <= 0 || self.height <= 0 {
            return [0.0, 0.0, 1.0];
        }
        let (x, y) = (x / MAP_XY_FACTOR, y / MAP_XY_FACTOR);
        let (fx, fy) = (x - x.floor(), y - y.floor());
        let (ix, iy) = (x.floor() as i32 + self.border, y.floor() as i32 + self.border);
        if ix < 1 || iy < 1 || ix > self.width - 3 || iy > self.height - 3 {
            return [0.0, 0.0, 1.0];
        }
        let d = |dx: i32, dy: i32| f32::from(self.height_at(ix + dx, iy + dy));
        let (d0, d1, d2, d3) = (d(0, 0), d(1, 0), d(1, 1), d(0, 1));
        let (d4, d5, d6, d7) = (d(0, -1), d(1, -1), d(2, 0), d(2, 1));
        // d10 (one sample left of d3) is read but unused in the original.
        let (d8, d9, d11) = (d(1, 2), d(0, 2), d(-1, 0));
        let (x0, x1, x2, x3) = (d1 - d11, d6 - d0, d7 - d3, d6 - d0);
        let (y0, y1, y2, y3) = (d3 - d4, d2 - d5, d8 - d1, d9 - d0);
        let left = x0 * (1.0 - fx) + fx * x3;
        let right = x1 * (1.0 - fx) + fx * x2;
        let dzx = left * (1.0 - fy) + fy * right;
        let left = y0 * (1.0 - fx) + fx * y3;
        let right = y1 * (1.0 - fx) + fx * y2;
        let dzy = left * (1.0 - fy) + fy * right;
        // Normalized_Cross_Product((a, 0, dzx), (0, a, dzy)), a = 2 * XY / HEIGHT_SCALE.
        let a = 2.0 * MAP_XY_FACTOR / MAP_HEIGHT_SCALE;
        let n = [-a * dzx, -a * dzy, a * a];
        let len = (n[0] * n[0] + n[1] * n[1] + n[2] * n[2]).sqrt();
        [n[0] / len, n[1] / len, n[2] / len]
    }

    /// `BaseHeightMap.cpp::getHeightMapHeight`: interpolate across the 0-to-2
    /// diagonal's two triangles. The outer samples use `getClipHeight`.
    pub fn ground_height(&self, x: f32, y: f32) -> f32 {
        if self.width <= 0 || self.height <= 0 {
            return 0.0;
        }
        let (x, y) = (x / MAP_XY_FACTOR, y / MAP_XY_FACTOR);
        let (fx, fy) = (x - x.floor(), y - y.floor());
        let (ix, iy) = (x.floor() as i32 + self.border, y.floor() as i32 + self.border);
        let h = |x: i32, y: i32| f32::from(self.height_at(x, y));
        if ix < 1 || iy < 1 || ix > self.width - 3 || iy > self.height - 3 {
            return h(ix.clamp(0, self.width - 1), iy.clamp(0, self.height - 1)) * MAP_HEIGHT_SCALE;
        }
        let (p0, p2) = (h(ix, iy), h(ix + 1, iy + 1));
        if fy > fx {
            let p3 = h(ix, iy + 1);
            (p3 + (1.0 - fy) * (p0 - p3) + fx * (p2 - p3)) * MAP_HEIGHT_SCALE
        } else {
            let p1 = h(ix + 1, iy);
            (p1 + fy * (p2 - p1) + (1.0 - fx) * (p0 - p1)) * MAP_HEIGHT_SCALE
        }
    }

    fn parse_height_map(&mut self, r: &mut Reader, version: u16) -> Result<(), MapError> {
        self.height_map_version = version;
        self.width = r.i32()?;
        self.height = r.i32()?;
        self.border = if version >= 3 { r.i32()? } else { 0 };
        self.boundaries = if version >= 4 {
            let n = r.i32()?;
            let mut b = Vec::new();
            for _ in 0..n.max(0) {
                b.push([r.i32()?, r.i32()?]);
            }
            b
        } else {
            vec![[self.width - 2 * self.border, self.height - 2 * self.border]]
        };
        let size = r.i32()?;
        if size <= 0 || size != self.width * self.height {
            return Err(r.corrupt("height data size does not match width * height"));
        }
        self.heights = r.bytes(size as usize)?.to_vec();
        if version == 1 {
            self.warnings.push("version 1 height map (5-unit cells) is not resampled".into());
        }
        r.expect_end(&mut self.warnings);
        Ok(())
    }

    fn parse_blend_tiles(&mut self, r: &mut Reader, version: u16) -> Result<(), MapError> {
        let size = r.i32()?;
        let data_size = self.width * self.height;
        if size != data_size || self.heights.is_empty() {
            return Err(r.corrupt("blend tile data size does not match the height map"));
        }
        let n = size as usize;
        let mut t = BlendTiles { version, ..Default::default() };
        t.tile_ndxes = r.i16s(n)?;
        t.blend_tile_ndxes = r.i16s(n)?;
        t.extra_blend_tile_ndxes = if version >= 6 { r.i16s(n)? } else { vec![0; n] };
        t.cliff_info_ndxes = if version >= 5 { r.i16s(n)? } else { vec![0; n] };
        let flip_width = ((self.width + 7) / 8) as usize;
        let h = self.height as usize;
        t.cliff_state = vec![0; flip_width * h];
        if version >= 7 {
            t.cliff_state_stored = true;
            if version == 7 {
                // Version 7 saved rows (width + 1) / 8 bytes wide.
                let bw = ((self.width + 1) / 8) as usize;
                let data = r.bytes(bw * h)?;
                for j in 0..h {
                    t.cliff_state[j * flip_width..j * flip_width + bw].copy_from_slice(&data[j * bw..j * bw + bw]);
                }
            } else {
                t.cliff_state.copy_from_slice(r.bytes(flip_width * h)?);
            }
        }
        t.num_bitmap_tiles = r.i32()?;
        let num_blended = r.i32()?;
        let num_cliff = if version >= 5 { r.i32()? } else { 1 };
        let num_classes = r.i32()?;
        for _ in 0..num_classes.max(0) {
            let first_tile = r.i32()?;
            let num_tiles = r.i32()?;
            let width = r.i32()?;
            let _legacy = r.i32()?;
            let name = r.ascii()?;
            t.texture_classes.push(TextureClass { first_tile, num_tiles, width, name });
        }
        if version >= 4 {
            t.num_edge_tiles = r.i32()?;
            let n = r.i32()?;
            for _ in 0..n.max(0) {
                let first_tile = r.i32()?;
                let num_tiles = r.i32()?;
                let width = r.i32()?;
                let name = r.ascii()?;
                t.edge_classes.push(TextureClass { first_tile, num_tiles, width, name });
            }
        }
        t.blended.push(BlendTile { custom_blend_edge_class: -1, ..Default::default() });
        for _ in 1..num_blended.max(1) {
            let mut b = BlendTile {
                blend_ndx: r.i32()?,
                horiz: r.u8()? != 0,
                vert: r.u8()? != 0,
                right_diagonal: r.u8()? != 0,
                left_diagonal: r.u8()? != 0,
                inverted: r.u8()?,
                ..Default::default()
            };
            b.long_diagonal = version >= 3 && r.u8()? != 0;
            b.custom_blend_edge_class = if version >= 4 { r.i32()? } else { -1 };
            if r.i32()? as u32 != 0x7ADA_0000 {
                return Err(r.corrupt("blend tile flag is not 0x7ADA0000"));
            }
            t.blended.push(b);
        }
        t.cliff_info.push(CliffInfo::default());
        if version >= 5 {
            for _ in 1..num_cliff.max(1) {
                let tile_index = r.i32()?;
                let mut u = [0.0; 4];
                let mut v = [0.0; 4];
                for k in 0..4 {
                    u[k] = r.f32()?;
                    v[k] = r.f32()?;
                }
                let flip = r.u8()? != 0;
                let mutant = r.u8()? != 0;
                t.cliff_info.push(CliffInfo { tile_index, u, v, flip, mutant });
            }
        }
        if version == 1 {
            self.warnings.push("version 1 blend tiles (5-unit cells) are not resampled".into());
        }
        r.expect_end(&mut self.warnings);
        self.tiles = Some(t);
        Ok(())
    }

    fn parse_objects(&mut self, r: &mut Reader, toc: &Toc) -> Result<(), MapError> {
        while r.end - r.pos >= CHUNK_HEADER {
            let chunk = r.open_chunk(toc)?;
            if chunk.label != "Object" {
                continue;
            }
            let mut c = chunk.reader;
            let mut position = [c.f32()?, c.f32()?, c.f32()?];
            if chunk.version <= 2 {
                position[2] = 0.0;
            }
            let angle = c.f32()?;
            let flags = c.i32()?;
            let name = c.ascii()?;
            let properties = if chunk.version >= 2 { c.dict(toc)? } else { Dict::default() };
            // Objects far outside the height range are dropped on load.
            let (min_z, max_z) = (-100.0 * MAP_XY_FACTOR, 255.0 * 10.0 * MAP_HEIGHT_SCALE);
            if position[2] < min_z || position[2] > max_z {
                self.warnings.push(format!("object {name} dropped: z {} out of range", position[2]));
                continue;
            }
            self.objects.push(MapObject { position, angle, flags, name, properties });
        }
        Ok(())
    }

    /// `SidesList::ParseSidesDataChunk`. At most `MAX_PLAYER_COUNT` (16) sides are kept;
    /// the `PlayerScriptsList` that follows is left for the script engine.
    fn parse_sides(&mut self, r: &mut Reader, toc: &Toc, version: u16) -> Result<(), MapError> {
        const MAX_PLAYER_COUNT: usize = 16;
        let count = r.i32()?;
        for i in 0..count.max(0) as usize {
            if i >= MAX_PLAYER_COUNT {
                // The original stops reading sides here (and fails on the rest).
                self.warnings.push(format!("{count} sides: only {MAX_PLAYER_COUNT} are read"));
                return Ok(());
            }
            let dict = r.dict(toc)?;
            let n = r.i32()?;
            let mut build_list = Vec::new();
            for _ in 0..n.max(0) {
                let building_name = r.ascii()?;
                let template = r.ascii()?;
                let position = [r.f32()?, r.f32()?, {
                    r.f32()?;
                    0.0
                }];
                let angle = r.f32()?;
                let initially_built = r.u8()? != 0;
                let rebuilds = r.i32()?;
                let (script, health, whiner, unsellable, repairable) = if version >= 3 {
                    (r.ascii()?, r.i32()?, r.u8()? != 0, r.u8()? != 0, r.u8()? != 0)
                } else {
                    // BuildListInfo defaults.
                    (String::new(), 100, true, false, true)
                };
                build_list.push(BuildListEntry {
                    building_name,
                    template,
                    position,
                    angle,
                    initially_built,
                    rebuilds,
                    script,
                    health,
                    whiner,
                    unsellable,
                    repairable,
                });
            }
            self.sides.sides.push(Side { dict, build_list });
        }
        if version >= 2 {
            let n = r.i32()?;
            for _ in 0..n.max(0) {
                let d = r.dict(toc)?;
                self.sides.teams.push(d);
            }
        }
        Ok(())
    }

    fn parse_triggers(&mut self, r: &mut Reader, version: u16) -> Result<(), MapError> {
        let count = r.i32()?;
        for _ in 0..count.max(0) {
            let name = r.ascii()?;
            let layer = if version >= 4 { r.ascii()? } else { String::new() };
            let id = r.i32()?;
            let is_water = version >= 2 && r.u8()? != 0;
            let (is_river, river_start) = if version >= 3 { (r.u8()? != 0, r.i32()?) } else { (false, 0) };
            let n = r.i32()?;
            let mut points = Vec::new();
            for _ in 0..n.max(0) {
                points.push([r.i32()?, r.i32()?, r.i32()?]);
            }
            if n < 2 {
                continue;
            }
            self.triggers.push(PolygonTrigger { name, layer, id, is_water, is_river, river_start, points });
        }
        if version == 1 {
            self.warnings.push("version 1 polygon triggers: the original adds a default water area".into());
        }
        Ok(())
    }

    fn parse_lighting(&mut self, r: &mut Reader, version: u16) -> Result<(), MapError> {
        let mut l = Lighting { time_of_day: r.i32()?, ..Default::default() };
        let read_light = |r: &mut Reader| -> Result<Light, MapError> {
            Ok(Light {
                ambient: [r.f32()?, r.f32()?, r.f32()?],
                diffuse: [r.f32()?, r.f32()?, r.f32()?],
                position: [r.f32()?, r.f32()?, r.f32()?],
            })
        };
        for t in &mut l.times {
            t.terrain[0] = read_light(r)?;
            t.objects[0] = read_light(r)?;
            if version >= 2 {
                for light in &mut t.objects[1..] {
                    *light = read_light(r)?;
                }
            }
            if version >= 3 {
                for light in &mut t.terrain[1..] {
                    *light = read_light(r)?;
                }
            }
        }
        if r.pos < r.end {
            l.shadow_color = Some(r.i32()? as u32);
        }
        r.expect_end(&mut self.warnings);
        self.lighting = Some(l);
        Ok(())
    }

    /// "patch bad maps" in `WorldHeightMap`'s constructor: out-of-range indexes become 0.
    fn patch_indexes(&mut self) {
        let Some(t) = &mut self.tiles else { return };
        let (nb, nc) = (t.blended.len() as i16, t.cliff_info.len() as i16);
        let mut patched = 0;
        for i in 0..t.tile_ndxes.len() {
            for (v, max) in [
                (&mut t.cliff_info_ndxes[i], nc),
                (&mut t.blend_tile_ndxes[i], nb),
                (&mut t.extra_blend_tile_ndxes[i], nb),
            ] {
                if *v < 0 || *v >= max {
                    *v = 0;
                    patched += 1;
                }
            }
        }
        if patched > 0 {
            self.warnings.push(format!("{patched} out-of-range tile indexes reset to 0"));
        }
    }
}

const CHUNK_HEADER: usize = 10;

type Toc = HashMap<u32, String>;

fn read_toc(data: &[u8]) -> Result<(Toc, usize), MapError> {
    if data.len() < 8 || &data[..4] != b"CkMp" {
        return Err(MapError::NotChunky);
    }
    let mut r = Reader { data, pos: 4, end: data.len(), label: "table of contents".into() };
    let count = r.i32()?;
    let mut toc = HashMap::new();
    for _ in 0..count.max(0) {
        let len = r.u8()? as usize;
        let name = String::from_utf8_lossy(r.bytes(len)?).into_owned();
        let id = r.i32()? as u32;
        toc.insert(id, name);
    }
    Ok((toc, r.pos))
}

struct Chunk<'a> {
    label: String,
    version: u16,
    size: u32,
    reader: Reader<'a>,
}

struct Reader<'a> {
    data: &'a [u8],
    pos: usize,
    end: usize,
    label: String,
}

impl<'a> Reader<'a> {
    fn truncated(&self) -> MapError {
        MapError::Truncated(self.label.clone())
    }
    fn corrupt(&self, why: &str) -> MapError {
        MapError::Corrupt(self.label.clone(), why.into())
    }
    fn bytes(&mut self, n: usize) -> Result<&'a [u8], MapError> {
        if self.end - self.pos < n {
            return Err(self.truncated());
        }
        let b = &self.data[self.pos..self.pos + n];
        self.pos += n;
        Ok(b)
    }
    fn u8(&mut self) -> Result<u8, MapError> {
        Ok(self.bytes(1)?[0])
    }
    fn u16(&mut self) -> Result<u16, MapError> {
        Ok(u16::from_le_bytes(self.bytes(2)?.try_into().unwrap()))
    }
    fn i32(&mut self) -> Result<i32, MapError> {
        Ok(i32::from_le_bytes(self.bytes(4)?.try_into().unwrap()))
    }
    fn f32(&mut self) -> Result<f32, MapError> {
        Ok(f32::from_le_bytes(self.bytes(4)?.try_into().unwrap()))
    }
    fn i16s(&mut self, n: usize) -> Result<Vec<i16>, MapError> {
        let b = self.bytes(n * 2)?;
        Ok(b.as_chunks::<2>().0.iter().map(|c| i16::from_le_bytes(*c)).collect())
    }
    /// `readAsciiString`: u16 length, then bytes.
    fn ascii(&mut self) -> Result<String, MapError> {
        let n = self.u16()? as usize;
        Ok(String::from_utf8_lossy(self.bytes(n)?).into_owned())
    }
    /// `readUnicodeString`: u16 count, then UTF-16 code units.
    fn unicode(&mut self) -> Result<String, MapError> {
        let n = self.u16()? as usize;
        let b = self.bytes(n * 2)?;
        let units: Vec<u16> = b.as_chunks::<2>().0.iter().map(|c| u16::from_le_bytes(*c)).collect();
        Ok(String::from_utf16_lossy(&units))
    }
    /// `readDict`: u16 count, then `(nameId << 8 | type)` and the value.
    fn dict(&mut self, toc: &Toc) -> Result<Dict, MapError> {
        let n = self.u16()?;
        let mut d = Vec::with_capacity(n as usize);
        for _ in 0..n {
            let key_type = self.i32()?;
            let key = toc.get(&((key_type >> 8) as u32)).cloned().unwrap_or_default();
            let v = match key_type & 0xff {
                0 => DictValue::Bool(self.u8()? != 0),
                1 => DictValue::Int(self.i32()?),
                2 => DictValue::Real(self.f32()?),
                3 => DictValue::Ascii(self.ascii()?),
                4 => DictValue::Unicode(self.unicode()?),
                t => return Err(self.corrupt(&format!("dictionary value type {t}"))),
            };
            d.push((key, v));
        }
        Ok(Dict(d))
    }
    fn open_chunk(&mut self, toc: &Toc) -> Result<Chunk<'a>, MapError> {
        let id = self.i32()? as u32;
        let version = self.u16()?;
        let size = self.i32()?;
        let label = toc.get(&id).cloned().unwrap_or_default();
        if size < 0 || self.end - self.pos < size as usize {
            return Err(MapError::Truncated(label));
        }
        let start = self.pos;
        self.pos += size as usize;
        Ok(Chunk {
            label: label.clone(),
            version,
            size: size as u32,
            reader: Reader { data: self.data, pos: start, end: start + size as usize, label },
        })
    }
    fn expect_end(&self, warnings: &mut Vec<String>) {
        if self.pos != self.end {
            warnings.push(format!("{}: {} bytes left over", self.label, self.end - self.pos));
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn terrain_height_follows_triangles_and_clamps_outer_samples() {
        let mut map = Map { width: 5, height: 5, border: 1, heights: vec![0; 25], ..Default::default() };
        map.heights[2 + 2 * 5] = 16;
        assert_eq!(map.ground_height(5.0, 5.0), 5.0);
        assert_eq!(map.ground_height(2.5, 7.5), 2.5);
        assert_eq!(map.ground_height(7.5, 2.5), 2.5);
        map.heights[0] = 32;
        assert_eq!(map.ground_height(-1000.0, -1000.0), 20.0);
    }

    struct W {
        names: Vec<String>,
        out: Vec<u8>,
    }
    impl W {
        fn id(&mut self, name: &str) -> u32 {
            if let Some(i) = self.names.iter().position(|n| n == name) {
                return i as u32 + 1;
            }
            self.names.push(name.into());
            self.names.len() as u32
        }
        fn chunk(&mut self, name: &str, version: u16, body: &[u8]) {
            let id = self.id(name);
            self.out.extend_from_slice(&id.to_le_bytes());
            self.out.extend_from_slice(&version.to_le_bytes());
            self.out.extend_from_slice(&(body.len() as i32).to_le_bytes());
            self.out.extend_from_slice(body);
        }
        fn finish(self) -> Vec<u8> {
            let mut f = b"CkMp".to_vec();
            f.extend_from_slice(&(self.names.len() as i32).to_le_bytes());
            for (i, n) in self.names.iter().enumerate() {
                f.push(n.len() as u8);
                f.extend_from_slice(n.as_bytes());
                f.extend_from_slice(&(i as u32 + 1).to_le_bytes());
            }
            f.extend_from_slice(&self.out);
            f
        }
    }

    fn i(v: i32) -> [u8; 4] {
        v.to_le_bytes()
    }

    #[test]
    fn height_map_objects_and_dict() {
        let mut w = W { names: vec![], out: vec![] };
        let mut hm = Vec::new();
        for v in [2, 2, 0, 1, 2, 2, 4] {
            hm.extend_from_slice(&i(v));
        }
        hm.extend_from_slice(&[1, 2, 3, 4]);
        w.chunk("HeightMapData", 4, &hm);

        let key = w.id("objectName");
        let mut obj = Vec::new();
        for f in [10.0f32, 20.0, 5.0, 1.5] {
            obj.extend_from_slice(&f.to_le_bytes());
        }
        obj.extend_from_slice(&i(0x100));
        obj.extend_from_slice(&3u16.to_le_bytes());
        obj.extend_from_slice(b"Foo");
        obj.extend_from_slice(&1u16.to_le_bytes());
        obj.extend_from_slice(&i(((key as i32) << 8) | 3));
        obj.extend_from_slice(&2u16.to_le_bytes());
        obj.extend_from_slice(b"Hi");
        let mut inner = W { names: std::mem::take(&mut w.names), out: vec![] };
        inner.chunk("Object", 3, &obj);
        w.names = inner.names;
        let list = inner.out;
        w.chunk("ObjectsList", 3, &list);

        let map = Map::parse(&w.finish()).unwrap();
        assert_eq!((map.width, map.height, map.border), (2, 2, 0));
        assert_eq!(map.boundaries, vec![[2, 2]]);
        assert_eq!(map.height_at(1, 1), 4);
        assert_eq!(map.objects.len(), 1);
        let o = &map.objects[0];
        assert_eq!(o.name, "Foo");
        assert_eq!(o.position, [10.0, 20.0, 5.0]);
        assert_eq!(o.properties.ascii("objectName"), Some("Hi"));
        assert!(map.warnings.is_empty(), "{:?}", map.warnings);
    }

    #[test]
    fn sides_build_lists_and_teams() {
        let mut w = W { names: vec![], out: vec![] };
        let s = |b: &mut Vec<u8>, v: &str| {
            b.extend_from_slice(&(v.len() as u16).to_le_bytes());
            b.extend_from_slice(v.as_bytes());
        };
        let player = w.id("playerName");
        let (team, owner) = (w.id("teamName"), w.id("teamOwner"));
        let mut b = Vec::new();
        b.extend_from_slice(&i(1));
        // Side dict: playerName = "civilian"
        b.extend_from_slice(&1u16.to_le_bytes());
        b.extend_from_slice(&i(((player as i32) << 8) | 3));
        s(&mut b, "civilian");
        // One build-list entry (version 3 fields).
        b.extend_from_slice(&i(1));
        s(&mut b, "Barracks 1");
        s(&mut b, "AmericaBarracks");
        for f in [100.0f32, 200.0, 7.0, 0.5] {
            b.extend_from_slice(&f.to_le_bytes());
        }
        b.push(1);
        b.extend_from_slice(&i(2));
        s(&mut b, "");
        b.extend_from_slice(&i(75));
        b.extend_from_slice(&[0, 1, 0]);
        // One team.
        b.extend_from_slice(&i(1));
        b.extend_from_slice(&2u16.to_le_bytes());
        b.extend_from_slice(&i(((team as i32) << 8) | 3));
        s(&mut b, "teamCivilian");
        b.extend_from_slice(&i(((owner as i32) << 8) | 3));
        s(&mut b, "civilian");
        w.chunk("SidesList", 3, &b);
        let map = Map::parse(&w.finish()).unwrap();
        let sides = &map.sides;
        assert_eq!(sides.sides.len(), 1);
        assert_eq!(sides.sides[0].dict.ascii("playerName"), Some("civilian"));
        let e = &sides.sides[0].build_list[0];
        assert_eq!((e.template.as_str(), e.position, e.angle), ("AmericaBarracks", [100.0, 200.0, 0.0], 0.5));
        assert_eq!((e.initially_built, e.rebuilds, e.health), (true, 2, 75));
        assert_eq!((e.whiner, e.unsellable, e.repairable), (false, true, false));
        assert_eq!(sides.teams[0].ascii("teamOwner"), Some("civilian"));
    }

    #[test]
    fn ground_normal_leans_away_from_the_rise() {
        // Heights rise by 2 steps per sample along x: 1.25 world units per 10.
        let (w, h) = (8, 8);
        let heights = (0..h).flat_map(|_| (0..w).map(|x| (x * 2) as u8)).collect();
        let map = Map { width: w, height: h, border: 0, heights, ..Default::default() };
        let n = map.ground_normal(25.0, 25.0);
        // dz/dx = 0.125: the normal is (-0.125, 0, 1), normalised.
        let len = (1.0f32 + 0.125 * 0.125).sqrt();
        assert!((n[0] + 0.125 / len).abs() < 1e-5 && n[1].abs() < 1e-6 && (n[2] - 1.0 / len).abs() < 1e-5, "{n:?}");
        assert_eq!(map.ground_normal(5.0, 5.0), [0.0, 0.0, 1.0], "edge samples point up");
    }
}
