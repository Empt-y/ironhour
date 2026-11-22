//! Shroud and fog of war, the logic side.
//!
//! Sources: `PartitionCell::addLooker`, `removeLooker` and
//! `getShroudStatusForPlayer`, `PartitionManager::doShroudReveal`,
//! `undoShroudReveal`, `queueUndoShroudReveal` and
//! `processPendingUndoShroudRevealQueue` (`PartitionManager.cpp`),
//! `DiscreteCircle.cpp`, `PartitionData::doCircleFill`, `doSmallFill` and
//! `getShroudedStatus`, `Object::look`, `unlook`, `handleShroud`,
//! `getShroudClearingRange` and `getShroudedStatus`, `PartitionFilterFreeOfFog`.
//!
//! Every player has a level per partition cell: 1 shrouded (never seen),
//! 0 fogged (seen before, nobody looking), below 0 clear (the count of
//! lookers). An object looks again when it enters a new cell, changes owner,
//! enters or leaves a container, dies, finishes construction or changes its
//! stealth status. The old look is undone `UnlookPersistDuration` later, so
//! the fog closes in behind a moving unit.
//!
//! Not ported yet: shrouders (`addShrouder`, GPS scramblers), map-wide
//! reveals and observers (scripts), vision spied through other players,
//! box geometry (covered as a circle of the bounding radius), the threat
//! and value maps, and the client side (drawing shroud, ghost objects).
use crate::Simulation;
use std::collections::{BTreeMap, VecDeque};

/// `CellShroudStatus` / `ObjectShroudStatus`.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub enum Visibility {
    Clear,
    Fogged,
    Shrouded,
}

/// A look to undo later (`SightingInfo`).
#[derive(Clone, Debug, PartialEq)]
pub struct Look {
    pub position: [f32; 2],
    pub radius: f32,
    pub players: Vec<u32>,
}

/// What makes an object look again when it changes.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct LookKey {
    pub cell: Option<(i32, i32)>,
    pub owner: u32,
    pub status: u64,
    pub contained_by: Option<u32>,
    pub dead: bool,
}

/// The shroud grids of every player.
#[derive(Clone, Debug, Default)]
pub struct Shroud {
    pub cell: f32,
    pub cols: i32,
    pub rows: i32,
    /// Per player: (`m_currentShroud`, `m_activeShroudLevel`) per cell.
    pub grids: BTreeMap<u32, Vec<(i16, i16)>>,
    /// `m_pendingUndoShroudReveals`: (frame queued for, look).
    pub pending: VecDeque<(u32, Look)>,
}

/// `DiscreteCircle`: its scanlines (x start, x end, y), mirrored rows included.
pub fn discrete_circle(cx: i32, cy: i32, radius: i32) -> Vec<(i32, i32, i32)> {
    let mut edges: Vec<(i32, i32, i32)> = vec![];
    let (mut x, mut y) = (0, radius);
    let mut d = (1 - radius) << 1;
    while y >= 0 {
        edges.push((cx - x, cx + x, cy + y));
        if d + y > 0 {
            y -= 1;
            d -= (y << 1) - 1;
        }
        if x > d {
            x += 1;
            d += (x << 1) + 1;
        }
    }
    // removeDuplicates: of consecutive edges on one row, the last stays.
    let mut kept: Vec<(i32, i32, i32)> = vec![];
    for (i, e) in edges.iter().enumerate() {
        if edges.get(i + 1).is_some_and(|n| n.2 == e.2) {
            continue;
        }
        kept.push(*e);
    }
    let mut out = vec![];
    for e in kept {
        out.push(e);
        if e.2 != cy {
            out.push((e.0, e.1, 2 * cy - e.2));
        }
    }
    out
}

fn status_of(level: i16) -> Visibility {
    match level {
        1 => Visibility::Shrouded,
        0 => Visibility::Fogged,
        _ => Visibility::Clear,
    }
}

