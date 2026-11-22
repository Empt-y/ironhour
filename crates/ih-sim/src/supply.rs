//! Supply gathering: docks, supply warehouses and centres, and supply trucks.
//!
//! Sources: `GameLogic/Object/Update/DockUpdate/{DockUpdate,SupplyWarehouseDockUpdate,
//! SupplyCenterDockUpdate}.cpp`, `GameLogic/AI/AIDock.cpp` (`AIDockMachine`),
//! `AIStates.cpp` (`AIDockState`), `AIUpdate.cpp` (`privateDock`),
//! `GameLogic/Object/Update/AIUpdate/SupplyTruckAIUpdate.cpp` (`SupplyTruckStateMachine`),
//! `Common/RTS/ResourceGatheringManager.cpp`, `ActionManager::canTransferSuppliesAt`,
//! `Player::findClosestByKindOf`, `PartitionManager::findPositionAround`/`tryPosition`,
//! and `W3DModelDraw::getPristineBonePositionsForConditionState` for dock bones.
//!
//! Not ported yet: workers (`WorkerAIUpdate`), the Chinook's own flight states
//! (it uses the truck machine), supply drones, stealth grants, supply boosts
//! from upgrades, crippled docks, sold or under-construction docks, shroud,
//! computer players' doubled scan distance, rally points after docking, and the
//! warehouse's damaged-model bones (bones come from the default model). Docks
//! are found in object order rather than through each player's registration
//! lists, which is the same order for objects that exist from the start.
use crate::{
    Simulation,
    ai::{AiState, Outcome},
    navigation::CELL,
    terrain::Waypoint,
    weapons::Relationship,
};

/// A model's dock bones (`DockStart`, `DockAction`, `DockEnd`, `DockWaitingNN`),
/// in model space; missing ones are zero, as the original leaves them.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct DockBones {
    pub start: [f32; 3],
    pub action: [f32; 3],
    pub end: [f32; 3],
    pub waiting: Vec<[f32; 3]>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum DockKind {
    Warehouse,
    Center,
}

/// `DockUpdate` with the warehouse's boxes.
#[derive(Clone, Debug, PartialEq)]
pub struct Dock {
    pub kind: DockKind,
    /// `NumberApproachPositions` (-1 for as many as needed).
    pub approach_count: i32,
    pub passthrough: bool,
    pub starting_boxes: i32,
    pub delete_when_empty: bool,
    pub open: bool,
    pub loaded: bool,
    pub approach_bones: i32,
    pub active: Option<u32>,
    pub inside: bool,
    pub enter: [f32; 3],
    pub dock: [f32; 3],
    pub exit: [f32; 3],
    pub approach: Vec<[f32; 3]>,
    pub owners: Vec<Option<u32>>,
    pub reached: Vec<bool>,
    pub boxes: i32,
}
impl Dock {
    pub fn from_module(class: &str, b: &ih_ini::Block) -> Option<Self> {
        let kind = match class {
            "SupplyWarehouseDockUpdate" => DockKind::Warehouse,
            "SupplyCenterDockUpdate" => DockKind::Center,
            _ => return None,
        };
        let mut d = Self {
            kind,
            approach_count: 0,
            passthrough: false,
            starting_boxes: 1,
            delete_when_empty: false,
            open: true,
            loaded: false,
            approach_bones: -1,
            active: None,
            inside: false,
            enter: [0.0; 3],
            dock: [0.0; 3],
            exit: [0.0; 3],
            approach: vec![],
            owners: vec![],
            reached: vec![],
            boxes: 0,
        };
        for f in b.fields() {
            let v = f.values();
            let first = v.first().map(String::as_str).unwrap_or("");
            let yes = first.eq_ignore_ascii_case("yes");
            match f.name.as_str() {
                "NumberApproachPositions" => d.approach_count = first.parse().unwrap_or(0),
                "AllowsPassthrough" => d.passthrough = yes,
                "StartingBoxes" if kind == DockKind::Warehouse => d.starting_boxes = first.parse().unwrap_or(1),
                "DeleteWhenEmpty" if kind == DockKind::Warehouse => d.delete_when_empty = yes,
                _ => {}
            }
        }
        let n = if d.approach_count == -1 { 10 } else { d.approach_count.max(0) as usize };
        d.approach = vec![[0.0; 3]; n];
        d.owners = vec![None; n];
        d.reached = vec![false; n];
        d.boxes = if kind == DockKind::Warehouse { d.starting_boxes } else { 0 };
        Some(d)
    }
}

/// `SupplyTruckStateMachine` states, with their original IDs.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum TruckState {
    Idle = 0,
    #[default]
    Busy = 1,
    Wanting = 2,
    Regrouping = 3,
    Docking = 4,
}

