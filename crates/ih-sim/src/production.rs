//! Players' money and power, and unit production.
//!
//! Sources: `Common/RTS/Money.cpp`, `Common/RTS/Energy.cpp`,
//! `Common/RTS/ProductionPrerequisite.cpp`, `Common/Thing/ThingTemplate.cpp`
//! (`calcCostToBuild`, `calcTimeToBuild`, `getBuildFacilityTemplate`),
//! `Common/System/BuildAssistant.cpp` (`canMakeUnit`, `isPossibleToMakeUnit`),
//! `Common/RTS/Player.cpp` (`canBuild`, `canBuildMoreOfType`),
//! `GameLogic/Object/Update/ProductionUpdate.cpp`, and
//! `GameLogic/Object/Update/ProductionExitUpdate/{Default,Queue,SupplyCenter}ProductionExitUpdate.cpp`.
//!
//! Not ported yet: upgrades in the queue, handicaps and faction cost or time
//! changes (all 1), parking places, sold factories, `SpawnPointProductionExitUpdate`
//! (needs bones), airborne creation's starting force, power bonuses from
//! upgrades and disabled power plants, max-simultaneous link keys, script-disabled
//! factories, and player types (every player is human, so `ONLY_BY_AI` units
//! cannot be built).
use crate::{
    SimError, Simulation,
    ai::AiState,
    goals::coord_of_cell,
    navigation::CELL,
    templates::{Prerequisite, Template},
    terrain::Waypoint,
};
use std::collections::{BTreeSet, VecDeque};

/// A player's money (`Money`) and sciences.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Player {
    pub money: u32,
    pub sciences: BTreeSet<String>,
    /// Upgrades researched and in research (`upgrades`).
    pub upgrades: crate::upgrades::PlayerUpgrades,
    /// `TunnelTracker` (`contain`).
    pub tunnels: crate::contain::TunnelTracker,
    /// Rank, skill and science purchase points, shared power timers (`sciences`).
    pub generals: crate::sciences::Generals,
    /// `m_radarCount` and `m_disableProofRadarCount` (`crates`).
    pub radar: (i32, i32),
}
impl Player {
    /// `Money::withdraw`: no more than the balance; returns what was taken.
    pub fn withdraw(&mut self, amount: u32) -> u32 {
        let amount = amount.min(self.money);
        self.money -= amount;
        amount
    }
    /// `Money::deposit`.
    pub fn deposit(&mut self, amount: u32) {
        self.money = self.money.wrapping_add(amount);
    }
}

/// `GameData` production values, with the original defaults (all 0).
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct ProductionSettings {
    pub min_low_energy_speed: f32,
    pub max_low_energy_speed: f32,
    pub low_energy_penalty: f32,
    pub multiple_factory: f32,
}

/// `ProductionUpdateModuleData`.
#[derive(Clone, Debug, PartialEq)]
pub struct ProductionData {
    pub max_queue: u32,
    pub num_doors: u32,
    pub door_opening: u32,
    pub door_wait_open: u32,
    pub door_closing: u32,
    pub complete_duration: u32,
    pub quantity: Vec<(String, u32)>,
}
impl ProductionData {
    pub fn parse(b: &ih_ini::Block) -> Self {
        let mut d = Self {
            max_queue: 9,
            num_doors: 0,
            door_opening: 0,
            door_wait_open: 0,
            door_closing: 0,
            complete_duration: 0,
            quantity: Vec::new(),
        };
        for f in b.fields() {
            let v = f.values();
            let int = || v.first().and_then(|s| ih_ini::values::scan_real(s.as_bytes()).ok()).unwrap_or(0.0);
            match f.name.as_str() {
                "MaxQueueEntries" => d.max_queue = int() as u32,
                "NumDoorAnimations" => d.num_doors = int() as u32,
                "DoorOpeningTime" => d.door_opening = crate::death::duration(int()),
                "DoorWaitOpenTime" => d.door_wait_open = crate::death::duration(int()),
                "DoorCloseTime" => d.door_closing = crate::death::duration(int()),
                "ConstructionCompleteDuration" => d.complete_duration = crate::death::duration(int()),
                "QuantityModifier" => {
                    if let Some(name) = v.first() {
                        let n = v.get(1).and_then(|s| s.parse().ok()).unwrap_or(1);
                        d.quantity.push((name.clone(), n));
                    }
                }
                _ => {}
            }
        }
        d
    }
}

