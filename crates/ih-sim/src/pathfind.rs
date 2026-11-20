//! The pathfinder's cell search, from `AIPathfind.cpp`: `Pathfinder::findPath`,
//! `internalFindPath` and `findClosestPath`; `examineNeighboringCells` with its
//! straight-line probe (`examineCellsCallback`), `checkChangeLayers`,
//! `checkForMovement`, `checkDestination`, `validMovementPosition`,
//! `buildActualPath` and `prependCells`, `Path::optimize`, `isLinePassable` and
//! `iterateCellsAlongLine`; `PathfindCell::costSoFar`, `costToGoal` and
//! `putOnSortedOpenList`.
//!
//! As in the original, terrain is tested at the cell a unit's centre is in. The
//! unit's footprint (`getRadiusAndCenter`) is tested only against other units'
//! cells: parked allies cost extra (and mark the path `blockedByAlly`, so they
//! are asked to move), parked enemies that cannot be crushed block.
//!
//! ironhour differences: the zone manager's hierarchical pre-pass
//! (`findHierarchicalPath` marking zone blocks passable) is stood in for by the
//! coarse corridor of `zones`; the logical extent is the whole grid (computer
//! players cannot path off the map yet); there is no ignored obstacle and no
//! dozer exception (both need structures as objects, M5).
use crate::{
    footprint::{CellType, surface},
    goals::{CellFlags, UnitCells, coord_of_cell},
    navigation::CELL,
    simulation::{CrushTest, ModuleKind, Object},
    templates::Templates,
    terrain::{Terrain, Waypoint},
    zones::{NO_ZONE, Zones},
};
use ih_formats::map::Map;
use std::{
    collections::{BTreeMap, BTreeSet, HashMap},
    sync::Arc,
};

pub const COST_ORTHOGONAL: u32 = 10;
pub const COST_DIAGONAL: u32 = 14;
const COST_TO_DISTANCE_FACTOR_SQR: f32 = (1.0 / 10.0) * (1.0 / 10.0);
/// `CELL_INFOS_TO_ALLOCATE`: the search gives up when the pool runs out.
const CELL_INFOS: usize = 30000;

/// A pathfind cell as `getCell` returns it.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Cell {
    pub layer: u32,
    pub x: i32,
    pub y: i32,
    pub kind: CellType,
    pub pinched: bool,
    /// `isObstacleFence`.
    pub fence: bool,
    /// `getConnectLayer`: the layer this cell links to (bridge ends).
    pub connect: Option<u32>,
    /// The terrain node, `None` for a bridge layer's cells off the deck.
    pub node: Option<usize>,
}

impl Terrain {
    /// `getCell(layer, x, y)`: ground cells on the grid; a bridge layer covers its
    /// extent, with the cells off the deck impassable, and falls back to the
    /// ground beyond it.
    pub fn cell_at(&self, layer: u32, x: i32, y: i32) -> Option<Cell> {
        if layer == 0 {
            if x < 0 || y < 0 || x >= self.width || y >= self.height {
                return None;
            }
            let i = (y * self.width + x) as usize;
            let connect =
                self.layers.iter().position(|l| l.cells.get(&(x, y)).is_some_and(|c| c.entry)).map(|l| l as u32 + 1);
            return Some(Cell {
                layer,
                x,
                y,
                kind: self.obstacles.kind[i],
                pinched: self.obstacles.pinched[i],
                fence: self.obstacles.fence[i],
                connect,
                node: Some(i),
            });
        }
        let l = self.layers.get(layer as usize - 1)?;
        // `allocateCells`: one cell of padding below and `hi` exclusive. Outside
        // the layer, `Pathfinder::getCell` returns the ground cell.
        if x < l.lo.0 || y < l.lo.1 || x >= l.hi.0 || y >= l.hi.1 {
            return self.cell_at(0, x, y);
        }
        let (kind, connect, node) = match l.cells.get(&(x, y)) {
            Some(c) => (CellType::Clear, c.entry.then_some(0), Some(c.node())),
            None => (CellType::Impassable, None, None),
        };
        Some(Cell { layer, x, y, kind, pinched: false, fence: false, connect, node })
    }
    /// `worldToCell`, clamped to the extent.
    pub fn world_to_cell(&self, p: [f32; 2]) -> (i32, i32) {
        (
            ((p[0] / CELL).floor() as i32).clamp(0, self.width - 1),
            ((p[1] / CELL).floor() as i32).clamp(0, self.height - 1),
        )
    }
}

/// What the pathfinder reads from the moving object.
#[derive(Clone, Copy, Debug)]
pub struct Mover {
    pub id: u32,
    pub owner: u32,
    pub layer: u32,
    /// `getRadiusAndCenter`.
    pub radius: i32,
    pub center: bool,
    /// `getCrusherLevel() > 0`.
    pub crusher: bool,
    /// The locomotor set's `getValidSurfaces`.
    pub surfaces: u8,
    pub infantry: bool,
    /// `canPathThroughUnits` (units leaving a factory, M5).
    pub through_units: bool,
    /// `isDownhillOnly`.
    pub downhill: bool,
    /// `isAircraftThatAdjustsDestination`.
    pub aircraft_goals: bool,
}

/// `checkForMovement`'s `TCheckMovementInfo` results.
#[derive(Clone, Copy, Debug, Default)]
struct MoveInfo {
    ally_fixed: i32,
    ally_moving: bool,
    enemy_fixed: bool,
}

/// A path: every cell node, and the optimised chain (`getNextOptimized`) the
/// unit follows.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Path {
    pub nodes: Vec<Waypoint>,
    can_optimize: Vec<bool>,
    next_optimized: Vec<usize>,
    /// `isBlockedByAlly`: the route goes through parked allies' cells.
    pub blocked_by_ally: bool,
}
impl Path {
    /// The optimised nodes after the start.
    pub fn optimized(&self) -> Vec<Waypoint> {
        let mut out = Vec::new();
        let mut i = 0;
        while i + 1 < self.nodes.len() {
            i = self.next_optimized[i];
            out.push(self.nodes[i]);
        }
        out
    }
}

