//! First ground navigation layer from `AIPathfind.cpp`: 10-unit cells,
//! orthogonal/diagonal costs (10/14), turn penalties (4/8/16), stable open-list
//! ties and line-of-sight path optimisation. `terrain` adds bridge layers and
//! transient queries; `zones` adds the initial ground hierarchy. Cells are
//! classified as `classifyMap` does (cliff state, water corners, cliff expansion);
//! crusher zones remain pending.
use crate::footprint::CellType;
use ih_formats::map::{MAP_HEIGHT_SCALE, MAP_XY_FACTOR, Map};
use std::{cmp::Reverse, collections::BinaryHeap};
pub const CELL: f32 = 10.0;

#[derive(Clone)]
pub struct Grid {
    pub width: i32,
    pub height: i32,
    pub blocked: Vec<bool>,
}
impl Grid {
    /// Terrain cells for `GROUND` locomotors; structures are added by
    /// `Terrain::from_map` (`footprint`) and other surfaces use `Terrain::view`.
    pub fn from_map(map: &Map) -> Self {
        let c = classify_map(map);
        Self {
            width: c.width,
            height: c.height,
            blocked: c.kind.iter().map(|k| !k.allows(crate::footprint::surface::GROUND)).collect(),
        }
    }
    pub fn block_disc(&mut self, p: [f32; 2], radius: f32) {
        let minx = ((p[0] - radius) / CELL).floor() as i32;
        let maxx = ((p[0] + radius) / CELL).floor() as i32;
        let miny = ((p[1] - radius) / CELL).floor() as i32;
        let maxy = ((p[1] + radius) / CELL).floor() as i32;
        for y in miny.max(0)..=maxy.min(self.height - 1) {
            for x in minx.max(0)..=maxx.min(self.width - 1) {
                let dx = (p[0] - ((x as f32 + 0.5) * CELL)).abs();
                let dy = (p[1] - ((y as f32 + 0.5) * CELL)).abs();
                if dx * dx + dy * dy <= (radius + CELL * 0.71) * (radius + CELL * 0.71) {
                    self.blocked[(y * self.width + x) as usize] = true;
                }
            }
        }
    }
    fn index(&self, x: i32, y: i32) -> Option<usize> {
        if x >= 0 && y >= 0 && x < self.width && y < self.height { Some((y * self.width + x) as usize) } else { None }
    }
    pub fn passable(&self, x: i32, y: i32, radius: f32) -> bool {
        let n = ((radius / CELL) - 0.5).ceil().max(0.0) as i32;
        for dy in -n..=n {
            for dx in -n..=n {
                if self.index(x + dx, y + dy).is_none_or(|i| self.blocked[i]) {
                    return false;
                }
            }
        }
        true
    }
    pub fn line_passable(&self, a: [f32; 2], b: [f32; 2], radius: f32) -> bool {
        let dx = b[0] - a[0];
        let dy = b[1] - a[1];
        let steps = (dx.abs().max(dy.abs()) / (CELL * 0.25)).ceil().max(1.0) as usize;
        let mut prev = ((a[0] / CELL).floor() as i32, (a[1] / CELL).floor() as i32);
        for step in 0..=steps {
            let t = step as f32 / steps as f32;
            let cell = (((a[0] + dx * t) / CELL).floor() as i32, ((a[1] + dy * t) / CELL).floor() as i32);
            if !self.passable(cell.0, cell.1, radius) {
                return false;
            }
            if cell.0 != prev.0
                && cell.1 != prev.1
                && (!self.passable(cell.0, prev.1, radius) || !self.passable(prev.0, cell.1, radius))
            {
                return false;
            }
            prev = cell;
        }
        true
    }
    pub fn path(&self, start: [f32; 2], goal: [f32; 2], radius: f32) -> Option<Vec<[f32; 2]>> {
        if !start.iter().chain(&goal).all(|f| f.is_finite()) {
            return None;
        }
        let (sx, sy) = ((start[0] / CELL).floor() as i32, (start[1] / CELL).floor() as i32);
        let (gx, gy) = ((goal[0] / CELL).floor() as i32, (goal[1] / CELL).floor() as i32);
        let (s, g) = (self.index(sx, sy)?, self.index(gx, gy)?);
        if !self.passable(gx, gy, radius) {
            return None;
        }
        if self.line_passable(start, goal, radius) {
            return Some(vec![goal]);
        }
        let mut costs = vec![u32::MAX; self.blocked.len()];
        let mut parent = vec![usize::MAX; self.blocked.len()];
        let mut closed = vec![false; self.blocked.len()];
        let mut open = BinaryHeap::new();
        let mut seq = 0u64;
        costs[s] = 0;
        open.push(Reverse((0u32, seq, s)));
        let heuristic = |x: i32, y: i32| {
            let (dx, dy) = ((gx - x).unsigned_abs(), (gy - y).unsigned_abs());
            10 * dx.max(dy) + 5 * dx.min(dy)
        };
        let dirs = [(0, 1), (1, 1), (1, 0), (1, -1), (0, -1), (-1, -1), (-1, 0), (-1, 1)];
        while let Some(Reverse((_, _, i))) = open.pop() {
            if closed[i] {
                continue;
            }
            if i == g {
                let mut raw = vec![goal];
                let mut p = i;
                while parent[p] != usize::MAX {
                    p = parent[p];
                    if p != s {
                        raw.push([
                            ((p % self.width as usize) as f32 + 0.5) * CELL,
                            ((p / self.width as usize) as f32 + 0.5) * CELL,
                        ]);
                    }
                }
                raw.reverse();
                let mut path = vec![];
                let mut at = start;
                let mut next = 0;
                while next < raw.len() {
                    let mut far = next;
                    while far + 1 < raw.len() && self.line_passable(at, raw[far + 1], radius) {
                        far += 1;
                    }
                    path.push(raw[far]);
                    at = raw[far];
                    next = far + 1;
                }
                return Some(path);
            }
            closed[i] = true;
            let (x, y) = ((i % self.width as usize) as i32, (i / self.width as usize) as i32);
            for (dx, dy) in dirs {
                let (nx, ny) = (x + dx, y + dy);
                let Some(n) = self.index(nx, ny) else { continue };
                if closed[n]
                    || !self.passable(nx, ny, radius)
                    || (dx != 0 && dy != 0 && (!self.passable(x + dx, y, radius) || !self.passable(x, y + dy, radius)))
                {
                    continue;
                }
                let turn = if parent[i] != usize::MAX {
                    let p = parent[i];
                    let px = (p % self.width as usize) as i32;
                    let py = (p / self.width as usize) as i32;
                    let (oldx, oldy) = (x - px, y - py);
                    if (oldx, oldy) == (dx, dy) {
                        0
                    } else {
                        let dot = oldx * dx + oldy * dy;
                        if dot > 0 {
                            4
                        } else if dot == 0 {
                            8
                        } else {
                            16
                        }
                    }
                } else {
                    0
                };
                let cost = costs[i] + if dx == 0 || dy == 0 { 10 } else { 14 } + turn;
                if cost < costs[n] {
                    costs[n] = cost;
                    parent[n] = i;
                    seq += 1;
                    open.push(Reverse((cost + heuristic(nx, ny), seq, n)));
                }
            }
        }
        None
    }
}
/// `PolygonTrigger::pointInTrigger`: bounds check, then a crossing test that
/// ignores horizontal edges, on integer points.
pub fn point_in_trigger(points: &[[i32; 3]], p: [i32; 2]) -> bool {
    let Some(first) = points.first() else { return false };
    let (mut lo, mut hi) = ([first[0], first[1]], [first[0], first[1]]);
    for q in points {
        lo = [lo[0].min(q[0]), lo[1].min(q[1])];
        hi = [hi[0].max(q[0]), hi[1].max(q[1])];
    }
    if p[0] < lo[0] || p[1] < lo[1] || p[0] > hi[0] || p[1] > hi[1] {
        return false;
    }
    let mut inside = false;
    for (i, a) in points.iter().enumerate() {
        let b = points[(i + 1) % points.len()];
        if a[1] == b[1] {
            continue;
        }
        if (a[1] < p[1] && b[1] < p[1]) || (a[1] >= p[1] && b[1] >= p[1]) || (a[0] < p[0] && b[0] < p[0]) {
            continue;
        }
        let (dy, dx) = (b[1] - a[1], b[0] - a[0]);
        let x = a[0] as f32 + (dx * (p[1] - a[1])) as f32 / dy as f32;
        if x >= p[0] as f32 {
            inside = !inside;
        }
    }
    inside
}