impl Shroud {
    pub fn new(cell: f32, world: [f32; 2]) -> Self {
        let cell = if cell > 0.0 { cell } else { 40.0 };
        Self {
            cell,
            cols: (world[0] / cell).ceil().max(1.0) as i32,
            rows: (world[1] / cell).ceil().max(1.0) as i32,
            grids: BTreeMap::new(),
            pending: VecDeque::new(),
        }
    }
    /// `worldToCell`.
    pub fn world_to_cell(&self, p: [f32; 2]) -> (i32, i32) {
        ((p[0] / self.cell).floor() as i32, (p[1] / self.cell).floor() as i32)
    }
    fn index(&self, x: i32, y: i32) -> Option<usize> {
        (x >= 0 && y >= 0 && x < self.cols && y < self.rows).then(|| (y * self.cols + x) as usize)
    }
    fn grid(&mut self, player: u32) -> &mut Vec<(i16, i16)> {
        let n = (self.cols * self.rows) as usize;
        // "Passive shroud" to begin with: (1, 0).
        self.grids.entry(player).or_insert_with(|| vec![(1, 0); n])
    }
    /// `getShroudStatusForPlayer(player, x, y)`: shrouded off the map.
    pub fn cell_status(&self, player: u32, x: i32, y: i32) -> Visibility {
        match (self.index(x, y), self.grids.get(&player)) {
            (Some(i), Some(g)) => status_of(g[i].0),
            _ => Visibility::Shrouded,
        }
    }
    /// The visibility at a world position.
    pub fn status_at(&self, player: u32, p: [f32; 2]) -> Visibility {
        let (x, y) = self.world_to_cell(p);
        self.cell_status(player, x, y)
    }
    fn circle_cells(&self, look: &Look) -> Vec<(i32, i32, i32)> {
        let (cx, cy) = self.world_to_cell(look.position);
        // worldToCellDist rounds up; at least one cell.
        let r = ((look.radius / self.cell).ceil() as i32).max(1);
        discrete_circle(cx, cy, r)
    }
    /// `doShroudReveal`.
    pub fn reveal(&mut self, look: &Look) {
        let lines = self.circle_cells(look);
        for &p in &look.players {
            for &(x1, x2, y) in &lines {
                for x in x1..=x2 {
                    if let Some(i) = self.index(x, y) {
                        // addLooker: a 1 goes straight to -1, otherwise down by one.
                        let c = &mut self.grid(p)[i];
                        c.0 = (c.0 - 1).min(-1);
                    }
                }
            }
        }
    }
    /// `undoShroudReveal`.
    pub fn unreveal(&mut self, look: &Look) {
        let lines = self.circle_cells(look);
        for &p in &look.players {
            for &(x1, x2, y) in &lines {
                for x in x1..=x2 {
                    if let Some(i) = self.index(x, y) {
                        // removeLooker: -1 goes up to min(active, 1), otherwise up by one.
                        let c = &mut self.grid(p)[i];
                        c.0 = if c.0 == -1 { c.1.min(1) } else { c.0 + 1 };
                    }
                }
            }
        }
    }
    /// `processPendingUndoShroudRevealQueue`.
    pub fn process_pending(&mut self, now: u32) {
        while self.pending.front().is_some_and(|(f, _)| *f < now) {
            let (_, look) = self.pending.pop_front().unwrap();
            self.unreveal(&look);
        }
    }
    /// The cells an object covers (`doSmallFill` / `doCircleFill`).
    pub fn object_cells(&self, p: [f32; 2], radius: f32, small: bool) -> Vec<(i32, i32)> {
        let mut out = std::collections::BTreeSet::new();
        if small {
            let r = radius.min(self.cell * 0.5);
            let (x1, y1) = self.world_to_cell([p[0] - r, p[1] - r]);
            let (x2, y2) = self.world_to_cell([p[0] + r, p[1] + r]);
            for x in x1..=x2 {
                for y in y1..=y2 {
                    out.insert((x, y));
                }
            }
        } else {
            let (cx, cy) = self.world_to_cell(p);
            let r = ((radius / self.cell).ceil() as i32).max(1);
            let (mut y, mut dec) = (r - 1, 3 - 2 * r);
            for x in 0..r {
                for (a, b, row) in [
                    (cx - x, cx + x, cy + y),
                    (cx - x, cx + x, cy - y),
                    (cx - y, cx + y, cy + x),
                    (cx - y, cx + y, cy - x),
                ] {
                    for c in a..=b {
                        out.insert((c, row));
                    }
                }
                if dec >= 0 {
                    dec += (1 - y) << 2;
                    y -= 1;
                }
                dec += (x << 2) + 6;
            }
        }
        out.into_iter().filter(|&(x, y)| self.index(x, y).is_some()).collect()
    }
}

impl Simulation {
    /// `Object::getShroudClearingRange`.
    fn shroud_clearing_range(&self, id: u32) -> f32 {
        let o = &self.objects[&id];
        if o.status & crate::stealth::bits().under_construction != 0 {
            return o.body.radius;
        }
        let t = &self.templates.objects[&o.template];
        let r = t.number("ShroudClearingRange", -1.0);
        if r == -1.0 { t.number("VisionRange", 0.0) } else { r }
    }

