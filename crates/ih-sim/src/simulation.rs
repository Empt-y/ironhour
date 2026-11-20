//! Ordered object/module updates and commands, following `GameLogic.cpp`,
//! `Object.cpp`, `AIUpdate.cpp`, `PhysicsUpdate.cpp`, `Locomotor.cpp`.
//! The ground steering/integration is an initial movement slice; specialised
//! locomotors, the full AI state machine and full collision forces remain pending.
use crate::{
    ai::{AiMachine, AiState, Outcome},
    disabled::{DisabledReason, DisabledStatus},
    locomotion::CELL,
    locomotion::{Body, LocoState, Physics},
    partition::Partition,
    rng::LogicRng,
    scheduler::{FOREVER, Phase, Scheduler},
    state_hash::StateHash,
    templates::{Locomotor, Templates},
    terrain::{Terrain, Waypoint},
    traffic::{self, Collider},
};
use ih_formats::map::Map;
use serde::{Deserialize, Serialize};
use std::{
    collections::{BTreeMap, VecDeque},
    sync::Arc,
};

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
pub enum Command {
    Move {
        object: u32,
        target: [f32; 2],
    },
    Stop {
        object: u32,
    },
    FollowPath {
        object: u32,
        points: Vec<[f32; 2]>,
    },
    Face {
        object: u32,
        target: [f32; 2],
    },
    FaceObject {
        object: u32,
        target: u32,
    },
    Disable {
        object: u32,
        reason: DisabledReason,
        #[serde(default)]
        frames: Option<u32>,
    },
    Enable {
        object: u32,
        reason: DisabledReason,
    },
    Destroy {
        object: u32,
    },
    Spawn {
        template: String,
        position: [f32; 2],
        #[serde(default)]
        owner: u32,
    },
}
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ModuleKind {
    Stub,
    Steering,
    Physics,
}
#[derive(Clone, Debug)]
pub struct Module {
    pub class: String,
    pub tag: String,
    pub kind: ModuleKind,
    pub updates: u32,
}
#[derive(Clone, Debug)]
pub struct Object {
    pub id: u32,
    pub template: String,
    pub owner: u32,
    pub position: [f32; 3],
    pub previous_position: [f32; 3],
    pub layer: u32,
    pub angle: f32,
    pub speed: f32,
    pub velocity: [f32; 2],
    pub target: Option<[f32; 2]>,
    pub path: VecDeque<Waypoint>,
    pub modules: Vec<Module>,
    pub blocked_frames: u32,
    pub blocked_by: Option<u32>,
    pub last_repath: u32,
    pub traffic_repaths: u32,
    pub ai: AiMachine,
    pub disabled: DisabledStatus,
    /// `PhysicsBehavior` state.
    pub physics: Physics,
    /// `Locomotor` state.
    pub loco: LocoState,
    /// This frame's braking slide from the locomotor (applied with the physics step).
    pub slide: [f32; 2],
    /// `AIUpdateInterface` blocked state from collisions.
    pub traffic: traffic::TrafficState,
    /// Index into `Map::objects` for the map's own objects.
    pub map_index: Option<usize>,
    /// Collision shape from the template.
    pub body: traffic::BodyShape,
}
impl Object {
    pub fn is_idle(&self) -> bool {
        !self.ai.active() && self.target.is_none()
    }
}
#[derive(Debug, thiserror::Error)]
#[error("{0}")]
pub struct SimError(pub String);
pub struct Simulation {
    pub frame: u32,
    pub objects: BTreeMap<u32, Object>,
    pub rng: LogicRng,
    pub scheduler: Scheduler,
    pub partition: Partition,
    pub map: Arc<Map>,
    pub templates: Arc<Templates>,
    pub grid: Terrain,
    /// `grid` as seen by locomotors with other surfaces (water, cliff, ...), by mask.
    pub views: BTreeMap<u8, Terrain>,
    pub stub_counts: BTreeMap<String, usize>,
    pub events: Vec<String>,
    pub failed_orders: u32,
    /// (frame, victim, crusher) for objects crushed so far.
    pub crushed: Vec<(u32, u32, u32)>,
    /// Move-out-of-the-way requests carried out (diagnostics).
    pub move_asides: u32,
    next_id: u32,
    sequence: u64,
    commands: BTreeMap<(u32, u64), Command>,
    setup_hash: u64,
    collision_radius_bound: f32,
}
impl Simulation {
    pub fn new(map: Arc<Map>, templates: Arc<Templates>, seed: u32) -> Self {
        let grid = Terrain::from_map(&map, &templates);
        // One view per distinct locomotor surface mask that sees the cells differently.
        let mut views = BTreeMap::new();
        for l in templates.locomotors.values().filter(|l| l.surface_movement()) {
            let mask = l.surface_mask();
            if !views.contains_key(&mask) && grid.obstacles.blocked_for(mask) != grid.ground.blocked {
                views.insert(mask, grid.view(mask));
            }
        }
        let mut h = StateHash::default();
        h.u32(4);
        h.u32(map.width as u32);
        h.u32(map.height as u32);
        h.u32(map.border as u32);
        h.bytes(&map.heights);
        h.u32(grid.width as u32);
        h.u32(grid.height as u32);
        for b in &grid.blocked {
            h.u32(u32::from(*b));
        }
        for k in &grid.obstacles.kind {
            h.u32(*k as u32);
        }
        h.u64(grid.layers.len() as u64);
        for layer in &grid.layers {
            let b = &layer.bridge;
            h.text(&b.name);
            for v in b.from.into_iter().chain(b.to).chain([b.min_y, b.max_y]) {
                h.float(v);
            }
            h.u64(layer.cells.len() as u64);
            for ((x, y), cell) in &layer.cells {
                h.u32(*x as u32);
                h.u32(*y as u32);
                h.u32(u32::from(cell.entry));
            }
        }
        h.u64(templates.objects.len() as u64);
        for (name, t) in &templates.objects {
            h.text(name);
            h.u64(t.fields.len() as u64);
            for (field, v) in &t.fields {
                h.text(field);
                h.u64(v.len() as u64);
                for v in v {
                    h.text(v);
                }
            }
            h.u64(t.modules.len() as u64);
            for m in &t.modules {
                h.text(&m.class);
                h.text(&m.tag);
            }
        }
        h.u64(templates.locomotors.len() as u64);
        for (name, l) in &templates.locomotors {
            h.text(name);
            for v in [l.speed, l.min_speed, l.acceleration, l.braking, l.turn_rate] {
                h.float(v);
            }
            h.u64(l.surfaces.len() as u64);
            for s in &l.surfaces {
                h.text(s);
            }
        }
        Self {
            frame: 0,
            objects: BTreeMap::new(),
            rng: LogicRng::new(seed),
            scheduler: Scheduler::default(),
            partition: Partition::default(),
            map,
            templates,
            views,
            grid,
            stub_counts: BTreeMap::new(),
            events: vec![],
            failed_orders: 0,
            crushed: Vec::new(),
            move_asides: 0,
            next_id: 1,
            sequence: 0,
            commands: BTreeMap::new(),
            setup_hash: h.0,
            collision_radius_bound: 0.0,
        }
    }
    pub fn spawn(&mut self, name: &str, p: [f32; 2], owner: u32) -> Result<u32, SimError> {
        if !p.iter().all(|v| v.is_finite()) {
            return Err(SimError("non-finite spawn position".into()));
        }
        let surface = self.grid.destination(&self.map, p);
        let z = self.grid.height_at(&self.map, surface);
        self.create(name, [p[0], p[1], z], 0.0, surface.layer, owner, None)
    }

    /// Creates the map's own logic objects, in map order (`GameLogic::startNewGame`):
    /// at the stored height above the ground, facing, on the layer nearest that
    /// height, owned by the player whose team is the object's `originalOwner`.
    /// Their obstacle cells are already in the terrain (`Terrain::from_map`).
    /// Returns the number created.
    pub fn load_map_objects(&mut self) -> Result<usize, SimError> {
        let placements = crate::map_objects::placements(&self.map, &self.templates);
        for p in &placements {
            let xy = [p.position[0], p.position[1]];
            let ground = self.map.ground_height(xy[0], xy[1]);
            let z = p.position[2] + ground;
            // getLayerForDestination: a deck only when the object is nearer it than the ground.
            let deck = self.grid.destination(&self.map, xy);
            let layer = if deck.layer != 0 && (self.grid.height_at(&self.map, deck) - z).abs() < (ground - z).abs() {
                deck.layer
            } else {
                0
            };
            self.create(&p.template, [xy[0], xy[1], z], p.angle, layer, p.owner, Some(p.index))?;
        }
        Ok(placements.len())
    }

