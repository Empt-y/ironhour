//! Group moves, from `AIGroup.cpp`: `groupMoveToPosition`,
//! `friend_computeGroundPath`, `friend_moveInfantryToPos`,
//! `friend_moveVehicleToPos`, `computeIndividualDestination`,
//! `groupTightenToPosition`, `getMinMaxAndCenter`, `clampToMap` and
//! `clampWaypointPosition`. A selection ordered somewhere far enough away
//! follows one shared ground path: infantry in three (or, from 16, five)
//! columns, vehicles in two (or three) lanes, each ending at its own spot near
//! the goal. Short moves keep the group's shape; a click inside the middle of
//! the group gathers it tightly around the point.
//!
//! The shared path is `findGroundPath`'s six-cell-wide path (`pathfind`).
//! Stand-in: tightening moves each unit to an adjusted goal rather than through `AIMoveAndTightenState`'s
//! approach path. Formations (`groupCreateFormation`) and waypoint appends are
//! not ported.
use crate::{Simulation, ai::AiState, navigation::CELL, simulation::SimError};
use std::collections::VecDeque;

/// `PATH_DIAMETER_IN_CELLS`.
const PATH_DIAMETER_IN_CELLS: i32 = 6;
/// `STD_WAYPOINT_CLAMP_MARGIN` and `STD_AIRCRAFT_EXTRA_MARGIN`.
const WAYPOINT_MARGIN: f32 = CELL * 4.0;
const AIRCRAFT_MARGIN: f32 = CELL * 10.0;

#[derive(Clone, Copy, PartialEq, Eq)]
enum Kind {
    Infantry,
    Vehicles,
}

/// `SimpleObjectIterator`: `insert` prepends, then a stable sort.
fn sorted(mut v: Vec<(u32, f32)>, far_to_near: bool) -> Vec<u32> {
    v.reverse();
    if far_to_near {
        v.sort_by(|a, b| b.1.total_cmp(&a.1));
    } else {
        v.sort_by(|a, b| a.1.total_cmp(&b.1));
    }
    v.into_iter().map(|(id, _)| id).collect()
}

fn normalized(v: [f32; 2]) -> [f32; 2] {
    ih_math::normalized(v)
}

impl Simulation {
    fn held(&self, id: u32) -> bool {
        self.objects[&id].disabled.has(crate::disabled::DisabledReason::Held)
    }
    fn kind_of(&self, id: u32, kind: &str) -> bool {
        self.templates.objects[&self.objects[&id].template].has_kind(kind)
    }
    fn move_priority(&self, id: u32) -> Option<i32> {
        let t = &self.templates.objects[&self.objects[&id].template];
        t.locomotor().and_then(|n| self.templates.locomotors.get(n)).map(|l| l.move_priority)
    }
    fn xy(&self, id: u32) -> [f32; 2] {
        let p = self.objects[&id].position;
        [p[0], p[1]]
    }

    /// `getMinMaxAndCenter` over AI members that are not held.
    fn min_max_center(&self, ids: &[u32]) -> ([f32; 2], [f32; 2], [f32; 2]) {
        let (mut lo, mut hi, mut sum, mut n) = ([1e10f32; 2], [-1e10f32; 2], [0.0f32; 2], 0);
        for &id in ids.iter().filter(|id| !self.held(**id) && self.has_ai(**id)) {
            let p = self.xy(id);
            sum = [sum[0] + p[0], sum[1] + p[1]];
            lo = [lo[0].min(p[0]), lo[1].min(p[1])];
            hi = [hi[0].max(p[0]), hi[1].max(p[1])];
            n += 1;
        }
        let n = n as f32;
        (lo, hi, [sum[0] / n, sum[1] / n])
    }

