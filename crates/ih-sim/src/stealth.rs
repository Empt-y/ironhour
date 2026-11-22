//! Stealth and detection.
//!
//! Sources: `GameLogic/Object/Update/StealthUpdate.cpp` (`allowedToStealth`,
//! `calcStealthOwner`, `update`, `markAsDetected`, `receiveGrant`,
//! `setWakeupIfInRange`), `StealthDetectorUpdate.cpp` (with
//! `PartitionFilterStealthedOrStealthGarrisoned`), `StealthUpgrade.cpp`,
//! `PartitionFilterStealthedAndUndetected` (`PartitionManager.cpp`), the
//! stealth checks in `WeaponSet::getAbleToAttackSpecificObject`,
//! `AIAttackApproachTargetState::updateInternal` and
//! `AIUpdateInterface::getNextMoodTarget`, and `OpenContain::addToContainList`
//! (`STEALTH_GARRISON` riders).
//!
//! Not ported yet: disguises (bomb trucks: `DisguisesAsTeam` objects never
//! enable their stealth), the client side (opacity, heat vision, IR particle
//! systems, sounds, EVA and radar events), stealth granted to slaves and
//! spawns, `USING_ABILITY` (no special abilities yet), script-disabled
//! stealth, and `recalcApparentControllingPlayer` for garrisons.
use crate::{
    Simulation,
    scheduler::FOREVER,
    simulation::ModuleKind,
    weapons::{Relationship, SLOTS},
};
use std::sync::OnceLock;

/// `TheStealthLevelNames` bits.
pub const NOT_WHILE_ATTACKING: u32 = 0x01;
pub const NOT_WHILE_MOVING: u32 = 0x02;
pub const NOT_WHILE_USING_ABILITY: u32 = 0x04;
pub const NOT_WHILE_FIRING_PRIMARY: u32 = 0x08;
pub const NOT_WHILE_FIRING_SECONDARY: u32 = 0x10;
pub const NOT_WHILE_FIRING_TERTIARY: u32 = 0x20;
pub const ONLY_WITH_BLACK_MARKET: u32 = 0x40;
pub const NOT_WHILE_TAKING_DAMAGE: u32 = 0x80;
pub const NOT_WHILE_FIRING_WEAPON: u32 = 0x38;
pub const NOT_WHILE_RIDERS_ATTACKING: u32 = 0x100;
const LEVEL_NAMES: [&str; 9] = [
    "ATTACKING",
    "MOVING",
    "USING_ABILITY",
    "FIRING_PRIMARY",
    "FIRING_SECONDARY",
    "FIRING_TERTIARY",
    "NO_BLACK_MARKET",
    "TAKING_DAMAGE",
    "RIDERS_ATTACKING",
];

/// The status bits stealth reads and writes.
pub struct Bits {
    pub can_stealth: u64,
    pub stealthed: u64,
    pub detected: u64,
    pub disguised: u64,
    pub ignoring_stealth: u64,
    pub sold: u64,
    pub under_construction: u64,
}
pub fn bits() -> &'static Bits {
    static B: OnceLock<Bits> = OnceLock::new();
    B.get_or_init(|| {
        let b = |n: &str| crate::construction::status_bits(&[n]);
        Bits {
            can_stealth: b("CAN_STEALTH"),
            stealthed: b("STEALTHED"),
            detected: b("DETECTED"),
            disguised: b("DISGUISED"),
            ignoring_stealth: b("IGNORING_STEALTH"),
            sold: b("SOLD"),
            under_construction: b("UNDER_CONSTRUCTION"),
        }
    })
}

fn yes(v: &str) -> bool {
    v.eq_ignore_ascii_case("yes") || v.eq_ignore_ascii_case("true")
}
fn real(v: &str) -> f32 {
    ih_ini::values::scan_real(v.trim_end_matches('%').as_bytes()).unwrap_or(0.0)
}

