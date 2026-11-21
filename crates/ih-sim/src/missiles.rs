//! Missiles, from `MissileAIUpdate.cpp` and the `THRUST` locomotor
//! (`Locomotor::moveTowardsPositionThrust`, `calcDirectionToApplyThrust`,
//! `tryToRotateVector3D`, `tryToOrientInThisDirection3D`,
//! `PhysicsBehavior::getForwardSpeed3D`).
//!
//! A missile leaves its launcher with an initial push along the launch
//! direction (raised for targets above), coasts until its ignition delay
//! passes, ignites (arming it and starting its fuel), flies straight until it
//! has travelled `DistanceToTravelBeforeTurning`, then steers: its thrust points
//! where it best cancels the velocity error towards the goal, within the
//! locomotor's thrust angle, and its body turns along its velocity. Within the
//! lock distance it cheats straight at the target. It detonates on touching
//! something its weapon collides with, on reaching the ground near its goal,
//! when out of fuel (if it detonates then), and destroys itself when a tracked
//! target is gone.
//!
//! Not yet: launch bones and the launcher's velocity (missiles start at the
//! launcher's position facing its way), ignition FX and exhaust, countermeasure
//! decoys and jamming, garrison hits, bridge layers, dive heights, and the
//! physics of a missile is its own simple flight integration (gravity,
//! aerodynamic friction, motive forces) rather than the full `PhysicsBehavior`.
use crate::{
    Simulation,
    body::{DAMAGE_UNRESISTABLE, DamageInfo},
    death::duration,
    templates::BIGNUM,
    weapons::WeaponTemplate,
};

/// `APPROACH_HEIGHT`.
const APPROACH_HEIGHT: f32 = 10.0;

/// `MissileAIUpdateModuleData`.
#[derive(Clone, Debug, PartialEq)]
pub struct MissileData {
    pub follow_target: bool,
    pub fuel_lifetime: u32,
    pub ignition_delay: u32,
    /// Per frame.
    pub initial_vel: f32,
    pub initial_dist: f32,
    pub dive_distance: f32,
    pub use_weapon_speed: bool,
    pub detonate_on_no_fuel: bool,
    pub lock_distance: f32,
    pub detonate_calls_kill: bool,
    pub kill_self_delay: u32,
}
impl MissileData {
    pub fn parse(block: &ih_ini::Block) -> Self {
        let mut d = Self {
            follow_target: true,
            fuel_lifetime: 0,
            ignition_delay: 0,
            initial_vel: 0.0,
            initial_dist: 0.0,
            dive_distance: 0.0,
            use_weapon_speed: false,
            detonate_on_no_fuel: false,
            lock_distance: 75.0,
            detonate_calls_kill: false,
            kill_self_delay: 3,
        };
        for f in block.fields() {
            let v = f.values();
            let real = || v.first().and_then(|s| ih_ini::values::scan_real(s.as_bytes()).ok()).unwrap_or(0.0);
            let yes = || v.first().is_some_and(|s| matches!(s.to_ascii_lowercase().as_str(), "yes" | "true" | "1"));
            match f.name.as_str() {
                "TryToFollowTarget" => d.follow_target = yes(),
                "FuelLifetime" => d.fuel_lifetime = duration(real()),
                "IgnitionDelay" => d.ignition_delay = duration(real()),
                "InitialVelocity" => d.initial_vel = real() * (1.0 / 30.0),
                "DistanceToTravelBeforeTurning" => d.initial_dist = real(),
                "DistanceToTargetBeforeDiving" => d.dive_distance = real(),
                "DistanceToTargetForLock" => d.lock_distance = real(),
                "UseWeaponSpeed" => d.use_weapon_speed = yes(),
                "DetonateOnNoFuel" => d.detonate_on_no_fuel = yes(),
                "DetonateCallsKill" => d.detonate_calls_kill = yes(),
                "KillSelfDelay" => d.kill_self_delay = duration(real()),
                _ => {}
            }
        }
        d
    }
}

/// `MissileStateType`.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
#[repr(u8)]
pub enum MissileState {
    #[default]
    Prelaunch = 0,
    Launch = 1,
    Ignition = 2,
    AttackNoTurn = 3,
    Attack = 4,
    Dead = 5,
    Kill = 6,
    KillSelf = 7,
}

