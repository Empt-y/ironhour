//! First ground navigation layer from `AIPathfind.cpp`: 10-unit cells,
//! orthogonal/diagonal costs (10/14), turn penalties (4/8/16), stable open-list
//! ties and line-of-sight path optimisation. `terrain` adds bridge layers and
//! transient queries; `zones` adds the initial ground hierarchy. Crushing and
//! multi-surface terrain classification remain pending.
use ih_formats::map::{MAP_HEIGHT_SCALE, Map};
use std::{cmp::Reverse, collections::BinaryHeap};
pub const CELL: f32 = 10.0;

#[derive(Clone)]
pub struct Grid {
    pub width: i32,
    pub height: i32,
    pub blocked: Vec<bool>,
}
impl Grid {
    /// Terrain cells only; structures are added by `Terrain::from_map` (`footprint`).
    pub fn from_map(map: &Map) -> Self {
        let bounds =
            map.boundaries.first().copied().unwrap_or([map.width - 2 * map.border, map.height - 2 * map.border]);
        let (w, h) = (bounds[0].max(1), bounds[1].max(1));
        let mut out = Self { width: w, height: h, blocked: vec![false; (w * h) as usize] };
        for y in 0..h {
            for x in 0..w {
                let (mx, my) = (x + map.border, y + map.border);
                let heights = [
                    map.height_at(mx, my),
                    map.height_at(mx + 1, my),
                    map.height_at(mx + 1, my + 1),
                    map.height_at(mx, my + 1),
                ];
                let slope = f32::from(heights.iter().max().unwrap() - heights.iter().min().unwrap()) * MAP_HEIGHT_SCALE;
                let stored_cliff = map.tiles.as_ref().is_some_and(|t| {
                    let row = ((map.width + 7) / 8) as usize;
                    t.cliff_state
                        .get(my as usize * row + (mx as usize >> 3))
                        .is_some_and(|bits| bits & (1 << (mx & 7)) != 0)
                });
                let p = [(x as f32 + 0.5) * CELL, (y as f32 + 0.5) * CELL];
                let wet = map.triggers.iter().filter(|t| t.is_water && t.points.len() > 2).any(|t| {
                    let below = t.points.iter().map(|p| p[2]).min().unwrap() as f32 > map.ground_height(p[0], p[1]);
                    below && inside(p, &t.points)
                });
                out.blocked[(y * w + x) as usize] = slope > 9.8 || stored_cliff || wet;
            }
        }
        out
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
fn inside(p: [f32; 2], points: &[[i32; 3]]) -> bool {
    let mut inside = false;
    let mut j = points.len() - 1;
    for i in 0..points.len() {
        let (a, b) = (points[i], points[j]);
        if (a[1] as f32 > p[1]) != (b[1] as f32 > p[1])
            && p[0] < (b[0] - a[0]) as f32 * (p[1] - a[1] as f32) / (b[1] - a[1]) as f32 + a[0] as f32
        {
            inside = !inside;
        }
        j = i;
    }
    inside
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
