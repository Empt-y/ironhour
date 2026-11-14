//! Sleepy update heap from `GameLogic.cpp`: packed frame/phase priority, strict
//! comparisons, left-child ties, in-place rescheduling and index maintenance.
//! Equal priorities deliberately do not gain an object-id/FIFO tiebreaker.
use std::collections::BTreeMap;
pub const FOREVER: u32 = 0x3fff_ffff;
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[repr(u32)]
pub enum Phase {
    Initial = 0,
    Physics = 1,
    Normal = 2,
    Final = 3,
}
pub type Key = (u32, usize);
#[derive(Clone, Debug)]
pub struct Entry {
    pub key: Key,
    pub frame: u32,
    pub phase: Phase,
}
impl Entry {
    fn priority(&self) -> u64 {
        (u64::from(self.frame) << 2) | self.phase as u64
    }
}
#[derive(Clone, Default, Debug)]
pub struct Scheduler {
    pub heap: Vec<Entry>,
    indices: BTreeMap<Key, usize>,
}
impl Scheduler {
    fn swap(&mut self, a: usize, b: usize) {
        self.heap.swap(a, b);
        self.indices.insert(self.heap[a].key, a);
        self.indices.insert(self.heap[b].key, b);
    }
    fn up(&mut self, mut i: usize) -> usize {
        while i > 0 {
            let p = (i - 1) / 2;
            if self.heap[p].priority() <= self.heap[i].priority() {
                break;
            }
            self.swap(i, p);
            i = p;
        }
        i
    }
    fn down(&mut self, mut i: usize) {
        loop {
            let mut c = i * 2 + 1;
            if c >= self.heap.len() {
                break;
            }
            if c + 1 < self.heap.len() && self.heap[c].priority() > self.heap[c + 1].priority() {
                c += 1;
            }
            if self.heap[i].priority() <= self.heap[c].priority() {
                break;
            }
            self.swap(i, c);
            i = c;
        }
    }
    pub fn insert(&mut self, key: Key, frame: u32, phase: Phase) {
        assert!(!self.indices.contains_key(&key));
        let i = self.heap.len();
        self.heap.push(Entry { key, frame, phase });
        self.indices.insert(key, i);
        self.up(i);
    }
    pub fn due(&self, frame: u32) -> Option<Key> {
        self.heap.first().filter(|e| e.frame <= frame).map(|e| e.key)
    }
    pub fn reschedule(&mut self, key: Key, frame: u32) {
        let Some(&i) = self.indices.get(&key) else { return };
        self.heap[i].frame = frame;
        let i = self.up(i);
        self.down(i);
    }
    /// `friend_awakenUpdateModule` preserves a module already due this frame.
    pub fn awaken(&mut self, key: Key, now: u32, delay: u32) {
        let Some(&i) = self.indices.get(&key) else { return };
        let frame = now.saturating_add(delay.max(1));
        if self.heap[i].frame == frame || (now > 0 && self.heap[i].frame == now && frame == now + 1) {
            return;
        }
        self.reschedule(key, frame);
    }
    pub fn remove(&mut self, key: Key) {
        let Some(i) = self.indices.remove(&key) else { return };
        self.heap.swap_remove(i);
        if i < self.heap.len() {
            self.indices.insert(self.heap[i].key, i);
            let i = self.up(i);
            self.down(i);
        }
    }
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn phases_and_heap_ties_match_source_not_fifo() {
        let mut s = Scheduler::default();
        for i in 0..4 {
            s.insert((i, 0), 1, Phase::Normal);
        }
        let mut got = vec![];
        while let Some(k) = s.due(1) {
            got.push(k.0);
            s.reschedule(k, 2);
        }
        assert_eq!(got, [0, 1, 3, 2]);
        s.insert((9, 0), 2, Phase::Initial);
        assert_eq!(s.due(2), Some((9, 0)));
        let mut s = Scheduler::default();
        for (id, phase) in [(1, Phase::Final), (2, Phase::Normal), (3, Phase::Physics), (4, Phase::Initial)] {
            s.insert((id, 0), 10, phase);
        }
        let mut got = vec![];
        while let Some(k) = s.due(10) {
            got.push(k.0);
            s.reschedule(k, 11);
        }
        assert_eq!(got, [4, 3, 2, 1]);
    }
    #[test]
    fn waking_due_module_keeps_its_turn_and_removal_keeps_indices_valid() {
        let mut s = Scheduler::default();
        s.insert((1, 0), 3, Phase::Initial);
        s.insert((2, 0), 5, Phase::Physics);
        s.awaken((1, 0), 3, 1);
        assert_eq!(s.due(3), Some((1, 0)));
        s.remove((1, 0));
        s.awaken((2, 0), 3, 1);
        assert_eq!(s.due(3), None);
        assert_eq!(s.due(4), Some((2, 0)));
    }
}