/// A missile's `MissileAIUpdate` state plus its locomotor overrides.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Missile {
    pub state: MissileState,
    pub state_frame: u32,
    pub launcher: u32,
    pub victim: u32,
    pub weapon: String,
    pub armed: bool,
    pub fuel_expires: u32,
    pub no_turn_dist_left: f32,
    pub max_accel: f32,
    pub original_target: [f32; 3],
    pub prev_pos: [f32; 3],
    pub tracking: bool,
    /// The move goal (`aiMoveToObject` follows `goal_object`).
    pub goal: [f32; 3],
    pub goal_object: u32,
    /// `setMaxAcceleration`, `setMaxTurnRate` and `setMaxSpeed` on the locomotor.
    pub loco_accel: f32,
    pub loco_turn: f32,
    pub loco_speed: Option<f32>,
    pub braking: bool,
    /// Missile flight: velocity, and pitch (the body's yaw is the object's angle).
    pub vel: [f32; 3],
    pub accel: [f32; 3],
    pub pitch: f32,
    pub motive_until: u32,
}

type V3 = [f32; 3];
fn sub(a: V3, b: V3) -> V3 {
    [a[0] - b[0], a[1] - b[1], a[2] - b[2]]
}
fn dot(a: V3, b: V3) -> f32 {
    a[0] * b[0] + a[1] * b[1] + a[2] * b[2]
}
fn len(a: V3) -> f32 {
    ih_math::sqrt(dot(a, a))
}
fn norm(a: V3) -> V3 {
    let l = len(a);
    if l > 0.0 { [a[0] / l, a[1] / l, a[2] / l] } else { a }
}
fn cross(a: V3, b: V3) -> V3 {
    [a[1] * b[2] - a[2] * b[1], a[2] * b[0] - a[0] * b[2], a[0] * b[1] - a[1] * b[0]]
}
fn nearly_zero(v: f32) -> bool {
    v.abs() < 0.0001
}
/// The X axis of a yaw/pitch transform.
fn forward(yaw: f32, pitch: f32) -> V3 {
    let cp = ih_math::cos(pitch);
    [cp * ih_math::cos(yaw), cp * ih_math::sin(yaw), ih_math::sin(pitch)]
}
/// `buildTransformMatrix(pos, dir)`: yaw and pitch facing `dir`.
fn orient(dir: V3) -> (f32, f32) {
    let h = ih_math::sqrt(dir[0] * dir[0] + dir[1] * dir[1]);
    (ih_math::atan2(dir[1], dir[0]), ih_math::atan2(dir[2], h))
}
/// `tryToRotateVector3D`.
fn rotate_towards(max_angle: f32, cur: V3, goal: V3) -> (V3, f32) {
    if nearly_zero(max_angle) {
        return (cur, 0.0);
    }
    let (c, g) = (norm(cur), norm(goal));
    let mut between = ih_math::acos(dot(c, g).clamp(-1.0, 1.0));
    let mut max_angle = max_angle;
    if max_angle < 0.0 {
        max_angle = -max_angle * between;
        if nearly_zero(max_angle) {
            return (cur, 0.0);
        }
    }
    if between.abs() <= max_angle {
        return (g, between);
    }
    // Rodrigues' rotation of `c` about the normalised cross product.
    let axis = norm(cross(c, g));
    between = max_angle;
    let (s, co) = (ih_math::sin(between), ih_math::cos(between));
    let k = cross(axis, c);
    let kd = dot(axis, c);
    let out = [
        c[0] * co + k[0] * s + axis[0] * kd * (1.0 - co),
        c[1] * co + k[1] * s + axis[1] * kd * (1.0 - co),
        c[2] * co + k[2] * s + axis[2] * kd * (1.0 - co),
    ];
    (out, between)
}

impl Simulation {
    fn missile_data(&self, id: u32) -> Option<MissileData> {
        let o = self.objects.get(&id)?;
        let t = &self.templates.objects[&o.template];
        t.modules.iter().find(|m| m.class == "MissileAIUpdate").map(|m| MissileData::parse(&m.block))
    }

