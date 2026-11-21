//! Building construction: placement, dozers, and structures under construction.
//!
//! Sources: `Common/System/BuildAssistant.cpp` (`buildObjectNow`,
//! `isLocationLegalToBuild`, `isLocationClearOfObjects`, `iterateFootprint`,
//! `checkSampleBuildLocation`, `isRemovableForConstruction`,
//! `clearRemovableForConstruction`, `moveObjectsForConstruction`),
//! `GameLogic/Object/Update/AIUpdate/DozerAIUpdate.cpp` (`construct`, `newTask`,
//! task bookkeeping, `findGoodBuildOrRepairPosition`, `DozerPrimaryStateMachine`,
//! `DozerActionStateMachine` and its build half, `aiDoCommand`,
//! `privateResumeConstruction`), `Player::onStructureConstructionComplete`, and
//! `Pathfinder::addObjectToPathfindMap`/`removeObjectFromPathfindMap`.
//!
//! Not ported yet: repairing and fortifying, bored dozers (repair and mine
//! clearing), bridges and scaffolding, walls (line builds), rebuilding,
//! terrain flattening under new buildings, `RestrictConstruction` terrain tiles,
//! shroud, supply borders (`SupplyBuildBorder`), stealthed objects in the way,
//! selling, and computer players' placement rules. Removing a structure's cells
//! from the pathfinder leaves the cells it pinched shut impassable.
use crate::{
    SimError, Simulation,
    ai::{AiState, Outcome},
    navigation::CELL,
    terrain::Waypoint,
    traffic::{Collider, Geom},
};

/// `DozerTask` (build, repair, fortify).
pub const TASK_BUILD: usize = 0;
pub const TASKS: usize = 3;
/// `MIN_ACTION_TOLERANCE`.
const MIN_ACTION_TOLERANCE: f32 = 70.0;

/// `DozerPrimaryStateMachine` states.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum Primary {
    #[default]
    Idle = 0,
    Build = 1,
    Repair = 2,
    Fortify = 3,
}
/// `DozerActionStateMachine` states.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum Action {
    #[default]
    PickActionPos = 0,
    MoveToActionPos = 1,
    DoAction = 2,
}
/// `DozerBuildSubTask`.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum BuildSub {
    #[default]
    SelectDockLocation = 0,
    MovingToDockLocation = 1,
    DoBuildAtDock = 2,
}

#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct Task {
    pub target: Option<u32>,
    pub order_frame: u32,
    /// Start, action and end dock points.
    pub dock: [Option<[f32; 3]>; 3],
}

/// `DozerAIUpdate` (and the dozer half of `WorkerAIUpdate`).
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Dozer {
    pub repair_per_second: f32,
    pub bored_frames: f32,
    pub bored_range: f32,
    pub tasks: [Task; TASKS],
    pub current: Option<usize>,
    pub primary: Primary,
    pub idle_since: u32,
    pub action: Action,
    pub goal: Option<u32>,
    pub goal_position: [f32; 3],
    pub build_sub: BuildSub,
    pub enter_frame: u32,
    /// `WorkerStateMachine` for workers: `Some(true)` acting as a supply truck.
    pub worker: Option<bool>,
}
impl Dozer {
    pub fn from_module(class: &str, b: &ih_ini::Block) -> Option<Self> {
        if !matches!(class, "DozerAIUpdate" | "WorkerAIUpdate") {
            return None;
        }
        let mut d = Self { worker: (class == "WorkerAIUpdate").then_some(false), ..Self::default() };
        for f in b.fields() {
            let v = f.values();
            let first = v.first().cloned().unwrap_or_default();
            let n = ih_ini::values::scan_real(first.trim_end_matches('%').as_bytes()).unwrap_or(0.0);
            match f.name.as_str() {
                "RepairHealthPercentPerSecond" => d.repair_per_second = n / 100.0,
                // INI::parseDurationReal: milliseconds to frames.
                "BoredTime" => d.bored_frames = n * 30.0 / 1000.0,
                "BoredRange" => d.bored_range = n,
                _ => {}
            }
        }
        Some(d)
    }
    fn most_recent(&self) -> Option<usize> {
        let mut best = None;
        let mut frame = 0;
        for (i, t) in self.tasks.iter().enumerate() {
            if t.target.is_some() && t.order_frame > frame {
                best = Some(i);
                frame = t.order_frame;
            }
        }
        best
    }
}

/// `LegalBuildCode`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum LegalBuild {
    Ok,
    RestrictedTerrain,
    NotFlatEnough,
    ObjectsInTheWay,
    NoClearPath,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Ret {
    Continue,
    Success,
    Failure,
}

