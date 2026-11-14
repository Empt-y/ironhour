//! Standing water: the flat water areas of a map.
//!
//! Ported from `W3DDevice/GameClient/Water/W3DWater.cpp` (`renderWater`,
//! `drawTrapezoidWater`, `setupFlatWaterShader` and its trapezoid pixel shader) and
//! the soft shoreline of `BaseHeightMap.cpp` (`updateShorelineTile`,
//! `initDestAlphaLUT`). Water type 0 (stock GameData).
//!
//! Each water polygon is drawn as a fan of four-sided pieces (point 0, k, k+1,
//! k+2), each a grid of up to 50x50 quads. The pixel colour is
//! `diffuse * water + sparkles * noise`; it is blended by an opacity that grows
//! with the depth of water over the terrain (`min(depth / (depth_scale), opacity)`),
//! which the original wrote into destination alpha.
//!
//! Rivers use `drawRiverWater`'s opposing-bank strip, per-vertex heights,
//! `RiverStart` orientation, length repeats and animated texture wobble.

use bytemuck::{Pod, Zeroable};
use glam::Vec3;
use ih_formats::map::{MAP_XY_FACTOR, Map, PolygonTrigger};

/// `WaterTransparency` and the time-of-day `WaterSet` values the flat water uses.
#[derive(Debug, Clone)]
pub struct WaterSettings {
    /// `TransparentWaterDepth`.
    pub transparent_depth: f32,
    /// `TransparentWaterMinOpacity`.
    pub min_opacity: f32,
    /// `StandingWaterColor` (0..1).
    pub standing_color: [f32; 3],
    /// `StandingWaterTexture`.
    pub texture: String,
    /// `AdditiveBlending`.
    pub additive: bool,
    /// The time of day's `DiffuseColor` (0..255, RGBA).
    pub diffuse: [u8; 4],
}

impl Default for WaterSettings {
    /// Stock `Water.ini`, afternoon.
    fn default() -> Self {
        Self {
            transparent_depth: 3.0,
            min_opacity: 1.0,
            standing_color: [1.0; 3],
            texture: "TWWater01.tga".into(),
            additive: false,
            diffuse: [185, 185, 185, 255],
        }
    }
}

impl WaterSettings {
    /// The vertex colour (`drawTrapezoidWater`). With the default white
    /// `StandingWaterColor`, terrain light 0's ambient plus each terrain light's
    /// diffuse scaled by how much it points down, times `DiffuseColor`. The original
    /// reads the packed colour's channels in BGR order, which is kept.
    pub fn vertex_color(&self, ambient: [f32; 3], lights: &[([f32; 3], [f32; 3])]) -> [f32; 4] {
        let c = self.standing_color;
        let alpha = f32::from(self.diffuse[3]) / 255.0;
        let rgb = if c == [1.0, 1.0, 1.0] {
            let mut s = Vec3::from(ambient);
            for (pos, diffuse) in lights {
                if -pos[2] > 0.0 {
                    s += -pos[2] * Vec3::from(*diffuse);
                }
            }
            let d = self.diffuse;
            // waterShadeR = waterDiffuse & 0xff (the blue byte), and so on.
            let w = Vec3::new(f32::from(d[2]), f32::from(d[1]), f32::from(d[0])) / 255.0;
            let v = s * w;
            // Packed with REAL_TO_INT, so values above 1 wrap in the original; clamp.
            [v.x.min(1.0), v.y.min(1.0), v.z.min(1.0)]
        } else if c == [0.0, 0.0, 0.0] {
            [1.0; 3]
        } else {
            c
        };
        [rgb[0], rgb[1], rgb[2], alpha]
    }
}

#[repr(C)]
#[derive(Debug, Clone, Copy, Pod, Zeroable)]
pub struct WaterVertex {
    pub position: [f32; 3],
    /// River texture u and distance along the strip; u = -1 for standing water.
    pub river_uv: [f32; 2],
}

#[derive(Debug, Default)]
pub struct WaterMesh {
    pub vertices: Vec<WaterVertex>,
    pub indices: Vec<u32>,
}

/// Builds every water area of the map.
pub fn water_mesh(map: &Map) -> WaterMesh {
    let mut mesh = WaterMesh::default();
    for t in map.triggers.iter().filter(|t| t.is_water && t.points.len() > 2) {
        if t.is_river {
            river(t, &mut mesh);
        } else {
            polygon(t, &mut mesh);
        }
    }
    mesh
}

