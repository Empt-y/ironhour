//! Upgrades: templates, player and object upgrades, upgrade modules, and
//! researching them in a production queue.
//!
//! Sources: `Common/System/Upgrade.cpp` (`UpgradeTemplate`, `UpgradeCenter`),
//! `Common/Thing/Module.cpp` (`UpgradeMuxData`), `GameLogic/Object/Upgrade/UpgradeModule.cpp`
//! (`UpgradeMux`), `Object::updateUpgradeModules`/`giveUpgrade`/`removeUpgrade`/
//! `affectedByUpgrade`/`canProduceUpgrade`, `Player::addUpgrade`/`removeUpgrade`/
//! `onUpgradeCompleted`/`addKindOfProductionCostChange`, `ProductionUpdate::queueUpgrade`/
//! `cancelUpgrade` and the upgrade half of its update, and the upgrade modules
//! `WeaponSetUpgrade`, `ArmorUpgrade`, `MaxHealthUpgrade`, `CommandSetUpgrade`,
//! `StatusBitsUpgrade`, `PowerPlantUpgrade`, `CostModifierUpgrade`,
//! `GrantScienceUpgrade` and `WeaponBonusUpgrade`.
//!
//! Every other upgrade module (locomotor sets, object creation, replacement,
//! sub-objects, model conditions, radar, stealth, special powers, shroud,
//! experience, passengers firing) records that it ran and has no effect yet.
//! Weapon bonuses are recorded but not applied; FX lists do not play;
//! veterancy upgrades do not exist yet.
use crate::{SimError, Simulation};
use std::collections::BTreeSet;

/// `UpgradeTemplate`.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct UpgradeTemplate {
    pub name: String,
    /// `UPGRADE_TYPE_OBJECT` (else `PLAYER`).
    pub object: bool,
    pub build_time: f32,
    pub cost: i32,
}
impl UpgradeTemplate {
    /// Parsed over a copy of `DefaultUpgrade` (`UpgradeCenter::newUpgrade`).
    pub fn parse(name: &str, base: Option<&UpgradeTemplate>, blocks: &[&ih_ini::Block]) -> Self {
        let mut u = base.cloned().unwrap_or_default();
        u.name = name.into();
        for b in blocks {
            for f in b.fields() {
                let v = f.values();
                let first = v.first().map(String::as_str).unwrap_or("");
                match f.name.as_str() {
                    "Type" => u.object = first.eq_ignore_ascii_case("OBJECT"),
                    "BuildTime" => u.build_time = ih_ini::values::scan_real(first.as_bytes()).unwrap_or(0.0),
                    "BuildCost" => u.cost = ih_ini::values::scan_int(first.as_bytes()).unwrap_or(0),
                    _ => {}
                }
            }
        }
        u
    }
    /// `calcTimeToBuild` (no power penalty for research).
    pub fn time_to_build(&self) -> i32 {
        (self.build_time * 30.0) as i32
    }
    /// `calcCostToBuild`.
    pub fn cost_to_build(&self) -> u32 {
        self.cost.max(0) as u32
    }
}

/// What an upgrade module does when it runs.
#[derive(Clone, Debug, PartialEq)]
pub enum Effect {
    WeaponSet,
    Armor,
    MaxHealth {
        add: f32,
        change: HealthChange,
    },
    CommandSet {
        set: String,
        alt: String,
        trigger_alt: String,
    },
    StatusBits {
        set: u64,
        clear: u64,
    },
    PowerPlant,
    CostModifier {
        kinds: Vec<String>,
        percent: f32,
    },
    GrantScience(String),
    WeaponBonus,
    /// Not ported yet; the module still counts as executed.
    Other,
}

/// `MaxHealthChangeType`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum HealthChange {
    SameCurrent,
    PreserveRatio,
    AddCurrentToo,
    FullyHeal,
}

