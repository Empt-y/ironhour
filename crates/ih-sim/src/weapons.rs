//! Weapons, from `Weapon.cpp` and `WeaponSet.cpp`: weapon templates
//! (`WeaponStore::parseWeaponTemplateDefinition`), per-object weapon sets
//! (`ThingTemplate::parseWeaponTemplateSet`, `WeaponSet::updateWeaponSet`,
//! `chooseBestWeaponForTarget`), clips, reloads and shot delays
//! (`reloadWithBonus`, `privateFireWeapon`, `getStatus`), attack ranges
//! (`isWithinAttackRange`, `FROM_BOUNDINGSPHERE_2D` with `RATIONALIZE_ATTACK_RANGE`)
//! and damage delivery (`fireWeaponTemplate`, `dealDamageInternal`, the
//! weapon store's delayed damage).
//!
//! Stand-ins and gaps: a projectile weapon deals its damage after the flight
//! time (distance over `WeaponSpeed`) instead of launching its projectile
//! object (projectiles come next); weapon bonuses are all neutral (no
//! veterancy, upgrades or garrisons yet); FX, OCLs, sounds, lasers and streams,
//! historic bonuses, scatter-target tables, request-assist, leech range,
//! shockwaves, turrets, pitch limits and firing line of sight are not ported;
//! every model has one barrel.
use crate::{
    Simulation,
    body::{DAMAGE_UNRESISTABLE, DamageInfo, HUGE_DAMAGE_AMOUNT},
    death::duration,
    templates::Template,
};

pub const SLOTS: usize = 3;
pub const ANTI_AIRBORNE_VEHICLE: u32 = 0x01;
pub const ANTI_GROUND: u32 = 0x02;
pub const ANTI_PROJECTILE: u32 = 0x04;
pub const ANTI_SMALL_MISSILE: u32 = 0x08;
pub const ANTI_MINE: u32 = 0x10;
pub const ANTI_AIRBORNE_INFANTRY: u32 = 0x20;
pub const ANTI_BALLISTIC_MISSILE: u32 = 0x40;
pub const ANTI_PARACHUTE: u32 = 0x80;
pub const AFFECTS_SELF: u32 = 0x01;
pub const AFFECTS_ALLIES: u32 = 0x02;
pub const AFFECTS_ENEMIES: u32 = 0x04;
pub const AFFECTS_NEUTRALS: u32 = 0x08;
pub const KILLS_SELF: u32 = 0x10;
pub const DOESNT_AFFECT_SIMILAR: u32 = 0x20;
pub const DOESNT_AFFECT_AIRBORNE: u32 = 0x40;
pub const COLLIDE_ALLIES: u32 = 0x01;
pub const COLLIDE_ENEMIES: u32 = 0x02;
pub const COLLIDE_STRUCTURES: u32 = 0x04;
pub const COLLIDE_SHRUBBERY: u32 = 0x08;
pub const COLLIDE_PROJECTILE: u32 = 0x10;
pub const COLLIDE_WALLS: u32 = 0x20;
pub const COLLIDE_SMALL_MISSILES: u32 = 0x40;
pub const COLLIDE_BALLISTIC_MISSILES: u32 = 0x80;
pub const COLLIDE_CONTROLLED_STRUCTURES: u32 = 0x100;
/// `PATHFIND_CELL_SIZE_F * 0.25`: `RATIONALIZE_ATTACK_RANGE`'s undersize.
const UNDERSIZE: f32 = 2.5;
/// `NO_MAX_SHOTS_LIMIT`.
pub const NO_MAX_SHOTS: i32 = 0x7fff_ffff;

fn names(id: &str) -> &'static [&'static str] {
    ih_ini::schema::schema().name_list(id).unwrap_or(&[])
}
fn index_of(list: &str, name: &str) -> Option<usize> {
    names(list).iter().position(|n| n.eq_ignore_ascii_case(name))
}