/// `StealthUpdateModuleData`.
#[derive(Clone, Debug, PartialEq)]
pub struct StealthData {
    pub delay: u32,
    /// `MoveThresholdSpeed`, per frame.
    pub move_threshold: f32,
    pub level: u32,
    pub required_status: u64,
    pub forbidden_status: u64,
    pub reveal_distance: f32,
    pub order_idle_enemies: bool,
    pub team_disguised: bool,
    pub innate: bool,
    pub use_rider: bool,
    pub black_market_frames: u32,
    pub granted_by_special_power: bool,
}
impl StealthData {
    pub fn from_module(b: &ih_ini::Block) -> Self {
        let mut d = Self {
            delay: u32::MAX,
            move_threshold: 0.0,
            level: 0,
            required_status: 0,
            forbidden_status: 0,
            reveal_distance: 0.0,
            order_idle_enemies: false,
            team_disguised: false,
            innate: true,
            use_rider: false,
            black_market_frames: 0,
            granted_by_special_power: false,
        };
        for f in b.fields() {
            let v = f.values();
            let first = v.first().map_or("", |s| s.as_str());
            let names: Vec<&str> = v.iter().map(|s| s.as_str()).collect();
            match f.name.as_str() {
                "StealthDelay" => d.delay = crate::death::duration(real(first)),
                // INI::parseVelocityReal: per second to per frame.
                "MoveThresholdSpeed" => d.move_threshold = real(first) / 30.0,
                // INI::parseBitString32 (NONE clears).
                "StealthForbiddenConditions" => {
                    d.level = names
                        .iter()
                        .filter_map(|n| LEVEL_NAMES.iter().position(|l| l.eq_ignore_ascii_case(n)))
                        .fold(0, |m, i| m | (1 << i))
                }
                "RequiredStatus" => d.required_status = crate::construction::status_bits(&names),
                "ForbiddenStatus" => d.forbidden_status = crate::construction::status_bits(&names),
                "RevealDistanceFromTarget" => d.reveal_distance = real(first),
                "OrderIdleEnemiesToAttackMeUponReveal" => d.order_idle_enemies = yes(first),
                "DisguisesAsTeam" => d.team_disguised = yes(first),
                "InnateStealth" => d.innate = yes(first),
                "UseRiderStealth" => d.use_rider = yes(first),
                "BlackMarketCheckDelay" => d.black_market_frames = crate::death::duration(real(first)),
                "GrantedBySpecialPower" => d.granted_by_special_power = yes(first),
                _ => {}
            }
        }
        d
    }
}

/// A `StealthUpdate`.
#[derive(Clone, Debug, PartialEq)]
pub struct Stealth {
    pub data: StealthData,
    pub module: usize,
    pub allowed_frame: u32,
    pub detection_expires: u32,
    pub next_black_market_check: u32,
    pub enabled: bool,
    /// `m_framesGranted`: 0 is forever.
    pub frames_granted: u32,
}

/// `StealthDetectorUpdateModuleData`.
#[derive(Clone, Debug, PartialEq)]
pub struct Detector {
    pub rate: u32,
    pub range: f32,
    pub enabled: bool,
    pub required_kinds: Vec<String>,
    pub forbidden_kinds: Vec<String>,
    pub while_garrisoned: bool,
    pub while_contained: bool,
}
impl Detector {
    pub fn from_module(b: &ih_ini::Block) -> Self {
        let mut d = Self {
            rate: 1,
            range: 0.0,
            enabled: true,
            required_kinds: vec![],
            forbidden_kinds: vec![],
            while_garrisoned: false,
            while_contained: false,
        };
        for f in b.fields() {
            let v = f.values();
            let first = v.first().map_or("", |s| s.as_str());
            match f.name.as_str() {
                "DetectionRate" => d.rate = crate::death::duration(real(first)),
                "DetectionRange" => d.range = real(first),
                "InitiallyDisabled" => d.enabled = !yes(first),
                "ExtraRequiredKindOf" => d.required_kinds = v.iter().filter(|k| *k != "NONE").cloned().collect(),
                "ExtraForbiddenKindOf" => d.forbidden_kinds = v.iter().filter(|k| *k != "NONE").cloned().collect(),
                "CanDetectWhileGarrisoned" => d.while_garrisoned = yes(first),
                "CanDetectWhileContained" => d.while_contained = yes(first),
                _ => {}
            }
        }
        d
    }
}