/// `OBJECT_STATUS_UNDER_CONSTRUCTION`.
pub fn under_construction_bit() -> u64 {
    ih_ini::schema::schema()
        .name_list("ObjectStatusMaskType::s_bitNameList")
        .unwrap_or(&[])
        .iter()
        .position(|n| n.eq_ignore_ascii_case("UNDER_CONSTRUCTION"))
        .map_or(0, |i| 1 << i)
}

impl Simulation {
    pub fn under_construction(&self, id: u32) -> bool {
        self.objects.get(&id).is_some_and(|o| o.status & under_construction_bit() != 0)
    }

    /// A template's geometry as a collider at a hypothetical place.
    fn template_collider(&self, name: &str, p: [f32; 3], angle: f32) -> Collider {
        let t = &self.templates.objects[name];
        let scale = t.number("Scale", 1.0);
        let geom = match t.values("Geometry").first().map(|g| g.to_ascii_uppercase()).as_deref() {
            Some("BOX") => Geom::Box,
            Some("CYLINDER") => Geom::Cylinder,
            _ => Geom::Sphere,
        };
        Collider {
            id: 0,
            infantry: false,
            position: p,
            radius: t.collision_radius(),
            geom,
            major: t.number("GeometryMajorRadius", 5.0) * scale,
            minor: t.number("GeometryMinorRadius", 0.0) * scale,
            bottom: 0.0,
            top: t.number("GeometryHeight", 5.0) * scale,
            angle,
            priority: 0,
            moving: false,
            enabled: true,
            immobile: true,
        }
    }
    /// `iteratePotentialCollisions` for a shape: objects whose geometry touches it.
    fn overlapping(&self, shape: &Collider) -> Vec<u32> {
        let reach = shape.radius + self.collision_radius_bound;
        self.partition
            .candidates([shape.position[0], shape.position[1]], reach, reach)
            .into_iter()
            .filter(|id| {
                let c = Collider::object(&self.objects[id]);
                crate::traffic::shapes_touch(shape, &c, 0.0)
            })
            .collect()
    }
    fn player_relationship(&self, player: u32, other: u32) -> crate::weapons::Relationship {
        use crate::weapons::Relationship::*;
        let owner = self.objects[&other].owner;
        if owner == player {
            Allies
        } else if owner == 0 || player == 0 {
            Neutral
        } else {
            Enemies
        }
    }
    /// `isRemovableForConstruction`.
    fn removable(&self, id: u32) -> bool {
        let o = &self.objects[&id];
        let t = &self.templates.objects[&o.template];
        !t.has_kind("INERT") && (t.has_kind("SHRUBBERY") || t.has_kind("CLEARED_BY_BUILD") || o.dead)
    }

    /// `iterateFootprint` with `checkSampleBuildLocation`; returns (restricted, lo z, hi z).
    fn sample_footprint(&self, name: &str, p: [f32; 3], angle: f32, step: f32, acc: &mut (bool, f32, f32)) {
        let t = &self.templates.objects[name];
        let scale = t.number("Scale", 1.0);
        let major = t.number("GeometryMajorRadius", 5.0) * scale;
        let minor = t.number("GeometryMinorRadius", 0.0) * scale;
        let boxed = t.values("Geometry").first().is_some_and(|g| g.eq_ignore_ascii_case("BOX"));
        let (half_h, half_w) = if boxed {
            (minor, major)
        } else {
            let r = crate::Simulation::bounding(t).0;
            (r, r)
        };
        let (c, s) = (ih_math::cos(angle), ih_math::sin(angle));
        let edge = self.templates.build.min_edge_distance;
        let hi_x = (self.map.width - 1) as f32 * CELL;
        let hi_y = (self.map.height - 1) as f32 * CELL;
        let mut y = -half_h;
        while y < half_h + step {
            if y > half_h {
                y = half_h;
            }
            let mut x = -half_w;
            while x < half_w + step {
                if x > half_w {
                    x = half_w;
                }
                let (wx, wy) = (p[0] + c * x - s * y, p[1] + s * x + c * y);
                if !boxed && ((wx - p[0]).powi(2) + (wy - p[1]).powi(2)).sqrt() > half_w {
                    x += step;
                    continue;
                }
                let z = self.map.ground_height(wx, wy);
                use crate::footprint::CellType;
                match self.grid.ground_kind((wx / CELL).floor() as i32, (wy / CELL).floor() as i32) {
                    None | Some(CellType::Water | CellType::Cliff | CellType::Impassable) => acc.0 = true,
                    _ => {}
                }
                acc.1 = acc.1.min(z);
                acc.2 = acc.2.max(z);
                if edge > 0.0 && (wx < edge || wx > hi_x - edge || wy < edge || wy > hi_y - edge) {
                    acc.0 = true;
                }
                x += step;
            }
            y += step;
        }
    }