/// `WeaponTemplate`.
#[derive(Clone, Debug, PartialEq)]
pub struct WeaponTemplate {
    pub name: String,
    pub primary_damage: f32,
    pub primary_radius: f32,
    pub secondary_damage: f32,
    pub secondary_radius: f32,
    pub attack_range: f32,
    pub minimum_range: f32,
    /// Radians.
    pub aim_delta: f32,
    pub scatter_radius: f32,
    pub infantry_inaccuracy: f32,
    pub damage_type: u8,
    pub death_type: u8,
    /// Per frame (`parseVelocityReal`).
    pub speed: f32,
    pub projectile: Option<String>,
    /// `MinWeaponSpeed` (per frame) and `ScaleWeaponSpeed`.
    pub min_speed: f32,
    pub scale_speed: bool,
    /// `ProjectileCollidesWith` (`TheWeaponCollideMaskNames` bits).
    pub collides: u32,
    pub damage_at_self: bool,
    /// `MissileCallsOnDie` (`getDieOnDetonate`).
    pub die_on_detonate: bool,
    pub affects: u32,
    pub anti: u32,
    pub clip_size: i32,
    pub clip_reload: u32,
    pub min_delay: u32,
    pub max_delay: u32,
    pub shots_per_barrel: i32,
    /// `AutoReloadsClip = YES` (`AUTO_RELOAD`).
    pub auto_reload: bool,
    pub pre_attack_delay: u32,
    /// `PreAttackType` (`PER_SHOT`, `PER_ATTACK`, `PER_CLIP`).
    pub pre_attack_type: u8,
    pub radius_damage_angle: f32,
    pub continue_range: f32,
    pub laser: bool,
    pub suspend_fx_delay: u32,
}
impl WeaponTemplate {
    fn new(name: &str) -> Self {
        Self {
            name: name.into(),
            primary_damage: 0.0,
            primary_radius: 0.0,
            secondary_damage: 0.0,
            secondary_radius: 0.0,
            attack_range: 0.0,
            minimum_range: 0.0,
            aim_delta: 0.0,
            scatter_radius: 0.0,
            infantry_inaccuracy: 0.0,
            damage_type: 0,
            death_type: 0,
            speed: 999_999.0,
            projectile: None,
            min_speed: 999_999.0,
            scale_speed: false,
            collides: COLLIDE_STRUCTURES,
            damage_at_self: false,
            die_on_detonate: false,
            affects: AFFECTS_ALLIES | AFFECTS_ENEMIES | AFFECTS_NEUTRALS,
            anti: ANTI_GROUND,
            clip_size: 0,
            clip_reload: 0,
            min_delay: 0,
            max_delay: 0,
            shots_per_barrel: 1,
            auto_reload: true,
            pre_attack_delay: 0,
            pre_attack_type: 0,
            radius_damage_angle: std::f32::consts::PI,
            continue_range: 0.0,
            laser: false,
            suspend_fx_delay: 0,
        }
    }
    /// Every definition of the weapon applied in order (overrides included).
    pub fn parse(name: &str, blocks: &[&ih_ini::Block]) -> Self {
        let mut w = Self::new(name);
        for b in blocks {
            for f in b.fields() {
                w.field(&f.name, &f.values());
            }
        }
        w
    }
    fn field(&mut self, key: &str, v: &[String]) {
        let real = |i: usize| {
            v.get(i).and_then(|s| ih_ini::values::scan_real(s.trim_end_matches('%').as_bytes()).ok()).unwrap_or(0.0)
        };
        let yes = || v.first().is_some_and(|s| matches!(s.to_ascii_lowercase().as_str(), "yes" | "true" | "1"));
        let first = || v.first().cloned().unwrap_or_default();
        let anti = |bit: u32, w: &mut Self| if yes() { w.anti |= bit } else { w.anti &= !bit };
        match key {
            "PrimaryDamage" => self.primary_damage = real(0),
            "PrimaryDamageRadius" => self.primary_radius = real(0),
            "SecondaryDamage" => self.secondary_damage = real(0),
            "SecondaryDamageRadius" => self.secondary_radius = real(0),
            "AttackRange" => self.attack_range = real(0),
            "MinimumAttackRange" => self.minimum_range = real(0),
            "AcceptableAimDelta" => self.aim_delta = real(0).to_radians(),
            "ScatterRadius" => self.scatter_radius = real(0),
            "ScatterRadiusVsInfantry" => self.infantry_inaccuracy = real(0),
            "DamageType" => self.damage_type = crate::body::damage_type(&first()).unwrap_or(0),
            "DeathType" => self.death_type = crate::body::death_type(&first()).unwrap_or(0),
            "WeaponSpeed" => self.speed = real(0) * (1.0 / 30.0),
            "MinWeaponSpeed" => self.min_speed = real(0) * (1.0 / 30.0),
            "ScaleWeaponSpeed" => self.scale_speed = yes(),
            "ProjectileCollidesWith" => {
                self.collides = 0;
                for n in v {
                    if let Some(i) = index_of("TheWeaponCollideMaskNames", n) {
                        self.collides |= 1 << i;
                    }
                }
            }
            "ProjectileObject" => {
                self.projectile = Some(first()).filter(|p| !p.is_empty() && !p.eq_ignore_ascii_case("None"))
            }
            "DamageDealtAtSelfPosition" => self.damage_at_self = yes(),
            "MissileCallsOnDie" => self.die_on_detonate = yes(),
            "RadiusDamageAffects" => {
                self.affects = 0;
                for n in v {
                    if let Some(i) = index_of("TheWeaponAffectsMaskNames", n) {
                        self.affects |= 1 << i;
                    }
                }
            }
            "AntiAirborneVehicle" => anti(ANTI_AIRBORNE_VEHICLE, self),
            "AntiGround" => anti(ANTI_GROUND, self),
            "AntiProjectile" => anti(ANTI_PROJECTILE, self),
            "AntiSmallMissile" => anti(ANTI_SMALL_MISSILE, self),
            "AntiMine" => anti(ANTI_MINE, self),
            "AntiParachute" => anti(ANTI_PARACHUTE, self),
            "AntiAirborneInfantry" => anti(ANTI_AIRBORNE_INFANTRY, self),
            "AntiBallisticMissile" => anti(ANTI_BALLISTIC_MISSILE, self),
            "ClipSize" => self.clip_size = real(0) as i32,
            "ClipReloadTime" => self.clip_reload = duration(real(0)),
            "DelayBetweenShots" => {
                // parseShotDelay: "<ms>" or "Min:<ms> Max:<ms>".
                let joined = v.join(" ");
                let parts: Vec<&str> = joined.split([' ', ':', '\t']).filter(|s| !s.is_empty()).collect();
                let num = |s: &str| ih_ini::values::scan_real(s.as_bytes()).unwrap_or(0.0) as i32 as f32;
                let (min, max) = if parts.first().is_some_and(|p| p.eq_ignore_ascii_case("Min")) {
                    let min = parts.get(1).map_or(0.0, |s| num(s));
                    let max = if parts.get(2).is_some_and(|p| p.eq_ignore_ascii_case("Max")) {
                        parts.get(3).map_or(min, |s| num(s))
                    } else {
                        min
                    };
                    (min, max)
                } else {
                    let d = parts.first().map_or(0.0, |s| num(s));
                    (d, d)
                };
                self.min_delay = duration(min);
                self.max_delay = duration(max);
            }
            "ShotsPerBarrel" => self.shots_per_barrel = real(0) as i32,
            "AutoReloadsClip" => self.auto_reload = index_of("TheWeaponReloadNames", &first()) == Some(0),
            "PreAttackDelay" => self.pre_attack_delay = duration(real(0)),
            "PreAttackType" => self.pre_attack_type = index_of("TheWeaponPrefireNames", &first()).unwrap_or(0) as u8,
            "RadiusDamageAngle" => self.radius_damage_angle = real(0).to_radians(),
            "ContinueAttackRange" => self.continue_range = real(0),
            "LaserName" => self.laser = !first().is_empty(),
            "SuspendFXDelay" => self.suspend_fx_delay = duration(real(0)),
            _ => {}
        }
    }
    /// `getAttackRange` with `RATIONALIZE_ATTACK_RANGE`.
    pub fn attack_range(&self) -> f32 {
        (self.attack_range - UNDERSIZE).max(0.0)
    }
    /// `getMinimumAttackRange`.
    pub fn minimum_range(&self) -> f32 {
        (self.minimum_range - UNDERSIZE).max(0.0)
    }
    /// `isContactWeapon`: an attack range under a cell.
    pub fn is_contact(&self) -> bool {
        self.attack_range - UNDERSIZE < crate::navigation::CELL
    }
}

