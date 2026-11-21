//! Bodies, damage and armour, from `ActiveBody.cpp`, `StructureBody.cpp`,
//! `HighlanderBody.cpp`, `ImmortalBody.cpp`, `InactiveBody.cpp`, `Armor.cpp`,
//! `Damage.h`, `Object::attemptDamage`/`attemptHealing`/`kill`/`onDie`,
//! `DieMuxData::isDieApplicable`, `DestroyDie` and `GameLogic::destroyObject`
//! with its end-of-frame `processDestroyList`.
//!
//! Not yet (each waits for the system it touches): damage FX and the body's
//! particle systems (particles), damaged sounds and the fear voice's audio (the
//! logic RNG draw is kept), retaliation (weapons and guard), subdual's
//! container and projectile side effects, killing pilots and garrisons
//! (containers), status damage, veterancy armour sets, structure rubble
//! geometry, and other die and damage modules.
use crate::{Simulation, simulation::ModuleKind, templates::Template};

/// Indexed by `DamageType`.
pub const DAMAGE_TYPES: usize = 38;
pub const DAMAGE_CRUSH: u8 = 1;
pub const DAMAGE_HEALING: u8 = 10;
pub const DAMAGE_UNRESISTABLE: u8 = 11;
pub const DAMAGE_KILLPILOT: u8 = 16;
pub const DAMAGE_SUBDUAL_MISSILE: u8 = 31;
pub const DAMAGE_SUBDUAL_UNRESISTABLE: u8 = 34;
pub const DAMAGE_KILL_GARRISONED: u8 = 36;
pub const DAMAGE_STATUS: u8 = 37;

pub const DEATH_NORMAL: u8 = 0;
pub const DEATH_NONE: u8 = 1;
pub const DEATH_CRUSHED: u8 = 2;

/// `HUGE_DAMAGE_AMOUNT`.
pub const HUGE_DAMAGE_AMOUNT: f32 = 999_999.0;
/// `YELLOW_DAMAGE_PERCENT`.
const YELLOW_DAMAGE_PERCENT: f32 = 0.25;

/// `IsSubdualDamage`.
pub fn is_subdual(t: u8) -> bool {
    (DAMAGE_SUBDUAL_MISSILE..=DAMAGE_SUBDUAL_UNRESISTABLE).contains(&t)
}

fn name_list(id: &str) -> &'static [&'static str] {
    ih_ini::schema::schema().name_list(id).unwrap_or(&[])
}
/// A `DamageType` by name.
pub fn damage_type(name: &str) -> Option<u8> {
    name_list("DamageTypeFlags::s_bitNameList").iter().position(|n| n.eq_ignore_ascii_case(name)).map(|i| i as u8)
}
/// A `DeathType` by name (`TheDeathNames`).
pub fn death_type(name: &str) -> Option<u8> {
    name_list("TheDeathNames").iter().position(|n| n.eq_ignore_ascii_case(name)).map(|i| i as u8)
}

/// `BodyDamageType`.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, PartialOrd, Ord)]
#[repr(u8)]
pub enum DamageState {
    #[default]
    Pristine = 0,
    Damaged = 1,
    ReallyDamaged = 2,
    Rubble = 3,
}

/// `DamageInfo` (`in` and `out`).
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct DamageInfo {
    /// 0 for none (`INVALID_ID`).
    pub source: u32,
    pub damage_type: u8,
    pub death_type: u8,
    pub amount: f32,
    pub kill: bool,
    pub dealt: f32,
    pub clipped: f32,
    pub no_effect: bool,
}
impl DamageInfo {
    pub fn new(source: u32, damage_type: u8, death_type: u8, amount: f32) -> Self {
        Self { source, damage_type, death_type, amount, kill: false, dealt: 0.0, clipped: 0.0, no_effect: false }
    }
}

