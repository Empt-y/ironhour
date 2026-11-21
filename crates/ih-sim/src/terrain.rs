//! Ground plus independent pristine bridge layers, following AIPathfind's layer
//! graph. Layer changes occur only at classified entry cells. Underpasses keep
//! their ground cells when the deck has at least ten units of vertical clearance.
use crate::{
    bridges::Bridge,
    footprint::Obstacles,
    navigation::{CELL, Grid},
    templates::Templates,
    zones::{NO_ZONE, ZoneCache, ZoneSummary},
};
use ih_formats::map::Map;
use std::{
    cmp::Reverse,
    collections::{BTreeMap, BinaryHeap},
    ops::Deref,
};

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Waypoint {
    pub position: [f32; 2],
    pub layer: u32,
}
impl Waypoint {
    pub fn ground(position: [f32; 2]) -> Self {
        Self { position, layer: 0 }
    }
}
#[derive(Clone)]
pub struct BridgeCell {
    pub entry: bool,
    index: usize,
}
impl BridgeCell {
    pub(crate) fn node(&self) -> usize {
        self.index
    }
}
#[derive(Clone)]
pub struct BridgeLayer {
    pub bridge: Bridge,
    pub cells: BTreeMap<(i32, i32), BridgeCell>,
    /// The layer's cell extent: cells inside it but off the deck are impassable.
    pub lo: (i32, i32),
    pub hi: (i32, i32),
}
#[derive(Clone)]
pub struct Terrain {
    pub ground: Grid,
    pub layers: Vec<BridgeLayer>,
    /// Obstacle cells under the map's structures (`footprint`).
    pub obstacles: Obstacles,
    nodes: Vec<(u32, i32, i32)>,
    zones: ZoneCache,
}
#[derive(Clone, Copy, Debug, Default)]
pub struct SearchStats {
    pub fine_expanded: usize,
    pub coarse_expanded: usize,
    pub component_rejected: bool,
    pub corridor_fallback: bool,
}
pub struct SearchReport {
    pub path: Option<Vec<Waypoint>>,
    pub stats: SearchStats,
}
impl Deref for Terrain {
    type Target = Grid;
    fn deref(&self) -> &Grid {
        &self.ground
    }
}
impl Terrain {
    pub fn from_map(map: &Map, templates: &Templates) -> Self {
        // Terrain and bridges first (`TheAI->pathfinder()->newMap`), then each map
        // object as it is created (`addObjectToPathfindMap`).
        let c = crate::navigation::classify_map(map);
        let mut t = Self::classified(c, templates.bridges.clone(), map);
        for p in crate::map_objects::placements(map, templates) {
            let template = &templates.objects[&p.template];
            let key = p.index as u32 + 1;
            if let Some(f) =
                crate::map_objects::footprint(template, [p.position[0], p.position[1]], p.angle, p.position[2], key)
            {
                t.obstacles.insert(&mut t.ground, &f);
            }
        }
        t
    }
    /// From a hand-made grid (blocked cells count as cliffs).
    pub fn new(ground: Grid, bridges: Vec<Bridge>, map: &Map) -> Self {
        let obstacles = Obstacles::new(&ground);
        Self::build(ground, obstacles, bridges, map)
    }
    /// From `classify_map`: the `GROUND` view, with cell types kept for other surfaces.
    pub fn classified(c: crate::navigation::Classified, bridges: Vec<Bridge>, map: &Map) -> Self {
        let ground = Grid {
            width: c.width,
            height: c.height,
            blocked: c.kind.iter().map(|k| !k.allows(crate::footprint::surface::GROUND)).collect(),
        };
        Self::build(ground, Obstacles::from_kinds(c.kind, c.pinched), bridges, map)
    }
    fn build(mut ground: Grid, mut obstacles: Obstacles, bridges: Vec<Bridge>, map: &Map) -> Self {
        let mut nodes: Vec<_> = (0..ground.height).flat_map(|y| (0..ground.width).map(move |x| (0, x, y))).collect();
        let mut layers = Vec::new();
        for bridge in bridges {
            let id = layers.len() as u32 + 1;
            let mut cells = BTreeMap::new();
            let (lo, hi) = bridge.bounds();
            let lo = [((lo[0] / CELL).floor() as i32 - 1).max(0), ((lo[1] / CELL).floor() as i32 - 1).max(0)];
            let hi = [
                ((hi[0] / CELL).ceil() as i32 + 1).min(ground.width - 1),
                ((hi[1] / CELL).ceil() as i32 + 1).min(ground.height - 1),
            ];
            for x in lo[0]..=hi[0] {
                for y in lo[1]..=hi[1] {
                    let (clear, entry, occupied) = bridge.classify(x, y);
                    let p = center(x, y);
                    if occupied && !entry && map.ground_height(p[0], p[1]) + 10.0 > bridge.height(p) {
                        let i = (y * ground.width + x) as usize;
                        ground.blocked[i] = true;
                        if obstacles.kind[i] == crate::footprint::CellType::Clear {
                            obstacles.kind[i] = crate::footprint::CellType::BridgeImpassable;
                        }
                    }
                    if clear {
                        cells.insert((x, y), BridgeCell { entry, index: nodes.len() });
                        nodes.push((id, x, y));
                    }
                }
            }
            layers.push(BridgeLayer { bridge, cells, lo: (lo[0], lo[1]), hi: (hi[0], hi[1]) });
        }
        Self { ground, layers, obstacles, nodes, zones: ZoneCache::default() }
    }
    /// The same terrain for a locomotor with these surfaces (`validMovementTerrain`):
    /// ground cells blocked by their cell type, with a fresh zone cache.
    pub fn view(&self, surfaces: u8) -> Self {
        let mut t = self.clone();
        t.ground.blocked = self.obstacles.blocked_for(surfaces);
        t.zones = ZoneCache::default();
        t
    }
    pub(crate) fn node_coordinates(&self) -> &[(u32, i32, i32)] {
        &self.nodes
    }
    /// Static terrain edits must invalidate the derived clearance/zone cache.
    /// Live traffic uses query filters and does not change these static zones.
    pub fn invalidate_zones(&mut self) {
        self.zones.clear();
    }
    /// `clientSafeQuickDoesPathExist`: both cells in the same connected zone component.
    pub fn quick_path_exists(&self, a: Waypoint, b: Waypoint, radius: f32) -> bool {
        let (ca, cb) = (cell(a.position), cell(b.position));
        let (Some(i), Some(j)) = (self.index(a.layer, ca.0, ca.1), self.index(b.layer, cb.0, cb.1)) else {
            return false;
        };
        i == j || self.zones.get(self, radius).connected(i, j)
    }
    /// A ground cell's type (`PathfindCell::getType`), if the cell exists.
    pub fn ground_kind(&self, x: i32, y: i32) -> Option<crate::footprint::CellType> {
        self.index(0, x, y).map(|i| self.obstacles.kind[i])
    }
    /// Whether the layer has this cell (`getCell` is non-null).
    pub fn has_cell(&self, layer: u32, x: i32, y: i32) -> bool {
        self.index(layer, x, y).is_some()
    }
    /// The zones of cells a unit's centre may enter (the pathfinder's zones ignore
    /// unit size).
    pub(crate) fn cell_zones(&self) -> std::sync::Arc<crate::zones::Zones> {
        self.zones.get(self, 0.0)
    }
    pub fn zone_summary(&self, radius: f32) -> ZoneSummary {
        self.zones.get(self, radius).summary()
    }
    pub(crate) fn zone_neighbours(&self, i: usize, radius: f32, passable: &[bool]) -> Vec<usize> {
        let (layer, x, y) = self.nodes[i];
        let mut out = Vec::with_capacity(6);
        for (dx, dy) in [(0, 1), (1, 0), (0, -1), (-1, 0)] {
            if let Some(n) = self.index(layer, x + dx, y + dy)
                && passable[n]
            {
                out.push(n);
            }
        }
        let here = Waypoint { position: center(x, y), layer };
        if layer == 0 {
            for l in 1..=self.layers.len() as u32 {
                if self.entry(l, x, y)
                    && self.can_transition(here, Waypoint { layer: l, ..here }, radius)
                    && let Some(n) = self.index(l, x, y)
                    && passable[n]
                {
                    out.push(n);
                }
            }
        } else if self.entry(layer, x, y)
            && self.can_transition(here, Waypoint { layer: 0, ..here }, radius)
            && let Some(n) = self.index(0, x, y)
            && passable[n]
        {
            out.push(n);
        }
        out
    }
    fn index(&self, layer: u32, x: i32, y: i32) -> Option<usize> {
        if layer == 0 {
            (x >= 0 && y >= 0 && x < self.width && y < self.height).then_some((y * self.width + x) as usize)
        } else {
            self.layers.get(layer as usize - 1)?.cells.get(&(x, y)).map(|c| c.index)
        }
    }
    /// `getLayerHeight` with its normal: the deck where it is above the ground.
    pub fn height_and_normal(&self, map: &Map, p: Waypoint) -> (f32, [f32; 3]) {
        let ground = (map.ground_height(p.position[0], p.position[1]), map.ground_normal(p.position[0], p.position[1]));
        if let Some(l) = p.layer.checked_sub(1).and_then(|i| self.layers.get(i as usize)) {
            let h = l.bridge.height(p.position);
            if h > ground.0 {
                return (h, l.bridge.normal());
            }
        }
        ground
    }
    pub fn height_at(&self, map: &Map, p: Waypoint) -> f32 {
        if p.layer == 0 {
            map.ground_height(p.position[0], p.position[1])
        } else {
            self.layers[p.layer as usize - 1].bridge.height(p.position)
        }
    }
    /// Two-coordinate orders and spawns select the highest deck at that point.
    /// `path` also accepts an explicit ground waypoint for underpass routes.
    pub fn destination(&self, map: &Map, position: [f32; 2]) -> Waypoint {
        let mut out = Waypoint::ground(position);
        let mut height = map.ground_height(position[0], position[1]);
        for (i, layer) in self.layers.iter().enumerate() {
            if layer.bridge.contains(position) && layer.bridge.height(position) > height {
                out.layer = i as u32 + 1;
                height = layer.bridge.height(position);
            }
        }
        out
    }
    pub fn passable_on(&self, layer: u32, x: i32, y: i32, radius: f32) -> bool {
        if layer == 0 {
            if self.ground.passable(x, y, radius) {
                return true;
            }
            if !self.ground.passable(x, y, 0.0) {
                return false;
            }
            let n = ((radius / CELL) - 0.5).ceil().max(0.0) as i32;
            // A footprint reaches the deck before its centre changes layer.
            // Only end portals may supply this extra ground clearance.
            for l in &self.layers {
                let near_entry =
                    (-n..=n).any(|dy| (-n..=n).any(|dx| l.cells.get(&(x + dx, y + dy)).is_some_and(|c| c.entry)));
                if near_entry
                    && (-n..=n).all(|dy| {
                        (-n..=n).all(|dx| {
                            self.ground.passable(x + dx, y + dy, 0.0) || l.cells.contains_key(&(x + dx, y + dy))
                        })
                    })
                {
                    return true;
                }
            }
            return false;
        }
        let Some(l) = self.layers.get(layer as usize - 1) else {
            return false;
        };
        if !l.cells.contains_key(&(x, y)) {
            return false;
        }
        let n = ((radius / CELL) - 0.5).ceil().max(0.0) as i32;
        for dy in -n..=n {
            for dx in -n..=n {
                if l.cells.contains_key(&(x + dx, y + dy)) {
                    continue;
                }
                // Footprints at an entry may straddle the bank, but never a rail.
                let p = center(x + dx, y + dy);
                let [along, across] = l.bridge.local(p);
                if across < l.bridge.min_y
                    || across > l.bridge.max_y
                    || (along >= 0.0 && along <= l.bridge.length())
                    || !self.ground.passable(x + dx, y + dy, 0.0)
                {
                    return false;
                }
            }
        }
        true
    }
    fn entry(&self, layer: u32, x: i32, y: i32) -> bool {
        layer != 0 && self.layers.get(layer as usize - 1).and_then(|l| l.cells.get(&(x, y))).is_some_and(|c| c.entry)
    }
    pub fn line_on(&self, a: Waypoint, b: Waypoint, radius: f32) -> bool {
        if a.layer != b.layer {
            return false;
        }
        if a.layer == 0 && self.layers.is_empty() {
            return self.ground.line_passable(a.position, b.position, radius);
        }
        let delta = [b.position[0] - a.position[0], b.position[1] - a.position[1]];
        let steps = (delta[0].abs().max(delta[1].abs()) / (CELL * 0.25)).ceil().max(1.0) as usize;
        let mut prev = cell(a.position);
        for i in 0..=steps {
            let t = i as f32 / steps as f32;
            let (x, y) = cell([a.position[0] + delta[0] * t, a.position[1] + delta[1] * t]);
            if !self.passable_on(a.layer, x, y, radius)
                || (x != prev.0
                    && y != prev.1
                    && (!self.passable_on(a.layer, x, prev.1, radius) || !self.passable_on(a.layer, prev.0, y, radius)))
            {
                return false;
            }
            prev = (x, y);
        }
        true
    }
    pub fn can_transition(&self, a: Waypoint, b: Waypoint, radius: f32) -> bool {
        let (x, y) = cell(a.position);
        cell(b.position) == (x, y)
            && (a.layer == 0 || b.layer == 0)
            && self.entry(a.layer.max(b.layer), x, y)
            && self.passable_on(a.layer, x, y, radius)
            && self.passable_on(b.layer, x, y, radius)
    }
    pub fn path(&self, start: Waypoint, goal: Waypoint, radius: f32) -> Option<Vec<Waypoint>> {
        self.path_report(start, goal, radius).path
    }
    pub fn path_report(&self, start: Waypoint, goal: Waypoint, radius: f32) -> SearchReport {
        let mut stats = SearchStats::default();
        let path = self.path_with_obstacles(start, goal, radius, (&|_| false, &|_, _| true), true, &mut stats);
        SearchReport { path, stats }
    }
    pub fn path_avoiding(
        &self,
        start: Waypoint,
        goal: Waypoint,
        radius: f32,
        blocked: &impl Fn(Waypoint) -> bool,
        segment_clear: &impl Fn(Waypoint, Waypoint) -> bool,
    ) -> Option<Vec<Waypoint>> {
        self.path_with_obstacles(start, goal, radius, (blocked, segment_clear), true, &mut SearchStats::default())
    }
    fn line_avoiding(
        &self,
        a: Waypoint,
        b: Waypoint,
        radius: f32,
        filters: (&impl Fn(Waypoint) -> bool, &impl Fn(Waypoint, Waypoint) -> bool),
    ) -> bool {
        let (blocked, segment_clear) = filters;
        if !self.line_on(a, b, radius) || !segment_clear(a, b) {
            return false;
        }
        let delta = [b.position[0] - a.position[0], b.position[1] - a.position[1]];
        let steps = (delta[0].abs().max(delta[1].abs()) / (CELL * 0.25)).ceil().max(1.0) as usize;
        (0..=steps).all(|i| {
            let t = i as f32 / steps as f32;
            !blocked(Waypoint {
                position: [a.position[0] + delta[0] * t, a.position[1] + delta[1] * t],
                layer: a.layer,
            })
        })
    }
    fn path_with_obstacles(
        &self,
        start: Waypoint,
        goal: Waypoint,
        radius: f32,
        filters: (&impl Fn(Waypoint) -> bool, &impl Fn(Waypoint, Waypoint) -> bool),
        hierarchical: bool,
        stats: &mut SearchStats,
    ) -> Option<Vec<Waypoint>> {
        let (blocked, _) = filters;
        if !start.position.iter().chain(&goal.position).all(|p| p.is_finite()) || !radius.is_finite() || radius < 0.0 {
            return None;
        }
        let (sx, sy) = cell(start.position);
        let (gx, gy) = cell(goal.position);
        let (s, g) = (self.index(start.layer, sx, sy)?, self.index(goal.layer, gx, gy)?);
        if !self.passable_on(start.layer, sx, sy, radius) || !self.passable_on(goal.layer, gx, gy, radius) {
            return None;
        }
        if blocked(goal) {
            return None;
        }
        if self.line_avoiding(start, goal, radius, filters) {
            return Some(vec![goal]);
        }
        let zones = hierarchical.then(|| self.zones.get(self, radius));
        let corridor = if let Some(zones) = &zones {
            if !zones.connected(s, g) {
                stats.component_rejected = true;
                return None;
            }
            let (allowed, expanded) = zones.corridor(s, g);
            stats.coarse_expanded = expanded;
            Some(allowed)
        } else {
            None
        };
        for attempt in 0..if hierarchical { 2 } else { 1 } {
            stats.corridor_fallback = attempt > 0;
            let mut costs = vec![u32::MAX; self.nodes.len()];
            let mut parent = vec![usize::MAX; self.nodes.len()];
            let mut closed = vec![false; self.nodes.len()];
            let mut open = BinaryHeap::new();
            let mut seq = 0u64;
            costs[s] = 0;
            open.push(Reverse((0u32, seq, s)));
            let dirs = [(0, 1), (1, 1), (1, 0), (1, -1), (0, -1), (-1, -1), (-1, 0), (-1, 1)];
            while let Some(Reverse((_, _, i))) = open.pop() {
                if closed[i] {
                    continue;
                }
                stats.fine_expanded += 1;
                if i == g {
                    return Some(self.build_path(start, goal, (s, g), &parent, radius, filters));
                }
                closed[i] = true;
                let (layer, x, y) = self.nodes[i];
                let mut neighbours = Vec::with_capacity(10);
                for (dx, dy) in dirs {
                    let (nx, ny) = (x + dx, y + dy);
                    let Some(n) = self.index(layer, nx, ny) else {
                        continue;
                    };
                    let clear = |x, y| {
                        self.passable_on(layer, x, y, radius) && !blocked(Waypoint { position: center(x, y), layer })
                    };
                    if !clear(nx, ny) || (dx != 0 && dy != 0 && (!clear(x + dx, y) || !clear(x, y + dy))) {
                        continue;
                    }
                    if i == s
                        && !self.line_avoiding(start, Waypoint { position: center(nx, ny), layer }, radius, filters)
                    {
                        continue;
                    }
                    let turn = if parent[i] != usize::MAX {
                        let (pl, px, py) = self.nodes[parent[i]];
                        if pl != layer || (x - px, y - py) == (dx, dy) {
                            0
                        } else {
                            match (x - px) * dx + (y - py) * dy {
                                1.. => 4,
                                0 => 8,
                                _ => 16,
                            }
                        }
                    } else {
                        0
                    };
                    neighbours.push((n, if dx == 0 || dy == 0 { 10 } else { 14 } + turn));
                }
                let here = Waypoint { position: center(x, y), layer };
                if layer == 0 {
                    for l in 1..=self.layers.len() as u32 {
                        let there = Waypoint { layer: l, ..here };
                        if self.can_transition(here, there, radius) && !blocked(there) {
                            neighbours.push((self.index(l, x, y).unwrap(), 0));
                        }
                    }
                } else if self.can_transition(here, Waypoint { layer: 0, ..here }, radius)
                    && !blocked(Waypoint { layer: 0, ..here })
                {
                    neighbours.push((self.index(0, x, y).unwrap(), 0));
                }
                for (n, cost) in neighbours {
                    if attempt == 0
                        && let (Some(zones), Some(allowed)) = (&zones, &corridor)
                    {
                        let zone = zones.cells[n];
                        if zone == NO_ZONE || !allowed[zone as usize] {
                            continue;
                        }
                    }
                    let cost = costs[i] + cost;
                    if !closed[n] && cost < costs[n] {
                        parent[n] = i;
                        costs[n] = cost;
                        let (_, nx, ny) = self.nodes[n];
                        let (dx, dy) = ((gx - nx).unsigned_abs(), (gy - ny).unsigned_abs());
                        seq += 1;
                        open.push(Reverse((cost + 10 * dx.max(dy) + 5 * dx.min(dy), seq, n)));
                    }
                }
            }
        }
        None
    }
    fn build_path(
        &self,
        start: Waypoint,
        goal: Waypoint,
        ends: (usize, usize),
        parent: &[usize],
        radius: f32,
        filters: (&impl Fn(Waypoint) -> bool, &impl Fn(Waypoint, Waypoint) -> bool),
    ) -> Vec<Waypoint> {
        let (s, g) = ends;
        let mut raw = vec![goal];
        let mut i = g;
        while parent[i] != usize::MAX {
            i = parent[i];
            if i != s {
                let (layer, x, y) = self.nodes[i];
                raw.push(Waypoint { layer, position: center(x, y) });
            }
        }
        raw.reverse();
        let mut path = Vec::new();
        let mut at = start;
        let mut next = 0;
        while next < raw.len() {
            let mut far = next;
            if at.layer == raw[next].layer {
                while far + 1 < raw.len()
                    && raw[far + 1].layer == at.layer
                    && self.line_avoiding(at, raw[far + 1], radius, filters)
                {
                    far += 1;
                }
            }
            path.push(raw[far]);
            at = raw[far];
            next = far + 1;
        }
        path
    }
}
fn cell(p: [f32; 2]) -> (i32, i32) {
    ((p[0] / CELL).floor() as i32, (p[1] / CELL).floor() as i32)
}
fn center(x: i32, y: i32) -> [f32; 2] {
    [(x as f32 + 0.5) * CELL, (y as f32 + 0.5) * CELL]
}