/// A `WeaponSet` block.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct WeaponSetDef {
    pub conditions: u32,
    pub weapons: [Option<String>; SLOTS],
    pub auto_choose: [u32; SLOTS],
    pub preferred_against: [Vec<String>; SLOTS],
    pub shared_reload: bool,
    pub lock_shared: bool,
}
impl WeaponSetDef {
    pub fn parse(block: &ih_ini::Block) -> Self {
        let mut s = Self { auto_choose: [u32::MAX; SLOTS], ..Default::default() };
        let slot = |v: &[String]| v.first().and_then(|n| index_of("TheWeaponSlotTypeNames", n));
        for f in block.fields() {
            let v = f.values();
            match f.name.as_str() {
                "Conditions" => {
                    s.conditions = 0;
                    for n in &v {
                        if let Some(i) = index_of("WeaponSetFlags::s_bitNameList", n) {
                            s.conditions |= 1 << i;
                        }
                    }
                }
                "Weapon" => {
                    if let Some(i) = slot(&v) {
                        s.weapons[i] = v.get(1).filter(|n| !n.eq_ignore_ascii_case("None")).cloned();
                    }
                }
                "AutoChooseSources" => {
                    if let Some(i) = slot(&v) {
                        s.auto_choose[i] = 0;
                        for n in &v[1..] {
                            if let Some(b) = index_of("TheCommandSourceMaskNames", n) {
                                s.auto_choose[i] |= 1 << b;
                            }
                        }
                    }
                }
                "PreferredAgainst" => {
                    if let Some(i) = slot(&v) {
                        s.preferred_against[i] = v[1..].iter().filter(|n| *n != "NONE").cloned().collect();
                    }
                }
                "ShareWeaponReloadTime" => s.shared_reload = v.first().is_some_and(|b| b.eq_ignore_ascii_case("yes")),
                "WeaponLockSharedAcrossSets" => {
                    s.lock_shared = v.first().is_some_and(|b| b.eq_ignore_ascii_case("yes"))
                }
                _ => {}
            }
        }
        s
    }
}
/// `findWeaponTemplateSet` (the armour sets' sparse match).
pub fn best_weapon_set(sets: &[WeaponSetDef], flags: u32) -> Option<usize> {
    let mut best = None;
    let (mut best_yes, mut best_extra) = (0, 999);
    for (i, s) in sets.iter().enumerate() {
        let yes = (flags & s.conditions).count_ones() as i32;
        let extra = (!flags & s.conditions).count_ones() as i32;
        if yes > best_yes || (yes >= best_yes && extra < best_extra) {
            best = Some(i);
            best_yes = yes;
            best_extra = extra;
        }
    }
    best
}

