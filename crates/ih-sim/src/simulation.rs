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
    /// A selection ordered to one point (`AIGroup::groupMoveToPosition`).
    GroupMove {
        objects: Vec<u32>,
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
    /// `aiAttackObject`.
    Attack {
        object: u32,
        target: u32,
    },
    /// `aiGuardPosition` (normal guard mode).
    Guard {
        object: u32,
        position: [f32; 2],
    },
    /// `aiAttackPosition` (force-fire at the ground).
    AttackPosition {
        object: u32,
        target: [f32; 2],
    },
    /// Damage of a named type (`Object::attemptDamage`), e.g. from a script.
    Damage {
        object: u32,
        amount: f32,
        #[serde(default = "default_damage_type")]
        damage_type: String,
        #[serde(default = "default_death_type")]
        death_type: String,
        #[serde(default)]
        source: u32,
    },
    Spawn {
        template: String,
        position: [f32; 2],
        #[serde(default)]
        owner: u32,
    },
    /// `Money::deposit` (starting money, scripts).
    Deposit {
        player: u32,
        amount: u32,
    },
    /// `Player::grantScience`.
    GrantScience {
        player: u32,
        science: String,
    },
    /// `ProductionUpdate::queueCreateUnit` (`MSG_QUEUE_UNIT_CREATE`).
    Produce {
        factory: u32,
        template: String,
    },
    /// `ProductionUpdate::cancelUnitCreate` (`MSG_CANCEL_UNIT_CREATE`).
    CancelProduction {
        factory: u32,
        production: u32,
    },
    /// `ExitInterface::setRallyPoint` (`MSG_SET_RALLY_POINT`).
    SetRallyPoint {
        object: u32,
        position: [f32; 2],
    },
    /// `aiDock` (`MSG_DOCK`): a supply truck to a warehouse or supply centre.
    Dock {
        object: u32,
        target: u32,
    },
    /// `BuildAssistant::buildObjectNow` (`MSG_DOZER_CONSTRUCT`): a dozer places a building.
    Construct {
        dozer: u32,
        template: String,
        position: [f32; 2],
        #[serde(default)]
        angle: f32,
    },
    /// `aiResumeConstruction` (`MSG_RESUME_CONSTRUCTION`).
    ResumeConstruction {
        dozer: u32,
        target: u32,
    },
    /// `aiRepair` (`MSG_DOZER_REPAIR`): a dozer repairs a building.
    Repair {
        dozer: u32,
        target: u32,
    },
    /// `BuildAssistant::sellObject` (`MSG_SELL`).
    Sell {
        object: u32,
    },
    /// `setVeterancyLevel` (scripts, cheats): 0 regular to 3 heroic.
    SetVeterancy {
        object: u32,
        level: u8,
    },
    /// `aiEnter` (`MSG_ENTER`): get into a transport.
    Enter {
        object: u32,
        target: u32,
    },
    /// `aiExit` (`MSG_EXIT`): a passenger gets out.
    Exit {
        object: u32,
    },
    /// `aiEvacuate` (`MSG_EVACUATE`): a transport lets everyone out.
    Evacuate {
        object: u32,
    },
    /// `ProductionUpdate::queueUpgrade` (`MSG_QUEUE_UPGRADE`).
    Research {
        factory: u32,
        upgrade: String,
    },
    /// `ProductionUpdate::cancelUpgrade` (`MSG_CANCEL_UPGRADE`).
    CancelResearch {
        factory: u32,
        upgrade: String,
    },
}
fn default_damage_type() -> String {
    "EXPLOSION".into()
}
fn default_death_type() -> String {
    "NORMAL".into()
}
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ModuleKind {
    Stub,
    /// `ProductionUpdate` (`production`).
    Production,
    /// A production exit module (`production::Exit`).
    ProductionExit,
    /// A supply dock (`supply::Dock`).
    Dock,
    /// A container (`contain::Contain`).
    Contain,
    /// `LifetimeUpdate` (`ocl`).
    Lifetime,
    /// `StealthUpdate` (`stealth`).
    Stealth,
    /// `StealthDetectorUpdate` (`stealth`).
    StealthDetector,
    Steering,
    Physics,
    /// The body module (`body`): not updated.
    Body,
    /// A die module (`body::DieKind`): runs when the object dies.
    Die,
    /// `SlowDeathBehavior`: a die module that updates while the object dies.
    SlowDeath,
    /// `DumbProjectileBehavior` (`projectiles`).
    DumbProjectile,
    /// `MissileAIUpdate` (`missiles`).
    Missile,
}
#[derive(Clone, Debug)]
pub struct Module {
    pub class: String,
    pub tag: String,
    pub kind: ModuleKind,
    pub updates: u32,
    pub die: Option<crate::body::DieKind>,
    /// `SlowDeathBehavior` state.
    pub death: crate::death::SlowDeathState,
}
#[derive(Clone, Debug)]
pub struct Object {
    pub id: u32,
    pub template: String,
    pub owner: u32,
    pub position: [f32; 3],
    pub previous_position: [f32; 3],
    /// The facing at the start of the frame (rotation guard in `integrate_ground`).
    pub previous_angle: f32,
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
    /// `m_pathfindGoalCell` and `m_pathfindCurCell` (`goals`).
    pub goal_cell: Option<(i32, i32)>,
    pub cur_cell: Option<(i32, i32)>,
    /// `getDestinationLayer`.
    pub destination_layer: u32,
    /// The body module's health (`body`); `None` without a body.
    pub health: Option<crate::body::Health>,
    /// `isEffectivelyDead`.
    pub dead: bool,
    /// `ObjectStatusMaskType` bits (`body::status`).
    pub status: u64,
    /// `m_weaponSet` (`weapons`).
    pub weapon_set: crate::weapons::WeaponSet,
    /// A launched dumb projectile's flight (`projectiles`).
    pub flight: Option<crate::projectiles::Flight>,
    /// A launched missile (`missiles`).
    pub missile: Option<crate::missiles::Missile>,
    /// `ProductionUpdate` state (`production`).
    pub production: Option<crate::production::Production>,
    /// The production exit module (`production`).
    pub exit: Option<crate::production::Exit>,
    /// A supply dock (`supply`).
    pub dock: Option<crate::supply::Dock>,
    /// `SupplyTruckAIUpdate` state (`supply`).
    pub truck: Option<crate::supply::Truck>,
    /// Upgrades and upgrade modules (`upgrades`).
    pub upgrades: crate::upgrades::ObjectUpgrades,
    /// `DozerAIUpdate` state (`construction`).
    pub dozer: Option<crate::construction::Dozer>,
    /// `getConstructionPercent` while under construction.
    pub construction: Option<f32>,
    /// `getBuilderID`.
    pub builder: Option<u32>,
    /// `m_soleHealingBenefactorID` and its expiry frame.
    pub sole_healer: Option<(u32, u32)>,
    /// A container module (`contain`).
    pub contain: Option<crate::contain::Contain>,
    /// `getContainedBy`.
    pub contained_by: Option<u32>,
    /// `getContainedByFrame`.
    pub contained_frame: u32,
    /// `getWeaponBonusCondition`.
    pub bonus_flags: u32,
    /// `ExperienceTracker` (`veterancy`).
    pub experience: crate::veterancy::Experience,
    /// `LifetimeUpdate::m_dieFrame`.
    pub die_frame: Option<u32>,
    /// `StealthUpdate`.
    pub stealth: Option<crate::stealth::Stealth>,
    /// `StealthDetectorUpdate`.
    pub detector: Option<crate::stealth::Detector>,
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
    /// Units' goal and position cells (`goals`).
    pub unit_cells: crate::goals::UnitCells,
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
    pub(crate) collision_radius_bound: f32,
    /// `m_objectsToDestroy`.
    pub(crate) destroy_list: Vec<u32>,
    /// The weapon store's delayed damage.
    pub(crate) delayed_damage: Vec<crate::weapons::DelayedDamage>,
    /// `BuildAssistant::m_sellList`: (object, frame sold), newest first.
    pub sell_list: Vec<(u32, u32)>,
    /// Players by index (`production::Player`), created on first use.
    pub players: BTreeMap<u32, crate::production::Player>,
}
impl Simulation {
    pub fn new(map: Arc<Map>, templates: Arc<Templates>, seed: u32) -> Self {
        let grid = Terrain::from_map(&map, &templates);
        // One view per distinct locomotor surface mask that sees the cells differently.
        let mut views = BTreeMap::new();
        for t in templates.objects.values() {
            let Some(l) = t.locomotor().and_then(|n| templates.locomotors.get(n)).filter(|l| l.surface_movement())
            else {
                continue;
            };
            let mask = view_key(t, l);
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
        h.u64(templates.armors.len() as u64);
        for (name, a) in &templates.armors {
            h.text(name);
            for c in a.coefficients {
                h.float(c);
            }
        }
        h.float(templates.damage.damaged);
        h.float(templates.damage.really_damaged);
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
            h.u64(t.armor_sets.len() as u64);
            for a in &t.armor_sets {
                h.u32(a.conditions);
                h.text(a.armor.as_deref().unwrap_or(""));
            }
            h.u64(t.modules.len() as u64);
            for m in &t.modules {
                h.text(&m.class);
                h.text(&m.tag);
            }
        }
        let g = templates.group;
        for v in
            [g.min_infantry as f32, g.min_vehicles as f32, g.min_distance, g.distance_requires_group, g.gather_factor]
        {
            h.float(v);
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
            unit_cells: Default::default(),
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
            destroy_list: Vec::new(),
            delayed_damage: Vec::new(),
            players: BTreeMap::new(),
            sell_list: Vec::new(),
        }
    }
    pub fn spawn(&mut self, name: &str, p: [f32; 2], owner: u32) -> Result<u32, SimError> {
        if !p.iter().all(|v| v.is_finite()) {
            return Err(SimError("non-finite spawn position".into()));
        }
        let surface = self.grid.destination(&self.map, p);
        let z = self.grid.height_at(&self.map, surface);
        let id = self.create(name, [p[0], p[1], z], 0.0, surface.layer, owner, None)?;
        // addObjectToPathfindMap for objects created in the game.
        self.add_to_pathfind_map(id);
        Ok(id)
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
        self.create_full(name, position, angle, layer, owner, map_index, 0)
    }
    /// `newObject` with starting status bits (for example `UNDER_CONSTRUCTION`).
    pub(crate) fn create_with_status(
        &mut self,
        name: &str,
        position: [f32; 3],
        angle: f32,
        layer: u32,
        owner: u32,
        status: u64,
    ) -> Result<u32, SimError> {
        self.create_full(name, position, angle, layer, owner, None, status)
    }
    #[allow(clippy::too_many_arguments)]
    fn create_full(
        &mut self,
        name: &str,
        position: [f32; 3],
        angle: f32,
        layer: u32,
        owner: u32,
        map_index: Option<usize>,
        status: u64,
    ) -> Result<u32, SimError> {
        let t =
            self.templates.objects.get(name).ok_or_else(|| SimError(format!("object template {name} is missing")))?;
        let id = self.next_id;
        self.next_id = self.next_id.checked_add(1).ok_or_else(|| SimError("object IDs exhausted".into()))?;
        let mut modules = vec![];
        let (mut stealth, mut detector) = (None, None);
        for m in t.modules.iter().filter(|m| matches!(m.kind.as_str(), "Behavior" | "Body")) {
            let known = ih_ini::schema::MODULES.iter().find(|d| d.name == m.class);
            let kind = if let Some(k) = crate::body::module_kind(m) {
                k
            } else if m.class == "DumbProjectileBehavior" {
                ModuleKind::DumbProjectile
            } else if m.class == "MissileAIUpdate" {
                ModuleKind::Missile
            } else if m.class == "PhysicsBehavior" {
                ModuleKind::Physics
            } else if m.class == "ProductionUpdate" {
                ModuleKind::Production
            } else if crate::supply::Dock::from_module(&m.class, &m.block).is_some() {
                ModuleKind::Dock
            } else if crate::contain::ContainData::from_module(&m.class, &m.block).is_some() {
                ModuleKind::Contain
            } else if m.class == "LifetimeUpdate" {
                ModuleKind::Lifetime
            } else if m.class == "StealthUpdate" {
                ModuleKind::Stealth
            } else if m.class == "StealthDetectorUpdate" {
                ModuleKind::StealthDetector
            } else if crate::production::Exit::from_module(&m.class, &m.block).is_some() {
                ModuleKind::ProductionExit
            } else if known.is_some_and(|d| d.tables.iter().any(|t| t.starts_with("AIUpdateModuleData::"))) {
                ModuleKind::Steering
            } else {
                ModuleKind::Stub
            };
            let index = modules.len();
            if kind == ModuleKind::Stub {
                *self.stub_counts.entry(m.class.clone()).or_default() += 1;
            } else if matches!(kind, ModuleKind::Steering | ModuleKind::Physics) {
                self.scheduler.insert(
                    (id, index),
                    self.frame.max(1),
                    if kind == ModuleKind::Steering { Phase::Initial } else { Phase::Physics },
                );
            } else if kind == ModuleKind::Missile {
                // An AI update: every frame, in the initial phase.
                self.scheduler.insert((id, index), self.frame.max(1), Phase::Initial);
            } else if kind == ModuleKind::DumbProjectile {
                // Updated every frame from creation.
                self.scheduler.insert((id, index), self.frame.max(1), Phase::Normal);
            } else if kind == ModuleKind::Production
                || kind == ModuleKind::Dock
                || kind == ModuleKind::Contain
                || kind == ModuleKind::ProductionExit && m.class == "QueueProductionExitUpdate"
            {
                // Updated every frame (UPDATE_SLEEP_NONE); the other exits sleep forever.
                self.scheduler.insert((id, index), self.frame.max(1), Phase::Normal);
            } else if kind == ModuleKind::Lifetime {
                // Scheduled once the object exists (LifetimeUpdate's constructor).
                self.scheduler.insert((id, index), FOREVER, Phase::Normal);
            } else if kind == ModuleKind::SlowDeath {
                // Asleep until the object dies.
                self.scheduler.insert((id, index), FOREVER, Phase::Normal);
            } else if kind == ModuleKind::Stealth && stealth.is_none() {
                // Awake from the start (unless granted by a special power, see stealth_created).
                let data = crate::stealth::StealthData::from_module(&m.block);
                stealth = Some(crate::stealth::Stealth {
                    allowed_frame: self.frame.saturating_add(data.delay),
                    enabled: !data.team_disguised,
                    data,
                    module: index,
                    detection_expires: 0,
                    next_black_market_check: 0,
                    frames_granted: 0,
                });
                self.scheduler.insert((id, index), self.frame.max(1), Phase::Normal);
            } else if kind == ModuleKind::StealthDetector && detector.is_none() {
                // The constructor's random first wake-up.
                let d = crate::stealth::Detector::from_module(&m.block);
                let wake = if d.enabled {
                    self.frame.max(1) + self.rng.integer(1, d.rate.max(1) as i32) as u32
                } else {
                    FOREVER
                };
                self.scheduler.insert((id, index), wake, Phase::Normal);
                detector = Some(d);
            }
            let die = crate::body::DieKind::from_module(&m.class, &m.block, &self.templates);
            modules.push(Module {
                class: m.class.clone(),
                tag: m.tag.clone(),
                kind,
                updates: 0,
                die,
                death: Default::default(),
            });
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
        let production = t
            .modules
            .iter()
            .find(|m| m.class == "ProductionUpdate" && matches!(m.kind.as_str(), "Behavior"))
            .map(|m| crate::production::Production::new(crate::production::ProductionData::parse(&m.block)));
        let exit = t
            .modules
            .iter()
            .filter(|m| m.kind == "Behavior")
            .find_map(|m| crate::production::Exit::from_module(&m.class, &m.block));
        let dock = t
            .modules
            .iter()
            .filter(|m| m.kind == "Behavior")
            .find_map(|m| crate::supply::Dock::from_module(&m.class, &m.block));
        let truck = t
            .modules
            .iter()
            .filter(|m| m.kind == "Behavior")
            .find_map(|m| crate::supply::Truck::from_module(&m.class, &m.block));
        let dozer = t
            .modules
            .iter()
            .filter(|m| m.kind == "Behavior")
            .find_map(|m| crate::construction::Dozer::from_module(&m.class, &m.block));
        let contain = t
            .modules
            .iter()
            .filter(|m| m.kind == "Behavior")
            .find_map(|m| crate::contain::ContainData::from_module(&m.class, &m.block))
            .map(crate::contain::Contain::new);
        let upgrade_modules: Vec<crate::upgrades::UpgradeModuleDef> = t
            .modules
            .iter()
            .filter(|m| m.kind == "Behavior")
            .filter_map(|m| crate::upgrades::UpgradeModuleDef::from_module(&m.class, &m.block))
            .collect();
        let upgrades = crate::upgrades::ObjectUpgrades {
            executed: vec![false; upgrade_modules.len()],
            modules: upgrade_modules,
            ..Default::default()
        };
        let body = traffic::BodyShape::new(t, &modules, &self.templates);
        let thresholds = (self.templates.damage.damaged, self.templates.damage.really_damaged);
        let health = crate::body::body_of(t).map(|(k, m)| crate::body::Health::new(k, &m.block, thresholds));
        self.objects.insert(
            id,
            Object {
                id,
                template: name.into(),
                owner,
                position,
                previous_position: position,
                previous_angle: angle,
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
                goal_cell: None,
                cur_cell: None,
                destination_layer: 0,
                health,
                dead: false,
                status,
                weapon_set: Default::default(),
                flight: None,
                missile: None,
                production,
                exit,
                dock,
                truck,
                upgrades,
                dozer,
                construction: None,
                builder: None,
                sole_healer: None,
                contain,
                contained_by: None,
                contained_frame: 0,
                bonus_flags: 0,
                experience: Default::default(),
                die_frame: None,
                stealth,
                detector,
            },
        );
        self.stealth_created(id);
        // The AI state machine starts idle (AIIdleState::onEnter).
        self.idle_enter(id);
        // Object::initObject: upgrade modules run with the player's upgrades.
        self.update_upgrade_modules(id);
        // TunnelContain::onObjectCreated
        self.register_tunnel(id);
        // LifetimeUpdate's constructor: a random lifetime.
        let templates = self.templates.clone();
        if let Some(m) = templates.objects[name].modules.iter().find(|m| m.class == "LifetimeUpdate") {
            let (min, max) = crate::ocl::lifetime_range(&m.block);
            self.set_lifetime(id, min, max);
        }
        self.update_weapon_set(id);
        self.rebuild_partition();
        self.update_pos(id);
        Ok(id)
    }
    pub fn enqueue(&mut self, frame: u32, command: Command) -> Result<(), SimError> {
        if frame < self.frame {
            return Err(SimError(format!("command frame {frame} is before current frame {}", self.frame)));
        }
        let finite = match &command {
            Command::Move { target, .. }
            | Command::Face { target, .. }
            | Command::GroupMove { target, .. }
            | Command::AttackPosition { target, .. }
            | Command::Guard { position: target, .. } => target.iter().all(|f| f.is_finite()),
            Command::FollowPath { points, .. } => points.iter().flatten().all(|f| f.is_finite()),
            Command::Spawn { position, .. }
            | Command::SetRallyPoint { position, .. }
            | Command::Construct { position, .. } => position.iter().all(|f| f.is_finite()),
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
        // Orders from the player (CMD_FROM_PLAYER).
        let ordered: Vec<u32> = match &c {
            Command::Move { object, .. }
            | Command::Stop { object }
            | Command::FollowPath { object, .. }
            | Command::Face { object, .. }
            | Command::FaceObject { object, .. }
            | Command::Attack { object, .. }
            | Command::AttackPosition { object, .. }
            | Command::Guard { object, .. }
            | Command::Dock { object, .. }
            | Command::Enter { object, .. }
            | Command::Exit { object } => vec![*object],
            Command::GroupMove { objects, .. } => objects.clone(),
            _ => vec![],
        };
        for id in &ordered {
            self.dozer_player_order(*id);
            if let Some(o) = self.objects.get_mut(id) {
                o.ai.cmd_from_ai = false;
            }
        }
        let r = self.apply_command(c);
        for id in ordered {
            self.dozer_after_player_order(id);
        }
        r
    }
    fn apply_command(&mut self, c: Command) -> Result<(), SimError> {
        match c {
            Command::Spawn { template, position, owner } => {
                self.spawn(&template, position, owner)?;
            }
            Command::Deposit { player, amount } => self.player(player).deposit(amount),
            Command::GrantScience { player, science } => {
                self.player(player).sciences.insert(science);
            }
            Command::Produce { factory, template } => {
                self.queue_create_unit(factory, &template)?;
            }
            Command::CancelProduction { factory, production } => self.cancel_unit_create(factory, production)?,
            Command::SetRallyPoint { object, position } => self.set_rally_point(object, position)?,
            Command::Research { factory, upgrade } => self.queue_upgrade(factory, &upgrade)?,
            Command::Construct { dozer, template, position, angle } => {
                self.build_object_now(dozer, &template, position, angle)?;
            }
            Command::ResumeConstruction { dozer, target } => self.resume_construction(dozer, target)?,
            Command::Repair { dozer, target } => self.repair(dozer, target, true)?,
            Command::Enter { object, target } => self.begin_enter(object, target, false)?,
            Command::SetVeterancy { object, level } => self.set_veterancy(object, level)?,
            Command::Exit { object } => self.begin_exit(object, false)?,
            Command::Evacuate { object } => self.evacuate(object, false)?,
            Command::Sell { object } => self.sell(object)?,
            Command::CancelResearch { factory, upgrade } => self.cancel_upgrade(factory, &upgrade)?,
            Command::Dock { object, target } => {
                if !self.objects.contains_key(&object) || !self.objects.contains_key(&target) {
                    return Err(SimError(format!("object {object} or {target} is missing")));
                }
                self.begin_dock(object, target, false);
            }
            Command::Damage { object, amount, damage_type, death_type, source } => {
                if !self.objects.contains_key(&object) {
                    return Err(SimError(format!("object {object} is missing")));
                }
                let t = crate::body::damage_type(&damage_type)
                    .ok_or_else(|| SimError(format!("unknown damage type {damage_type}")))?;
                let d = crate::body::death_type(&death_type)
                    .ok_or_else(|| SimError(format!("unknown death type {death_type}")))?;
                self.attempt_damage(object, crate::body::DamageInfo::new(source, t, d, amount));
            }
            Command::Destroy { object } => {
                self.remove(object)?;
            }
            Command::Guard { object, position } => {
                self.begin_guard(object, position)?;
            }
            Command::Attack { object, target } => {
                let pos = self.objects.get(&target).map_or([0.0; 3], |o| o.position);
                self.begin_attack(object, Some(target), pos)?;
            }
            Command::AttackPosition { object, target } => {
                let z = self.grid.height_at(&self.map, Waypoint::ground(target));
                self.begin_attack(object, None, [target[0], target[1], z])?;
            }
            Command::Stop { object } => {
                if !self.objects.contains_key(&object) {
                    return Err(SimError(format!("object {object} is missing")));
                }
                // SupplyTruckAIUpdate::privateIdle from the player (not workers).
                let o = self.objects.get_mut(&object).unwrap();
                if o.dozer.is_none()
                    && let Some(t) = o.truck.as_mut()
                {
                    t.force_busy = true;
                }
                self.end_order(object, Outcome::Interrupted);
            }
            Command::Move { object, target } => {
                self.begin_move(object, target, AiState::MoveTo, VecDeque::new())?;
            }
            Command::GroupMove { objects, target } => {
                self.group_move(&objects, target)?;
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
    pub(crate) fn validate_mobile_quiet(&self, id: u32) -> Result<(), SimError> {
        self.validate_mobile(id, false)
    }
    fn validate_mobile(&self, id: u32, face: bool) -> Result<(), SimError> {
        let o = self.objects.get(&id).ok_or_else(|| SimError(format!("object {id} is missing")))?;
        let t = &self.templates.objects[&o.template];
        if t.has_kind("IMMOBILE") || o.disabled.any() || o.dead {
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
    pub(crate) fn terrain(&self, id: u32) -> &Terrain {
        let t = &self.templates.objects[&self.objects[&id].template];
        t.locomotor()
            .and_then(|n| self.templates.locomotors.get(n))
            .and_then(|l| self.views.get(&view_key(t, l)))
            .unwrap_or(&self.grid)
    }
    pub(crate) fn route_to(&self, id: u32, target: [f32; 2]) -> Result<VecDeque<Waypoint>, SimError> {
        // A held goal keeps the layer it was chosen on (`getLayerForDestination` picks
        // by height, and an adjusted goal sits at its layer's height).
        let o = &self.objects[&id];
        let goal = if o.goal_cell.is_some() && self.ground_movement(id) {
            Waypoint { position: target, layer: o.destination_layer }
        } else {
            self.grid.destination(&self.map, target)
        };
        self.route_to_waypoint(id, goal, true).map(|(path, _)| path)
    }
    /// What the pathfinder reads from an object.
    pub(crate) fn mover(&self, id: u32) -> crate::pathfind::Mover {
        let o = &self.objects[&id];
        let t = &self.templates.objects[&o.template];
        let l = t.locomotor().and_then(|n| self.templates.locomotors.get(n));
        let (radius, center) = crate::goals::radius_and_center(t.collision_radius());
        crate::pathfind::Mover {
            id,
            owner: o.owner,
            layer: o.layer,
            radius,
            center,
            crusher: t.number("CrusherLevel", 0.0) > 0.0,
            surfaces: l.map_or(0, |l| view_key(t, l)),
            infantry: t.has_kind("INFANTRY"),
            through_units: false,
            downhill: l.is_some_and(|l| l.downhill_only),
            aircraft_goals: !self.ground_movement(id)
                && l.is_some_and(|l| {
                    matches!(l.appearance, crate::templates::Appearance::Hover | crate::templates::Appearance::Wings)
                }),
            ignore_obstacle: o
                .ai
                .ignore_obstacle
                .filter(|other| self.objects.contains_key(other))
                .map_or(0, |other| self.obstacle_key(other)),
        }
    }
    /// The pathfinder for an object, over its terrain view.
    pub(crate) fn pathfinder(&self, id: u32) -> crate::pathfind::Pathfinder<'_> {
        crate::pathfind::Pathfinder {
            terrain: self.terrain(id),
            map: &self.map,
            units: &self.unit_cells,
            objects: &self.objects,
            templates: &self.templates,
            mover: self.mover(id),
        }
    }
    /// `AIUpdateInterface::computePath`: flyers and clear lines to an intermediate
    /// point take the straight line (`computeQuickPath`); otherwise `findPath`,
    /// falling back to `findClosestPath`. Returns the path (without its start) and
    /// the goal it ends at.
    pub(crate) fn route_to_waypoint(
        &self,
        id: u32,
        goal: Waypoint,
        final_goal: bool,
    ) -> Result<(VecDeque<Waypoint>, [f32; 2]), SimError> {
        let o = &self.objects[&id];
        let target = goal.position;
        if self.flyer(id) {
            // canComputeQuickPath / computeQuickPath: flyers go straight to the goal.
            return Ok((VecDeque::from([Waypoint { position: target, layer: 0 }]), target));
        }
        let pf = self.pathfinder(id);
        let from = [o.position[0], o.position[1]];
        if !final_goal && pf.is_line_passable(o.layer, from, target, false, true) {
            return Ok((VecDeque::from([goal]), target));
        }
        let unreachable = || SimError(format!("object {id}: target is blocked or unreachable"));
        // The original clips a destination beyond the pathfind extent to its edge;
        // until scripts send units off the map, ironhour rejects it.
        let size = [self.grid.width as f32 * CELL, self.grid.height as f32 * CELL];
        if !(0.0..size[0]).contains(&target[0]) || !(0.0..size[1]).contains(&target[1]) {
            return Err(unreachable());
        }
        if !pf.valid_position(goal.layer, target) {
            return Err(SimError(format!("object {id}: target {target:?} is blocked")));
        }
        if let Some(path) = pf.find_path(from, target, goal.layer) {
            return Ok((path.optimized().into(), path.nodes.last().map_or(target, |w| w.position)));
        }
        pf.find_closest_path(from, target, goal.layer, false, 0.0)
            .map(|(path, to)| (path.optimized().into(), path.nodes.last().map_or(to, |w| w.position)))
            .ok_or_else(|| SimError(format!("object {id}: no path from {from:?} to {target:?}")))
    }
    pub(crate) fn begin_move(
        &mut self,
        id: u32,
        target: [f32; 2],
        state: AiState,
        remaining: VecDeque<[f32; 2]>,
    ) -> Result<(), SimError> {
        self.begin_move_ignoring(id, target, state, remaining, None)
    }
    /// `begin_move` with `ignoreObstacle` set before the path is computed.
    pub(crate) fn begin_move_ignoring(
        &mut self,
        id: u32,
        target: [f32; 2],
        state: AiState,
        remaining: VecDeque<[f32; 2]>,
        ignore: Option<u32>,
    ) -> Result<(), SimError> {
        self.validate_mobile(id, false)?;
        let previous = self.objects[&id].ai.ignore_obstacle;
        self.objects.get_mut(&id).unwrap().ai.ignore_obstacle = ignore;
        let r = self.begin_move_inner(id, target, state, remaining);
        let o = self.objects.get_mut(&id).unwrap();
        o.ai.ignore_obstacle = if r.is_ok() { ignore } else { previous };
        r
    }
    fn begin_move_inner(
        &mut self,
        id: u32,
        target: [f32; 2],
        state: AiState,
        remaining: VecDeque<[f32; 2]>,
    ) -> Result<(), SimError> {
        let last = remaining.is_empty();
        let goal = self.choose_goal(id, target, state, last);
        let (path, target) = self.route_to_waypoint(id, goal, state == AiState::MoveTo || last)?;
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
        self.move_allies(id);
        Ok(())
    }
    /// `AIInternalMoveToState::onEnter`: a move adjusts its goal to a free cell
    /// (`adjustDestination`, else `snapClosestGoalPosition`) and claims it
    /// (`updateGoal`). Follow-path legs do not adjust; the final leg claims its
    /// point, earlier legs release the goal.
    pub(crate) fn choose_goal(&mut self, id: u32, target: [f32; 2], state: AiState, last: bool) -> Waypoint {
        let layer = self.grid.destination(&self.map, target).layer;
        let mut goal = target;
        if state == AiState::MoveTo {
            if !self.adjust_destination(id, &mut goal, layer) {
                self.snap_closest_goal(id, &mut goal, layer);
            }
            self.update_goal(id, goal, layer);
        } else if last {
            // The final leg adjusts its goal (`m_adjustFinal`) for ground movers.
            if self.ground_movement(id) && !self.adjust_destination(id, &mut goal, layer) {
                self.snap_closest_goal(id, &mut goal, layer);
            }
            self.update_goal(id, goal, layer);
        } else {
            self.remove_goal(id);
        }
        Waypoint { position: goal, layer }
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
    pub(crate) fn wake_ai(&mut self, id: u32) {
        for (i, m) in self.objects[&id].modules.iter().enumerate() {
            if m.kind == ModuleKind::Steering {
                self.scheduler.awaken((id, i), self.frame, 1);
            }
        }
    }
    pub(crate) fn end_order(&mut self, id: u32, outcome: Outcome) {
        self.end_order_inner(id, outcome);
        // AIIdleState::onEnter.
        self.idle_enter(id);
        self.wake_ai(id);
    }
    fn end_order_inner(&mut self, id: u32, outcome: Outcome) {
        // AIIdleState::onEnter: the goal becomes where the unit stands.
        let (p, layer) = (self.objects[&id].position, self.objects[&id].layer);
        self.update_goal(id, [p[0], p[1]], layer);
        let o = self.objects.get_mut(&id).unwrap();
        if outcome == Outcome::Failure {
            self.failed_orders += 1;
        }
        o.ai.finish(outcome, self.frame);
        // Supply trucks keep their last command source (`ownerPlayerCommanded`
        // reads it after an AI-given move or dock ends).
        if o.truck.is_none() {
            o.ai.cmd_from_ai = false;
        }
        o.target = None;
        o.path.clear();
        o.speed = 0.0;
        o.velocity = [0.0; 2];
        o.physics.vel = [0.0, 0.0, o.physics.vel[2]];
        o.blocked_frames = 0;
        o.blocked_by = None;
        o.traffic.stuck = false;
    }
    fn halt_all_docks(&mut self) {
        let ids: Vec<u32> = self.objects.values().filter(|o| o.ai.halted_dock.is_some()).map(|o| o.id).collect();
        for id in ids {
            self.halt_docks(id);
        }
    }
    /// A new object (projectiles and other created objects).
    pub(crate) fn create_object(
        &mut self,
        name: &str,
        position: [f32; 3],
        angle: f32,
        layer: u32,
        owner: u32,
    ) -> Result<u32, SimError> {
        self.create(name, position, angle, layer, owner, None)
    }
    pub fn remove(&mut self, id: u32) -> Result<(), SimError> {
        if self.objects.contains_key(&id) {
            self.contain_on_delete(id);
            self.forget_cells(id);
            self.upgrades_on_delete(id);
            self.remove_from_pathfind_map(id);
        }
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
            o.previous_angle = o.angle;
        }
        while self.commands.first_key_value().is_some_and(|((f, _), _)| *f <= self.frame) {
            let ((frame, seq), command) = self.commands.pop_first().unwrap();
            if let Err(e) = self.command(command) {
                self.events.push(format!("frame {frame} command {seq}: {e}"));
            }
        }
        self.halt_all_docks();
        self.wake_forced();
        while let Some((id, index)) = self.scheduler.due(self.frame) {
            let kind = self.objects[&id].modules[index].kind;
            let exceptions = match kind {
                ModuleKind::Steering => 1 << (DisabledReason::Held as u32),
                ModuleKind::Physics | ModuleKind::SlowDeath | ModuleKind::DumbProjectile | ModuleKind::Missile => {
                    u32::MAX
                }
                // DisabledTypesToProcess defaults to HELD.
                ModuleKind::Production | ModuleKind::Stealth | ModuleKind::StealthDetector => {
                    1 << (DisabledReason::Held as u32)
                }
                ModuleKind::Stub
                | ModuleKind::Body
                | ModuleKind::Die
                | ModuleKind::ProductionExit
                | ModuleKind::Dock
                | ModuleKind::Contain
                | ModuleKind::Lifetime => 0,
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
                ModuleKind::Physics if self.objects[&id].missile.is_some() => {
                    self.missile_physics(id);
                    1
                }
                ModuleKind::Physics => {
                    self.integrate_ground(id);
                    self.physics_sleep(id)
                }
                ModuleKind::SlowDeath => self.slow_death_update(id, index),
                ModuleKind::DumbProjectile => self.dumb_projectile_update(id, index),
                ModuleKind::Missile => self.missile_update(id),
                ModuleKind::Production => self.production_update(id),
                ModuleKind::ProductionExit => self.exit_update(id),
                ModuleKind::Dock => self.dock_update(id),
                ModuleKind::Contain => self.contain_update(id),
                ModuleKind::Lifetime => self.lifetime_update(id),
                ModuleKind::Stealth => self.stealth_update(id),
                ModuleKind::StealthDetector => self.detector_update(id),
                ModuleKind::Stub | ModuleKind::Body | ModuleKind::Die => FOREVER,
            };
            self.update_pos(id);
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
        // TheBuildAssistant->UPDATE(), before the partition's update.
        self.sell_update();
        self.process_collisions();
        self.projectile_collisions();
        self.wake_forced();
        self.rebuild_partition();
        self.process_destroy_list();
        self.update_delayed_damage();
        self.halt_all_docks();
        self.frame = self.frame.checked_add(1).expect("logic frame overflow");
    }
    /// `PhysicsBehavior::calcSleepTime` (`SLEEPY_PHYSICS`), after the update ran.
    /// Overlap tracking (`getCurrentOverlap`) is not kept yet and counts as none.
    fn physics_sleep(&mut self, id: u32) -> u32 {
        let frame = self.frame;
        let o = self.objects.get_mut(&id).unwrap();
        // The overlap seen this update becomes the previous one (cleared for PhysicsCollide).
        o.physics.previous_overlap = o.physics.current_overlap;
        o.physics.current_overlap = 0;
        o.physics.ever_run = true;
        o.physics.woken = false;
        let p = &o.physics;
        let ground = if o.layer == 0 { self.map.ground_height(o.position[0], o.position[1]) } else { f32::MAX };
        let resting = p.vel == [0.0; 3]
            && p.accel == [0.0; 3]
            && p.motive_expires <= frame
            && o.layer == 0
            && o.position[2] - ground <= 0.0
            && o.slide == [0.0; 2]
            && p.current_overlap == 0
            && p.previous_overlap == 0;
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
        // Workers: the AI first, then the worker machine and one of its brains.
        if self.objects[&id].dozer.as_ref().is_some_and(|d| d.worker.is_some()) {
            self.update_ai_inner(id);
            self.halt_docks(id);
            self.worker_update(id);
            self.halt_docks(id);
            return 1;
        }
        if self.objects[&id].truck.is_some() && !self.objects[&id].ai.dead {
            self.truck_update(id);
            self.halt_docks(id);
            let delay = self.update_ai_inner(id);
            self.halt_docks(id);
            // The truck machine never sleeps.
            return delay.min(1);
        }
        let delay = self.update_ai_inner(id);
        self.halt_docks(id);
        if self.objects.get(&id).is_some_and(|o| o.dozer.is_some()) {
            self.dozer_update(id);
            self.halt_docks(id);
            return 1;
        }
        delay
    }
    fn update_ai_inner(&mut self, id: u32) -> u32 {
        let o = &self.objects[&id];
        if o.ai.dead {
            return self.update_dead(id);
        }
        // A passenger is held, but its AI still runs (it cannot move).
        if o.ai.state == AiState::Exit && o.disabled.names() == [DisabledReason::Held] {
            return self.update_exit(id);
        }
        let passenger = o.contained_by.is_some() && o.disabled.names() == [DisabledReason::Held];
        if o.disabled.any() && !passenger {
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
            AiState::Idle => {
                let loco = self.idle_locomotor(id);
                let idle = self.idle_update(id);
                if !self.objects[&id].ai.active() {
                    return loco.min(idle);
                }
                return 1;
            }
            AiState::Guard => return self.update_guard(id),
            AiState::Dock => return self.update_dock(id),
            AiState::Enter => return self.update_enter(id),
            AiState::Exit => return self.update_exit(id),
            AiState::Dead => return self.update_dead(id),
            AiState::AttackObject | AiState::AttackPosition => return self.update_attack(id),
            AiState::MoveTo | AiState::FollowPath => {
                self.repath_traffic(id);
                self.drive(id);
                let o = self.objects.get_mut(&id).unwrap();
                if o.target.is_none() {
                    let mut next = if o.ai.state == AiState::FollowPath { o.ai.remaining.pop_front() } else { None };
                    // Points within a cell of the unit are skipped ("tooClose").
                    while let Some(p) = next
                        && (p[0] - o.position[0]).powi(2) + (p[1] - o.position[1]).powi(2) < CELL * CELL
                    {
                        next = o.ai.remaining.pop_front();
                    }
                    if let Some(target) = next {
                        let last = self.objects[&id].ai.remaining.is_empty();
                        let goal = self.choose_goal(id, target, AiState::FollowPath, last);
                        match self.route_to_waypoint(id, goal, last) {
                            Ok((path, target)) => {
                                let o = self.objects.get_mut(&id).unwrap();
                                o.path = path;
                                // The first node is the start of the current segment (`Path` keeps its start node).
                                o.path
                                    .push_front(Waypoint { position: [o.position[0], o.position[1]], layer: o.layer });
                                o.target = Some(target);
                                o.last_repath = self.frame;
                            }
                            // AIFollowPathState: an unreachable point counts as a failed
                            // leg, so the unit moves on; the order fails on the last one.
                            Err(_) if !last => {
                                let o = self.objects.get_mut(&id).unwrap();
                                o.ai.retries = o.ai.retries.saturating_sub(1);
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
        // An order that ended this update entered the idle state, which runs
        // next frame (end_order's wake-up would otherwise be overwritten).
        if self.objects[&id].ai.active() || self.objects[&id].ai.idle.inited {
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
    pub(crate) fn drive(&mut self, id: u32) {
        if self.objects[&id].contained_by.is_some() {
            let o = self.objects.get_mut(&id).unwrap();
            o.target = None;
            o.path.clear();
            return;
        }
        let t = &self.templates.objects[&self.objects[&id].template];
        if t.locomotor().and_then(|n| self.templates.locomotors.get(n)).is_none() {
            return;
        }
        let mover = self.mover(id);
        // Out of the map while it steers, so the pathfinder can read the others.
        let mut owned = self.objects.remove(&id).unwrap();
        let o = &mut owned;
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
        let grid = self.views.get(&view_key(t, l)).unwrap_or(&self.grid);
        let pf = crate::pathfind::Pathfinder {
            terrain: grid,
            map: &self.map,
            units: &self.unit_cells,
            objects: &self.objects,
            templates: &self.templates,
            mover,
        };
        steer(o, l, grid, &self.map, self.frame, radii, speed, &mut blocked, &pf);
        if !blocked && o.blocked_frames > 1 {
            o.blocked_frames = 1;
        }
        o.traffic.max_blocked_speed = traffic::FAST_AS_POSSIBLE;
        self.objects.insert(id, owned);
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

    pub(crate) fn rebuild_partition_now(&mut self) {
        self.rebuild_partition();
    }
    fn rebuild_partition(&mut self) {
        // Passengers are out of the partition (`addOrRemoveObjFromWorld`).
        let entries = self.objects.values().filter(|o| o.contained_by.is_none()).map(|o| crate::partition::Entry {
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
        // fixInvalidPosition pushes them off invalid cells. A step over a deck's
        // side still stops, since falling off a bridge is not modelled yet; at the
        // ends units drive on and off (`updateLayer` moves them to the ground).
        if layer != 0
            && !self.terrain(id).line_on(
                Waypoint { position: [from[0], from[1]], layer },
                Waypoint { position: to, layer },
                radius,
            )
            && self.grid.layers[layer as usize - 1].bridge.beside_deck(to)
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
            let mut n = ih_math::normalized([other.position[0] - to[0], other.position[1] - to[1]]);
            let rest = [velocity[0] * (1.0 - fraction), velocity[1] * (1.0 - fraction)];
            let into = rest[0] * n[0] + rest[1] * n[1];
            if into > 0.0 {
                let at = body.with_position([to[0], to[1], body.position[2]]);
                // The centre-to-centre normal is a circle's contact normal. Against a
                // box, sliding along one of the boxes' edge directions gets past a
                // corner where that does not, so the best of those is taken too.
                let mut slides = vec![[rest[0] - n[0] * into, rest[1] - n[1] * into]];
                if other.geom == traffic::Geom::Box || body.geom == traffic::Geom::Box {
                    for angle in [other.angle, body.angle] {
                        let (c, s) = (ih_math::cos(angle), ih_math::sin(angle));
                        for axis in [[c, s], [-s, c]] {
                            let d = rest[0] * axis[0] + rest[1] * axis[1];
                            slides.push([axis[0] * d, axis[1] * d]);
                        }
                    }
                }
                let mut best = ([0.0f32; 2], 0.0f32);
                for slide in slides {
                    let (f2, _) = clip(at, slide);
                    let moved = [slide[0] * f2, slide[1] * f2];
                    let len = ih_math::length(moved);
                    if len > best.1 + 1e-6 {
                        best = (moved, len);
                    }
                }
                to = [to[0] + best.0[0], to[1] + best.0[1]];
                if best.1 > 0.0 {
                    // Velocity keeps only the component along the slide taken.
                    let d = ih_math::normalized(best.0);
                    n = [-d[1], d[0]];
                    if n[0] * (other.position[0] - to[0]) + n[1] * (other.position[1] - to[1]) < 0.0 {
                        n = [-n[0], -n[1]];
                    }
                }
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
        let mut enters = Vec::new();
        for a in self.objects.keys().copied() {
            if self.objects[&a].contained_by.is_some() {
                continue;
            }
            let ca = self.collider(a);
            if !ca.enabled || ca.immobile {
                continue;
            }
            // Pairs of immobile objects are left out (nothing moves either of them).
            for cb in self.nearby(a, traffic::CONTACT_RANGE) {
                if (cb.id <= a && !cb.immobile) || !ca.interacts(cb) {
                    continue;
                }
                // OpenContain::onCollide: a unit entering touches its container.
                let entering = |x: u32, y: u32| {
                    let o = &self.objects[&x];
                    o.ai.state == AiState::Enter && o.ai.enter == Some(y)
                };
                if (entering(a, cb.id) || entering(cb.id, a)) && traffic::contact(ca, cb) {
                    enters.push(if entering(a, cb.id) { (cb.id, a) } else { (a, cb.id) });
                }
                let ignored = |x: u32, y: u32| self.objects[&x].ai.ignore_obstacle == Some(y);
                if ignored(a, cb.id) || ignored(cb.id, a) {
                    continue;
                }
                // The partition's collision test on the real shapes (within contact range).
                if traffic::contact(ca, cb) {
                    pairs.push((a.min(cb.id), a.max(cb.id)));
                }
            }
        }
        pairs.sort_unstable();
        enters.sort_unstable();
        enters.dedup();
        for (container, rider) in enters {
            if self.objects.contains_key(&container) && self.objects.contains_key(&rider) {
                self.contain_collide(container, rider);
            }
        }
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
            // SquishCollide: HUGE_DAMAGE_AMOUNT of DAMAGE_CRUSH.
            self.crushed.push((self.frame, victim, crusher));
            let info = crate::body::DamageInfo::new(
                crusher,
                crate::body::DAMAGE_CRUSH,
                crate::body::DEATH_CRUSHED,
                crate::body::HUGE_DAMAGE_AMOUNT,
            );
            self.attempt_damage(victim, info);
        }
    }

    pub(crate) fn has_ai(&self, id: u32) -> bool {
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
        if ta.has_kind("INFANTRY") && tb.has_kind("INFANTRY") && !other_moving {
            // ironhour: infantry pass through infantry here (see `traffic`), so a
            // stationary one does not hold them up.
            return false;
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
        let (goal, _, _) = point_on_path(&mut probe, self.terrain(id), &self.map, &self.pathfinder(id));
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
    pub(crate) fn move_away_from_unit(&mut self, id: u32, unit: u32) {
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
        crate::pathfind::crush_or_squish(&self.templates, &self.objects, a, b, test)
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
    fn overlap_collision(&mut self, a: u32, b: u32) -> bool {
        let v = self.objects[&a].physics.vel;
        if v.iter().all(|c| c.abs() < 0.01) {
            return false;
        }
        let crushing = self.can_crush_or_squish(a, b, CrushTest::Crush);
        let crushed = self.can_crush_or_squish(b, a, CrushTest::Crush);
        if crushing && crushed {
            return false;
        }
        if crushing && !crushed {
            // addOverlap: the object being driven over this frame.
            self.objects.get_mut(&a).unwrap().physics.current_overlap = b;
        }
        crushed || crushing
    }

    pub(crate) fn repath_traffic(&mut self, id: u32) {
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
        // `computePath` while blocked and stuck: units' cells now steer the search
        // (parked allies cost more, parked enemies block).
        let goal = if o.goal_cell.is_some() && self.ground_movement(id) {
            Waypoint { position: target, layer: o.destination_layer }
        } else {
            self.grid.destination(&self.map, target)
        };
        let path = self
            .pathfinder(id)
            .find_path([o.position[0], o.position[1]], goal.position, goal.layer)
            .map(|p| p.optimized());
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
            self.move_allies(id);
        } else if o.ai.state == AiState::FollowPath && !o.ai.remaining.is_empty() {
            // AIFollowPathState::update: a failed leg is retried while retries last,
            // then the unit moves on to the next point.
            if o.ai.retries > 0 {
                o.ai.retries -= 1;
            } else {
                o.target = None;
                o.path.clear();
                o.traffic.stuck = false;
                o.blocked_frames = 0;
            }
        }
    }
    /// Live colliding object pairs, for headless invariants and diagnostics.
    pub fn overlaps(&self) -> Vec<(u32, u32)> {
        let mut pairs = Vec::new();
        // Passengers are out of the world; units just out of a transport or
        // into one ignore collisions (`setIgnoreCollisionTime`, `ignoreObstacle`).
        let excused = |x: u32, y: u32| {
            let o = &self.objects[&x];
            o.traffic.ignore_until > self.frame || o.ai.ignore_obstacle == Some(y)
        };
        for id in self.objects.keys().copied() {
            if self.objects[&id].contained_by.is_some() {
                continue;
            }
            let a = self.collider(id);
            if a.immobile {
                continue;
            }
            for b in self.nearby(id, 0.0) {
                if id < b.id
                    && !a.immobile
                    && !b.immobile
                    && !excused(id, b.id)
                    && !excused(b.id, id)
                    && traffic::overlaps(a, b)
                    && !self.crush_pair(id, b.id)
                {
                    pairs.push((id, b.id));
                }
            }
        }
        pairs
    }
    pub fn state_hash(&self) -> u64 {
        let mut h = StateHash::default();
        h.u32(33);
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
        h.u64(self.unit_cells.cells.len() as u64);
        for ((l, x, y), c) in &self.unit_cells.cells {
            for v in [*l, *x as u32, *y as u32, c.flags as u32, c.goal_unit, c.pos_unit, c.goal_aircraft] {
                h.u32(v);
            }
        }
        h.u64(self.objects.len() as u64);
        for o in self.objects.values() {
            h.u32(o.id);
            h.text(&o.template);
            h.u32(o.owner);
            h.u32(o.map_index.map_or(0, |i| i as u32 + 1));
            h.u32(u32::from(o.dead));
            h.u64(o.status);
            h.u32(u32::from(o.health.is_some()));
            if let Some(b) = &o.health {
                h.u32(b.kind as u32);
                for v in [b.current, b.previous, b.max, b.initial, b.subdual, b.subdual_cap, b.scalar] {
                    h.float(v);
                }
                h.u32(b.state as u32);
                h.u32(u32::from(b.indestructible));
                h.u32(b.armor_flags);
                h.u32(b.last_damage_frame);
                h.u32(b.last_healing_frame);
                h.u32(u32::from(b.die_called));
                h.u32(u32::from(b.last_damage.is_some()));
                if let Some(d) = b.last_damage {
                    h.u32(d.source);
                    h.u32(u32::from(d.damage_type));
                    h.u32(u32::from(d.death_type));
                    h.float(d.amount);
                    h.u32(u32::from(d.kill));
                    h.float(d.dealt);
                    h.float(d.clipped);
                }
            }
            for c in [o.goal_cell, o.cur_cell] {
                h.u32(u32::from(c.is_some()));
                let (x, y) = c.unwrap_or_default();
                h.u32(x as u32);
                h.u32(y as u32);
            }
            h.u32(o.destination_layer);
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
            h.u32(o.ai.retries);
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
            for v in o.physics.vel.into_iter().chain(o.physics.prev_accel).chain(o.slide) {
                h.float(v);
            }
            h.u32(o.physics.motive_expires);
            h.u32(u32::from(o.physics.braking_status) | u32::from(o.physics.ever_run) << 1);
            h.u32(o.physics.current_overlap);
            h.u32(o.physics.turning as u32);
            h.u32(o.physics.previous_overlap);
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
                let d = m.death;
                for v in [d.sink_frame, d.midpoint_frame, d.destruction_frame] {
                    h.u32(v);
                }
                for b in [d.activated, d.midpoint_done, d.flung, d.bounced] {
                    h.u32(u32::from(b));
                }
            }
            h.u32(u32::from(o.ai.dead));
            let ws = &o.weapon_set;
            h.u32(ws.set.map_or(0, |s| s as u32 + 1));
            h.u32(u32::from(ws.current));
            h.u32(u32::from(ws.lock));
            h.u32(u32::from(ws.shared_reload));
            for w in &ws.weapons {
                h.u32(u32::from(w.is_some()));
                if let Some(w) = w {
                    h.text(&w.template);
                    h.u32(w.status as u32);
                    for v in [w.ammo, w.max_shots, w.barrel, w.shots_for_barrel] {
                        h.u32(v as u32);
                    }
                    for v in [w.fire_again_frame, w.pre_attack_finished, w.last_reload_started, w.last_fire_frame] {
                        h.u32(v);
                    }
                }
            }
            h.u32(u32::from(o.flight.is_some()));
            if let Some(f) = &o.flight {
                h.u32(f.launcher);
                h.u32(f.victim);
                h.text(&f.weapon);
                h.u32(f.lifespan_frame);
                h.u64(f.path.len() as u64);
                for p in &f.path {
                    for v in p {
                        h.float(*v);
                    }
                }
                for v in f.start.into_iter().chain(f.end) {
                    h.float(v);
                }
                h.float(f.speed);
                h.u32(f.segments as u32);
                h.u64(f.step as u64);
                h.u32(u32::from(f.detonated));
            }
            h.u32(u32::from(o.missile.is_some()));
            if let Some(m) = &o.missile {
                h.u32(m.state as u32);
                h.u32(m.state_frame);
                h.u32(m.launcher);
                h.u32(m.victim);
                h.text(&m.weapon);
                h.u32(u32::from(m.armed));
                h.u32(m.fuel_expires);
                h.u32(m.goal_object);
                h.u32(u32::from(m.tracking));
                h.u32(u32::from(m.braking));
                h.u32(m.motive_until);
                for v in
                    [m.no_turn_dist_left, m.max_accel, m.loco_accel, m.loco_turn, m.loco_speed.unwrap_or(-1.0), m.pitch]
                {
                    h.float(v);
                }
                for v in m.original_target.into_iter().chain(m.prev_pos).chain(m.goal).chain(m.vel).chain(m.accel) {
                    h.float(v);
                }
            }
            h.u32(u32::from(o.ai.cmd_from_ai));
            let i = o.ai.idle;
            for v in [u32::from(i.inited), i.sleep_offset, i.next_mood_check, u32::from(i.randomly_offset)] {
                h.u32(v);
            }
            h.u32(u32::from(o.ai.guard.is_some()));
            if let Some(g) = &o.ai.guard {
                for v in g.position {
                    h.float(v);
                }
                h.u32(g.nemesis);
                h.u32(g.sub as u32);
                h.u32(g.next_scan);
            }
            h.u32(o.health.as_ref().map_or(0, |b| u32::from(b.last_damage_cleared)));
            h.u32(u32::from(o.ai.attack.is_some()));
            if let Some(m) = &o.ai.attack {
                h.u32(u32::from(m.follow));
                if let Some(e) = m.exit {
                    for v in e.center {
                        h.float(v);
                    }
                    h.float(e.radius_sqr);
                    h.u32(e.give_up_frame);
                    for b in [e.if_expired, e.if_outside_radius, e.if_no_unit] {
                        h.u32(u32::from(b));
                    }
                }
                h.u32(m.victim.unwrap_or(0));
                for v in m.position.into_iter().chain(m.prev_victim_pos) {
                    h.float(v);
                }
                h.u32(m.sub as u32);
                h.u32(m.approach_frame);
                h.u32(u32::from(m.stop_if_in_range));
                h.u32(u32::from(m.can_turn_in_place));
            }
        }
        h.u64(self.players.len() as u64);
        for (id, p) in &self.players {
            h.u32(*id);
            h.u32(p.money);
            h.u64(p.sciences.len() as u64);
            for s in &p.sciences {
                h.text(s);
            }
            for list in [&p.tunnels.tunnels, &p.tunnels.riders] {
                h.u64(list.len() as u64);
                for v in list {
                    h.u32(*v);
                }
            }
            for set in [&p.upgrades.in_progress, &p.upgrades.completed] {
                h.u64(set.len() as u64);
                for s in set {
                    h.text(s);
                }
            }
            h.u64(p.upgrades.cost_changes.len() as u64);
            for (kinds, percent, refs) in &p.upgrades.cost_changes {
                h.u64(kinds.len() as u64);
                for k in kinds {
                    h.text(k);
                }
                h.float(*percent);
                h.u32(*refs);
            }
        }
        h.u64(self.sell_list.len() as u64);
        for (id, f) in &self.sell_list {
            h.u32(*id);
            h.u32(*f);
        }
        for o in self.objects.values() {
            h.u32(o.contained_by.unwrap_or(0));
            h.u32(o.contained_frame);
            h.u32(o.bonus_flags);
            h.u32(o.die_frame.unwrap_or(0));
            h.u32(u32::from(o.stealth.is_some()));
            if let Some(s) = &o.stealth {
                h.u32(s.allowed_frame);
                h.u32(s.detection_expires);
                h.u32(s.next_black_market_check);
                h.u32(u32::from(s.enabled));
                h.u32(s.frames_granted);
            }
            h.u32(u32::from(o.detector.as_ref().is_some_and(|d| d.enabled)));
            let e = o.experience;
            h.u32(u32::from(e.level));
            h.u32(e.points as u32);
            h.float(e.scalar);
            h.u32(e.sink.unwrap_or(0));
            h.u32(o.ai.enter.unwrap_or(0));
            h.u32(u32::from(o.contain.is_some()));
            if let Some(c) = &o.contain {
                h.u64(c.riders.len() as u64);
                for r in &c.riders {
                    h.u32(*r);
                }
                for v in [c.extra_slots as u32, c.exit_not_busy, c.door_countdown, c.which_exit as u32] {
                    h.u32(v);
                }
                h.u32(u32::from(c.payload_created));
                for v in c.rally.unwrap_or_default() {
                    h.float(v);
                }
                h.u32(c.original_owner.unwrap_or(u32::MAX));
                for p in &c.points {
                    h.u32(p.unwrap_or(0));
                }
            }
            let (healer, until) = o.sole_healer.unwrap_or_default();
            h.u32(healer);
            h.u32(until);
            h.u32(o.construction.map_or(u32::MAX, f32::to_bits));
            h.u32(o.builder.unwrap_or(0));
            h.u32(u32::from(o.dozer.is_some()));
            if let Some(d) = &o.dozer {
                for t in &d.tasks {
                    h.u32(t.target.unwrap_or(0));
                    h.u32(t.order_frame);
                    for p in t.dock {
                        for v in p.unwrap_or([f32::NAN; 3]) {
                            h.float(v);
                        }
                    }
                }
                h.u32(d.current.map_or(u32::MAX, |c| c as u32));
                h.u32(d.primary as u32);
                h.u32(d.idle_since);
                h.u32(d.action as u32);
                h.u32(d.goal.unwrap_or(0));
                for v in d.goal_position {
                    h.float(v);
                }
                h.u32(d.build_sub as u32);
                h.u32(d.enter_frame);
                h.u32(d.worker.map_or(u32::MAX, u32::from));
            }
            let u = &o.upgrades;
            h.u64(u.completed.len() as u64);
            for s in &u.completed {
                h.text(s);
            }
            for e in &u.executed {
                h.u32(u32::from(*e));
            }
            h.u32(u.weapon_set_flags);
            h.u32(u32::from(u.weapon_bonus));
            h.text(u.command_set.as_deref().unwrap_or(""));
            h.u32(o.ai.ignore_obstacle.unwrap_or(0));
            h.u32(u32::from(o.production.is_some()));
            if let Some(p) = &o.production {
                h.u32(p.next_id);
                h.u32(p.complete_frame);
                for d in p.doors {
                    for v in [d.opened, d.wait_open, d.closed] {
                        h.u32(v);
                    }
                }
                h.u64(p.queue.len() as u64);
                for e in &p.queue {
                    h.u32(e.id);
                    h.text(&e.template);
                    h.u32(u32::from(e.upgrade));
                    h.u32(e.frames as u32);
                    h.float(e.percent);
                    h.u32(e.total);
                    h.u32(e.produced);
                    h.u32(e.door.unwrap_or(u32::MAX));
                }
            }
            h.u32(u32::from(o.dock.is_some()));
            if let Some(d) = &o.dock {
                h.u32(u32::from(d.open));
                h.u32(u32::from(d.loaded));
                h.u32(d.approach_bones as u32);
                h.u32(d.active.unwrap_or(0));
                h.u32(u32::from(d.inside));
                h.u32(d.boxes as u32);
                h.u64(d.owners.len() as u64);
                for (o, r) in d.owners.iter().zip(&d.reached) {
                    h.u32(o.unwrap_or(0));
                    h.u32(u32::from(*r));
                }
            }
            h.u32(u32::from(o.truck.is_some()));
            if let Some(t) = &o.truck {
                h.u32(t.state as u32);
                h.u32(t.boxes as u32);
                h.u32(t.preferred.unwrap_or(0));
                h.u32(u32::from(t.force_wanting));
                h.u32(u32::from(t.force_busy));
            }
            h.u32(u32::from(o.ai.dock.is_some()));
            if let Some(m) = &o.ai.dock {
                h.u32(m.goal);
                h.u32(m.sub.map_or(u32::MAX, |s| s as u32));
                h.u32(m.approach as u32);
                h.u32(m.enter_frame);
                h.u32(m.next_action);
                for v in m.goal_position {
                    h.float(v);
                }
            }
            h.u32(u32::from(o.exit.is_some()));
            if let Some(e) = &o.exit {
                h.u32(e.current_delay);
                h.u32(e.burst);
                h.u32(u32::from(e.rally.is_some()));
                for v in e.rally.unwrap_or_default() {
                    h.float(v);
                }
            }
        }
        h.u64(self.scheduler.heap.len() as u64);
        for e in &self.scheduler.heap {
            h.u32(e.key.0);
            h.u64(e.key.1 as u64);
            h.u32(e.frame);
            h.u32(e.phase as u32);
        }
        h.u64(self.delayed_damage.len() as u64);
        for d in &self.delayed_damage {
            h.text(&d.weapon);
            for v in d.position {
                h.float(v);
            }
            h.u32(d.frame);
            h.u32(d.source);
            h.u32(d.victim);
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
        Command::Guard { object, position } => {
            h.u32(13);
            h.u32(*object);
            h.float(position[0]);
            h.float(position[1]);
        }
        Command::Attack { object, target } => {
            h.u32(11);
            h.u32(*object);
            h.u32(*target);
        }
        Command::AttackPosition { object, target } => {
            h.u32(12);
            h.u32(*object);
            h.float(target[0]);
            h.float(target[1]);
        }
        Command::Damage { object, amount, damage_type, death_type, source } => {
            h.u32(10);
            h.u32(*object);
            h.float(*amount);
            h.text(damage_type);
            h.text(death_type);
            h.u32(*source);
        }
        Command::Spawn { template, position, owner } => {
            h.u32(3);
            h.text(template);
            for v in position {
                h.float(*v);
            }
            h.u32(*owner);
        }
        Command::GroupMove { objects, target } => {
            h.u32(9);
            h.u64(objects.len() as u64);
            for o in objects {
                h.u32(*o);
            }
            h.float(target[0]);
            h.float(target[1]);
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
        Command::Deposit { player, amount } => {
            h.u32(14);
            h.u32(*player);
            h.u32(*amount);
        }
        Command::GrantScience { player, science } => {
            h.u32(15);
            h.u32(*player);
            h.text(science);
        }
        Command::Produce { factory, template } => {
            h.u32(16);
            h.u32(*factory);
            h.text(template);
        }
        Command::CancelProduction { factory, production } => {
            h.u32(17);
            h.u32(*factory);
            h.u32(*production);
        }
        Command::SetRallyPoint { object, position } => {
            h.u32(18);
            h.u32(*object);
            h.float(position[0]);
            h.float(position[1]);
        }
        Command::Dock { object, target } => {
            h.u32(19);
            h.u32(*object);
            h.u32(*target);
        }
        Command::Construct { dozer, template, position, angle } => {
            h.u32(22);
            h.u32(*dozer);
            h.text(template);
            h.float(position[0]);
            h.float(position[1]);
            h.float(*angle);
        }
        Command::ResumeConstruction { dozer, target } => {
            h.u32(23);
            h.u32(*dozer);
            h.u32(*target);
        }
        Command::Repair { dozer, target } => {
            h.u32(24);
            h.u32(*dozer);
            h.u32(*target);
        }
        Command::Sell { object } => {
            h.u32(25);
            h.u32(*object);
        }
        Command::SetVeterancy { object, level } => {
            h.u32(29);
            h.u32(*object);
            h.u32(u32::from(*level));
        }
        Command::Enter { object, target } => {
            h.u32(26);
            h.u32(*object);
            h.u32(*target);
        }
        Command::Exit { object } => {
            h.u32(27);
            h.u32(*object);
        }
        Command::Evacuate { object } => {
            h.u32(28);
            h.u32(*object);
        }
        Command::Research { factory, upgrade } => {
            h.u32(20);
            h.u32(*factory);
            h.text(upgrade);
        }
        Command::CancelResearch { factory, upgrade } => {
            h.u32(21);
            h.u32(*factory);
            h.text(upgrade);
        }
    }
}
/// The goal point and remaining distance along a path (`Path::computePointOnPath`).
/// `path[0]` is the start of the current segment. Advances `path` past segments
/// the object has moved beyond (switching layer where allowed).
fn point_on_path(o: &mut Object, grid: &Terrain, map: &Map, pf: &crate::pathfind::Pathfinder) -> ([f32; 3], f32, u32) {
    // `isLinePassable(obj, surfaces, layer, pos, to, false, true)`.
    let line = |layer: u32, a: [f32; 2], b: [f32; 2]| pf.is_line_passable(layer, a, b, false, true);
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
        o.path.pop_front();
    }
    // Standing on the next node (as after braking onto a deck end): it is passed, so
    // the aim moves past it, as the original's look-ahead point does.
    while o.path.len() > 2 && {
        let n = o.path[1];
        ih_math::length([n.position[0] - pos[0], n.position[1] - pos[1]]) < 0.01
    } {
        o.path.pop_front();
    }
    let (a, b) = (o.path[0], o.path[1]);
    // The segment's bridge layer, if any, then `Pathfinder::updateLayer`: the
    // object is on it only while on the deck or at an end, near the deck height.
    let mut layer = if b.layer > 0 { b.layer } else { a.layer };
    if let Some(l) = layer.checked_sub(1).and_then(|i| grid.layers.get(i as usize))
        && !l.bridge.interacts(o.position, minor_radius(&pf.templates.objects[&o.template]))
    {
        layer = 0;
    }
    if layer != o.layer {
        o.layer = layer;
        o.position[2] = grid.height_at(map, Waypoint { position: pos, layer });
    }
    let (dir, len) = segment(a, b);
    let mut along = dot(dir, sub(pos, a.position)).max(0.0);
    let to = sub(pos, a.position);
    let off2 = dot(to, to) - along * along;
    let off = if off2 <= 0.0 { 0.0 } else { ih_math::sqrt(off2) };
    // Basically on the path: aim at the next node. The farther off the path, the
    // closer the goal moves to the projected point.
    let mut k = (off / (3.0 * crate::locomotion::CELL)).min(1.0);
    let mut goal = None;
    if line(o.layer, pos, b.position) {
        goal = Some(b.position);
        let mut try_ahead = along > len * 0.5 && a.layer == b.layer && o.layer == 0;
        let very_close = len - along < 1.0;
        if very_close {
            try_ahead = true;
        }
        if try_ahead && let Some(next) = o.path.get(2) {
            let mid = [(b.position[0] + next.position[0]) * 0.5, (b.position[1] + next.position[1]) * 0.5];
            if very_close || line(b.layer, pos, mid) {
                goal = Some(mid);
            }
        }
    } else if k > 0.5 {
        let try_dist = along + 0.5 * (len - along);
        let p = [a.position[0] + try_dist * dir[0], a.position[1] + try_dist * dir[1]];
        if line(o.layer, pos, p) {
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
    pf: &crate::pathfind::Pathfinder,
) {
    let pos = [o.position[0], o.position[1]];
    // setPathExtraDistance: on a follow-path leg with more points after it, the
    // distance on to the next point (plus 4 cells if another follows) keeps the
    // locomotor from braking for this one.
    let extra = match (o.ai.state, o.target, o.ai.remaining.front()) {
        (crate::ai::AiState::FollowPath, Some(t), Some(n)) => {
            ih_math::length([n[0] - t[0], n[1] - t[1]]) + if o.ai.remaining.len() > 1 { 4.0 * CELL } else { 0.0 }
        }
        _ => 0.0,
    };
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
    let (goal, remaining, _) = point_on_path(o, grid, map, pf);
    // With an extra distance the leg uses the "easy" success test: close enough
    // along the path (`getTreatAsAircraftForLocoDistToGoal`, `CloseEnoughDist`).
    if extra > 1.0 && remaining < l.close_enough_dist {
        o.path.clear();
        o.target = None;
        o.blocked_frames = 0;
        o.blocked_by = None;
        o.traffic.stuck = false;
        return;
    }
    let layer = o.layer;
    // AIEnterState: `setAllowInvalidPosition(true)` while walking into a container.
    let allow_invalid = o.ai.state == AiState::Enter;
    // validMovementTerrain: the cell under the point is passable on this layer. On
    // a bridge layer, as in the original, the impassable cells around the deck
    // count as valid; only cells beyond the layer do not.
    let valid = |p: [f32; 2]| {
        if allow_invalid {
            return true;
        }
        let c = crate::locomotion::CELL;
        let (x, y) = ((p[0] / c).floor() as i32, (p[1] / c).floor() as i32);
        if layer == 0 {
            return grid.passable_on(layer, x, y, 0.0);
        }
        grid.cell_at(layer, x, y).is_some_and(|cell| {
            cell.kind == crate::footprint::CellType::Impassable || cell.kind.allows(l.surface_mask())
        })
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
    let slide = body.move_towards_position(l, goal, remaining + extra, speed, blocked, &valid, z);
    // Turning about an off-centre pivot moves the body; that motion goes through
    // the same collision clipping as the rest of this frame's displacement.
    let pivot = [o.position[0] - before[0], o.position[1] - before[1]];
    o.position[0] = before[0];
    o.position[1] = before[1];
    o.slide = [slide[0] + pivot[0], slide[1] + pivot[1]];
}

/// The terrain view for a template: its locomotor's surfaces, plus the crusher
/// flag when it can crush (`getCrusherLevel() > 0`), whose view opens fence cells.
/// `GeometryInfo::getMinorRadius`: a box's minor radius, else the major one.
fn minor_radius(t: &crate::templates::Template) -> f32 {
    let major = t.number("GeometryMajorRadius", 5.0);
    let box_ = t.values("Geometry").first().is_some_and(|g| g.eq_ignore_ascii_case("BOX"));
    let minor = if box_ { t.number("GeometryMinorRadius", 0.0) } else { major };
    minor * t.number("Scale", 1.0)
}

pub(crate) fn view_key(t: &crate::templates::Template, l: &Locomotor) -> u8 {
    let crusher = if t.number("CrusherLevel", 0.0) > 0.0 { crate::footprint::surface::CRUSHER } else { 0 };
    l.surface_mask() | crusher
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
    /// Where a free goal at `p` ends up (`adjustDestination` on an empty map).
    fn snapped(s: &Simulation, id: u32, p: [f32; 2]) -> [f32; 2] {
        let r = s.templates.objects[&s.objects[&id].template].collision_radius();
        let (_, center) = crate::goals::radius_and_center(r);
        let q = if center { p } else { [p[0] + 5.0, p[1] + 5.0] };
        crate::goals::coord_of_cell((q[0] / 10.0).floor() as i32, (q[1] / 10.0).floor() as i32, center)
    }
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
        assert!((a.objects[&1].position[0] - snapped(&a, 1, [125.0, 0.0])[0]).abs() < 0.5);
        assert!(a.objects[&1].target.is_none());
        assert_eq!(a.state_hash(), 0xc0152113b75327cd, "portable synthetic movement fixture changed");
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
        assert!((a.objects[&1].position[0] - snapped(&a, 1, [125.0, 0.0])[0]).abs() < 0.5);
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
        // A body and a death that removes it (`ActiveBody`, `DestroyDie`).
        let defs = ih_ini::parse(
            b"Object X
Body = ActiveBody Body
MaxHealth = 100
InitialHealth = 100
End
Behavior = DestroyDie Die
End
End
",
        );
        let x = &defs.blocks[0];
        for b in x.blocks() {
            let args = b.args();
            soldier.modules.push(crate::templates::ModuleDef {
                class: args[0].clone(),
                tag: args[1].clone(),
                kind: b.kind.clone(),
                block: b.clone(),
            });
        }
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
        assert!((s.objects[&1].position[0] - snapped(&s, 1, [175.0, 0.0])[0]).abs() < 0.5);
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
        assert!((s.objects[&1].position[0] - snapped(&s, 1, [175.0, 0.0])[0]).abs() < 0.5, "{:?}", s.objects[&1]);
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
        // Either the mover repaths round the parked ally or the ally steps aside
        // (`moveAllies`: the new path runs through its cells).
        assert!(s.objects[&1].traffic_repaths > 0 || s.move_asides > 0);
        assert!((s.objects[&1].position[0] - snapped(&s, 1, [175.0, 0.0])[0]).abs() < 0.5);
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
        assert!(s.objects[&1].traffic_repaths > 0 || s.move_asides > 0);
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
        assert!((s.objects[&1].position[1] - snapped(&s, 1, [0.0, 175.0])[1]).abs() < 0.5);
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
        assert_eq!(a.state_hash(), 0xa8e0f2f6de2e439d, "portable traffic fixture changed");
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
        // A later order to the ally's goal takes the nearest free cell instead
        // (`adjustDestination`: "Don't usurp your allies goals").
        s.enqueue(s.frame, Command::Move { object: 1, target: [175.0, 105.0] }).unwrap();
        for _ in 0..240 {
            s.step();
            assert!(s.overlaps().is_empty());
        }
        let (a, b) = (s.objects[&1].position, s.objects[&2].position);
        assert!(s.objects[&1].target.is_none());
        assert!(ih_math::length([a[0] - 175.0, a[1] - 105.0]) < 30.0, "{a:?}");
        assert!(ih_math::length([a[0] - b[0], a[1] - b[1]]) >= 19.0, "{a:?} {b:?}");
        // Destroying the blocker releases its cells.
        s.enqueue(s.frame, Command::Destroy { object: 2 }).unwrap();
        s.step();
        assert!(s.unit_cells.cells.values().all(|c| c.goal_unit != 2 && c.pos_unit != 2));
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
            from: [65.0, 100.0, 8.0],
            to: [135.0, 100.0, 8.0],
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
                assert_eq!(o.position[2], 8.0);
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
        assert_eq!(a.state_hash(), 0xcbde734037fae148, "portable bridge fixture changed");
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
