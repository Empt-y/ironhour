//! Pristine bridge geometry for headless terrain logic. `W3DBridge::load` takes
//! the posed LEFT mesh's Y bounds; `getBridgeInfo` scales those bounds about the
//! map endpoints. Deck height is the endpoint plane (`Bridge::getBridgeHeight`).
use crate::{SimError, navigation::CELL};
use ih_formats::{
    map::{Map, object_flags},
    w3d::{self, Hierarchy, W3dFile},
};
use ih_ini::definitions::Definitions;
use ih_vfs::Vfs;
use std::collections::BTreeMap;

#[derive(Clone, Debug)]
pub struct Bridge {
    pub name: String,
    pub from: [f32; 3],
    pub to: [f32; 3],
    pub min_y: f32,
    pub max_y: f32,
}
impl Bridge {
    pub fn direction(&self) -> [f32; 2] {
        ih_math::normalized([self.to[0] - self.from[0], self.to[1] - self.from[1]])
    }
    pub fn length(&self) -> f32 {
        ih_math::length([self.to[0] - self.from[0], self.to[1] - self.from[1]])
    }
    pub fn local(&self, p: [f32; 2]) -> [f32; 2] {
        let d = self.direction();
        let p = [p[0] - self.from[0], p[1] - self.from[1]];
        [p[0] * d[0] + p[1] * d[1], -p[0] * d[1] + p[1] * d[0]]
    }
    pub fn world(&self, x: f32, y: f32) -> [f32; 2] {
        let d = self.direction();
        [self.from[0] + d[0] * x - d[1] * y, self.from[1] + d[1] * x + d[0] * y]
    }
    pub fn contains(&self, p: [f32; 2]) -> bool {
        let [x, y] = self.local(p);
        x >= 0.0 && x <= self.length() && y >= self.min_y && y <= self.max_y
    }
    pub fn height(&self, p: [f32; 2]) -> f32 {
        self.from[2] + self.local(p)[0] / self.length() * (self.to[2] - self.from[2])
    }
    /// The deck plane's normal (`getBridgeHeight`): tilted along the span only.
    pub fn normal(&self) -> [f32; 3] {
        let slope = (self.to[2] - self.from[2]) / self.length();
        let d = self.direction();
        let n = [-slope * d[0], -slope * d[1], 1.0];
        let len = ih_math::sqrt(n[0] * n[0] + n[1] * n[1] + 1.0);
        [n[0] / len, n[1] / len, n[2] / len]
    }
    /// Ray/deck intersection for client picking; only the finite deck is hit.
    pub fn ray_hit(&self, origin: [f32; 3], dir: [f32; 3]) -> Option<(f32, [f32; 2])> {
        let slope = (self.to[2] - self.from[2]) / self.length();
        let axis = self.direction();
        let denominator = dir[2] - slope * (dir[0] * axis[0] + dir[1] * axis[1]);
        if denominator.abs() < 0.00001 {
            return None;
        }
        let t = (self.height([origin[0], origin[1]]) - origin[2]) / denominator;
        let p = [origin[0] + t * dir[0], origin[1] + t * dir[1]];
        (t >= 0.0 && self.contains(p)).then_some((t, p))
    }
    pub fn bounds(&self) -> ([f32; 2], [f32; 2]) {
        let corners = [
            self.world(0.0, self.min_y),
            self.world(0.0, self.max_y),
            self.world(self.length(), self.min_y),
            self.world(self.length(), self.max_y),
        ];
        let mut lo = [f32::INFINITY; 2];
        let mut hi = [f32::NEG_INFINITY; 2];
        for p in corners {
            for i in 0..2 {
                lo[i] = lo[i].min(p[i]);
                hi[i] = hi[i].max(p[i]);
            }
        }
        (lo, hi)
    }
    /// `Bridge::isCellOnEnd`: the box crosses an end line (inset a cell from the rails).
    pub fn box_on_end(&self, lo: [f32; 2], hi: [f32; 2]) -> bool {
        [0.0, self.length()]
            .into_iter()
            .any(|end| line_in_box(self.world(end, self.min_y + CELL), self.world(end, self.max_y - CELL), lo, hi))
    }
    /// `TerrainLogic::objectInteractsWithBridgeLayer`: on the deck or at an end,
    /// within `LAYER_Z_CLOSE_ENOUGH_F` of the deck height.
    pub fn interacts(&self, p: [f32; 3], minor_radius: f32) -> bool {
        self.on_deck_or_end([p[0], p[1]], minor_radius) && (p[2] - self.height([p[0], p[1]])).abs() <= 10.0
    }
    /// Beside the deck: along the span but outside the rails.
    pub fn beside_deck(&self, p: [f32; 2]) -> bool {
        let [along, across] = self.local(p);
        (0.0..=self.length()).contains(&along) && !(self.min_y..=self.max_y).contains(&across)
    }
    /// The 2D part of `interacts`.
    pub fn on_deck_or_end(&self, p: [f32; 2], minor_radius: f32) -> bool {
        let r = minor_radius + CELL / 2.0;
        self.contains(p) || self.box_on_end([p[0] - r, p[1] - r], [p[0] + r, p[1] + r])
    }
    /// Cell classification from `PathfindLayer::classifyLayerMapCell` and the
    /// three `Bridge::isCell*` tests. End lines inset one cell from the rails;
    /// entry lines sit half a cell outside each end. Side tests expand 0.51 and
    /// 1.02 cells, matching the original corner rejection.
    /// Returns (clear deck, ground entry, occupies cell). Partial rail cells
    /// also obstruct ground traffic when the deck has insufficient clearance.
    pub fn classify(&self, x: i32, y: i32) -> (bool, bool, bool) {
        let lo = [x as f32 * CELL, y as f32 * CELL];
        let hi = [lo[0] + CELL, lo[1] + CELL];
        let corners = [lo, [lo[0], hi[1]], hi, [hi[0], lo[1]]].iter().filter(|p| self.contains(**p)).count();
        if corners == 4 {
            return (true, false, true);
        }
        for expand in [0.51 * CELL, 1.02 * CELL] {
            for side in [self.min_y - expand, self.max_y + expand] {
                if line_in_box(self.world(0.0, side), self.world(self.length(), side), lo, hi) {
                    return (false, false, true);
                }
            }
        }
        let mut clear = false;
        let mut entry = false;
        if self.max_y - self.min_y > 2.0 * CELL {
            for end in [0.0, self.length()] {
                clear |= line_in_box(self.world(end, self.min_y + CELL), self.world(end, self.max_y - CELL), lo, hi);
            }
            for end in [-0.5 * CELL, self.length() + 0.5 * CELL] {
                entry |= line_in_box(self.world(end, self.min_y + CELL), self.world(end, self.max_y - CELL), lo, hi);
            }
        }
        (clear || entry, entry, corners > 0 || clear || entry)
    }
}