    /// `getCenter`: AI members, else every member, never the held ones.
    fn group_center(&self, ids: &[u32]) -> Option<[f32; 2]> {
        let pick = |ai: bool| -> Vec<[f32; 2]> {
            ids.iter().filter(|id| !self.held(**id) && (!ai || self.has_ai(**id))).map(|id| self.xy(*id)).collect()
        };
        let mut v = pick(true);
        if v.is_empty() && !ids.is_empty() {
            v = pick(false);
        }
        if v.is_empty() {
            return None;
        }
        let n = v.len() as f32;
        Some([v.iter().map(|p| p[0]).sum::<f32>() / n, v.iter().map(|p| p[1]).sum::<f32>() / n])
    }

    /// The map extent (`getExtent`).
    fn extent(&self) -> ([f32; 2], [f32; 2]) {
        ([0.0, 0.0], [self.grid.width as f32 * CELL, self.grid.height as f32 * CELL])
    }
    /// `clampToMap` for a human player: inside the map less one cell.
    fn clamp_to_map(&self, p: &mut [f32; 2]) {
        let (lo, hi) = self.extent();
        p[0] = p[0].clamp(lo[0] + CELL, hi[0] - CELL);
        p[1] = p[1].clamp(lo[1] + CELL, hi[1] - CELL);
    }

    /// `friend_computeGroundPath`: the shared path, when the move is far or big enough.
    fn ground_path(&mut self, ids: &[u32], pos: [f32; 2]) -> Option<Vec<[f32; 2]>> {
        let g = self.templates.group;
        let (lo, hi, center) = self.min_max_center(ids);
        let mut dist2 = 4.0 * g.distance_requires_group * g.distance_requires_group;
        let (mut infantry, mut vehicles) = (0, 0);
        let mut center_unit: Option<(u32, f32)> = None;
        for &id in ids {
            self.remove_goal(id);
            if self.held(id) || !self.has_ai(id) {
                continue;
            }
            if self.kind_of(id, "INFANTRY") {
                infantry += 1;
            } else if self.kind_of(id, "VEHICLE") {
                if self.kind_of(id, "AIRCRAFT") {
                    continue;
                }
                vehicles += 1;
            } else {
                continue;
            }
            let p = self.xy(id);
            let d = (p[0] - pos[0]).powi(2) + (p[1] - pos[1]).powi(2);
            if d < dist2 {
                dist2 = d;
            }
            let c = (p[0] - center[0]).powi(2) + (p[1] - center[1]).powi(2);
            if center_unit.is_none_or(|(_, best)| c < best) {
                center_unit = Some((id, c));
            }
        }
        let (center_id, _) = center_unit?;
        let center = self.xy(center_id);
        let (dx, dy) = (hi[0] - lo[0], hi[1] - lo[1]);
        let requires = g.distance_requires_group * g.distance_requires_group;
        if dx * dx + dy * dy > requires {
            dist2 = dx * dx + dy * dy;
        }
        if dist2 < g.min_distance * g.min_distance {
            return None;
        }
        let mut close_enough = dist2 > requires || infantry > 6 || vehicles > 4;
        if !close_enough {
            // Close enough when every infantry unit has a clear line to the centre.
            close_enough = ids.iter().filter(|id| self.kind_of(**id, "INFANTRY") && self.has_ai(**id)).all(|&id| {
                let o = &self.objects[&id];
                self.pathfinder(id).is_line_passable(o.layer, self.xy(id), center, false, true)
            });
        }
        if !close_enough {
            return None;
        }
        self.find_ground_path(center_id, center, pos)
    }

    /// `findGroundPath` over the ground terrain, `PATH_DIAMETER_IN_CELLS` wide,
    /// not crushing: the optimised nodes, from the group centre.
    fn find_ground_path(&self, any: u32, from: [f32; 2], to: [f32; 2]) -> Option<Vec<[f32; 2]>> {
        let pf = crate::pathfind::Pathfinder { terrain: &self.grid, ..self.pathfinder(any) };
        let path = pf.find_ground_path(from, to, PATH_DIAMETER_IN_CELLS, false)?;
        let mut out = vec![path.nodes[0].position];
        out.extend(path.optimized().into_iter().map(|w| w.position));
        Some(out)
    }