/// `DoorInfo`.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct Door {
    pub opened: u32,
    pub wait_open: u32,
    pub closed: u32,
}

/// `ProductionEntry` for a unit.
#[derive(Clone, Debug, PartialEq)]
pub struct Entry {
    pub id: u32,
    /// The unit, or the upgrade when `upgrade` is set.
    pub template: String,
    /// `PRODUCTION_UPGRADE`.
    pub upgrade: bool,
    pub frames: i32,
    pub percent: f32,
    pub total: u32,
    pub produced: u32,
    /// `m_exitDoor`; `None` is `DOOR_NONE_AVAILABLE`.
    pub door: Option<u32>,
}

/// `ProductionUpdate` state.
#[derive(Clone, Debug, PartialEq)]
pub struct Production {
    pub data: ProductionData,
    pub queue: VecDeque<Entry>,
    /// `m_uniqueID`.
    pub next_id: u32,
    pub doors: [Door; 4],
    pub complete_frame: u32,
}
impl Production {
    pub fn new(data: ProductionData) -> Self {
        Self { data, queue: VecDeque::new(), next_id: 1, doors: [Door::default(); 4], complete_frame: 0 }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ExitKind {
    Default,
    Queue,
    SupplyCenter,
}

/// A production exit module (`ExitInterface`).
#[derive(Clone, Debug, PartialEq)]
pub struct Exit {
    pub kind: ExitKind,
    pub create_point: [f32; 3],
    pub natural_rally: [f32; 3],
    pub exit_delay: u32,
    pub initial_burst: u32,
    pub allow_airborne: bool,
    pub current_delay: u32,
    pub burst: u32,
    pub rally: Option<[f32; 2]>,
}
impl Exit {
    pub fn from_module(class: &str, b: &ih_ini::Block) -> Option<Self> {
        let kind = match class {
            "DefaultProductionExitUpdate" => ExitKind::Default,
            "QueueProductionExitUpdate" => ExitKind::Queue,
            "SupplyCenterProductionExitUpdate" => ExitKind::SupplyCenter,
            _ => return None,
        };
        let mut e = Self {
            kind,
            create_point: [0.0; 3],
            natural_rally: [0.0; 3],
            exit_delay: 0,
            initial_burst: 0,
            allow_airborne: false,
            current_delay: 0,
            burst: 0,
            rally: None,
        };
        for f in b.fields() {
            let v = f.values();
            let num = |i: usize| v.get(i).and_then(|s| ih_ini::values::scan_real(s.as_bytes()).ok()).unwrap_or(0.0);
            // INI::parseCoord3D: `X:1 Y:2 Z:3`.
            let coord = || match ih_ini::values::parse_value(
                &ih_ini::schema::FieldDef { name: None, parser: "INI::parseCoord3D", user: "NULL" },
                &f.rest,
            ) {
                Ok(ih_ini::values::Value::Coord3(c)) => c,
                _ => [0.0; 3],
            };
            match f.name.as_str() {
                "UnitCreatePoint" => e.create_point = coord(),
                "NaturalRallyPoint" => e.natural_rally = coord(),
                "ExitDelay" if kind == ExitKind::Queue => e.exit_delay = crate::death::duration(num(0)),
                "InitialBurst" if kind == ExitKind::Queue => e.initial_burst = num(0) as u32,
                "AllowAirborneCreation" if kind == ExitKind::Queue => {
                    e.allow_airborne = v.first().is_some_and(|s| s.eq_ignore_ascii_case("yes"))
                }
                _ => {}
            }
        }
        e.burst = e.initial_burst;
        Some(e)
    }
    /// `isFreeToExit` (queue exits); the others always have `DOOR_1`.
    fn reserve(&self) -> Option<u32> {
        match self.kind {
            ExitKind::Queue => (self.burst > 0 || self.current_delay == 0).then_some(0),
            _ => Some(0),
        }
    }
}

/// `CanMakeType`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum CanMake {
    Ok,
    NoPrereq,
    NoMoney,
    QueueFull,
    MaxedOut,
}

/// `Object::getTransformMatrix` applied to a point: rotation about z, then translation.
pub(crate) fn transform(position: [f32; 3], angle: f32, p: [f32; 3]) -> [f32; 3] {
    let (s, c) = (ih_math::sin(angle), ih_math::cos(angle));
    [position[0] + c * p[0] - s * p[1], position[1] + s * p[0] + c * p[1], position[2] + p[2]]
}

impl Simulation {
    pub fn player(&mut self, id: u32) -> &mut Player {
        if let std::collections::btree_map::Entry::Vacant(e) = self.players.entry(id) {
            e.insert(Player::default());
            // Player::init: resetRank.
            self.reset_rank(id);
        }
        self.players.get_mut(&id).unwrap()
    }
    pub fn money(&self, player: u32) -> u32 {
        self.players.get(&player).map_or(0, |p| p.money)
    }