    fn create(
        &mut self,
        name: &str,
        position: [f32; 3],
        angle: f32,
        layer: u32,
        owner: u32,
        map_index: Option<usize>,
    ) -> Result<u32, SimError> {
        let t =
            self.templates.objects.get(name).ok_or_else(|| SimError(format!("object template {name} is missing")))?;
        let id = self.next_id;
        self.next_id = self.next_id.checked_add(1).ok_or_else(|| SimError("object IDs exhausted".into()))?;
        let mut modules = vec![];
        for m in t.modules.iter().filter(|m| matches!(m.kind.as_str(), "Behavior" | "Body")) {
            let known = ih_ini::schema::MODULES.iter().find(|d| d.name == m.class);
            let kind = if m.class == "PhysicsBehavior" {
                ModuleKind::Physics
            } else if known.is_some_and(|d| d.tables.iter().any(|t| t.starts_with("AIUpdateModuleData::"))) {
                ModuleKind::Steering
            } else {
                ModuleKind::Stub
            };
            let index = modules.len();
            if kind == ModuleKind::Stub {
                *self.stub_counts.entry(m.class.clone()).or_default() += 1;
            } else {
                self.scheduler.insert(
                    (id, index),
                    self.frame.max(1),
                    if kind == ModuleKind::Steering { Phase::Initial } else { Phase::Physics },
                );
            }
            modules.push(Module { class: m.class.clone(), tag: m.tag.clone(), kind, updates: 0 });
        }
        let mut physics = Physics { data: t.physics().unwrap_or_default(), ..Default::default() };
        let loco = match t.locomotor().and_then(|n| self.templates.locomotors.get(n)) {
            Some(l) => {
                // Locomotor::setPhysicsOptions
                physics.extra_friction = l.extra_2d_friction;
                physics.stick_to_ground = l.stick_to_ground;
                physics.airborne_friction = l.apply_2d_friction_when_airborne;
                LocoState::new(l, self.frame, &mut self.rng)
            }
            None => LocoState::default(),
        };
        let body = traffic::BodyShape::new(t, &modules, &self.templates);
        self.objects.insert(
            id,
            Object {
                id,
                template: name.into(),
                owner,
                position,
                previous_position: position,
                layer,
                angle,
                speed: 0.0,
                velocity: [0.0; 2],
                target: None,
                path: VecDeque::new(),
                modules,
                blocked_frames: 0,
                blocked_by: None,
                last_repath: 0,
                traffic_repaths: 0,
                ai: AiMachine::default(),
                disabled: DisabledStatus::default(),
                physics,
                loco,
                slide: [0.0; 2],
                traffic: traffic::TrafficState::default(),
                map_index,
                body,
            },
        );
        self.rebuild_partition();
        Ok(id)
    }
    pub fn enqueue(&mut self, frame: u32, command: Command) -> Result<(), SimError> {
        if frame < self.frame {
            return Err(SimError(format!("command frame {frame} is before current frame {}", self.frame)));
        }
        let finite = match &command {
            Command::Move { target, .. } | Command::Face { target, .. } => target.iter().all(|f| f.is_finite()),
            Command::FollowPath { points, .. } => points.iter().flatten().all(|f| f.is_finite()),
            Command::Spawn { position, .. } => position.iter().all(|f| f.is_finite()),
            _ => true,
        };
        if !finite {
            return Err(SimError("non-finite command coordinate".into()));
        }
        self.commands.insert((frame, self.sequence), command);
        self.sequence += 1;
        Ok(())
    }
    fn command(&mut self, c: Command) -> Result<(), SimError> {
        match c {
            Command::Spawn { template, position, owner } => {
                self.spawn(&template, position, owner)?;
            }
            Command::Destroy { object } => {
                self.remove(object)?;
            }
            Command::Stop { object } => {
                if !self.objects.contains_key(&object) {
                    return Err(SimError(format!("object {object} is missing")));
                }
                self.end_order(object, Outcome::Interrupted);
            }
            Command::Move { object, target } => {
                self.begin_move(object, target, AiState::MoveTo, VecDeque::new())?;
            }
            Command::FollowPath { object, points } => {
                let mut points: VecDeque<_> = points.into();
                let first =
                    points.pop_front().ok_or_else(|| SimError("follow_path requires at least one point".into()))?;
                self.begin_move(object, first, AiState::FollowPath, points)?;
            }
            Command::Face { object, target } => {
                self.begin_face(object, Some(target), None)?;
            }
            Command::FaceObject { object, target } => {
                if !self.objects.contains_key(&target) {
                    return Err(SimError(format!("face target {target} is missing")));
                }
                self.begin_face(object, None, Some(target))?;
            }
            Command::Disable { object, reason, frames } => {
                let o = self.objects.get_mut(&object).ok_or_else(|| SimError(format!("object {object} is missing")))?;
                o.disabled.set(reason, self.frame, frames);
                if o.disabled.any() {
                    o.speed = 0.0;
                    o.velocity = [0.0; 2];
                    o.physics.vel = [0.0, 0.0, o.physics.vel[2]];
                    o.blocked_frames = 0;
                    o.blocked_by = None;
                    o.traffic.stuck = false;
                }
                if o.ai.active() {
                    self.wake_ai(object);
                }
            }
            Command::Enable { object, reason } => {
                let o = self.objects.get_mut(&object).ok_or_else(|| SimError(format!("object {object} is missing")))?;
                o.disabled.clear(reason);
                o.disabled.expire(self.frame);
                if o.ai.active() {
                    self.wake_ai(object);
                }
            }
        }
        Ok(())
    }
    fn validate_mobile(&self, id: u32, face: bool) -> Result<(), SimError> {
        let o = self.objects.get(&id).ok_or_else(|| SimError(format!("object {id} is missing")))?;
        let t = &self.templates.objects[&o.template];
        if t.has_kind("IMMOBILE") || o.disabled.any() {
            return Err(SimError(format!("object {id} is immobile or disabled")));
        }
        let l = t.locomotor().and_then(|n| self.templates.locomotors.get(n));
        if l.is_none_or(|l| {
            l.surface_mask() == 0 || l.speed <= 0.0 || (face && (l.min_speed != 0.0 || l.turn_rate <= 0.0))
        }) || !o.modules.iter().any(|m| m.kind == ModuleKind::Steering)
            || !o.modules.iter().any(|m| m.kind == ModuleKind::Physics)
        {
            return Err(SimError(format!(
                "{} has no implemented {}",
                o.template,
                if face { "turning in place" } else { "movement" }
            )));
        }
        Ok(())
    }
    /// Air-only locomotion (`LOCOMOTORSURFACE_AIR` without ground).
    fn flyer(&self, id: u32) -> bool {
        let o = &self.objects[&id];
        self.templates.objects[&o.template]
            .locomotor()
            .and_then(|n| self.templates.locomotors.get(n))
            .is_some_and(|l| l.air() && !l.ground())
    }

    /// `doLocomotor` with no goal: `locoUpdate_maintainCurrentPosition`. Hovering and
    /// circling need it every frame; anything resting on the ground can sleep.
    fn idle_locomotor(&mut self, id: u32) -> u32 {
        let frame = self.frame;
        let o = self.objects.get_mut(&id).unwrap();
        let t = &self.templates.objects[&o.template];
        let Some(l) = t.locomotor().and_then(|n| self.templates.locomotors.get(n)) else { return FOREVER };
        if !l.air() {
            return FOREVER;
        }
        let z = heights(&self.grid, &self.map, [o.position[0], o.position[1]], o.layer);
        let constant = Body {
            position: &mut o.position,
            angle: &mut o.angle,
            physics: &mut o.physics,
            loco: &mut o.loco,
            major_radius: 0.0,
            bounding_radius: 0.0,
            frame,
        }
        .maintain_current_position(l, z);
        if constant { 1 } else { FOREVER }
    }

    /// The terrain view for the object's locomotor surfaces (`grid` for ground units).
    fn terrain(&self, id: u32) -> &Terrain {
        let t = &self.templates.objects[&self.objects[&id].template];
        t.locomotor()
            .and_then(|n| self.templates.locomotors.get(n))
            .and_then(|l| self.views.get(&l.surface_mask()))
            .unwrap_or(&self.grid)
    }
    fn route_to(&self, id: u32, target: [f32; 2]) -> Result<VecDeque<Waypoint>, SimError> {
        let o = &self.objects[&id];
        let t = &self.templates.objects[&o.template];
        if self.flyer(id) {
            // canComputeQuickPath / computeQuickPath: flyers go straight to the goal.
            return Ok(VecDeque::from([Waypoint { position: target, layer: 0 }]));
        }
        self.terrain(id)
            .path(
                Waypoint { position: [o.position[0], o.position[1]], layer: o.layer },
                self.grid.destination(&self.map, target),
                t.radius(),
            )
            .map(Into::into)
            .ok_or_else(|| SimError(format!("object {id}: target is blocked or unreachable")))
    }
    fn begin_move(
        &mut self,
        id: u32,
        target: [f32; 2],
        state: AiState,
        remaining: VecDeque<[f32; 2]>,
    ) -> Result<(), SimError> {
        self.validate_mobile(id, false)?;
        let path = self.route_to(id, target)?;
        let o = self.objects.get_mut(&id).unwrap();
        o.ai.begin(state, self.frame);
        o.ai.remaining = remaining;
        o.path = path;
        // The first node is the start of the current segment (`Path` keeps its start node).
        o.path.push_front(Waypoint { position: [o.position[0], o.position[1]], layer: o.layer });
        o.target = Some(target);
        o.blocked_frames = 0;
        o.blocked_by = None;
        o.traffic.stuck = false;
        o.last_repath = self.frame;
        self.wake_ai(id);
        Ok(())
    }
    fn begin_face(&mut self, id: u32, target: Option<[f32; 2]>, object: Option<u32>) -> Result<(), SimError> {
        self.validate_mobile(id, true)?;
        let o = self.objects.get_mut(&id).unwrap();
        o.ai.begin(if object.is_some() { AiState::FaceObject } else { AiState::FacePosition }, self.frame);
        o.ai.face_position = target;
        o.ai.face_object = object;
        o.target = None;
        o.path.clear();
        o.speed = 0.0;
        o.velocity = [0.0; 2];
        o.physics.vel = [0.0, 0.0, o.physics.vel[2]];
        o.blocked_frames = 0;
        o.blocked_by = None;
        o.traffic.stuck = false;
        self.wake_ai(id);
        Ok(())
    }
    fn wake_ai(&mut self, id: u32) {
        for (i, m) in self.objects[&id].modules.iter().enumerate() {
            if m.kind == ModuleKind::Steering {
                self.scheduler.awaken((id, i), self.frame, 1);
            }
        }
    }
    fn end_order(&mut self, id: u32, outcome: Outcome) {
        let o = self.objects.get_mut(&id).unwrap();
        if outcome == Outcome::Failure {
            self.failed_orders += 1;
        }
        o.ai.finish(outcome, self.frame);
        o.target = None;
        o.path.clear();
        o.speed = 0.0;
        o.velocity = [0.0; 2];
        o.physics.vel = [0.0, 0.0, o.physics.vel[2]];
        o.blocked_frames = 0;
        o.blocked_by = None;
        o.traffic.stuck = false;
    }
    pub fn remove(&mut self, id: u32) -> Result<(), SimError> {
        let o = self.objects.remove(&id).ok_or_else(|| SimError(format!("object {id} is missing")))?;
        for i in 0..o.modules.len() {
            self.scheduler.remove((id, i));
        }
        self.rebuild_partition();
        Ok(())
    }
    pub fn step(&mut self) {
        for o in self.objects.values_mut() {
            o.previous_position = o.position;
        }
        while self.commands.first_key_value().is_some_and(|((f, _), _)| *f <= self.frame) {
            let ((frame, seq), command) = self.commands.pop_first().unwrap();
            if let Err(e) = self.command(command) {
                self.events.push(format!("frame {frame} command {seq}: {e}"));
            }
        }
        self.wake_forced();
        while let Some((id, index)) = self.scheduler.due(self.frame) {
            let kind = self.objects[&id].modules[index].kind;
            let exceptions = match kind {
                ModuleKind::Steering => 1 << (DisabledReason::Held as u32),
                ModuleKind::Physics => u32::MAX,
                ModuleKind::Stub => 0,
            };
            if !self.objects[&id].disabled.allows_module(exceptions) {
                self.scheduler.reschedule((id, index), self.frame.saturating_add(1));
                continue;
            }
            let kind = {
                let o = self.objects.get_mut(&id).unwrap();
                o.modules[index].updates += 1;
                o.modules[index].kind
            };
            let delay = match kind {
                ModuleKind::Steering => {
                    let delay = self.update_ai(id);
                    self.wake_physics(id);
                    delay
                }
                ModuleKind::Physics => {
                    self.integrate_ground(id);
                    self.physics_sleep(id)
                }
                ModuleKind::Stub => FOREVER,
            };
            self.scheduler.reschedule((id, index), self.frame.saturating_add(delay.max(1)));
        }
        let mut awakened = Vec::new();
        for o in self.objects.values_mut() {
            if o.disabled.expire(self.frame) && o.ai.active() {
                awakened.push(o.id);
            }
        }
        for id in awakened {
            self.wake_ai(id);
        }
        self.process_collisions();
        self.wake_forced();
        self.rebuild_partition();
        self.frame = self.frame.checked_add(1).expect("logic frame overflow");
    }
    /// `PhysicsBehavior::calcSleepTime` (`SLEEPY_PHYSICS`), after the update ran.
    /// Overlap tracking (`getCurrentOverlap`) is not kept yet and counts as none.
    fn physics_sleep(&mut self, id: u32) -> u32 {
        let frame = self.frame;
        let o = self.objects.get_mut(&id).unwrap();
        o.physics.ever_run = true;
        o.physics.woken = false;
        let p = &o.physics;
        let ground = if o.layer == 0 { self.map.ground_height(o.position[0], o.position[1]) } else { f32::MAX };
        let resting = p.vel == [0.0; 3]
            && p.accel == [0.0; 3]
            && p.motive_expires <= frame
            && o.layer == 0
            && o.position[2] - ground <= 0.0
            && o.slide == [0.0; 2];
        if resting { FOREVER } else { 1 }
    }
    /// `setWakeFrame(UPDATE_SLEEP_NONE)` after a force from another module: the
    /// physics module runs next frame, or this frame if it is already due.
    fn wake_physics(&mut self, id: u32) {
        let Some(o) = self.objects.get_mut(&id) else { return };
        // The braking slide is applied with the physics step here, so it wakes it too.
        if !o.physics.woken && o.slide == [0.0; 2] {
            return;
        }
        o.physics.woken = false;
        if let Some(i) = o.modules.iter().position(|m| m.kind == ModuleKind::Physics) {
            self.scheduler.awaken((id, i), self.frame, 1);
        }
    }
    /// Wakes every body that received a force outside an AI update (commands, collisions).
    fn wake_forced(&mut self) {
        let ids: Vec<u32> =
            self.objects.values().filter(|o| o.physics.woken || o.slide != [0.0; 2]).map(|o| o.id).collect();
        for id in ids {
            self.wake_physics(id);
        }
    }
    fn update_ai(&mut self, id: u32) -> u32 {
        let o = &self.objects[&id];
        if o.disabled.any() {
            return if o.ai.active() { 1 } else { FOREVER };
        }
        if o.ai.move_out.is_some() {
            self.drive(id);
            let o = &self.objects[&id];
            let until = o.ai.move_out.as_ref().map_or(0, |m| m.until);
            if o.target.is_none() || self.frame >= until {
                self.end_move_out(id);
            }
            return 1;
        }
        match o.ai.state {
            AiState::Idle => return self.idle_locomotor(id),
            AiState::MoveTo | AiState::FollowPath => {
                self.repath_traffic(id);
                self.drive(id);
                let o = self.objects.get_mut(&id).unwrap();
                if o.target.is_none() {
                    let next = if o.ai.state == AiState::FollowPath { o.ai.remaining.pop_front() } else { None };
                    if let Some(target) = next {
                        match self.route_to(id, target) {
                            Ok(path) => {
                                let o = self.objects.get_mut(&id).unwrap();
                                o.path = path;
                                // The first node is the start of the current segment (`Path` keeps its start node).
                                o.path
                                    .push_front(Waypoint { position: [o.position[0], o.position[1]], layer: o.layer });
                                o.target = Some(target);
                                o.last_repath = self.frame;
                            }
                            Err(_) => self.end_order(id, Outcome::Failure),
                        }
                    } else {
                        self.end_order(id, Outcome::Success);
                    }
                }
            }
            AiState::FaceObject | AiState::FacePosition => {
                let target = if let Some(other) = o.ai.face_object {
                    self.objects.get(&other).map(|o| [o.position[0], o.position[1]])
                } else {
                    o.ai.face_position
                };
                let Some(target) = target else {
                    self.end_order(id, Outcome::Failure);
                    return FOREVER;
                };
                let o = self.objects.get_mut(&id).unwrap();
                let d = [target[0] - o.position[0], target[1] - o.position[1]];
                let delta = ih_math::angle_delta(o.angle, ih_math::atan2(d[1], d[0]));
                if d[0] * d[0] + d[1] * d[1] < 0.000001 || delta.abs() < 0.035 {
                    self.end_order(id, Outcome::Success);
                } else {
                    let t = &self.templates.objects[&o.template];
                    let l = t.locomotor().and_then(|n| self.templates.locomotors.get(n)).unwrap();
                    o.angle += delta.clamp(-l.turn_rate, l.turn_rate);
                }
            }
        }
        if self.objects[&id].ai.active() {
            1
        } else {
            // doLocomotor keeps hovering and circling flyers going once idle.
            let air = self.templates.objects[&self.objects[&id].template]
                .locomotor()
                .and_then(|n| self.templates.locomotors.get(n))
                .is_some_and(|l| l.air());
            if air { 1 } else { FOREVER }
        }
    }
    /// `AIUpdateInterface::doLocomotor` for path following: collisions found last
    /// frame block this frame's drive, capped to how fast the blocker moves away.
    fn drive(&mut self, id: u32) {
        let o = self.objects.get_mut(&id).unwrap();
        let t = &self.templates.objects[&o.template];
        let Some(l) = t.locomotor().and_then(|n| self.templates.locomotors.get(n)) else { return };
        let radii = (t.number("GeometryMajorRadius", 5.0) * t.number("Scale", 1.0), t.collision_radius(), t.radius());
        let tr = &mut o.traffic;
        if tr.is_blocked {
            o.blocked_frames = o.blocked_frames.saturating_add(1);
        } else {
            o.blocked_frames = 0;
        }
        tr.is_blocked = false;
        let mut blocked = o.blocked_frames > 0;
        let mut speed = l.speed;
        if blocked && speed > tr.max_blocked_speed {
            speed = tr.max_blocked_speed;
            tr.bump_speed_limit = tr.bump_speed_limit.min(speed) * 0.95;
            speed = tr.bump_speed_limit;
        } else {
            blocked = false;
            if tr.bump_speed_limit < traffic::FAST_AS_POSSIBLE {
                tr.bump_speed_limit = tr.bump_speed_limit.max(speed * 0.2) * 1.05;
            }
            speed = speed.min(tr.bump_speed_limit);
        }
        let grid = self.views.get(&l.surface_mask()).unwrap_or(&self.grid);
        steer(o, l, grid, &self.map, self.frame, radii, speed, &mut blocked);
        if !blocked && o.blocked_frames > 1 {
            o.blocked_frames = 1;
        }
        o.traffic.max_blocked_speed = traffic::FAST_AS_POSSIBLE;
    }