/// `SupplyTruckAIUpdate`.
#[derive(Clone, Debug, PartialEq)]
pub struct Truck {
    pub max_boxes: i32,
    pub center_delay: u32,
    pub warehouse_delay: u32,
    pub scan_distance: f32,
    pub state: TruckState,
    pub boxes: i32,
    pub preferred: Option<u32>,
    pub force_wanting: bool,
    pub force_busy: bool,
}
impl Truck {
    pub fn from_module(class: &str, b: &ih_ini::Block) -> Option<Self> {
        if !matches!(class, "SupplyTruckAIUpdate" | "ChinookAIUpdate" | "WorkerAIUpdate") {
            return None;
        }
        let mut t = Self {
            max_boxes: 0,
            center_delay: 0,
            warehouse_delay: 0,
            scan_distance: 0.0,
            // initDefaultState: the first defined state.
            state: TruckState::Busy,
            boxes: 0,
            preferred: None,
            force_wanting: false,
            force_busy: false,
        };
        for f in b.fields() {
            let v = f.values();
            let n = v.first().and_then(|s| ih_ini::values::scan_real(s.as_bytes()).ok()).unwrap_or(0.0);
            match f.name.as_str() {
                "MaxBoxes" => t.max_boxes = n as i32,
                "SupplyCenterActionDelay" => t.center_delay = crate::death::duration(n),
                "SupplyWarehouseActionDelay" => t.warehouse_delay = crate::death::duration(n),
                "SupplyWarehouseScanDistance" => t.scan_distance = n,
                _ => {}
            }
        }
        Some(t)
    }
}

/// `AIDockMachine` states.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum DockSub {
    Approach = 0,
    WaitForClearance = 1,
    AdvancePosition = 2,
    MoveToEntry = 3,
    MoveToDock = 4,
    ProcessDock = 5,
    MoveToExit = 6,
    MoveToRally = 7,
}
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Next {
    State(DockSub),
    Done(bool),
}
impl DockSub {
    /// (on success, on failure) from `AIDockMachine::AIDockMachine`.
    fn next(self) -> (Next, Next) {
        use DockSub::*;
        use Next::{Done, State};
        match self {
            Approach => (State(WaitForClearance), Done(false)),
            WaitForClearance => (State(MoveToEntry), Done(false)),
            AdvancePosition => (State(WaitForClearance), Done(false)),
            MoveToEntry => (State(MoveToDock), State(MoveToExit)),
            MoveToDock => (State(ProcessDock), State(MoveToExit)),
            ProcessDock => (State(MoveToExit), State(MoveToExit)),
            MoveToExit => (State(MoveToRally), Done(false)),
            MoveToRally => (Done(true), Done(false)),
        }
    }
    fn moves(self) -> bool {
        !matches!(self, DockSub::WaitForClearance | DockSub::ProcessDock)
    }
}

/// The `AI_DOCK` state's sub-machine.
#[derive(Clone, Debug, PartialEq)]
pub struct DockMachine {
    pub goal: u32,
    /// `None` once the machine has exited.
    pub sub: Option<DockSub>,
    /// `m_approachPosition`.
    pub approach: i32,
    pub enter_frame: u32,
    pub next_action: u32,
    pub goal_position: [f32; 3],
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Ret {
    Continue,
    Success,
    Failure,
}

/// `PATHFIND_CELL_SIZE_F * 0.4`, truncated by the integer random call.
const JITTER: i32 = 4;

impl Simulation {
    fn dock_of(&self, id: u32) -> Option<&Dock> {
        self.objects.get(&id).and_then(|o| o.dock.as_ref())
    }
    fn dock_mut(&mut self, id: u32) -> Option<&mut Dock> {
        self.objects.get_mut(&id).and_then(|o| o.dock.as_mut())
    }
    fn to_world(&self, id: u32, p: [f32; 3]) -> [f32; 3] {
        let o = &self.objects[&id];
        crate::production::transform(o.position, o.angle, p)
    }
    fn airborne(&self, id: u32) -> bool {
        let o = &self.objects[&id];
        self.templates.objects[&o.template]
            .locomotor()
            .and_then(|n| self.templates.locomotors.get(n))
            .is_some_and(|l| l.surfaces.iter().any(|s| s.eq_ignore_ascii_case("AIR")))
    }

    /// `DockUpdate::loadDockPositions` (bones from the default model).
    fn load_dock_positions(&mut self, id: u32) {
        let o = &self.objects[&id];
        let t = &self.templates.objects[&o.template];
        let ignore = t.has_kind("IGNORE_DOCKING_BONES");
        let bones = self.templates.dock_bones.get(&o.template).cloned().unwrap_or_default();
        let d = self.dock_mut(id).unwrap();
        d.loaded = true;
        if ignore {
            d.approach_bones = 0;
            return;
        }
        d.enter = bones.start;
        d.dock = bones.action;
        d.exit = bones.end;
        if d.approach_count != -1 {
            let n = d.approach_count.max(0) as usize;
            d.approach_bones = bones.waiting.len().min(n) as i32;
            for (i, a) in d.approach.iter_mut().enumerate().take(n) {
                *a = bones.waiting.get(i).copied().unwrap_or_default();
            }
        } else {
            d.approach_bones = 0;
        }
    }
    fn ensure_loaded(&mut self, id: u32) {
        if self.dock_of(id).is_some_and(|d| !d.loaded) {
            self.load_dock_positions(id);
        }
    }