fn line_in_box(a: [f32; 2], b: [f32; 2], lo: [f32; 2], hi: [f32; 2]) -> bool {
    let (mut enter, mut leave) = (0.0f32, 1.0f32);
    for i in 0..2 {
        let delta = b[i] - a[i];
        if delta == 0.0 {
            if a[i] < lo[i] || a[i] > hi[i] {
                return false;
            }
        } else {
            let (u, v) = ((lo[i] - a[i]) / delta, (hi[i] - a[i]) / delta);
            enter = enter.max(u.min(v));
            leave = leave.min(u.max(v));
            if enter > leave {
                return false;
            }
        }
    }
    true
}

pub fn load(map: &Map, defs: &Definitions, vfs: &Vfs, language: &str) -> Result<Vec<Bridge>, SimError> {
    let mut files = Files { vfs, language, files: BTreeMap::new() };
    let mut widths = BTreeMap::new();
    let mut out = Vec::new();
    for pair in map.objects.windows(2) {
        if pair[0].flags & object_flags::BRIDGE_POINT1 == 0 || pair[1].flags & object_flags::BRIDGE_POINT2 == 0 {
            continue;
        }
        let name = &pair[0].name;
        let (min_y, max_y) = if let Some(v) = widths.get(name) {
            *v
        } else {
            let field = |key: &str| {
                defs.field("Bridge", name, key)
                    .or_else(|| defs.field("Bridge", "DefaultBridge", key))
                    .and_then(|v| v.first().cloned())
            };
            let model = field("BridgeModelName").ok_or_else(|| SimError(format!("bridge {name}: no model")))?;
            let scale = field("BridgeScale").and_then(|v| ih_ini::values::scan_real(v.as_bytes()).ok()).unwrap_or(1.0);
            if !scale.is_finite() || scale <= 0.0 {
                return Err(SimError(format!("bridge {name}: invalid scale")));
            }
            let (lo, hi) = files.left_bounds(&model)?;
            let bounds = (lo * scale, hi * scale);
            widths.insert(name.clone(), bounds);
            bounds
        };
        let endpoint = |p: [f32; 3]| [p[0], p[1], map.ground_height(p[0], p[1]) + 0.25];
        let bridge = Bridge {
            name: name.clone(),
            from: endpoint(pair[0].position),
            to: endpoint(pair[1].position),
            min_y,
            max_y,
        };
        if !bridge.from.into_iter().chain(bridge.to).all(f32::is_finite) || bridge.length() < 0.001 {
            return Err(SimError(format!("bridge {name}: invalid endpoints")));
        }
        out.push(bridge);
    }
    // Landmark bridges (`IsBridge` objects, `Bridge::Bridge(Object*)`): the object's
    // box is the deck, flat at the object's height, from end to end of the box.
    let field = |name: &str, key: &str| defs.field("Object", name, key).and_then(|v| v.first().cloned());
    let real = |name: &str, key: &str, default: f32| {
        field(name, key).and_then(|v| ih_ini::values::scan_real(v.as_bytes()).ok()).unwrap_or(default)
    };
    for o in &map.objects {
        if o.flags & (object_flags::BRIDGE_POINT1 | object_flags::BRIDGE_POINT2) != 0 {
            continue;
        }
        if !field(&o.name, "IsBridge").is_some_and(|v| v.eq_ignore_ascii_case("yes")) {
            continue;
        }
        let (major, minor) = (real(&o.name, "GeometryMajorRadius", 1.0), real(&o.name, "GeometryMinorRadius", 1.0));
        let z = o.position[2] + map.ground_height(o.position[0], o.position[1]);
        let (c, s) = (ih_math::cos(o.angle), ih_math::sin(o.angle));
        let bridge = Bridge {
            name: o.name.clone(),
            from: [o.position[0] - major * c, o.position[1] - major * s, z],
            to: [o.position[0] + major * c, o.position[1] + major * s, z],
            min_y: -minor,
            max_y: minor,
        };
        if !bridge.from.into_iter().chain(bridge.to).all(f32::is_finite) || bridge.length() < 0.001 {
            return Err(SimError(format!("landmark bridge {}: invalid geometry", o.name)));
        }
        out.push(bridge);
    }
    Ok(out)
}