    /// Leaving `AI_MOVE_OUT_OF_THE_WAY` (`AIMoveOutOfTheWayState::onExit`): drop the
    /// path and resume the interrupted order, routing again to its target.
    fn end_move_out(&mut self, id: u32) {
        let o = self.objects.get_mut(&id).unwrap();
        let resume = o.ai.move_out.take().and_then(|m| m.resume_target);
        o.ai.move_out_of_way = [None; 2];
        o.path.clear();
        o.target = None;
        o.traffic.stuck = false;
        o.blocked_frames = 0;
        let Some(target) = resume else { return };
        match self.route_to(id, target) {
            Ok(path) => {
                let o = self.objects.get_mut(&id).unwrap();
                o.path = path;
                o.path.push_front(Waypoint { position: [o.position[0], o.position[1]], layer: o.layer });
                o.target = Some(target);
                o.last_repath = self.frame;
            }
            Err(_) => self.end_order(id, Outcome::Failure),
        }
    }

    fn rebuild_partition(&mut self) {
        let entries = self.objects.values().map(|o| crate::partition::Entry {
            id: o.id,
            position: o.position,
            radius: o.body.radius,
            immobile: o.body.immobile,
        });
        // The map plus a margin; immobile objects are clipped to it.
        let margin = 4.0 * crate::partition::BUCKET;
        let size = [self.grid.width as f32 * CELL, self.grid.height as f32 * CELL];
        self.partition.rebuild(entries, ([-margin, -margin], [size[0] + margin, size[1] + margin]));
        // Only mobile radii widen queries (`Partition`).
        self.collision_radius_bound =
            self.objects.values().filter(|o| !o.body.immobile).map(|o| o.body.radius).fold(0.0, f32::max);
    }
    fn collider(&self, id: u32) -> Collider {
        let o = &self.objects[&id];
        Collider::object(o)
    }
    fn nearby(&self, id: u32, distance: f32) -> Vec<Collider> {
        let body = self.collider(id);
        self.partition
            .candidates(
                [body.position[0], body.position[1]],
                body.radius + distance,
                body.radius + self.collision_radius_bound + distance,
            )
            .into_iter()
            .filter(|other| *other != id)
            .map(|other| self.collider(other))
            .filter(|b| b.enabled)
            .collect()
    }
    fn integrate_ground(&mut self, id: u32) {
        let o = &self.objects[&id];
        if o.disabled.any() {
            let o = self.objects.get_mut(&id).unwrap();
            o.speed = 0.0;
            o.velocity = [0.0; 2];
            o.physics.vel = [0.0, 0.0, o.physics.vel[2]];
            return;
        }
        let resting = o.physics.vel == [0.0; 3] && o.physics.accel == [0.0; 3] && o.slide == [0.0; 2];
        if o.target.is_none() && resting {
            return;
        }
        // PhysicsBehavior::update: forces from this frame's locomotor call, gravity
        // and friction become velocity; the braking slide moves the body directly.
        let frame = self.frame;
        let (from, layer) = (o.position, o.layer);
        let ground_here = self.grid.height_at(&self.map, Waypoint { position: [from[0], from[1]], layer });
        let velocity = {
            let o = self.objects.get_mut(&id).unwrap();
            let mut loco = o.loco;
            let mut body = Body {
                position: &mut o.position,
                angle: &mut o.angle,
                physics: &mut o.physics,
                loco: &mut loco,
                major_radius: 0.0,
                bounding_radius: 0.0,
                frame,
            };
            let step = body.physics_update(crate::locomotion::DEFAULT_GRAVITY, false, ground_here);
            let v = [step[0] + o.slide[0], step[1] + o.slide[1]];
            o.slide = [0.0; 2];
            v
        };
        let o = &self.objects[&id];
        let radius = self.templates.objects[&o.template].radius();
        let to = [from[0] + velocity[0], from[1] + velocity[1]];
        // On the ground the original lets bodies move freely and the locomotor's
        // fixInvalidPosition pushes them off invalid cells. Deck edges still stop a
        // step, since falling off a bridge is not modelled yet.
        if layer != 0
            && !self.terrain(id).line_on(
                Waypoint { position: [from[0], from[1]], layer },
                Waypoint { position: to, layer },
                radius,
            )
        {
            let ground = self.grid.height_at(&self.map, Waypoint { position: [from[0], from[1]], layer });
            let o = self.objects.get_mut(&id).unwrap();
            o.velocity = [0.0; 2];
            o.physics.vel = [0.0, 0.0, o.physics.vel[2]];
            o.speed = 0.0;
            // Gravity was applied above; the body still rests on the ground.
            let mut loco = o.loco;
            Body {
                position: &mut o.position,
                angle: &mut o.angle,
                physics: &mut o.physics,
                loco: &mut loco,
                major_radius: 0.0,
                bounding_radius: 0.0,
                frame,
            }
            .clamp_to_ground(ground);
            return;
        }
        let mut body = self.collider(id);
        let dz = self.grid.height_at(&self.map, Waypoint { position: to, layer }) - from[2];
        body.bottom += dz.min(0.0);
        body.top += dz.max(0.0);
        let others: Vec<Collider> =
            // Structures are kept out of by their obstacle cells (and fixInvalidPosition):
            // AI units get no collision force from them (`processCollision`).
            self.nearby(id, ih_math::length(velocity))
                .into_iter()
                .filter(|c| !c.immobile && !self.crush_pair(id, c.id))
                .collect();
        let clip = |start: Collider, v: [f32; 2]| {
            let mut fraction = 1.0f32;
            let mut blocker = None;
            for other in &others {
                let f = traffic::step_fraction(start, v, *other);
                if f < fraction {
                    fraction = f;
                    blocker = Some(*other);
                }
            }
            (fraction, blocker)
        };
        let (fraction, hit) = clip(body, velocity);
        let mut to = [from[0] + velocity[0] * fraction, from[1] + velocity[1] * fraction];
        // Overlap is not allowed here (the original lets bodies overlap and sorts
        // them out with collision forces), so the rest of the step slides along the
        // contact instead of stopping: only motion into the other body is removed.
        let mut normal = None;
        if let Some(other) = hit {
            let n = ih_math::normalized([other.position[0] - to[0], other.position[1] - to[1]]);
            let rest = [velocity[0] * (1.0 - fraction), velocity[1] * (1.0 - fraction)];
            let into = rest[0] * n[0] + rest[1] * n[1];
            if into > 0.0 {
                let slide = [rest[0] - n[0] * into, rest[1] - n[1] * into];
                let at = body.with_position([to[0], to[1], body.position[2]]);
                let (f2, _) = clip(at, slide);
                to = [to[0] + slide[0] * f2, to[1] + slide[1] * f2];
            }
            normal = Some(n);
        }
        let ground = self.grid.height_at(&self.map, Waypoint { position: to, layer });
        let o = self.objects.get_mut(&id).unwrap();
        o.position[0] = to[0];
        o.position[1] = to[1];
        if let Some(n) = normal {
            let v = &mut o.physics.vel;
            let into = v[0] * n[0] + v[1] * n[1];
            if into > 0.0 {
                v[0] -= n[0] * into;
                v[1] -= n[1] * into;
            }
        }
        let mut loco = o.loco;
        let mut body = Body {
            position: &mut o.position,
            angle: &mut o.angle,
            physics: &mut o.physics,
            loco: &mut loco,
            major_radius: 0.0,
            bounding_radius: 0.0,
            frame,
        };
        body.clamp_to_ground(ground);
        o.speed = body.forward_speed_2d();
        let position = o.position;
        o.velocity = [position[0] - from[0], position[1] - from[1]];
        self.partition.relocate(id, from, position);
    }
    /// The partition manager's collision pass for ground bodies: every touching
    /// pair, in object-ID order, calls `onCollide` both ways.
    fn process_collisions(&mut self) {
        let mut pairs = Vec::new();
        for a in self.objects.keys().copied() {
            let ca = self.collider(a);
            if !ca.enabled || ca.immobile {
                continue;
            }
            // Pairs of immobile objects are left out (nothing moves either of them).
            for cb in self.nearby(a, traffic::CONTACT_RANGE) {
                if (cb.id <= a && !cb.immobile) || !ca.interacts(cb) {
                    continue;
                }
                let d = ih_math::length([cb.position[0] - ca.position[0], cb.position[1] - ca.position[1]]);
                if d <= ca.radius + cb.radius + traffic::CONTACT_RANGE {
                    pairs.push((a.min(cb.id), a.max(cb.id)));
                }
            }
        }
        pairs.sort_unstable();
        let mut squished = Vec::new();
        for (a, b) in pairs {
            for (x, y) in [(a, b), (b, a)] {
                if squished.iter().any(|(s, _)| *s == x || *s == y) {
                    continue;
                }
                // Collide modules run with physics: SquishCollide first.
                if self.squished_by(x, y) {
                    squished.push((x, y));
                    continue;
                }
                self.on_collide(x, y);
            }
        }
        for (victim, crusher) in squished {
            // HUGE_DAMAGE_AMOUNT of DAMAGE_CRUSH: until bodies and death modules exist
            // (M5), a squished object is removed outright.
            self.crushed.push((self.frame, victim, crusher));
            let _ = self.remove(victim);
        }
    }