/// `TerrainLogic::getWaterHandle` and `getWaterHeight`: the highest water area
/// containing the point (water at or above z 0 only). Grid water is not supported.
pub fn water_height(map: &Map, x: f32, y: f32) -> Option<f32> {
    let p = [(x + 0.5).floor() as i32, (y + 0.5).floor() as i32];
    let mut best: Option<i32> = None;
    for t in map.triggers.iter().filter(|t| t.is_water) {
        let z = t.points[0][2];
        if point_in_trigger(&t.points, p) && z >= best.unwrap_or(0) {
            best = Some(z);
        }
    }
    best.map(|z| z as f32)
}

/// `TerrainLogic::isUnderwater`.
pub fn is_underwater(map: &Map, x: f32, y: f32) -> bool {
    water_height(map, x, y).is_some_and(|w| map.ground_height(x, y) < w)
}

/// `WorldHeightMap::getCliffState` for a heightmap cell: the stored flip-state
/// bits, or `setCellCliffFlagFromHeights` when the map does not store them.
fn cliff_state(map: &Map, ix: i32, iy: i32) -> bool {
    if let Some(t) = map.tiles.as_ref().filter(|t| t.cliff_state_stored) {
        let row = ((map.width + 7) / 8) as usize;
        return t.cliff_state.get(iy as usize * row + (ix as usize >> 3)).is_some_and(|b| b & (1 << (ix & 7)) != 0);
    }
    if ix >= map.width - 1 || iy >= map.height - 1 {
        return false;
    }
    let h =
        [map.height_at(ix, iy), map.height_at(ix + 1, iy), map.height_at(ix, iy + 1), map.height_at(ix + 1, iy + 1)]
            .map(|v| f32::from(v) * MAP_HEIGHT_SCALE);
    let (min, max) = h.iter().fold((f32::MAX, f32::MIN), |(lo, hi), v| (lo.min(*v), hi.max(*v)));
    // PATHFIND_CLIFF_SLOPE_LIMIT_F
    max - min > 9.8
}

