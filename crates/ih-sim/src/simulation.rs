//! Ordered object/module updates and commands, following `GameLogic.cpp`,
//! `Object.cpp`, `AIUpdate.cpp`, `PhysicsUpdate.cpp`, `Locomotor.cpp`.
//! The ground steering/integration is an initial movement slice; specialised
//! locomotors, the full AI state machine and full collision forces remain pending.
use crate::{
    ai::{AiMachine, AiState, Outcome},
    disabled::{DisabledReason, DisabledStatus},
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
    pub stub_counts: BTreeMap<String, usize>,
    pub events: Vec<String>,
    pub failed_orders: u32,
    next_id: u32,
    sequence: u64,
    commands: BTreeMap<(u32, u64), Command>,
    setup_hash: u64,
    collision_radius_bound: f32,
}
impl Simulation {
    pub fn new(map: Arc<Map>, templates: Arc<Templates>, seed: u32) -> Self {
        let grid = Terrain::from_map(&map, &templates);
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
            grid,
            stub_counts: BTreeMap::new(),
            events: vec![],
            failed_orders: 0,
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
        let surface = self.grid.destination(&self.map, p);
        let z = self.grid.height_at(&self.map, surface);
        let position = [p[0], p[1], z];
        self.objects.insert(
            id,
            Object {
                id,
                template: name.into(),
                owner,
                position,
                previous_position: position,
                layer: surface.layer,
                angle: 0.0,
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
                    o.blocked_frames = 0;
                    o.blocked_by = None;
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
        if l.is_none_or(|l| !l.ground() || l.speed <= 0.0 || (face && (l.min_speed != 0.0 || l.turn_rate <= 0.0)))
            || !o.modules.iter().any(|m| m.kind == ModuleKind::Steering)
            || !o.modules.iter().any(|m| m.kind == ModuleKind::Physics)
        {
            return Err(SimError(format!(
                "{} has no implemented ground {}",
                o.template,
                if face { "turning in place" } else { "movement" }
            )));
        }
        Ok(())
    }
    fn route_to(&self, id: u32, target: [f32; 2]) -> Result<VecDeque<Waypoint>, SimError> {
        let o = &self.objects[&id];
        let t = &self.templates.objects[&o.template];
        self.grid
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
        o.target = Some(target);
        o.blocked_frames = 0;
        o.blocked_by = None;
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
        o.blocked_frames = 0;
        o.blocked_by = None;
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
        o.blocked_frames = 0;
        o.blocked_by = None;
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
                ModuleKind::Steering => self.update_ai(id),
                ModuleKind::Physics => {
                    self.integrate_ground(id);
                    1
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
        self.rebuild_partition();
        self.frame = self.frame.checked_add(1).expect("logic frame overflow");
    }
    fn update_ai(&mut self, id: u32) -> u32 {
        let o = &self.objects[&id];
        if o.disabled.any() {
            return if o.ai.active() { 1 } else { FOREVER };
        }
        match o.ai.state {
            AiState::Idle => return FOREVER,
            AiState::MoveTo | AiState::FollowPath => {
                self.repath_traffic(id);
                let o = self.objects.get_mut(&id).unwrap();
                let t = &self.templates.objects[&o.template];
                if let Some(l) = t.locomotor().and_then(|n| self.templates.locomotors.get(n)) {
                    steer(o, l, &self.grid, &self.map);
                }
                if o.target.is_none() {
                    let next = if o.ai.state == AiState::FollowPath { o.ai.remaining.pop_front() } else { None };
                    if let Some(target) = next {
                        match self.route_to(id, target) {
                            Ok(path) => {
                                let o = self.objects.get_mut(&id).unwrap();
                                o.path = path;
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
        if self.objects[&id].ai.active() { 1 } else { FOREVER }
    }
    fn rebuild_partition(&mut self) {
        self.partition.rebuild(self.objects.values().map(|o| (o.id, o.position)));
        self.collision_radius_bound = self.objects.values().map(|o| self.collider(o.id).radius).fold(0.0, f32::max);
    }
    fn collider(&self, id: u32) -> Collider {
        let o = &self.objects[&id];
        Collider::object(o, &self.templates.objects[&o.template], &self.templates)
    }
    fn nearby(&self, id: u32, distance: f32) -> Vec<Collider> {
        let body = self.collider(id);
        self.partition
            .candidates([body.position[0], body.position[1]], body.radius + self.collision_radius_bound + distance)
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
            return;
        }
        if o.target.is_none() {
            return;
        }
        let (from, velocity, layer) = (o.position, o.velocity, o.layer);
        let to = [from[0] + velocity[0], from[1] + velocity[1]];
        let radius = self.templates.objects[&o.template].radius();
        if !self.grid.line_on(
            Waypoint { position: [from[0], from[1]], layer },
            Waypoint { position: to, layer },
            radius,
        ) {
            let o = self.objects.get_mut(&id).unwrap();
            o.velocity = [0.0; 2];
            o.speed = 0.0;
            o.blocked_by = None;
            o.blocked_frames = o.blocked_frames.saturating_add(1);
            return;
        }
        let mut body = self.collider(id);
        let dz = self.grid.height_at(&self.map, Waypoint { position: to, layer }) - from[2];
        body.bottom += dz.min(0.0);
        body.top += dz.max(0.0);
        let mut fraction = 1.0;
        let mut blocker = None;
        for other in self.nearby(id, ih_math::length(velocity)) {
            let f = traffic::step_fraction(body, velocity, other);
            if f < fraction {
                fraction = f;
                blocker = Some(other.id);
            }
        }
        let to = [from[0] + velocity[0] * fraction, from[1] + velocity[1] * fraction];
        let position = [to[0], to[1], self.grid.height_at(&self.map, Waypoint { position: to, layer })];
        let o = self.objects.get_mut(&id).unwrap();
        o.position = position;
        o.blocked_by = blocker;
        o.blocked_frames = if blocker.is_some() { o.blocked_frames.saturating_add(1) } else { 0 };
        if blocker.is_some() {
            o.velocity = [velocity[0] * fraction, velocity[1] * fraction];
            o.speed *= fraction;
        }
        self.partition.relocate(id, from, position);
    }
    fn repath_traffic(&mut self, id: u32) {
        let o = &self.objects[&id];
        if o.blocked_frames < traffic::REPATH_DELAY || self.frame.saturating_sub(o.last_repath) < traffic::REPATH_DELAY
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
        let path =
            self.grid.path_avoiding(from, goal, self.templates.objects[&o.template].radius(), &blocked, &segment_clear);
        let o = self.objects.get_mut(&id).unwrap();
        o.last_repath = self.frame;
        if let Some(path) = path {
            o.path = path.into();
            o.traffic_repaths += 1;
            o.blocked_frames = 0;
            o.blocked_by = None;
        }
    }
    /// Live colliding object pairs, for headless invariants and diagnostics.
    pub fn overlaps(&self) -> Vec<(u32, u32)> {
        let mut pairs = Vec::new();
        for id in self.objects.keys().copied() {
            let a = self.collider(id);
            for b in self.nearby(id, 0.0) {
                if id < b.id && traffic::overlaps(a, b) {
                    pairs.push((id, b.id));
                }
            }
        }
        pairs
    }
    pub fn state_hash(&self) -> u64 {
        let mut h = StateHash::default();
        h.u32(4);
        h.u64(self.setup_hash);
        h.u32(self.frame);
        h.u32(self.next_id);
        h.u64(self.sequence);
        h.u32(self.failed_orders);
        for v in self.rng.state {
            h.u32(v);
        }
        h.u64(self.objects.len() as u64);
        for o in self.objects.values() {
            h.u32(o.id);
            h.text(&o.template);
            h.u32(o.owner);
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
fn steer(o: &mut Object, l: &Locomotor, grid: &Terrain, map: &Map) {
    let pos = [o.position[0], o.position[1]];
    while o.path.front().is_some_and(|p| {
        ih_math::length([p.position[0] - pos[0], p.position[1] - pos[1]]) < if o.path.len() == 1 { 0.5 } else { 0.001 }
    }) {
        let next = *o.path.front().unwrap();
        if next.layer != o.layer {
            if !grid.can_transition(Waypoint { position: pos, layer: o.layer }, next, 0.0) {
                break;
            }
            o.layer = next.layer;
            o.position[2] = grid.height_at(map, Waypoint { position: pos, layer: o.layer });
        }
        o.path.pop_front();
    }
    let Some(next) = o.path.front().copied() else {
        o.target = None;
        o.speed = 0.0;
        o.velocity = [0.0; 2];
        o.blocked_frames = 0;
        o.blocked_by = None;
        return;
    };
    let delta = [next.position[0] - pos[0], next.position[1] - pos[1]];
    let distance = ih_math::length(delta);
    let desired = ih_math::atan2(delta[1], delta[0]);
    let angle = ih_math::angle_delta(o.angle, desired);
    let turn = l.turn_rate.max(0.001);
    o.angle += angle.clamp(-turn, turn);
    let mut remaining = distance;
    let mut previous = next;
    for p in o.path.iter().skip(1) {
        remaining += ih_math::length([p.position[0] - previous.position[0], p.position[1] - previous.position[1]]);
        previous = *p;
    }
    let stop_margin = if o.path.len() == 1 { 0.25 } else { 0.0 };
    let desired_speed =
        l.speed.min(ih_math::sqrt((2.0 * l.braking.max(0.001) * (remaining - stop_margin).max(0.0)) / 1.05));
    if o.speed < desired_speed {
        o.speed = (o.speed + l.acceleration.max(0.001)).min(desired_speed);
    } else {
        o.speed = (o.speed - l.braking.max(0.001)).max(desired_speed);
    }
    let alignment = ih_math::cos(ih_math::angle_delta(o.angle, desired)).max(0.0);
    let amount = (o.speed * alignment).min(distance);
    // Follow the path corridor while the body turns toward its direction. The
    // original force/suspension model will replace this initial ground integrator.
    let unit = ih_math::normalized(delta);
    o.velocity = [unit[0] * amount, unit[1] * amount];
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
        assert_eq!(a.state_hash(), 0xf3653e098b76c23b, "portable synthetic movement fixture changed");
        b.rng.next_u32();
        assert_ne!(a.state_hash(), b.state_hash());
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
        assert!(a.objects.values().any(|o| o.traffic_repaths > 0));
        assert_eq!(a.state_hash(), 0x66512131ec5b2c98, "portable traffic fixture changed");
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
        assert_eq!(a.state_hash(), 0xeb2e076f238f6b44, "portable bridge fixture changed");
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