    /// `DockUpdate::computeApproachPosition`.
    fn approach_position(&mut self, dock: u32, i: usize, docker: u32) -> [f32; 3] {
        self.ensure_loaded(dock);
        let d = self.dock_of(dock).unwrap();
        let mut p = self.to_world(dock, d.approach[i]);
        if d.approach_bones == 0 {
            let (us, them) = (self.objects[&dock].position, self.objects[&docker].position);
            let v = [them[0] - us[0], them[1] - us[1], them[2] - us[2]];
            let len = ih_math::sqrt(v[0] * v[0] + v[1] * v[1] + v[2] * v[2]);
            let major = self.templates.objects[&self.objects[&dock].template].number("GeometryMajorRadius", 5.0)
                * self.templates.objects[&self.objects[&dock].template].number("Scale", 1.0);
            if len > 0.0 {
                for k in 0..3 {
                    p[k] += v[k] / len * (major / 2.0);
                }
            }
        }
        let ignore = self.airborne(docker).then_some(dock);
        self.find_position_around(p, 0.0, 100.0, ignore, Some(docker), 1e10).unwrap_or(p)
    }

    /// `PartitionManager::findPositionAround` with a random start angle.
    pub(crate) fn find_position_around(
        &mut self,
        center: [f32; 3],
        min: f32,
        max: f32,
        ignore: Option<u32>,
        path_for: Option<u32>,
        max_z: f32,
    ) -> Option<[f32; 3]> {
        let (w, h) = (self.map.width as f32 * CELL, self.map.height as f32 * CELL);
        if center[0] < 0.0 || center[1] < 0.0 || center[0] >= w || center[1] >= h {
            return Some(center);
        }
        let start = self.rng.real(0.0, ih_math::TAU);
        let mut dist = min;
        while dist <= max {
            let spacing = if dist == min { ih_math::TAU } else { (5.0 / (dist + 1.0)) * (ih_math::TAU / 6.0) };
            let samples = ((ih_math::TAU / spacing) / 2.0).ceil() as i32;
            for i in 0..samples {
                if let Some(p) = self.try_position(center, dist, start + spacing * i as f32, ignore, path_for, max_z) {
                    return Some(p);
                }
                if i != 0
                    && let Some(p) =
                        self.try_position(center, dist, start - spacing * i as f32, ignore, path_for, max_z)
                {
                    return Some(p);
                }
            }
            dist += 5.0;
        }
        None
    }

    /// `PartitionManager::tryPosition` (ground layer, no flags).
    fn try_position(
        &self,
        center: [f32; 3],
        dist: f32,
        angle: f32,
        ignore: Option<u32>,
        path_for: Option<u32>,
        max_z: f32,
    ) -> Option<[f32; 3]> {
        use crate::footprint::CellType;
        let x = dist * ih_math::cos(angle) + center[0];
        let y = dist * ih_math::sin(angle) + center[1];
        let z = self.map.ground_height(x, y);
        if (z - center[2]).abs() > max_z {
            return None;
        }
        let (cx, cy) = ((x / CELL).floor() as i32, (y / CELL).floor() as i32);
        let kind = self.grid.ground_kind(cx, cy)?;
        // Cliffs, impassable cells and water (`isUnderwater`) are refused.
        if matches!(kind, CellType::Cliff | CellType::Impassable | CellType::Water) {
            return None;
        }
        // iteratePotentialCollisions with a 5-unit sphere.
        for other in self.partition.candidates([x, y], 5.0, 5.0 + self.collision_radius_bound) {
            if Some(other) == ignore || Some(other) == path_for {
                continue;
            }
            let o = &self.objects[&other];
            if o.status & crate::body::status::NO_COLLISIONS != 0 {
                continue;
            }
            let r = Self::bounding(&self.templates.objects[&o.template]).0;
            if (o.position[0] - x).powi(2) + (o.position[1] - y).powi(2) < (r + 5.0).powi(2) {
                return None;
            }
        }
        if let Some(src) = path_for
            && self.has_ai(src)
        {
            let s = &self.objects[&src];
            let radius = self.templates.objects[&s.template].collision_radius();
            let a = Waypoint { position: [s.position[0], s.position[1]], layer: s.layer };
            if !self.grid.quick_path_exists(a, Waypoint::ground([x, y]), radius) {
                return None;
            }
        }
        Some([x, y, z])
    }

    // --- DockUpdate ---

