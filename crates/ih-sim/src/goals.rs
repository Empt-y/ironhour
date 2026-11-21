//! Units' cells in the pathfinder, from `AIPathfind.cpp`: each mobile AI unit
//! marks the cells it stands on (`updatePos`/`removePos`) and the cells of its
//! goal (`updateGoal`/`removeGoal`), with `PathfindCell::setGoalUnit` and
//! `setPosUnit`'s flag rules. Move orders pick a free goal near the requested
//! point (`adjustDestination`, `checkForAdjust`, `checkDestination`,
//! `snapClosestGoalPosition`), so units sent to one spot spread out instead of
//! fighting over it.
use crate::{Simulation, footprint::CellType, navigation::CELL, simulation::ModuleKind, terrain::Waypoint};
use std::collections::BTreeMap;

/// `PathfindCell::CellFlags`.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
#[repr(u8)]
pub enum CellFlags {
    #[default]
    NoUnits = 0,
    UnitGoal = 1,
    UnitPresentMoving = 2,
    UnitPresentFixed = 3,
    UnitGoalOtherMoving = 5,
}

/// A cell's `PathfindCellInfo` unit fields.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct CellUnits {
    pub flags: CellFlags,
    /// 0 for none (`INVALID_ID`).
    pub goal_unit: u32,
    pub pos_unit: u32,
    pub goal_aircraft: u32,
}

/// Unit cells by (layer, x, y); cells with no units are absent.
#[derive(Clone, Debug, Default)]
pub struct UnitCells {
    pub cells: BTreeMap<(u32, i32, i32), CellUnits>,
}

impl UnitCells {
    pub fn get(&self, layer: u32, x: i32, y: i32) -> CellUnits {
        self.cells.get(&(layer, x, y)).copied().unwrap_or_default()
    }
    fn put(&mut self, key: (u32, i32, i32), c: CellUnits) {
        if c == CellUnits::default() {
            self.cells.remove(&key);
        } else {
            self.cells.insert(key, c);
        }
    }
    /// `PathfindCell::setGoalUnit`.
    pub fn set_goal_unit(&mut self, key: (u32, i32, i32), unit: u32) {
        let mut c = self.get(key.0, key.1, key.2);
        if unit == 0 {
            c.goal_unit = 0;
            c.flags = if c.pos_unit == 0 { CellFlags::NoUnits } else { CellFlags::UnitPresentMoving };
        } else {
            c.goal_unit = unit;
            c.flags = if unit == c.pos_unit {
                CellFlags::UnitPresentFixed
            } else if c.pos_unit == 0 {
                CellFlags::UnitGoal
            } else {
                CellFlags::UnitGoalOtherMoving
            };
        }
        self.put(key, c);
    }
    /// `PathfindCell::setPosUnit`.
    pub fn set_pos_unit(&mut self, key: (u32, i32, i32), unit: u32) {
        let mut c = self.get(key.0, key.1, key.2);
        if unit == 0 {
            c.pos_unit = 0;
            c.flags = if c.goal_unit == 0 { CellFlags::NoUnits } else { CellFlags::UnitGoal };
        } else {
            if c.goal_unit != 0 && c.goal_unit == c.pos_unit {
                return; // "A unit is already occupying this cell."
            }
            c.pos_unit = unit;
            c.flags = if unit == c.goal_unit {
                CellFlags::UnitPresentFixed
            } else if c.goal_unit == 0 {
                CellFlags::UnitPresentMoving
            } else {
                CellFlags::UnitGoalOtherMoving
            };
        }
        self.put(key, c);
    }
    /// `PathfindCell::setGoalAircraft`.
    pub fn set_goal_aircraft(&mut self, key: (u32, i32, i32), unit: u32) {
        let mut c = self.get(key.0, key.1, key.2);
        c.goal_aircraft = unit;
        self.put(key, c);
    }
}