/// A cell's `PathfindCellInfo` during one search.
#[derive(Clone, Copy, Debug)]
struct Info {
    cell: Cell,
    open: bool,
    closed: bool,
    cost: u32,
    total: u32,
    parent: Option<usize>,
    /// The open-list key it was inserted with.
    open_key: (u32, u64),
    blocked_by_ally: bool,
}

struct Search {
    info: HashMap<usize, Info>,
    open: BTreeSet<(u32, u64, usize)>,
    seq: u64,
    tunneling: bool,
    zones: Arc<Zones>,
    /// Zones the hierarchical pre-pass let through; `None` for all.
    passable: Option<Vec<bool>>,
}
impl Search {
    fn new(zones: Arc<Zones>) -> Self {
        Self { info: HashMap::new(), open: BTreeSet::new(), seq: 0, tunneling: false, zones, passable: None }
    }
    fn has(&self, n: usize) -> bool {
        self.info.contains_key(&n)
    }
    fn on_list(&self, n: usize) -> bool {
        self.info.get(&n).is_some_and(|i| i.open || i.closed)
    }
    /// `allocateInfo`.
    fn allocate(&mut self, c: Cell) -> bool {
        let Some(n) = c.node else { return false };
        if self.info.contains_key(&n) {
            return true;
        }
        if self.info.len() >= CELL_INFOS {
            return false;
        }
        self.info.insert(
            n,
            Info {
                cell: c,
                open: false,
                closed: false,
                cost: 0,
                total: 0,
                parent: None,
                open_key: (0, 0),
                blocked_by_ally: false,
            },
        );
        true
    }
    fn get(&self, n: usize) -> &Info {
        &self.info[&n]
    }
    fn get_mut(&mut self, n: usize) -> &mut Info {
        self.info.get_mut(&n).unwrap()
    }
    /// `putOnSortedOpenList`: after every cell of no greater total cost.
    fn put_open(&mut self, n: usize) {
        self.seq += 1;
        let seq = self.seq;
        let i = self.get_mut(n);
        i.open = true;
        i.closed = false;
        i.open_key = (i.total, seq);
        let key = (i.total, seq, n);
        self.open.insert(key);
    }
    fn remove_open(&mut self, n: usize) {
        let i = self.get_mut(n);
        i.open = false;
        let (total, seq) = i.open_key;
        self.open.remove(&(total, seq, n));
    }
    fn pop_open(&mut self) -> Option<usize> {
        let (_, _, n) = self.open.pop_first()?;
        self.get_mut(n).open = false;
        Some(n)
    }
    fn close(&mut self, n: usize) {
        let i = self.get_mut(n);
        i.open = false;
        i.closed = true;
    }
    /// `PathfindZoneManager::isPassable` for a ground cell.
    fn zone_passable(&self, c: &Cell) -> bool {
        let Some(allowed) = &self.passable else { return true };
        let zone = c.node.map_or(NO_ZONE, |n| self.zones.cells[n]);
        zone != NO_ZONE && allowed[zone as usize]
    }
    /// `PathfindCell::costSoFar`.
    fn cost_so_far(&self, to: &Cell, parent: usize) -> u32 {
        let p = self.get(parent);
        let prev_dir = (p.cell.x - to.x, p.cell.y - to.y);
        let mut cost = p.cost + if prev_dir.0 == 0 || prev_dir.1 == 0 { COST_ORTHOGONAL } else { COST_DIAGONAL };
        if to.pinched {
            cost += COST_DIAGONAL;
        }
        let mut turns = 0;
        if let Some(pp) = p.parent {
            let g = self.get(pp).cell;
            let dir = (g.x - p.cell.x, g.y - p.cell.y);
            if dir != prev_dir {
                turns = match dir.0 * prev_dir.0 + dir.1 * prev_dir.1 {
                    1.. => 4,
                    0 => 8,
                    _ => 16,
                };
            }
        }
        cost + turns
    }
}

/// `PathfindCell::costToGoal`.
fn cost_to_goal(c: &Cell, goal: &Cell) -> u32 {
    let (dx, dy) = ((c.x - goal.x).unsigned_abs(), (c.y - goal.y).unsigned_abs());
    if dx > dy {
        COST_ORTHOGONAL * dx + (COST_ORTHOGONAL * dy) / 2
    } else {
        COST_ORTHOGONAL * dy + (COST_ORTHOGONAL * dx) / 2
    }
}

/// The pathfinder over one terrain view, with the units' cells.
pub struct Pathfinder<'a> {
    pub terrain: &'a Terrain,
    pub map: &'a Map,
    pub units: &'a UnitCells,
    pub objects: &'a BTreeMap<u32, Object>,
    pub templates: &'a Templates,
    pub mover: Mover,
}