    fn is_clear_to_approach(&self, dock: u32, docker: u32) -> bool {
        let d = self.dock_of(dock).unwrap();
        d.approach_count == -1 || d.owners.iter().any(|o| o.is_none() || *o == Some(docker))
    }
    /// `reserveApproachPosition`: the docker's slot, or the first free one.
    fn reserve_approach(&mut self, dock: u32, docker: u32) -> Option<(usize, [f32; 3])> {
        self.ensure_loaded(dock);
        let d = self.dock_of(dock).unwrap();
        let slot =
            d.owners.iter().position(|o| *o == Some(docker)).or_else(|| d.owners.iter().position(Option::is_none));
        let i = match slot {
            Some(i) => {
                self.dock_mut(dock).unwrap().owners[i] = Some(docker);
                i
            }
            None if d.approach_count == -1 => {
                let d = self.dock_mut(dock).unwrap();
                d.approach.push([0.0; 3]);
                d.owners.push(Some(docker));
                d.reached.push(false);
                self.load_dock_positions(dock);
                self.dock_of(dock).unwrap().owners.len() - 1
            }
            None => return None,
        };
        let p = self.approach_position(dock, i, docker);
        Some((i, p))
    }
    /// `advanceApproachPosition`.
    fn advance_approach(&mut self, dock: u32, docker: u32, index: i32) -> Option<(usize, [f32; 3])> {
        self.ensure_loaded(dock);
        if index <= 0 {
            return None;
        }
        let i = index as usize;
        let d = self.dock_mut(dock).unwrap();
        if d.owners[i - 1].is_some() {
            return None;
        }
        d.owners[i - 1] = Some(docker);
        d.reached[i - 1] = false;
        d.owners[i] = None;
        d.reached[i] = false;
        let p = self.approach_position(dock, i - 1, docker);
        Some((i - 1, p))
    }
    fn is_clear_to_advance(&self, dock: u32, docker: u32, index: i32) -> bool {
        if index < 0 {
            return false;
        }
        let d = self.dock_of(dock).unwrap();
        let i = index as usize;
        d.owners[i] == Some(docker) && d.reached[i] && i > 0 && d.owners[i - 1].is_none()
    }
    /// `getEnterPosition`, `getDockPosition`, `getExitPosition`: the bone, or
    /// where the docker stands if the model has no `DockStart`.
    fn dock_point(&mut self, dock: u32, docker: u32, which: DockSub) -> [f32; 3] {
        self.ensure_loaded(dock);
        let d = self.dock_of(dock).unwrap();
        if d.enter == [0.0; 3] {
            if which == DockSub::MoveToEntry && self.airborne(docker) {
                return self.objects[&dock].position;
            }
            return self.objects[&docker].position;
        }
        let bone = match which {
            DockSub::MoveToEntry => d.enter,
            DockSub::MoveToDock => d.dock,
            _ => d.exit,
        };
        self.to_world(dock, bone)
    }
    fn on_approach_reached(&mut self, dock: u32, docker: u32) {
        let d = self.dock_mut(dock).unwrap();
        if let Some(i) = d.owners.iter().position(|o| *o == Some(docker)) {
            d.reached[i] = true;
        }
    }
    fn on_enter_reached(&mut self, dock: u32, docker: u32) {
        let d = self.dock_mut(dock).unwrap();
        d.inside = true;
        if let Some(i) = d.owners.iter().position(|o| *o == Some(docker)) {
            d.owners[i] = None;
            d.reached[i] = false;
        }
    }
    fn on_exit_reached(&mut self, dock: u32, docker: u32) {
        let d = self.dock_mut(dock).unwrap();
        d.inside = false;
        if d.active == Some(docker) {
            d.active = None;
        }
    }
    /// `DockUpdate::cancelDock`.
    pub(crate) fn cancel_dock(&mut self, dock: u32, docker: u32) {
        let Some(d) = self.dock_mut(dock) else { return };
        for i in 0..d.owners.len() {
            if d.owners[i] == Some(docker) {
                d.owners[i] = None;
                d.reached[i] = false;
            }
        }
        if d.active == Some(docker) {
            d.active = None;
            d.inside = false;
        }
    }
    /// `DockUpdate::update` (every frame).
    pub(crate) fn dock_update(&mut self, id: u32) -> u32 {
        if let Some(d) = self.dock_mut(id)
            && d.active.is_none()
            && let Some(i) = d.reached.iter().position(|r| *r)
        {
            d.active = d.owners[i];
        }
        1
    }
    /// `SupplyWarehouseDockUpdate::action` / `SupplyCenterDockUpdate::action`.
    fn dock_action(&mut self, dock: u32, docker: u32) -> bool {
        let kind = self.dock_of(dock).unwrap().kind;
        if self.objects.get(&docker).and_then(|o| o.truck.as_ref()).is_none() {
            return false;
        }
        match kind {
            DockKind::Warehouse => {
                if self.dock_of(dock).unwrap().boxes == 0 {
                    return false;
                }
                let r = Self::bounding(&self.templates.objects[&self.objects[&docker].template]).0;
                if self.attack_distance_sqr(docker, None, Some(dock), [0.0; 3]) > (r * 2.0).powi(2) {
                    // Not close enough: nudge the docker.
                    let dx = self.rng.integer(-JITTER, JITTER) as f32;
                    let dy = self.rng.integer(-JITTER, JITTER) as f32;
                    let o = self.objects.get_mut(&docker).unwrap();
                    o.position[0] += dx;
                    o.position[1] += dy;
                    self.update_pos(docker);
                    return false;
                }
                let d = self.dock_mut(dock).unwrap();
                d.boxes -= 1;
                let (left, delete) = (d.boxes, d.delete_when_empty);
                let t = self.objects.get_mut(&docker).unwrap().truck.as_mut().unwrap();
                // gainOneBox
                if t.boxes >= t.max_boxes {
                    self.dock_mut(dock).unwrap().boxes += 1;
                    return false;
                }
                t.boxes += 1;
                if left == 0 && delete {
                    self.destroy_object(dock);
                    return false;
                }
                true
            }
            DockKind::Center => {
                let owner = self.objects[&dock].owner;
                let t = self.objects.get_mut(&docker).unwrap().truck.as_mut().unwrap();
                let value = t.boxes as u32 * self.templates.supply_box_value;
                t.boxes = 0;
                if value > 0 {
                    self.player(owner).deposit(value);
                }
                false
            }
        }
    }