/// An upgrade module's `UpgradeMuxData` and effect.
#[derive(Clone, Debug, PartialEq)]
pub struct UpgradeModuleDef {
    pub class: String,
    pub triggers: Vec<String>,
    pub conflicts: Vec<String>,
    pub removes: Vec<String>,
    pub requires_all: bool,
    pub effect: Effect,
}
impl UpgradeModuleDef {
    /// Modules whose data includes `UpgradeMuxData`.
    pub fn from_module(class: &str, b: &ih_ini::Block) -> Option<Self> {
        let known = ih_ini::schema::MODULES.iter().find(|d| d.name == class)?;
        if !known.tables.iter().any(|t| t.contains("UpgradeMuxData")) {
            return None;
        }
        let mut d = Self {
            class: class.into(),
            triggers: vec![],
            conflicts: vec![],
            removes: vec![],
            requires_all: false,
            effect: Effect::Other,
        };
        let status_bits = |v: &[String]| {
            let list = ih_ini::schema::schema().name_list("ObjectStatusMaskType::s_bitNameList").unwrap_or(&[]);
            v.iter()
                .filter_map(|n| list.iter().position(|l| l.eq_ignore_ascii_case(n)))
                .fold(0u64, |m, i| m | (1u64 << i))
        };
        let (mut add, mut change) = (0.0, HealthChange::SameCurrent);
        let (mut set, mut alt, mut trigger_alt) = (String::new(), String::new(), String::new());
        let (mut to_set, mut to_clear) = (0u64, 0u64);
        let (mut kinds, mut percent) = (vec![], 0.0);
        let mut science = String::new();
        for f in b.fields() {
            let v = f.values();
            let first = v.first().cloned().unwrap_or_default();
            match f.name.as_str() {
                "TriggeredBy" => d.triggers.extend(v),
                "ConflictsWith" => d.conflicts.extend(v),
                "RemovesUpgrades" => d.removes.extend(v),
                "RequiresAllTriggers" => d.requires_all = first.eq_ignore_ascii_case("yes"),
                "AddMaxHealth" => add = ih_ini::values::scan_real(first.as_bytes()).unwrap_or(0.0),
                "ChangeType" => {
                    change = match first.to_ascii_uppercase().as_str() {
                        "PRESERVE_RATIO" => HealthChange::PreserveRatio,
                        "ADD_CURRENT_HEALTH_TOO" => HealthChange::AddCurrentToo,
                        "FULLY_HEAL" => HealthChange::FullyHeal,
                        _ => HealthChange::SameCurrent,
                    }
                }
                "CommandSet" => set = first,
                "CommandSetAlt" => alt = first,
                "TriggerAlt" => trigger_alt = first,
                "StatusToSet" => to_set = status_bits(&v),
                "StatusToClear" => to_clear = status_bits(&v),
                "EffectKindOf" => kinds = v,
                // INI::parsePercentToReal
                "Percentage" => {
                    percent = ih_ini::values::scan_real(first.trim_end_matches('%').as_bytes()).unwrap_or(0.0) / 100.0
                }
                "GrantScience" => science = first,
                _ => {}
            }
        }
        d.effect = match class {
            "WeaponSetUpgrade" => Effect::WeaponSet,
            "ArmorUpgrade" => Effect::Armor,
            "MaxHealthUpgrade" => Effect::MaxHealth { add, change },
            "CommandSetUpgrade" => Effect::CommandSet { set, alt, trigger_alt },
            "StatusBitsUpgrade" => Effect::StatusBits { set: to_set, clear: to_clear },
            "PowerPlantUpgrade" => Effect::PowerPlant,
            "CostModifierUpgrade" => Effect::CostModifier { kinds, percent },
            "GrantScienceUpgrade" => Effect::GrantScience(science),
            "WeaponBonusUpgrade" => Effect::WeaponBonus,
            _ => Effect::Other,
        };
        Some(d)
    }
}

/// An object's upgrade state.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct ObjectUpgrades {
    /// `m_objectUpgradesCompleted`.
    pub completed: BTreeSet<String>,
    /// The object's upgrade modules, in module order.
    pub modules: Vec<UpgradeModuleDef>,
    /// `m_upgradeExecuted` per module.
    pub executed: Vec<bool>,
    /// `WEAPONSET_PLAYER_UPGRADE` and other weapon set flags.
    pub weapon_set_flags: u32,
    /// `WEAPONBONUSCONDITION_PLAYER_UPGRADE` (not applied yet).
    pub weapon_bonus: bool,
    /// `setCommandSetStringOverride`.
    pub command_set: Option<String>,
}

/// A player's upgrades and the cost changes they bring.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct PlayerUpgrades {
    pub in_progress: BTreeSet<String>,
    pub completed: BTreeSet<String>,
    /// `m_kindOfPercentProductionChangeList`: (kinds, percent, references).
    pub cost_changes: Vec<(Vec<String>, f32, u32)>,
}