    /// `projectileLaunchAtObjectOrPosition` / `projectileFireAtObjectOrPosition`.
    /// False when the projectile is no missile.
    pub(crate) fn launch_missile(
        &mut self,
        launcher: u32,
        wt: &WeaponTemplate,
        victim: Option<u32>,
        dest: [f32; 3],
    ) -> bool {
        let Some(name) = &wt.projectile else { return false };
        let Some(t) = self.templates.objects.get(name) else { return false };
        if !t.modules.iter().any(|m| m.class == "MissileAIUpdate") {
            return false;
        }
        let lo = &self.objects[&launcher];
        let (owner, mut start, layer, yaw) = (lo.owner, lo.position, lo.layer, lo.angle);
        // No launch bones yet: the top of the launcher's geometry stands in.
        start[2] += self.templates.objects[&lo.template].number("GeometryHeight", 0.0)
            * self.templates.objects[&lo.template].number("Scale", 1.0);
        let Ok(id) = self.create_object(name, start, yaw, layer, owner) else { return false };
        let d = self.missile_data(id).unwrap();
        let mut m = Missile {
            launcher,
            weapon: wt.name.clone(),
            no_turn_dist_left: d.initial_dist,
            max_accel: BIGNUM,
            prev_pos: start,
            state_frame: self.frame,
            ..Default::default()
        };
        let mut initial = d.initial_vel;
        if d.use_weapon_speed {
            initial = wt.speed;
            m.max_accel = wt.speed;
            m.loco_speed = Some(wt.speed);
            m.loco_accel = m.max_accel;
        }
        let target_pos = victim.map_or(dest, |v| self.objects[&v].position);
        let delta_z = target_pos[2] - start[2];
        let xy = ih_math::sqrt((target_pos[0] - start[0]).powi(2) + (target_pos[1] - start[1]).powi(2)).max(1.0);
        let z_factor = if delta_z > 0.0 { delta_z / xy } else { 0.0 };
        let mut dir = forward(yaw, 0.0);
        dir[2] += 2.0 * z_factor;
        let dir = norm(dir);
        let mass = self.templates.objects[name].physics().unwrap_or_default().mass;
        if initial > 0.0 {
            // applyMotiveForce of mass * initial velocity.
            for i in 0..3 {
                m.accel[i] += (mass * initial * dir[i]) / mass;
            }
            m.motive_until = self.frame + crate::locomotion::MOTIVE_FRAMES;
        }
        let (y, p) = orient(dir);
        m.pitch = p;
        m.state = MissileState::Launch;
        if let Some(v) = victim.filter(|_| d.follow_target) {
            m.goal = target_pos;
            m.goal_object = v;
            m.original_target = target_pos;
            m.tracking = true;
            m.victim = v;
        } else {
            let mut initial_pos = dest;
            m.original_target = dest;
            if d.lock_distance > 0.0 {
                initial_pos[2] += APPROACH_HEIGHT;
            }
            m.goal = initial_pos;
        }
        m.prev_pos = start;
        let o = self.objects.get_mut(&id).unwrap();
        o.angle = y;
        o.missile = Some(m);
        true
    }

    fn switch_missile(m: &mut Missile, s: MissileState, frame: u32) {
        if m.state != s {
            m.state = s;
            m.state_frame = frame;
        }
    }

