//! Object creation lists (`ObjectCreationList`) and `LifetimeUpdate`.
//!
//! Sources: `GameLogic/Object/ObjectCreationList.cpp` (`GenericObjectCreationNugget`
//! for `CreateObject` and `CreateDebris`, `ApplyRandomForceNugget`,
//! `FireWeaponNugget`, `AttackNugget`, `calcRandomForce`, `createInternal`),
//! `GameLogic/Object/Update/LifetimeUpdate.cpp`, `CreateObjectDie.cpp`,
//! `ObjectCreationUpgrade.cpp`, `SlowDeathBehavior::doPhaseStuff` and the OCL
//! half of `WeaponTemplate::fireWeaponTemplate`.
//!
//! Not ported yet: `DeliverPayload` (aircraft deliveries), debris models,
//! animations and particle systems (client), bouncing and spin (the rates are
//! drawn but not kept), floating, invulnerability, slaved objects, ignoring
//! collisions with the source, fades, and `RequiresLivePlayer` (players are
//! always alive). As in the original, an `ObjectNames`/`ModelNames` list keeps
//! only every other name (its parser reads two tokens per name).
use crate::Simulation;

const LIKE_EXISTING: u32 = 1;
const ON_GROUND_ALIGNED: u32 = 2;
const SEND_IT_FLYING: u32 = 4;
const SEND_IT_UP: u32 = 8;
const SEND_IT_OUT: u32 = 0x10;
const RANDOM_FORCE: u32 = 0x20;
const INHERIT_VELOCITY: u32 = 0x80;
const WHIRLING: u32 = 0x100;
const DISPOSITIONS: [&str; 9] = [
    "LIKE_EXISTING",
    "ON_GROUND_ALIGNED",
    "SEND_IT_FLYING",
    "SEND_IT_UP",
    "SEND_IT_OUT",
    "RANDOM_FORCE",
    "FLOATING",
    "INHERIT_VELOCITY",
    "WHIRLING",
];

/// `GenericObjectCreationNugget`.
#[derive(Clone, Debug, PartialEq)]
pub struct Generic {
    pub names: Vec<String>,
    /// `CreateObject` (else `CreateDebris`, made as `GenericDebris`).
    pub objects: bool,
    pub count: i32,
    pub offset: [f32; 3],
    pub disposition: u32,
    pub intensity: f32,
    pub spin: f32,
    pub yaw: f32,
    pub roll: f32,
    pub pitch: f32,
    pub min_mag: f32,
    pub max_mag: f32,
    pub min_pitch: f32,
    pub max_pitch: f32,
    pub min_life: u32,
    pub max_life: u32,
    pub put_in_container: Option<String>,
    pub contain_inside_source: bool,
    pub inherits_veterancy: bool,
    pub skip_if_airborne: bool,
    pub min_health: f32,
    pub max_health: f32,
    pub spread: bool,
    pub min_a: f32,
    pub min_b: f32,
    pub max_dist: f32,
    pub preserve_layer: bool,
    pub mass: f32,
    pub extra_friction: f32,
    pub orient_in_force: bool,
    pub dies_on_bad_land: bool,
    pub anim_sets: i32,
}

/// One entry of an object creation list.
#[derive(Clone, Debug, PartialEq)]
pub enum Nugget {
    Create(Box<Generic>),
    ApplyRandomForce {
        spin: f32,
        min_mag: f32,
        max_mag: f32,
        min_pitch: f32,
        max_pitch: f32,
    },
    FireWeapon(String),
    Attack {
        shots: i32,
    },
    /// Not ported (`DeliverPayload`).
    Other,
}

fn real(v: &[String]) -> f32 {
    v.first().and_then(|s| ih_ini::values::scan_real(s.trim_end_matches('%').as_bytes()).ok()).unwrap_or(0.0)
}
fn yes(v: &[String]) -> bool {
    v.first().is_some_and(|s| s.eq_ignore_ascii_case("yes"))
}