/// `ArmorTemplate`: a damage coefficient per damage type.
#[derive(Clone, Debug, PartialEq)]
pub struct ArmorTemplate {
    pub coefficients: [f32; DAMAGE_TYPES],
}
impl Default for ArmorTemplate {
    fn default() -> Self {
        Self { coefficients: [1.0; DAMAGE_TYPES] }
    }
}
impl ArmorTemplate {
    /// `ArmorStore::parseArmorDefinition` (`Armor = <type or Default> <percent>`).
    pub fn parse(block: &ih_ini::Block) -> Self {
        let mut a = Self::default();
        for f in block.fields().filter(|f| f.name == "Armor") {
            let v = f.values();
            let (Some(name), Some(pct)) = (v.first(), v.get(1)) else { continue };
            let pct = ih_ini::values::scan_real(pct.trim_end_matches('%').as_bytes()).unwrap_or(100.0) / 100.0;
            if name.eq_ignore_ascii_case("Default") {
                a.coefficients = [pct; DAMAGE_TYPES];
            } else if let Some(t) = damage_type(name) {
                a.coefficients[t as usize] = pct;
            }
        }
        a
    }
    /// `ArmorTemplate::adjustDamage`.
    pub fn adjust(&self, t: u8, damage: f32) -> f32 {
        if t == DAMAGE_UNRESISTABLE || t == DAMAGE_SUBDUAL_UNRESISTABLE {
            return damage;
        }
        (damage * self.coefficients[t as usize]).max(0.0)
    }
}

/// An `ArmorSet` block: `Conditions` bits (`ArmorSetFlags`), the armour and its
/// damage FX.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct ArmorSetDef {
    pub conditions: u32,
    pub armor: Option<String>,
    pub damage_fx: Option<String>,
}
impl ArmorSetDef {
    pub fn parse(block: &ih_ini::Block) -> Self {
        let mut s = Self::default();
        let list = name_list("ArmorSetFlags::s_bitNameList");
        for f in block.fields() {
            let v = f.values();
            match f.name.as_str() {
                "Conditions" => {
                    s.conditions = 0;
                    for name in &v {
                        if let Some(i) = list.iter().position(|n| n.eq_ignore_ascii_case(name)) {
                            s.conditions |= 1 << i;
                        }
                    }
                }
                "Armor" => s.armor = v.first().filter(|n| !n.eq_ignore_ascii_case("None")).cloned(),
                "DamageFX" => s.damage_fx = v.first().filter(|n| !n.eq_ignore_ascii_case("None")).cloned(),
                _ => {}
            }
        }
        s
    }
}
/// `SparseMatchFinder::findBestInfo`: the most matching condition bits, then
/// the fewest extra ones; the first such set wins.
pub fn best_armor_set(sets: &[ArmorSetDef], flags: u32) -> Option<&ArmorSetDef> {
    let mut best = None;
    let (mut best_yes, mut best_extra) = (0, 999);
    for s in sets {
        let yes = (flags & s.conditions).count_ones() as i32;
        let extra = (!flags & s.conditions).count_ones() as i32;
        if yes > best_yes || (yes >= best_yes && extra < best_extra) {
            best = Some(s);
            best_yes = yes;
            best_extra = extra;
        }
    }
    best
}

/// The body module's kind.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[repr(u8)]
pub enum BodyKind {
    Active = 0,
    /// `StructureBody`: an active body (its constructor ID waits for dozers).
    Structure = 1,
    /// `HighlanderBody`: damage other than unresistable leaves one hit point.
    Highlander = 2,
    /// `ImmortalBody`: health never drops below one.
    Immortal = 3,
    /// `InactiveBody`: no health; only unresistable damage, which kills.
    Inactive = 4,
}
impl BodyKind {
    pub fn from_class(class: &str) -> Option<Self> {
        Some(match class {
            "ActiveBody" | "UndeadBody" | "HiveStructureBody" => BodyKind::Active,
            "StructureBody" => BodyKind::Structure,
            "HighlanderBody" => BodyKind::Highlander,
            "ImmortalBody" => BodyKind::Immortal,
            "InactiveBody" => BodyKind::Inactive,
            _ => return None,
        })
    }
}