impl Simulation {
    fn has_status(&self, id: u32, bit: u64) -> bool {
        self.objects.get(&id).is_some_and(|o| o.status & bit != 0)
    }

    /// Stealthed, not detected and not disguised: what most checks mean by hidden.
    pub fn stealthed_and_undetected(&self, id: u32) -> bool {
        let b = bits();
        self.objects.get(&id).is_some_and(|o| o.status & (b.stealthed | b.detected | b.disguised) == b.stealthed)
    }

    /// `OpenContain::getStealthUnitsContained`.
    pub(crate) fn stealth_units_contained(&self, id: u32) -> usize {
        let Some(c) = self.objects.get(&id).and_then(|o| o.contain.as_ref()) else { return 0 };
        c.riders
            .iter()
            .filter(|r| {
                self.objects.get(r).is_some_and(|o| self.templates.objects[&o.template].has_kind("STEALTH_GARRISON"))
            })
            .count()
    }

    /// `PartitionFilterStealthedAndUndetected(me, false)`: true when `other`
    /// counts as hidden from `me` (disguises are not ported, so a disguiser
    /// is never hidden).
    pub(crate) fn hidden_from(&self, me: u32, other: u32) -> bool {
        let Some(o) = self.objects.get(&other) else { return false };
        let b = bits();
        if o.status & b.stealthed != 0 && o.status & b.detected == 0 {
            return !self.templates.objects[&o.template].has_kind("DISGUISER");
        }
        // A building garrisoned only by stealthed enemies nobody detected.
        if let Some(c) = &o.contain
            && let Some(&first) = c.riders.first()
            && self.stealth_units_contained(other) == c.riders.len()
            && !self.has_status(first, b.detected)
        {
            return self.relationship(me, first) == Relationship::Enemies;
        }
        false
    }

    /// `WeaponSet::getAbleToAttackSpecificObject`'s stealth veto.
    pub(crate) fn stealth_prevents_attack(&self, source: u32, victim: u32) -> bool {
        if self.has_status(source, bits().ignoring_stealth) {
            return false;
        }
        let b = bits();
        let Some(v) = self.objects.get(&victim) else { return false };
        // A disguiser is attackable unless disguised (disguises are not ported).
        v.status & b.stealthed != 0
            && v.status & b.detected == 0
            && !self.templates.objects[&v.template].has_kind("DISGUISER")
    }

    /// The `StealthUpdate` module's constructor.
    pub(crate) fn stealth_created(&mut self, id: u32) {
        let Some(s) = self.objects.get(&id).and_then(|o| o.stealth.as_ref()) else { return };
        let (innate, module, granted) = (s.data.innate, s.module, s.data.granted_by_special_power);
        if innate {
            self.objects.get_mut(&id).unwrap().status |= bits().can_stealth;
        }
        if granted {
            self.scheduler.reschedule((id, module), FOREVER);
        }
    }

    /// `StealthUpdate::calcStealthOwner`: the first rider when the rider's
    /// stealth rules decide.
    fn stealth_owner(&self, id: u32) -> u32 {
        let o = &self.objects[&id];
        if o.stealth.as_ref().is_some_and(|s| s.data.use_rider)
            && let Some(&r) = o.contain.as_ref().and_then(|c| c.riders.first())
        {
            return r;
        }
        id
    }
    /// The stealth owner's delay and reveal order (the container's own when
    /// the rider has no stealth).
    fn owner_rules(&self, id: u32, owner: u32) -> (u32, bool) {
        let own = &self.objects[&id].stealth.as_ref().unwrap().data;
        let d = self.objects.get(&owner).and_then(|o| o.stealth.as_ref()).map_or(own, |s| &s.data);
        (d.delay, d.order_idle_enemies)
    }