    /// `countObjectsByThingTemplate` (dead objects count; nothing is under construction yet).
    fn count_owned(&self, player: u32, template: &str) -> usize {
        let building = crate::construction::under_construction_bit();
        self.objects
            .values()
            .filter(|o| o.owner == player && o.template == template && o.status & building == 0)
            .count()
    }

    /// `Energy`: production and consumption from each owned object's `EnergyProduction`.
    pub fn energy(&self, player: u32) -> (i32, i32) {
        let (mut production, mut consumption) = (0, 0);
        let skip = crate::body::status::DESTROYED | crate::construction::under_construction_bit();
        for o in self.objects.values().filter(|o| o.owner == player && o.status & skip == 0) {
            let e = self.templates.objects[&o.template].number("EnergyProduction", 0.0) as i32;
            if e < 0 {
                consumption -= e;
            } else {
                production += e;
            }
            // Player::addPowerBonus from a power plant upgrade.
            production += self.power_bonus(o);
        }
        (production, consumption)
    }
    /// `Energy::getEnergySupplyRatio`.
    fn energy_ratio(&self, player: u32) -> f32 {
        let (p, c) = self.energy(player);
        if c == 0 { p as f32 } else { p as f32 / c as f32 }
    }

    /// `ThingTemplate::calcCostToBuild`.
    pub fn cost_to_build(&self, t: &Template, player: u32) -> u32 {
        // getBuildCost() * factionModifier * handicap; the faction modifier is the
        // upgrades' kind-of cost changes (no per-name changes yet), the handicap 1.
        let faction = (1.0 + 0.0) * self.cost_change(player, t);
        (t.number("BuildCost", 0.0) as u16 as f32 * faction * 1.0) as i32 as u32
    }

    /// `ProductionPrerequisite::getExistingBuildFacilityTemplate` of the first prerequisite.
    fn build_facility(&self, t: &Template, player: u32) -> Option<String> {
        match t.prerequisites.first()? {
            Prerequisite::Objects(names) => names.iter().find(|n| self.count_owned(player, n) > 0).cloned(),
            Prerequisite::Science(_) => None,
        }
    }

    /// `ThingTemplate::calcTimeToBuild`.
    pub fn time_to_build(&self, t: &Template, player: u32) -> i32 {
        let g = self.templates.production;
        let mut time = (t.number("BuildTime", 0.0) * 30.0) as i32;
        time = (time as f32 * 1.0) as i32; // handicap
        time = (time as f32 * (1.0 + 0.0)) as i32; // faction time change
        let mut percent = self.energy_ratio(player);
        if percent > 1.0 {
            percent = 1.0;
        }
        let short = (1.0 - percent) * g.low_energy_penalty;
        let mut rate = (1.0 - short).max(g.min_low_energy_speed);
        if percent < 1.0 {
            rate = rate.min(g.max_low_energy_speed);
        }
        if rate <= 0.0 {
            rate = 0.01;
        }
        time = (time as f32 / rate) as i32;
        let appears_at_rally =
            t.values("BuildCompletion").first().is_none_or(|v| v.eq_ignore_ascii_case("APPEARS_AT_RALLY_POINT"));
        if appears_at_rally && let Some(f) = self.build_facility(t, player) {
            let count = self.count_owned(player, &f);
            if g.multiple_factory > 0.0 {
                for _ in 1..count {
                    time = (time as f32 * g.multiple_factory) as i32;
                }
            }
        }
        time
    }