    // --- ResourceGatheringManager ---

    /// `ActionManager::canTransferSuppliesAt`.
    fn can_transfer(&self, truck: u32, dest: u32) -> bool {
        let Some(o) = self.objects.get(&dest) else { return false };
        let Some(d) = o.dock.as_ref() else { return false };
        let sold = crate::construction::status_bits(&["SOLD"]);
        if o.dead || self.under_construction(dest) || self.under_construction(truck) || o.status & sold != 0 {
            return false;
        }
        let Some(t) = self.objects[&truck].truck.as_ref() else { return false };
        match d.kind {
            DockKind::Warehouse => d.boxes != 0 && self.relationship(dest, truck) != Relationship::Enemies,
            DockKind::Center => t.boxes != 0 && o.owner == self.objects[&truck].owner,
        }
    }
    /// `computeRelativeCost`: squared 3D distance, or none.
    fn relative_cost(&self, truck: u32, dest: u32) -> Option<f32> {
        if !self.can_transfer(truck, dest) || !self.is_clear_to_approach(dest, truck) {
            return None;
        }
        let (a, b) = (self.objects[&truck].position, self.objects[&dest].position);
        Some((a[0] - b[0]).powi(2) + (a[1] - b[1]).powi(2) + (a[2] - b[2]).powi(2))
    }
    fn find_best(&self, truck: u32, kind: DockKind) -> Option<u32> {
        let t = self.objects[&truck].truck.as_ref()?;
        if let Some(p) = t.preferred
            && self.dock_of(p).is_some_and(|d| d.kind == kind)
            && self.relative_cost(truck, p).is_some()
        {
            return Some(p);
        }
        let max = if kind == DockKind::Warehouse { t.scan_distance * t.scan_distance } else { f32::MAX };
        let mut best = None;
        let mut best_cost = f32::MAX;
        for o in self.objects.values().filter(|o| o.dock.as_ref().is_some_and(|d| d.kind == kind)) {
            if let Some(cost) = self.relative_cost(truck, o.id)
                && cost < best_cost
                && (kind == DockKind::Center || cost < max)
            {
                best = Some(o.id);
                best_cost = cost;
            }
        }
        best
    }

    // --- AI_DOCK ---

    /// `aiDock` (`privateDock`).
    pub(crate) fn begin_dock(&mut self, id: u32, dock: u32, from_ai: bool) {
        if self.validate_mobile_quiet(id).is_err() || !self.objects.contains_key(&dock) {
            return;
        }
        let frame = self.frame;
        let o = self.objects.get_mut(&id).unwrap();
        o.ai.begin(AiState::Dock, frame);
        o.ai.cmd_from_ai = from_ai;
        if !from_ai && let Some(t) = o.truck.as_mut() {
            t.preferred = Some(dock);
        }
        self.halt_docks(id);
        if self.dock_of(dock).is_none() {
            self.end_order(id, Outcome::Failure);
            return;
        }
        let o = self.objects.get_mut(&id).unwrap();
        o.ai.ignore_obstacle = Some(dock);
        o.ai.dock = Some(DockMachine {
            goal: dock,
            sub: None,
            approach: -1,
            enter_frame: 0,
            next_action: 0,
            goal_position: [0.0; 3],
        });
        match self.dock_set(id, DockSub::Approach) {
            Ret::Continue => {}
            Ret::Success => self.end_order(id, Outcome::Success),
            Ret::Failure => self.end_order(id, Outcome::Failure),
        }
        self.wake_ai(id);
    }