    fn any_black_market(&self, owner: u32) -> bool {
        let b = bits();
        self.objects.values().any(|o| {
            o.owner == owner
                && !o.dead
                && o.status & (b.under_construction | b.sold) == 0
                && self.templates.objects[&o.template].has_kind("FS_BLACK_MARKET")
        })
    }

    /// `StealthUpdate::allowedToStealth`.
    fn allowed_to_stealth(&mut self, id: u32, owner: u32) -> bool {
        let now = self.frame;
        let s = self.objects[&id].stealth.clone().unwrap();
        let mut flags = s.data.level;
        if owner != id
            && let Some(os) = self.objects.get(&owner).and_then(|o| o.stealth.as_ref())
        {
            flags = os.data.level;
        }
        let o = &self.objects[&id];
        let firing = o.ai.attack.as_ref().is_some_and(|m| m.sub == crate::attack::Sub::Fire);
        if flags & NOT_WHILE_ATTACKING != 0 && firing {
            return false;
        }
        if flags & ONLY_WITH_BLACK_MARKET != 0 && s.next_black_market_check < now {
            // The check time only moves on by the delay, so it can lag behind.
            let next = s.next_black_market_check + s.data.black_market_frames + self.rng.integer(0, 10) as u32;
            self.objects.get_mut(&id).unwrap().stealth.as_mut().unwrap().next_black_market_check = next;
            let owner_player = self.objects[&id].owner;
            if !self.any_black_market(owner_player) {
                return false;
            }
        }
        let o = &self.objects[&id];
        if !self.has_status(owner, bits().can_stealth) {
            return false;
        }
        if flags & NOT_WHILE_TAKING_DAMAGE != 0
            && let Some(h) = &o.health
            && h.last_damage_frame != u32::MAX
            && h.last_damage_frame >= now.wrapping_sub(1)
            && h.last_damage.is_some_and(|d| d.damage_type != crate::body::DAMAGE_HEALING)
        {
            return false;
        }
        let d = &s.data;
        if d.required_status != 0 && o.status & d.required_status != d.required_status {
            return false;
        }
        if o.status & d.forbidden_status != 0 {
            return false;
        }
        if flags & NOT_WHILE_FIRING_WEAPON != 0 && firing {
            if flags & NOT_WHILE_FIRING_WEAPON == NOT_WHILE_FIRING_WEAPON {
                return false;
            }
            let last = now.wrapping_sub(1);
            for (slot, w) in o.weapon_set.weapons.iter().enumerate().take(SLOTS.min(3)) {
                if flags & (NOT_WHILE_FIRING_PRIMARY << slot) != 0
                    && w.as_ref().is_some_and(|w| w.last_fire_frame >= last)
                {
                    return false;
                }
            }
        }
        if let Some(c) = o.contained_by.and_then(|c| self.objects.get(&c)).and_then(|c| c.contain.as_ref())
            && !c.data.garrison
        {
            return false;
        }
        if flags & NOT_WHILE_RIDERS_ATTACKING != 0
            && let Some(c) = &o.contain
            && c.data.passengers_fire
            && c.riders.iter().any(|r| self.objects.get(r).is_some_and(|r| r.ai.attack.is_some()))
        {
            return false;
        }
        let v = o.physics.vel;
        let speed = ih_math::sqrt(v[0] * v[0] + v[1] * v[1] + v[2] * v[2]);
        !(flags & NOT_WHILE_MOVING != 0
            && o.modules.iter().any(|m| m.kind == ModuleKind::Physics)
            && speed > d.move_threshold)
    }