    /// `isLocationLegalToBuild` with terrain, clear path and object overlap checks.
    pub fn location_legal_to_build(&self, name: &str, p: [f32; 2], angle: f32, builder: Option<u32>) -> LegalBuild {
        let (w, h) = (self.map.width as f32 * CELL, self.map.height as f32 * CELL);
        if p[0] < 0.0 || p[1] < 0.0 || p[0] >= w || p[1] >= h {
            return LegalBuild::RestrictedTerrain;
        }
        let at = [p[0], p[1], self.map.ground_height(p[0], p[1])];
        let code = self.location_clear_of_objects(name, at, angle, builder);
        if code != LegalBuild::Ok {
            return code;
        }
        if let Some(b) = builder
            && !self.templates.objects[&self.objects[&b].template].has_kind("IMMOBILE")
        {
            if !self.has_ai(b) {
                return LegalBuild::NoClearPath;
            }
            let o = &self.objects[&b];
            let radius = self.templates.objects[&o.template].collision_radius();
            let from = Waypoint { position: [o.position[0], o.position[1]], layer: o.layer };
            if !self.grid.quick_path_exists(from, Waypoint::ground(p), radius) {
                return LegalBuild::NoClearPath;
            }
        }
        if self.grid.destination(&self.map, p).layer != 0 {
            return LegalBuild::RestrictedTerrain;
        }
        let mut acc = (false, f32::MAX, f32::MIN);
        for step in [3.0 * CELL, CELL] {
            self.sample_footprint(name, at, angle, step, &mut acc);
            if acc.0 {
                return LegalBuild::RestrictedTerrain;
            }
            if acc.2 - acc.1 > self.templates.build.height_variation {
                return LegalBuild::NotFlatEnough;
            }
        }
        LegalBuild::Ok
    }

    /// `isLocationClearOfObjects` (no stealth options).
    fn location_clear_of_objects(&self, name: &str, p: [f32; 3], angle: f32, builder: Option<u32>) -> LegalBuild {
        use crate::weapons::Relationship::*;
        let shape = self.template_collider(name, p, angle);
        for them in self.overlapping(&shape) {
            let rel = builder.map_or(Neutral, |b| self.relationship(b, them));
            let t = &self.templates.objects[&self.objects[&them].template];
            if self.removable(them) || t.has_kind("MINE") || t.has_kind("INERT") {
                continue;
            }
            if t.has_kind("IMMOBILE") || self.objects[&them].disabled.any() || (builder.is_some() && rel == Enemies) {
                return LegalBuild::ObjectsInTheWay;
            }
        }
        // Structures nearby, with their factory exit lanes and extra bib widths.
        let t = &self.templates.objects[name];
        let range = 2.0 * (shape.major + shape.minor);
        let exit_width = t.number("FactoryExitWidth", 0.0);
        let my_extra = t.number("FactoryExtraBibWidth", 0.0);
        let bounds = |c: &Collider, extra: f32, minor_extra: f32| {
            if c.geom == Geom::Box {
                Collider { major: c.major + extra, minor: c.minor + minor_extra, ..*c }
            } else {
                Collider { geom: Geom::Box, major: c.major + extra, minor: c.major + extra, ..*c }
            }
        };
        let lane = |c: &Collider, width: f32| {
            let off = c.major + width / 2.0;
            let pos = [
                c.position[0] + ih_math::cos(c.angle) * off,
                c.position[1] + ih_math::sin(c.angle) * off,
                c.position[2],
            ];
            let minor = if c.geom == Geom::Box { c.minor } else { c.major };
            Collider { geom: Geom::Box, position: pos, major: width / 2.0, minor, ..*c }
        };
        let my_bounds = bounds(&shape, my_extra, my_extra);
        let my_lane = (exit_width > 0.0).then(|| lane(&shape, exit_width));
        for them in self.partition.candidates([p[0], p[1]], range + self.collision_radius_bound, range) {
            let o = &self.objects[&them];
            let ot = &self.templates.objects[&o.template];
            if !ot.has_kind("STRUCTURE") || self.removable(them) {
                continue;
            }
            let r = Self::bounding(ot).0;
            let d = ((o.position[0] - p[0]).powi(2) + (o.position[1] - p[1]).powi(2)).sqrt() - r;
            if d > range {
                continue;
            }
            let theirs = Collider::object(o);
            let his_extra = ot.number("FactoryExtraBibWidth", 0.0);
            let his_width = ot.number("FactoryExitWidth", 0.0);
            // As in the original, his box grows its minor radius by my extra width.
            let his_bounds = bounds(&theirs, his_extra, my_extra);
            if crate::traffic::shapes_touch(&his_bounds, &my_bounds, 0.0) {
                return LegalBuild::ObjectsInTheWay;
            }
            if my_lane.is_none() && his_width <= 0.0 && his_extra == 0.0 && my_extra == 0.0 {
                continue;
            }
            if ot.has_kind("IMMOBILE") {
                let his_lane = (his_width > 0.0).then(|| lane(&theirs, his_width));
                if my_lane.is_some_and(|l| crate::traffic::shapes_touch(&his_bounds, &l, 0.0))
                    || his_lane.is_some_and(|l| crate::traffic::shapes_touch(&l, &my_bounds, 0.0))
                    || my_lane.zip(his_lane).is_some_and(|(a, b)| crate::traffic::shapes_touch(&b, &a, 0.0))
                {
                    return LegalBuild::ObjectsInTheWay;
                }
            }
        }
        LegalBuild::Ok
    }

