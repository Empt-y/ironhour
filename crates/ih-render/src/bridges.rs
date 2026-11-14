//! Pristine map bridges, ported from `W3DBridgeBuffer.cpp`: `load`,
//! `getIndicesNVertices`, `getModelVertices`, `getModelVerticesFixed`.
//! The model's posed LEFT/SPAN/RIGHT meshes define the end lengths and repeating
//! span. X is fitted to the map endpoints; Y and Z retain `BridgeScale`.

use crate::{
    Assets, BuildOptions, model, pose,
    roads::RoadMesh,
    terrain::{TerrainLights, TerrainVertex},
};
use glam::{Mat4, Vec3};

#[derive(Clone, Debug)]
pub struct BridgeType {
    pub model: String,
    pub texture: String,
    pub scale: f32,
}

#[derive(Clone, Default)]
struct Part {
    vertices: Vec<(Vec3, Vec3, [f32; 2])>,
    indices: Vec<u32>,
}
impl Part {
    fn x_bounds(&self) -> (f32, f32) {
        self.vertices.iter().fold((f32::INFINITY, f32::NEG_INFINITY), |(lo, hi), (p, _, _)| (lo.min(p.x), hi.max(p.x)))
    }
}

fn parts(model: &model::Model) -> [Part; 3] {
    let bones = model.hierarchy.as_ref().map(|h| pose::base_pose(h, Mat4::IDENTITY)).unwrap_or_default();
    let mut parts: [Part; 3] = Default::default();
    for b in model.batches.iter().filter(|b| b.pass == 0) {
        let short = b.mesh.split_once('.').map_or(b.mesh.as_str(), |(_, s)| s).to_ascii_uppercase();
        let Some(k) = ["BRIDGE_LEFT", "BRIDGE_SPAN", "BRIDGE_RIGHT"].iter().position(|s| short.starts_with(s)) else {
            continue;
        };
        let out = &mut parts[k];
        let base = out.vertices.len() as u32;
        for v in &b.vertices {
            let m = bones.get(v.bone as usize).copied().unwrap_or(Mat4::IDENTITY);
            out.vertices.push((
                m.transform_point3(Vec3::from(v.position)),
                m.transform_vector3(Vec3::from(v.normal)),
                v.uv0,
            ));
        }
        out.indices.extend(b.indices.iter().map(|i| base + i));
    }
    parts
}

/// X intervals for the end meshes and repeating section, as the original rounds
/// `spannable` to the nearest whole span. Invalid model ranges become fixed.
fn layout(parts: &[Part; 3], desired_length: f32) -> Option<(f32, Vec<(usize, f32)>)> {
    if parts[0].vertices.is_empty() {
        return None;
    }
    let (left_min, left_max) = parts[0].x_bounds();
    let offset = -left_min;
    if parts[1].vertices.is_empty() || parts[2].vertices.is_empty() {
        return Some(((left_max - left_min).max(1.0), vec![(0, offset)]));
    }
    let (span_min, span_max) = parts[1].x_bounds();
    let (right_min, right_max) = parts[2].x_bounds();
    let length = (right_max - left_min).max(1.0);
    let span = right_min - left_max;
    let sectional = left_max <= span_min + 0.05 * length && right_min >= span_max - 0.05 * length && span > 0.001;
    let count =
        if sectional { ((desired_length - (length - span) + span * 0.5) / span).floor().max(0.0) as usize } else { 1 };
    let bridge_length = length + (count as f32 - 1.0) * span;
    if !bridge_length.is_finite() || bridge_length <= 0.0 {
        return None;
    }
    let mut pieces = vec![(0, offset)];
    pieces.extend((0..count).map(|i| (1, offset + i as f32 * span)));
    pieces.push((2, offset + (count as f32 - 1.0) * span));
    Some((bridge_length, pieces))
}

pub fn build(
    assets: &mut Assets,
    def: &BridgeType,
    from: Vec3,
    to: Vec3,
    lights: &TerrainLights,
    num_lights: usize,
) -> Result<RoadMesh, String> {
    let model = model::build(assets, &def.model, &BuildOptions::default()).map_err(|e| e.to_string())?;
    let parts = parts(&model);
    let delta = to - from;
    let desired = delta.length();
    if desired < 0.001 || !def.scale.is_finite() || def.scale <= 0.0 {
        return Err("invalid bridge length or scale".into());
    }
    let (length, pieces) =
        layout(&parts, desired).ok_or_else(|| format!("{}: invalid or missing BRIDGE_LEFT mesh", def.model))?;
    let x = delta / length;
    let y = Vec3::new(-delta.y, delta.x, 0.0).normalize_or_zero() * def.scale;
    let dz = delta.z / desired;
    // The source's vecZ X component is in world X, even when the bridge turns.
    let z = Vec3::new(-dz, 0.0, (1.0 - dz * dz).max(0.0).sqrt()) * def.scale;
    let mut mesh = RoadMesh { texture: def.texture.clone(), ..Default::default() };
    for (k, offset) in pieces {
        let part = &parts[k];
        let base = mesh.vertices.len() as u32;
        for (p, n, uv) in &part.vertices {
            let world = from + x * (p.x + offset) + y * p.y + z * p.z;
            let n = (x * n.x + y * n.y + z * n.z).normalize_or_zero();
            let mut c = Vec3::from(lights.ambient);
            for i in 0..num_lights.min(3) {
                let ray = -Vec3::from(lights.position[i]).normalize_or_zero();
                c += Vec3::from(lights.diffuse[i]) * ray.dot(n).clamp(0.0, 1.0);
            }
            let c = c.clamp(Vec3::ZERO, Vec3::ONE);
            let c = (c * 255.0).floor() / 255.0;
            mesh.vertices.push(TerrainVertex {
                position: world.to_array(),
                color: [c.x, c.y, c.z, 1.0],
                uv0: *uv,
                uv1: [0.0; 2],
            });
        }
        mesh.indices.extend(part.indices.iter().map(|i| base + i));
    }
    Ok(mesh)
}

#[cfg(test)]
mod tests {
    use super::*;
    fn part(lo: f32, hi: f32) -> Part {
        Part {
            vertices: vec![(Vec3::new(lo, 0.0, 0.0), Vec3::Z, [0.0; 2]), (Vec3::new(hi, 0.0, 0.0), Vec3::Z, [0.0; 2])],
            indices: vec![],
        }
    }
    #[test]
    fn spans_repeat_between_endpieces_and_short_bridges_can_have_no_span() {
        let p = [part(-100.0, -50.0), part(-50.0, 50.0), part(50.0, 100.0)];
        let (len, pieces) = layout(&p, 450.0).unwrap();
        assert_eq!(len, 500.0);
        assert_eq!(pieces, [(0, 100.0), (1, 100.0), (1, 200.0), (1, 300.0), (1, 400.0), (2, 400.0)]);
        let (len, pieces) = layout(&p, 80.0).unwrap();
        assert_eq!(len, 100.0);
        assert_eq!(pieces, [(0, 100.0), (2, 0.0)]);
    }
    #[test]
    fn fixed_bridge_uses_left_mesh_extent() {
        let (len, pieces) = layout(&[part(-30.0, 70.0), Part::default(), Part::default()], 300.0).unwrap();
        assert_eq!(len, 100.0);
        assert_eq!(pieces, [(0, 30.0)]);
    }
}