    /// `Player::canBuild`: buildable status and prerequisites.
    fn can_build(&self, t: &Template, player: u32) -> bool {
        match t.values("Buildable").first().map(|s| s.to_ascii_uppercase()).as_deref() {
            Some("NO") => return false,
            Some("IGNORE_PREREQUISITES") => return true,
            // Every player is human for now.
            Some("ONLY_BY_AI") => return false,
            _ => {}
        }
        let sciences = self.players.get(&player).map(|p| &p.sciences);
        t.prerequisites.iter().all(|p| match p {
            Prerequisite::Science(s) => sciences.is_some_and(|set| set.contains(s)),
            Prerequisite::Objects(names) => names.iter().any(|n| self.count_owned(player, n) > 0),
        })
    }

    /// `Player::canBuildMoreOfType`: existing objects, plus queued ones for units.
    fn can_build_more(&self, t: &Template, player: u32) -> bool {
        let max = t.number("MaxSimultaneousOfType", 0.0) as u32;
        if max == 0 {
            return true;
        }
        let mut count = self.count_owned(player, &t.name);
        if !t.has_kind("STRUCTURE") {
            for o in self.objects.values().filter(|o| o.owner == player) {
                if let Some(p) = &o.production {
                    count += p.queue.iter().filter(|e| !e.upgrade && e.template == t.name).count();
                }
            }
        }
        count < max as usize
    }

    /// `BuildAssistant::canMakeUnit`.
    pub fn can_make_unit(&self, factory: u32, unit: &str) -> CanMake {
        let Some(f) = self.objects.get(&factory) else { return CanMake::NoPrereq };
        let Some(t) = self.templates.objects.get(unit) else { return CanMake::NoPrereq };
        if !self.can_build_more(t, f.owner) {
            return CanMake::MaxedOut;
        }
        // isPossibleToMakeUnit: a build button for it in the factory's command set.
        let ft = &self.templates.objects[&f.template];
        let set = ft.values("CommandSet").first().and_then(|n| self.templates.command_sets.get(n));
        let found = set.is_some_and(|set| {
            set.iter()
                .filter_map(|b| self.templates.command_buttons.get(b))
                .any(|(command, object)| matches!(command.as_str(), "UNIT_BUILD" | "DOZER_CONSTRUCT") && object == unit)
        });
        if !found || !self.can_build(t, f.owner) {
            return CanMake::NoPrereq;
        }
        if let Some(p) = &f.production
            && p.queue.len() as u32 >= p.data.max_queue
        {
            return CanMake::QueueFull;
        }
        if self.cost_to_build(t, f.owner) > self.money(f.owner) {
            return CanMake::NoMoney;
        }
        CanMake::Ok
    }

    /// `ProductionUpdate::queueCreateUnit`. Returns the production ID.
    pub(crate) fn queue_create_unit(&mut self, factory: u32, unit: &str) -> Result<u32, SimError> {
        if self.objects.get(&factory).and_then(|o| o.production.as_ref()).is_none() {
            return Err(SimError(format!("object {factory} cannot produce")));
        }
        let can = self.can_make_unit(factory, unit);
        if can != CanMake::Ok {
            if can == CanMake::NoMoney {
                self.notices.push(crate::notices::Notice::InsufficientFunds { player: self.objects[&factory].owner });
            }
            return Err(SimError(format!("object {factory} cannot make {unit}: {can:?}")));
        }
        let owner = self.objects[&factory].owner;
        let cost = self.cost_to_build(&self.templates.objects[unit], owner);
        self.player(owner).withdraw(cost);
        let p = self.objects.get_mut(&factory).unwrap().production.as_mut().unwrap();
        let total = p.data.quantity.iter().find(|(n, _)| n == unit).map_or(1, |(_, q)| *q);
        let id = p.next_id;
        p.next_id += 1;
        p.queue.push_back(Entry {
            id,
            template: unit.into(),
            upgrade: false,
            frames: 0,
            percent: 0.0,
            total,
            produced: 0,
            door: None,
        });
        Ok(id)
    }