/// `WeaponStatus`.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
#[repr(u8)]
pub enum Status {
    #[default]
    OutOfAmmo = 0,
    ReadyToFire = 1,
    BetweenFiringShots = 2,
    ReloadingClip = 3,
    PreAttack = 4,
}

/// A `Weapon`.
#[derive(Clone, Debug, PartialEq)]
pub struct Weapon {
    pub template: String,
    pub slot: u8,
    pub status: Status,
    pub ammo: i32,
    pub fire_again_frame: u32,
    pub pre_attack_finished: u32,
    pub last_reload_started: u32,
    pub max_shots: i32,
    pub barrel: i32,
    pub shots_for_barrel: i32,
    pub last_fire_frame: u32,
}
impl Weapon {
    fn new(t: &WeaponTemplate, slot: u8) -> Self {
        Self {
            template: t.name.clone(),
            slot,
            status: Status::OutOfAmmo,
            ammo: 0,
            fire_again_frame: 0,
            pre_attack_finished: 0,
            last_reload_started: 0,
            max_shots: NO_MAX_SHOTS,
            barrel: 0,
            shots_for_barrel: t.shots_per_barrel,
            last_fire_frame: 0,
        }
    }
    /// `getStatus` (which also refreshes the stored status).
    pub fn status(&mut self, now: u32) -> Status {
        if now < self.pre_attack_finished {
            return Status::PreAttack;
        }
        if now >= self.fire_again_frame {
            self.status = if self.ammo > 0 { Status::ReadyToFire } else { Status::OutOfAmmo };
        }
        self.status
    }
}

/// An object's `WeaponSet`.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct WeaponSet {
    /// Index into the template's weapon sets.
    pub set: Option<usize>,
    pub weapons: [Option<Weapon>; SLOTS],
    pub current: u8,
    /// `WeaponLockType`.
    pub lock: u8,
    /// `m_curWeaponTemplateSet->isSharedReloadTime`.
    pub shared_reload: bool,
}

/// Delayed damage in the weapon store.
#[derive(Clone, Debug, PartialEq)]
pub struct DelayedDamage {
    pub weapon: String,
    pub position: [f32; 3],
    pub frame: u32,
    pub source: u32,
    pub victim: u32,
}

/// `Relationship`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Relationship {
    Enemies,
    Neutral,
    Allies,
}

impl Simulation {
    /// `getRelationship` until players and teams exist: the same owner is an
    /// ally, the neutral player (0) is neutral to everyone else, other owners
    /// are enemies.
    pub fn relationship(&self, a: u32, b: u32) -> Relationship {
        let (Some(oa), Some(ob)) = (self.objects.get(&a), self.objects.get(&b)) else { return Relationship::Neutral };
        if oa.owner == ob.owner {
            Relationship::Allies
        } else if oa.owner == 0 || ob.owner == 0 {
            Relationship::Neutral
        } else {
            Relationship::Enemies
        }
    }

    fn weapon_template(&self, name: &str) -> Option<&WeaponTemplate> {
        self.templates.weapons.get(name)
    }

    /// `WeaponSet::updateWeaponSet`: on a new set, fresh weapons with full clips.
    pub(crate) fn update_weapon_set(&mut self, id: u32) {
        let o = &self.objects[&id];
        let t = &self.templates.objects[&o.template];
        let Some(set) = best_weapon_set(&t.weapon_sets, 0) else { return };
        if o.weapon_set.set == Some(set) {
            return;
        }
        let def = t.weapon_sets[set].clone();
        let frame = self.frame;
        let mut ws = WeaponSet {
            set: Some(set),
            shared_reload: def.shared_reload,
            current: if def.lock_shared { o.weapon_set.current } else { 0 },
            lock: if def.lock_shared { o.weapon_set.lock } else { 0 },
            ..Default::default()
        };
        for i in (0..SLOTS).rev() {
            if let Some(wt) = def.weapons[i].as_ref().and_then(|n| self.weapon_template(n)) {
                let mut w = Weapon::new(wt, i as u8);
                // loadAmmoNow
                w.ammo = if wt.clip_size > 0 { wt.clip_size } else { 0x7fff_ffff };
                w.status = Status::ReloadingClip;
                w.last_reload_started = frame;
                w.fire_again_frame = frame;
                ws.weapons[i] = Some(w);
            }
        }
        self.objects.get_mut(&id).unwrap().weapon_set = ws;
    }