    /// `StealthUpdate::update`.
    pub(crate) fn stealth_update(&mut self, id: u32) -> u32 {
        let owner = self.stealth_owner(id);
        let (delay, _) = self.owner_rules(id, owner);
        let now = self.frame;
        let s = self.objects[&id].stealth.clone().unwrap();
        let sleep = |enabled: bool| if enabled { 1 } else { FOREVER };
        if !s.enabled {
            return FOREVER;
        }
        if s.data.reveal_distance > 0.0
            && let Some(v) = self.objects[&id].ai.attack.as_ref().and_then(|m| m.victim)
            && let Some(vo) = self.objects.get(&v)
        {
            let (a, b) = (self.objects[&id].position, vo.position);
            let d2 = (a[0] - b[0]) * (a[0] - b[0]) + (a[1] - b[1]) * (a[1] - b[1]);
            if d2 <= s.data.reveal_distance * s.data.reveal_distance {
                self.mark_as_detected(id, 0);
                return sleep(self.objects[&id].stealth.as_ref().unwrap().enabled);
            }
        }
        if s.frames_granted > 0 {
            let left = s.frames_granted - 1;
            self.objects.get_mut(&id).unwrap().stealth.as_mut().unwrap().frames_granted = left;
            // A player's own order ends granted stealth.
            if !self.objects[&id].ai.cmd_from_ai && self.objects[&id].ai.active() {
                self.receive_grant(id, false, 0);
            }
            if left == 0 {
                self.receive_grant(id, false, 0);
            }
        }
        let b = bits();
        if self.allowed_to_stealth(id, owner) {
            if self.objects[&id].stealth.as_ref().unwrap().allowed_frame > now {
                return sleep(self.objects[&id].stealth.as_ref().unwrap().enabled);
            }
            self.objects.get_mut(&id).unwrap().status |= b.stealthed;
        } else {
            let o = self.objects.get_mut(&id).unwrap();
            o.stealth.as_mut().unwrap().allowed_frame = now.saturating_add(delay);
            o.status &= !b.stealthed;
        }
        let o = self.objects.get_mut(&id).unwrap();
        if o.stealth.as_ref().unwrap().detection_expires > now {
            o.status |= b.detected;
        } else {
            o.status &= !b.detected;
        }
        sleep(o.stealth.as_ref().unwrap().enabled)
    }

    /// `StealthUpdate::receiveGrant` (special powers grant stealth for a time).
    pub fn receive_grant(&mut self, id: u32, active: bool, frames: u32) {
        let Some(s) = self.objects.get(&id).and_then(|o| o.stealth.as_ref()) else { return };
        if s.data.team_disguised {
            return;
        }
        let module = s.module;
        let now = self.frame;
        let b = bits();
        let o = self.objects.get_mut(&id).unwrap();
        let s = o.stealth.as_mut().unwrap();
        s.enabled = active;
        if active {
            o.status |= b.can_stealth | b.stealthed;
            s.allowed_frame = now;
            s.frames_granted = frames;
            self.scheduler.reschedule((id, module), now.saturating_add(1));
        } else {
            o.status &= !(b.can_stealth | b.stealthed);
            s.allowed_frame = FOREVER;
            s.frames_granted = 0;
        }
    }