/// `getRadiusAndCenter`: the half-width in cells a unit covers, and whether its
/// centre sits in a cell's middle (odd widths) or on a corner (even widths).
pub fn radius_and_center(bounding_radius: f32) -> (i32, bool) {
    const MAX_RADIUS: i32 = 2;
    let mut diameter = 2.0 * bounding_radius;
    if diameter > CELL && diameter < 2.0 * CELL {
        diameter = 2.0 * CELL;
    }
    let mut r = (diameter / CELL + 0.3).floor() as i32;
    if r == 0 {
        r = 1;
    }
    let mut center = r & 1 != 0;
    r /= 2;
    if r > MAX_RADIUS {
        r = MAX_RADIUS;
        center = true;
    }
    (r, center)
}

/// The cell a unit's footprint is keyed to.
fn unit_cell(p: [f32; 2], center: bool) -> (i32, i32) {
    if center {
        ((p[0] / CELL).floor() as i32, (p[1] / CELL).floor() as i32)
    } else {
        ((0.5 + p[0] / CELL).floor() as i32, (0.5 + p[1] / CELL).floor() as i32)
    }
}

/// `adjustCoordToCell`.
pub fn coord_of_cell(x: i32, y: i32, center: bool) -> [f32; 2] {
    if center {
        [(x as f32 + 0.5) * CELL, (y as f32 + 0.5) * CELL]
    } else {
        // The original adds 0.05 as a double.
        [(f64::from(x) + 0.05) as f32 * CELL, (f64::from(y) + 0.05) as f32 * CELL]
    }
}

impl Simulation {
    /// Mobile AI objects take part (`KINDOF_IMMOBILE` and objects without AI do not).
    fn tracked(&self, id: u32) -> bool {
        let o = &self.objects[&id];
        !self.templates.objects[&o.template].has_kind("IMMOBILE")
            && o.modules.iter().any(|m| m.kind == ModuleKind::Steering)
    }
    /// `isDoingGroundMovement`.
    pub(crate) fn ground_movement(&self, id: u32) -> bool {
        let o = &self.objects[&id];
        let t = &self.templates.objects[&o.template];
        t.locomotor().and_then(|n| self.templates.locomotors.get(n)).is_some_and(|l| l.surface_movement())
            && !o.disabled.has(crate::disabled::DisabledReason::Held)
    }
    pub(crate) fn footprint(&self, id: u32) -> (i32, bool) {
        radius_and_center(self.templates.objects[&self.objects[&id].template].collision_radius())
    }
    fn cell_exists(&self, layer: u32, x: i32, y: i32) -> bool {
        self.grid.has_cell(layer, x, y)
    }
    /// The cells a footprint covers around `c`.
    fn covered(c: (i32, i32), r: i32, center: bool) -> impl Iterator<Item = (i32, i32)> {
        let above = r + i32::from(center);
        (c.0 - r..c.0 + above).flat_map(move |i| (c.1 - r..c.1 + above).map(move |j| (i, j)))
    }

    /// `Pathfinder::updatePos`, after the object moved.
    pub(crate) fn update_pos(&mut self, id: u32) {
        if !self.objects.contains_key(&id) || !self.tracked(id) {
            return;
        }
        if !self.ground_movement(id) {
            if self.objects[&id].cur_cell.is_some() {
                self.remove_pos(id);
            }
            return;
        }
        let (r, center) = self.footprint(id);
        let o = &self.objects[&id];
        let new = unit_cell([o.position[0], o.position[1]], center);
        if o.cur_cell == Some(new) {
            return;
        }
        let layer = o.layer;
        if let Some(cur) = o.cur_cell {
            self.clear_pos(id, cur, r, center, layer);
        }
        self.objects.get_mut(&id).unwrap().cur_cell = Some(new);
        for (i, j) in Self::covered(new, r, center) {
            if self.cell_exists(layer, i, j) {
                self.unit_cells.set_pos_unit((layer, i, j), id);
            }
        }
    }
    fn clear_pos(&mut self, id: u32, cur: (i32, i32), r: i32, center: bool, layer: u32) {
        for (i, j) in Self::covered(cur, r, center) {
            for l in if layer == 0 { vec![0] } else { vec![layer, 0] } {
                if self.unit_cells.get(l, i, j).pos_unit == id {
                    self.unit_cells.set_pos_unit((l, i, j), 0);
                }
            }
        }
    }
    /// `Pathfinder::removePos`.
    pub(crate) fn remove_pos(&mut self, id: u32) {
        if !self.tracked(id) {
            return;
        }
        let (r, center) = self.footprint(id);
        let o = self.objects.get_mut(&id).unwrap();
        let (cur, layer) = (o.cur_cell.take(), o.layer);
        if let Some(cur) = cur {
            self.clear_pos(id, cur, r, center, layer);
        }
    }