    /// `getVictimAntiMask`.
    pub(crate) fn victim_anti_mask(&self, victim: u32) -> u32 {
        let o = &self.objects[&victim];
        let t = &self.templates.objects[&o.template];
        if t.has_kind("SMALL_MISSILE") {
            ANTI_SMALL_MISSILE
        } else if t.has_kind("BALLISTIC_MISSILE") {
            ANTI_BALLISTIC_MISSILE
        } else if t.has_kind("PROJECTILE") {
            ANTI_PROJECTILE
        } else if t.has_kind("MINE") || t.has_kind("DEMOTRAP") {
            ANTI_MINE | ANTI_GROUND
        } else if self.airborne_target(victim) {
            if t.has_kind("VEHICLE") {
                ANTI_AIRBORNE_VEHICLE
            } else if t.has_kind("INFANTRY") {
                ANTI_AIRBORNE_INFANTRY
            } else if t.has_kind("PARACHUTE") {
                ANTI_PARACHUTE
            } else {
                0
            }
        } else {
            ANTI_GROUND
        }
    }
    /// `isAirborneTarget`: set by an airborne locomotor while it flies.
    pub fn airborne_target(&self, id: u32) -> bool {
        let o = &self.objects[&id];
        let t = &self.templates.objects[&o.template];
        t.locomotor().and_then(|n| self.templates.locomotors.get(n)).is_some_and(|l| l.air() && !l.ground())
            && self.significantly_above_terrain(id)
    }
    /// `isSignificantlyAboveTerrain`: more than three frames' fall above the ground.
    pub fn significantly_above_terrain(&self, id: u32) -> bool {
        let o = &self.objects[&id];
        let ground = self.grid.height_at(
            &self.map,
            crate::terrain::Waypoint { position: [o.position[0], o.position[1]], layer: o.layer },
        );
        o.position[2] - ground > -9.0 * crate::locomotion::DEFAULT_GRAVITY
    }

    /// Bounding radii (`calcBoundingStuff`): circle, sphere, and the z offset to the centre.
    pub(crate) fn bounding(t: &Template) -> (f32, f32, f32) {
        let scale = t.number("Scale", 1.0);
        let major = t.number("GeometryMajorRadius", 5.0) * scale;
        let minor = t.number("GeometryMinorRadius", 0.0) * scale;
        let height = t.number("GeometryHeight", 0.0) * scale;
        match t.values("Geometry").first().map(|g| g.to_ascii_uppercase()) {
            Some(g) if g == "BOX" => (
                ih_math::sqrt(major * major + minor * minor),
                ih_math::sqrt(major * major + minor * minor + height * height * 0.25),
                height * 0.5,
            ),
            Some(g) if g == "CYLINDER" => (major, (height * 0.5).max(major), height * 0.5),
            _ => (major, major, 0.0),
        }
    }
    /// `getDistanceSquared(FROM_BOUNDINGSPHERE_2D)` from an object (at `from`, or
    /// where it stands) to another object or a point.
    pub fn attack_distance_sqr(&self, id: u32, from: Option<[f32; 2]>, victim: Option<u32>, pos: [f32; 3]) -> f32 {
        let o = &self.objects[&id];
        let a = from.unwrap_or([o.position[0], o.position[1]]);
        let b = victim.map_or([pos[0], pos[1]], |v| [self.objects[&v].position[0], self.objects[&v].position[1]]);
        let mut rad = Self::bounding(&self.templates.objects[&o.template]).0;
        if let Some(v) = victim {
            rad += Self::bounding(&self.templates.objects[&self.objects[&v].template]).0;
        }
        let d2 = (b[0] - a[0]).powi(2) + (b[1] - a[1]).powi(2);
        if rad <= 0.0 {
            return d2;
        }
        let shrunk = ih_math::sqrt(d2) - rad;
        if shrunk <= 0.0 { 0.0 } else { shrunk * shrunk }
    }
    /// `Weapon::isWithinAttackRange` (`isGoalPosWithinAttackRange` with `from`).
    pub fn within_attack_range(
        &self,
        id: u32,
        slot: usize,
        from: Option<[f32; 2]>,
        victim: Option<u32>,
        pos: [f32; 3],
    ) -> bool {
        let Some(w) = &self.objects[&id].weapon_set.weapons[slot] else { return false };
        let Some(wt) = self.weapon_template(&w.template) else { return false };
        let d2 = self.attack_distance_sqr(id, from, victim, pos);
        if d2 < wt.minimum_range().powi(2) {
            return false;
        }
        // Contact weapons against structures need touching geometry (approximated
        // by the 2D boundary distance being zero).
        if d2 <= wt.attack_range().powi(2) {
            if wt.is_contact()
                && let Some(v) = victim
                && self.templates.objects[&self.objects[&v].template].has_kind("STRUCTURE")
            {
                return d2 <= 0.0;
            }
            return true;
        }
        false
    }

    /// `estimateWeaponDamage` (no containers yet).
    pub(crate) fn estimate_weapon_damage(&self, id: u32, slot: usize, victim: u32) -> f32 {
        let o = &self.objects[&id];
        let Some(w) = &o.weapon_set.weapons[slot] else { return 0.0 };
        let Some(wt) = self.weapon_template(&w.template) else { return 0.0 };
        if w.status == Status::OutOfAmmo && !wt.auto_reload {
            return 0.0;
        }
        let vt = &self.templates.objects[&self.objects[&victim].template];
        if vt.has_kind("SHRUBBERY") {
            return if wt.death_type == 3 { 1.0 } else { 0.0 };
        }
        let disarm = crate::body::damage_type("DISARM").unwrap_or(20);
        if wt.damage_type == disarm {
            return if vt.has_kind("MINE") || vt.has_kind("BOOBY_TRAP") || vt.has_kind("DEMOTRAP") { 1.0 } else { 0.0 };
        }
        if Some(wt.damage_type) == crate::body::damage_type("DEPLOY") && !self.airborne_target(victim) {
            return 1.0;
        }
        self.estimate_damage(victim, wt.damage_type, wt.primary_damage)
    }