    /// `MissileAIUpdate::update`, then the AI's move towards the goal.
    pub(crate) fn missile_update(&mut self, id: u32) -> u32 {
        let Some(d) = self.missile_data(id) else { return crate::scheduler::FOREVER };
        let Some(mut m) = self.objects[&id].missile.clone() else { return 1 };
        let frame = self.frame;
        let pos = self.objects[&id].position;
        if m.no_turn_dist_left > 0.0 && m.state as u8 >= MissileState::Ignition as u8 {
            m.no_turn_dist_left -= len(sub(pos, m.prev_pos));
            m.prev_pos = pos;
        }
        if pos[2] < 0.0 {
            self.destroy_object(id);
            return crate::scheduler::FOREVER;
        }
        // The tracked goal object (`aiMoveToObject`).
        if m.goal_object != 0 {
            match self.objects.get(&m.goal_object) {
                Some(v) if !v.dead => m.goal = v.position,
                _ => m.goal_object = 0,
            }
        }
        let mut detonate = false;
        match m.state {
            MissileState::Prelaunch => {
                m.loco_accel = 0.0;
                m.loco_turn = 0.0;
            }
            MissileState::Launch | MissileState::Ignition => {
                if m.state == MissileState::Launch {
                    m.loco_accel = 0.0;
                    m.loco_turn = 0.0;
                    if frame - m.state_frame >= d.ignition_delay {
                        Self::switch_missile(&mut m, MissileState::Ignition, frame);
                    }
                }
                if m.state == MissileState::Ignition {
                    m.loco_accel = m.max_accel;
                    m.loco_turn = 0.0;
                    m.armed = true;
                    m.fuel_expires = if d.fuel_lifetime > 0 { frame + d.fuel_lifetime } else { 0x7fff_ffff };
                    Self::switch_missile(&mut m, MissileState::AttackNoTurn, frame);
                }
            }
            MissileState::AttackNoTurn | MissileState::Attack => {
                let turn_ok = m.state == MissileState::Attack;
                if frame >= m.fuel_expires {
                    if d.detonate_on_no_fuel {
                        detonate = true;
                    } else {
                        m.loco_accel = 0.0;
                        m.loco_turn = 0.0;
                    }
                } else {
                    m.loco_accel = m.max_accel;
                    m.loco_turn = if turn_ok { BIGNUM } else { 0.0 };
                }
                if !detonate && d.lock_distance > 0.0 {
                    let goal =
                        if m.tracking && m.goal_object != 0 { self.objects[&m.goal_object].position } else { m.goal };
                    let dist2 = (goal[0] - pos[0]).powi(2) + (goal[1] - pos[1]).powi(2);
                    let mut lock = d.lock_distance;
                    if !m.tracking {
                        lock *= 0.5;
                    }
                    if dist2 < lock * lock {
                        if !m.tracking {
                            m.goal = m.original_target;
                        }
                        Self::switch_missile(&mut m, MissileState::Kill, frame);
                    }
                }
                if !detonate && m.state != MissileState::Kill {
                    if m.no_turn_dist_left <= 0.0 {
                        Self::switch_missile(&mut m, MissileState::Attack, frame);
                    }
                    if m.tracking && m.goal_object == 0 {
                        // airborneTargetGone
                        m.fuel_expires = frame;
                        Self::switch_missile(&mut m, MissileState::KillSelf, frame);
                    }
                }
            }
            MissileState::Kill => {
                if frame >= m.fuel_expires {
                    if d.detonate_on_no_fuel {
                        detonate = true;
                    } else {
                        m.fuel_expires = frame;
                        Self::switch_missile(&mut m, MissileState::KillSelf, frame);
                    }
                } else {
                    m.braking = true;
                    m.loco_accel = m.max_accel;
                    m.loco_turn = BIGNUM;
                    // isIdle: the move is done once at the goal.
                    let speed =
                        self.missile_locomotor(id).map_or(0.0, |l| m.loco_speed.map_or(l.speed, |v| l.speed.min(v)));
                    if len(sub(m.goal, pos)) < speed.max(1.0) {
                        if m.goal_object != 0 {
                            let close = speed;
                            let d3 = self.boundary_distance_3d(id, m.goal_object);
                            if d3 < close * close {
                                let gp = self.objects[&m.goal_object].position;
                                self.objects.get_mut(&id).unwrap().position = gp;
                                detonate = true;
                            }
                        } else {
                            detonate = true;
                        }
                    }
                    if m.tracking && m.goal_object == 0 && !detonate {
                        m.fuel_expires = frame;
                        Self::switch_missile(&mut m, MissileState::KillSelf, frame);
                    }
                }
            }
            MissileState::KillSelf => {
                if frame.wrapping_sub(m.state_frame) >= d.kill_self_delay {
                    if d.detonate_calls_kill {
                        let max = self.objects[&id].health.as_ref().map_or(0.0, |h| h.max);
                        let death = crate::body::death_type("NORMAL").unwrap_or(0);
                        self.attempt_damage(id, DamageInfo::new(0, DAMAGE_UNRESISTABLE, death, max));
                    } else {
                        self.destroy_object(id);
                    }
                    Self::switch_missile(&mut m, MissileState::Dead, frame);
                }
            }
            MissileState::Dead => {
                m.loco_accel = 0.0;
                m.loco_turn = 0.0;
            }
        }
        if let Some(o) = self.objects.get_mut(&id) {
            o.missile = Some(m);
        }
        if detonate {
            self.missile_detonate(id);
        }
        if self.objects.contains_key(&id) {
            self.missile_thrust(id);
        }
        1
    }

    fn missile_locomotor(&self, id: u32) -> Option<&crate::templates::Locomotor> {
        let t = &self.templates.objects[&self.objects[&id].template];
        t.locomotor().and_then(|n| self.templates.locomotors.get(n))
    }