/// An object's body (`ActiveBody` state).
#[derive(Clone, Debug, PartialEq)]
pub struct Health {
    pub kind: BodyKind,
    pub current: f32,
    pub previous: f32,
    pub max: f32,
    pub initial: f32,
    pub subdual: f32,
    pub subdual_cap: f32,
    pub state: DamageState,
    pub indestructible: bool,
    /// `m_damageScalar`.
    pub scalar: f32,
    /// `m_curArmorSetFlags`.
    pub armor_flags: u32,
    pub last_damage: Option<DamageInfo>,
    /// `0xffffffff` until damaged.
    pub last_damage_frame: u32,
    /// `m_lastDamageCleared` (`getClearableLastAttacker`).
    pub last_damage_cleared: bool,
    pub last_healing_frame: u32,
    /// `InactiveBody::m_dieCalled`.
    pub die_called: bool,
}
impl Health {
    pub fn new(kind: BodyKind, block: &ih_ini::Block, thresholds: (f32, f32)) -> Self {
        let real = |key: &str| {
            block
                .fields()
                .filter(|f| f.name == key)
                .last()
                .and_then(|f| f.values().first().and_then(|v| ih_ini::values::scan_real(v.as_bytes()).ok()))
                .unwrap_or(0.0)
        };
        let (max, initial) = (real("MaxHealth"), real("InitialHealth"));
        let mut h = Self {
            kind,
            current: initial,
            previous: initial,
            max,
            initial,
            subdual: 0.0,
            subdual_cap: real("SubdualDamageCap"),
            state: DamageState::Pristine,
            indestructible: false,
            scalar: 1.0,
            armor_flags: 0,
            last_damage: None,
            last_damage_frame: u32::MAX,
            last_damage_cleared: false,
            last_healing_frame: u32::MAX,
            die_called: false,
        };
        if kind == BodyKind::Inactive {
            h.current = 0.0;
            h.previous = 0.0;
        } else {
            h.state = calc_state(h.current, h.max, thresholds);
        }
        h
    }
    /// `isSubdued`.
    pub fn subdued(&self) -> bool {
        self.max <= self.subdual
    }
}

/// `calcDamageState` (`UnitDamagedThreshold`, `UnitReallyDamagedThreshold`).
pub fn calc_state(health: f32, max: f32, (damaged, really): (f32, f32)) -> DamageState {
    let ratio = health / max;
    if ratio > damaged {
        DamageState::Pristine
    } else if ratio > really {
        DamageState::Damaged
    } else if ratio > 0.0 {
        DamageState::ReallyDamaged
    } else {
        DamageState::Rubble
    }
}

/// `DieMuxData`: which deaths a die module handles.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct DieMux {
    pub death_types: u32,
    pub veterancy: u32,
    pub exempt_status: u64,
    pub required_status: u64,
}
impl DieMux {
    pub fn parse(block: &ih_ini::Block) -> Self {
        let mut m = Self { death_types: u32::MAX, veterancy: u32::MAX, exempt_status: 0, required_status: 0 };
        for f in block.fields() {
            let v = f.values();
            match f.name.as_str() {
                "DeathTypes" => m.death_types = flag_list(&v, name_list("TheDeathNames"), m.death_types, true),
                "VeterancyLevels" => m.veterancy = flag_list(&v, name_list("TheVeterancyNames"), m.veterancy, false),
                "ExemptStatus" => m.exempt_status = status_mask(&v),
                "RequiredStatus" => m.required_status = status_mask(&v),
                _ => {}
            }
        }
        m
    }
    /// `isDieApplicable` (veterancy is always regular until experience exists).
    pub fn applies(&self, death_type: u8, status: u64) -> bool {
        // `getDeathTypeFlag` shifts by `dt - 1`, so NORMAL tests bit 31.
        let bit = 1u32.wrapping_shl((death_type as u32).wrapping_sub(1));
        if self.death_types & bit == 0 {
            return false;
        }
        if self.veterancy & 1 == 0 {
            return false;
        }
        if self.exempt_status != 0 && status & self.exempt_status != 0 {
            return false;
        }
        !(self.required_status != 0 && status & self.required_status != self.required_status)
    }
}
/// `INI::parseDeathTypeFlags` / `parseVeterancyLevelFlags`: `ALL`, `NONE`, `+X`,
/// `-X`. Death flags use `setDeathTypeFlag`'s `dt - 1` shift.
fn flag_list(v: &[String], list: &[&str], start: u32, death: bool) -> u32 {
    let mut out = start;
    for t in v {
        if t.eq_ignore_ascii_case("ALL") {
            out = u32::MAX;
        } else if t.eq_ignore_ascii_case("NONE") {
            out = 0;
        } else if let Some(name) = t.strip_prefix(['+', '-']) {
            let Some(i) = list.iter().position(|n| n.eq_ignore_ascii_case(name)) else { continue };
            let bit = if death { 1u32.wrapping_shl((i as u32).wrapping_sub(1)) } else { 1 << i };
            if t.starts_with('+') {
                out |= bit;
            } else {
                out &= !bit;
            }
        }
    }
    out
}
fn status_mask(v: &[String]) -> u64 {
    let list = name_list("ObjectStatusMaskType::s_bitNameList");
    v.iter().filter_map(|n| list.iter().position(|l| l.eq_ignore_ascii_case(n))).fold(0, |m, i| m | 1 << i)
}

