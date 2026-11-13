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
//! Rivers (`isRiver`) have their own flowing geometry in the original; here they are
//! drawn as standing water for now.

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
        polygon(t, &mut mesh);
    }
    mesh
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
            mesh.vertices.push(WaterVertex { position: v.to_array() });
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