    /// `friend_moveInfantryToPos` and `friend_moveVehicleToPos`: lay the units of
    /// one kind out along the shared path. False when the group is too small or
    /// the path too short.
    fn move_kind_along(&mut self, ids: &[u32], pos: [f32; 2], path: &[[f32; 2]], kind: Kind) -> bool {
        let Some(center) = self.group_center(ids) else { return false };
        let far_enough = (PATH_DIAMETER_IN_CELLS as f32 * CELL).powi(2);
        let d2 = |a: [f32; 2], b: [f32; 2]| (a[0] - b[0]).powi(2) + (a[1] - b[1]).powi(2);
        let start_point = path[0];
        let Some(start_node) = path.iter().position(|n| d2(*n, start_point) > far_enough) else { return false };
        let end_point = *path.last().unwrap();
        let end_node = path.iter().rposition(|n| d2(*n, end_point) > far_enough);
        let end_node = match (kind, end_node) {
            (Kind::Vehicles, Some(0)) | (_, None) => return false,
            (_, Some(i)) => i,
        };
        let mut start_vector = normalized([path[start_node][0] - start_point[0], path[start_node][1] - start_point[1]]);
        let end_vector = normalized([end_point[0] - path[end_node][0], end_point[1] - path[end_node][1]]);
        let mut start_normal = normalized([-start_vector[1], start_vector[0]]);
        let end_normal = normalized([-end_vector[1], end_vector[0]]);
        let mut members = Vec::new();
        let mut use_end = false;
        for &id in ids {
            if self.held(id) || !self.has_ai(id) {
                continue;
            }
            match kind {
                Kind::Infantry => {
                    if !self.kind_of(id, "INFANTRY") {
                        continue;
                    }
                    if self.kind_of(id, "MOB_NEXUS") {
                        return false;
                    }
                }
                Kind::Vehicles => {
                    if !self.kind_of(id, "VEHICLE") || !self.ground_movement(id) {
                        continue;
                    }
                }
            }
            self.remove_goal(id);
            let p = self.xy(id);
            members.push(id);
            if d2(p, start_point) > d2(p, end_point) {
                use_end = true;
            }
        }
        let n = members.len() as i32;
        let min = match kind {
            Kind::Infantry => self.templates.group.min_infantry,
            Kind::Vehicles => self.templates.group.min_vehicles,
        };
        if n < min {
            return false;
        }
        if use_end {
            start_vector = end_vector;
            start_normal = end_normal;
        }
        let across = |s: &Self, id: u32| {
            let p = s.xy(id);
            (p[0] - center[0]) * start_normal[0] + (p[1] - center[1]) * start_normal[1]
        };
        // With the end vector the original re-inserts twice, which keeps the order and
        // only recomputes the keys.
        let order: Vec<(u32, f32)> = members.iter().map(|&id| (id, across(self, id))).collect();
        let by_side = sorted(order, true);
        // Initial column assignment by position across the path.
        let mut columns: std::collections::BTreeMap<u32, (i32, i32)> = std::collections::BTreeMap::new();
        let mut along = Vec::new();
        for (index, &id) in by_side.iter().enumerate() {
            let index = index as i32;
            let (wide, narrow) = match kind {
                Kind::Infantry => {
                    let column = (1 - index / ((n + 1) / 3).max(1)).max(-1);
                    let five = (2 - index / ((n + 3) / 5).max(1)).max(-2);
                    (if n < 16 { column } else { five }, column)
                }
                Kind::Vehicles => {
                    let mut column = 1 - index / ((n + 1) / 2).max(1);
                    if column == 0 {
                        column = -1;
                    }
                    let three = (1 - index / ((n + 1) / 3).max(1)).max(-1);
                    (if n < 5 { column } else { three }, column)
                }
            };
            columns.insert(id, (wide, narrow));
            let p = self.xy(id);
            let mut adjust = 0.0;
            if kind == Kind::Infantry {
                match self.move_priority(id).unwrap_or(2) {
                    1 => adjust = -100.0 * CELL,
                    0 => adjust = -200.0 * CELL,
                    _ => {}
                }
            }
            along.push((id, adjust + (p[0] - center[0]) * start_vector[0] + (p[1] - center[1]) * start_vector[1]));
        }
        let along = sorted(along, true);
        // Balance the columns, front movers first (vehicles ignore priority, so
        // they are balanced once per priority group, as the original loops).
        let (narrow_slots, wide_slots): (Vec<usize>, usize) = match kind {
            Kind::Infantry => ((0..3).collect(), 5),
            Kind::Vehicles => (vec![0, 2], 3),
        };
        let mut narrow_count = [0i32; 3];
        let mut wide_count = [0i32; 5];
        let wide_mid = (wide_slots / 2) as i32;
        for group in (0..=2).rev() {
            for &id in &along {
                if kind == Kind::Infantry && self.move_priority(id).unwrap_or(1) != group {
                    continue;
                }
                let (mut wide, mut narrow) = columns[&id];
                let min_n = narrow_slots.iter().map(|&i| narrow_count[i]).min().unwrap();
                let min_w = (0..wide_slots).map(|i| wide_count[i]).min().unwrap();
                let mut best = None;
                let mut delta = 10000;
                for &i in &narrow_slots {
                    if narrow_count[i] == min_n {
                        let d = ((1 + narrow) - i as i32).abs();
                        if d < delta {
                            delta = d;
                            best = Some(i);
                        }
                    }
                }
                if let Some(b) = best {
                    narrow_count[b] += 1;
                    narrow = b as i32 - 1;
                }
                let mut best = None;
                let mut delta = 10000;
                for (i, count) in wide_count.iter().enumerate().take(wide_slots) {
                    if *count == min_w {
                        let d = ((wide_mid + wide) - i as i32).abs();
                        if d < delta {
                            delta = d;
                            best = Some(i);
                        }
                    }
                }
                if let Some(b) = best {
                    wide_count[b] += 1;
                    wide = b as i32 - wide_mid;
                }
                let small = match kind {
                    Kind::Infantry => n < 16,
                    Kind::Vehicles => n < 5,
                };
                if small {
                    wide = narrow;
                }
                columns.insert(id, (wide, narrow));
            }
        }
        // Each unit's own path along the shared one, then its end spot.
        let mut factors = [0i32; 5];
        for &id in &along {
            let (wide, narrow) = columns[&id];
            let factor = factors[(wide + 2) as usize];
            factors[(wide + 2) as usize] = factor + 1;
            let mut points: Vec<[f32; 2]> = Vec::new();
            let mut node = Some(start_node);
            let mut previous = 0usize;
            let mut prev_pos = self.xy(id);
            while let Some(ni) = node {
                let mut dest = path[ni];
                let Some(next) = (ni + 1..path.len()).find(|&t| d2(path[t], dest) > far_enough) else { break };
                let corner = [path[next][0] - path[previous][0], path[next][1] - path[previous][1]];
                let corner_normal = normalized([-corner[1], corner[0]]);
                let offset = match kind {
                    Kind::Infantry => CELL * 2.1,
                    Kind::Vehicles => CELL * 1.5,
                };
                dest[0] += offset * narrow as f32 * corner_normal[0];
                dest[1] += offset * narrow as f32 * corner_normal[1];
                let side = if factor & 1 != 0 { 0.5 } else { -0.5 };
                dest[0] += side * CELL * corner_normal[0];
                dest[1] += side * CELL * corner_normal[1];
                let cur = [dest[0] - prev_pos[0], dest[1] - prev_pos[1]];
                self.clamp_to_map(&mut dest);
                if corner[0] * cur[0] + corner[1] * cur[1] > 0.0 {
                    points.push(dest);
                    prev_pos = dest;
                }
                let advanced = ni + 1;
                node = (advanced < path.len()).then_some(advanced);
                if let Some(ni) = node {
                    for t in previous + 1..ni {
                        if d2(path[t], path[ni]) > far_enough {
                            previous = t;
                        }
                    }
                }
            }
            let mut dest = pos;
            let (column, offset) = match kind {
                Kind::Infantry => (wide.clamp(-2, 2), CELL * 2.2),
                Kind::Vehicles => (wide.clamp(-3, 3), if n < 5 { CELL * 1.5 } else { CELL * 3.2 }),
            };
            dest[0] += offset * column as f32 * end_normal[0];
            dest[1] += offset * column as f32 * end_normal[1];
            if factor & 1 != 0 {
                dest[0] += CELL * end_normal[0];
                dest[1] += CELL * end_normal[1];
            }
            if kind == Kind::Infantry {
                let delta = (self.move_priority(id).unwrap_or(1) - 2) as f32;
                dest[0] += delta * CELL * end_vector[0];
                dest[1] += delta * CELL * end_vector[1];
            }
            dest[0] -= factor as f32 * offset * end_vector[0];
            dest[1] -= factor as f32 * offset * end_vector[1];
            while let Some(last) = points.last() {
                let cur = [dest[0] - last[0], dest[1] - last[1]];
                if end_vector[0] * cur[0] + end_vector[1] * cur[1] <= 0.0 {
                    points.pop();
                } else {
                    break;
                }
            }
            self.clamp_to_map(&mut dest);
            // adjustDestination's result is not checked here in the original.
            self.adjust_destination(id, &mut dest, 0);
            self.update_goal(id, dest, 0);
            points.push(dest);
            self.follow_points(id, points);
        }
        true
    }

