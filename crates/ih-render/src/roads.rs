//! Roads painted over the terrain.
//!
//! Ported from `W3DDevice/GameClient/W3DRoadBuffer.cpp`: `addMapObjects` (pairs of
//! road-point map objects become segments, identical segments are dropped),
//! `preloadRoadSegment` and `loadFloat4PtSection` (a ribbon floating 1/8 height
//! step above the highest corner of each cell it crosses, columns one cell apart,
//! dropping columns that a straight line between their neighbours already covers),
//! `RoadSegment::updateSegLighting` (the terrain's static light at the nearest
//! sample), and `drawRoads` (alpha blended, road types in collection order).
//!
//! Junctions, curves and cross-type joins follow the same file in `topology`.

mod topology;

use glam::{Vec2, Vec3};
use ih_formats::map::{MAP_HEIGHT_SCALE, MAP_XY_FACTOR, Map, object_flags};

use crate::terrain::{TerrainLights, TerrainVertex, static_diffuse};

/// A `Road` definition from `Roads.ini` (`TerrainRoadType`).
#[derive(Debug, Clone, PartialEq)]
pub struct RoadType {
    pub name: String,
    pub texture: String,
    /// `RoadWidth`.
    pub width: f32,
    /// `RoadWidthInTexture`.
    pub width_in_texture: f32,
}

/// The geometry of one road type.
#[derive(Debug, Default)]
pub struct RoadMesh {
    pub texture: String,
    pub vertices: Vec<TerrainVertex>,
    pub indices: Vec<u32>,
}

/// `DEFAULT_ROAD_SCALE` for road objects with no `Road` definition.
const DEFAULT_ROAD_SCALE: f32 = 8.0;