    /// `Pathfinder::updateGoal`: the unit's goal cells move to `goal`.
    pub(crate) fn update_goal(&mut self, id: u32, goal: [f32; 2], layer: u32) {
        if !self.tracked(id) {
            return;
        }
        let (r, center) = self.footprint(id);
        if !self.ground_movement(id) {
            // updateAircraftGoal: only hover and wings keep aircraft goals.
            self.remove_goal(id);
            let t = &self.templates.objects[&self.objects[&id].template];
            let adjusts = t.locomotor().and_then(|n| self.templates.locomotors.get(n)).is_some_and(|l| {
                matches!(l.appearance, crate::templates::Appearance::Hover | crate::templates::Appearance::Wings)
            });
            if !adjusts {
                return;
            }
            let new = unit_cell(goal, center);
            self.objects.get_mut(&id).unwrap().goal_cell = Some(new);
            for (i, j) in Self::covered(new, r, center) {
                if self.cell_exists(0, i, j) {
                    self.unit_cells.set_goal_aircraft((0, i, j), id);
                }
            }
            return;
        }
        let new = unit_cell(goal, center);
        let o = &self.objects[&id];
        if o.destination_layer == layer && o.goal_cell == Some(new) {
            return;
        }
        self.remove_goal(id);
        let o = self.objects.get_mut(&id).unwrap();
        o.destination_layer = layer;
        o.goal_cell = Some(new);
        // A deck's goal cells are marked on the deck only (bridge ends are not modelled).
        for (i, j) in Self::covered(new, r, center) {
            if self.cell_exists(layer, i, j) {
                self.unit_cells.set_goal_unit((layer, i, j), id);
            }
        }
    }
    /// `Pathfinder::removeGoal`.
    pub(crate) fn remove_goal(&mut self, id: u32) {
        if !self.tracked(id) {
            return;
        }
        let (mut r, center) = self.footprint(id);
        if r == 0 {
            r = 1;
        }
        let o = self.objects.get_mut(&id).unwrap();
        let (Some(goal), layer) = (o.goal_cell.take(), o.destination_layer) else { return };
        for (i, j) in Self::covered(goal, r, center) {
            let c = self.unit_cells.get(0, i, j);
            if c.goal_unit == id {
                self.unit_cells.set_goal_unit((0, i, j), 0);
            }
            if c.goal_aircraft == id {
                self.unit_cells.set_goal_aircraft((0, i, j), 0);
            }
            if layer != 0 && self.unit_cells.get(layer, i, j).goal_unit == id {
                self.unit_cells.set_goal_unit((layer, i, j), 0);
            }
        }
    }
    /// `Pathfinder::moveAllies`: after a new path, idle allies standing on it (by
    /// their position cells, within the mover's footprint) are asked to move out of
    /// the way (`aiMoveAwayFromUnit`). Infantry pass through infantry; infantry ask
    /// vehicles only when they block the path. Our paths are sparse, so they are
    /// walked a cell at a time. Dozers and harvesters always clear their path.
    pub(crate) fn move_allies(&mut self, id: u32) {
        if self.kind_of_id(id, "NO_COLLIDE") {
            return;
        }
        let (r, center) = self.footprint(id);
        let o = &self.objects[&id];
        let layer_of = |w: &Waypoint| w.layer;
        let mut found: Vec<u32> = Vec::new();
        for pair in o.path.iter().collect::<Vec<_>>().windows(2).rev() {
            let (a, b) = (pair[0], pair[1]);
            let len = ih_math::length([b.position[0] - a.position[0], b.position[1] - a.position[1]]);
            let steps = (len / CELL).ceil().max(1.0) as i32;
            for s in 0..=steps {
                let t = s as f32 / steps as f32;
                let p = [
                    a.position[0] + (b.position[0] - a.position[0]) * t,
                    a.position[1] + (b.position[1] - a.position[1]) * t,
                ];
                let c = ((p[0] / CELL).floor() as i32, (p[1] / CELL).floor() as i32);
                for (i, j) in Self::covered(c, r, center) {
                    let u = self.unit_cells.get(layer_of(b), i, j).pos_unit;
                    if u != 0 && u != id && !found.contains(&u) {
                        found.push(u);
                    }
                }
            }
        }
        let me_infantry = self.kind_of_id(id, "INFANTRY");
        let owner = self.objects[&id].owner;
        for other in found {
            let Some(ob) = self.objects.get(&other) else { continue };
            if ob.owner != owner {
                continue; // only allies
            }
            if me_infantry && self.kind_of_id(other, "INFANTRY") {
                continue; // infantry walk through infantry
            }
            if ob.target.is_none() && ob.ai.move_out.is_none() {
                self.move_away_from_unit(other, id);
            }
        }
    }
    fn kind_of_id(&self, id: u32, kind: &str) -> bool {
        self.templates.objects[&self.objects[&id].template].has_kind(kind)
    }