    fn has_ai(&self, id: u32) -> bool {
        self.objects[&id].modules.iter().any(|m| m.kind == ModuleKind::Steering)
    }

    /// `PhysicsBehavior::onCollide` for two ground bodies. Crushing and overlap
    /// (`checkForOverlapCollision`) are not ported yet.
    fn on_collide(&mut self, a: u32, b: u32) {
        let (oa, ob) = (&self.objects[&a], &self.objects[&b]);
        let (ta, tb) = (&self.templates.objects[&oa.template], &self.templates.objects[&ob.template]);
        let b_physics = ob.modules.iter().any(|m| m.kind == ModuleKind::Physics);
        let b_immobile = tb.has_kind("IMMOBILE");
        if !b_physics && !b_immobile {
            return; // insubstantial
        }
        if !oa.modules.iter().any(|m| m.kind == ModuleKind::Physics) {
            return;
        }
        let immobile = ta.has_kind("IMMOBILE");
        if self.overlap_collision(a, b) {
            return; // overlap rather than bounce
        }
        // AI objects move under their own initiative, not by being bounced.
        if self.has_ai(a) && !self.process_collision(a, b) {
            return;
        }
        let (ca, cb) = (self.collider(a), self.collider(b));
        let delta = [cb.position[0] - ca.position[0], cb.position[1] - ca.position[1]];
        let dist2 = delta[0] * delta[0] + delta[1] * delta[1];
        let reach = ca.radius + cb.radius + traffic::CONTACT_RANGE;
        if dist2 > reach * reach {
            return;
        }
        let dist = ih_math::sqrt(dist2);
        let overlap = ca.radius + cb.radius - dist;
        let dist = dist.max(1.0);
        let frame = self.frame;
        let o = self.objects.get_mut(&a).unwrap();
        let mut loco = o.loco;
        let mut body = Body {
            position: &mut o.position,
            angle: &mut o.angle,
            physics: &mut o.physics,
            loco: &mut loco,
            major_radius: 0.0,
            bounding_radius: 0.0,
            frame,
        };
        let factor = if b_immobile {
            // Bounce off immobile objects like the ground (StructureStiffness 0.3),
            // dropping the previous velocity.
            let mass = if immobile { 999_999.0 } else { body.physics.data.mass };
            let mag = body.speed().max(1.0 / 150.0);
            body.physics.vel = [0.0; 3];
            -mag * mass * 0.3
        } else {
            -overlap.min(5.0)
        };
        body.apply_force([factor * delta[0] / dist, factor * delta[1] / dist, 0.0]);
    }

    /// `AIUpdateInterface::processCollision`: records whether `a` is blocked by `b`
    /// and returns whether physics should push it (never, for these cases).
    fn process_collision(&mut self, a: u32, b: u32) -> bool {
        if !self.has_ai(b) {
            return false;
        }
        // isDoingGroundMovement: the other needs a ground locomotor.
        let tb = &self.templates.objects[&self.objects[&b].template];
        if tb.locomotor().and_then(|n| self.templates.locomotors.get(n)).is_none_or(|l| !l.surface_movement()) {
            return false;
        }
        if self.objects[&a].traffic.ignore_until > self.frame {
            return false;
        }
        let self_moving = self.objects[&a].target.is_some();
        let other_moving = self.objects[&b].target.is_some();
        if !self_moving || !self.blocked_by_object(a, b) {
            return false;
        }
        let max = self.max_blocked_speed(a, b);
        let a_infantry = self.templates.objects[&self.objects[&a].template].has_kind("INFANTRY");
        let b_infantry = self.templates.objects[&self.objects[&b].template].has_kind("INFANTRY");
        let need_a = self.need_to_rotate(a);
        let deadlock = other_moving
            && !need_a
            && self.blocked_by_object(b, a)
            && !self.need_to_rotate(b)
            && !self.collider(a).higher_priority(self.collider(b));
        let o = self.objects.get_mut(&a).unwrap();
        o.traffic.is_blocked = true;
        o.blocked_by = Some(b);
        o.traffic.max_blocked_speed = o.traffic.max_blocked_speed.min(max);
        let away = self.is_moving_away_from(b, a);
        if !away && b_infantry && !a_infantry {
            // The original skips infantry using an ability or busy; neither exists yet.
            self.move_away_from_unit(b, a);
            return false;
        }
        let o = self.objects.get_mut(&a).unwrap();
        if o.blocked_frames == 0 {
            o.blocked_frames = 1;
        }
        if !need_a {
            if !other_moving {
                o.traffic.stuck = true;
            } else if deadlock {
                // Deadlocked: the lower-priority unit gets out of the way.
                self.move_away_from_unit(a, b);
            }
        } else {
            // Rotating: don't accumulate blocked frames.
            o.blocked_frames = 1;
        }
        false
    }

    /// `AIUpdateInterface::blockedBy`. Dead states are not modelled.
    fn blocked_by_object(&self, a: u32, b: u32) -> bool {
        let (oa, ob) = (&self.objects[&a], &self.objects[&b]);
        let pos = [oa.position[0], oa.position[1]];
        if let Some(goal) = oa.target
            && (goal[0] - pos[0]).abs() < CELL
            && (goal[1] - pos[1]).abs() < CELL
        {
            return false; // approaching the goal: ignore obstacles
        }
        if self.can_crush_or_squish(a, b, CrushTest::Either) {
            return false; // just run over them
        }
        if oa.loco.moving_backwards {
            return false;
        }
        let other_moving = ob.target.is_some();
        let (ta, tb) = (&self.templates.objects[&oa.template], &self.templates.objects[&ob.template]);
        let our = [ih_math::cos(oa.angle), ih_math::sin(oa.angle)];
        let their = [ih_math::cos(ob.angle), ih_math::sin(ob.angle)];
        let dot = our[0] * their[0] + our[1] * their[1];
        if ta.has_kind("INFANTRY") && tb.has_kind("INFANTRY") && dot <= 0.25 {
            return false; // crossing infantry pass through
        }
        let (mut dx, mut dy) = (pos[0] - ob.position[0], pos[1] - ob.position[1]);
        let cur = dx * dx + dy * dy;
        if cur < CELL * CELL * 0.0001 {
            return self.collider(a).higher_priority(self.collider(b));
        }
        if oa.blocked_frames > 30 && dot <= 0.0 {
            return false;
        }
        let rel = |o: &Object, p: [f32; 2]| {
            ih_math::angle_delta(o.angle, ih_math::atan2(p[1] - o.position[1], p[0] - o.position[0]))
        };
        let collision = rel(oa, [ob.position[0], ob.position[1]]);
        let other_angle = rel(ob, pos);
        let pi = std::f32::consts::PI;
        if collision.abs() > pi / 2.0 {
            return false; // moving away
        }
        let mut limit = pi / 4.0;
        if !other_moving {
            limit *= 0.75;
        }
        if collision.abs() > limit {
            if dot <= 0.0 {
                return false;
            }
            if other_moving && other_angle.abs() > limit {
                dx += our[0] - their[0];
                dy += our[1] - their[1];
                if cur > dx * dx + dy * dy {
                    if self.collider(a).higher_priority(self.collider(b)) {
                        return false;
                    }
                } else {
                    return false; // moving away from each other
                }
            } else {
                return false;
            }
        }
        true
    }

    /// `AIUpdateInterface::calculateMaxBlockedSpeed`.
    fn max_blocked_speed(&self, a: u32, b: u32) -> f32 {
        let (oa, ob) = (&self.objects[&a], &self.objects[&b]);
        let current = oa.traffic.max_blocked_speed;
        let to = ih_math::normalized([ob.position[0] - oa.position[0], ob.position[1] - oa.position[1]]);
        let dot = to[0] * ih_math::cos(ob.angle) + to[1] * ih_math::sin(ob.angle);
        if dot < 0.0 {
            return 0.0; // running into us
        }
        let away = ih_math::length([ob.physics.vel[0], ob.physics.vel[1]]) * dot;
        let ours = to[0] * ih_math::cos(oa.angle) + to[1] * ih_math::sin(oa.angle);
        if ours <= 0.0 {
            return current;
        }
        (away / ours).min(current)
    }