    /// `chooseBestWeaponForTarget` (`PREFER_MOST_DAMAGE`, player commands).
    pub(crate) fn choose_weapon(&mut self, id: u32, victim: Option<u32>) -> bool {
        let frame = self.frame;
        if self.objects[&id].weapon_set.lock != 0 {
            return true;
        }
        let Some(victim) = victim else {
            self.objects.get_mut(&id).unwrap().weapon_set.current = 0;
            return true;
        };
        let anti = self.victim_anti_mask(victim);
        let vt = self.templates.objects[&self.objects[&victim].template].clone();
        let (mut found, mut found_backup) = (false, false);
        let (mut best, mut best_backup) = (0.0f32, 0.0f32);
        let (mut decision, mut backup) = (0u8, 0u8);
        let set = {
            let o = &self.objects[&id];
            o.weapon_set.set.map(|s| self.templates.objects[&o.template].weapon_sets[s].clone())
        };
        for i in (0..SLOTS).rev() {
            let Some(set) = &set else { break };
            let Some(mut w) = self.objects[&id].weapon_set.weapons[i].clone() else { continue };
            // CMD_FROM_PLAYER (bit 0); DEFAULT_SWITCH_WEAPON (bit 4) still allows it.
            if set.auto_choose[i] & 1 == 0 && set.auto_choose[i] & (1 << 4) == 0 {
                continue;
            }
            let Some(wt) = self.weapon_template(&w.template).cloned() else { continue };
            let status = w.status(frame);
            if status == Status::OutOfAmmo && !wt.auto_reload {
                continue;
            }
            if wt.anti & anti == 0 {
                continue;
            }
            let mut damage = self.estimate_weapon_damage(id, i, victim);
            let mut ready = status == Status::ReadyToFire;
            if damage <= 0.0 && wt.damage_type != DAMAGE_UNRESISTABLE {
                continue;
            }
            if !set.preferred_against[i].is_empty() && set.preferred_against[i].iter().any(|k| vt.has_kind(k)) {
                damage = 1e10;
                ready = status != Status::OutOfAmmo;
            }
            if !ready {
                if damage >= best_backup {
                    best_backup = damage;
                    backup = i as u8;
                    found_backup = true;
                }
            } else if damage >= best {
                best = damage;
                decision = i as u8;
                found = true;
            }
        }
        let ws = &mut self.objects.get_mut(&id).unwrap().weapon_set;
        if found {
            ws.current = decision;
        } else if found_backup {
            ws.current = backup;
            found = true;
        } else {
            ws.current = 0;
        }
        found
    }

    /// The current weapon's status (refreshed).
    pub(crate) fn current_status(&mut self, id: u32) -> Option<Status> {
        let frame = self.frame;
        let ws = &mut self.objects.get_mut(&id)?.weapon_set;
        let cur = ws.current as usize;
        ws.weapons[cur].as_mut().map(|w| w.status(frame))
    }
    /// `isOutOfAmmo`: no weapon, or all out of ammo.
    pub(crate) fn out_of_ammo(&mut self, id: u32) -> bool {
        let frame = self.frame;
        let ws = &mut self.objects.get_mut(&id).unwrap().weapon_set;
        ws.weapons.iter_mut().flatten().all(|w| w.status(frame) == Status::OutOfAmmo)
    }

    /// `preFireWeapon`.
    pub(crate) fn pre_fire(&mut self, id: u32, victim: Option<u32>) {
        let frame = self.frame;
        let o = self.objects.get_mut(&id).unwrap();
        let cur = o.weapon_set.current as usize;
        let Some(w) = o.weapon_set.weapons[cur].as_mut() else { return };
        let Some(wt) = self.templates.weapons.get(&w.template) else { return };
        // getPreAttackDelay: per shot always, per attack on the first shot at a
        // victim, per clip on a full clip.
        let delay = match wt.pre_attack_type {
            0 => wt.pre_attack_delay,
            1 if w.last_fire_frame == 0 || victim.is_none() => wt.pre_attack_delay,
            2 if wt.clip_size > 0 && w.ammo == wt.clip_size => wt.pre_attack_delay,
            _ => 0,
        };
        if delay > 0 {
            w.status = Status::PreAttack;
            w.pre_attack_finished = frame + delay;
        }
    }