    // --- pathfinder ---

    pub(crate) fn obstacle_key(&self, id: u32) -> u32 {
        self.objects[&id].map_index.map_or(0x8000_0000 | id, |i| i as u32 + 1)
    }
    fn refresh_views(&mut self) {
        self.grid.invalidate_zones();
        let masks: Vec<u8> = self.views.keys().copied().collect();
        for m in masks {
            let v = self.grid.view(m);
            self.views.insert(m, v);
        }
    }
    /// `addObjectToPathfindMap` for objects created during the game.
    pub(crate) fn add_to_pathfind_map(&mut self, id: u32) {
        let o = &self.objects[&id];
        let t = &self.templates.objects[&o.template];
        let ground = self.map.ground_height(o.position[0], o.position[1]);
        let key = self.obstacle_key(id);
        if let Some(f) =
            crate::map_objects::footprint(t, [o.position[0], o.position[1]], o.angle, o.position[2] - ground, key)
        {
            let g = &mut self.grid;
            g.obstacles.insert(&mut g.ground, &f);
            self.refresh_views();
        }
    }
    /// `removeObjectFromPathfindMap`.
    pub(crate) fn remove_from_pathfind_map(&mut self, id: u32) {
        let key = self.obstacle_key(id);
        let g = &mut self.grid;
        if g.obstacles.remove(&mut g.ground, key) {
            self.refresh_views();
        }
    }

    // --- placing a building ---

    /// `BuildAssistant::buildObjectNow` for a dozer (`MSG_DOZER_CONSTRUCT`).
    pub(crate) fn build_object_now(
        &mut self,
        dozer: u32,
        what: &str,
        p: [f32; 2],
        angle: f32,
    ) -> Result<u32, SimError> {
        let fail = |why: &str| Err(SimError(format!("object {dozer} cannot build {what}: {why}")));
        if !self.objects.contains_key(&dozer) || !self.templates.objects.contains_key(what) {
            return fail("missing");
        }
        if self.objects[&dozer].dozer.is_none() {
            return fail("not a dozer");
        }
        let player = self.objects[&dozer].owner;
        if !self.possible_to_make_unit(dozer, what) {
            return fail("NoPrereq");
        }
        let at = [p[0], p[1], self.map.ground_height(p[0], p[1])];
        // clearRemovableForConstruction
        let shape = self.template_collider(what, at, angle);
        for them in self.overlapping(&shape) {
            let t = &self.templates.objects[&self.objects[&them].template];
            if self.removable(them) && !t.has_kind("ALWAYS_SELECTABLE") {
                self.destroy_object(them);
            }
        }
        // moveObjectsForConstruction: a human player's build fails on anything unmovable.
        if !self.move_objects_for_construction(what, at, angle, player) {
            return fail("ObjectsInTheWay");
        }
        // aiIdle, then DozerAIUpdate::construct.
        self.ai_idle(dozer);
        self.construct(dozer, what, p, angle)
    }