    /// `ProductionUpdate::cancelUnitCreate`: refunds the full cost.
    pub(crate) fn cancel_unit_create(&mut self, factory: u32, production: u32) -> Result<(), SimError> {
        let o = self.objects.get_mut(&factory).ok_or_else(|| SimError(format!("object {factory} is missing")))?;
        let owner = o.owner;
        let Some(p) = o.production.as_mut() else { return Err(SimError(format!("object {factory} cannot produce"))) };
        let Some(i) = p.queue.iter().position(|e| e.id == production && !e.upgrade) else { return Ok(()) };
        let e = p.queue.remove(i).unwrap();
        let cost = self.cost_to_build(&self.templates.objects[&e.template], owner);
        self.player(owner).deposit(cost);
        Ok(())
    }

    /// `setRallyPoint` on the exit module.
    pub(crate) fn set_rally_point(&mut self, object: u32, position: [f32; 2]) -> Result<(), SimError> {
        let o = self.objects.get_mut(&object).ok_or_else(|| SimError(format!("object {object} is missing")))?;
        if let Some(c) = o.contain.as_mut() {
            c.rally = Some(position);
            return Ok(());
        }
        let e = o.exit.as_mut().ok_or_else(|| SimError(format!("object {object} has no exit")))?;
        e.rally = Some(position);
        Ok(())
    }

    /// `QueueProductionExitUpdate::update`.
    pub(crate) fn exit_update(&mut self, id: u32) -> u32 {
        if let Some(e) = self.objects.get_mut(&id).and_then(|o| o.exit.as_mut()) {
            if e.reserve().is_some() {
                e.current_delay = 0;
            } else {
                e.current_delay -= 1;
            }
        }
        1
    }

    /// `ProductionUpdate::updateDoors`.
    fn update_doors(p: &mut Production, now: u32) {
        for d in &mut p.doors {
            if d.opened != 0 {
                if now - d.opened > p.data.door_opening {
                    d.opened = 0;
                    d.wait_open = now;
                }
            } else if d.wait_open != 0 {
                if now - d.wait_open > p.data.door_wait_open {
                    d.wait_open = 0;
                    d.closed = now;
                }
            } else if d.closed != 0 && now - d.closed > p.data.door_closing {
                d.closed = 0;
            }
        }
    }

    /// `ProductionUpdate::update` (every frame).
    pub(crate) fn production_update(&mut self, id: u32) -> u32 {
        let now = self.frame;
        let owner = self.objects[&id].owner;
        let sold = self.objects[&id].status & crate::construction::status_bits(&["SOLD"]) != 0;
        let Some(p) = self.objects.get_mut(&id).and_then(|o| o.production.as_mut()) else { return 1 };
        if p.data.num_doors > 0 {
            Self::update_doors(p, now);
        }
        if p.complete_frame != 0 && now - p.complete_frame > p.data.complete_duration {
            p.complete_frame = 0;
        }
        // A factory being sold builds nothing.
        if sold {
            return 1;
        }
        let Some((unit, upgrade)) = p.queue.front().map(|e| (e.template.clone(), e.upgrade)) else { return 1 };
        let total = if upgrade {
            let Some(u) = self.templates.upgrades.get(&unit) else { return 1 };
            u.time_to_build()
        } else {
            let Some(t) = self.templates.objects.get(&unit) else { return 1 };
            self.time_to_build(t, owner)
        };
        let p = self.objects.get_mut(&id).unwrap().production.as_mut().unwrap();
        let e = p.queue.front_mut().unwrap();
        e.frames += 1;
        e.percent = e.frames as f32 / total as f32 * 100.0;
        if e.percent < 100.0 {
            return 1;
        }
        if upgrade {
            self.finish_research(id, &unit);
            if let Some(p) = self.objects.get_mut(&id).and_then(|o| o.production.as_mut()) {
                p.queue.pop_front();
            }
            return 1;
        }
        if self.objects[&id].exit.is_none() {
            self.events.push(format!("frame {now}: cannot create {unit}, object {id} has no exit"));
            self.objects.get_mut(&id).unwrap().production.as_mut().unwrap().queue.pop_front();
            return 1;
        }
        let tries = {
            let e = &self.objects[&id].production.as_ref().unwrap().queue[0];
            e.total - e.produced
        };
        for _ in 0..tries {
            let o = self.objects.get_mut(&id).unwrap();
            let reserve = o.exit.as_ref().unwrap().reserve();
            let p = o.production.as_mut().unwrap();
            let e = p.queue.front_mut().unwrap();
            if e.door.is_none() {
                e.door = reserve;
            }
            let Some(door) = e.door else { continue };
            let animated = p.data.num_doors > 0 && door < 4;
            if animated {
                let d = &mut p.doors[door as usize];
                if d.opened == 0 && d.wait_open == 0 && d.closed == 0 {
                    d.opened = now;
                } else if d.wait_open != 0 || d.closed != 0 {
                    // A closing door goes straight back to waiting open.
                    d.wait_open = now;
                }
            }
            if p.complete_frame == 0 {
                p.complete_frame = now;
            }
            if !animated || p.doors[door as usize].wait_open != 0 {
                p.queue.front_mut().unwrap().door = None;
                let production = self.objects[&id].production.as_ref().unwrap().queue.front().unwrap().id;
                match self.exit_object(id, &unit) {
                    Ok(unit) => self.script_produced_unit(id, production, unit),
                    Err(err) => self.events.push(format!("frame {now}: {err}")),
                }
                let p = self.objects.get_mut(&id).unwrap().production.as_mut().unwrap();
                p.queue.front_mut().unwrap().produced += 1;
            }
        }
        let p = self.objects.get_mut(&id).unwrap().production.as_mut().unwrap();
        if p.queue.front().is_some_and(|e| e.produced >= e.total) {
            p.queue.pop_front();
        }
        1
    }