/// What a module does when its object dies.
#[derive(Clone, Debug, PartialEq)]
pub enum DieKind {
    /// `DestroyDie`.
    Destroy(DieMux),
    /// `SlowDeathBehavior` (`death`).
    SlowDeath(Box<crate::death::SlowDeathData>),
}
impl DieKind {
    pub fn from_module(class: &str, block: &ih_ini::Block, templates: &crate::templates::Templates) -> Option<Self> {
        match class {
            "DestroyDie" => Some(DieKind::Destroy(DieMux::parse(block))),
            "SlowDeathBehavior" => {
                Some(DieKind::SlowDeath(Box::new(crate::death::SlowDeathData::parse(block, &|kind, name| {
                    templates.effects.contains(&(kind.to_string(), name.to_string()))
                }))))
            }
            _ => None,
        }
    }
}

/// `ObjectStatusTypes` bits that the logic sets so far.
pub mod status {
    /// `OBJECT_STATUS_DESTROYED`.
    pub const DESTROYED: u64 = 1 << 1;
    /// `OBJECT_STATUS_NO_COLLISIONS`.
    pub const NO_COLLISIONS: u64 = 1 << 5;
}

impl Simulation {
    fn thresholds(&self) -> (f32, f32) {
        (self.templates.damage.damaged, self.templates.damage.really_damaged)
    }
    fn armor_for(&self, t: &Template, flags: u32) -> Option<&crate::body::ArmorTemplate> {
        best_armor_set(&t.armor_sets, flags).and_then(|s| s.armor.as_ref()).and_then(|a| self.templates.armors.get(a))
    }
    /// `ActiveBody::estimateDamage` (containers and construction come later).
    pub fn estimate_damage(&self, id: u32, damage_type: u8, amount: f32) -> f32 {
        let Some(o) = self.objects.get(&id) else { return 0.0 };
        let Some(h) = &o.health else { return 0.0 };
        if h.kind == BodyKind::Inactive {
            return if damage_type == DAMAGE_UNRESISTABLE { amount } else { 0.0 };
        }
        if (is_subdual(damage_type) && h.subdual_cap <= 0.0) || damage_type == DAMAGE_KILL_GARRISONED {
            return 0.0;
        }
        let t = &self.templates.objects[&o.template];
        self.armor_for(t, h.armor_flags).map_or(amount, |a| a.adjust(damage_type, amount))
    }

    /// `ActiveBody::internalChangeHealth` (with `ImmortalBody`'s floor).
    pub(crate) fn change_health(&mut self, id: u32, mut delta: f32) {
        let thresholds = self.thresholds();
        let o = self.objects.get_mut(&id).unwrap();
        let Some(h) = o.health.as_mut() else { return };
        match h.kind {
            BodyKind::Inactive => return,
            BodyKind::Immortal => delta = delta.max(-h.current + 1.0),
            _ => {}
        }
        h.previous = h.current;
        h.current = (h.current + delta).min(h.max).max(0.0);
        h.state = calc_state(h.current, h.max, thresholds);
        o.dead = h.current <= 0.0;
    }