#[derive(Clone)]
struct Segment {
    p1: Vec2,
    p2: Vec2,
    scale: f32,
    width_in_texture: f32,
    kind: Option<usize>,
    /// Bottom-left, bottom-right, top-left, top-right.
    corners: [Vec2; 4],
    flags: [i32; 2],
    radius: f32,
    shape: Shape,
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum Shape {
    Straight,
    Curve,
    Y,
    H,
    HFlip,
    Tee,
    FourWay,
    AlphaJoin,
}

impl Segment {
    fn new(p1: Vec2, p2: Vec2, scale: f32, width_in_texture: f32, kind: Option<usize>) -> Self {
        let n = (p2 - p1).normalize_or_zero().perp() * (width_in_texture * scale * 0.5);
        Self {
            p1,
            p2,
            scale,
            width_in_texture,
            kind,
            corners: [p1 - n, p2 - n, p1 + n, p2 + n],
            flags: [0; 2],
            radius: 1.5,
            shape: Shape::Straight,
        }
    }
}

/// Builds every road of the map, one mesh per road type in `types` order.
pub fn road_meshes(map: &Map, types: &[RoadType], lights: &TerrainLights, num_lights: usize) -> Vec<RoadMesh> {
    if map.width < 2 || map.height < 2 {
        return Vec::new();
    }
    let mut segments = segments(map, types);
    let stacking = topology::connect(&mut segments, types.len());
    let mut out: Vec<RoadMesh> =
        types.iter().map(|t| RoadMesh { texture: t.texture.clone(), ..Default::default() }).collect();
    for shape in
        [Shape::Straight, Shape::Curve, Shape::Y, Shape::H, Shape::HFlip, Shape::Tee, Shape::FourWay, Shape::AlphaJoin]
    {
        for s in segments.iter().filter(|s| s.shape == shape) {
            let Some(k) = s.kind else { continue };
            if !s.scale.is_finite() || s.scale <= 0.0 || !s.width_in_texture.is_finite() || s.width_in_texture <= 0.0 {
                continue;
            }
            float_section(map, &section(s), lights, num_lights, &mut out[k]);
        }
    }
    let mut out: Vec<_> = out.into_iter().enumerate().collect();
    out.sort_by_key(|(k, _)| stacking[*k]);
    let mut out: Vec<_> = out.into_iter().map(|(_, mesh)| mesh).collect();
    out.retain(|m| !m.indices.is_empty());
    out
}

/// `addMapObjects`: a `ROAD_POINT1` object followed by a `ROAD_POINT2` object.
fn segments(map: &Map, types: &[RoadType]) -> Vec<Segment> {
    let mut out: Vec<Segment> = Vec::new();
    let objs = &map.objects;
    let mut i = 0;
    while i < objs.len() {
        let o = &objs[i];
        if o.flags & object_flags::ROAD_POINT1 == 0 {
            i += 1;
            continue;
        }
        let Some(o2) = objs.get(i + 1) else { break };
        if o2.flags & object_flags::ROAD_POINT2 == 0 {
            i += 1;
            continue;
        }
        let p1 = Vec2::new(o.position[0], o.position[1]);
        let mut p2 = Vec2::new(o2.position[0], o2.position[1]);
        if p1 == p2 {
            p2.x += 0.25;
        }
        let kind = types.iter().position(|t| t.name == o.name);
        let (scale, width_in_texture) =
            kind.map_or((DEFAULT_ROAD_SCALE, 1.0), |k| (types[k].width, types[k].width_in_texture));
        let dup = out.iter().any(|s| (s.p1 == p1 && s.p2 == p2) || (s.p1 == p2 && s.p2 == p1));
        if !dup {
            let mut segment = Segment::new(p1, p2, scale, width_in_texture, kind);
            segment.flags = [o.flags, o2.flags];
            segment.radius = if o.flags & object_flags::ROAD_CORNER_TIGHT != 0 { 0.5 } else { 1.5 };
            out.push(segment);
        }
        i += 2;
    }
    out
}

/// `getMaxCellHeight`: the highest corner of the cell under (x, y).
fn max_cell_height(map: &Map, x: f32, y: f32) -> f32 {
    let ix = ((x / MAP_XY_FACTOR) as i32 + map.border).clamp(0, (map.width - 2).max(0));
    let iy = ((y / MAP_XY_FACTOR) as i32 + map.border).clamp(0, (map.height - 2).max(0));
    let h = |x, y| f32::from(map.height_at(x, y)) * MAP_HEIGHT_SCALE;
    h(ix, iy).max(h(ix + 1, iy)).max(h(ix + 1, iy + 1)).max(h(ix, iy + 1))
}

/// `preloadRoadSegment` + `loadFloat4PtSection` (columns always collapsed to two
/// vertices, as the original's `if (true)` makes them).
struct Section {
    loc: Vec2,
    normal: Vec2,
    vector: Vec2,
    corners: [Vec2; 4],
    uv: Vec2,
    scale: Vec2,
}

/// `preloadRoadSegment`, `loadCurve`, `loadTee`, `loadY`, `loadH`, `loadAlphaJoin`.
fn section(s: &Segment) -> Section {
    let d = (s.p2 - s.p1).normalize_or_zero();
    let n = d.perp();
    let w = s.scale;
    let half = s.width_in_texture * w * 0.5;
    let mut out = Section {
        loc: s.p1,
        normal: n * half,
        vector: s.p2 - s.p1,
        corners: s.corners,
        uv: Vec2::new(0.0, 85.0 / 512.0),
        scale: Vec2::splat(w),
    };
    let rect = |bl: Vec2, u: Vec2, v: Vec2| [bl, bl + u, bl + v, bl + u + v];
    match s.shape {
        Shape::Straight => {}
        Shape::Curve => {
            let (u, v) = (d * w, n * half);
            let tight = s.radius == 0.5;
            let mut c = rect(s.p1 - v, u * if tight { 0.5 } else { 1.0 }, v * 2.0);
            c[1] += u * 0.1 + v * if tight { 0.2 } else { 0.4 };
            c[0] -= v * if tight { 0.1 } else { 0.2 };
            c[0] -= u * 0.02;
            c[2] -= u * 0.02;
            c[3] += -u * 0.4 + v * if tight { 0.2 } else { 0.4 };
            out.corners = c;
            out.vector = u;
            out.uv = Vec2::new(4.0 / 512.0, if tight { 425.0 } else { 255.0 } / 512.0);
        }
        Shape::Tee | Shape::FourWay => {
            let t = w * 1.03 * 0.5;
            let v = n * t;
            let u = d * (half + t);
            out.corners = rect(s.p1 - d * half - v, u, v * 2.0);
            out.vector = u;
            out.normal = v;
            out.uv = Vec2::new(425.0 / 512.0, if s.shape == Shape::FourWay { 425.0 } else { 255.0 } / 512.0);
        }
        Shape::Y => {
            let (u, v) = (d * w * 1.59, n * w);
            out.corners = rect(s.p1 + v * 0.29 - u * 0.5 - v * 1.08, u, v * 1.08);
            out.vector = u;
            out.normal = v;
            out.uv = Vec2::new(255.0 / 512.0, 226.0 / 512.0);
        }
        Shape::H | Shape::HFlip => {
            let (u, v) = (d * w, n * w * 1.35);
            out.corners =
                rect(s.p1 - v * if s.shape == Shape::HFlip { 0.2 } else { 0.8 } - d * half, d * half + u * 1.2, v);
            out.vector = u;
            out.normal = if s.shape == Shape::HFlip { -v } else { v };
            out.uv = Vec2::new(202.0 / 512.0, 364.0 / 512.0);
        }
        Shape::AlphaJoin => {
            let (u, v) = (d * w * 48.0 / 128.0, n * s.width_in_texture * (1.0 + 8.0 / 128.0));
            out.corners = rect(s.p1 - v * 0.5 - u * 0.65, u, v);
            out.vector = u;
            out.normal = v;
            out.scale.y = s.width_in_texture;
            out.uv = Vec2::new(106.0 / 512.0, 425.0 / 512.0);
        }
    }
    out
}

fn float_section(map: &Map, s: &Section, lights: &TerrainLights, num_lights: usize, mesh: &mut RoadMesh) {
    const FLOAT_AMOUNT: f32 = MAP_HEIGHT_SCALE / 8.0;
    const MAX_ERROR: f32 = MAP_HEIGHT_SCALE * 1.1;
    let road_vector = s.vector;
    let normal = s.normal;
    let half = normal.length();
    let [bl, br, tl, tr] = s.corners;

    let road_len = road_vector.length();
    let rn = normal.normalize();
    let rv = road_vector.normalize();
    let u_count = ((road_len / MAP_XY_FACTOR) as i32 + 1).max(2) as usize;
    let v_count = ((2.0 * half / MAP_XY_FACTOR) as i32 + 1).clamp(2, 100) as usize;
    let origin = bl;
    let u1 = br - bl;
    let v1 = tl - bl;
    let v2 = tr - br;
    let u2 = (tr - tl) + (v1 - v2);

    #[derive(Clone, Copy)]
    struct Column {
        lo: Vec3,
        hi: Vec3,
        u_index: f32,
        deleted: bool,
    }
    let column = |i: usize| -> Column {
        let fi = i as f32 / (u_count - 1) as f32;
        let fib = 1.0 - fi;
        let mut max_h = f32::MIN;
        let mut pts = [Vec2::ZERO; 2];
        for j in 0..v_count {
            let fj = j as f32 / (v_count - 1) as f32;
            let fjb = 1.0 - fj;
            let p = origin + u1 * fjb * fi + u2 * fj * fi + v1 * fib * fj + v2 * fi * fj;
            max_h = max_h.max(max_cell_height(map, p.x, p.y));
            if j == 0 {
                pts[0] = p;
            }
            if j == v_count - 1 {
                pts[1] = p;
            }
        }
        Column { lo: pts[0].extend(max_h), hi: pts[1].extend(max_h), u_index: i as f32, deleted: false }
    };

    let mut kept: Vec<Column> = Vec::new();
    let mut prev: Option<Column> = None;
    let mut cur = column(0);
    for i in 1..=u_count {
        let next = (i < u_count).then(|| column(i));
        if let (Some(p), Some(n)) = (prev, next) {
            // Keep the original's swapped interpolation weights, including its
            // behaviour after earlier columns have been dropped.
            let lerp = |a: f32, b: f32| {
                (a * (cur.u_index - p.u_index) + b * (n.u_index - cur.u_index)) / (n.u_index - p.u_index)
            };
            let z0 = lerp(p.lo.z, n.lo.z);
            let z1 = lerp(p.hi.z, n.hi.z);
            if z0 >= cur.lo.z && z0 < cur.lo.z + MAX_ERROR && z1 >= cur.hi.z && z1 < cur.hi.z + MAX_ERROR {
                cur.deleted = true;
            }
        }
        if !cur.deleted {
            kept.push(cur);
            prev = Some(cur);
        }
        match next {
            Some(n) => cur = n,
            None => break,
        }
    }
    // The original's interpolation reads prev/next of the original grid; kept
    // columns only update prev, which is what happens there too.

    let border = map.border;
    let mut vertex = |p: Vec3| {
        let d = p.truncate() - s.loc;
        let v = rn.dot(d);
        let u = rv.dot(d);
        let sx = (p.x / MAP_XY_FACTOR + 0.5) as i32 + border;
        let sy = (p.y / MAP_XY_FACTOR + 0.5) as i32 + border;
        let l = static_diffuse(map, num_lights, lights, sx, sy);
        mesh.vertices.push(TerrainVertex {
            position: [p.x, p.y, p.z + FLOAT_AMOUNT],
            color: [l[0], l[1], l[2], 1.0],
            uv0: [s.uv.x + u / (s.scale.x * 4.0), s.uv.y - v / (s.scale.y * 4.0)],
            uv1: [0.0, 0.0],
        });
        mesh.vertices.len() as u32 - 1
    };
    let mut last: Option<(u32, u32)> = None;
    for c in &kept {
        let a = vertex(c.lo);
        let b = vertex(c.hi);
        if let Some((pa, pb)) = last {
            mesh.indices.extend([pb, pa, a, pb, a, b]);
        }
        last = Some((a, b));
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use ih_formats::map::{Dict, MapObject};

    fn point(name: &str, x: f32, y: f32, flags: i32) -> MapObject {
        MapObject { name: name.into(), position: [x, y, 999.0], angle: 0.0, flags, properties: Dict::default() }
    }

    fn fixture() -> (Map, Vec<RoadType>, TerrainLights) {
        let map = Map {
            width: 8,
            height: 8,
            border: 1,
            heights: vec![16; 64],
            objects: vec![
                point("Lane", 0.0, 20.0, object_flags::ROAD_POINT1),
                point("Lane", 50.0, 20.0, object_flags::ROAD_POINT2),
            ],
            ..Default::default()
        };
        let types =
            vec![RoadType { name: "Lane".into(), texture: "lane.tga".into(), width: 20.0, width_in_texture: 0.5 }];
        let lights =
            TerrainLights { ambient: [0.2, 0.3, 0.4], diffuse: [[0.5; 3]; 3], position: [[0.0, 0.0, -1.0]; 3] };
        (map, types, lights)
    }

    #[test]
    fn flat_ribbon_matches_original_height_uv_lighting_and_winding() {
        let (map, types, lights) = fixture();
        let meshes = road_meshes(&map, &types, &lights, 1);
        let mesh = &meshes[0];
        // All interior columns collapse on flat ground. Object z is ignored.
        assert_eq!(mesh.vertices.len(), 4);
        assert_eq!(mesh.indices, [1, 0, 2, 1, 2, 3]);
        assert_eq!(mesh.vertices[0].position, [0.0, 15.0, 10.0 + MAP_HEIGHT_SCALE / 8.0]);
        assert_eq!(mesh.vertices[3].position, [50.0, 25.0, 10.0 + MAP_HEIGHT_SCALE / 8.0]);
        assert_eq!(mesh.vertices[0].uv0, [0.0, 85.0 / 512.0 + 5.0 / 80.0]);
        assert_eq!(mesh.vertices[3].uv0, [50.0 / 80.0, 85.0 / 512.0 - 5.0 / 80.0]);
        assert_eq!(mesh.vertices[0].color, [178.0 / 255.0, 204.0 / 255.0, 229.0 / 255.0, 1.0]);
        for tri in mesh.indices.as_chunks::<3>().0 {
            let [a, b, c] = [tri[0], tri[1], tri[2]].map(|i| Vec3::from(mesh.vertices[i as usize].position));
            assert!((b - a).cross(c - a).z > 0.0);
        }
    }

    #[test]
    fn ridge_keeps_columns_and_raises_both_edges() {
        let (mut map, types, lights) = fixture();
        // A ridge through the ribbon's centre must lift even its two outer edges.
        map.heights[3 + 3 * 8] = 80;
        let mesh = &road_meshes(&map, &types, &lights, 0)[0];
        assert!(mesh.vertices.len() > 4);
        assert!(mesh.vertices.iter().any(|v| v.position[2] == 50.0 + MAP_HEIGHT_SCALE / 8.0));
        for pair in mesh.vertices.as_chunks::<2>().0 {
            assert_eq!(pair[0].position[2], pair[1].position[2]);
        }
        assert_eq!(max_cell_height(&map, -1000.0, -1000.0), 10.0);
        assert_eq!(max_cell_height(&map, 1000.0, 1000.0), 10.0);
    }

    #[test]
    fn pairs_deduplicate_reversed_segments_and_ignore_orphans() {
        let (mut map, mut types, lights) = fixture();
        types.push(RoadType { name: "Other".into(), texture: "other.tga".into(), ..types[0].clone() });
        // Duplicate suppression is by endpoints, including across road types.
        map.objects.extend([
            point("Other", 50.0, 20.0, object_flags::ROAD_POINT1),
            point("Other", 0.0, 20.0, object_flags::ROAD_POINT2),
            point("Lane", 0.0, 0.0, object_flags::ROAD_POINT1),
            point("Tree", 40.0, 40.0, 0),
            point("Lane", 30.0, 30.0, object_flags::ROAD_POINT2),
            point("Lane", 0.0, 0.0, object_flags::ROAD_POINT1),
        ]);
        assert_eq!(segments(&map, &types).len(), 1);
        let meshes = road_meshes(&map, &types, &lights, 0);
        assert_eq!(meshes.len(), 1);
        assert_eq!(meshes[0].texture, "lane.tga");
    }

    #[test]
    fn zero_length_segment_is_nudged_and_invalid_width_is_skipped() {
        let (mut map, mut types, lights) = fixture();
        map.objects[1].position = map.objects[0].position;
        let meshes = road_meshes(&map, &types, &lights, 0);
        assert_eq!(meshes[0].vertices[2].position[0], 0.25);
        assert!(meshes[0].vertices.iter().all(|v| v.position.iter().chain(&v.uv0).all(|x| x.is_finite())));
        types[0].width = 0.0;
        assert!(road_meshes(&map, &types, &lights, 0).is_empty());
        assert!(road_meshes(&Map::default(), &types, &lights, 0).is_empty());
    }
}