    fn boundary_distance_3d(&self, a: u32, b: u32) -> f32 {
        let (oa, ob) = (&self.objects[&a], &self.objects[&b]);
        let (_, ra, za) = Self::bounding(&self.templates.objects[&oa.template]);
        let (_, rb, zb) = Self::bounding(&self.templates.objects[&ob.template]);
        let d = [
            ob.position[0] - oa.position[0],
            ob.position[1] - oa.position[1],
            ob.position[2] + zb - oa.position[2] - za,
        ];
        let s = len(d) - ra - rb;
        if s <= 0.0 { 0.0 } else { s * s }
    }

    /// `moveTowardsPositionThrust` towards the goal.
    fn missile_thrust(&mut self, id: u32) {
        let Some(l) = self.missile_locomotor(id).cloned() else { return };
        let o = &self.objects[&id];
        let Some(mut m) = o.missile.clone() else { return };
        if matches!(m.state, MissileState::Dead | MissileState::KillSelf) {
            return;
        }
        let pos = o.position;
        let goal = m.goal;
        let on_path = len(sub(goal, pos));
        // getMaxSpeedForCondition, getMaxAcceleration, getMaxTurnRate: the
        // template's values, capped by the missile's overrides.
        let max_speed = m.loco_speed.map_or(l.speed, |v| l.speed.min(v));
        let mut desired = max_speed.max(l.min_speed);
        let fwd = forward(o.angle, m.pitch);
        // getForwardSpeed3D (per-axis magnitude, signed by the dot product).
        let comps = [m.vel[0] * fwd[0], m.vel[1] * fwd[1], m.vel[2] * fwd[2]];
        let fs = len(comps);
        let actual = if comps[0] + comps[1] + comps[2] >= 0.0 { fs } else { -fs };
        if l.braking > 0.0 {
            let delta = actual - l.min_speed;
            let slow = if delta <= 0.0 { 0.0 } else { (delta * delta / l.braking.abs()) * 0.5 * 1.05 };
            if on_path < slow {
                desired = l.min_speed;
            }
        }
        let mut local_goal = goal;
        if l.preferred_height != 0.0 {
            let surface = self.map.ground_height(pos[0], pos[1]);
            local_goal[2] = l.preferred_height + surface;
            local_goal[2] = pos[2] + (local_goal[2] - pos[2]) * l.preferred_height_damping;
        }
        let speed_delta = desired - actual;
        let max_accel =
            if speed_delta > 0.0 || l.braking == 0.0 { l.acceleration.min(m.loco_accel) } else { -l.braking };
        let max_turn = l.turn_rate.min(m.loco_turn);
        // calcDirectionToApplyThrust
        let to_goal = sub(local_goal, pos);
        let mut thrust_goal = fwd;
        if !nearly_zero(dot(to_goal, to_goal)) {
            let mut cv = m.vel;
            cv[2] += crate::locomotion::DEFAULT_GRAVITY;
            let dist = len(to_goal);
            let cvm = len(cv);
            let denom = dot(cv, cv) - max_accel * max_accel;
            let mut found = false;
            if !nearly_zero(denom) {
                let mut t = (dist * (cvm + max_accel)) / denom;
                let t2 = (dist * (cvm - max_accel)) / denom;
                if t >= 0.0 || t2 >= 0.0 {
                    if t < 0.0 || (t2 >= 0.0 && t2 < t) {
                        t = t2;
                    }
                    if !nearly_zero(t) {
                        thrust_goal = norm([to_goal[0] / t - cv[0], to_goal[1] / t - cv[1], to_goal[2] / t - cv[2]]);
                        found = true;
                    }
                }
            }
            if !found {
                thrust_goal = norm(to_goal);
            }
        }
        let max_thrust_angle = if max_turn > 0.0 { l.max_thrust_angle } else { 0.0 };
        let (thrust_dir, thrust_angle) = rotate_towards(max_thrust_angle, fwd, thrust_goal);
        let (mut yaw, mut pitch) = (o.angle, m.pitch);
        if !nearly_zero(len(m.vel)) {
            let mut vel = m.vel;
            let mut adjust = true;
            let mut turn = max_turn;
            if m.braking {
                vel = sub(goal, pos);
                if nearly_zero(dot(vel, vel)) {
                    adjust = false;
                }
                turn *= 3.0;
            }
            if adjust {
                let (dir, rel) = rotate_towards(turn, fwd, vel);
                if rel != 0.0 {
                    (yaw, pitch) = orient(dir);
                }
            }
        }
        if speed_delta != 0.0 || thrust_angle != 0.0 {
            let mfs = if max_speed <= 0.0 { 0.01 } else { max_speed };
            let damping = (max_accel / mfs).clamp(0.0, 1.0);
            for i in 0..3 {
                m.accel[i] += thrust_dir[i] * max_accel - m.vel[i] * damping;
            }
            m.motive_until = self.frame + crate::locomotion::MOTIVE_FRAMES;
        }
        m.pitch = pitch;
        let o = self.objects.get_mut(&id).unwrap();
        o.angle = yaw;
        o.missile = Some(m);
    }