    /// `Object::attemptDamage` and the body's `attemptDamage`.
    pub fn attempt_damage(&mut self, id: u32, mut info: DamageInfo) -> DamageInfo {
        let Some(h) = self.objects.get(&id).and_then(|o| o.health.clone()) else { return info };
        if info.damage_type == DAMAGE_HEALING && h.kind != BodyKind::Inactive && !info.kill {
            return self.attempt_healing_info(id, info);
        }
        if h.kind == BodyKind::Inactive {
            if info.damage_type == DAMAGE_HEALING {
                info.dealt = 0.0;
                info.clipped = 0.0;
                info.no_effect = true;
                return info;
            }
            info.dealt = 0.0;
            info.clipped = 0.0;
            info.no_effect = true;
            if info.damage_type == DAMAGE_UNRESISTABLE {
                info.no_effect = false;
                if !h.die_called {
                    self.objects.get_mut(&id).unwrap().health.as_mut().unwrap().die_called = true;
                    self.on_die(id, info);
                }
            }
            return info;
        }
        // HighlanderBody: everything but unresistable damage leaves one hit point.
        if h.kind == BodyKind::Highlander && info.damage_type != DAMAGE_UNRESISTABLE {
            info.amount = info.amount.min(h.current - 1.0);
        }
        if h.indestructible {
            return info;
        }
        info.dealt = 0.0;
        info.clipped = 0.0;
        if self.objects[&id].dead {
            return info;
        }
        let t = &self.templates.objects[&self.objects[&id].template];
        let mut amount =
            self.armor_for(t, h.armor_flags).map_or(info.amount, |a| a.adjust(info.damage_type, info.amount));
        let (mut already_handled, mut allow_modifier) = (false, true);
        if matches!(info.damage_type, DAMAGE_KILLPILOT | DAMAGE_KILL_GARRISONED | DAMAGE_STATUS) {
            // Pilots, garrisons and status effects come with containers and statuses.
            already_handled = true;
            allow_modifier = false;
        }
        if is_subdual(info.damage_type) {
            if h.subdual_cap <= 0.0 {
                return info;
            }
            let o = self.objects.get_mut(&id).unwrap();
            let hb = o.health.as_mut().unwrap();
            hb.subdual = (hb.subdual + amount).min(hb.subdual_cap);
            // onSubdualChange: DISABLED_SUBDUED for everything but projectiles.
            let now = hb.subdued();
            if now != h.subdued() && !t.has_kind("PROJECTILE") {
                let frame = self.frame;
                let o = self.objects.get_mut(&id).unwrap();
                if now {
                    o.disabled.set(crate::disabled::DisabledReason::Subdued, frame, None);
                } else {
                    o.disabled.clear(crate::disabled::DisabledReason::Subdued);
                }
            }
            already_handled = true;
            allow_modifier = false;
        }
        if allow_modifier && info.damage_type != DAMAGE_UNRESISTABLE {
            amount *= h.scalar;
        }
        if amount > 0.0 || info.kill {
            let old_state = h.state;
            if info.kill {
                amount = self.objects[&id].health.as_ref().unwrap().current;
            }
            if !already_handled {
                self.change_health(id, -amount);
            }
            let frame = self.frame;
            // isFactionStructure: any of the FS_ kinds.
            let source_counts = self.objects.get(&info.source).map(|s| {
                let st = &self.templates.objects[&s.template];
                st.has_kind("VEHICLE")
                    || st.has_kind("INFANTRY")
                    || st.values("KindOf").iter().any(|k| k.starts_with("FS_"))
            });
            let last_source = self.objects[&id].health.as_ref().unwrap().last_damage.map(|l| l.source);
            let last_alive = last_source.is_some_and(|s| self.objects.contains_key(&s));
            let o = self.objects.get_mut(&id).unwrap();
            let hb = o.health.as_mut().unwrap();
            info.dealt = amount;
            info.clipped = hb.previous - hb.current;
            if hb.last_damage_frame != frame && hb.last_damage_frame != frame.wrapping_sub(1) {
                hb.last_damage = Some(info);
                hb.last_damage_cleared = false;
                hb.last_damage_frame = frame;
            } else if let Some(counts) = source_counts {
                // A new source replaces a recent one only when it is a unit or a
                // faction building (or the recent one is gone).
                if !last_alive || counts {
                    hb.last_damage = Some(info);
                    hb.last_damage_cleared = false;
                    hb.last_damage_frame = frame;
                }
            }
            let (prev, cur, max, state) = (hb.previous, hb.current, hb.max, hb.state);
            if state != old_state {
                self.events_damage_state(id, state);
            }
            if prev / max > YELLOW_DAMAGE_PERCENT && cur / max < YELLOW_DAMAGE_PERCENT && cur > 0.0 {
                // The fear voice (audio, M6) plays one time in four.
                let _ = self.rng.integer(0, 99);
            }
            if cur <= 0.0 && prev > 0.0 {
                self.on_die(id, info);
            }
        }
        info
    }