    /// `AIDockState::update`.
    pub(crate) fn update_dock(&mut self, id: u32) -> u32 {
        let r = self.dock_tick(id);
        match r {
            Ret::Continue => {}
            Ret::Success => self.end_order(id, Outcome::Success),
            Ret::Failure => self.end_order(id, Outcome::Failure),
        }
        self.halt_docks(id);
        1
    }

    /// `AIDockState::onExit` for machines dropped by a new order or the end of one:
    /// `AIDockMachine::halt` cancels the dock and resets the current state.
    pub(crate) fn halt_docks(&mut self, id: u32) {
        let Some(m) = self.objects.get_mut(&id).and_then(|o| o.ai.halted_dock.take()) else { return };
        self.cancel_dock(m.goal, id);
        if let Some(sub) = m.sub {
            self.dock_exit(id, m.goal, sub, true);
        }
        if self.objects[&id].ai.state != AiState::Dock {
            let o = self.objects.get_mut(&id).unwrap();
            if o.ai.ignore_obstacle == Some(m.goal) {
                o.ai.ignore_obstacle = None;
            }
        }
    }

    fn dock_machine(&self, id: u32) -> Option<&DockMachine> {
        self.objects.get(&id).and_then(|o| o.ai.dock.as_ref())
    }
    fn dock_machine_mut(&mut self, id: u32) -> &mut DockMachine {
        self.objects.get_mut(&id).unwrap().ai.dock.as_mut().unwrap()
    }

    fn dock_tick(&mut self, id: u32) -> Ret {
        let Some(sub) = self.dock_machine(id).and_then(|m| m.sub) else { return Ret::Failure };
        let r = self.dock_state_update(id, sub);
        self.dock_after(id, sub, r)
    }

    /// `checkForTransitions` and the state changes it leads to.
    fn dock_after(&mut self, id: u32, mut sub: DockSub, mut r: Ret) -> Ret {
        for _ in 0..20 {
            let goal = self.dock_machine(id).unwrap().goal;
            let next = match r {
                Ret::Continue => {
                    // AI_DOCK_WAIT_FOR_CLEARANCE: ableToAdvance.
                    if sub == DockSub::WaitForClearance
                        && self.dock_of(goal).is_some()
                        && self.is_clear_to_advance(goal, id, self.dock_machine(id).unwrap().approach)
                    {
                        Next::State(DockSub::AdvancePosition)
                    } else {
                        return Ret::Continue;
                    }
                }
                Ret::Success => sub.next().0,
                Ret::Failure => sub.next().1,
            };
            self.dock_exit(id, goal, sub, false);
            match next {
                Next::Done(ok) => {
                    self.dock_machine_mut(id).sub = None;
                    return if ok { Ret::Success } else { Ret::Failure };
                }
                Next::State(s) => {
                    sub = s;
                    self.dock_machine_mut(id).sub = Some(s);
                    r = self.dock_enter(id, s);
                }
            }
        }
        Ret::Failure
    }
    fn dock_set(&mut self, id: u32, sub: DockSub) -> Ret {
        self.dock_machine_mut(id).sub = Some(sub);
        let r = self.dock_enter(id, sub);
        self.dock_after(id, sub, r)
    }

    fn dock_open(&self, goal: u32) -> Option<bool> {
        self.dock_of(goal).map(|d| d.open)
    }

    fn dock_enter(&mut self, id: u32, sub: DockSub) -> Ret {
        let goal = self.dock_machine(id).unwrap().goal;
        let Some(open) = self.dock_open(goal) else { return Ret::Failure };
        let passthrough = self.dock_of(goal).unwrap().passthrough;
        match sub {
            DockSub::Approach | DockSub::AdvancePosition => {
                if !open {
                    self.cancel_dock(goal, id);
                    return Ret::Failure;
                }
                let index = self.dock_machine(id).unwrap().approach;
                let got = if sub == DockSub::Approach {
                    self.reserve_approach(goal, id)
                } else {
                    self.advance_approach(goal, id, index)
                };
                let Some((i, p)) = got else { return Ret::Failure };
                let m = self.dock_machine_mut(id);
                m.approach = i as i32;
                m.goal_position = p;
                self.objects.get_mut(&id).unwrap().ai.ignore_obstacle = None;
                self.internal_move_enter(id, p, true)
            }
            DockSub::WaitForClearance => {
                self.dock_machine_mut(id).enter_frame = self.frame;
                Ret::Continue
            }
            DockSub::MoveToEntry | DockSub::MoveToDock => {
                if !open {
                    self.cancel_dock(goal, id);
                    return Ret::Failure;
                }
                if passthrough {
                    self.objects.get_mut(&id).unwrap().ai.ignore_obstacle = Some(goal);
                }
                let p = self.dock_point(goal, id, sub);
                let m = self.dock_machine_mut(id);
                m.goal_position = p;
                if sub == DockSub::MoveToEntry {
                    m.approach = -1;
                }
                // The dock state does not adjust its goal for pass-through docks.
                let adjust = !(sub == DockSub::MoveToDock && passthrough);
                self.internal_move_enter(id, p, adjust)
            }
            DockSub::ProcessDock => {
                self.set_next_action(id, goal);
                Ret::Continue
            }
            DockSub::MoveToExit => {
                let p = self.dock_point(goal, id, sub);
                if passthrough {
                    self.objects.get_mut(&id).unwrap().ai.ignore_obstacle = Some(goal);
                }
                self.dock_machine_mut(id).goal_position = p;
                self.internal_move_enter(id, p, !passthrough)
            }
            // Supply docks have no rally point after docking.
            DockSub::MoveToRally => Ret::Success,
        }
    }

