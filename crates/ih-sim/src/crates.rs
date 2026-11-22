//! Crates and radar.
//!
//! Sources: `GameLogic/System/CrateSystem.cpp` (`CrateTemplate`),
//! `Die/CreateCrateDie.cpp`, `Collide/CrateCollide/CrateCollide.cpp`
//! (`isValidToExecute`, `onCollide`), `MoneyCrateCollide.cpp`,
//! `HealCrateCollide.cpp`, `UnitCrateCollide.cpp`, `VeterancyCrateCollide.cpp`,
//! `ExperienceTracker::gainExpForLevel`, `Upgrade/RadarUpgrade.cpp` and
//! `Player::addRadar`/`removeRadar`/`hasRadar`.
//!
//! Not ported yet: salvage, shroud, sabotage and car-bomb crates, the
//! computer player's crate hunting (`notifyCrate`), parachutes, execute FX
//! and animations, and radar's power-outage edge (`onDisabledEdge`) and
//! script-disabled radar.
use crate::Simulation;

/// `CrateTemplate`.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct CrateTemplate {
    pub chance: f32,
    pub veterancy: Option<u8>,
    pub killed_by: Vec<String>,
    pub killer_science: Option<String>,
    pub owned_by_maker: bool,
    /// `CrateObject`: (template, chance).
    pub objects: Vec<(String, f32)>,
}

/// The `CrateData` definitions.
pub fn load(defs: &ih_ini::definitions::Definitions) -> std::collections::BTreeMap<String, CrateTemplate> {
    let mut out = std::collections::BTreeMap::new();
    for name in defs.names("CrateData") {
        let mut c = CrateTemplate::default();
        for b in defs.all("CrateData", name) {
            for f in b.fields() {
                let v = f.values();
                let first = v.first().cloned().unwrap_or_default();
                let real = |s: &str| ih_ini::values::scan_real(s.as_bytes()).unwrap_or(0.0);
                match f.name.as_str() {
                    "CreationChance" => c.chance = real(&first),
                    "VeterancyLevel" => {
                        c.veterancy = crate::veterancy::LEVELS
                            .iter()
                            .position(|l| l.eq_ignore_ascii_case(&first))
                            .map(|i| i as u8)
                    }
                    "KilledByType" => c.killed_by = v.iter().filter(|k| *k != "NONE").cloned().collect(),
                    "KillerScience" => c.killer_science = (!first.eq_ignore_ascii_case("NONE")).then_some(first),
                    "OwnedByMaker" => c.owned_by_maker = first.eq_ignore_ascii_case("yes"),
                    "CrateObject" => {
                        if let Some(p) = v.get(1) {
                            c.objects.push((first, real(p)));
                        }
                    }
                    _ => {}
                }
            }
        }
        out.insert(name.clone(), c);
    }
    out
}

/// What a crate does when picked up.
#[derive(Clone, Debug, PartialEq)]
pub enum CrateKind {
    /// `MoneyCrateCollide`: money, plus a boost per completed upgrade.
    Money { amount: u32, boosts: Vec<(String, u32)> },
    /// `HealCrateCollide`: heals all the player's objects.
    Heal,
    /// `UnitCrateCollide`: units of a template.
    Unit { template: String, count: u32 },
    /// `VeterancyCrateCollide`.
    Veterancy { range: f32, adds_owner_veterancy: bool, pilot: bool },
}

/// `CrateCollideModuleData` with its kind.
#[derive(Clone, Debug, PartialEq)]
pub struct CrateCollide {
    pub kind: CrateKind,
    pub required: Vec<String>,
    pub forbidden: Vec<String>,
    pub forbid_owner: bool,
    pub building_pickup: bool,
    pub pickup_science: Option<String>,
}