/// Parses an `ObjectCreationList` block into its nuggets.
pub fn parse(b: &ih_ini::Block) -> Vec<Nugget> {
    let mut out = vec![];
    for n in b.blocks() {
        let fields: Vec<(String, Vec<String>)> = n.fields().map(|f| (f.name.clone(), f.values())).collect();
        let get = |k: &str| fields.iter().rev().find(|(n, _)| n.eq_ignore_ascii_case(k)).map(|(_, v)| v.clone());
        let deg = |k: &str| get(k).map_or(0.0, |v| real(&v).to_radians());
        let per_sec = |k: &str, d: f32| get(k).map_or(d, |v| real(&v).to_radians() / 30.0);
        match n.kind.as_str() {
            "CreateObject" | "CreateDebris" => {
                let objects = n.kind == "CreateObject";
                let list = get(if objects { "ObjectNames" } else { "ModelNames" }).unwrap_or_default();
                let disposition = get("Disposition").map_or(ON_GROUND_ALIGNED, |v| {
                    v.iter()
                        .filter_map(|d| DISPOSITIONS.iter().position(|n| n.eq_ignore_ascii_case(d)))
                        .fold(0, |m, i| m | 1 << i)
                });
                let pct = |k: &str| get(k).map_or(1.0, |v| real(&v) / 100.0);
                let life = |k: &str| get(k).map_or(0, |v| crate::death::duration(real(&v)));
                let coord = get("Offset").map_or([0.0; 3], |_| {
                    let raw = n.fields().filter(|f| f.name.eq_ignore_ascii_case("Offset")).last().unwrap().rest.clone();
                    match ih_ini::values::parse_value(
                        &ih_ini::schema::FieldDef { name: None, parser: "INI::parseCoord3D", user: "NULL" },
                        &raw,
                    ) {
                        Ok(ih_ini::values::Value::Coord3(c)) => c,
                        _ => [0.0; 3],
                    }
                });
                out.push(Nugget::Create(Box::new(Generic {
                    // parseDebrisObjectNames takes every other token.
                    names: list.iter().step_by(2).cloned().collect(),
                    objects,
                    count: get("Count").map_or(1, |v| real(&v) as i32),
                    offset: coord,
                    disposition,
                    intensity: get("DispositionIntensity").map_or(0.0, |v| real(&v)),
                    spin: per_sec("SpinRate", -1.0),
                    yaw: per_sec("YawRate", -1.0),
                    roll: per_sec("RollRate", -1.0),
                    pitch: per_sec("PitchRate", -1.0),
                    min_mag: get("MinForceMagnitude").map_or(0.0, |v| real(&v)),
                    max_mag: get("MaxForceMagnitude").map_or(0.0, |v| real(&v)),
                    min_pitch: deg("MinForcePitch"),
                    max_pitch: deg("MaxForcePitch"),
                    min_life: life("MinLifetime"),
                    max_life: life("MaxLifetime"),
                    put_in_container: get("PutInContainer").and_then(|v| v.first().cloned()),
                    contain_inside_source: get("ContainInsideSourceObject").is_some_and(|v| yes(&v)),
                    inherits_veterancy: get("InheritsVeterancy").is_some_and(|v| yes(&v)),
                    skip_if_airborne: get("SkipIfSignificantlyAirborne").is_some_and(|v| yes(&v)),
                    min_health: pct("MinHealth"),
                    max_health: pct("MaxHealth"),
                    spread: get("SpreadFormation").is_some_and(|v| yes(&v)),
                    min_a: get("MinDistanceAFormation").map_or(0.0, |v| real(&v)),
                    min_b: get("MinDistanceBFormation").map_or(0.0, |v| real(&v)),
                    max_dist: get("MaxDistanceFormation").map_or(0.0, |v| real(&v)),
                    preserve_layer: get("PreserveLayer").is_none_or(|v| yes(&v)),
                    mass: get("Mass").map_or(0.0, |v| real(&v)),
                    // parseFrictionPerSec
                    extra_friction: get("ExtraFriction").map_or(0.0, |v| real(&v) / 30.0),
                    orient_in_force: get("OrientInForceDirection").is_some_and(|v| yes(&v)),
                    dies_on_bad_land: get("DiesOnBadLand").is_some_and(|v| yes(&v)),
                    anim_sets: fields.iter().filter(|(n, _)| n.eq_ignore_ascii_case("AnimationSet")).count() as i32,
                })));
            }
            "ApplyRandomForce" => out.push(Nugget::ApplyRandomForce {
                spin: per_sec("SpinRate", 0.0),
                min_mag: get("MinForceMagnitude").map_or(0.0, |v| real(&v)),
                max_mag: get("MaxForceMagnitude").map_or(0.0, |v| real(&v)),
                min_pitch: deg("MinForcePitch"),
                max_pitch: deg("MaxForcePitch"),
            }),
            "FireWeapon" => {
                out.push(Nugget::FireWeapon(get("Weapon").and_then(|v| v.first().cloned()).unwrap_or_default()))
            }
            "Attack" => out.push(Nugget::Attack { shots: get("NumberOfShots").map_or(1, |v| real(&v) as i32) }),
            _ => out.push(Nugget::Other),
        }
    }
    out
}

