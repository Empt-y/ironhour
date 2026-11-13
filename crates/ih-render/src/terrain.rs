//! Terrain: the tile atlas, per-vertex lighting, and the base, blend and extra-blend
//! geometry.
//!
//! Ported from `W3DDevice/GameClient/WorldHeightMap.cpp` (`readTexClass`,
//! `countTiles`, `readTiles`, `updateTileTexturePositions`, `getUVForNdx`,
//! `getUVForTileIndex`, `getAlphaUVData`, `getExtraAlphaUVData`), `TerrainTex.cpp`
//! (`TerrainTextureClass::update`), `HeightMap.cpp` (`updateVB`, the index buffer,
//! `renderExtraBlendTiles`) and `BaseHeightMap.cpp` (`doTheLight`, `getStaticDiffuse`).
//!
//! The original draws each height-map cell as its own quad of four vertices in
//! two passes over the same geometry: the base tile, then the blend tile with
//! per-vertex alpha. Here both are one pass: `mix(base, blend, alpha) * diffuse`,
//! which is what the two blended passes produce. Cells where three textures meet
//! get a third, alpha-blended quad.

use bytemuck::{Pod, Zeroable};
use ih_formats::map::{BlendTile, BlendTiles, FLIPPED_MASK, INVERTED_MASK, MAP_HEIGHT_SCALE, MAP_XY_FACTOR, Map};
use std::collections::HashMap;

/// `TEXTURE_WIDTH`: the atlas is always 2048 pixels wide.
pub const TEXTURE_WIDTH: i32 = 2048;
/// `TILE_PIXEL_EXTENT`: a source tile is 64x64 pixels, a cell shows a quarter of one.
pub const TILE_PIXEL_EXTENT: i32 = 64;
/// `TILE_OFFSET`: space between texture classes in the atlas (a 4-pixel border each side).
pub const TILE_OFFSET: i32 = 8;
const TILE_BYTES: usize = (TILE_PIXEL_EXTENT * TILE_PIXEL_EXTENT * 4) as usize;

/// `PATHFIND_CLIFF_SLOPE_LIMIT_F`: height difference across a cell that makes it a cliff.
pub const CLIFF_SLOPE_LIMIT: f32 = 9.8;

/// GameData switches that change how terrain is built.
#[derive(Debug, Clone, Copy)]
pub struct TerrainOptions {
    /// `AdjustCliffTextures`: stretch tiles over steep cells.
    pub adjust_cliff_textures: bool,
    /// `Use3WayTerrainBlends`: draw the extra blend layer.
    pub use_3way_blends: bool,
    /// `NumberGlobalLights`.
    pub num_global_lights: usize,
}

impl Default for TerrainOptions {
    /// Stock Zero Hour's GameData.ini.
    fn default() -> Self {
        Self { adjust_cliff_textures: true, use_3way_blends: true, num_global_lights: 3 }
    }
}

/// Terrain lights for the current time of day (`m_terrainAmbient/Diffuse/LightPos`).
#[derive(Debug, Clone, Copy)]
pub struct TerrainLights {
    pub ambient: [f32; 3],
    pub diffuse: [[f32; 3]; 3],
    /// Not normalised: the original uses the stored values as they are.
    pub position: [[f32; 3]; 3],
}

impl TerrainLights {
    pub fn from_map(map: &Map) -> Option<Self> {
        let l = map.lighting.as_ref()?.current();
        Some(Self {
            ambient: l.terrain[0].ambient,
            diffuse: [l.terrain[0].diffuse, l.terrain[1].diffuse, l.terrain[2].diffuse],
            position: [l.terrain[0].position, l.terrain[1].position, l.terrain[2].position],
        })
    }
}

/// An RGBA8 image, rows top to bottom.
#[derive(Debug, Clone)]
pub struct Atlas {
    pub width: u32,
    pub height: u32,
    pub pixels: Vec<u8>,
}

#[repr(C)]
#[derive(Debug, Clone, Copy, Pod, Zeroable, Default)]
pub struct TerrainVertex {
    pub position: [f32; 3],
    /// Lighting in rgb; blend alpha in a.
    pub color: [f32; 4],
    pub uv0: [f32; 2],
    pub uv1: [f32; 2],
}