    /// `AIUpdateInterface::needToRotate`: more than 6 degrees off the path goal.
    fn need_to_rotate(&self, id: u32) -> bool {
        let o = &self.objects[&id];
        let t = &self.templates.objects[&o.template];
        if t.locomotor().and_then(|n| self.templates.locomotors.get(n)).is_some_and(|l| l.wander_width_factor > 0.0) {
            return false;
        }
        if o.path.len() < 2 {
            return false;
        }
        let mut probe = o.clone();
        let (goal, _, _) = point_on_path(&mut probe, self.terrain(id), &self.map, t.radius());
        let rel = ih_math::angle_delta(o.angle, ih_math::atan2(goal[1] - o.position[1], goal[0] - o.position[0]));
        rel.abs() > std::f32::consts::PI / 30.0
    }
    /// `AIUpdateInterface::isMovingAwayFrom`.
    fn is_moving_away_from(&self, id: u32, from: u32) -> bool {
        let o = &self.objects[&id];
        o.ai.move_out.is_some() && o.ai.move_out_of_way.contains(&Some(from))
    }

    /// `AIUpdateInterface::privateMoveAwayFromUnit`: step off `unit`'s path (and the
    /// path of the unit made way for before) for up to ten seconds.
    fn move_away_from_unit(&mut self, id: u32, unit: u32) {
        let mobile = {
            let o = &self.objects[&id];
            let t = &self.templates.objects[&o.template];
            self.has_ai(id)
                && !t.has_kind("IMMOBILE")
                && t.locomotor().and_then(|n| self.templates.locomotors.get(n)).is_some_and(|l| l.surface_movement())
                && !o.disabled.any()
        };
        if !mobile {
            return;
        }
        let frame = self.frame;
        let o = self.objects.get_mut(&id).unwrap();
        if o.ai.move_out.is_some() && o.ai.move_out_of_way.contains(&Some(unit)) {
            if o.traffic.is_blocked {
                o.traffic.ignore_until = frame + 60; // cheat for 2 seconds
            }
            return;
        }
        o.ai.move_out_of_way = [Some(unit), o.ai.move_out_of_way[0]];
        let second = o.ai.move_out_of_way[1];
        if self.objects[&unit].path.len() < 2 {
            return;
        }
        let Some(path) = self.move_away_path(id, unit, second) else { return };
        let o = self.objects.get_mut(&id).unwrap();
        let resume = match o.ai.move_out.take() {
            Some(m) => m.resume_target,
            None => o.target,
        };
        let goal = path.back().map(|w| w.position);
        o.path = path;
        o.target = goal;
        o.blocked_frames = 0;
        o.traffic.stuck = false;
        o.ai.move_out = Some(crate::ai::MoveOutOfWay { until: frame + 300, resume_target: resume });
        self.move_asides += 1;
        // Pathfinder::moveAllies (asking allies on the new path to move too) is not ported.
        self.wake_ai(id);
    }

    /// `Pathfinder::getMoveAwayFromPath`: the cheapest reachable cell whose box,
    /// grown by both units' sizes, no longer touches the paths to avoid.
    fn move_away_path(&self, id: u32, unit: u32, second: Option<u32>) -> Option<VecDeque<Waypoint>> {
        let o = &self.objects[&id];
        let t = &self.templates.objects[&o.template];
        let other = &self.objects[&unit];
        let ot = &self.templates.objects[&other.template];
        // Pathfinder::getRadiusAndCenter
        let cells = |t: &crate::templates::Template| {
            let mut d = 2.0 * t.collision_radius();
            if d > CELL && d < 2.0 * CELL {
                d = 2.0 * CELL;
            }
            let mut r = ((d / CELL) + 0.3).floor() as i32;
            if r == 0 {
                r = 1;
            }
            let mut center = r & 1 == 1;
            r /= 2;
            if r > 2 {
                r = 2;
                center = true;
            }
            (r, center)
        };
        let (radius, center) = cells(t);
        let (other_radius, other_center) = cells(ot);
        let mut half = radius as f32 * CELL - CELL / 4.0;
        if center {
            half += CELL / 2.0;
        }
        half += other_radius as f32 * CELL;
        if other_center {
            half += CELL / 2.0;
        }
        let mut segments: Vec<([f32; 2], [f32; 2])> =
            other.path.iter().zip(other.path.iter().skip(1)).map(|(a, b)| (a.position, b.position)).collect();
        if let Some(s) = second.and_then(|s| self.objects.get(&s)) {
            segments.extend(s.path.iter().zip(s.path.iter().skip(1)).map(|(a, b)| (a.position, b.position)));
        }
        let layer = o.layer;
        let body_radius = t.radius();
        let mut start_pos = [o.position[0], o.position[1]];
        if !center {
            // The original adds half a cell to x twice here.
            start_pos[0] += CELL;
        }
        let start = ((start_pos[0] / CELL).floor() as i32, (start_pos[1] / CELL).floor() as i32);
        let cell_point = |c: (i32, i32)| {
            if center {
                [c.0 as f32 * CELL + CELL / 2.0, c.1 as f32 * CELL + CELL / 2.0]
            } else {
                [c.0 as f32 * CELL, c.1 as f32 * CELL]
            }
        };
        let me = self.collider(id);
        let free = |p: [f32; 2]| {
            let ground = self.grid.height_at(&self.map, Waypoint { position: p, layer });
            let body = me.with_position([p[0], p[1], ground]);
            self.objects.keys().filter(|k| **k != id).all(|k| {
                let c = self.collider(*k);
                !body.interacts(c)
                    || ih_math::length([c.position[0] - p[0], c.position[1] - p[1]]) >= body.radius + c.radius
            })
        };
        // Dijkstra over cells: orthogonal 10, diagonal 14 (COST_ORTHOGONAL/DIAGONAL).
        let mut open = std::collections::BinaryHeap::new();
        // cell -> (cost, parent cell)
        type Cell = (i32, i32);
        let mut best: BTreeMap<Cell, (u32, Option<Cell>)> = BTreeMap::new();
        best.insert(start, (0, None));
        open.push(std::cmp::Reverse((0u32, start)));
        let mut expanded = 0;
        while let Some(std::cmp::Reverse((cost, cell))) = open.pop() {
            if best.get(&cell).is_some_and(|(c, _)| *c < cost) {
                continue;
            }
            expanded += 1;
            if expanded > 4000 {
                return None;
            }
            let p = cell_point(cell);
            let lo = [p[0] - half, p[1] - half];
            let hi = [p[0] + half, p[1] + half];
            let overlap = cell == start || segments.iter().any(|(a, b)| segment_in_box(*a, *b, lo, hi));
            if !overlap && free(p) {
                let mut chain = vec![cell];
                let mut cur = cell;
                while let Some((_, Some(prev))) = best.get(&cur) {
                    chain.push(*prev);
                    cur = *prev;
                }
                chain.reverse();
                let mut path: VecDeque<Waypoint> =
                    chain.into_iter().skip(1).map(|c| Waypoint { position: cell_point(c), layer }).collect();
                path.push_front(Waypoint { position: [o.position[0], o.position[1]], layer });
                return Some(path);
            }
            for (dx, dy, step) in
                [(1, 0, 10), (-1, 0, 10), (0, 1, 10), (0, -1, 10), (1, 1, 14), (1, -1, 14), (-1, 1, 14), (-1, -1, 14)]
            {
                let n = (cell.0 + dx, cell.1 + dy);
                if !self.terrain(id).passable_on(layer, n.0, n.1, body_radius) {
                    continue;
                }
                let c = cost + step;
                if best.get(&n).is_none_or(|(b, _)| c < *b) {
                    best.insert(n, (c, Some(cell)));
                    open.push(std::cmp::Reverse((c, n)));
                }
            }
        }
        None
    }

    /// `Object::canCrushOrSquish`. Allies (same owner, until players and teams
    /// exist) never crush each other.
    pub fn can_crush_or_squish(&self, a: u32, b: u32, test: CrushTest) -> bool {
        let (Some(oa), Some(ob)) = (self.objects.get(&a), self.objects.get(&b)) else { return false };
        if oa.disabled.has(DisabledReason::Unmanned) || oa.owner == ob.owner {
            return false;
        }
        let (ta, tb) = (&self.templates.objects[&oa.template], &self.templates.objects[&ob.template]);
        let crusher = ta.number("CrusherLevel", 0.0) as i32;
        if crusher <= 0 {
            return false;
        }
        if test != CrushTest::Crush && tb.modules.iter().any(|m| m.class == "SquishCollide") {
            return true;
        }
        test != CrushTest::Squish && crusher > tb.number("CrushableLevel", 255.0) as i32
    }

    /// Either body may drive over the other.
    fn crush_pair(&self, a: u32, b: u32) -> bool {
        self.can_crush_or_squish(a, b, CrushTest::Either) || self.can_crush_or_squish(b, a, CrushTest::Either)
    }

    /// `SquishCollide::onCollide`: an enemy crusher whose geometry reaches this
    /// body's centre (radius 1) while moving toward it crushes it.
    fn squished_by(&self, a: u32, b: u32) -> bool {
        let (oa, ob) = (&self.objects[&a], &self.objects[&b]);
        let ta = &self.templates.objects[&oa.template];
        if !ta.modules.iter().any(|m| m.class == "SquishCollide") {
            return false;
        }
        let tb = &self.templates.objects[&ob.template];
        if tb.number("CrusherLevel", 0.0) <= 0.0 || oa.owner == ob.owner {
            return false;
        }
        if !ob.modules.iter().any(|m| m.kind == ModuleKind::Physics) {
            return false;
        }
        let to = [oa.position[0] - ob.position[0], oa.position[1] - ob.position[1]];
        if ih_math::length(to) > self.collider(b).radius + 1.0 {
            return false;
        }
        to[0] * ob.physics.vel[0] + to[1] * ob.physics.vel[1] > 0.0
    }

    /// `PhysicsBehavior::checkForOverlapCollision`: a moving crusher overlaps what it
    /// can crush, and the crushee stays passive. The crush damage to vehicles
    /// (front/back crush points) waits for bodies and damage.
    fn overlap_collision(&self, a: u32, b: u32) -> bool {
        let v = self.objects[&a].physics.vel;
        if v.iter().all(|c| c.abs() < 0.01) {
            return false;
        }
        let crushing = self.can_crush_or_squish(a, b, CrushTest::Crush);
        let crushed = self.can_crush_or_squish(b, a, CrushTest::Crush);
        if crushing && crushed {
            return false;
        }
        crushed || crushing
    }