struct Files<'a> {
    vfs: &'a Vfs,
    language: &'a str,
    files: BTreeMap<String, W3dFile>,
}
impl Files<'_> {
    fn file(&mut self, stem: &str) -> Result<W3dFile, SimError> {
        let stem = stem.to_ascii_lowercase();
        if let Some(file) = self.files.get(&stem) {
            return Ok(file.clone());
        }
        for path in [format!("Data\\{}\\Art\\W3D\\{stem}.w3d", self.language), format!("Art\\W3D\\{stem}.w3d")] {
            if let Some(bytes) = self.vfs.read(&path).map_err(|e| SimError(e.to_string()))? {
                let file = w3d::parse(&bytes).map_err(|e| SimError(format!("{path}: {e}")))?;
                self.files.insert(stem, file.clone());
                return Ok(file);
            }
        }
        Err(SimError(format!("bridge model {stem}.w3d is missing")))
    }
    fn left_bounds(&mut self, name: &str) -> Result<(f32, f32), SimError> {
        let file = self.file(name)?;
        let hlod = file.hlods.iter().find(|h| h.name.eq_ignore_ascii_case(name));
        let hierarchy = match hlod {
            Some(h) => {
                file.hierarchies.iter().find(|t| t.name.eq_ignore_ascii_case(&h.hierarchy)).cloned().or_else(|| {
                    self.file(&h.hierarchy)
                        .ok()?
                        .hierarchies
                        .into_iter()
                        .find(|t| t.name.eq_ignore_ascii_case(&h.hierarchy))
                })
            }
            None => None,
        };
        if hlod.is_some() && hierarchy.is_none() {
            return Err(SimError(format!("bridge {name}: missing hierarchy")));
        }
        let subs: Vec<_> = hlod
            .into_iter()
            .flat_map(|h| {
                h.lods
                    .iter()
                    .max_by(|a, b| a.max_screen_size.total_cmp(&b.max_screen_size))
                    .into_iter()
                    .flat_map(|l| &l.objects)
                    .chain(&h.aggregates)
            })
            .collect();
        let mut points = Vec::new();
        for sub in subs {
            let short = sub.name.split_once('.').map_or(sub.name.as_str(), |(_, s)| s);
            if !short.to_ascii_uppercase().starts_with("BRIDGE_LEFT") {
                continue;
            }
            let source = if file.meshes.iter().any(|m| m.full_name().eq_ignore_ascii_case(&sub.name)) {
                file.clone()
            } else {
                self.file(sub.name.split('.').next().unwrap_or(&sub.name))?
            };
            let mesh = source
                .meshes
                .iter()
                .find(|m| m.full_name().eq_ignore_ascii_case(&sub.name))
                .ok_or_else(|| SimError(format!("bridge mesh {} is missing", sub.name)))?;
            let h = hierarchy.as_ref().unwrap();
            for p in &mesh.vertices {
                points.push(posed_point(h, sub.bone as usize, *p)?);
            }
        }
        if hlod.is_none() {
            for mesh in &file.meshes {
                if mesh.name.to_ascii_uppercase().starts_with("BRIDGE_LEFT") {
                    points.extend(&mesh.vertices);
                }
            }
        }
        y_bounds(&points).ok_or_else(|| SimError(format!("bridge {name}: invalid or missing BRIDGE_LEFT geometry")))
    }
}
fn y_bounds(points: &[[f32; 3]]) -> Option<(f32, f32)> {
    if !points.iter().flatten().all(|p| p.is_finite()) {
        return None;
    }
    let lo = points.iter().map(|p| p[1]).fold(f32::INFINITY, f32::min);
    let hi = points.iter().map(|p| p[1]).fold(f32::NEG_INFINITY, f32::max);
    (hi > lo).then_some((lo, hi))
}
// Apply rigid rest transforms from the child up to (but excluding) pivot 0,
// matching HTree::Base_Update. Scalar operations deliberately avoid SIMD/FMA.
fn posed_point(h: &Hierarchy, mut bone: usize, mut p: [f32; 3]) -> Result<[f32; 3], SimError> {
    while bone != 0 {
        let pivot = h.pivots.get(bone).ok_or_else(|| SimError(format!("{}: invalid bridge bone {bone}", h.name)))?;
        let [x, y, z, w] = pivot.rotation;
        let t = [2.0 * (y * p[2] - z * p[1]), 2.0 * (z * p[0] - x * p[2]), 2.0 * (x * p[1] - y * p[0])];
        p = [
            p[0] + w * t[0] + y * t[2] - z * t[1] + pivot.translation[0],
            p[1] + w * t[1] + z * t[0] - x * t[2] + pivot.translation[1],
            p[2] + w * t[2] + x * t[1] - y * t[0] + pivot.translation[2],
        ];
        match pivot.parent {
            Some(parent) if (parent as usize) < bone => bone = parent as usize,
            None => break,
            _ => return Err(SimError(format!("{}: invalid bridge bone ancestry", h.name))),
        }
    }
    Ok(p)
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn landmark_bridges_come_from_the_object_box() {
        let mut defs = Definitions::default();
        defs.add_document(ih_ini::parse(
            b"Object Span
IsBridge = Yes
Geometry = BOX
GeometryMajorRadius = 100
GeometryMinorRadius = 20
End
",
        ));
        let map = Map {
            width: 44,
            height: 44,
            border: 2,
            boundaries: vec![[40, 40]],
            heights: vec![0; 44 * 44],
            objects: vec![ih_formats::map::MapObject {
                position: [200.0, 200.0, 30.0],
                angle: std::f32::consts::FRAC_PI_2,
                flags: 0,
                name: "Span".into(),
                properties: Default::default(),
            }],
            ..Default::default()
        };
        let bridges = load(&map, &defs, &Vfs::new(), "English").unwrap();
        assert_eq!(bridges.len(), 1);
        let b = &bridges[0];
        // Along the facing (north), 200 long, 40 wide, flat at the object's height.
        assert!((b.from[1] - 100.0).abs() < 1e-3 && (b.to[1] - 300.0).abs() < 1e-3 && (b.from[0] - 200.0).abs() < 1e-3);
        assert_eq!((b.min_y, b.max_y, b.from[2], b.to[2]), (-20.0, 20.0, 30.0, 30.0));
    }
    #[test]
    fn sloped_rotated_deck_height_and_ray_pick() {
        let b = Bridge {
            name: "test".into(),
            from: [30.0, 20.0, 10.0],
            to: [30.0, 120.0, 30.0],
            min_y: -20.0,
            max_y: 20.0,
        };
        assert_eq!(b.height([30.0, 70.0]), 20.0);
        assert!(b.contains([15.0, 70.0]));
        assert!(!b.contains([5.0, 70.0]));
        assert_eq!(b.ray_hit([30.0, 70.0, 50.0], [0.0, 0.0, -1.0]), Some((30.0, [30.0, 70.0])));
        assert!(b.ray_hit([5.0, 70.0, 50.0], [0.0, 0.0, -1.0]).is_none());
        assert!(b.classify(2, 1).1);
        assert!(!b.classify(0, 6).0);
    }
    #[test]
    fn posed_bounds_include_bone_translation_and_rotation() {
        use ih_formats::w3d::Pivot;
        let pivot = |parent, translation, rotation| Pivot {
            name: String::new(),
            parent,
            translation,
            rotation,
            euler: [0.0; 3],
        };
        let h = Hierarchy {
            name: "B".into(),
            version: 0,
            center: [0.0; 3],
            pivots: vec![
                pivot(None, [100.0; 3], [0.0, 0.0, 0.0, 1.0]),
                pivot(Some(0), [0.0, 7.0, 0.0], [0.0, 0.0, 1.0, 0.0]),
            ],
        };
        assert_eq!(posed_point(&h, 1, [2.0, 3.0, 0.0]).unwrap(), [-2.0, 4.0, 0.0]);
        assert!(posed_point(&h, 2, [0.0; 3]).is_err());
    }
}