    /// `aiFollowPath`: errors (an immobile or disabled unit) leave the unit be.
    fn follow_points(&mut self, id: u32, points: Vec<[f32; 2]>) {
        let mut points: VecDeque<[f32; 2]> = points.into();
        // An unreachable first point is a failed leg: start from the next one.
        while let Some(first) = points.front().copied() {
            if points.len() == 1 || self.route_to(id, first).is_ok() {
                break;
            }
            points.pop_front();
        }
        let Some(first) = points.pop_front() else { return };
        if self.begin_move(id, first, AiState::FollowPath, points).is_err() {
            self.failed_orders += 1;
        }
    }

    /// `computeIndividualDestination`: the same offset from the group centre at
    /// the goal, at most six bounding radii.
    fn individual_destination(&mut self, id: u32, goal: [f32; 2], center: [f32; 2]) -> [f32; 2] {
        let p = self.xy(id);
        let v = [p[0] - center[0], p[1] - center[1]];
        let limit = 6.0 * self.templates.objects[&self.objects[&id].template].collision_radius();
        let length = ih_math::length(v).min(limit);
        let v = normalized(v);
        let mut dest = [goal[0] + v[0] * length, goal[1] + v[1] * length];
        if self.ground_movement(id) {
            let layer = self.grid.destination(&self.map, goal).layer;
            // With a group destination the original first tries `tightenPath` and a
            // path-cost check, then falls back to the plain adjustment used here.
            self.adjust_destination(id, &mut dest, layer);
            self.update_goal(id, dest, 0);
        }
        dest
    }