#[derive(Debug, Default)]
pub struct TerrainMesh {
    /// Base and blend layers: four vertices per cell.
    pub vertices: Vec<TerrainVertex>,
    pub indices: Vec<u32>,
    /// Three-way blend layer, drawn alpha-blended over the base.
    pub extra_vertices: Vec<TerrainVertex>,
    pub extra_indices: Vec<u32>,
}

/// Placement of a texture class in the atlas.
#[derive(Debug, Clone, Copy, Default)]
struct ClassPos {
    x: i32,
    y: i32,
}

pub struct Terrain<'a> {
    map: &'a Map,
    tiles: &'a BlendTiles,
    opts: TerrainOptions,
    /// Per source tile: RGBA pixels in file row order, and its atlas location.
    source: Vec<Option<Box<[u8]>>>,
    tile_pos: Vec<(i32, i32)>,
    class_pos: Vec<ClassPos>,
    /// `m_terrainTexHeight`.
    tex_height: i32,
    /// Cliff bits, recomputed from heights for maps saved before version 7.
    cliff_state: Vec<u8>,
    pub warnings: Vec<String>,
}

impl<'a> Terrain<'a> {
    /// `textures` maps a `TerrainType` name to its file under `Art\Terrain\`;
    /// `read` loads a file from the game's file system.
    pub fn new(
        map: &'a Map,
        opts: TerrainOptions,
        textures: &HashMap<String, String>,
        mut read: impl FnMut(&str) -> Option<Vec<u8>>,
    ) -> Option<Self> {
        let tiles = map.tiles.as_ref()?;
        let n = tiles.num_bitmap_tiles.max(0) as usize;
        let mut t = Terrain {
            map,
            tiles,
            opts,
            source: vec![None; n],
            tile_pos: vec![(0, 0); n],
            class_pos: vec![ClassPos::default(); tiles.texture_classes.len()],
            tex_height: 0,
            cliff_state: tiles.cliff_state.clone(),
            warnings: Vec::new(),
        };
        for class in &tiles.texture_classes {
            // readTexClass: findTerrain is an exact name match.
            let Some(file) = textures.get(&class.name) else {
                t.warnings.push(format!("terrain type {} is not defined", class.name));
                continue;
            };
            let path = format!("Art\\Terrain\\{file}");
            let Some(data) = read(&path) else {
                t.warnings.push(format!("{path}: not found"));
                continue;
            };
            let available = count_tiles(&data);
            if available < class.num_tiles {
                t.warnings.push(format!("{path}: has {available} tiles, class needs {}", class.num_tiles));
                continue;
            }
            let mut num = class.num_tiles;
            let mut width = 1;
            for w in (1..=10).rev() {
                if num >= w * w {
                    num = w * w;
                    width = w;
                    break;
                }
            }
            let _ = num;
            if let Some(cut) = read_tiles(&data, width) {
                for (i, tile) in cut.into_iter().enumerate() {
                    let idx = class.first_tile as usize + i;
                    if idx < t.source.len() {
                        t.source[idx] = Some(tile);
                    }
                }
            }
        }
        if !tiles.cliff_state_stored {
            t.init_cliff_flags_from_heights();
        }
        let height = t.update_tile_texture_positions();
        t.tex_height = (height.max(1) as u32).next_power_of_two() as i32;
        Some(t)
    }