    /// `ActiveBody::attemptHealing`.
    fn attempt_healing_info(&mut self, id: u32, mut info: DamageInfo) -> DamageInfo {
        let o = &self.objects[&id];
        let t = &self.templates.objects[&o.template];
        if !t.has_kind("BRIDGE") && !t.has_kind("BRIDGE_TOWER") && o.dead {
            return info;
        }
        let h = o.health.as_ref().unwrap();
        info.dealt = 0.0;
        info.clipped = 0.0;
        let amount = self.armor_for(t, h.armor_flags).map_or(info.amount, |a| a.adjust(info.damage_type, info.amount));
        if amount > 0.0 {
            let old_state = h.state;
            self.change_health(id, amount);
            let frame = self.frame;
            let hb = self.objects.get_mut(&id).unwrap().health.as_mut().unwrap();
            info.dealt = amount;
            info.clipped = hb.previous - hb.current;
            hb.last_damage = Some(info);
            hb.last_damage_cleared = false;
            hb.last_damage_frame = frame;
            hb.last_healing_frame = frame;
            let state = hb.state;
            if state != old_state {
                self.events_damage_state(id, state);
            }
        }
        info
    }

    /// `Object::attemptHealing`.
    pub fn attempt_healing(&mut self, id: u32, amount: f32, source: u32) -> DamageInfo {
        self.attempt_damage(id, DamageInfo::new(source, DAMAGE_HEALING, DEATH_NONE, amount))
    }

    /// `Object::kill`: damage that kills whatever the amount.
    pub fn kill(&mut self, id: u32, damage_type: u8, death_type: u8) -> DamageInfo {
        let max = self.objects.get(&id).and_then(|o| o.health.as_ref()).map_or(0.0, |h| h.max);
        let mut info = DamageInfo::new(0, damage_type, death_type, max);
        info.kill = true;
        self.attempt_damage(id, info)
    }

    /// `onBodyDamageStateChange` listeners and `evaluateVisualCondition`: the
    /// client reads the state from the body; no damage modules are ported yet.
    fn events_damage_state(&mut self, _id: u32, _state: DamageState) {}

    /// `Object::onDie`: each die module in order.
    pub(crate) fn on_die(&mut self, id: u32, info: DamageInfo) {
        let Some(o) = self.objects.get(&id) else { return };
        let dies: Vec<(usize, DieKind)> =
            o.modules.iter().enumerate().filter_map(|(i, m)| m.die.clone().map(|d| (i, d))).collect();
        self.stop_dead(id);
        for (i, d) in dies {
            let Some(o) = self.objects.get(&id) else { return };
            let status = o.status;
            match d {
                DieKind::Destroy(mux) if mux.applies(info.death_type, status) => self.destroy_object(id),
                DieKind::Destroy(_) => {}
                DieKind::SlowDeath(_) => self.slow_death_on_die(id, i, info),
            }
        }
    }
    fn stop_dead(&mut self, id: u32) {
        self.remove_goal(id);
        let o = self.objects.get_mut(&id).unwrap();
        o.path.clear();
        o.target = None;
        o.ai.remaining.clear();
    }

    /// `GameLogic::destroyObject`: marked now, removed at the end of the frame.
    pub fn destroy_object(&mut self, id: u32) {
        let Some(o) = self.objects.get_mut(&id) else { return };
        if o.status & status::DESTROYED != 0 {
            return;
        }
        o.status |= status::DESTROYED;
        o.path.clear();
        o.target = None;
        self.destroy_list.push(id);
    }
    /// `GameLogic::processDestroyList`.
    pub(crate) fn process_destroy_list(&mut self) {
        for id in std::mem::take(&mut self.destroy_list) {
            let _ = self.remove(id);
        }
    }
}

/// The body module of a template: its kind and module definition.
pub fn body_of(t: &Template) -> Option<(BodyKind, &crate::templates::ModuleDef)> {
    t.modules.iter().filter(|m| m.kind == "Body").find_map(|m| BodyKind::from_class(&m.class).map(|k| (k, m)))
}

/// Whether a module is handled by the body or as a die module.
pub fn module_kind(m: &crate::templates::ModuleDef) -> Option<ModuleKind> {
    if m.kind == "Body" && BodyKind::from_class(&m.class).is_some() {
        return Some(ModuleKind::Body);
    }
    match m.class.as_str() {
        "DestroyDie" => Some(ModuleKind::Die),
        "SlowDeathBehavior" => Some(ModuleKind::SlowDeath),
        _ => None,
    }
}