    fn set_next_action(&mut self, id: u32, goal: u32) {
        let t = self.objects[&id].truck.as_ref();
        let delay = match (t, self.dock_of(goal).map(|d| d.kind)) {
            (Some(t), Some(DockKind::Warehouse)) => t.warehouse_delay,
            (Some(t), Some(DockKind::Center)) => t.center_delay,
            _ => 0,
        };
        let frame = self.frame;
        self.dock_machine_mut(id).next_action = frame + delay;
    }

    fn dock_state_update(&mut self, id: u32, sub: DockSub) -> Ret {
        let goal = self.dock_machine(id).unwrap().goal;
        let Some(open) = self.dock_open(goal) else { return Ret::Failure };
        match sub {
            DockSub::WaitForClearance => {
                if !open {
                    self.cancel_dock(goal, id);
                    return Ret::Failure;
                }
                if self.dock_of(goal).unwrap().active == Some(id) {
                    return Ret::Success;
                }
                if self.dock_machine(id).unwrap().enter_frame + 30 * 30 < self.frame {
                    return Ret::Failure;
                }
                Ret::Continue
            }
            DockSub::ProcessDock => {
                if self.frame < self.dock_machine(id).unwrap().next_action {
                    return Ret::Continue;
                }
                self.set_next_action(id, goal);
                if !open || !self.dock_action(goal, id) {
                    return Ret::Success;
                }
                Ret::Continue
            }
            DockSub::MoveToDock if !open => Ret::Failure,
            DockSub::MoveToRally => Ret::Success,
            _ => self.internal_move_update(id),
        }
    }

    fn dock_exit(&mut self, id: u32, goal: u32, sub: DockSub, reset: bool) {
        let open = self.dock_open(goal);
        match sub {
            DockSub::Approach | DockSub::AdvancePosition => {
                if let Some(open) = open {
                    if reset || !open {
                        self.cancel_dock(goal, id);
                    } else {
                        self.on_approach_reached(goal, id);
                    }
                }
            }
            DockSub::WaitForClearance => {
                if open.is_some_and(|o| !o || reset) {
                    self.cancel_dock(goal, id);
                }
            }
            DockSub::MoveToEntry => {
                if let Some(open) = open {
                    if reset || !open {
                        self.cancel_dock(goal, id);
                    } else {
                        self.on_enter_reached(goal, id);
                    }
                }
            }
            DockSub::MoveToDock => {
                if open.is_some_and(|o| !o || reset) {
                    self.cancel_dock(goal, id);
                }
            }
            DockSub::MoveToExit => {
                if open.is_some() {
                    self.on_exit_reached(goal, id);
                }
            }
            DockSub::ProcessDock | DockSub::MoveToRally => {}
        }
        if sub.moves() {
            // AIInternalMoveToState::onExit: the move stops.
            if let Some(o) = self.objects.get_mut(&id) {
                o.target = None;
                o.path.clear();
            }
        }
    }

    /// `AIInternalMoveToState::onEnter`.
    fn internal_move_enter(&mut self, id: u32, goal: [f32; 3], adjust: bool) -> Ret {
        let state = if adjust { AiState::MoveTo } else { AiState::FollowPath };
        let goal = self.choose_goal(id, [goal[0], goal[1]], state, adjust);
        match self.route_to_waypoint(id, goal, true) {
            Ok((path, to)) => {
                let o = self.objects.get_mut(&id).unwrap();
                o.path = path;
                o.path.push_front(Waypoint { position: [o.position[0], o.position[1]], layer: o.layer });
                o.target = Some(to);
                o.blocked_frames = 0;
                o.blocked_by = None;
                o.traffic.stuck = false;
                o.last_repath = self.frame;
                self.move_allies(id);
                Ret::Continue
            }
            Err(_) => Ret::Failure,
        }
    }
    /// `AIInternalMoveToState::update`.
    fn internal_move_update(&mut self, id: u32) -> Ret {
        self.repath_traffic(id);
        self.drive(id);
        if self.objects[&id].target.is_none() { Ret::Success } else { Ret::Continue }
    }

    // --- SupplyTruckStateMachine ---