    fn look_key(&self, id: u32) -> LookKey {
        let o = &self.objects[&id];
        let b = crate::stealth::bits();
        LookKey {
            cell: o.contained_by.is_none().then(|| self.shroud.world_to_cell([o.position[0], o.position[1]])),
            owner: o.owner,
            status: o.status & (b.stealthed | b.detected | b.disguised | b.under_construction),
            contained_by: o.contained_by,
            dead: o.dead,
        }
    }

    /// `Object::handleShroud`: undo the last look (later) and look again.
    fn handle_shroud(&mut self, id: u32) {
        let now = self.frame;
        let persist = self.templates.unlook_persist;
        let o = self.objects.get_mut(&id).unwrap();
        for look in [o.look.take(), o.reveal_all_look.take()].into_iter().flatten() {
            self.shroud.pending.push_back((now + persist, look));
        }
        let o = &self.objects[&id];
        if o.dead || o.status & crate::body::status::DESTROYED != 0 {
            return;
        }
        // Not from inside a tunnel or transport.
        if let Some(c) = o.contained_by.and_then(|c| self.objects.get(&c)).and_then(|c| c.contain.as_ref())
            && !c.data.garrison
        {
            return;
        }
        let owner = o.owner;
        let position = [o.position[0], o.position[1]];
        let t = &self.templates.objects[&o.template];
        let reveal_all = t.number("ShroudRevealToAllRange", -1.0);
        let reveal_kind = t.has_kind("REVEAL_TO_ALL");
        self.player(owner);
        let players: Vec<u32> = self.players.keys().copied().collect();
        let range = self.shroud_clearing_range(id);
        if range > 0.0 {
            let mut mask: Vec<u32> = if reveal_kind {
                players.clone()
            } else {
                players
                    .iter()
                    .copied()
                    .filter(|p| {
                        *p == owner || self.players_relationship(owner, *p) == crate::weapons::Relationship::Allies
                    })
                    .collect()
            };
            mask.sort_unstable();
            mask.dedup();
            let look = Look { position, radius: range, players: mask };
            self.shroud.reveal(&look);
            self.objects.get_mut(&id).unwrap().look = Some(look);
        }
        let o = &self.objects[&id];
        if reveal_all > 0.0
            && o.status & crate::stealth::bits().under_construction == 0
            && !self.stealthed_and_undetected(id)
        {
            let mask: Vec<u32> = players
                .iter()
                .copied()
                .filter(|p| *p != owner && self.players_relationship(owner, *p) != crate::weapons::Relationship::Allies)
                .collect();
            let look = Look { position, radius: reveal_all, players: mask };
            self.shroud.reveal(&look);
            self.objects.get_mut(&id).unwrap().reveal_all_look = Some(look);
        }
    }

    /// The partition manager's update: objects whose cell (or ownership,
    /// container, life or stealth) changed look again, then due un-looks run.
    pub(crate) fn update_shroud(&mut self) {
        let ids: Vec<u32> = self.objects.keys().copied().collect();
        for id in ids {
            let key = self.look_key(id);
            if self.objects[&id].look_key != Some(key) {
                self.objects.get_mut(&id).unwrap().look_key = Some(key);
                self.handle_shroud(id);
            }
        }
        self.shroud.process_pending(self.frame);
    }

    /// An object leaving the world: its looks are undone later.
    pub(crate) fn shroud_on_remove(&mut self, id: u32) {
        let now = self.frame;
        let persist = self.templates.unlook_persist;
        if let Some(o) = self.objects.get_mut(&id) {
            for look in [o.look.take(), o.reveal_all_look.take()].into_iter().flatten() {
                self.shroud.pending.push_back((now + persist, look));
            }
        }
    }

    /// `Object::getShroudedStatus(player)`.
    pub fn shrouded_status(&self, id: u32, player: u32) -> Visibility {
        let Some(o) = self.objects.get(&id) else { return Visibility::Shrouded };
        let t = &self.templates.objects[&o.template];
        if t.has_kind("ALWAYS_VISIBLE") || o.contained_by.is_some() {
            return Visibility::Clear;
        }
        let small = t.values("GeometryIsSmall").first().is_some_and(|v| v.eq_ignore_ascii_case("yes"));
        let cells = self.shroud.object_cells([o.position[0], o.position[1]], o.body.radius, small);
        if cells.is_empty() {
            return Visibility::Shrouded;
        }
        // Clear if any cell is clear, fogged if none is but one is fogged.
        cells.iter().map(|&(x, y)| self.shroud.cell_status(player, x, y)).min().unwrap()
    }
}