    /// The missile's flight: gravity, aerodynamic friction, Euler integration;
    /// touching the ground is a collision with nothing.
    pub(crate) fn missile_physics(&mut self, id: u32) {
        let Some(mut m) = self.objects[&id].missile.clone() else { return };
        let t = &self.templates.objects[&self.objects[&id].template];
        let data = t.physics().unwrap_or_default();
        m.accel[2] += crate::locomotion::DEFAULT_GRAVITY;
        let aero = -data.aerodynamic_friction.clamp(0.0, 0.99);
        for i in 0..3 {
            m.accel[i] += m.vel[i] * aero;
            m.vel[i] += m.accel[i];
        }
        m.accel = [0.0; 3];
        let o = self.objects.get_mut(&id).unwrap();
        for i in 0..3 {
            o.position[i] += m.vel[i];
        }
        let pos = o.position;
        o.missile = Some(m);
        let ground = self.map.ground_height(pos[0], pos[1]);
        let falling = self.objects[&id].missile.as_ref().is_some_and(|m| m.vel[2] < 0.0);
        if pos[2] <= ground && falling {
            self.missile_hit(id, None);
            if let Some(o) = self.objects.get_mut(&id) {
                o.position[2] = o.position[2].max(ground);
                if let Some(m) = o.missile.as_mut() {
                    m.vel[2] = m.vel[2].max(0.0);
                }
            }
        }
    }

    /// `MissileAIUpdate::projectileHandleCollision`: true once it detonated.
    pub(crate) fn missile_hit(&mut self, id: u32, other: Option<u32>) -> bool {
        let Some(m) = self.objects.get(&id).and_then(|o| o.missile.clone()) else { return false };
        if !m.armed || matches!(m.state, MissileState::KillSelf | MissileState::Dead) {
            return false;
        }
        let pos = self.objects[&id].position;
        match other {
            None => {
                let delta = sub(pos, m.goal);
                if delta[2] > crate::navigation::CELL && len(delta) > 3.0 * crate::navigation::CELL {
                    return false;
                }
            }
            Some(o) => {
                let Some(wt) = self.templates.weapons.get(&m.weapon).cloned() else { return false };
                if !self.should_collide(&wt, m.launcher, id, o, m.victim) {
                    return false;
                }
            }
        }
        self.missile_detonate(id);
        if let Some(o) = self.objects.get_mut(&id) {
            o.status |= crate::body::status::NO_COLLISIONS;
        }
        true
    }

    /// `MissileAIUpdate::detonate`.
    fn missile_detonate(&mut self, id: u32) {
        let Some(m) = self.objects.get(&id).and_then(|o| o.missile.clone()) else { return };
        let pos = self.objects[&id].position;
        if let Some(wt) = self.templates.weapons.get(&m.weapon).cloned() {
            self.projectile_detonation(id, &wt, pos);
            if wt.die_on_detonate {
                self.kill_detonated(id);
            }
        } else {
            self.kill_detonated(id);
        }
        let frame = self.frame;
        if let Some(m) = self.objects.get_mut(&id).and_then(|o| o.missile.as_mut()) {
            Self::switch_missile(m, MissileState::KillSelf, frame);
        }
    }
    fn kill_detonated(&mut self, id: u32) {
        let max = self.objects.get(&id).and_then(|o| o.health.as_ref()).map_or(0.0, |h| h.max);
        let death = crate::body::death_type("DETONATED").unwrap_or(10);
        self.attempt_damage(id, DamageInfo::new(0, DAMAGE_UNRESISTABLE, death, max));
    }
}