    /// `Weapon::privateFireWeapon` for the current weapon. Returns whether it reloaded.
    pub(crate) fn fire_current_weapon(&mut self, id: u32, victim: Option<u32>, pos: [f32; 3]) -> bool {
        let frame = self.frame;
        let cur = self.objects[&id].weapon_set.current as usize;
        let Some(mut w) = self.objects[&id].weapon_set.weapons[cur].clone() else { return false };
        let Some(wt) = self.weapon_template(&w.template).cloned() else { return false };
        if w.status(frame) != Status::ReadyToFire {
            self.objects.get_mut(&id).unwrap().weapon_set.weapons[cur] = Some(w);
            return false;
        }
        let mut reloaded = false;
        if w.ammo > 0 {
            // One barrel per model until drawables report their FIRE bones.
            if w.barrel >= 1 {
                w.barrel = 0;
                w.shots_for_barrel = wt.shots_per_barrel;
            }
            self.fire_weapon_template(id, &wt, victim, pos, false);
            w.last_fire_frame = frame;
            w.ammo -= 1;
            w.max_shots -= 1;
            w.shots_for_barrel -= 1;
            if w.shots_for_barrel <= 0 {
                w.barrel += 1;
                w.shots_for_barrel = wt.shots_per_barrel;
            }
            if w.ammo <= 0 {
                if wt.auto_reload {
                    // reloadWithBonus
                    w.ammo = if wt.clip_size > 0 { wt.clip_size } else { 0x7fff_ffff };
                    w.status = Status::ReloadingClip;
                    w.last_reload_started = frame;
                    w.fire_again_frame = frame + wt.clip_reload;
                    reloaded = true;
                } else {
                    w.status = Status::OutOfAmmo;
                    w.fire_again_frame = 0x7fff_ffff;
                }
            } else {
                w.status = Status::BetweenFiringShots;
                let delay = if wt.min_delay == wt.max_delay {
                    wt.min_delay as i32
                } else {
                    self.rng.integer(wt.min_delay as i32, wt.max_delay as i32)
                };
                w.last_reload_started = frame;
                w.fire_again_frame = frame + delay.max(0) as u32;
            }
        }
        let (status, again) = (w.status, w.fire_again_frame);
        let shared = self.objects[&id].weapon_set.shared_reload;
        let ws = &mut self.objects.get_mut(&id).unwrap().weapon_set;
        ws.weapons[cur] = Some(w);
        if shared && matches!(status, Status::BetweenFiringShots | Status::ReloadingClip) {
            for other in ws.weapons.iter_mut().flatten() {
                other.fire_again_frame = again;
                other.status = status;
            }
        }
        reloaded
    }

    /// `WeaponTemplate::fireWeaponTemplate` (without FX, OCLs or lasers).
    pub(crate) fn fire_weapon_template(
        &mut self,
        id: u32,
        wt: &WeaponTemplate,
        victim: Option<u32>,
        pos: [f32; 3],
        detonation: bool,
    ) {
        let source = self.objects[&id].position;
        let victim_pos = victim.map_or(pos, |v| self.objects[&v].position);
        let d2 = self.attack_distance_sqr(id, None, victim, victim_pos);
        if d2 > wt.attack_range().powi(2) {
            return;
        }
        if d2 < wt.minimum_range().powi(2) && !detonation {
            return;
        }
        let mut dest = victim_pos;
        let infantry = victim.is_some_and(|v| self.templates.objects[&self.objects[&v].template].has_kind("INFANTRY"));
        if wt.scatter_radius > 0.0 || (wt.infantry_inaccuracy > 0.0 && infantry) {
            let mut scatter = wt.scatter_radius;
            let mut layer = 0;
            if let Some(v) = victim {
                let vo = &self.objects[&v];
                if self.templates.objects[&vo.template].has_kind("STRUCTURE") {
                    dest[2] += Self::bounding(&self.templates.objects[&vo.template]).2;
                }
                if wt.infantry_inaccuracy > 0.0 && infantry {
                    scatter += wt.infantry_inaccuracy;
                }
                layer = vo.layer;
            }
            let r = self.rng.real(0.0, scatter);
            let a = self.rng.real(0.0, 2.0 * std::f32::consts::PI);
            dest[0] += r * ih_math::cos(a);
            dest[1] += r * ih_math::sin(a);
            dest[2] = self.grid.height_at(&self.map, crate::terrain::Waypoint { position: [dest[0], dest[1]], layer });
        }
        // A projectile object flies to its (scattered) destination.
        if wt.projectile.is_some() && !detonation {
            let scattered = wt.scatter_radius > 0.0 || (wt.infantry_inaccuracy > 0.0 && infantry);
            let target = if scattered { None } else { victim };
            if self.launch_projectile(id, wt, target, dest) || self.launch_missile(id, wt, target, dest) {
                return;
            }
        }
        let delay = {
            let v = [victim_pos[0] - source[0], victim_pos[1] - source[1], victim_pos[2] - source[2]];
            ih_math::sqrt(v[0] * v[0] + v[1] * v[1] + v[2] * v[2]) / wt.speed
        };
        let damage_id = if wt.damage_at_self { 0 } else { victim.unwrap_or(0) };
        let damage_pos = if wt.damage_at_self { source } else { victim_pos };
        // A projectile without a dumb-projectile behaviour (missiles, not ported
        // yet) lands after its flight.
        let damage_pos = if wt.projectile.is_some() && !detonation { dest } else { damage_pos };
        if delay < 1.0 {
            self.deal_damage(id, wt, damage_id, damage_pos);
        } else {
            self.delayed_damage.push(DelayedDamage {
                weapon: wt.name.clone(),
                position: damage_pos,
                frame: self.frame + delay.ceil() as u32,
                source: id,
                victim: damage_id,
            });
        }
    }