    /// Creates a produced unit and sends it out (`exitObjectViaDoor`).
    pub(crate) fn exit_object(&mut self, factory: u32, unit: &str) -> Result<u32, SimError> {
        let f = &self.objects[&factory];
        let (position, angle, layer, owner) = (f.position, f.angle, f.layer, f.owner);
        let e = f.exit.clone().unwrap();
        let mut loc = transform(position, angle, e.create_point);
        if e.kind == ExitKind::Queue {
            let ground = self.map.ground_height(loc[0], loc[1]);
            if loc[2] != ground && !e.allow_airborne {
                loc[2] = ground;
            }
        } else if e.kind == ExitKind::SupplyCenter {
            loc[2] = self.map.ground_height(loc[0], loc[1]);
        } else {
            loc[2] = self.grid.height_at(&self.map, Waypoint { position: [loc[0], loc[1]], layer });
        }
        // The default exits keep the factory's layer; the queue exit leaves the ground layer.
        let layer = if e.kind == ExitKind::Queue { 0 } else { layer };
        let new = self.create_object(unit, loc, angle, layer, owner)?;
        let n = transform(position, angle, e.natural_rally);
        let mut first = [n[0], n[1]];
        if e.kind == ExitKind::Queue {
            // Pathfinder::snapPosition
            let (_, center) = self.footprint(new);
            let mut p = first;
            if !center {
                p[0] += CELL / 2.0;
                p[1] += CELL / 2.0;
            }
            first = coord_of_cell((p[0] / CELL).floor() as i32, (p[1] / CELL).floor() as i32, center);
        }
        let mut rest = VecDeque::new();
        if e.kind == ExitKind::SupplyCenter {
            rest.extend(e.rally);
        } else if let Some(mut r) = e.rally {
            let layer = self.grid.destination(&self.map, r).layer;
            if self.ground_movement(new) && self.adjust_destination(new, &mut r, layer) {
                rest.push_back(r);
            }
        } else if e.kind == ExitKind::Queue {
            rest.push_back(first);
        }
        if self.has_ai(new) {
            // aiFollowExitProductionPath(path, factory, CMD_FROM_AI)
            match self.begin_move_ignoring(new, first, AiState::FollowPath, rest, Some(factory)) {
                Ok(()) => {
                    let o = self.objects.get_mut(&new).unwrap();
                    o.ai.cmd_from_ai = true;
                    if e.kind == ExitKind::SupplyCenter
                        && let Some(t) = o.truck.as_mut()
                    {
                        t.force_wanting = true;
                    }
                }
                Err(err) => self.events.push(format!("frame {}: produced {new} cannot exit: {err}", self.frame)),
            }
        }
        if let Some(e) = self.objects.get_mut(&factory).and_then(|o| o.exit.as_mut())
            && e.kind == ExitKind::Queue
        {
            e.current_delay = e.exit_delay;
            e.burst = e.burst.saturating_sub(1);
        }
        Ok(new)
    }
}