    /// `updateTileTexturePositions`: packs texture classes into the atlas, widest
    /// first, and returns the height used.
    fn update_tile_texture_positions(&mut self) -> i32 {
        const PER_ROW: usize = (TEXTURE_WIDTH / (TILE_PIXEL_EXTENT + TILE_OFFSET)) as usize;
        let mut grid = [[true; PER_ROW]; PER_ROW];
        let mut max_height = 0;
        for tile_width in (1..=PER_ROW as i32).rev() {
            for (ci, class) in self.tiles.texture_classes.iter().enumerate() {
                let width = class.width;
                if width != tile_width {
                    continue;
                }
                let w = width as usize;
                let mut found = None;
                // The search stops at the first free cell of each row, whether or not
                // the block fits there.
                'rows: for row in 0..(PER_ROW + 1).saturating_sub(w) {
                    for column in 0..(PER_ROW + 1).saturating_sub(w) {
                        if grid[row][column] {
                            let open = (0..w).all(|i| (0..w).all(|j| grid[row + j][column + i]));
                            if open {
                                found = Some((row, column));
                                break 'rows;
                            }
                            break;
                        }
                    }
                }
                let Some((row, column)) = found else {
                    self.warnings.push(format!("texture class {} does not fit in the atlas", class.name));
                    continue;
                };
                let x0 = TILE_OFFSET / 2 + column as i32 * (TILE_PIXEL_EXTENT + TILE_OFFSET);
                let y0 = TILE_OFFSET / 2 + row as i32 * (TILE_PIXEL_EXTENT + TILE_OFFSET);
                self.class_pos[ci] = ClassPos { x: x0, y: y0 };
                max_height = max_height.max(y0 + width * TILE_PIXEL_EXTENT + TILE_OFFSET / 2);
                for i in 0..w {
                    for j in 0..w {
                        grid[row + j][column + i] = false;
                        let base = class.first_tile as usize + i + j * w;
                        if base >= self.source.len() || self.source[base].is_none() {
                            continue;
                        }
                        self.tile_pos[base] =
                            (x0 + i as i32 * TILE_PIXEL_EXTENT, y0 + (width - j as i32 - 1) * TILE_PIXEL_EXTENT);
                    }
                }
            }
        }
        max_height
    }

    /// `TerrainTextureClass::update`: copies tiles in (rows inverted), then wraps a
    /// 4-pixel border around each class. Colours are reduced to 5 bits per channel,
    /// as in the original's A1R5G5B5 texture.
    pub fn atlas(&self) -> Atlas {
        let (w, h) = (TEXTURE_WIDTH as usize, self.tex_height as usize);
        let mut px = vec![0u8; w * h * 4];
        let five = |c: u8| {
            let v = c >> 3;
            (v << 3) | (v >> 2)
        };
        let ext = TILE_PIXEL_EXTENT as usize;
        for (i, tile) in self.source.iter().enumerate() {
            let Some(tile) = tile else { continue };
            let (x, y) = self.tile_pos[i];
            if x <= 0 {
                continue;
            }
            for j in 0..ext {
                let src_row = ext - 1 - j;
                let row = y as usize + j;
                if row >= h {
                    continue;
                }
                for k in 0..ext {
                    let s = (src_row * ext + k) * 4;
                    let d = (row * w + x as usize + k) * 4;
                    px[d] = five(tile[s]);
                    px[d + 1] = five(tile[s + 1]);
                    px[d + 2] = five(tile[s + 2]);
                    px[d + 3] = 255;
                }
            }
        }
        for (ci, class) in self.tiles.texture_classes.iter().enumerate() {
            let o = self.class_pos[ci];
            if o.x <= 0 {
                continue;
            }
            let width = (class.width * TILE_PIXEL_EXTENT) as usize;
            let (ox, oy) = (o.x as usize, o.y as usize);
            let copy = |px: &mut Vec<u8>, dst: usize, src: usize, n: usize| {
                if src + n * 4 <= px.len() && dst + n * 4 <= px.len() {
                    px.copy_within(src..src + n * 4, dst);
                }
            };
            for j in 0..width {
                let row = (oy + j) * w;
                copy(&mut px, (row + ox - 4) * 4, (row + ox + width - 4) * 4, 4);
                copy(&mut px, (row + ox + width) * 4, (row + ox) * 4, 4);
            }
            for j in 0..4 {
                let before = (oy - j - 1) * w + ox - 4;
                copy(&mut px, before * 4, (before + width * w) * 4, width + 8);
                let after = (oy + j) * w + ox - 4;
                copy(&mut px, (after + width * w) * 4, after * 4, width + 8);
            }
        }
        Atlas { width: w as u32, height: h as u32, pixels: px }
    }

    fn height(&self, x: i32, y: i32) -> i32 {
        i32::from(self.map.height_at(x, y))
    }

    fn init_cliff_flags_from_heights(&mut self) {
        let fw = ((self.map.width + 7) / 8) as usize;
        for x in 0..self.map.width - 1 {
            for y in 0..self.map.height - 1 {
                let hs = [self.height(x, y), self.height(x + 1, y), self.height(x, y + 1), self.height(x + 1, y + 1)]
                    .map(|h| h as f32 * MAP_HEIGHT_SCALE);
                let min = hs.iter().cloned().fold(f32::MAX, f32::min);
                let max = hs.iter().cloned().fold(f32::MIN, f32::max);
                if max - min > CLIFF_SLOPE_LIMIT {
                    self.cliff_state[y as usize * fw + (x as usize >> 3)] |= 1 << (x & 7);
                }
            }
        }
    }

    /// Whether a cell is flagged as a cliff (`getCliffState`).
    pub fn cliff_state(&self, x: i32, y: i32) -> bool {
        if x < 0 || y < 0 || x >= self.map.width || y >= self.map.height {
            return false;
        }
        let fw = ((self.map.width + 7) / 8) as usize;
        self.cliff_state[y as usize * fw + (x as usize >> 3)] & (1 << (x & 7)) != 0
    }

    /// `getUVForNdx` (quarter tiles).
    fn uv_for_ndx(&self, tile_ndx: i32) -> (f32, f32, f32, f32) {
        let base = (tile_ndx >> 2) as usize;
        if base >= self.source.len() || self.source[base].is_none() {
            return (0.0, 0.0, 0.0, 0.0);
        }
        let (x, y) = self.tile_pos[base];
        let th = self.tex_height as f32;
        let tw = TEXTURE_WIDTH as f32;
        let (mut nu, mut nv) = (x as f32 / tw, y as f32 / th);
        let (mut xu, mut xv) = ((x + TILE_PIXEL_EXTENT) as f32 / tw, (y + TILE_PIXEL_EXTENT) as f32 / th);
        let mid_x = (nu + xu) / 2.0;
        let mid_y = (nv + xv) / 2.0;
        if tile_ndx & 2 != 0 {
            xv = mid_y;
        } else {
            nv = mid_y;
        }
        if tile_ndx & 1 != 0 {
            nu = mid_x;
        } else {
            xu = mid_x;
        }
        (nu, nv, xu, xv)
    }

    fn class_of(&self, base: i32) -> Option<usize> {
        self.tiles
            .texture_classes
            .iter()
            .position(|c| c.first_tile >= 0 && base >= c.first_tile && base < c.first_tile + c.num_tiles)
    }

    /// `getUVForTileIndex`. Returns UVs for corners (x,y), (x+1,y), (x+1,y+1),
    /// (x,y+1), and whether the cell was stretched or cliff-mapped (which decides
    /// its diagonal).
    fn uv_for_tile_index(&self, ndx: usize, tile_ndx: i32) -> ([f32; 4], [f32; 4], bool) {
        let (nu, nv, xu, xv) = self.uv_for_ndx(tile_ndx);
        let mut u = [nu, xu, xu, nu];
        let mut v = [xv, xv, nv, nv];
        if !self.opts.adjust_cliff_textures || nu == 0.0 {
            return (u, v, false);
        }
        let th = self.tex_height as f32;
        let tw = TEXTURE_WIDTH as f32;
        let cliff_ndx = self.tiles.cliff_info_ndxes[ndx];
        if cliff_ndx != 0 {
            let info = &self.tiles.cliff_info[cliff_ndx as usize];
            let n1 = tile_ndx >> 2;
            let n2 = info.tile_index >> 2;
            if let Some(ci) = self.class_of(n1) {
                let c = &self.tiles.texture_classes[ci];
                if n2 >= c.first_tile && n2 < c.first_tile + c.num_tiles {
                    let p = self.class_pos[ci];
                    let min_u = p.x as f32 / tw;
                    let max_v = (p.y + c.width * TILE_PIXEL_EXTENT) as f32 / th;
                    let v_factor = tw / th;
                    for k in 0..4 {
                        u[k] = info.u[k] + min_u;
                        v[k] = info.v[k] * v_factor + max_v;
                    }
                    return (u, v, info.flip);
                }
            }
        }
        // DO_OLD_UV: stretch tiles over steep cells.
        const STRETCH_LIMIT: f32 = 1.5;
        const TILE_LIMIT: f32 = 4.0;
        const TALL_STRETCH_LIMIT: f32 = 2.0;
        const DIAMOND_STRETCH_LIMIT: f32 = 2.4;
        const HEIGHT_SCALE: f32 = MAP_HEIGHT_SCALE / MAP_XY_FACTOR;
        let w = self.map.width as usize;
        let data_size = self.map.heights.len();
        if ndx + w + 1 >= data_size {
            return (u, v, true);
        }
        let hm = &self.map.heights;
        let (h0, h1, h2, h3) =
            (i32::from(hm[ndx]), i32::from(hm[ndx + 1]), i32::from(hm[ndx + w + 1]), i32::from(hm[ndx + w]));
        let min_h = h0.min(h1).min(h2).min(h3);
        let max_h = h0.max(h1).max(h2).max(h3);
        let delta_h = max_h - min_h;
        let below_limit = min_h + (2 * delta_h + 1) / 3;
        let above_limit = min_h + (delta_h + 1) / 3;
        let below = [h0, h1, h2, h3].iter().filter(|&&h| h < below_limit).count();
        let above = [h0, h1, h2, h3].iter().filter(|&&h| h > above_limit).count();
        let dh = delta_h as f32 * HEIGHT_SCALE;
        if dh < STRETCH_LIMIT {
            return (u, v, false);
        }
        let Some(ci) = self.class_of(tile_ndx >> 2) else { return (u, v, false) };
        let c = &self.tiles.texture_classes[ci];
        let p = self.class_pos[ci];
        let nub = p.x as f32;
        let nvb = p.y as f32 / th;
        let xub = (nub + (c.width * TILE_PIXEL_EXTENT) as f32) / tw;
        let xvb = (p.y + c.width * TILE_PIXEL_EXTENT) as f32 / th;
        let divisor = (TILE_LIMIT / dh).clamp(1.0, TILE_LIMIT);
        let delta_v = xvb - nvb;
        if above != 1 && below != 1 && (above != 2 || below != 2) && dh < DIAMOND_STRETCH_LIMIT {
            return (u, v, false);
        }
        if below == 1 || above > below {
            // one low corner
            if h0 == min_h {
                v[0] = nv + delta_v / divisor;
            } else if h1 == min_h {
                v[1] = nv + delta_v / divisor;
            } else if h2 == min_h {
                v[2] = xv - delta_v / divisor;
            } else if h3 == min_h {
                v[3] = xv - delta_v / divisor;
            }
        } else if above == 1 || below > above {
            // one high corner
            if h0 == max_h {
                v[0] = nv + delta_v / divisor;
            } else if h1 == max_h {
                v[1] = nv + delta_v / divisor;
            } else if h2 == max_h {
                v[2] = xv - delta_v / divisor;
            } else if h3 == max_h {
                v[3] = xv - delta_v / divisor;
            }
        } else {
            // two up, two down
            if dh < TALL_STRETCH_LIMIT {
                return (u, v, false);
            }
            let stretch = |a: i32, b: i32| {
                let d = (a - b) as f32 * HEIGHT_SCALE;
                let d = (1.0 + d * d).sqrt();
                if d < STRETCH_LIMIT { 1.0 } else { d.min(TILE_LIMIT) }
            };
            let dx = stretch(h3, h2) * (xu - nu);
            let dy = stretch(h3, h0) * (xv - nv);
            u = [nu, nu + dx, nu + dx, nu];
            v = [nv + dy, nv + dy, nv, nv];
            let dx = stretch(h1, h0) * (xu - nu);
            let dy = stretch(h2, h1) * (xv - nv);
            u[1] = u[0] + dx;
            v[1] = v[3] + dy;
        }
        // Keep the coordinates inside the class.
        let mut adj_v = 0.0f32;
        for vk in &v {
            adj_v = adj_v.max(nvb - vk);
        }
        for vk in &mut v {
            *vk += adj_v;
        }
        let (mut adj_u, mut adj_v) = (0.0f32, 0.0f32);
        for k in 0..4 {
            adj_u = adj_u.max(u[k] - xub);
            adj_v = adj_v.max(v[k] - xvb);
        }
        for k in 0..4 {
            u[k] -= adj_u;
            v[k] -= adj_v;
        }
        (u, v, true)
    }

    /// Alpha pattern of a blend tile (`getAlphaUVData`/`getExtraAlphaUVData`), and
    /// whether the cell's diagonal must flip.
    fn blend_alpha(b: &BlendTile) -> ([u8; 4], bool) {
        let mut a = [0u8; 4];
        let mut flip = false;
        let inverted = b.inverted & INVERTED_MASK != 0;
        if b.horiz {
            flip = b.inverted & FLIPPED_MASK != 0;
            if inverted {
                a[0] = 255;
                a[3] = 255;
            } else {
                a[1] = 255;
                a[2] = 255;
            }
        }
        if b.vert {
            flip = b.inverted & FLIPPED_MASK != 0;
            if inverted {
                a[0] = 255;
                a[1] = 255;
            } else {
                a[2] = 255;
                a[3] = 255;
            }
        }
        if b.right_diagonal {
            if inverted {
                a[1] = 255;
                if b.long_diagonal {
                    a[0] = 255;
                    a[2] = 255;
                }
            } else {
                flip = true;
                a[2] = 255;
                if b.long_diagonal {
                    a[1] = 255;
                    a[3] = 255;
                }
            }
        }
        if b.left_diagonal {
            if inverted {
                flip = true;
                a[0] = 255;
                if b.long_diagonal {
                    a[1] = 255;
                    a[3] = 255;
                }
            } else {
                a[3] = 255;
                if b.long_diagonal {
                    a[0] = 255;
                    a[2] = 255;
                }
            }
        }
        if b.custom_blend_edge_class >= 0 {
            return ([0; 4], false);
        }
        (a, flip)
    }

    /// Vertex lighting at sample (x, y) (`doTheLight` with the normal from
    /// `updateVB`/`getStaticDiffuse`: central differences, clamped at the edges).
    fn light(&self, lights: &TerrainLights, x: i32, y: i32) -> [f32; 3] {
        let (w, h) = (self.map.width, self.map.height);
        let (un0, up1) = ((x - 1).max(0), (x + 1).min(w - 1));
        let (vn0, vp1) = ((y - 1).max(0), (y + 1).min(h - 1));
        let l2r = glam::Vec3::new(
            2.0 * MAP_XY_FACTOR,
            0.0,
            MAP_HEIGHT_SCALE * (self.height(up1, y) - self.height(un0, y)) as f32,
        );
        let n2f = glam::Vec3::new(
            0.0,
            2.0 * MAP_XY_FACTOR,
            MAP_HEIGHT_SCALE * (self.height(x, vp1) - self.height(x, vn0)) as f32,
        );
        let n = l2r.cross(n2f).normalize();
        let mut c = glam::Vec3::from(lights.ambient);
        for i in 0..self.opts.num_global_lights.min(3) {
            let ray = -glam::Vec3::from(lights.position[i]);
            let shade = ray.dot(n).clamp(0.0, 1.0);
            c += shade * glam::Vec3::from(lights.diffuse[i]);
        }
        let c = c.clamp(glam::Vec3::ZERO, glam::Vec3::ONE);
        // REAL_TO_INT truncates when packing the colour.
        [(c.x * 255.0).trunc() / 255.0, (c.y * 255.0).trunc() / 255.0, (c.z * 255.0).trunc() / 255.0]
    }

    /// Builds the terrain geometry in world units: x and y are
    /// `(index - border) * MAP_XY_FACTOR`, z is `height * MAP_HEIGHT_SCALE`.
    pub fn mesh(&self, lights: &TerrainLights) -> TerrainMesh {
        let (w, h) = (self.map.width, self.map.height);
        let border = self.map.border;
        let mut lit = Vec::with_capacity((w * h) as usize);
        for y in 0..h {
            for x in 0..w {
                lit.push(self.light(lights, x, y));
            }
        }
        let pos = |x: i32, y: i32| {
            [
                (x - border) as f32 * MAP_XY_FACTOR,
                (y - border) as f32 * MAP_XY_FACTOR,
                self.height(x, y) as f32 * MAP_HEIGHT_SCALE,
            ]
        };
        let corners = |x: i32, y: i32| [(x, y), (x + 1, y), (x + 1, y + 1), (x, y + 1)];
        let mut mesh = TerrainMesh::default();
        let t = self.tiles;
        for y in 0..h - 1 {
            for x in 0..w - 1 {
                let ndx = (y * w + x) as usize;
                let (u, v, stretched) = self.uv_for_tile_index(ndx, i32::from(t.tile_ndxes[ndx]));
                let blend = t.blend_tile_ndxes[ndx] as usize;
                let (ua, va, alpha, mut flip, stretched) = if blend == 0 {
                    (u, v, [0u8; 4], false, stretched)
                } else {
                    let b = &t.blended[blend];
                    let (ua, va, s) = self.uv_for_tile_index(ndx, b.blend_ndx);
                    let (a, f) = Self::blend_alpha(b);
                    (ua, va, a, f, s)
                };
                if stretched {
                    let (p0, p1, p2, p3) =
                        (self.height(x, y), self.height(x + 1, y), self.height(x + 1, y + 1), self.height(x, y + 1));
                    flip = (p0 - p2).abs() > (p1 - p3).abs();
                }
                let base = mesh.vertices.len() as u32;
                for (k, (cx, cy)) in corners(x, y).into_iter().enumerate() {
                    let l = lit[(cy * w + cx) as usize];
                    mesh.vertices.push(TerrainVertex {
                        position: pos(cx, cy),
                        color: [l[0], l[1], l[2], f32::from(alpha[k]) / 255.0],
                        uv0: [u[k], v[k]],
                        uv1: [ua[k], va[k]],
                    });
                }
                mesh.indices.extend(quad_indices(base, flip));

                // renderExtraBlendTiles
                let extra = t.extra_blend_tile_ndxes[ndx] as usize;
                if self.opts.use_3way_blends && extra != 0 {
                    let b = &t.blended[extra];
                    let (eu, ev, cliff) = self.uv_for_tile_index(ndx, b.blend_ndx);
                    let (a, mut flip) = Self::blend_alpha(b);
                    let (p0, p1, p2, p3) =
                        (self.height(x, y), self.height(x + 1, y), self.height(x + 1, y + 1), self.height(x, y + 1));
                    if cliff && (p0 - p2).abs() > (p1 - p3).abs() {
                        flip = true;
                    }
                    let base = mesh.extra_vertices.len() as u32;
                    for (k, (cx, cy)) in corners(x, y).into_iter().enumerate() {
                        let l = lit[(cy * w + cx) as usize];
                        mesh.extra_vertices.push(TerrainVertex {
                            position: pos(cx, cy),
                            color: [l[0], l[1], l[2], f32::from(a[k]) / 255.0],
                            uv0: [eu[k], ev[k]],
                            uv1: [0.0, 0.0],
                        });
                    }
                    mesh.extra_indices.extend(quad_indices(base, flip));
                }
            }
        }
        mesh
    }
}