fn flag(list: &str, name: &str) -> u32 {
    ih_ini::schema::schema()
        .name_list(list)
        .unwrap_or(&[])
        .iter()
        .position(|n| n.eq_ignore_ascii_case(name))
        .map_or(0, |i| 1 << i)
}

impl Simulation {
    fn upgrade(&self, name: &str) -> Option<&UpgradeTemplate> {
        self.templates.upgrades.get(name)
    }
    /// The command set in use (`getCommandSetString`).
    pub fn command_set_of(&self, id: u32) -> Option<String> {
        let o = self.objects.get(&id)?;
        o.upgrades
            .command_set
            .clone()
            .or_else(|| self.templates.objects[&o.template].values("CommandSet").first().cloned())
    }
    /// `getProductionCostChangeBasedOnKindOf`.
    pub(crate) fn cost_change(&self, player: u32, t: &crate::templates::Template) -> f32 {
        let mut start = 1.0f32;
        if let Some(p) = self.players.get(&player) {
            for (kinds, percent, _) in &p.upgrades.cost_changes {
                if kinds.iter().all(|k| t.has_kind(k)) {
                    start *= 1.0 + percent;
                }
            }
        }
        start
    }
    /// `EnergyBonus` of the player's objects whose power plant upgrade ran.
    pub(crate) fn power_bonus(&self, o: &crate::simulation::Object) -> i32 {
        let ran =
            o.upgrades.modules.iter().zip(&o.upgrades.executed).any(|(m, e)| *e && m.effect == Effect::PowerPlant);
        if ran { self.templates.objects[&o.template].number("EnergyBonus", 0.0) as i32 } else { 0 }
    }