impl CrateCollide {
    pub fn from_module(class: &str, b: &ih_ini::Block) -> Option<Self> {
        let mut kind = match class {
            "MoneyCrateCollide" => CrateKind::Money { amount: 0, boosts: vec![] },
            "HealCrateCollide" => CrateKind::Heal,
            "UnitCrateCollide" => CrateKind::Unit { template: String::new(), count: 0 },
            "VeterancyCrateCollide" => CrateKind::Veterancy { range: 0.0, adds_owner_veterancy: false, pilot: false },
            _ => return None,
        };
        let mut c = CrateCollide {
            kind: CrateKind::Heal,
            required: vec![],
            forbidden: vec![],
            forbid_owner: false,
            building_pickup: false,
            pickup_science: None,
        };
        for f in b.fields() {
            let v = f.values();
            let first = v.first().cloned().unwrap_or_default();
            let yes = first.eq_ignore_ascii_case("yes");
            let int = |s: &str| ih_ini::values::scan_int(s.as_bytes()).unwrap_or(0).max(0) as u32;
            match (f.name.as_str(), &mut kind) {
                ("RequiredKindOf", _) => c.required = v.iter().filter(|k| *k != "NONE").cloned().collect(),
                ("ForbiddenKindOf", _) => c.forbidden = v.iter().filter(|k| *k != "NONE").cloned().collect(),
                ("ForbidOwnerPlayer", _) => c.forbid_owner = yes,
                ("BuildingPickup", _) => c.building_pickup = yes,
                ("PickupScience", _) => c.pickup_science = (!first.eq_ignore_ascii_case("NONE")).then_some(first),
                ("MoneyProvided", CrateKind::Money { amount, .. }) => *amount = int(&first),
                // UpgradedBoost = UpgradeType:Upgrade_X Boost:N
                ("UpgradedBoost", CrateKind::Money { boosts, .. }) => {
                    let value = |key: &str| {
                        v.iter().find_map(|t| {
                            t.split_once(':').filter(|(k, _)| k.eq_ignore_ascii_case(key)).map(|(_, x)| x.to_string())
                        })
                    };
                    if let (Some(u), Some(n)) = (value("UpgradeType"), value("Boost")) {
                        boosts.push((u, int(&n)));
                    }
                }
                ("UnitType", CrateKind::Unit { template, .. }) => *template = first,
                ("UnitCount", CrateKind::Unit { count, .. }) => *count = int(&first),
                ("EffectRange", CrateKind::Veterancy { range, .. }) => {
                    *range = ih_ini::values::scan_real(first.as_bytes()).unwrap_or(0.0)
                }
                ("AddsOwnerVeterancy", CrateKind::Veterancy { adds_owner_veterancy, .. }) => {
                    *adds_owner_veterancy = yes
                }
                ("IsPilot", CrateKind::Veterancy { pilot, .. }) => *pilot = yes,
                _ => {}
            }
        }
        c.kind = kind;
        Some(c)
    }
}

impl Simulation {
    /// `CreateCrateDie::onDie`.
    pub(crate) fn create_crate_die(&mut self, id: u32, crates: &[String], killer: u32) {
        let killer = self.objects.get(&killer).map(|k| k.id);
        if killer.is_some_and(|k| self.relationship(k, id) == crate::weapons::Relationship::Allies) {
            return;
        }
        for name in crates {
            let Some(c) = self.templates.crates.get(name).cloned() else { continue };
            // The chance is always drawn.
            if self.rng.real(0.0, 1.0) >= c.chance {
                continue;
            }
            if c.veterancy.is_some_and(|l| l != self.objects[&id].experience.level) {
                continue;
            }
            if !c.killed_by.is_empty() {
                let ok = killer.is_some_and(|k| {
                    let t = &self.templates.objects[&self.objects[&k].template];
                    c.killed_by.iter().all(|kind| t.has_kind(kind))
                });
                if !ok {
                    continue;
                }
            }
            if let Some(s) = &c.killer_science {
                let owner = killer.map(|k| self.objects[&k].owner);
                if !owner.is_some_and(|o| self.player(o).sciences.contains(s)) {
                    continue;
                }
            }
            let Some(made) = self.create_crate(id, &c) else { continue };
            if c.owned_by_maker {
                let owner = self.objects[&id].owner;
                self.objects.get_mut(&made).unwrap().owner = owner;
            }
        }
    }