/// The cell index buffer: triangles (0,2,3),(0,1,2), or with the diagonal flipped
/// (1,3,0),(1,2,3).
fn quad_indices(base: u32, flip: bool) -> [u32; 6] {
    if flip {
        [base + 1, base + 3, base, base + 1, base + 2, base + 3]
    } else {
        [base, base + 2, base + 3, base, base + 1, base + 2]
    }
}

/// Targa header fields `countTiles`/`readTiles` use. Like the original, they read
/// pixels straight after the 18-byte header (the image ID field is not skipped)
/// and ignore the origin bit, so tiles come out in the file's row order.
struct TgaHeader {
    color_map_type: u8,
    image_type: u8,
    width: i32,
    height: i32,
    depth: u8,
}

fn tga_header(d: &[u8]) -> Option<TgaHeader> {
    if d.len() < 18 {
        return None;
    }
    Some(TgaHeader {
        color_map_type: d[1],
        image_type: d[2],
        width: i32::from(i16::from_le_bytes([d[12], d[13]])),
        height: i32::from(i16::from_le_bytes([d[14], d[15]])),
        depth: d[16],
    })
}

/// `WorldHeightMap::countTiles`.
pub fn count_tiles(data: &[u8]) -> i32 {
    let Some(h) = tga_header(data) else { return 0 };
    let tw = h.width / TILE_PIXEL_EXTENT;
    let th = h.height / TILE_PIXEL_EXTENT;
    if h.color_map_type != 0 || (h.image_type != 2 && h.image_type != 10) || !(24..=32).contains(&h.depth) {
        return 0;
    }
    if tw > 10 || th > 10 {
        return 0;
    }
    let n = tw.min(th);
    n * n
}

