//! Deterministic spatial buckets, the initial subset of `PartitionManager.cpp`.
//! Query results use object ID order regardless of which bucket is visited first.
//!
//! Mobile objects are bucketed by their centre and found by widening queries by
//! the largest mobile radius. Immobile objects (structures, trees, Contra's
//! map-sized water planes) are registered in every bucket their bounding circle
//! covers, as the original registers objects in each partition cell they touch,
//! so they never widen the queries.
use std::collections::BTreeMap;
pub const BUCKET: f32 = 64.0;

#[derive(Clone, Copy, Debug)]
pub struct Entry {
    pub id: u32,
    pub position: [f32; 3],
    pub radius: f32,
    pub immobile: bool,
}

#[derive(Clone, Default, Debug)]
pub struct Partition {
    pub buckets: BTreeMap<(i32, i32), Vec<u32>>,
    /// Immobile objects, in every bucket their bounding circle covers.
    pub fixed: BTreeMap<(i32, i32), Vec<u32>>,
}
fn key(p: [f32; 2]) -> (i32, i32) {
    ((p[0] / BUCKET).floor() as i32, (p[1] / BUCKET).floor() as i32)
}
impl Partition {
    /// `extent` is the area worth registering immobile objects in (the map).
    pub fn rebuild(&mut self, objects: impl Iterator<Item = Entry>, extent: ([f32; 2], [f32; 2])) {
        self.buckets.clear();
        self.fixed.clear();
        let (lo_k, hi_k) = (key(extent.0), key(extent.1));
        for e in objects {
            if e.immobile {
                let lo = key([e.position[0] - e.radius, e.position[1] - e.radius]);
                let hi = key([e.position[0] + e.radius, e.position[1] + e.radius]);
                for y in lo.1.max(lo_k.1)..=hi.1.min(hi_k.1) {
                    for x in lo.0.max(lo_k.0)..=hi.0.min(hi_k.0) {
                        self.fixed.entry((x, y)).or_default().push(e.id);
                    }
                }
                // Outside the extent: still findable at its centre.
                let c = key([e.position[0], e.position[1]]);
                if c.0 < lo_k.0 || c.1 < lo_k.1 || c.0 > hi_k.0 || c.1 > hi_k.1 {
                    self.fixed.entry(c).or_default().push(e.id);
                }
            } else {
                self.buckets.entry(key([e.position[0], e.position[1]])).or_default().push(e.id);
            }
        }
        for ids in self.buckets.values_mut().chain(self.fixed.values_mut()) {
            ids.sort_unstable();
        }
    }
    /// Physics commits positions immediately, so later modules in the same
    /// frame must see the new bucket rather than last frame's position.
    /// (Immobile objects pushed by a collision are re-registered at the next rebuild.)
    pub fn relocate(&mut self, id: u32, from: [f32; 3], to: [f32; 3]) {
        let (old, new) = (key([from[0], from[1]]), key([to[0], to[1]]));
        if old == new {
            return;
        }
        let Some(ids) = self.buckets.get_mut(&old) else { return };
        let Ok(i) = ids.binary_search(&id) else { return };
        ids.remove(i);
        if ids.is_empty() {
            self.buckets.remove(&old);
        }
        let ids = self.buckets.entry(new).or_default();
        if let Err(i) = ids.binary_search(&id) {
            ids.insert(i, id);
        }
    }
    /// Mobile objects whose centre lies within `mobile_radius` of `p` (by bucket),
    /// and immobile objects whose bounding circle reaches within `radius`.
    pub fn candidates(&self, p: [f32; 2], radius: f32, mobile_radius: f32) -> Vec<u32> {
        let mut ids = vec![];
        for (map, r) in [(&self.buckets, mobile_radius), (&self.fixed, radius)] {
            let (lo, hi) = (key([p[0] - r, p[1] - r]), key([p[0] + r, p[1] + r]));
            for y in lo.1..=hi.1 {
                for x in lo.0..=hi.0 {
                    if let Some(v) = map.get(&(x, y)) {
                        ids.extend(v);
                    }
                }
            }
        }
        ids.sort_unstable();
        ids.dedup();
        ids
    }
}
#[cfg(test)]
mod tests {
    use super::*;
    fn e(id: u32, x: f32, radius: f32, immobile: bool) -> Entry {
        Entry { id, position: [x, 0.0, 0.0], radius, immobile }
    }
    const MAP: ([f32; 2], [f32; 2]) = ([-1000.0, -1000.0], [1000.0, 1000.0]);
    #[test]
    fn relocation_updates_queries_within_a_tick_and_keeps_sorted_ids() {
        let mut p = Partition::default();
        p.rebuild([e(3, 0.0, 1.0, false), e(1, 0.0, 1.0, false), e(2, 130.0, 1.0, false)].into_iter(), MAP);
        p.relocate(1, [0.0; 3], [130.0, 0.0, 0.0]);
        assert_eq!(p.candidates([0.0; 2], 1.0, 1.0), [3]);
        assert_eq!(p.candidates([130.0, 0.0], 1.0, 1.0), [1, 2]);
    }
    #[test]
    fn large_immobile_objects_are_found_without_widening_queries() {
        let mut p = Partition::default();
        p.rebuild([e(1, 0.0, 5000.0, true), e(2, 300.0, 10.0, true), e(3, 500.0, 2.0, false)].into_iter(), MAP);
        // Far from the plane's centre but inside its circle; the clamp keeps it small.
        assert_eq!(p.candidates([900.0, 0.0], 1.0, 1.0), [1]);
        assert!(p.fixed.len() <= 32 * 32);
        assert_eq!(p.candidates([315.0, 0.0], 6.0, 6.0), [1, 2]);
        assert_eq!(p.candidates([500.0, 0.0], 1.0, 1.0), [1, 3]);
    }
}