    /// `CreateCrateDie::createCrate`: crates are neutral.
    fn create_crate(&mut self, id: u32, c: &CrateTemplate) -> Option<u32> {
        let pick = self.rng.real(0.0, 1.0);
        let mut total = 0.0;
        let name = c.objects.iter().find(|(_, chance)| {
            total += chance;
            total > pick
        })?;
        let name = name.0.clone();
        if !self.templates.objects.contains_key(&name) {
            return None;
        }
        let o = &self.objects[&id];
        let (center, layer) = (o.position, o.layer);
        let spot = if layer != 0 {
            Some(center)
        } else {
            self.find_position_around(center, 0.0, 5.0, Some(id), None, f32::MAX)
                .or_else(|| self.find_position_around(center, 0.0, 125.0, None, None, f32::MAX))
        }?;
        let crate_id = self.create_object(&name, spot, 0.0, 0, 0).ok()?;
        let angle = self.rng.real(0.0, ih_math::TAU);
        let o = self.objects.get_mut(&crate_id).unwrap();
        o.angle = angle;
        o.layer = layer;
        Some(crate_id)
    }

    /// `CrateCollide::onCollide`: the crate `id` touched `other`.
    pub(crate) fn crate_collide(&mut self, id: u32, other: u32) {
        let Some(c) = self.objects.get(&id).and_then(|o| o.crate_collide.clone()) else { return };
        if self.objects[&id].dead || self.objects[&id].status & crate::body::status::DESTROYED != 0 {
            return;
        }
        if !self.crate_valid(id, other, &c) {
            return;
        }
        if self.execute_crate(id, other, &c) {
            self.destroy_object(id);
        }
    }

    /// `CrateCollide::isValidToExecute` (and the veterancy crate's extra checks).
    fn crate_valid(&mut self, id: u32, other: u32, c: &CrateCollide) -> bool {
        let Some(o) = self.objects.get(&other) else { return false };
        if o.owner == 0 {
            return false;
        }
        let t = &self.templates.objects[&o.template];
        let building = c.building_pickup && t.has_kind("STRUCTURE");
        if !self.has_ai(other) && !building {
            return false;
        }
        if !c.required.iter().all(|k| t.has_kind(k)) || c.forbidden.iter().any(|k| t.has_kind(k)) {
            return false;
        }
        if o.dead {
            return false;
        }
        let me = &self.objects[&id];
        let ground = self.map.ground_height(me.position[0], me.position[1]);
        if me.position[2] > ground + 0.5 && !building {
            return false;
        }
        if c.forbid_owner && me.owner == o.owner {
            return false;
        }
        let owner = o.owner;
        if let Some(s) = &c.pickup_science
            && !self.players.get(&owner).is_some_and(|p| p.sciences.contains(s))
        {
            return false;
        }
        let o = &self.objects[&other];
        if t.has_kind("PARACHUTE") {
            return false;
        }
        if let CrateKind::Veterancy { pilot, .. } = c.kind {
            let ground = self.map.ground_height(o.position[0], o.position[1]);
            if o.position[2] > ground + 10.0 {
                return false;
            }
            let levels = self.crate_levels(id, &c.kind);
            if levels <= 0 || !self.can_gain_levels(other, levels) {
                return false;
            }
            if pilot && self.objects[&id].owner != self.objects[&other].owner {
                return false;
            }
        }
        true
    }

    fn crate_levels(&self, id: u32, kind: &CrateKind) -> i32 {
        match kind {
            CrateKind::Veterancy { adds_owner_veterancy: true, .. } => i32::from(self.objects[&id].experience.level),
            _ => 1,
        }
    }