    /// `StealthUpdate::markAsDetected`: `frames` 0 uses the stealth delay.
    pub fn mark_as_detected(&mut self, id: u32, frames: u32) {
        if self.objects.get(&id).is_none_or(|o| o.stealth.is_none()) {
            return;
        }
        let owner = self.stealth_owner(id);
        let (delay, order_idles) = self.owner_rules(id, owner);
        let now = self.frame;
        let s = self.objects.get_mut(&id).unwrap().stealth.as_mut().unwrap();
        if frames == 0 {
            s.detection_expires = now.saturating_add(delay);
        } else if s.detection_expires < now + frames {
            s.detection_expires = now + frames;
        }
        if order_idles {
            // setWakeupIfInRange for every enemy object with an AI.
            let me = self.objects[&id].position;
            let enemies: Vec<u32> = self
                .objects
                .values()
                .filter(|o| {
                    o.id != id
                        && !o.dead
                        && o.modules.iter().any(|m| m.kind == ModuleKind::Steering)
                        && self.players_relationship(o.owner, self.objects[&id].owner) == Relationship::Enemies
                })
                .map(|o| o.id)
                .collect();
            for e in enemies {
                let o = &self.objects[&e];
                let vision = self.templates.objects[&o.template].number("VisionRange", 0.0);
                let p = o.position;
                let d = [p[0] - me[0], p[1] - me[1], p[2] - me[2]];
                if ih_math::sqrt(d[0] * d[0] + d[1] * d[1] + d[2] * d[2]) > vision {
                    continue;
                }
                // AIUpdateInterface::wakeUpAndAttemptToTarget
                if !o.ai.active() {
                    let o = self.objects.get_mut(&e).unwrap();
                    o.ai.idle.next_mood_check = now;
                    o.ai.idle.randomly_offset = true;
                }
            }
        }
    }

    /// `StealthDetectorUpdate::update`.
    pub(crate) fn detector_update(&mut self, id: u32) -> u32 {
        let o = &self.objects[&id];
        let d = o.detector.clone().unwrap();
        let b = bits();
        if o.dead {
            return FOREVER;
        }
        if o.status & b.under_construction != 0 {
            return 1;
        }
        if o.status & b.sold != 0 {
            return FOREVER;
        }
        if let Some(c) = o.contained_by.and_then(|c| self.objects.get(&c)).and_then(|c| c.contain.as_ref())
            && (if c.data.garrison { !d.while_garrisoned } else { !d.while_contained })
        {
            return d.rate;
        }
        let range =
            if d.range > 0.0 { d.range } else { self.templates.objects[&o.template].number("VisionRange", 0.0) };
        let me = o.position;
        let found: Vec<u32> = self
            .objects
            .values()
            .filter(|t| {
                if t.id == id || t.dead || t.contained_by.is_some() {
                    return false;
                }
                // PartitionFilterStealthedOrStealthGarrisoned
                let stealthy = t.status & b.stealthed != 0
                    || t.contain.as_ref().is_some_and(|c| c.data.garrison) && self.stealth_units_contained(t.id) > 0;
                if !stealthy || self.relationship(id, t.id) == Relationship::Allies {
                    return false;
                }
                let tt = &self.templates.objects[&t.template];
                if !d.required_kinds.iter().all(|k| tt.has_kind(k)) || d.forbidden_kinds.iter().any(|k| tt.has_kind(k))
                {
                    return false;
                }
                let dx = t.position[0] - me[0];
                let dy = t.position[1] - me[1];
                dx * dx + dy * dy <= range * range
            })
            .map(|t| t.id)
            .collect();
        for them in found {
            if self.objects[&them].stealth.is_some() {
                self.mark_as_detected(them, d.rate + 1);
            } else {
                let riders = self.objects[&them].contain.as_ref().map(|c| c.riders.clone()).unwrap_or_default();
                for r in riders {
                    if self.objects.get(&r).is_some_and(|o| o.stealth.is_some())
                        && self.objects[&id].owner != self.objects[&r].owner
                        && self.relationship(id, r) != Relationship::Allies
                    {
                        self.mark_as_detected(r, d.rate + 2);
                    }
                }
            }
        }
        d.rate
    }

    /// `StealthDetectorUpdate::setSDEnabled`.
    pub fn set_detector_enabled(&mut self, id: u32, enabled: bool) {
        let Some(o) = self.objects.get_mut(&id) else { return };
        let Some(d) = o.detector.as_mut() else { return };
        d.enabled = enabled;
        if let Some(i) = o.modules.iter().position(|m| m.kind == ModuleKind::StealthDetector) {
            self.scheduler.reschedule((id, i), if enabled { self.frame.saturating_add(1) } else { FOREVER });
        }
    }
}