/// `LifetimeUpdate`'s `MinLifetime`/`MaxLifetime` in frames.
pub fn lifetime_range(b: &ih_ini::Block) -> (u32, u32) {
    let mut r = (0, 0);
    for f in b.fields() {
        let v = real(&f.values());
        match f.name.as_str() {
            "MinLifetime" => r.0 = crate::death::duration(v),
            "MaxLifetime" => r.1 = crate::death::duration(v),
            _ => {}
        }
    }
    r
}

impl Simulation {
    /// `LifetimeUpdate::calcSleepDelay`: when the object dies.
    pub(crate) fn set_lifetime(&mut self, id: u32, min: u32, max: u32) {
        let Some(i) = self.objects[&id].modules.iter().position(|m| m.class == "LifetimeUpdate") else { return };
        let delay = (self.rng.integer(min as i32, max as i32) as u32).max(1);
        self.objects.get_mut(&id).unwrap().die_frame = Some(self.frame + delay);
        self.scheduler.reschedule((id, i), self.frame + delay);
    }
    /// `LifetimeUpdate::update`.
    pub(crate) fn lifetime_update(&mut self, id: u32) -> u32 {
        let death = crate::body::death_type("NORMAL").unwrap_or(0);
        self.kill(id, crate::body::DAMAGE_UNRESISTABLE, death);
        crate::scheduler::FOREVER
    }