    /// `groupMoveToPosition` for a player's selection (no waypoint append).
    pub fn group_move(&mut self, ids: &[u32], target: [f32; 2]) -> Result<(), SimError> {
        let ids: Vec<u32> = ids.iter().copied().filter(|id| self.objects.contains_key(id)).collect();
        if ids.is_empty() {
            return Err(SimError("group move has no objects".into()));
        }
        let mut pos = target;
        let (mut lo, mut hi, _) = self.min_max_center(&ids);
        let (mut did_infantry, mut did_vehicles) = (false, false);
        if let Some(path) = self.ground_path(&ids, pos) {
            did_infantry = self.move_kind_along(&ids, pos, &path, Kind::Infantry);
            did_vehicles = self.move_kind_along(&ids, pos, &path, Kind::Vehicles);
        }
        let mut tighten = false;
        let gather = self.templates.group.gather_factor;
        if gather > 0.0 {
            // ScaleRect2D, then whether the click is inside it.
            let s = gather - 1.0;
            let (dw, dh) = ((hi[0] - lo[0]) * s * 0.5, (hi[1] - lo[1]) * s * 0.5);
            lo = [lo[0] - dw, lo[1] - dh];
            hi = [hi[0] + dw, hi[1] + dh];
            tighten = pos[0] >= lo[0] && pos[0] <= hi[0] && pos[1] >= lo[1] && pos[1] <= hi[1];
        }
        let mut extra = 0.0f32;
        for &id in &ids {
            if self.kind_of(id, "PRODUCED_AT_HELIPAD") {
                extra =
                    extra.max(self.templates.objects[&self.objects[&id].template].number("GeometryMajorRadius", 1.0));
            } else if self.kind_of(id, "AIRCRAFT") {
                if !self.ground_movement(id) {
                    tighten = false; // "Don't tighten aircraft. It is a bad idea."
                }
                extra = extra.max(AIRCRAFT_MARGIN);
            }
        }
        // clampWaypointPosition (the margin is an Int in the original).
        let margin = (WAYPOINT_MARGIN + extra).trunc();
        let (elo, ehi) = self.extent();
        if pos[0] > ehi[0] - margin {
            pos[0] = ehi[0] - margin;
        } else if pos[0] < elo[0] + margin {
            pos[0] = elo[0] + margin;
        }
        if pos[1] > ehi[1] - margin {
            pos[1] = ehi[1] - margin;
        } else if pos[1] < elo[1] + margin {
            pos[1] = elo[1] + margin;
        }
        if tighten {
            // The original measures both sides with the x extent.
            let dx = ((hi[0] - lo[0]) / CELL) as i32;
            let dy = ((hi[0] - lo[0]) / CELL) as i32;
            if dx * dy < 2000 {
                self.tighten_to(&ids, pos);
                return Ok(());
            }
        }
        // Individual moves, nearest the goal first.
        let mut near = Vec::new();
        for &id in &ids {
            if self.held(id) || self.kind_of(id, "IMMOBILE") || !self.has_ai(id) {
                continue;
            }
            if self.kind_of(id, "INFANTRY") && did_infantry {
                continue;
            }
            if self.kind_of(id, "VEHICLE")
                && did_vehicles
                && self.ground_movement(id)
                && !self.kind_of(id, "CLIFF_JUMPER")
            {
                continue;
            }
            self.remove_goal(id);
            let p = self.xy(id);
            near.push((id, (p[0] - pos[0]).powi(2) + (p[1] - pos[1]).powi(2)));
        }
        let near = sorted(near, false);
        let mut center = [0.0; 2];
        for (i, &id) in near.iter().enumerate() {
            if i == 0 {
                center = self.xy(id);
            }
            let dest = self.individual_destination(id, pos, center);
            if self.begin_move(id, dest, AiState::MoveTo, VecDeque::new()).is_err() {
                self.failed_orders += 1;
            }
        }
        Ok(())
    }

    /// `groupTightenToPosition`: every unit to the point, nearest first.
    fn tighten_to(&mut self, ids: &[u32], pos: [f32; 2]) {
        let mut near = Vec::new();
        for &id in ids {
            if self.held(id) || self.kind_of(id, "IMMOBILE") || !self.has_ai(id) {
                continue;
            }
            let p = self.xy(id);
            near.push((id, (p[0] - pos[0]).powi(2) + (p[1] - pos[1]).powi(2)));
        }
        for id in sorted(near, false) {
            if self.begin_move(id, pos, AiState::MoveTo, VecDeque::new()).is_err() {
                self.failed_orders += 1;
            }
        }
    }
}