    /// `ExperienceTracker::canGainExpForLevel` (trainable, and room to grow).
    fn can_gain_levels(&self, id: u32, levels: i32) -> bool {
        let o = &self.objects[&id];
        let trainable = self.templates.objects[&o.template]
            .values("IsTrainable")
            .first()
            .is_some_and(|v| v.eq_ignore_ascii_case("yes"));
        trainable && i32::from(o.experience.level) + levels <= 3
    }

    /// `ExperienceTracker::gainExpForLevel`.
    fn gain_levels(&mut self, id: u32, levels: i32, scale: bool) {
        if !self.can_gain_levels(id, levels) {
            return;
        }
        let o = &self.objects[&id];
        let target = (i32::from(o.experience.level) + levels) as usize;
        let need = self.templates.objects[&o.template]
            .values("ExperienceRequired")
            .get(target)
            .and_then(|s| ih_ini::values::scan_int(s.as_bytes()).ok())
            .unwrap_or(0);
        let gain = need - o.experience.points;
        self.add_experience(id, gain, scale);
    }

    /// `executeCrateBehavior`: true when the crate is used up.
    fn execute_crate(&mut self, id: u32, other: u32, c: &CrateCollide) -> bool {
        let owner = self.objects[&other].owner;
        match &c.kind {
            CrateKind::Money { amount, boosts } => {
                let boost = boosts
                    .iter()
                    .find(|(u, _)| self.player(owner).upgrades.completed.contains(u))
                    .map_or(0, |(_, b)| *b);
                self.player(owner).deposit(amount + boost);
                true
            }
            CrateKind::Heal => {
                // Player::healAllObjects
                let ids: Vec<u32> = self.objects.values().filter(|o| o.owner == owner).map(|o| o.id).collect();
                for i in ids {
                    let info = crate::body::DamageInfo::new(
                        0,
                        crate::body::DAMAGE_HEALING,
                        crate::body::DEATH_NORMAL,
                        crate::body::HUGE_DAMAGE_AMOUNT,
                    );
                    self.attempt_damage(i, info);
                }
                true
            }
            CrateKind::Unit { template, count } => {
                if !self.templates.objects.contains_key(template) {
                    return false;
                }
                let (pos, angle) = (self.objects[&other].position, self.objects[&other].angle);
                for _ in 0..*count {
                    let at = self.find_position_around(pos, 0.0, 20.0, None, None, f32::MAX).unwrap_or(pos);
                    if let Ok(n) = self.create_object(template, at, angle, 0, owner) {
                        let _ = n;
                    }
                }
                true
            }
            CrateKind::Veterancy { range, pilot, .. } => {
                // Only a crate (a pilot) whose AI is trying to reach this object.
                let goal = self.objects[&id].ai.enter;
                if !self.has_ai(id) || goal != Some(other) {
                    return false;
                }
                let levels = self.crate_levels(id, &c.kind);
                if *range == 0.0 {
                    self.gain_levels(other, levels, !pilot);
                } else {
                    let p = self.objects[&other].position;
                    let near: Vec<u32> = self
                        .objects
                        .values()
                        .filter(|o| {
                            o.owner == owner && o.contained_by.is_none() && {
                                let d = [o.position[0] - p[0], o.position[1] - p[1]];
                                d[0] * d[0] + d[1] * d[1] <= range * range
                            }
                        })
                        .map(|o| o.id)
                        .collect();
                    for n in near {
                        self.gain_levels(n, levels, !pilot);
                    }
                }
                true
            }
        }
    }

    /// `Player::hasRadar`.
    pub fn has_radar(&self, player: u32) -> bool {
        self.players.get(&player).is_some_and(|p| p.radar.0 > 0)
    }
    /// `Player::addRadar` / `removeRadar`.
    pub(crate) fn change_radar(&mut self, player: u32, disable_proof: bool, add: bool) {
        let r = &mut self.player(player).radar;
        let d = if add { 1 } else { -1 };
        r.0 += d;
        if disable_proof {
            r.1 += d;
        }
    }
}