    /// `ObjectCreationList::create(ocl, primary, secondary)`: from an object.
    pub fn create_ocl(&mut self, name: &str, primary: u32, secondary: Option<u32>) -> Option<u32> {
        let nuggets = self.templates.ocls.get(name)?.clone();
        let mut first = None;
        for n in &nuggets {
            if !self.objects.contains_key(&primary) {
                break;
            }
            let made = match n {
                Nugget::Create(g) => {
                    if g.skip_if_airborne && self.significantly_above_terrain(primary) {
                        None
                    } else {
                        let o = &self.objects[&primary];
                        let (p, a) = (o.position, o.angle);
                        self.really_create(g, p, Some(a), a, Some(primary))
                    }
                }
                Nugget::ApplyRandomForce { spin, min_mag, max_mag, min_pitch, max_pitch } => {
                    let f = self.random_force(*min_mag, *max_mag, *min_pitch, *max_pitch);
                    self.apply_force_to(primary, f);
                    for _ in 0..3 {
                        let _ = self.rng.real(-spin, *spin);
                    }
                    None
                }
                // The positional nuggets, with the objects' positions.
                Nugget::FireWeapon(_) | Nugget::Attack { .. } => {
                    let pos = self.objects[&primary].position;
                    let sec = secondary.and_then(|s| self.objects.get(&s)).map(|o| o.position);
                    self.positional_nugget(n, primary, pos, sec)
                }
                Nugget::Other => None,
            };
            first = first.or(made);
        }
        first
    }
    /// `ObjectCreationList::create(ocl, primaryObj, pos, secondary, angle)`: at a place.
    pub fn create_ocl_at(
        &mut self,
        name: &str,
        primary: u32,
        pos: [f32; 3],
        secondary: Option<[f32; 3]>,
        angle: Option<f32>,
    ) -> Option<u32> {
        let nuggets = self.templates.ocls.get(name)?.clone();
        let mut first = None;
        for n in &nuggets {
            let made = match n {
                Nugget::Create(g) => self.really_create(g, pos, None, angle.unwrap_or(0.0), Some(primary)),
                _ => self.positional_nugget(n, primary, pos, secondary),
            };
            first = first.or(made);
        }
        first
    }
    fn positional_nugget(
        &mut self,
        n: &Nugget,
        primary: u32,
        _pos: [f32; 3],
        secondary: Option<[f32; 3]>,
    ) -> Option<u32> {
        let sec = secondary?;
        if !self.objects.contains_key(&primary) {
            return None;
        }
        match n {
            // createAndFireTempWeapon at the secondary position.
            Nugget::FireWeapon(w) => {
                if let Some(wt) = self.weapon_template(w).cloned() {
                    self.fire_weapon_template(primary, &wt, None, sec, false);
                }
            }
            Nugget::Attack { .. } => {
                let _ = self.begin_attack(primary, None, sec);
                self.objects.get_mut(&primary).unwrap().ai.cmd_from_ai = true;
            }
            _ => {}
        }
        None
    }

    /// `calcRandomForce`.
    fn random_force(&mut self, min_mag: f32, max_mag: f32, min_pitch: f32, max_pitch: f32) -> [f32; 3] {
        let angle = self.rng.real(0.0, 2.0 * std::f32::consts::PI);
        let pitch = self.rng.real(min_pitch, max_pitch);
        let mag = self.rng.real(min_mag, max_mag);
        let (ca, sa, cp, sp) = (ih_math::cos(angle), ih_math::sin(angle), ih_math::cos(pitch), ih_math::sin(pitch));
        [mag * ca * cp, mag * sa * cp, mag * sp]
    }

    /// `GenericObjectCreationNugget::reallyCreate`. `mtx_angle` is the primary's
    /// facing when created from an object (its transform).
    fn really_create(
        &mut self,
        g: &Generic,
        pos: [f32; 3],
        mtx_angle: Option<f32>,
        orientation: f32,
        source: Option<u32>,
    ) -> Option<u32> {
        if g.names.is_empty() {
            return None;
        }
        let owner = source.and_then(|s| self.objects.get(&s)).map_or(0, |o| o.owner);
        let mut first = None;
        let container = match &g.put_in_container {
            Some(c) if self.templates.objects.contains_key(c) => {
                let id = self.create_object(c, pos, 0.0, 0, owner).ok()?;
                first = Some(id);
                Some(id)
            }
            _ => None,
        };
        for _ in 0..g.count {
            let pick = self.rng.integer(0, g.names.len() as i32 - 1) as usize;
            let tmpl = if g.objects { g.names[pick].clone() } else { "GenericDebris".to_string() };
            if !self.templates.objects.contains_key(&tmpl) {
                continue;
            }
            let Ok(obj) = self.create_object(&tmpl, pos, 0.0, 0, owner) else { continue };
            first = first.or(Some(obj));
            if g.preserve_layer
                && container.is_none()
                && let Some(l) = source.and_then(|s| self.objects.get(&s)).map(|o| o.layer).filter(|l| *l != 0)
            {
                self.objects.get_mut(&obj).unwrap().layer = l;
            }
            if let Some(c) = container
                && self.objects[&c].contain.is_some()
                && self.can_enter_object(obj, c, true)
            {
                self.add_to_contain(c, obj);
            }
            let at = if g.spread {
                let min = self.rng.real(g.min_a, g.min_b);
                self.find_position_around(pos, min, g.max_dist, None, None, 1e10).unwrap_or(pos)
            } else {
                pos
            };
            self.do_stuff_to_obj(g, obj, at, mtx_angle, orientation, source);
        }
        if let Some(c) = container {
            self.do_stuff_to_obj(g, c, pos, mtx_angle, orientation, source);
        }
        first
    }