impl Pathfinder<'_> {
    fn cell(&self, layer: u32, x: i32, y: i32) -> Option<Cell> {
        self.terrain.cell_at(layer, x, y)
    }
    /// `getClippedCell`.
    fn clipped_cell(&self, layer: u32, p: [f32; 2]) -> Option<Cell> {
        let (x, y) = self.terrain.world_to_cell(p);
        self.cell(layer, x, y)
    }
    /// `validMovementPosition(isCrusher, surfaces, toCell)`.
    pub fn valid(&self, c: &Cell) -> bool {
        if self.mover.crusher && c.fence && c.kind == CellType::Obstacle {
            return true;
        }
        if c.layer == 0 {
            // The view's blocked flags: cell type against the surfaces, and decks
            // too low to pass under.
            return !self.terrain.ground.blocked[c.node.unwrap()];
        }
        c.kind.allows(self.mover.surfaces & !surface::CRUSHER)
    }
    /// `validMovementPosition(isCrusher, layer, locomotorSet, pos)`.
    pub fn valid_position(&self, layer: u32, p: [f32; 2]) -> bool {
        self.clipped_cell(layer, p).is_some_and(|c| self.valid(&c))
    }
    fn ally(&self, other: &Object) -> bool {
        // Allies are the same owner until players and teams exist.
        other.owner == self.mover.owner
    }
    fn can_crush(&self, other: u32) -> bool {
        crush_or_squish(self.templates, self.objects, self.mover.id, other, CrushTest::Either)
    }
    fn covered(&self, x: i32, y: i32) -> impl Iterator<Item = (i32, i32)> + use<> {
        let (r, above) = (self.mover.radius, self.mover.radius + i32::from(self.mover.center));
        (x - r..x + above).flat_map(move |i| (y - r..y + above).map(move |j| (i, j)))
    }

    /// `checkForMovement`: `None` where the footprint leaves the map or meets a
    /// parked ally without AI.
    fn check_for_movement(&self, x: i32, y: i32, layer: u32, transient: bool) -> Option<MoveInfo> {
        let mut info = MoveInfo::default();
        let mut allies: Vec<u32> = Vec::new();
        const MAX_ALLY: usize = 5;
        for (i, j) in self.covered(x, y) {
            let cell = self.cell(layer, i, j)?;
            let c = self.units.get(cell.layer, i, j);
            // (`allyGoal` only matters to attack paths.)
            if c.flags == CellFlags::NoUnits || c.pos_unit == self.mover.id {
                continue;
            }
            let mut check = false;
            let mut unit = None;
            if matches!(c.flags, CellFlags::UnitPresentMoving | CellFlags::UnitGoalOtherMoving) {
                unit = self.objects.get(&c.pos_unit);
                if unit.is_some_and(|u| self.ally(u)) {
                    info.ally_moving = true;
                }
                check = transient;
            }
            if c.flags == CellFlags::UnitPresentFixed {
                check = true;
                unit = self.objects.get(&c.pos_unit);
            }
            let Some(unit) = unit.filter(|_| check) else { continue };
            let t = &self.templates.objects[&unit.template];
            if self.mover.infantry && t.has_kind("INFANTRY") {
                continue;
            }
            if self.ally(unit) {
                if !unit.modules.iter().any(|m| m.kind == ModuleKind::Steering) {
                    return None; // "can't path through not-idle units"
                }
                if !allies.contains(&unit.id) {
                    info.ally_fixed += 1;
                    if allies.len() < MAX_ALLY {
                        allies.push(unit.id);
                    }
                }
            } else if !self.can_crush(unit.id) {
                info.enemy_fixed = true;
            }
        }
        Some(info)
    }

    /// `checkDestination`: whether the unit may end with its footprint at this cell.
    pub fn check_destination(&self, x: i32, y: i32, layer: u32) -> bool {
        for (i, j) in self.covered(x, y) {
            let Some(cell) = self.cell(layer, i, j) else {
                return false; // off the map
            };
            let c = self.units.get(cell.layer, i, j);
            if self.mover.aircraft_goals {
                if c.goal_aircraft == 0 || c.goal_aircraft == self.mover.id {
                    continue;
                }
                return false;
            }
            // No ignored obstacle yet (`getIgnoredObstacleID`).
            if matches!(cell.kind, CellType::Obstacle | CellType::Impassable | CellType::BridgeImpassable) {
                return false;
            }
            if c.flags == CellFlags::NoUnits || c.goal_unit == self.mover.id || c.goal_unit == 0 {
                continue;
            }
            let Some(unit) = self.objects.get(&c.goal_unit) else { continue };
            if self.ally(unit) {
                return false; // "Don't usurp your allies goals."
            }
            if c.flags == CellFlags::UnitPresentFixed && !self.can_crush(c.goal_unit) {
                return false; // "Don't move to an occupied cell."
            }
        }
        true
    }

    /// `iterateCellsAlongLine` (Bresenham); stops at the first nonzero result. A
    /// cell off the layer ends the walk as a success, as in the original.
    fn iterate(
        &self,
        start: (i32, i32),
        end: (i32, i32),
        layer: u32,
        f: &mut dyn FnMut(Option<Cell>, Cell) -> i32,
    ) -> i32 {
        let (dx, dy) = ((end.0 - start.0).abs(), (end.1 - start.1).abs());
        let (mut x, mut y) = start;
        let (mut xinc1, mut xinc2) = if end.0 >= start.0 { (1, 1) } else { (-1, -1) };
        let (mut yinc1, mut yinc2) = if end.1 >= start.1 { (1, 1) } else { (-1, -1) };
        let (den, mut num, numadd, pixels);
        if dx >= dy {
            xinc1 = 0;
            yinc2 = 0;
            den = dx;
            num = dx / 2;
            numadd = dy;
            pixels = dx;
        } else {
            xinc2 = 0;
            yinc1 = 0;
            den = dy;
            num = dy / 2;
            numadd = dx;
            pixels = dy;
        }
        let mut from = None;
        for _ in 0..=pixels {
            let Some(mut to) = self.cell(layer, x, y) else { return 0 };
            let r = f(from, to);
            if r != 0 {
                return r;
            }
            num += numadd;
            if num >= den {
                num -= den;
                x += xinc1;
                y += yinc1;
                from = Some(to);
                let Some(next) = self.cell(layer, x, y) else { return 0 };
                to = next;
                let r = f(from, to);
                if r != 0 {
                    return r;
                }
            }
            x += xinc2;
            y += yinc2;
            from = Some(to);
        }
        0
    }

    /// `isLinePassable`.
    pub fn is_line_passable(&self, layer: u32, a: [f32; 2], b: [f32; 2], blocked: bool, allow_pinched: bool) -> bool {
        let (s, e) = (self.terrain.world_to_cell(a), self.terrain.world_to_cell(b));
        self.iterate(s, e, layer, &mut |from, to| {
            let Some(info) = self.check_for_movement(to.x, to.y, to.layer, blocked) else { return 1 };
            if info.ally_fixed > 0 || info.enemy_fixed {
                return 1;
            }
            if !allow_pinched && to.pinched {
                return 1;
            }
            if from.is_some_and(|f| to.layer != 0 && f.layer == to.layer) && to.kind == CellType::Clear {
                return 0;
            }
            if !self.valid(&to) {
                return 1;
            }
            0
        }) == 0
    }

    fn ground_height_at_corner(&self, c: &Cell) -> f32 {
        self.map.ground_height(c.x as f32 * CELL, c.y as f32 * CELL)
    }

    /// `checkChangeLayers`: a bridge end opens the cell on the other layer at the
    /// same cost.
    fn check_change_layers(&self, s: &mut Search, parent: usize) {
        let p = *s.get(parent);
        let Some(l) = p.cell.connect else { return };
        let Some(c) = self.cell(l, p.cell.x, p.cell.y) else { return };
        let Some(n) = c.node else { return };
        if s.on_list(n) || !s.allocate(c) {
            return;
        }
        let i = s.get_mut(n);
        i.parent = Some(parent);
        i.cost = p.cost;
        i.total = p.total;
        s.put_open(n);
    }

    /// `examineCellsCallback`: the straight line from a cell towards the goal.
    fn examine_line(&self, s: &mut Search, from: Option<Cell>, to: Cell, goal: &Cell) -> i32 {
        if s.tunneling {
            return 1;
        }
        let Some(from) = from else { return 0 };
        if !self.valid(&to) {
            return 1;
        }
        if to.layer == 0 && !s.zone_passable(&to) {
            return 1;
        }
        let Some(n) = to.node else { return 1 };
        let on_list = s.on_list(n);
        if to.pinched {
            return 1;
        }
        let Some(info) = self.check_for_movement(to.x, to.y, from.layer, false) else { return 1 };
        if info.enemy_fixed {
            return 1;
        }
        let from_node = from.node.unwrap();
        let new_cost = s.get(from_node).cost + COST_ORTHOGONAL / 2;
        if to.kind == CellType::Cliff || info.ally_fixed > 0 {
            return 1;
        }
        if !s.allocate(to) {
            return 1;
        }
        s.get_mut(n).blocked_by_ally = false;
        let remaining = cost_to_goal(&to, goal);
        if on_list && s.get(n).cost <= new_cost {
            return 0;
        }
        let i = s.get_mut(n);
        i.cost = new_cost;
        i.parent = Some(from_node);
        i.total = new_cost + remaining;
        let (open, closed) = (i.open, i.closed);
        if closed {
            s.get_mut(n).closed = false;
        }
        if open {
            s.remove_open(n);
        }
        s.put_open(n);
        0
    }

    /// `examineNeighboringCells` (without attack distances). Returns the cells
    /// newly allocated; when the pool runs out it stops early.
    fn examine(&self, s: &mut Search, parent: usize, goal: Option<&Cell>, start: (i32, i32)) -> usize {
        let pc = s.get(parent).cell;
        if let Some(goal) = goal
            && !s.tunneling
            && !self.mover.downhill
        {
            self.iterate((pc.x, pc.y), (goal.x, goal.y), pc.layer, &mut |from, to| {
                self.examine_line(s, from, to, goal)
            });
        }
        const DELTA: [(i32, i32); 8] = [(1, 0), (0, 1), (-1, 0), (0, -1), (1, 1), (-1, 1), (-1, -1), (1, -1)];
        const ADJACENT: [usize; 5] = [0, 1, 2, 3, 0];
        let mut flags = [false; 8];
        let mut count = 0;
        for (i, (dx, dy)) in DELTA.into_iter().enumerate() {
            let (nx, ny) = (pc.x + dx, pc.y + dy);
            let Some(nc) = self.cell(pc.layer, nx, ny) else { continue };
            let not_zone_passable = nc.layer == 0 && !s.zone_passable(&nc);
            // Off-deck cells of a bridge layer have no node to search.
            let Some(n) = nc.node else { continue };
            if s.on_list(n) {
                continue;
            }
            if i >= 4 && !flags[ADJACENT[i - 4]] && !flags[ADJACENT[i - 3]] {
                continue; // one of the adjacent sides must be open
            }
            if self.mover.downhill && self.ground_height_at_corner(&pc) < self.ground_height_at_corner(&nc) {
                continue;
            }
            let mut movement_valid = true;
            if !self.valid(&nc) {
                movement_valid = false;
                if !s.tunneling {
                    continue;
                }
            }
            flags[i] = true;
            let (sdx, sdy) = ((nx - start.0).abs(), (ny - start.1).abs());
            let info = match self.check_for_movement(nx, ny, pc.layer, false) {
                Some(info) if !info.enemy_fixed => info,
                other => {
                    if !s.tunneling {
                        continue;
                    }
                    movement_valid = false;
                    other.unwrap_or_default()
                }
            };
            if movement_valid && !nc.pinched {
                s.tunneling = false;
            }
            if !s.has(n) {
                if !s.allocate(nc) {
                    return count; // "Out of cells for pathing..."
                }
                count += 1;
            }
            let mut new_cost = s.cost_so_far(&nc, parent);
            if info.ally_moving && sdx < 10 && sdy < 10 {
                new_cost += 3 * COST_DIAGONAL;
            }
            if nc.kind == CellType::Cliff && !nc.pinched {
                if (self.ground_height_at_corner(&pc) - self.ground_height_at_corner(&nc)).abs() < CELL {
                    new_cost += 7 * COST_DIAGONAL;
                }
            } else if nc.pinched {
                new_cost += COST_ORTHOGONAL;
            }
            let mut blocked_by_ally = false;
            if info.ally_fixed > 0 {
                new_cost += 3 * COST_DIAGONAL;
                if !self.mover.through_units {
                    blocked_by_ally = true;
                }
            }
            let mut remaining = goal.map_or(0, |g| cost_to_goal(&nc, g));
            if not_zone_passable {
                new_cost += 100 * COST_ORTHOGONAL;
            }
            if nc.kind == CellType::Obstacle {
                new_cost += 100 * COST_ORTHOGONAL;
            }
            if s.tunneling && !self.valid(&nc) {
                new_cost += 10 * COST_ORTHOGONAL;
            }
            if s.tunneling {
                remaining = 0; // "find the closest valid cell"
            }
            let e = s.get_mut(n);
            e.blocked_by_ally = blocked_by_ally;
            e.cost = new_cost;
            e.parent = Some(parent);
            e.total = new_cost + remaining;
            let (open, closed) = (e.open, e.closed);
            if closed {
                s.get_mut(n).closed = false;
            }
            if open {
                s.remove_open(n);
            }
            s.put_open(n);
        }
        count
    }

    /// Start and goal cells and the search state shared by both searches.
    fn begin(&self, s: &mut Search, start: Cell, goal: Cell) -> Option<(usize, usize)> {
        let (sn, gn) = (start.node?, goal.node?);
        if !s.allocate(goal) || !s.allocate(start) {
            return None;
        }
        // startPathfind
        let i = s.get_mut(sn);
        i.parent = None;
        i.cost = 0;
        i.total = cost_to_goal(&start, &goal);
        s.put_open(sn);
        Some((sn, gn))
    }

    /// `findPath` (`internalFindPath`): `to` is the goal on `layer`.
    pub fn find_path(&self, from: [f32; 2], to: [f32; 2], layer: u32) -> Option<Path> {
        if to == [0.0, 0.0] {
            return None;
        }
        let mut s = Search::new(self.terrain.cell_zones());
        let adjust = if self.mover.center { to } else { [to[0] + CELL / 2.0, to[1] + CELL / 2.0] };
        let goal = self.clipped_cell(layer, adjust)?;
        if !self.check_destination(goal.x, goal.y, layer) {
            return None;
        }
        let start_ndx = self.terrain.world_to_cell(from);
        let start = self.clipped_cell(self.mover.layer, from)?;
        s.tunneling = start.kind == CellType::Obstacle;
        let connected = start.node.zip(goal.node).is_some_and(|(a, b)| s.zones.connected(a, b));
        // Into or out of a structure the original compares terrain zones, which
        // ignore structures; ironhour's zones do not, so that test is skipped.
        if !(goal.kind == CellType::Obstacle || s.tunneling) && !connected {
            return None;
        }
        if !self.valid(&goal) {
            return None;
        }
        if !self.valid(&start) {
            s.tunneling = true; // "somehow we got to an impassable location"
        }
        if connected {
            s.passable = Some(s.zones.corridor(start.node.unwrap(), goal.node.unwrap()).0);
        }
        let (_, gn) = self.begin(&mut s, start, goal)?;
        while let Some(p) = s.pop_open() {
            if p == gn {
                s.tunneling = false;
                return Some(self.build_actual_path(&s, from, gn, false));
            }
            s.close(p);
            self.check_change_layers(&mut s, p);
            self.examine(&mut s, p, Some(&goal), start_ndx);
        }
        None
    }

    /// `findClosestPath`: a path to the goal, else to the reachable cell nearest
    /// it where the unit may stop. Returns the path and the goal it ends at.
    pub fn find_closest_path(
        &self,
        from: [f32; 2],
        to: [f32; 2],
        layer: u32,
        blocked: bool,
        cost_multiplier: f32,
    ) -> Option<(Path, [f32; 2])> {
        if self.mover.surfaces & !surface::CRUSHER == 0 {
            return None;
        }
        let mut s = Search::new(self.terrain.cell_zones());
        let adjust = if self.mover.center { to } else { [to[0] + CELL / 2.0, to[1] + CELL / 2.0] };
        let goal = self.clipped_cell(layer, adjust)?;
        let start_ndx = self.terrain.world_to_cell(from);
        let start = self.clipped_cell(self.mover.layer, from)?;
        if !self.valid(&start) {
            s.tunneling = true; // "relax the constraints"
        }
        match self.check_for_movement(start_ndx.0, start_ndx.1, self.mover.layer, blocked) {
            Some(i) if !i.enemy_fixed => {}
            _ => s.tunneling = true,
        }
        let mut hierarchical = false;
        if !s.tunneling
            && let (Some(a), Some(b)) = (start.node, goal.node)
            && s.zones.connected(a, b)
        {
            s.passable = Some(s.zones.corridor(a, b).0);
            hierarchical = true;
        }
        let started_stuck = s.tunneling;
        let (_, gn) = self.begin(&mut s, start, goal)?;
        let mut closest: Option<usize> = None;
        let (mut closest_dist, mut closest_screen) = (f32::MAX, f32::MAX);
        let mut count = 0usize;
        let mut found_goal = false;
        while let Some(p) = s.pop_open() {
            let pc = s.get(p).cell;
            if p == gn {
                if closest.is_some() && !self.mover.through_units && !self.check_destination(pc.x, pc.y, pc.layer) {
                    found_goal = true;
                    continue;
                }
                s.tunneling = false;
                return Some((self.build_actual_path(&s, from, gn, blocked), to));
            }
            s.close(p);
            if !s.tunneling && self.check_destination(pc.x, pc.y, pc.layer) && (!started_stuck || self.valid(&pc)) {
                let (dx, dy) = ((goal.x - pc.x).abs() as f32, (goal.y - pc.y).abs() as f32);
                let mut d = dx * dx + dy * dy;
                if d < closest_screen {
                    closest_screen = d;
                }
                let cost = s.get(p).cost as f32;
                d += (cost * (cost * COST_TO_DISTANCE_FACTOR_SQR)) * cost_multiplier;
                if d < closest_dist {
                    closest = Some(p);
                    closest_dist = d;
                }
            }
            let (dx, dy) = ((goal.x - pc.x).abs() as f32, (goal.y - pc.y).abs() as f32);
            if dx * dx + dy * dy > closest_screen * 4.0
                && (!hierarchical || count > 2000 || closest_screen < 10.0 * 10.0 * CELL)
            {
                continue;
            }
            if !found_goal {
                self.check_change_layers(&mut s, p);
                count += self.examine(&mut s, p, Some(&goal), start_ndx);
            }
        }
        let c = closest?;
        let cc = s.get(c).cell;
        s.tunneling = false;
        let to = [cc.x as f32 * CELL + CELL / 2.0, cc.y as f32 * CELL + CELL / 2.0];
        Some((self.build_actual_path(&s, from, c, blocked), to))
    }

    /// `buildActualPath`: `prependCells`, then `Path::optimize`.
    fn build_actual_path(&self, s: &Search, from: [f32; 2], goal: usize, blocked: bool) -> Path {
        let mut goal = goal;
        let g = s.get(goal);
        if g.cell.pinched
            && let Some(p) = g.parent
            && !s.get(p).cell.pinched
        {
            goal = p;
        }
        let mut path = self.prepend_cells(s, from, goal, self.mover.center);
        self.optimize(&mut path, blocked);
        path
    }

    /// `prependCells`: the cells from the start to `goal`, back to front, with
    /// the start position in front.
    fn prepend_cells(&self, s: &Search, from: [f32; 2], goal: usize, center: bool) -> Path {
        let mut rev: Vec<(Waypoint, bool)> = Vec::new();
        let mut blocked_by_ally = false;
        let goal_alone = s.get(goal).parent.is_none();
        let mut cell = goal;
        let mut prev: Option<Cell> = None;
        while let Some(parent) = s.get(cell).parent {
            let c = s.get(cell).cell;
            let pos = coord_of_cell(c.x, c.y, center);
            if let Some(pc) = prev
                && (c.x, c.y) == (pc.x, pc.y)
            {
                let layer = if c.layer == 0 { pc.layer } else { c.layer };
                rev.last_mut().unwrap().0.layer = layer;
                cell = parent;
                continue;
            }
            let mut can_optimize = true;
            if c.kind == CellType::Cliff {
                if prev.is_some_and(|p| p.kind != CellType::Cliff)
                    && let Some(first) = rev.last_mut()
                {
                    first.1 = false;
                }
            } else if prev.is_some_and(|p| p.kind == CellType::Cliff) {
                can_optimize = false;
            }
            rev.push((Waypoint { position: pos, layer: c.layer }, can_optimize));
            if s.get(cell).blocked_by_ally {
                blocked_by_ally = true;
            }
            prev = Some(c);
            cell = parent;
        }
        let start = s.get(cell).cell;
        if goal_alone {
            rev.push((Waypoint { position: coord_of_cell(start.x, start.y, center), layer: start.layer }, true));
        }
        if rev.last().is_none_or(|(w, _)| w.position != from) {
            rev.push((Waypoint { position: from, layer: start.layer }, true));
        }
        rev.reverse();
        Path {
            nodes: rev.iter().map(|(w, _)| *w).collect(),
            can_optimize: rev.iter().map(|(_, c)| *c).collect(),
            next_optimized: Vec::new(),
            blocked_by_ally,
        }
    }

    /// `Path::optimize`: each anchor links to the farthest later node it has a
    /// clear line to (at most three steps past a layer change, never past a node
    /// that may not be optimised).
    fn optimize(&self, path: &mut Path, blocked: bool) {
        let n = path.nodes.len();
        path.next_optimized = (1..=n).collect();
        if n < 2 {
            return;
        }
        let last = n - 1;
        let pos = |i: usize| path.nodes[i].position;
        let mut anchor = 0;
        let mut first = true;
        let first_layer = path.nodes[0].layer;
        const ALLOWED_STEPS: usize = 3;
        while anchor != last {
            let mut optimized = false;
            let mut layer = path.nodes[anchor].layer;
            let mut cur = layer;
            let mut count = 0;
            let mut node = anchor + 1;
            while node < last {
                count += 1;
                if cur == 0 {
                    if path.nodes[node].layer != cur {
                        layer = path.nodes[node].layer;
                        if count > ALLOWED_STEPS {
                            break;
                        }
                    }
                } else if path.nodes[node + 1].layer != cur && count > ALLOWED_STEPS {
                    break;
                }
                cur = path.nodes[node].layer;
                if !path.can_optimize[node] {
                    break;
                }
                node += 1;
            }
            if first {
                layer = first_layer;
                first = false;
            }
            while node != anchor {
                let (a, b) = (pos(anchor), pos(node));
                let mut passable = self.is_line_passable(layer, a, b, blocked, false);
                if self.clipped_cell(layer, b).is_some_and(|c| c.kind == CellType::Cliff && !c.pinched) {
                    passable = true;
                }
                if !passable {
                    let (dx, dy) = ((b[0] - a[0]) as i32, (b[1] - a[1]) as i32);
                    let cell = CELL as i32;
                    if (dx.abs() == cell && dy.abs() == cell) || straight_run(path, anchor, node) {
                        passable = true;
                    }
                }
                if passable {
                    path.next_optimized[anchor] = node;
                    anchor = node;
                    optimized = true;
                    break;
                }
                node -= 1;
            }
            if !optimized {
                path.next_optimized[anchor] = anchor + 1;
                anchor += 1;
            }
        }
    }
}