    /// `SupplyTruckAIUpdate::update`: the truck machine runs before the AI.
    pub(crate) fn truck_update(&mut self, id: u32) {
        let Some(state) = self.objects[&id].truck.as_ref().map(|t| t.state) else { return };
        let r = self.truck_state_update(id, state);
        self.truck_after(id, state, r);
    }
    fn set_truck_state(&mut self, id: u32, s: TruckState) {
        self.objects.get_mut(&id).unwrap().truck.as_mut().unwrap().state = s;
    }
    fn truck_after(&mut self, id: u32, mut state: TruckState, mut r: Ret) {
        use TruckState::*;
        for _ in 0..20 {
            let next = match r {
                Ret::Continue => match self.truck_condition(id, state) {
                    Some(n) => n,
                    None => return,
                },
                // (success, failure) from `SupplyTruckStateMachine`.
                Ret::Success => match state {
                    Regrouping => Wanting,
                    _ => Busy,
                },
                Ret::Failure => match state {
                    Wanting => Regrouping,
                    _ => Busy,
                },
            };
            state = next;
            self.set_truck_state(id, next);
            r = self.truck_enter(id, next);
        }
    }
    fn truck_condition(&self, id: u32, state: TruckState) -> Option<TruckState> {
        use TruckState::*;
        let o = &self.objects[&id];
        let t = o.truck.as_ref()?;
        let idle = !o.ai.active();
        let docking = o.ai.state == AiState::Dock;
        let not_docking_or_idle = !idle && !docking;
        match state {
            Busy => {
                if idle {
                    Some(Idle)
                } else if docking {
                    Some(Docking)
                } else {
                    None
                }
            }
            Idle => {
                if t.force_busy {
                    Some(Busy)
                } else if t.force_wanting {
                    Some(Wanting)
                } else if docking {
                    Some(Docking)
                } else if not_docking_or_idle {
                    Some(Busy)
                } else {
                    None
                }
            }
            Wanting => {
                if docking {
                    Some(Docking)
                } else if not_docking_or_idle {
                    Some(Busy)
                } else {
                    None
                }
            }
            Regrouping => (!o.ai.cmd_from_ai).then_some(Busy),
            Docking => {
                if t.force_busy {
                    Some(Busy)
                } else if idle {
                    Some(Wanting)
                } else if not_docking_or_idle {
                    Some(Busy)
                } else {
                    None
                }
            }
        }
    }
    fn truck_enter(&mut self, id: u32, state: TruckState) -> Ret {
        let t = self.objects.get_mut(&id).unwrap().truck.as_mut().unwrap();
        match state {
            TruckState::Busy => t.force_busy = false,
            TruckState::Wanting | TruckState::Docking => t.force_wanting = false,
            TruckState::Idle => {
                // SupplyTruckIdleState::onEnter: a worker goes back to being a dozer.
                self.exiting_supply_truck_state(id);
            }
            TruckState::Regrouping => return self.regroup_enter(id),
        }
        Ret::Continue
    }
    fn truck_state_update(&mut self, id: u32, state: TruckState) -> Ret {
        match state {
            TruckState::Wanting => {
                let boxes = self.objects[&id].truck.as_ref().unwrap().boxes;
                let kind = if boxes > 0 { DockKind::Center } else { DockKind::Warehouse };
                if let Some(dock) = self.find_best(id, kind) {
                    self.begin_dock(id, dock, true);
                    return Ret::Success;
                }
                Ret::Failure
            }
            TruckState::Regrouping => {
                if self.objects[&id].ai.active() {
                    Ret::Continue
                } else {
                    Ret::Success
                }
            }
            _ => Ret::Continue,
        }
    }
    /// `RegroupingState::onEnter`: back to the nearest cash generator, command
    /// centre or structure.
    fn regroup_enter(&mut self, id: u32) -> Ret {
        self.objects.get_mut(&id).unwrap().ai.ignore_obstacle = None;
        let owner = self.objects[&id].owner;
        let p = self.objects[&id].position;
        let mut dest = None;
        for kind in ["CASH_GENERATOR", "COMMANDCENTER", "STRUCTURE"] {
            let mut best = f32::MAX;
            for o in self.objects.values().filter(|o| o.owner == owner && o.id != id) {
                if !self.templates.objects[&o.template].has_kind(kind) {
                    continue;
                }
                let d = (o.position[0] - p[0]).powi(2) + (o.position[1] - p[1]).powi(2);
                if d < best {
                    best = d;
                    dest = Some(o.id);
                }
            }
            if dest.is_some() {
                break;
            }
        }
        let Some(dest) = dest else { return Ret::Failure };
        if self.attack_distance_sqr(id, None, Some(dest), [0.0; 3]) < 225.0 {
            return Ret::Continue;
        }
        let at = self.objects[&dest].position;
        let Some(to) = self.find_position_around(at, 0.0, 100.0, None, None, 1e10) else { return Ret::Failure };
        if self.begin_move(id, [to[0], to[1]], AiState::MoveTo, Default::default()).is_ok() {
            self.objects.get_mut(&id).unwrap().ai.cmd_from_ai = true;
        }
        Ret::Continue
    }
}