    /// `WeaponStore::update`: delayed damage that is due.
    pub(crate) fn update_delayed_damage(&mut self) {
        let frame = self.frame;
        let mut i = 0;
        while i < self.delayed_damage.len() {
            if self.delayed_damage[i].frame <= frame {
                let d = self.delayed_damage.remove(i);
                if let Some(wt) = self.weapon_template(&d.weapon).cloned() {
                    self.deal_damage(d.source, &wt, d.victim, d.position);
                }
            } else {
                i += 1;
            }
        }
    }

    /// `WeaponTemplate::dealDamageInternal`.
    fn deal_damage(&mut self, source: u32, wt: &WeaponTemplate, victim: u32, pos: [f32; 3]) {
        if source == 0 || (victim == 0 && pos == [0.0; 3]) {
            return;
        }
        let source_alive = self.objects.contains_key(&source);
        let primary = self.objects.get(&victim).map(|o| o.id);
        let pos = primary.map_or(pos, |v| self.objects[&v].position);
        let radius = wt.primary_radius.max(wt.secondary_radius);
        // A projectile's damage is credited to its launcher.
        let credit = self.objects.get(&source).map_or(source, |o| {
            o.flight.as_ref().map(|f| f.launcher).or(o.missile.as_ref().map(|m| m.launcher)).unwrap_or(source)
        });
        let info = |amount: f32| DamageInfo::new(credit, wt.damage_type, wt.death_type, amount);
        let victims: Vec<(u32, f32)> = if radius > 0.0 {
            // iterateObjectsInRange(FROM_BOUNDINGSPHERE_3D), in object order.
            self.objects
                .values()
                .filter_map(|o| {
                    let (_, sphere, dz) = Self::bounding(&self.templates.objects[&o.template]);
                    let d = [o.position[0] - pos[0], o.position[1] - pos[1], o.position[2] + dz - pos[2]];
                    let dist = ih_math::sqrt(d[0] * d[0] + d[1] * d[1] + d[2] * d[2]) - sphere;
                    let d2 = if dist <= 0.0 { 0.0 } else { dist * dist };
                    (d2 <= radius * radius).then_some((o.id, d2))
                })
                .collect()
        } else {
            if wt.affects & KILLS_SELF != 0 && source_alive {
                self.attempt_damage(source, info(HUGE_DAMAGE_AMOUNT));
                return;
            }
            primary.into_iter().map(|v| (v, 0.0)).collect()
        };
        for (v, d2) in victims {
            if !self.objects.contains_key(&v) {
                continue;
            }
            let mut kill_self = false;
            if source_alive && Some(v) != primary {
                if wt.affects & KILLS_SELF != 0 && v == source {
                    kill_self = true;
                } else {
                    if wt.affects & AFFECTS_SELF == 0 && v == source {
                        continue;
                    }
                    if wt.affects & DOESNT_AFFECT_SIMILAR != 0
                        && self.objects[&v].template == self.objects[&source].template
                        && self.relationship(source, v) == Relationship::Allies
                    {
                        continue;
                    }
                    if wt.affects & DOESNT_AFFECT_AIRBORNE != 0 && self.significantly_above_terrain(v) {
                        continue;
                    }
                    let required = match self.relationship(v, source) {
                        Relationship::Allies => AFFECTS_ALLIES,
                        Relationship::Enemies => AFFECTS_ENEMIES,
                        Relationship::Neutral => AFFECTS_NEUTRALS,
                    };
                    if wt.affects & required == 0 {
                        continue;
                    }
                }
            }
            if wt.radius_damage_angle < std::f32::consts::PI {
                if !source_alive {
                    continue;
                }
                let (s, o) = (&self.objects[&source], &self.objects[&v]);
                let d = [o.position[0] - s.position[0], o.position[1] - s.position[1], o.position[2] - s.position[2]];
                let len = ih_math::sqrt(d[0] * d[0] + d[1] * d[1] + d[2] * d[2]);
                let facing = [ih_math::cos(s.angle), ih_math::sin(s.angle)];
                let dot = if len > 0.0 { (facing[0] * d[0] + facing[1] * d[1]) / len } else { 0.0 };
                if dot < ih_math::cos(wt.radius_damage_angle) {
                    continue;
                }
            }
            let amount = if kill_self {
                HUGE_DAMAGE_AMOUNT
            } else if d2 <= wt.primary_radius * wt.primary_radius {
                wt.primary_damage
            } else {
                wt.secondary_damage
            };
            self.attempt_damage(v, info(amount));
        }
    }
}