    fn repath_traffic(&mut self, id: u32) {
        let o = &self.objects[&id];
        // AIInternalMoveToState: recompute when blocked and stuck, or blocked for two
        // seconds; ironhour also spaces recomputations by REPATH_DELAY.
        if !(o.traffic.stuck || o.blocked_frames > 60)
            || self.frame.saturating_sub(o.last_repath) < traffic::REPATH_DELAY
        {
            return;
        }
        let Some(target) = o.target else {
            return;
        };
        let body = self.collider(id);
        if let Some(other) = o.blocked_by.filter(|id| self.objects.contains_key(id)).map(|id| self.collider(id))
            && other.moving
            && body.higher_priority(other)
            && o.blocked_frames < 3 * traffic::REPATH_DELAY
        {
            return;
        }
        let from = Waypoint { position: [o.position[0], o.position[1]], layer: o.layer };
        let goal = self.grid.destination(&self.map, target);
        let obstacles: Vec<_> = self
            .objects
            .keys()
            .copied()
            .filter(|other| *other != id)
            .map(|id| self.collider(id))
            .filter(|b| b.enabled)
            .collect();
        let blocked = |p: Waypoint| {
            let candidate = body.with_position([p.position[0], p.position[1], self.grid.height_at(&self.map, p)]);
            obstacles.iter().any(|other| {
                if !candidate.interacts(*other) {
                    return false;
                }
                let delta = [p.position[0] - other.position[0], p.position[1] - other.position[1]];
                let d2 = delta[0] * delta[0] + delta[1] * delta[1];
                let r = body.radius + other.radius + 2.0;
                if d2 >= r * r {
                    return false;
                }
                // Contact padding must not trap a unit at the beginning of its
                // new path. Permit only motion away from that initial contact.
                let initial = [from.position[0] - other.position[0], from.position[1] - other.position[1]];
                let initial2 = initial[0] * initial[0] + initial[1] * initial[1];
                let progress = [p.position[0] - from.position[0], p.position[1] - from.position[1]];
                !(initial2 < r * r && d2 >= initial2 && progress[0] * progress[0] + progress[1] * progress[1] <= 100.0)
            })
        };
        let segment_clear = |a: Waypoint, b: Waypoint| {
            let candidate = body.with_position([a.position[0], a.position[1], self.grid.height_at(&self.map, a)]);
            let velocity = [b.position[0] - a.position[0], b.position[1] - a.position[1]];
            obstacles.iter().all(|other| traffic::step_fraction(candidate, velocity, *other) >= 1.0)
        };
        let path = self.terrain(id).path_avoiding(
            from,
            goal,
            self.templates.objects[&o.template].radius(),
            &blocked,
            &segment_clear,
        );
        let o = self.objects.get_mut(&id).unwrap();
        o.last_repath = self.frame;
        if let Some(path) = path {
            o.path = path.into();
            // The first node is the start of the current segment (`Path` keeps its start node).
            o.path.push_front(Waypoint { position: [o.position[0], o.position[1]], layer: o.layer });
            o.traffic_repaths += 1;
            o.blocked_frames = 0;
            o.blocked_by = None;
            o.traffic.stuck = false;
        }
    }
    /// Live colliding object pairs, for headless invariants and diagnostics.
    pub fn overlaps(&self) -> Vec<(u32, u32)> {
        let mut pairs = Vec::new();
        for id in self.objects.keys().copied() {
            let a = self.collider(id);
            if a.immobile {
                continue;
            }
            for b in self.nearby(id, 0.0) {
                if id < b.id && !a.immobile && !b.immobile && traffic::overlaps(a, b) && !self.crush_pair(id, b.id) {
                    pairs.push((id, b.id));
                }
            }
        }
        pairs
    }
    pub fn state_hash(&self) -> u64 {
        let mut h = StateHash::default();
        h.u32(11);
        h.u64(self.setup_hash);
        h.u32(self.frame);
        h.u32(self.next_id);
        h.u64(self.sequence);
        h.u32(self.failed_orders);
        h.u32(self.move_asides);
        h.u64(self.crushed.len() as u64);
        for (f, v, c) in &self.crushed {
            h.u32(*f);
            h.u32(*v);
            h.u32(*c);
        }
        for v in self.rng.state {
            h.u32(v);
        }
        h.u64(self.objects.len() as u64);
        for o in self.objects.values() {
            h.u32(o.id);
            h.text(&o.template);
            h.u32(o.owner);
            h.u32(o.map_index.map_or(0, |i| i as u32 + 1));
            h.u32(o.layer);
            h.u32(o.blocked_frames);
            h.u32(o.blocked_by.unwrap_or(0));
            h.u32(o.last_repath);
            h.u32(o.traffic_repaths);
            h.u32(o.ai.state as u32);
            h.u32(o.ai.entered_frame);
            h.u32(o.ai.transitions);
            h.u32(o.ai.outcome as u32);
            h.u32(o.ai.completed_orders);
            h.u32(o.ai.failed_orders);
            h.u32(o.ai.interrupted_orders);
            h.u64(o.ai.remaining.len() as u64);
            for p in &o.ai.remaining {
                h.float(p[0]);
                h.float(p[1]);
            }
            h.u32(u32::from(o.ai.face_position.is_some()));
            if let Some(p) = o.ai.face_position {
                h.float(p[0]);
                h.float(p[1]);
            }
            h.u32(o.ai.face_object.unwrap_or(0));
            h.u32(o.disabled.mask);
            for t in o.disabled.until {
                h.u32(t);
            }
            for v in o.position.into_iter().chain(o.previous_position).chain([o.angle, o.speed]).chain(o.velocity) {
                h.float(v);
            }
            let tr = &o.traffic;
            h.u32(u32::from(tr.is_blocked) | u32::from(tr.stuck) << 1);
            h.u32(tr.ignore_until);
            h.u32(o.ai.move_out.as_ref().map_or(0, |m| m.until));
            for v in o.ai.move_out.as_ref().and_then(|m| m.resume_target).unwrap_or([f32::NAN; 2]) {
                h.float(v);
            }
            for w in o.ai.move_out_of_way {
                h.u32(w.unwrap_or(0));
            }
            h.float(tr.max_blocked_speed);
            h.float(tr.bump_speed_limit);
            for v in o.physics.vel.into_iter().chain(o.slide) {
                h.float(v);
            }
            h.u32(o.physics.motive_expires);
            h.u32(u32::from(o.physics.braking_status) | u32::from(o.physics.ever_run) << 1);
            let l = &o.loco;
            h.u32(
                u32::from(l.is_braking)
                    | u32::from(l.moving_backwards) << 1
                    | u32::from(l.three_point_turn) << 2
                    | u32::from(l.offset_increasing) << 3,
            );
            for v in [l.braking_factor, l.angle_offset, l.offset_increment] {
                h.float(v);
            }
            h.u32(l.donut_timer);
            h.u32(u32::from(o.target.is_some()));
            if let Some(p) = o.target {
                for v in p {
                    h.float(v);
                }
            }
            h.u64(o.path.len() as u64);
            for p in &o.path {
                h.u32(p.layer);
                for v in &p.position {
                    h.float(*v);
                }
            }
            h.u64(o.modules.len() as u64);
            for m in &o.modules {
                h.text(&m.class);
                h.text(&m.tag);
                h.u32(m.kind as u32);
                h.u32(m.updates);
            }
        }
        h.u64(self.scheduler.heap.len() as u64);
        for e in &self.scheduler.heap {
            h.u32(e.key.0);
            h.u64(e.key.1 as u64);
            h.u32(e.frame);
            h.u32(e.phase as u32);
        }
        h.u64(self.commands.len() as u64);
        for ((f, seq), c) in &self.commands {
            h.u32(*f);
            h.u64(*seq);
            hash_command(c, &mut h);
        }
        h.0
    }
}
fn hash_command(c: &Command, h: &mut StateHash) {
    match c {
        Command::Move { object, target } => {
            h.u32(0);
            h.u32(*object);
            for v in target {
                h.float(*v);
            }
        }
        Command::Stop { object } => {
            h.u32(1);
            h.u32(*object);
        }
        Command::Destroy { object } => {
            h.u32(2);
            h.u32(*object);
        }
        Command::Spawn { template, position, owner } => {
            h.u32(3);
            h.text(template);
            for v in position {
                h.float(*v);
            }
            h.u32(*owner);
        }
        Command::FollowPath { object, points } => {
            h.u32(4);
            h.u32(*object);
            h.u64(points.len() as u64);
            for p in points {
                h.float(p[0]);
                h.float(p[1]);
            }
        }
        Command::Face { object, target } => {
            h.u32(5);
            h.u32(*object);
            h.float(target[0]);
            h.float(target[1]);
        }
        Command::FaceObject { object, target } => {
            h.u32(6);
            h.u32(*object);
            h.u32(*target);
        }
        Command::Disable { object, reason, frames } => {
            h.u32(7);
            h.u32(*object);
            h.u32(*reason as u32);
            h.u32(u32::from(frames.is_some()));
            if let Some(n) = frames {
                h.u32(*n);
            }
        }
        Command::Enable { object, reason } => {
            h.u32(8);
            h.u32(*object);
            h.u32(*reason as u32);
        }
    }
}
/// The goal point and remaining distance along a path (`Path::computePointOnPath`).
/// `path[0]` is the start of the current segment. Advances `path` past segments
/// the object has moved beyond (switching layer where allowed).
fn point_on_path(o: &mut Object, grid: &Terrain, map: &Map, radius: f32) -> ([f32; 3], f32, u32) {
    let pos = [o.position[0], o.position[1]];
    let sub = |a: [f32; 2], b: [f32; 2]| [a[0] - b[0], a[1] - b[1]];
    let dot = |a: [f32; 2], b: [f32; 2]| a[0] * b[0] + a[1] * b[1];
    let segment = |a: Waypoint, b: Waypoint| {
        let d = sub(b.position, a.position);
        let len = ih_math::length(d);
        (if len > 0.0 { [d[0] / len, d[1] / len] } else { [0.0; 2] }, len)
    };
    // Find the closest segment.
    let mut close = None;
    let mut best = f32::MAX;
    let (mut total, mut to_pos) = (0.0f32, 0.0f32);
    let n = o.path.len();
    for i in 0..n - 1 {
        let (a, b) = (o.path[i], o.path[i + 1]);
        let (dir, len) = segment(a, b);
        let mut along = dot(dir, sub(pos, a.position));
        let p = if along < 0.0 {
            along = 0.0;
            a.position
        } else if along > len {
            if i + 2 == n {
                along = len;
                b.position
            } else {
                // Beyond this segment: a sharp bend's next segment claims the point.
                total += len;
                continue;
            }
        } else {
            [a.position[0] + along * dir[0], a.position[1] + along * dir[1]]
        };
        let off = sub(pos, p);
        let d2 = dot(off, off);
        if d2 < best {
            best = d2;
            close = Some(i);
            to_pos = total + along;
        }
        total += len;
    }
    let mut dist_along = total - to_pos;
    // Drop the nodes before the closest segment.
    for _ in 0..close.unwrap_or(0) {
        let next = o.path[1];
        if next.layer != o.layer {
            if !grid.can_transition(Waypoint { position: pos, layer: o.layer }, next, 0.0) {
                break;
            }
            o.layer = next.layer;
            o.position[2] = grid.height_at(map, Waypoint { position: pos, layer: o.layer });
        }
        o.path.pop_front();
    }
    let here = Waypoint { position: pos, layer: o.layer };
    let (a, b) = (o.path[0], o.path[1]);
    let (dir, len) = segment(a, b);
    let mut along = dot(dir, sub(pos, a.position)).max(0.0);
    let to = sub(pos, a.position);
    let off2 = dot(to, to) - along * along;
    let off = if off2 <= 0.0 { 0.0 } else { ih_math::sqrt(off2) };
    // Basically on the path: aim at the next node. The farther off the path, the
    // closer the goal moves to the projected point.
    let mut k = (off / (3.0 * crate::locomotion::CELL)).min(1.0);
    let mut goal = None;
    if grid.line_on(here, b, radius) {
        goal = Some(b.position);
        let mut try_ahead = along > len * 0.5 && a.layer == b.layer && o.layer == 0;
        let very_close = len - along < 1.0;
        if very_close {
            try_ahead = true;
        }
        if try_ahead && let Some(next) = o.path.get(2) {
            let mid = [(b.position[0] + next.position[0]) * 0.5, (b.position[1] + next.position[1]) * 0.5];
            // The original skips this check when very close; without fixInvalidPosition
            // a cut corner would stall against the terrain, so it is always checked.
            if grid.line_on(
                Waypoint { position: pos, layer: b.layer },
                Waypoint { position: mid, layer: b.layer },
                radius,
            ) {
                goal = Some(mid);
            }
        }
    } else if k > 0.5 {
        let try_dist = along + 0.5 * (len - along);
        let p = [a.position[0] + try_dist * dir[0], a.position[1] + try_dist * dir[1]];
        if grid.line_on(here, Waypoint { position: p, layer: o.layer }, radius) {
            k = 0.5;
            goal = Some(p);
        }
    }
    along += (1.0 - k) * (len - along);
    let goal = goal.unwrap_or_else(|| {
        if along > len {
            b.position
        } else {
            let p = [a.position[0] + along * dir[0], a.position[1] + along * dir[1]];
            if (pos[0] - p[0]).abs() < 1.0 && (pos[1] - p[1]).abs() < 1.0 && o.path.len() > 2 {
                o.path[2].position
            } else {
                p
            }
        }
    });
    let delta = ih_math::length(sub(goal, pos));
    // PATHFIND_CLOSE_ENOUGH
    if delta > dist_along && dist_along > 1.0 {
        dist_along = delta;
    }
    let layer = b.layer;
    ([goal[0], goal[1], grid.height_at(map, Waypoint { position: goal, layer })], dist_along, layer)
}