/// `BaseHeightMapRenderObjClass::isCliffCell`: the heightmap cell under the point.
pub fn is_cliff_cell(map: &Map, x: f32, y: f32) -> bool {
    if map.width < 2 || map.height < 2 {
        return false;
    }
    // `Int iX = x / MAP_XY_FACTOR` truncates.
    let ix = ((x / MAP_XY_FACTOR) as i32 + map.border).clamp(0, map.width - 2);
    let iy = ((y / MAP_XY_FACTOR) as i32 + map.border).clamp(0, map.height - 2);
    cliff_state(map, ix, iy)
}

/// The classified ground cells (`Pathfinder::classifyMap`), before objects.
pub struct Classified {
    pub width: i32,
    pub height: i32,
    pub kind: Vec<CellType>,
    /// The pinched border `classifyMap` leaves around cliffs.
    pub pinched: Vec<bool>,
}

/// `Pathfinder::classifyMap` over the playable area: `classifyMapCell` (a cliff
/// at the top-left corner, water if any corner is underwater), then cliffs grown
/// one cell into clear ground and a pinched border added around them.
pub fn classify_map(map: &Map) -> Classified {
    let bounds = map.boundaries.first().copied().unwrap_or([map.width - 2 * map.border, map.height - 2 * map.border]);
    let (w, h) = (bounds[0].max(1), bounds[1].max(1));
    let mut kind = vec![CellType::Clear; (w * h) as usize];
    for j in 0..h {
        for i in 0..w {
            let (x0, y0) = (i as f32 * CELL, j as f32 * CELL);
            let (x1, y1) = (x0 + CELL, y0 + CELL);
            let mut t = CellType::Clear;
            if is_cliff_cell(map, x0, y0) {
                t = CellType::Cliff;
            }
            if [(x0, y0), (x0, y1), (x1, y1), (x1, y0)].into_iter().any(|(x, y)| is_underwater(map, x, y)) {
                t = CellType::Water;
            }
            kind[(j * w + i) as usize] = t;
        }
    }
    let index = |i: i32, j: i32| (j * w + i) as usize;
    let cliff_like = |k: CellType| k as u8 & CellType::Cliff as u8 != 0;
    let mark_border = |kind: &[CellType], pinched: &mut [bool]| {
        for j in 0..h {
            for i in 0..w {
                if !cliff_like(kind[index(i, j)]) {
                    continue;
                }
                for k in (i - 1).max(0)..=(i + 1).min(w - 1) {
                    for l in (j - 1).max(0)..=(j + 1).min(h - 1) {
                        if kind[index(k, l)] == CellType::Clear {
                            pinched[index(k, l)] = true;
                        }
                    }
                }
            }
        }
    };
    // Expand all cliff cells one step (marked pinched, then converted).
    let mut pinched = vec![false; kind.len()];
    mark_border(&kind, &mut pinched);
    for (k, p) in kind.iter_mut().zip(&pinched) {
        if *p && *k == CellType::Clear {
            *k = CellType::Cliff;
        }
    }
    // Then a border of pinched cells around the cliffs.
    mark_border(&kind, &mut pinched);
    Classified { width: w, height: h, kind, pinched }
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn path_goes_through_wall_gap_without_cutting_corners() {
        let mut g = Grid { width: 20, height: 20, blocked: vec![false; 400] };
        for y in 0..19 {
            if y != 15 {
                g.blocked[y * 20 + 10] = true;
            }
        }
        let start = [25.0, 25.0];
        let goal = [175.0, 25.0];
        let path = g.path(start, goal, 1.0).unwrap();
        let mut last = start;
        for p in &path {
            assert!(g.line_passable(last, *p, 1.0));
            last = *p;
        }
        assert_eq!(path.last(), Some(&goal));
        assert!(path.iter().any(|p| p[1] > 140.0));
        assert_eq!(path, g.path(start, goal, 1.0).unwrap());
    }
    #[test]
    fn blocked_and_outside_goals_are_rejected_and_wide_units_need_clearance() {
        let mut g = Grid { width: 5, height: 5, blocked: vec![false; 25] };
        g.blocked[2] = true;
        assert!(g.path([5.0, 5.0], [25.0, 5.0], 1.0).is_none());
        assert!(g.path([5.0, 5.0], [100.0, 5.0], 1.0).is_none());
        assert!(g.passable(0, 0, 1.0));
        assert!(!g.passable(0, 0, 15.0));
    }
}