    /// `GenericObjectCreationNugget::doStuffToObj`.
    fn do_stuff_to_obj(
        &mut self,
        g: &Generic,
        obj: u32,
        pos: [f32; 3],
        mtx_angle: Option<f32>,
        mut orientation: f32,
        source: Option<u32>,
    ) {
        if g.max_life > 0 {
            self.set_lifetime(obj, g.min_life, g.max_life);
        }
        if !g.objects && g.anim_sets > 0 {
            let _ = self.rng.integer(0, g.anim_sets - 1);
        }
        let mut offset = g.offset;
        if let Some(a) = mtx_angle {
            let (c, s) = (ih_math::cos(a), ih_math::sin(a));
            offset = [c * offset[0] - s * offset[1], s * offset[0] + c * offset[1], offset[2]];
        }
        let mut chunk = [pos[0] + offset[0], pos[1] + offset[1], pos[2] + offset[2]];
        // setInitialHealth(percent)
        let pct = self.rng.real(g.min_health, g.max_health);
        if let Some(h) = self.objects.get_mut(&obj).unwrap().health.as_mut() {
            h.current = (h.max * pct).min(h.max);
            h.previous = h.current;
        }
        if g.inherits_veterancy
            && let Some(level) = source.and_then(|s| self.objects.get(&s)).map(|o| o.experience.level)
        {
            let _ = self.set_veterancy(obj, level);
        }
        let d = g.disposition;
        if d & INHERIT_VELOCITY != 0
            && let Some(v) = source.and_then(|s| self.objects.get(&s)).map(|o| o.physics.vel)
        {
            self.apply_force_to(obj, v);
        }
        if d & LIKE_EXISTING != 0 {
            let o = self.objects.get_mut(&obj).unwrap();
            o.angle = mtx_angle.unwrap_or(orientation);
            o.position = chunk;
            if self.templates.objects[&self.objects[&obj].template].has_kind("STRUCTURE") {
                let z = self.map.ground_height(pos[0], pos[1]);
                self.objects.get_mut(&obj).unwrap().position[2] = z;
                self.add_to_pathfind_map(obj);
            }
        }
        if d & ON_GROUND_ALIGNED != 0 {
            let dest = self.grid.destination(&self.map, [chunk[0], chunk[1]]);
            let a = self.rng.real(0.0, 2.0 * std::f32::consts::PI);
            let mut z = self.grid.height_at(&self.map, dest);
            if dest.layer != 0 {
                z += 1.0;
            }
            chunk[2] = z;
            let o = self.objects.get_mut(&obj).unwrap();
            o.angle = a;
            o.layer = dest.layer;
            o.position = chunk;
        }
        if d & SEND_IT_OUT != 0 {
            let a = self.rng.real(0.0, 2.0 * std::f32::consts::PI);
            chunk[2] = self.map.ground_height(chunk[0], chunk[1]);
            let o = self.objects.get_mut(&obj).unwrap();
            o.angle = a;
            o.position = chunk;
            if !g.objects {
                o.physics.data.mass = g.mass.max(f32::MIN_POSITIVE);
            }
            o.physics.extra_friction = g.extra_friction;
            let h = 4.0 * g.intensity;
            let force = [self.rng.real(-h, h), self.rng.real(-h, h), 0.0];
            self.apply_force_to(obj, force);
            if g.orient_in_force {
                orientation = ih_math::atan2(force[1], force[0]);
            }
        }
        if d & (SEND_IT_FLYING | SEND_IT_UP | RANDOM_FORCE) != 0 {
            let o = self.objects.get_mut(&obj).unwrap();
            if let Some(a) = mtx_angle {
                o.angle = a;
            }
            o.position = chunk;
            if !g.objects {
                o.physics.data.mass = g.mass.max(f32::MIN_POSITIVE);
            }
            o.physics.extra_friction = g.extra_friction;
            let spin = if g.spin >= 0.0 { g.spin } else { std::f32::consts::PI / 32.0 * g.intensity };
            let rates = [g.yaw, g.roll, g.pitch].map(|r| if r >= 0.0 { r } else { spin });
            for r in rates {
                let _ = self.rng.real(-r, r);
            }
            let force = if d & SEND_IT_FLYING != 0 {
                let (h, v) = (4.0 * g.intensity, 3.0 * g.intensity);
                [self.rng.real(-h, h), self.rng.real(-h, h), self.rng.real(v * 0.33, v)]
            } else if d & SEND_IT_UP != 0 {
                let (h, v) = (2.0 * g.intensity, 4.0 * g.intensity);
                [self.rng.real(-h, h), self.rng.real(-h, h), self.rng.real(v * 0.75, v)]
            } else {
                self.random_force(g.min_mag, g.max_mag, g.min_pitch, g.max_pitch)
            };
            self.apply_force_to(obj, force);
            if g.orient_in_force {
                orientation = ih_math::atan2(force[1], force[0]);
            }
            // setAngles(orientation, 0, 0)
            self.objects.get_mut(&obj).unwrap().angle = orientation;
        }
        if d & WHIRLING != 0 {
            for _ in 0..3 {
                let _ = self.rng.real(-g.intensity, g.intensity);
            }
        }
        if g.contain_inside_source
            && let Some(s) = source
        {
            if self.objects.get(&s).is_some_and(|o| o.contain.is_some()) && self.can_enter_object(obj, s, true) {
                self.add_to_contain(s, obj);
            } else {
                self.destroy_object(obj);
            }
        }
        if g.dies_on_bad_land && self.objects.contains_key(&obj) {
            use crate::footprint::CellType;
            let p = self.objects[&obj].position;
            let cell =
                self.grid.ground_kind((p[0] / crate::navigation::CELL) as i32, (p[1] / crate::navigation::CELL) as i32);
            if matches!(cell, None | Some(CellType::Cliff | CellType::Water | CellType::Impassable)) {
                let death = crate::body::death_type("NORMAL").unwrap_or(0);
                self.kill(obj, crate::body::DAMAGE_UNRESISTABLE, death);
            }
        }
        self.update_pos(obj);
    }

    /// `PhysicsBehavior::applyForce` (`Body::apply_force` outside a physics update).
    pub(crate) fn apply_force_to(&mut self, id: u32, f: [f32; 3]) {
        let frame = self.frame;
        let Some(o) = self.objects.get_mut(&id) else { return };
        if f.iter().any(|v| v.is_nan()) {
            return;
        }
        let mut m = f;
        if o.physics.motive_expires > frame {
            let d = [ih_math::cos(o.angle), ih_math::sin(o.angle)];
            let lateral = f[0] * -d[1] + f[1] * d[0];
            m[0] = lateral * -d[1];
            m[1] = lateral * d[0];
        }
        let inv = 1.0 / o.physics.data.mass;
        for (a, v) in o.physics.accel.iter_mut().zip(m) {
            *a += v * inv;
        }
        o.physics.woken = true;
    }
}