#[cfg(test)]
mod tests {
    use super::*;
    fn grid_terrain(width: i32, height: i32) -> Terrain {
        Terrain::new(Grid { width, height, blocked: vec![false; (width * height) as usize] }, vec![], &Map::default())
    }
    #[test]
    fn disconnected_zones_reject_without_fine_search() {
        let mut t = grid_terrain(100, 100);
        for y in 0..100 {
            t.ground.blocked[y * 100 + 50] = true;
        }
        let report = t.path_report(Waypoint::ground([55.0, 55.0]), Waypoint::ground([955.0, 55.0]), 1.0);
        assert!(report.path.is_none());
        assert!(report.stats.component_rejected);
        assert_eq!(report.stats.fine_expanded, 0);
        assert_eq!(t.zone_summary(1.0).components, 2);
    }
    #[test]
    fn coarse_corridor_reduces_fine_work_on_large_detour() {
        let mut t = grid_terrain(100, 100);
        for y in 0..100 {
            if y != 85 {
                t.ground.blocked[y * 100 + 50] = true;
            }
        }
        let (start, goal) = (Waypoint::ground([55.0, 55.0]), Waypoint::ground([955.0, 55.0]));
        let report = t.path_report(start, goal, 1.0);
        let mut baseline = SearchStats::default();
        let plain = t.path_with_obstacles(start, goal, 1.0, (&|_| false, &|_, _| true), false, &mut baseline);
        assert!(report.path.is_some() && plain.is_some());
        assert!(report.stats.coarse_expanded > 0);
        assert!(
            report.stats.fine_expanded < baseline.fine_expanded,
            "hierarchy {:?}; plain {:?}",
            report.stats,
            baseline
        );
        let mut at = start;
        for p in report.path.unwrap() {
            assert!(t.line_on(at, p, 1.0));
            at = p;
        }
    }
    #[test]
    fn traffic_blocked_corridor_falls_back_to_another_static_route() {
        let mut t = grid_terrain(80, 60);
        for y in 0..60 {
            if y != 10 && y != 45 {
                t.ground.blocked[y * 80 + 40] = true;
            }
        }
        let (start, goal) = (Waypoint::ground([55.0, 105.0]), Waypoint::ground([755.0, 105.0]));
        let blocked = |p: Waypoint| p.position[0] > 440.0 && p.position[0] < 460.0 && p.position[1] < 350.0;
        let mut stats = SearchStats::default();
        let path = t.path_with_obstacles(start, goal, 1.0, (&blocked, &|_, _| true), true, &mut stats).unwrap();
        assert!(stats.corridor_fallback, "{stats:?}");
        assert!(path.iter().any(|p| p.position[1] > 350.0));
    }
    #[test]
    fn clearance_classes_have_independent_connectivity() {
        let mut t = grid_terrain(50, 50);
        for y in 0..50 {
            if y != 25 {
                t.ground.blocked[y * 50 + 25] = true;
            }
        }
        let (start, goal) = (Waypoint::ground([25.0, 255.0]), Waypoint::ground([475.0, 255.0]));
        assert!(t.path(start, goal, 1.0).is_some());
        assert_eq!(t.zone_summary(1.0).components, 1);
        let wide = t.path_report(start, goal, 15.0);
        assert!(wide.path.is_none() && wide.stats.component_rejected);
        assert_eq!(t.zone_summary(15.0).components, 2);
        assert!(t.path(start, goal, 1.0).is_some());
    }
    #[test]
    fn terrain_clone_and_invalidation_do_not_reuse_edited_zones() {
        let mut a = grid_terrain(30, 30);
        assert_eq!(a.zone_summary(1.0).components, 1);
        let mut b = a.clone();
        for y in 0..30 {
            b.ground.blocked[y * 30 + 15] = true;
        }
        assert_eq!(b.zone_summary(1.0).components, 2);
        assert_eq!(a.zone_summary(1.0).components, 1);
        a.ground.blocked = b.ground.blocked.clone();
        a.invalidate_zones();
        assert_eq!(a.zone_summary(1.0).components, 2);
    }
    #[test]
    fn seeded_grids_keep_unrestricted_reachability() {
        let mut rng = crate::rng::LogicRng::new(37);
        for _ in 0..12 {
            let mut t = grid_terrain(24, 24);
            for b in &mut t.ground.blocked {
                *b = rng.next_u32() % 100 < 28;
            }
            for _ in 0..40 {
                let (a, b) = ((rng.next_u32() % 576) as usize, (rng.next_u32() % 576) as usize);
                if t.ground.blocked[a] || t.ground.blocked[b] {
                    continue;
                }
                let p = |i: usize| [(i % 24) as f32 * CELL + 5.0, (i / 24) as f32 * CELL + 5.0];
                let expected = t.ground.path(p(a), p(b), 1.0).is_some();
                let actual = t.path(Waypoint::ground(p(a)), Waypoint::ground(p(b)), 1.0).is_some();
                assert_eq!(actual, expected, "{a}->{b}");
            }
        }
    }
    fn terrain(height: f32) -> Terrain {
        let mut g = Grid { width: 30, height: 30, blocked: vec![false; 900] };
        for y in 0..30 {
            for x in 12..18 {
                g.blocked[y * 30 + x] = true;
            }
        }
        let bridge = Bridge {
            name: "test".into(),
            from: [105.0, 150.0, height],
            to: [195.0, 150.0, height],
            min_y: -30.0,
            max_y: 30.0,
        };
        Terrain::new(g, vec![bridge], &Map::default())
    }
    #[test]
    fn route_crosses_blocked_water_only_through_bridge_ends() {
        let t = terrain(20.0);
        let start = Waypoint::ground([65.0, 155.0]);
        let goal = Waypoint::ground([235.0, 155.0]);
        assert!(t.ground.path(start.position, goal.position, 1.0).is_none());
        let path = t.path(start, goal, 1.0).unwrap();
        assert!(path.iter().any(|p| p.layer == 1));
        let mut at = start;
        for p in &path {
            assert!(t.line_on(at, *p, 1.0) || t.can_transition(at, *p, 1.0), "{at:?} -> {p:?}");
            at = *p;
        }
        assert_eq!(path.last(), Some(&goal));
        assert_eq!(path, t.path(start, goal, 1.0).unwrap());
        let vehicle = t.path(start, goal, 15.0).unwrap();
        assert!(vehicle.iter().any(|p| p.layer == 1));
        assert!(!t.can_transition(
            Waypoint::ground([155.0, 155.0]),
            Waypoint { position: [155.0, 155.0], layer: 1 },
            1.0
        ));
        assert!(!t.passable_on(1, 15, 12, 25.0));
    }
    #[test]
    fn underpass_is_independent_and_low_deck_blocks_ground() {
        let mut t = terrain(20.0);
        t.ground.blocked.fill(false);
        let a = Waypoint::ground([155.0, 55.0]);
        let b = Waypoint::ground([155.0, 245.0]);
        assert!(t.path(a, b, 1.0).unwrap().iter().all(|p| p.layer == 0));
        let low = terrain(5.0);
        assert!(low.ground.blocked[15 * 30 + 11]);
        assert!(!terrain(20.0).ground.blocked[15 * 30 + 11]);
    }
}