/// Follows the path with the object's locomotor (`AIUpdate` path following calling
/// `Locomotor::locoUpdate_moveTowardsPosition`, or `locoUpdate_maintainCurrentPosition`
/// once the path is done). `radii` are the major, bounding and path radii.
#[allow(clippy::too_many_arguments)]
fn steer(
    o: &mut Object,
    l: &Locomotor,
    grid: &Terrain,
    map: &Map,
    frame: u32,
    radii: (f32, f32, f32),
    speed: f32,
    blocked: &mut bool,
) {
    let pos = [o.position[0], o.position[1]];
    let arrived = o.path.len() < 2
        || (o.path.len() == 2 && {
            let last = o.path[1];
            if ih_math::length([last.position[0] - pos[0], last.position[1] - pos[1]]) < 0.5 {
                if last.layer != o.layer && grid.can_transition(Waypoint { position: pos, layer: o.layer }, last, 0.0) {
                    o.layer = last.layer;
                    o.position[2] = grid.height_at(map, Waypoint { position: pos, layer: o.layer });
                }
                true
            } else {
                false
            }
        });
    if arrived {
        o.path.clear();
        o.target = None;
        let mut body = Body {
            position: &mut o.position,
            angle: &mut o.angle,
            physics: &mut o.physics,
            loco: &mut o.loco,
            major_radius: radii.0,
            bounding_radius: radii.1,
            frame,
        };
        body.maintain_current_position(l, heights(grid, map, pos, o.layer));
        o.speed = 0.0;
        o.velocity = [0.0; 2];
        o.blocked_frames = 0;
        o.blocked_by = None;
        o.traffic.stuck = false;
        return;
    }
    // Flyers take the straight line (`computeQuickPath`): their path is start and goal.
    let (goal, remaining, _) = point_on_path(o, grid, map, radii.2);
    let layer = o.layer;
    // validMovementTerrain: the cell under the point is passable on this layer.
    let valid = |p: [f32; 2]| {
        let c = crate::locomotion::CELL;
        grid.passable_on(layer, (p[0] / c).floor() as i32, (p[1] / c).floor() as i32, 0.0)
    };
    let mut body = Body {
        position: &mut o.position,
        angle: &mut o.angle,
        physics: &mut o.physics,
        loco: &mut o.loco,
        major_radius: radii.0,
        bounding_radius: radii.1,
        frame,
    };
    let before = [body.position[0], body.position[1]];
    let z = heights(grid, map, before, layer);
    let slide = body.move_towards_position(l, goal, remaining, speed, blocked, &valid, z);
    // Turning about an off-centre pivot moves the body; that motion goes through
    // the same collision clipping as the rest of this frame's displacement.
    let pivot = [o.position[0] - before[0], o.position[1] - before[1]];
    o.position[0] = before[0];
    o.position[1] = before[1];
    o.slide = [slide[0] + pivot[0], slide[1] + pivot[1]];
}

