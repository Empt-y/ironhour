//! Deterministic spatial buckets, the initial subset of `PartitionManager.cpp`.
//! Query results use object ID order regardless of which bucket is visited first.
use std::collections::BTreeMap;
#[derive(Clone, Default, Debug)]
pub struct Partition {
    pub buckets: BTreeMap<(i32, i32), Vec<u32>>,
}
impl Partition {
    pub fn rebuild(&mut self, objects: impl Iterator<Item = (u32, [f32; 3])>) {
        self.buckets.clear();
        for (id, p) in objects {
            self.buckets.entry(((p[0] / 64.0).floor() as i32, (p[1] / 64.0).floor() as i32)).or_default().push(id);
        }
        for ids in self.buckets.values_mut() {
            ids.sort_unstable();
        }
    }
    /// Physics commits positions immediately, so later modules in the same
    /// frame must see the new bucket rather than last frame's position.
    pub fn relocate(&mut self, id: u32, from: [f32; 3], to: [f32; 3]) {
        let key = |p: [f32; 3]| ((p[0] / 64.0).floor() as i32, (p[1] / 64.0).floor() as i32);
        let (old, new) = (key(from), key(to));
        if old == new {
            return;
        }
        if let Some(ids) = self.buckets.get_mut(&old) {
            if let Ok(i) = ids.binary_search(&id) {
                ids.remove(i);
            }
            if ids.is_empty() {
                self.buckets.remove(&old);
            }
        }
        let ids = self.buckets.entry(new).or_default();
        if let Err(i) = ids.binary_search(&id) {
            ids.insert(i, id);
        }
    }
    pub fn candidates(&self, p: [f32; 2], radius: f32) -> Vec<u32> {
        let mut ids = vec![];
        for y in ((p[1] - radius) / 64.0).floor() as i32..=((p[1] + radius) / 64.0).floor() as i32 {
            for x in ((p[0] - radius) / 64.0).floor() as i32..=((p[0] + radius) / 64.0).floor() as i32 {
                if let Some(v) = self.buckets.get(&(x, y)) {
                    ids.extend(v);
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
    #[test]
    fn relocation_updates_queries_within_a_tick_and_keeps_sorted_ids() {
        let mut p = Partition::default();
        p.rebuild([(3, [0.0; 3]), (1, [0.0; 3]), (2, [130.0, 0.0, 0.0])].into_iter());
        p.relocate(1, [0.0; 3], [130.0, 0.0, 0.0]);
        assert_eq!(p.candidates([0.0; 2], 1.0), [3]);
        assert_eq!(p.candidates([130.0, 0.0], 1.0), [1, 2]);
    }
}