    /// Removes a destroyed object's cells.
    pub(crate) fn forget_cells(&mut self, id: u32) {
        self.remove_goal(id);
        self.remove_pos(id);
    }

    /// `Pathfinder::checkDestination`: whether a unit may end at this cell.
    fn check_destination(&self, id: u32, x: i32, y: i32, layer: u32, r: i32, center: bool) -> bool {
        let mut pf = self.pathfinder(id);
        (pf.mover.radius, pf.mover.center) = (r, center);
        pf.check_destination(x, y, layer)
    }

    /// `Pathfinder::checkForAdjust` (without a group destination).
    #[allow(clippy::too_many_arguments)]
    fn check_for_adjust(&self, id: u32, x: i32, y: i32, layer: u32, r: i32, center: bool, dest: &mut [f32; 2]) -> bool {
        if !self.cell_exists(layer, x, y) {
            return false;
        }
        if layer == 0 && self.grid.obstacles.kind[(y * self.grid.width + x) as usize] == CellType::Cliff {
            return false; // "no final destinations on cliffs"
        }
        if !self.check_destination(id, x, y, layer, r, center) {
            return false;
        }
        let adjusted = coord_of_cell(x, y, center);
        let o = &self.objects[&id];
        let t = &self.templates.objects[&o.template];
        if !t.has_kind("AIRCRAFT") {
            let terrain = self.terrain(id);
            // The pathfinder's zones ignore unit size.
            let radius = 0.0;
            let here = Waypoint { position: [o.position[0], o.position[1]], layer: o.layer };
            let to = |p: [f32; 2]| Waypoint { position: p, layer };
            let reachable = terrain.quick_path_exists(here, to(adjusted), radius)
                || (!terrain.quick_path_exists(here, to(*dest), radius)
                    && terrain.quick_path_exists(to(*dest), to(adjusted), radius));
            if !reachable {
                return false;
            }
        }
        *dest = adjusted;
        true
    }