    fn move_objects_for_construction(&mut self, what: &str, at: [f32; 3], angle: f32, player: u32) -> bool {
        let mut shape = self.template_collider(what, at, angle);
        if shape.geom != Geom::Box {
            shape = Collider { geom: Geom::Box, minor: shape.major, top: 10.0, ..shape };
        }
        let radius = (shape.major.powi(2) + shape.minor.powi(2)).sqrt() * 1.4;
        let mut unmovable = false;
        for them in self.overlapping(&shape) {
            let t = &self.templates.objects[&self.objects[&them].template];
            if t.has_kind("MINE") || t.has_kind("INERT") || t.has_kind("ALWAYS_SELECTABLE") || self.removable(them) {
                continue;
            }
            use crate::weapons::Relationship::*;
            if matches!(self.player_relationship(player, them), Neutral | Allies) && self.has_ai(them) {
                let varied = self.rng.real(0.5, 1.5) * radius;
                let dir = self.rng.real(-std::f32::consts::PI, std::f32::consts::PI);
                let to = [at[0] + varied * ih_math::cos(dir), at[1] + varied * ih_math::sin(dir)];
                if self.begin_move(them, to, AiState::MoveTo, Default::default()).is_ok() {
                    self.objects.get_mut(&them).unwrap().ai.cmd_from_ai = true;
                }
            } else {
                unmovable = true;
            }
        }
        !unmovable
    }

    /// `BuildAssistant::isPossibleToMakeUnit` for a dozer's construct button.
    fn possible_to_make_unit(&self, builder: u32, what: &str) -> bool {
        let Some(set) = self.command_set_of(builder).and_then(|s| self.templates.command_sets.get(&s)) else {
            return false;
        };
        let found = set
            .iter()
            .filter_map(|b| self.templates.command_buttons.get(b))
            .any(|(command, object)| matches!(command.as_str(), "UNIT_BUILD" | "DOZER_CONSTRUCT") && object == what);
        found && self.can_make_unit(builder, what) != crate::production::CanMake::NoPrereq
    }

    /// `aiIdle(CMD_FROM_AI)`.
    fn ai_idle(&mut self, id: u32) {
        if self.objects[&id].ai.active() {
            self.end_order(id, Outcome::Interrupted);
        }
        self.objects.get_mut(&id).unwrap().ai.cmd_from_ai = true;
    }

    /// `DozerAIUpdate::construct` (not a rebuild).
    fn construct(&mut self, dozer: u32, what: &str, p: [f32; 2], angle: f32) -> Result<u32, SimError> {
        let fail = |why: String| Err(SimError(format!("object {dozer} cannot build {what}: {why}")));
        let can = self.can_make_unit(dozer, what);
        if can != crate::production::CanMake::Ok {
            return fail(format!("{can:?}"));
        }
        let legal = self.location_legal_to_build(what, p, angle, Some(dozer));
        if legal != LegalBuild::Ok {
            return fail(format!("{legal:?}"));
        }
        let player = self.objects[&dozer].owner;
        let z = self.map.ground_height(p[0], p[1]);
        let id = self.create_with_status(what, [p[0], p[1], z], angle, 0, player, under_construction_bit())?;
        // WorkerAIUpdate::construct leaves supply truck duty first.
        self.exiting_supply_truck_state(dozer);
        let cost = self.cost_to_build(&self.templates.objects[what], player);
        self.player(player).withdraw(cost);
        self.add_to_pathfind_map(id);
        let o = self.objects.get_mut(&id).unwrap();
        o.construction = Some(0.0);
        o.builder = Some(dozer);
        if let Some(h) = &o.health {
            let delta = -h.current + 1.0;
            self.change_health(id, delta);
        }
        self.new_task(dozer, TASK_BUILD, id);
        Ok(id)
    }

    /// `privateResumeConstruction` (`MSG_RESUME_CONSTRUCTION`).
    pub(crate) fn resume_construction(&mut self, dozer: u32, target: u32) -> Result<(), SimError> {
        let ok = self.objects.get(&dozer).is_some_and(|d| d.dozer.is_some())
            && self.under_construction(target)
            && self.objects[&target].owner == self.objects[&dozer].owner;
        if !ok {
            return Err(SimError(format!("object {dozer} cannot resume building {target}")));
        }
        if self.dozer(dozer).current.is_none() {
            self.ai_idle(dozer);
        }
        self.new_task(dozer, TASK_BUILD, target);
        Ok(())
    }

    fn dozer(&self, id: u32) -> &Dozer {
        self.objects[&id].dozer.as_ref().unwrap()
    }
    fn dozer_mut(&mut self, id: u32) -> &mut Dozer {
        self.objects.get_mut(&id).unwrap().dozer.as_mut().unwrap()
    }