impl Pathfinder<'_> {
    /// `clearCellForDiameter`: the widest even diameter (down from `diameter`, by
    /// two) clear of terrain and of parked units that may not be driven over;
    /// the corners of the square are cut once the radius exceeds one cell.
    pub fn clear_cell_for_diameter(&self, crusher: bool, x: i32, y: i32, layer: u32, diameter: i32) -> i32 {
        let radius = diameter / 2;
        let above = if radius == 0 { 1 } else { radius };
        let cut_corners = radius > 1;
        let mut clear = true;
        'outer: for i in x - radius..x + above {
            let x_edge = i == x - radius || i == x + above - 1;
            for j in y - radius..y + above {
                let y_edge = j == y - radius || j == y + above - 1;
                if x_edge && y_edge && cut_corners {
                    continue;
                }
                let Some(cell) = self.cell(layer, i, j) else {
                    return 0; // off the map
                };
                if cell.kind != CellType::Clear && !(cell.kind == CellType::Obstacle && cell.fence && crusher) {
                    clear = false;
                }
                let c = self.units.get(cell.layer, i, j);
                if c.flags == CellFlags::UnitPresentFixed
                    && diameter >= 2
                    && let Some(o) = self.objects.get(&c.pos_unit)
                {
                    let crushable = self.templates.objects[&o.template].number("CrushableLevel", 255.0) as i32;
                    if crushable > i32::from(crusher) {
                        clear = false;
                    }
                }
                if !clear {
                    break 'outer;
                }
            }
        }
        if clear {
            return if radius == 0 { 1 } else { 2 * radius };
        }
        if diameter < 2 {
            return 0;
        }
        self.clear_cell_for_diameter(crusher, x, y, layer, diameter - 2)
    }

    /// `isGroundPathPassable`.
    fn is_ground_path_passable(&self, crusher: bool, a: [f32; 2], layer: u32, b: [f32; 2], diameter: i32) -> bool {
        let (s, e) = (self.terrain.world_to_cell(a), self.terrain.world_to_cell(b));
        self.iterate(s, e, layer, &mut |from, to| {
            if self.clear_cell_for_diameter(crusher, to.x, to.y, to.layer, diameter) == diameter {
                return 0;
            }
            if from.is_some_and(|f| to.layer != 0 && f.layer == to.layer) {
                return 0;
            }
            1
        }) == 0
    }

    /// `findGroundPath`: a path `diameter` cells wide for a group (`AIGroup`),
    /// over clear unpinched ground; cells narrower than the width cost more. The
    /// goal moves to a nearby cell that is clear for the full width.
    pub fn find_ground_path(&self, from: [f32; 2], to: [f32; 2], diameter: i32, crusher: bool) -> Option<Path> {
        if to == [0.0, 0.0] {
            return None;
        }
        let t = self.terrain;
        let mut s = Search::new(t.cell_zones());
        let layer = t.destination(self.map, to).layer;
        let mut cell = t.world_to_cell(to);
        let fits = |c: (i32, i32)| self.clear_cell_for_diameter(crusher, c.0, c.1, layer, diameter) == diameter;
        if !fits(cell) {
            let mut offset = 1;
            const MAX_OFFSET: i32 = 8;
            'search: while offset < MAX_OFFSET {
                let saved = cell;
                for (dx, dy) in [(1, 0), (0, 1), (-1, 0), (-1, 0), (0, -1), (0, -1), (1, 0), (1, 0)] {
                    cell = (cell.0 + dx * offset, cell.1 + dy * offset);
                    if fits(cell) {
                        break 'search;
                    }
                }
                offset += 1;
                cell = saved;
            }
            if offset >= MAX_OFFSET {
                return None;
            }
        }
        let goal = self.cell(layer, cell.0, cell.1)?;
        let from_layer = t.destination(self.map, from).layer;
        let start = self.clipped_cell(from_layer, from)?;
        let (Some(sn), Some(gn)) = (start.node, goal.node) else { return None };
        if !s.zones.connected(sn, gn) {
            return None;
        }
        s.passable = Some(s.zones.corridor(sn, gn).0);
        self.begin(&mut s, start, goal)?;
        // `clipIsPassable`: the zone at a cell clipped to the map.
        let clip_passable = |s: &Search, x: i32, y: i32| {
            let (x, y) = (x.clamp(0, t.width - 1), y.clamp(0, t.height - 1));
            t.cell_at(0, x, y).is_some_and(|c| s.zone_passable(&c))
        };
        // The original reuses the last computed cost for the goal cell.
        let mut new_cost = 0u32;
        while let Some(p) = s.pop_open() {
            if p == gn {
                let mut path = self.prepend_cells(&s, from, gn, false);
                self.optimize_ground(&mut path, crusher, diameter);
                return Some(path);
            }
            s.close(p);
            self.check_change_layers(&mut s, p);
            let pc = s.get(p).cell;
            self.iterate((pc.x, pc.y), (goal.x, goal.y), pc.layer, &mut |from, to| {
                let Some(from) = from else { return 0 };
                let Some(n) = to.node else { return 1 };
                if s.on_list(n) {
                    return 1;
                }
                if self.clear_cell_for_diameter(crusher, to.x, to.y, to.layer, diameter) != diameter {
                    return 1;
                }
                if !s.allocate(to) {
                    return 1;
                }
                let fnode = from.node.unwrap();
                let cost = s.get(fnode).cost + COST_ORTHOGONAL / 2;
                let i = s.get_mut(n);
                i.blocked_by_ally = false;
                i.cost = cost;
                i.parent = Some(fnode);
                i.total = cost + cost_to_goal(&to, &goal);
                s.put_open(n);
                0
            });
            const DELTA: [(i32, i32); 8] = [(1, 0), (0, 1), (-1, 0), (0, -1), (1, 1), (-1, 1), (-1, -1), (1, -1)];
            const ADJACENT: [usize; 5] = [0, 1, 2, 3, 0];
            let mut flags = [false; 8];
            for (i, (dx, dy)) in DELTA.into_iter().enumerate() {
                let (nx, ny) = (pc.x + dx, pc.y + dy);
                let Some(nc) = self.cell(pc.layer, nx, ny) else { continue };
                let Some(n) = nc.node else { continue };
                if nc.layer == 0
                    && !s.zone_passable(&nc)
                    && ![(3, 3), (-3, 3), (3, -3), (-3, -3)].iter().any(|(ox, oy)| clip_passable(&s, nx + ox, ny + oy))
                {
                    continue;
                }
                let on_list = s.on_list(n);
                if n != gn {
                    if i >= 4 && !flags[ADJACENT[i - 4]] && !flags[ADJACENT[i - 3]] {
                        continue;
                    }
                    let clear = self.clear_cell_for_diameter(crusher, nx, ny, nc.layer, diameter);
                    if nc.kind != CellType::Clear || nc.pinched {
                        continue;
                    }
                    flags[i] = true;
                    if !s.allocate(nc) {
                        continue;
                    }
                    new_cost = s.cost_so_far(&nc, p);
                    if clear < diameter {
                        new_cost += (0.6 * ((diameter - clear) as f32 * COST_ORTHOGONAL as f32)) as u32;
                    }
                    s.get_mut(n).blocked_by_ally = false;
                }
                if on_list && s.get(n).cost <= new_cost {
                    continue;
                }
                let e = s.get_mut(n);
                e.cost = new_cost;
                e.parent = Some(p);
                e.total = new_cost + cost_to_goal(&nc, &goal);
                let (open, closed) = (e.open, e.closed);
                if closed {
                    s.get_mut(n).closed = false;
                }
                if open {
                    s.remove_open(n);
                }
                s.put_open(n);
            }
        }
        None
    }

    /// `Path::optimizeGroundPath`: as `optimize`, with the wide-path line test
    /// and without the cliff and no-optimise rules.
    fn optimize_ground(&self, path: &mut Path, crusher: bool, diameter: i32) {
        let n = path.nodes.len();
        path.next_optimized = (1..=n).collect();
        if n < 2 {
            return;
        }
        let last = n - 1;
        let pos = |i: usize| path.nodes[i].position;
        let mut anchor = 0;
        const ALLOWED_STEPS: usize = 3;
        while anchor != last {
            let mut optimized = false;
            let mut layer = path.nodes[anchor].layer;
            let mut cur = layer;
            let mut count = 0;
            let mut node = anchor + 1;
            while node < last {
                count += 1;
                if cur == 0 {
                    if path.nodes[node].layer != cur {
                        layer = path.nodes[node].layer;
                        if count > ALLOWED_STEPS {
                            break;
                        }
                    }
                } else if path.nodes[node + 1].layer != cur && count > ALLOWED_STEPS {
                    break;
                }
                cur = path.nodes[node].layer;
                node += 1;
            }
            while node != anchor {
                let passable = self.is_ground_path_passable(crusher, pos(anchor), layer, pos(node), diameter)
                    || straight_run(path, anchor, node);
                if passable {
                    path.next_optimized[anchor] = node;
                    anchor = node;
                    optimized = true;
                    break;
                }
                node -= 1;
            }
            if !optimized {
                path.next_optimized[anchor] = anchor + 1;
                anchor += 1;
            }
        }
    }
}