    /// `Pathfinder::adjustDestination`: the nearest cell, in an outward square
    /// spiral of up to 400 cells, where the unit may end. False if none.
    pub(crate) fn adjust_destination(&self, id: u32, dest: &mut [f32; 2], layer: u32) -> bool {
        let t = &self.templates.objects[&self.objects[&id].template];
        if t.has_kind("PROJECTILE") {
            return true;
        }
        let (r, center) = self.footprint(id);
        let mut p = *dest;
        if !center {
            p[0] += CELL / 2.0;
            p[1] += CELL / 2.0;
        }
        let (mut i, mut j) = ((p[0] / CELL).floor() as i32, (p[1] / CELL).floor() as i32);
        if self.check_for_adjust(id, i, j, layer, r, center, dest) {
            return true;
        }
        let mut limit = 400;
        let mut delta = 1;
        while limit > 0 {
            for (di, dj, grow) in [(1, 0, false), (0, 1, true), (-1, 0, false), (0, -1, true)] {
                for _ in 0..delta {
                    i += di;
                    j += dj;
                    limit -= 1;
                    if self.check_for_adjust(id, i, j, layer, r, center, dest) {
                        return true;
                    }
                }
                if grow {
                    delta += 1;
                }
            }
        }
        false
    }

    /// `Pathfinder::snapClosestGoalPosition`, after `adjustDestination` failed.
    pub(crate) fn snap_closest_goal(&self, id: u32, dest: &mut [f32; 2], layer: u32) {
        let (r, center) = self.footprint(id);
        let mut p = *dest;
        if !center {
            p[0] += CELL / 2.0;
            p[1] += CELL / 2.0;
        }
        let c = ((p[0] / CELL).floor() as i32, (p[1] / CELL).floor() as i32);
        *dest = coord_of_cell(c.0, c.1, center);
        if self.check_destination(id, c.0, c.1, layer, r, center) {
            return;
        }
        let around = || (c.0 - 1..c.0 + 2).flat_map(move |i| (c.1 - 1..c.1 + 2).map(move |j| (i, j)));
        for (i, j) in around() {
            if self.check_destination(id, i, j, layer, r, center) {
                *dest = coord_of_cell(i, j, center);
                return;
            }
        }
        if r == 0 {
            for (i, j) in around() {
                if self.cell_exists(layer, i, j) {
                    let g = self.unit_cells.get(layer, i, j).goal_unit;
                    if g == 0 || g == id {
                        *dest = coord_of_cell(i, j, center);
                        return;
                    }
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn radius_and_center_follow_the_original_rounding() {
        // Small units cover one cell, centred.
        assert_eq!(radius_and_center(3.0), (0, true));
        // Diameters between one and two cells count as two: an even width, on a corner.
        assert_eq!(radius_and_center(6.0), (1, false));
        // 30 wide: 3 cells, centred.
        assert_eq!(radius_and_center(15.0), (1, true));
        // Capped at radius 2, centred.
        assert_eq!(radius_and_center(40.0), (2, true));
    }

    #[test]
    fn cell_flags_follow_set_goal_and_set_pos() {
        let mut c = UnitCells::default();
        let k = (0, 3, 4);
        c.set_pos_unit(k, 7);
        assert_eq!(c.get(0, 3, 4).flags, CellFlags::UnitPresentMoving);
        c.set_goal_unit(k, 9);
        assert_eq!(c.get(0, 3, 4).flags, CellFlags::UnitGoalOtherMoving);
        c.set_pos_unit(k, 0);
        assert_eq!(c.get(0, 3, 4).flags, CellFlags::UnitGoal);
        c.set_pos_unit(k, 9);
        assert_eq!(c.get(0, 3, 4).flags, CellFlags::UnitPresentFixed);
        // A unit sitting on its own goal keeps the cell.
        c.set_pos_unit(k, 5);
        assert_eq!(c.get(0, 3, 4).pos_unit, 9);
        c.set_goal_unit(k, 0);
        c.set_pos_unit(k, 0);
        assert!(c.cells.is_empty());
    }
}