    /// `findGoodBuildOrRepairPosition`.
    fn good_build_position(&mut self, me: u32, target: u32) -> [f32; 3] {
        let (ours, theirs) = (self.objects[&me].position, self.objects[&target].position);
        let v = [ours[0] - theirs[0], ours[1] - theirs[1], ours[2] - theirs[2]];
        let len = ih_math::sqrt(v[0] * v[0] + v[1] * v[1] + v[2] * v[2]);
        let t = &self.templates.objects[&self.objects[&target].template];
        let major = t.number("GeometryMajorRadius", 5.0) * t.number("Scale", 1.0);
        let mut p = theirs;
        if len > 0.0 {
            for k in 0..3 {
                p[k] += v[k] / len * (major / 2.0);
            }
        }
        self.find_position_around(p, 0.0, 100.0, None, Some(me), 10.0).unwrap_or(p)
    }

    /// `DozerAIUpdate::newTask`.
    fn new_task(&mut self, dozer: u32, task: usize, target: u32) {
        if self.dozer(dozer).tasks[task].target.is_some() {
            self.cancel_task(dozer, task);
        }
        let worker = self.dozer(dozer).worker.is_some();
        if worker && let Some(t) = self.objects.get_mut(&dozer).unwrap().truck.as_mut() {
            // "If we are dozing, we don't want any supply truck stuff going on."
            t.preferred = None;
        }
        let position = self.good_build_position(dozer, target);
        self.objects.get_mut(&target).unwrap().builder = Some(dozer);
        let tp = self.objects[&target].position;
        let mut off = [position[0] - tp[0], position[1] - tp[1]];
        let len = ih_math::length(off);
        if len > 0.0 {
            off = [off[0] / len * 5.0 * CELL, off[1] / len * 5.0 * CELL];
        }
        // Workers end where they built; dozers back away five cells.
        let end = if worker { position } else { [position[0] + off[0], position[1] + off[1], position[2]] };
        let frame = self.frame;
        let d = self.dozer_mut(dozer);
        d.tasks[task] =
            Task { target: Some(target), order_frame: frame, dock: [Some(position), Some(position), Some(end)] };
        self.reset_dozer_machine(dozer);
        // WorkerAIUpdate::newTask forces the switch out of supply truck duty.
        if self.dozer(dozer).worker == Some(true) {
            self.ai_idle(dozer);
            self.dozer_mut(dozer).worker = Some(false);
            self.worker_transitions(dozer);
        }
    }
    /// `cancelTask`.
    fn cancel_task(&mut self, dozer: u32, task: usize) {
        self.internal_cancel_task(dozer, task);
        self.reset_dozer_machine(dozer);
    }
    fn internal_task_complete(&mut self, dozer: u32, task: usize) {
        self.dozer_mut(dozer).tasks[task] = Task::default();
    }
    fn internal_cancel_task(&mut self, dozer: u32, task: usize) {
        self.dozer_mut(dozer).tasks[task] = Task::default();
        self.ai_idle(dozer);
    }

    /// Player orders to a dozer cancel its current task (`DozerAIUpdate::aiDoCommand`).
    pub(crate) fn dozer_player_order(&mut self, id: u32) {
        let Some(d) = self.objects.get(&id).and_then(|o| o.dozer.as_ref()) else { return };
        if let Some(t) = d.current {
            self.cancel_task(id, t);
        }
    }
    /// After a player order has been given (`resetToDefaultState`).
    pub(crate) fn dozer_after_player_order(&mut self, id: u32) {
        if self.objects.get(&id).is_some_and(|o| o.dozer.is_some()) {
            self.reset_dozer_machine(id);
        }
    }

    // --- WorkerStateMachine ---

    /// `WorkerAIUpdate::update` after the AI's own update: pick a brain, run it.
    pub(crate) fn worker_update(&mut self, id: u32) {
        if self.objects[&id].dead {
            return;
        }
        self.worker_transitions(id);
        if self.dozer(id).worker == Some(true) {
            self.truck_update(id);
        } else {
            self.dozer_update(id);
        }
    }
    /// `checkForTransitions` of the worker machine, following each change
    /// (`supplyTruckSubMachineWantsToEnter` / `supplyTruckSubMachineReadyToLeave`).
    /// The states' onExit hooks never run in the original: their signature does
    /// not override `State::onExit`.
    fn worker_transitions(&mut self, id: u32) {
        for _ in 0..20 {
            let o = &self.objects[&id];
            let t = o.truck.as_ref().unwrap();
            let wants = t.force_wanting || o.ai.state == AiState::Dock;
            let truck = self.dozer(id).worker == Some(true);
            if !truck && wants {
                self.dozer_mut(id).worker = Some(true);
            } else if truck && !wants && t.state == crate::supply::TruckState::Busy {
                self.dozer_mut(id).worker = Some(false);
            } else {
                return;
            }
        }
    }
    /// `WorkerAIUpdate::exitingSupplyTruckState`.
    pub(crate) fn exiting_supply_truck_state(&mut self, id: u32) {
        if self.objects[&id].dozer.as_ref().and_then(|d| d.worker) == Some(true) {
            self.ai_idle(id);
            self.dozer_mut(id).worker = Some(false);
            self.worker_transitions(id);
        }
    }