/// `optimize`'s `mightBePassable`: the nodes between are in one straight line
/// along an axis or diagonal (with the original's reuse of `dx` and `dy`).
fn straight_run(path: &Path, anchor: usize, node: usize) -> bool {
    let pos = |i: usize| path.nodes[i].position;
    let (a, b) = (pos(anchor), pos(node));
    let mut dx = (b[0] - a[0]) as i32;
    let mut dy = (b[1] - a[1]) as i32;
    let step = |t: usize| ((pos(t + 1)[0] - pos(t)[0]) as i32, (pos(t + 1)[1] - pos(t)[1]) as i32);
    let mut might = false;
    if dx == 0 {
        might = true;
        for t in (anchor + 1..node).rev() {
            dx = step(t).0;
            if dx != 0 {
                might = false;
            }
        }
    }
    if dy == 0 {
        might = true;
        for t in (anchor + 1..node).rev() {
            dy = step(t).1;
            if dy != 0 {
                might = false;
            }
        }
    }
    if dx == dy {
        might = true;
        for t in (anchor + 1..node).rev() {
            (dx, dy) = step(t);
            if dy != dx {
                might = false;
            }
        }
    }
    if dx == -dy {
        might = true;
        for t in (anchor + 1..node).rev() {
            (dx, dy) = step(t);
            if dy != -dx {
                might = false;
            }
        }
    }
    might
}

/// `Object::canCrushOrSquish`. Allies (same owner, until players and teams
/// exist) never crush each other.
pub fn crush_or_squish(
    templates: &Templates,
    objects: &BTreeMap<u32, Object>,
    a: u32,
    b: u32,
    test: CrushTest,
) -> bool {
    let (Some(oa), Some(ob)) = (objects.get(&a), objects.get(&b)) else { return false };
    if oa.disabled.has(crate::disabled::DisabledReason::Unmanned) || oa.owner == ob.owner {
        return false;
    }
    let (ta, tb) = (&templates.objects[&oa.template], &templates.objects[&ob.template]);
    let crusher = ta.number("CrusherLevel", 0.0) as i32;
    if crusher <= 0 {
        return false;
    }
    if test != CrushTest::Crush && tb.modules.iter().any(|m| m.class == "SquishCollide") {
        return true;
    }
    test != CrushTest::Squish && crusher > tb.number("CrushableLevel", 255.0) as i32
}