/// `WorldHeightMap::readTiles`: cuts `rows * rows` tiles out of a Targa. Each tile
/// is 64x64 RGBA in file row order.
pub fn read_tiles(data: &[u8], rows: i32) -> Option<Vec<Box<[u8]>>> {
    let h = tga_header(data)?;
    let mut tw = h.width / TILE_PIXEL_EXTENT;
    let mut th = h.height / TILE_PIXEL_EXTENT;
    if h.height == TILE_PIXEL_EXTENT / 2 {
        th = 1;
    }
    if h.width == TILE_PIXEL_EXTENT / 2 {
        tw = 1;
    }
    if tw < rows && th < rows {
        return None;
    }
    let compressed = h.image_type & 8 != 0;
    let bpp = usize::from(h.depth).div_ceil(8);
    if !(3..=4).contains(&bpp) {
        return None;
    }
    let mut tiles: Vec<Box<[u8]>> = (0..rows * rows).map(|_| vec![0u8; TILE_BYTES].into_boxed_slice()).collect();
    let mut s = 18usize;
    let mut buf = [0u8; 4];
    let mut repeat = 0u32;
    let mut running = false;
    let ext = TILE_PIXEL_EXTENT;
    let read = |s: &mut usize, buf: &mut [u8; 4]| {
        for (i, b) in buf.iter_mut().enumerate().take(bpp) {
            *b = data.get(*s + i).copied().unwrap_or(0);
        }
        *s += bpp;
    };
    for row in 0..rows * ext {
        for column in 0..h.width {
            let (r, g, b, a);
            if row < h.height {
                if compressed && repeat == 0 {
                    let flag = data.get(s).copied().unwrap_or(0);
                    s += 1;
                    repeat = u32::from(flag & 0x7f) + 1;
                    running = flag & 0x80 != 0;
                    if running {
                        read(&mut s, &mut buf);
                    }
                }
                if compressed {
                    repeat -= 1;
                }
                if !running {
                    read(&mut s, &mut buf);
                }
                r = buf[2];
                g = buf[1];
                b = buf[0];
                a = if bpp == 4 { buf[3] } else { 255 };
            } else {
                (r, g, b, a) = (0, 0, 0, 0);
            }
            if column >= rows * ext {
                continue;
            }
            let tile = (column / ext + rows * (row / ext)) as usize;
            let p = (((row % ext) * ext + column % ext) * 4) as usize;
            tiles[tile][p..p + 4].copy_from_slice(&[r, g, b, a]);
        }
    }
    Some(tiles)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tga(w: i16, h: i16, pixel: impl Fn(i32, i32) -> [u8; 3]) -> Vec<u8> {
        let mut d = vec![0u8; 18];
        d[2] = 2;
        d[12..14].copy_from_slice(&w.to_le_bytes());
        d[14..16].copy_from_slice(&h.to_le_bytes());
        d[16] = 24;
        for row in 0..i32::from(h) {
            for col in 0..i32::from(w) {
                let [r, g, b] = pixel(col, row);
                d.extend_from_slice(&[b, g, r]);
            }
        }
        d
    }

    #[test]
    fn tiles_are_cut_in_file_row_order() {
        let data = tga(128, 128, |c, r| [(c / 64) as u8 * 100, (r / 64) as u8 * 100, 7]);
        assert_eq!(count_tiles(&data), 4);
        let tiles = read_tiles(&data, 2).unwrap();
        // tile index = column/64 + rows * (row/64)
        assert_eq!(&tiles[1][..3], &[100, 0, 7]);
        assert_eq!(&tiles[2][..3], &[0, 100, 7]);
    }

    #[test]
    fn blend_alpha_patterns() {
        let b = BlendTile { horiz: true, custom_blend_edge_class: -1, ..Default::default() };
        assert_eq!(Terrain::blend_alpha(&b), ([0, 255, 255, 0], false));
        let b = BlendTile { right_diagonal: true, custom_blend_edge_class: -1, ..Default::default() };
        assert_eq!(Terrain::blend_alpha(&b), ([0, 0, 255, 0], true));
        let b = BlendTile { vert: true, inverted: 3, custom_blend_edge_class: -1, ..Default::default() };
        assert_eq!(Terrain::blend_alpha(&b), ([255, 255, 0, 0], true));
    }
}