    // --- DozerPrimaryStateMachine ---

    /// `resetToDefaultState`: the current state exits, idle is entered.
    fn reset_dozer_machine(&mut self, id: u32) {
        let p = self.dozer(id).primary;
        self.primary_exit(id, p);
        self.dozer_mut(id).primary = Primary::Idle;
        let r = self.primary_enter(id, Primary::Idle);
        self.primary_after(id, Primary::Idle, r);
    }
    /// `DozerAIUpdate::update`, after the AI's own update.
    pub(crate) fn dozer_update(&mut self, id: u32) {
        if self.objects[&id].dead {
            return;
        }
        let p = self.dozer(id).primary;
        let r = self.primary_update(id, p);
        self.primary_after(id, p, r);
    }
    fn primary_after(&mut self, id: u32, mut state: Primary, mut r: Ret) {
        for _ in 0..20 {
            let next = match r {
                Ret::Continue => {
                    if state != Primary::Idle {
                        return;
                    }
                    let d = self.dozer(id);
                    // isBuildMostImportant (repair and fortify are not ported).
                    if !self.objects[&id].ai.active() && d.most_recent() == Some(TASK_BUILD) {
                        Primary::Build
                    } else {
                        return;
                    }
                }
                Ret::Success | Ret::Failure => Primary::Idle,
            };
            self.primary_exit(id, state);
            state = next;
            self.dozer_mut(id).primary = next;
            r = self.primary_enter(id, next);
        }
    }
    fn primary_enter(&mut self, id: u32, state: Primary) -> Ret {
        let frame = self.frame;
        let d = self.dozer_mut(id);
        match state {
            Primary::Idle => {
                d.idle_since = frame;
                Ret::Continue
            }
            _ => {
                d.current = Some(state as usize - 1);
                // The action machine starts over (`resetToDefaultState`).
                d.action = Action::PickActionPos;
                Ret::Continue
            }
        }
    }
    fn primary_exit(&mut self, id: u32, state: Primary) {
        if state != Primary::Idle {
            self.dozer_mut(id).current = None;
        }
    }
    fn primary_update(&mut self, id: u32, state: Primary) -> Ret {
        match state {
            Primary::Idle => {
                // Bored dozers (repair, mine clearing) are not ported; the timer is kept.
                if self.objects[&id].ai.active() {
                    let frame = self.frame;
                    self.dozer_mut(id).idle_since = frame;
                }
                Ret::Continue
            }
            Primary::Build => self.action_update(id, TASK_BUILD),
            _ => Ret::Failure,
        }
    }

    // --- DozerActionStateMachine ---