/// Integer nanosecond accumulator: clients interpolate between 30 Hz snapshots.
#[derive(Default)]
pub struct LogicClock {
    units: u128,
}
impl LogicClock {
    pub fn advance(&mut self, elapsed: std::time::Duration) -> u32 {
        self.units += elapsed.as_nanos() * 30;
        let n = self.units / 1_000_000_000;
        self.units %= 1_000_000_000;
        n as u32
    }
    pub fn alpha(&self) -> f32 {
        self.units as f32 / 1_000_000_000.0
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use ih_ini::definitions::Definitions;
    fn sim() -> Simulation {
        let mut defs = Definitions::default();
        defs.add_document(ih_ini::parse(b"Object Tank\nLocomotor = SET_NORMAL L\nGeometryMajorRadius = 1\nBehavior = AIUpdateInterface AI\nEnd\nBehavior = PhysicsBehavior P\nEnd\nEnd\nLocomotor L\nSurfaces = GROUND\nSpeed = 30\nAcceleration = 900\nBraking = 900\nTurnRate = 360\nEnd\n"));
        Simulation::new(
            Arc::new(Map {
                width: 24,
                height: 24,
                border: 2,
                boundaries: vec![[20, 20]],
                heights: vec![0; 24 * 24],
                ..Default::default()
            }),
            Arc::new(Templates::from_definitions(&defs)),
            1,
        )
    }
    #[test]
    fn replay_matches_every_tick_and_stops_at_the_goal() {
        let mut a = sim();
        let mut b = sim();
        for s in [&mut a, &mut b] {
            s.spawn("Tank", [25.0, 25.0], 0).unwrap();
            s.enqueue(1, Command::Move { object: 1, target: [125.0, 25.0] }).unwrap();
        }
        for _ in 0..180 {
            a.step();
            b.step();
            assert_eq!(a.state_hash(), b.state_hash());
        }
        assert!((a.objects[&1].position[0] - 125.0).abs() < 0.5);
        assert!(a.objects[&1].target.is_none());
        assert_eq!(a.state_hash(), 0xc43c1aec5d1675a0, "portable synthetic movement fixture changed");
        b.rng.next_u32();
        assert_ne!(a.state_hash(), b.state_hash());
    }
    #[test]
    fn resting_physics_sleeps_until_a_force_wakes_it() {
        let mut a = sim();
        a.spawn("Tank", [25.0, 25.0], 0).unwrap();
        let physics = a.objects[&1].modules.iter().position(|m| m.kind == ModuleKind::Physics).unwrap();
        let wake = |s: &Simulation| s.scheduler.heap.iter().find(|e| e.key == (1, physics)).unwrap().frame;
        // The first update always runs (UPDATE_EVER_RUN); a body at rest then sleeps.
        for _ in 0..3 {
            a.step();
        }
        assert!(wake(&a) >= FOREVER, "asleep");
        let updates = a.objects[&1].modules[physics].updates;
        // The order wakes the AI; its first motive force wakes physics for the
        // frame after, so the body does not move in the frame the force arrives.
        a.enqueue(a.frame, Command::Move { object: 1, target: [125.0, 25.0] }).unwrap();
        let start = a.objects[&1].position;
        let mut woken_at = None;
        for _ in 0..3 {
            a.step();
            if woken_at.is_none() && wake(&a) == a.frame {
                woken_at = Some(a.frame);
                assert_eq!(a.objects[&1].modules[physics].updates, updates);
                assert_eq!(a.objects[&1].position, start);
                assert!(a.objects[&1].physics.motive_expires > a.frame - 1, "a motive force arrived");
            }
        }
        assert!(woken_at.is_some(), "physics was never woken");
        for _ in 0..180 {
            a.step();
        }
        assert!((a.objects[&1].position[0] - 125.0).abs() < 0.5);
        // Stopped at the goal: asleep again.
        assert!(wake(&a) >= FOREVER, "asleep");
    }
    #[test]
    fn future_commands_are_hashed_and_destroy_removes_scheduled_modules() {
        let mut a = sim();
        a.spawn("Tank", [25.0, 25.0], 0).unwrap();
        let before = a.state_hash();
        a.enqueue(2, Command::Destroy { object: 1 }).unwrap();
        assert_ne!(before, a.state_hash());
        for _ in 0..3 {
            a.step();
        }
        assert!(a.objects.is_empty());
        assert!(a.scheduler.heap.is_empty());
    }
    #[test]
    fn navigation_cache_warmup_does_not_change_simulation_state_or_paths() {
        let mut a = traffic_sim();
        let mut b = traffic_sim();
        for s in [&mut a, &mut b] {
            s.spawn("Tank", [25.0, 105.0], 0).unwrap();
            s.spawn("Tank", [95.0, 105.0], 0).unwrap();
            s.enqueue(1, Command::Move { object: 1, target: [175.0, 105.0] }).unwrap();
        }
        for radius in [15.0, 1.0, 25.0, 10.0] {
            a.grid.zone_summary(radius);
        }
        assert_eq!(a.state_hash(), b.state_hash());
        for _ in 0..600 {
            a.step();
            b.step();
            assert_eq!(a.state_hash(), b.state_hash());
        }
    }
    fn traffic_sim() -> Simulation {
        let base = sim();
        let mut templates = (*base.templates).clone();
        templates.objects.get_mut("Tank").unwrap().fields.insert("GeometryMajorRadius".into(), vec!["10".into()]);
        Simulation::new(base.map, Arc::new(templates), 1)
    }
    /// A crushing tank and squishable infantry (CrusherLevel / SquishCollide).
    fn crush_sim() -> Simulation {
        let base = traffic_sim();
        let mut templates = (*base.templates).clone();
        templates.objects.get_mut("Tank").unwrap().fields.insert("CrusherLevel".into(), vec!["2".into()]);
        let mut soldier = templates.objects["Tank"].clone();
        soldier.name = "Soldier".into();
        soldier.fields.insert("GeometryMajorRadius".into(), vec!["3".into()]);
        soldier.fields.insert("CrusherLevel".into(), vec!["0".into()]);
        soldier.fields.insert("CrushableLevel".into(), vec!["0".into()]);
        soldier.fields.insert("KindOf".into(), vec!["INFANTRY".into()]);
        let mut squish = soldier.modules[0].clone();
        squish.class = "SquishCollide".into();
        squish.tag = "Squish".into();
        soldier.modules.push(squish);
        templates.objects.insert("Soldier".into(), soldier);
        Simulation::new(base.map, Arc::new(templates), 1)
    }
    #[test]
    fn tank_squishes_enemy_infantry_in_its_path() {
        let mut s = crush_sim();
        s.spawn("Tank", [25.0, 105.0], 1).unwrap();
        let soldier = s.spawn("Soldier", [95.0, 105.0], 2).unwrap();
        assert!(s.can_crush_or_squish(1, soldier, CrushTest::Squish));
        assert!(!s.can_crush_or_squish(soldier, 1, CrushTest::Either));
        s.enqueue(1, Command::Move { object: 1, target: [175.0, 105.0] }).unwrap();
        for _ in 0..600 {
            s.step();
        }
        assert!(!s.objects.contains_key(&soldier), "{:?}", s.objects);
        assert_eq!(s.crushed.len(), 1);
        assert_eq!((s.crushed[0].1, s.crushed[0].2), (soldier, 1));
        assert!((s.objects[&1].position[0] - 175.0).abs() < 0.5);
    }
    #[test]
    fn allied_infantry_steps_aside_instead_of_being_crushed() {
        let mut s = crush_sim();
        s.spawn("Tank", [25.0, 105.0], 1).unwrap();
        let soldier = s.spawn("Soldier", [95.0, 105.0], 1).unwrap();
        assert!(!s.can_crush_or_squish(1, soldier, CrushTest::Either));
        s.enqueue(1, Command::Move { object: 1, target: [175.0, 105.0] }).unwrap();
        for _ in 0..900 {
            s.step();
            assert!(s.overlaps().is_empty(), "frame {}", s.frame);
        }
        assert!(s.objects.contains_key(&soldier));
        assert!(s.crushed.is_empty());
        assert!(s.move_asides > 0);
        assert!((s.objects[&1].position[0] - 175.0).abs() < 0.5, "{:?}", s.objects[&1]);
        assert!(s.objects[&soldier].target.is_none() && s.objects[&soldier].ai.move_out.is_none());
    }
    #[test]
    fn segment_box_intersection() {
        assert!(segment_in_box([0.0, 5.0], [10.0, 5.0], [4.0, 4.0], [6.0, 6.0]));
        assert!(!segment_in_box([0.0, 0.0], [10.0, 0.0], [4.0, 4.0], [6.0, 6.0]));
        assert!(segment_in_box([5.0, 5.0], [5.0, 5.0], [4.0, 4.0], [6.0, 6.0]));
    }
    #[test]
    fn parked_obstacle_causes_repath_without_overlap() {
        let mut s = traffic_sim();
        s.spawn("Tank", [25.0, 105.0], 0).unwrap();
        s.spawn("Tank", [95.0, 105.0], 0).unwrap();
        s.enqueue(1, Command::Move { object: 1, target: [175.0, 105.0] }).unwrap();
        for _ in 0..600 {
            s.step();
            assert!(s.overlaps().is_empty(), "frame {}", s.frame);
        }
        assert!(s.events.is_empty(), "{:?}", s.events);
        assert!(s.objects[&1].target.is_none(), "{:?}", s.objects[&1]);
        assert!(s.objects[&1].traffic_repaths > 0);
        assert!((s.objects[&1].position[0] - 175.0).abs() < 0.5);
        assert_eq!(s.objects[&2].position[..2], [95.0, 105.0]);
    }
    #[test]
    fn stock_sized_box_units_escape_contact_when_start_is_off_cell_centre() {
        let base = traffic_sim();
        let mut templates = (*base.templates).clone();
        let fields = &mut templates.objects.get_mut("Tank").unwrap().fields;
        fields.insert("Geometry".into(), vec!["BOX".into()]);
        fields.insert("GeometryMajorRadius".into(), vec!["15".into()]);
        fields.insert("GeometryMinorRadius".into(), vec!["10".into()]);
        let mut s = Simulation::new(base.map, Arc::new(templates), 1);
        s.spawn("Tank", [25.0, 100.0], 0).unwrap();
        s.spawn("Tank", [95.0, 100.0], 0).unwrap();
        s.enqueue(1, Command::Move { object: 1, target: [175.0, 100.0] }).unwrap();
        for _ in 0..900 {
            s.step();
            assert!(s.overlaps().is_empty());
        }
        assert!(s.objects[&1].target.is_none(), "{:?}", s.objects[&1]);
        assert!(s.objects[&1].traffic_repaths > 0);
    }
    #[test]
    fn ground_unit_passes_under_an_occupied_deck() {
        let base = traffic_sim();
        let mut templates = (*base.templates).clone();
        templates.bridges.push(crate::bridges::Bridge {
            name: "B".into(),
            from: [60.0, 100.0, 20.0],
            to: [140.0, 100.0, 20.0],
            min_y: -30.0,
            max_y: 30.0,
        });
        let mut s = Simulation::new(base.map, Arc::new(templates), 1);
        s.spawn("Tank", [100.0, 25.0], 0).unwrap();
        s.spawn("Tank", [100.0, 100.0], 0).unwrap();
        assert_eq!(s.objects[&2].layer, 1);
        s.enqueue(1, Command::Move { object: 1, target: [100.0, 175.0] }).unwrap();
        for _ in 0..600 {
            s.step();
            assert!(s.overlaps().is_empty());
        }
        assert!(s.objects[&1].target.is_none());
        assert_eq!(s.objects[&1].layer, 0);
        assert_eq!(s.objects[&1].traffic_repaths, 0);
        assert!((s.objects[&1].position[1] - 175.0).abs() < 0.5);
    }
    #[test]
    fn head_on_units_yield_and_replay_without_overlap() {
        let mut a = traffic_sim();
        let mut b = traffic_sim();
        for s in [&mut a, &mut b] {
            s.spawn("Tank", [25.0, 105.0], 0).unwrap();
            s.spawn("Tank", [175.0, 105.0], 0).unwrap();
            s.enqueue(1, Command::Move { object: 1, target: [175.0, 105.0] }).unwrap();
            s.enqueue(1, Command::Move { object: 2, target: [25.0, 105.0] }).unwrap();
        }
        for _ in 0..900 {
            a.step();
            b.step();
            assert_eq!(a.state_hash(), b.state_hash());
            assert!(a.overlaps().is_empty(), "frame {}", a.frame);
        }
        assert!(a.events.is_empty());
        assert!(a.objects.values().all(|o| o.target.is_none()), "{:?}", a.objects);
        // The deadlocked lower-priority tank gets out of the way (aiMoveAwayFromUnit).
        assert!(a.move_asides > 0, "{:?}", a.objects);
        assert_eq!(a.state_hash(), 0x2b95715fc4e5b451, "portable traffic fixture changed");
    }
    #[test]
    fn faster_follower_keeps_spacing_and_destroy_releases_blocker() {
        let mut s = traffic_sim();
        let mut templates = (*s.templates).clone();
        let mut slow = templates.objects["Tank"].clone();
        slow.name = "Slow".into();
        slow.fields.insert("Locomotor".into(), vec!["SET_NORMAL".into(), "SlowL".into()]);
        templates.objects.insert("Slow".into(), slow);
        let mut loco = templates.locomotors["L"].clone();
        loco.name = "SlowL".into();
        loco.speed = 0.5;
        templates.locomotors.insert("SlowL".into(), loco);
        s = Simulation::new(s.map, Arc::new(templates), 1);
        s.spawn("Tank", [25.0, 105.0], 0).unwrap();
        s.spawn("Slow", [65.0, 105.0], 0).unwrap();
        s.enqueue(1, Command::Move { object: 1, target: [145.0, 105.0] }).unwrap();
        s.enqueue(1, Command::Move { object: 2, target: [175.0, 105.0] }).unwrap();
        for _ in 0..480 {
            s.step();
            assert!(s.overlaps().is_empty());
        }
        assert!(s.objects.values().all(|o| o.target.is_none()));
        // A later order to the occupied goal must wait, then resume after removal.
        s.enqueue(s.frame, Command::Move { object: 1, target: [175.0, 105.0] }).unwrap();
        s.enqueue(s.frame + 90, Command::Destroy { object: 2 }).unwrap();
        for _ in 0..240 {
            s.step();
            assert!(s.overlaps().is_empty());
        }
        assert!(s.objects[&1].target.is_none());
        assert!((s.objects[&1].position[0] - 175.0).abs() < 0.5);
    }
    #[test]
    fn recorded_unit_crosses_bridge_and_can_stop_on_deck() {
        use crate::bridges::Bridge;
        use ih_formats::map::PolygonTrigger;
        let base = sim();
        let mut map = (*base.map).clone();
        map.triggers.push(PolygonTrigger {
            name: "water".into(),
            layer: String::new(),
            id: 1,
            is_water: true,
            is_river: false,
            river_start: 0,
            points: vec![[80, 0, 10], [120, 0, 10], [120, 200, 10], [80, 200, 10]],
        });
        let mut templates = (*base.templates).clone();
        templates.bridges.push(Bridge {
            name: "B".into(),
            from: [65.0, 100.0, 20.0],
            to: [135.0, 100.0, 20.0],
            min_y: -30.0,
            max_y: 30.0,
        });
        let (map, templates) = (Arc::new(map), Arc::new(templates));
        let mut a = Simulation::new(map.clone(), templates.clone(), 1);
        let mut b = Simulation::new(map, templates, 1);
        for s in [&mut a, &mut b] {
            s.spawn("Tank", [25.0, 105.0], 0).unwrap();
            s.enqueue(1, Command::Move { object: 1, target: [175.0, 105.0] }).unwrap();
            s.enqueue(240, Command::Move { object: 1, target: [105.0, 105.0] }).unwrap();
        }
        let mut crossed = false;
        for _ in 0..480 {
            a.step();
            b.step();
            assert_eq!(a.state_hash(), b.state_hash());
            let o = &a.objects[&1];
            if o.layer == 1 {
                assert_eq!(o.position[2], 20.0);
                crossed = true;
            }
            if a.frame == 240 {
                assert!(o.target.is_none());
                assert!((o.position[0] - 175.0).abs() < 0.5);
                assert_eq!(o.layer, 0);
            }
        }
        assert!(crossed);
        assert!(a.events.is_empty(), "{:?}", a.events);
        assert!(a.objects[&1].target.is_none());
        assert_eq!(a.objects[&1].layer, 1);
        assert!((a.objects[&1].position[0] - 105.0).abs() < 0.5);
        assert_eq!(a.state_hash(), 0xc8d798e2102d6555, "portable bridge fixture changed");
    }
    #[test]
    fn clock_does_not_depend_on_render_frame_partition() {
        let mut a = LogicClock::default();
        let mut b = LogicClock::default();
        let x = a.advance(std::time::Duration::from_secs(1));
        let mut y = 0;
        for _ in 0..100 {
            y += b.advance(std::time::Duration::from_millis(10));
        }
        assert_eq!((x, y), (30, 30));
        assert_eq!(a.alpha(), b.alpha());
    }
}

/// `CrushSquishTestType`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum CrushTest {
    Crush,
    Squish,
    Either,
}

/// `LineInRegion`: whether segment a-b touches the box lo-hi (Liang-Barsky).
fn segment_in_box(a: [f32; 2], b: [f32; 2], lo: [f32; 2], hi: [f32; 2]) -> bool {
    let d = [b[0] - a[0], b[1] - a[1]];
    let (mut t0, mut t1) = (0.0f32, 1.0f32);
    for axis in 0..2 {
        for (p, q) in [(-d[axis], a[axis] - lo[axis]), (d[axis], hi[axis] - a[axis])] {
            if p == 0.0 {
                if q < 0.0 {
                    return false;
                }
            } else {
                let r = q / p;
                if p < 0.0 {
                    t0 = t0.max(r);
                } else {
                    t1 = t1.min(r);
                }
                if t0 > t1 {
                    return false;
                }
            }
        }
    }
    true
}

/// Heights under a body: its layer's surface and, over water, the water level
/// (`TerrainLogic::isUnderwater`, water polygon areas).
fn heights(grid: &Terrain, map: &Map, p: [f32; 2], layer: u32) -> crate::locomotion::Heights {
    let ground = grid.height_at(map, Waypoint { position: p, layer });
    // getWaterHandle: the highest water area over the point.
    let water = crate::navigation::water_height(map, p[0], p[1]);
    let surface = match water {
        Some(w) if w > ground => w,
        _ => ground,
    };
    crate::locomotion::Heights { ground, surface }
}