/// `drawRiverWater`: walk away from the starting edge around both banks.
fn river(t: &PolygonTrigger, mesh: &mut WaterMesh) {
    let n = t.points.len();
    let pairs = n / 2;
    if pairs < 2 || t.river_start < 0 || t.river_start as usize >= n - 1 {
        return;
    }
    let mut inner = t.river_start as usize;
    let mut outer = inner + 1;
    let p = |i: usize| Vec3::new(t.points[i][0] as f32, t.points[i][1] as f32, t.points[i][2] as f32);
    let width = (p(inner) - p(outer)).truncate().length();
    if width <= 0.0 {
        return;
    }
    // The original omits the closing polygon edge from this total.
    let total: f32 = (0..n - 1).map(|i| (p(i) - p(i + 1)).truncate().length()).sum();
    let v_scale = (total * 0.5 - width) / width / (pairs - 1) as f32;
    let base = mesh.vertices.len() as u32;
    for i in 0..pairs {
        mesh.vertices.push(WaterVertex { position: p(outer).to_array(), river_uv: [0.5, v_scale * i as f32] });
        mesh.vertices.push(WaterVertex { position: p(inner).to_array(), river_uv: [0.0, v_scale * i as f32] });
        inner = (inner + n - 1) % n;
        outer = (outer + 1) % n;
    }
    for i in 0..pairs as u32 - 1 {
        let a = base + i * 2;
        mesh.indices.extend([a, a + 1, a + 3, a, a + 3, a + 2]);
    }
}

fn polygon(t: &PolygonTrigger, mesh: &mut WaterMesh) {
    let p = |i: usize| {
        let q = t.points[i];
        Vec3::new(q[0] as f32, q[1] as f32, q[2] as f32)
    };
    let n = t.points.len();
    let mut k = 1;
    while k < n - 1 {
        let pt3 = p(0);
        let pt2 = p(k);
        let pt1 = p(k + 1);
        let pt0 = if k + 2 < n { p(k + 2) } else { p(k + 1) };
        trapezoid([pt0, pt1, pt2, pt3], mesh);
        k += 2;
    }
}

/// `drawTrapezoidWater`: a bilinear patch over the four points.
fn trapezoid(points: [Vec3; 4], mesh: &mut WaterMesh) {
    let origin = points[0];
    let mut u1 = points[1];
    let mut v1 = points[3];
    let u2 = points[2] - v1;
    let v2 = points[2] - u1;
    u1 -= origin;
    v1 -= origin;
    let count = |a: Vec3, b: Vec3| (((a.length() + b.length()) / (8.0 * MAP_XY_FACTOR)) as i32).clamp(1, 50) as usize;
    let uc = count(u1, u2) + 1;
    let vc = count(v1, v2) + 1;
    let base = mesh.vertices.len() as u32;
    for j in 0..vc {
        let dv = j as f32 / (vc - 1) as f32;
        for i in 0..uc {
            let du = i as f32 / (uc - 1) as f32;
            let v = origin + u1 * du + v1 * dv + dv * du * (v2 - v1);
            mesh.vertices.push(WaterVertex { position: v.to_array(), river_uv: [-1.0, 0.0] });
        }
    }
    let at = |i: usize, j: usize| base + (j * uc + i) as u32;
    for j in 0..vc - 1 {
        for i in 0..uc - 1 {
            mesh.indices.extend([at(i, j), at(i + 1, j + 1), at(i, j + 1), at(i, j), at(i + 1, j), at(i + 1, j + 1)]);
        }
    }
}

/// The height map as an R8 image for the shoreline depth test: (width, height, bytes).
pub fn height_image(map: &Map) -> (u32, u32, Vec<u8>) {
    (map.width as u32, map.height as u32, map.heights.clone())
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn river_walks_banks_and_keeps_point_heights_and_original_repeat_count() {
        let t = PolygonTrigger {
            name: "river".into(),
            layer: String::new(),
            id: 1,
            is_water: true,
            is_river: true,
            river_start: 0,
            points: vec![[0, 0, 10], [20, 0, 10], [20, 100, 20], [20, 200, 30], [0, 200, 30], [0, 100, 20]],
        };
        let mut mesh = WaterMesh::default();
        river(&t, &mut mesh);
        assert_eq!(mesh.vertices.len(), 6);
        assert_eq!(mesh.indices.len(), 12);
        assert_eq!(mesh.vertices[0].position, [20.0, 0.0, 10.0]);
        assert_eq!(mesh.vertices[3].position, [0.0, 100.0, 20.0]);
        assert_eq!(mesh.vertices[4].river_uv, [0.5, 7.5]);
        assert_eq!(&mesh.indices[..6], [0, 1, 3, 0, 3, 2]);
        let mut t = t;
        t.points[1] = t.points[0];
        let mut mesh = WaterMesh::default();
        river(&t, &mut mesh);
        assert!(mesh.indices.is_empty());
    }
}
