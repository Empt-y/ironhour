//! Initial ground hierarchy from AIPathfind's ZoneBlock/PathfindZoneManager:
//! 10x10 cell blocks, local connected regions, global equivalence and a coarse
//! route before fine search. Bridge components connect through end portals.
//! Surface-combination/crusher tables and incremental terrain edits are pending.
use crate::terrain::Terrain;
use std::{
    cmp::Reverse,
    collections::{BTreeMap, BTreeSet, BinaryHeap, VecDeque},
    sync::{Arc, RwLock},
};
pub const BLOCK_SIZE: i32 = 10;
pub const NO_ZONE: u32 = u32::MAX;

#[derive(Clone, Copy, Debug, Default)]
pub struct ZoneSummary {
    pub zones: usize,
    pub connections: usize,
    pub components: usize,
}
#[derive(Default)]
pub(crate) struct ZoneCache(RwLock<BTreeMap<i32, Arc<Zones>>>);
impl Clone for ZoneCache {
    // A terrain clone may subsequently be edited independently.
    fn clone(&self) -> Self {
        Self::default()
    }
}
impl ZoneCache {
    pub fn get(&self, terrain: &Terrain, radius: f32) -> Arc<Zones> {
        let clearance = ((radius / 10.0) - 0.5).ceil().max(0.0) as i32;
        if let Some(zones) = self.0.read().expect("zone cache poisoned").get(&clearance) {
            return zones.clone();
        }
        let zones = Arc::new(Zones::build(terrain, radius));
        self.0.write().expect("zone cache poisoned").entry(clearance).or_insert(zones).clone()
    }
    pub fn clear(&mut self) {
        self.0.get_mut().expect("zone cache poisoned").clear();
    }
}

pub(crate) struct Zones {
    pub cells: Vec<u32>,
    edges: Vec<Vec<u32>>,
    centres: Vec<(i32, i32)>,
    components: Vec<u32>,
    summary: ZoneSummary,
}
impl Zones {
    fn build(terrain: &Terrain, radius: f32) -> Self {
        let nodes = terrain.node_coordinates();
        let passable: Vec<_> = nodes.iter().map(|(layer, x, y)| terrain.passable_on(*layer, *x, *y, radius)).collect();
        let mut cells = vec![NO_ZONE; nodes.len()];
        let mut centres = Vec::new();
        let mut queue = VecDeque::new();
        let block = |i: usize| {
            let (layer, x, y) = nodes[i];
            (layer, if layer == 0 { x / BLOCK_SIZE } else { 0 }, if layer == 0 { y / BLOCK_SIZE } else { 0 })
        };
        for seed in 0..nodes.len() {
            if !passable[seed] || cells[seed] != NO_ZONE {
                continue;
            }
            let zone = centres.len() as u32;
            cells[seed] = zone;
            queue.push_back(seed);
            let (mut sx, mut sy, mut count) = (0i64, 0i64, 0i64);
            while let Some(i) = queue.pop_front() {
                sx += i64::from(nodes[i].1);
                sy += i64::from(nodes[i].2);
                count += 1;
                for n in terrain.zone_neighbours(i, radius, &passable) {
                    if cells[n] == NO_ZONE && block(n) == block(seed) {
                        cells[n] = zone;
                        queue.push_back(n);
                    }
                }
            }
            centres.push(((sx / count) as i32, (sy / count) as i32));
        }
        let mut edges = vec![BTreeSet::new(); centres.len()];
        for i in 0..nodes.len() {
            if cells[i] == NO_ZONE {
                continue;
            }
            for n in terrain.zone_neighbours(i, radius, &passable) {
                if cells[n] != cells[i] {
                    edges[cells[i] as usize].insert(cells[n]);
                }
            }
        }
        let edges: Vec<Vec<u32>> = edges.into_iter().map(|e| e.into_iter().collect()).collect();
        let mut components = vec![NO_ZONE; centres.len()];
        let mut count = 0;
        for seed in 0..centres.len() {
            if components[seed] != NO_ZONE {
                continue;
            }
            components[seed] = count;
            queue.push_back(seed);
            while let Some(i) = queue.pop_front() {
                for n in &edges[i] {
                    let n = *n as usize;
                    if components[n] == NO_ZONE {
                        components[n] = count;
                        queue.push_back(n);
                    }
                }
            }
            count += 1;
        }
        let summary = ZoneSummary {
            zones: centres.len(),
            connections: edges.iter().map(Vec::len).sum::<usize>() / 2,
            components: count as usize,
        };
        Self { cells, edges, centres, components, summary }
    }
    pub fn summary(&self) -> ZoneSummary {
        self.summary
    }
    pub fn connected(&self, a: usize, b: usize) -> bool {
        let (a, b) = (self.cells[a], self.cells[b]);
        a != NO_ZONE && b != NO_ZONE && self.components[a as usize] == self.components[b as usize]
    }
    /// Coarse shortest route plus one adjacent zone of slack. If transient
    /// traffic blocks this corridor, fine search falls back to the component.
    pub fn corridor(&self, a: usize, b: usize) -> (Vec<bool>, usize) {
        let (a, b) = (self.cells[a] as usize, self.cells[b] as usize);
        let mut costs = vec![u32::MAX; self.edges.len()];
        let mut parent = vec![usize::MAX; self.edges.len()];
        let mut closed = vec![false; self.edges.len()];
        let mut open = BinaryHeap::new();
        let mut sequence = 0u64;
        costs[a] = 0;
        open.push(Reverse((0u32, sequence, a)));
        let mut expanded = 0;
        while let Some(Reverse((_, _, i))) = open.pop() {
            if closed[i] {
                continue;
            }
            closed[i] = true;
            expanded += 1;
            if i == b {
                break;
            }
            for n in &self.edges[i] {
                let n = *n as usize;
                let cost = costs[i] + distance(self.centres[i], self.centres[n]).max(1);
                if !closed[n] && cost < costs[n] {
                    costs[n] = cost;
                    parent[n] = i;
                    sequence += 1;
                    open.push(Reverse((cost + distance(self.centres[n], self.centres[b]), sequence, n)));
                }
            }
        }
        let mut allowed = vec![false; self.edges.len()];
        let mut i = b;
        loop {
            allowed[i] = true;
            for n in &self.edges[i] {
                allowed[*n as usize] = true;
            }
            if i == a {
                break;
            }
            i = parent[i];
            assert_ne!(i, usize::MAX, "connected zone graph has no coarse path");
        }
        (allowed, expanded)
    }
}
fn distance(a: (i32, i32), b: (i32, i32)) -> u32 {
    let (dx, dy) = ((a.0 - b.0).unsigned_abs(), (a.1 - b.1).unsigned_abs());
    10 * dx.max(dy) + 4 * dx.min(dy)
}