    fn action_update(&mut self, id: u32, task: usize) -> Ret {
        let mut state = self.dozer(id).action;
        let mut r = self.action_state_update(id, task, state);
        for _ in 0..20 {
            let next = match (state, r) {
                (_, Ret::Continue) => return Ret::Continue,
                (Action::PickActionPos, Ret::Success) => Action::MoveToActionPos,
                (Action::PickActionPos, Ret::Failure) => return Ret::Failure,
                (Action::MoveToActionPos, Ret::Success) => Action::DoAction,
                (Action::MoveToActionPos, Ret::Failure) => Action::PickActionPos,
                (Action::DoAction, r) => return r,
            };
            state = next;
            self.dozer_mut(id).action = next;
            r = self.action_enter(id, task, next);
        }
        Ret::Failure
    }
    fn action_enter(&mut self, id: u32, _task: usize, state: Action) -> Ret {
        if state == Action::DoAction {
            let frame = self.frame;
            let d = self.dozer_mut(id);
            d.enter_frame = frame;
            d.build_sub = BuildSub::SelectDockLocation;
        }
        Ret::Continue
    }
    fn action_state_update(&mut self, id: u32, task: usize, state: Action) -> Ret {
        match state {
            Action::PickActionPos => {
                let Some(goal) = self.dozer(id).tasks[task].target.filter(|g| self.objects.contains_key(g)) else {
                    self.dozer_mut(id).goal = None;
                    self.cancel_task(id, task);
                    return Ret::Failure;
                };
                let p = match self.dozer(id).tasks[task].dock[0] {
                    Some(p) => p,
                    None => self.objects[&goal].position,
                };
                let d = self.dozer_mut(id);
                d.goal = Some(goal);
                d.goal_position = p;
                self.objects.get_mut(&id).unwrap().ai.ignore_obstacle = Some(goal);
                self.ai_move(id, [p[0], p[1]]);
                Ret::Success
            }
            Action::MoveToActionPos => {
                let Some(goal) = self.dozer(id).goal.filter(|g| self.objects.contains_key(g)) else {
                    return Ret::Failure;
                };
                if task == TASK_BUILD && self.objects[&goal].builder != Some(id) {
                    self.internal_task_complete(id, task);
                    self.dozer_mut(id).goal = None;
                    return Ret::Failure;
                }
                let gp = self.dozer(id).goal_position;
                let d2 = self.attack_distance_sqr(id, None, None, gp);
                let r = Self::bounding(&self.templates.objects[&self.objects[&id].template]).1;
                if d2 <= MIN_ACTION_TOLERANCE.max(r + 15.0).powi(2) {
                    return Ret::Success;
                }
                if !self.objects[&id].ai.active() {
                    return Ret::Failure;
                }
                Ret::Continue
            }
            Action::DoAction => self.do_build(id, task),
        }
    }

    /// `aiMoveToPosition(CMD_FROM_AI)`, keeping the dozer's ignored obstacle.
    fn ai_move(&mut self, id: u32, to: [f32; 2]) {
        let ignore = self.objects[&id].ai.ignore_obstacle;
        if self.begin_move_ignoring(id, to, AiState::MoveTo, Default::default(), ignore).is_ok() {
            self.objects.get_mut(&id).unwrap().ai.cmd_from_ai = true;
        }
    }

    /// The build half of `DozerActionDoActionState::update`.
    fn do_build(&mut self, id: u32, task: usize) -> Ret {
        let Some(goal) = self.dozer(id).goal.filter(|g| self.objects.contains_key(g)) else { return Ret::Failure };
        if task != TASK_BUILD {
            return Ret::Failure;
        }
        if self.objects[&id].owner != self.objects[&goal].owner {
            return Ret::Failure;
        }
        if self.dozer(id).build_sub == BuildSub::SelectDockLocation {
            if let Some(p) = self.dozer(id).tasks[task].dock[1] {
                self.ai_move(id, [p[0], p[1]]);
            }
            self.dozer_mut(id).build_sub = BuildSub::MovingToDockLocation;
        }
        if self.dozer(id).build_sub == BuildSub::MovingToDockLocation && !self.objects[&id].ai.active() {
            self.dozer_mut(id).build_sub = BuildSub::DoBuildAtDock;
        }
        if self.dozer(id).build_sub != BuildSub::DoBuildAtDock {
            return Ret::Continue;
        }
        let owner = self.objects[&id].owner;
        let t = &self.templates.objects[&self.objects[&goal].template];
        let frames = self.time_to_build(t, owner);
        let step = 100.0 / frames as f32;
        let max = self.objects[&goal].health.as_ref().map_or(0.0, |h| h.max);
        let o = self.objects.get_mut(&goal).unwrap();
        let percent = o.construction.unwrap_or(100.0) + step;
        o.construction = Some(percent);
        o.builder = Some(id);
        self.change_health(goal, max / frames as f32);
        if percent < 100.0 {
            return Ret::Continue;
        }
        // Construction complete.
        let o = self.objects.get_mut(&goal).unwrap();
        o.status &= !under_construction_bit();
        o.construction = None;
        // onStructureConstructionComplete: back into the pathfinder; power now counts.
        self.remove_from_pathfind_map(goal);
        self.add_to_pathfind_map(goal);
        self.update_upgrade_modules(goal);
        let end = self.dozer(id).tasks[task].dock[2].unwrap_or(self.objects[&id].position);
        let mut to = [end[0], end[1]];
        let layer = self.grid.destination(&self.map, to).layer;
        self.adjust_destination(id, &mut to, layer);
        self.ai_move(id, to);
        self.internal_task_complete(id, task);
        self.dozer_mut(id).goal = None;
        Ret::Success
    }
}