    fn mask_of(&self, id: u32) -> BTreeSet<String> {
        let o = &self.objects[&id];
        let mut mask = self.players.get(&o.owner).map(|p| p.upgrades.completed.clone()).unwrap_or_default();
        mask.extend(o.upgrades.completed.iter().cloned());
        mask
    }
    /// Activation names that are real upgrades (others never match).
    fn known<'a>(&'a self, names: &'a [String]) -> impl Iterator<Item = &'a String> + 'a {
        names.iter().filter(|n| self.templates.upgrades.contains_key(*n))
    }
    /// `UpgradeMux::wouldUpgrade`.
    fn would_upgrade(&self, m: &UpgradeModuleDef, executed: bool, mask: &BTreeSet<String>) -> bool {
        let activation: Vec<&String> = self.known(&m.triggers).collect();
        if activation.is_empty() || mask.is_empty() || executed {
            return false;
        }
        if self.known(&m.conflicts).any(|c| mask.contains(c)) {
            return false;
        }
        if m.requires_all {
            activation.iter().all(|a| mask.contains(*a))
        } else {
            activation.iter().any(|a| mask.contains(*a))
        }
    }

    /// `Object::updateUpgradeModules`.
    pub(crate) fn update_upgrade_modules(&mut self, id: u32) {
        let Some(o) = self.objects.get(&id) else { return };
        if o.status & crate::body::status::DESTROYED != 0 {
            return;
        }
        for i in 0..o.upgrades.modules.len() {
            let mask = self.mask_of(id);
            let o = &self.objects[&id];
            if o.upgrades.executed[i] {
                continue;
            }
            let m = o.upgrades.modules[i].clone();
            if self.would_upgrade(&m, false, &mask) {
                // giveSelfUpgrade: removals first, then the upgrade itself.
                for r in m.removes.clone() {
                    if self.templates.upgrades.contains_key(&r) {
                        self.remove_object_upgrade(id, &r);
                    }
                }
                self.upgrade_implementation(id, &m);
                self.objects.get_mut(&id).unwrap().upgrades.executed[i] = true;
            }
        }
    }
    /// `Object::removeUpgrade`: modules it triggered may run again (their effect stays).
    fn remove_object_upgrade(&mut self, id: u32, name: &str) {
        let o = self.objects.get_mut(&id).unwrap();
        o.upgrades.completed.remove(name);
        for i in 0..o.upgrades.modules.len() {
            if o.upgrades.executed[i] && o.upgrades.modules[i].triggers.iter().any(|t| t == name) {
                o.upgrades.executed[i] = false;
            }
        }
    }
    /// `Object::giveUpgrade`.
    pub fn give_object_upgrade(&mut self, id: u32, name: &str) {
        if let Some(o) = self.objects.get_mut(&id) {
            o.upgrades.completed.insert(name.into());
            self.update_upgrade_modules(id);
        }
    }
    /// `Player::addUpgrade(UPGRADE_STATUS_COMPLETE)` and `onUpgradeCompleted`.
    pub fn complete_player_upgrade(&mut self, player: u32, name: &str) {
        let p = &mut self.player(player).upgrades;
        p.in_progress.remove(name);
        p.completed.insert(name.into());
        let ids: Vec<u32> = self.objects.values().filter(|o| o.owner == player).map(|o| o.id).collect();
        for id in ids {
            self.update_upgrade_modules(id);
        }
    }

    fn upgrade_implementation(&mut self, id: u32, m: &UpgradeModuleDef) {
        match &m.effect {
            Effect::WeaponSet => {
                self.objects.get_mut(&id).unwrap().upgrades.weapon_set_flags |=
                    flag("WeaponSetFlags::s_bitNameList", "PLAYER_UPGRADE");
                self.update_weapon_set(id);
            }
            Effect::Armor => {
                if let Some(h) = self.objects.get_mut(&id).unwrap().health.as_mut() {
                    h.armor_flags |= flag("ArmorSetFlags::s_bitNameList", "PLAYER_UPGRADE");
                }
            }
            Effect::MaxHealth { add, change } => self.set_max_health(id, *add, *change),
            Effect::CommandSet { set, alt, trigger_alt } => {
                let o = &self.objects[&id];
                let has = |n: &str| {
                    self.players.get(&o.owner).is_some_and(|p| p.upgrades.completed.contains(n))
                        || o.upgrades.completed.contains(n)
                };
                let chosen = if self.templates.upgrades.contains_key(trigger_alt) && has(trigger_alt) {
                    alt.clone()
                } else {
                    set.clone()
                };
                self.objects.get_mut(&id).unwrap().upgrades.command_set = Some(chosen);
            }
            Effect::StatusBits { set, clear } => {
                let o = self.objects.get_mut(&id).unwrap();
                o.status |= set;
                o.status &= !clear;
            }
            Effect::CostModifier { kinds, percent } => {
                let owner = self.objects[&id].owner;
                let list = &mut self.player(owner).upgrades.cost_changes;
                if let Some(e) = list.iter_mut().find(|(k, p, _)| k == kinds && p == percent) {
                    e.2 += 1;
                } else {
                    list.push((kinds.clone(), *percent, 1));
                }
            }
            Effect::GrantScience(s) => {
                let owner = self.objects[&id].owner;
                self.player(owner).sciences.insert(s.clone());
            }
            Effect::WeaponBonus => self.objects.get_mut(&id).unwrap().upgrades.weapon_bonus = true,
            // The power bonus is read from the executed module (`power_bonus`).
            Effect::PowerPlant | Effect::Other => {}
        }
    }

    /// `ActiveBody::setMaxHealth` by the upgrade's amount.
    fn set_max_health(&mut self, id: u32, add: f32, change: HealthChange) {
        let Some(h) = self.objects.get_mut(&id).unwrap().health.as_mut() else { return };
        let prev = h.max;
        let max = prev + add;
        h.max = max;
        h.initial = max;
        let delta = match change {
            HealthChange::PreserveRatio if prev != 0.0 => max * (h.current / prev) - h.current,
            HealthChange::AddCurrentToo => max - prev,
            HealthChange::FullyHeal => max - h.current,
            _ => return,
        };
        self.change_health(id, delta);
    }

    /// `onDelete` of cost modifier upgrades (power bonuses go with the object).
    pub(crate) fn upgrades_on_delete(&mut self, id: u32) {
        let Some(o) = self.objects.get(&id) else { return };
        let owner = o.owner;
        let changes: Vec<(Vec<String>, f32)> = o
            .upgrades
            .modules
            .iter()
            .zip(&o.upgrades.executed)
            .filter_map(|(m, e)| match &m.effect {
                Effect::CostModifier { kinds, percent } if *e => Some((kinds.clone(), *percent)),
                _ => None,
            })
            .collect();
        for (kinds, percent) in changes {
            let list = &mut self.player(owner).upgrades.cost_changes;
            if let Some(i) = list.iter().position(|(k, p, _)| *k == kinds && *p == percent) {
                list[i].2 -= 1;
                if list[i].2 == 0 {
                    list.remove(i);
                }
            }
        }
    }

    /// `Object::affectedByUpgrade`.
    fn affected_by_upgrade(&self, id: u32, name: &str) -> bool {
        let mut mask = self.mask_of(id);
        mask.insert(name.into());
        let o = &self.objects[&id];
        o.upgrades.modules.iter().zip(&o.upgrades.executed).any(|(m, e)| self.would_upgrade(m, *e, &mask))
    }
    /// `Object::canProduceUpgrade`: an upgrade button in the command set.
    fn can_produce_upgrade(&self, id: u32, name: &str) -> bool {
        let Some(set) = self.command_set_of(id).and_then(|s| self.templates.command_sets.get(&s)) else {
            return false;
        };
        set.iter().any(|b| self.templates.command_button_upgrades.get(b).is_some_and(|u| u == name))
    }

    /// `ProductionUpdate::queueUpgrade`.
    pub(crate) fn queue_upgrade(&mut self, factory: u32, name: &str) -> Result<(), SimError> {
        let fail = |why: &str| Err(SimError(format!("object {factory} cannot research {name}: {why}")));
        let Some(u) = self.upgrade(name).cloned() else { return fail("no such upgrade") };
        let Some(f) = self.objects.get(&factory) else { return fail("missing") };
        let Some(p) = f.production.as_ref() else { return fail("no production") };
        let owner = f.owner;
        if !u.object && self.money(owner) < u.cost_to_build() {
            return fail("NoMoney");
        }
        if u.object && (f.upgrades.completed.contains(name) || !self.affected_by_upgrade(factory, name)) {
            return fail("not applicable");
        }
        if p.queue.iter().any(|e| e.upgrade && e.template == name) {
            return fail("already queued");
        }
        if !self.can_produce_upgrade(factory, name) {
            return fail("NoPrereq");
        }
        let pu = self.players.get(&owner).map(|p| &p.upgrades);
        if !u.object && pu.is_some_and(|p| p.completed.contains(name) || p.in_progress.contains(name)) {
            return fail("already researched");
        }
        if p.queue.len() as u32 >= p.data.max_queue {
            return fail("QueueFull");
        }
        self.player(owner).withdraw(u.cost_to_build());
        let p = self.objects.get_mut(&factory).unwrap().production.as_mut().unwrap();
        p.queue.push_back(crate::production::Entry {
            id: 0,
            template: name.into(),
            upgrade: true,
            frames: 0,
            percent: 0.0,
            total: 1,
            produced: 0,
            door: None,
        });
        // Player::addUpgrade(UPGRADE_STATUS_IN_PRODUCTION), for either type.
        self.player(owner).upgrades.in_progress.insert(name.into());
        Ok(())
    }

    /// `ProductionUpdate::cancelUpgrade`.
    pub(crate) fn cancel_upgrade(&mut self, factory: u32, name: &str) -> Result<(), SimError> {
        let Some(u) = self.upgrade(name).cloned() else { return Ok(()) };
        let Some(f) = self.objects.get(&factory) else { return Err(SimError(format!("object {factory} is missing"))) };
        let owner = f.owner;
        if !u.object && !self.players.get(&owner).is_some_and(|p| p.upgrades.in_progress.contains(name)) {
            return Ok(());
        }
        let Some(p) = self.objects.get_mut(&factory).unwrap().production.as_mut() else { return Ok(()) };
        let Some(i) = p.queue.iter().position(|e| e.upgrade && e.template == name) else { return Ok(()) };
        p.queue.remove(i);
        self.player(owner).deposit(u.cost_to_build());
        if !u.object {
            // Player::removeUpgrade
            let p = &mut self.player(owner).upgrades;
            p.in_progress.remove(name);
            p.completed.remove(name);
        }
        Ok(())
    }

    /// The upgrade half of `ProductionUpdate::update` at 100%.
    pub(crate) fn finish_research(&mut self, factory: u32, name: &str) {
        let object = self.upgrade(name).is_some_and(|u| u.object);
        let owner = self.objects[&factory].owner;
        if object {
            self.give_object_upgrade(factory, name);
        } else {
            self.complete_player_upgrade(owner, name);
        }
    }
}
