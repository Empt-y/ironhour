//! Ground locomotion and rigid-body physics.
//!
//! Ported from `GameLogic/Object/Locomotor.cpp` (`locoUpdate_moveTowardsPosition`,
//! `moveTowardsPositionTreads`, `...Wheels`, `...Legs`, `...Other`,
//! `rotateObjAroundLocoPivot`, `locoUpdate_maintainCurrentPosition`, the braking
//! slide) and `Object/Update/PhysicsUpdate.cpp` (`applyForce`, `applyMotiveForce`,
//! `applyFrictionalForces`, the Euler integration and ground clamp of `update`,
//! `getForwardSpeed2D`, `scrubVelocity2D`).
//!
//! The locomotor turns the body and applies a motive force along its facing;
//! physics integrates that into velocity with gravity and friction. Collision
//! clipping of the resulting displacement stays in `Simulation::integrate_ground`.
//!
//! Not yet ported: damage-state speed/turn penalties (pristine values are used),
//! `fixInvalidPosition`, bouncing, pitch/roll/yaw rates, falling damage, the air,
//! hover, wings, thrust and climber locomotors, and `ZAxisBehavior` other than
//! resting on the ground.

use crate::templates::{Appearance, Locomotor, PhysicsData};

/// `PATHFIND_CELL_SIZE_F`.
pub const CELL: f32 = 10.0;
/// `MOTIVE_FRAMES`: a motive force marks the body as driven for a third of a second.
pub const MOTIVE_FRAMES: u32 = 30 / 3;
const MAX_BRAKING_FACTOR: f32 = 5.0;
const DONUT_FRAMES: u32 = 75; // DONUT_TIME_DELAY_SECONDS (2.5) * 30
const DONUT_DISTANCE: f32 = 4.0 * CELL;
const MIN_NON_AERO_FRICTION: f32 = 0.01;
const MAX_FRICTION: f32 = 0.99;
/// Stock `Gravity = -64` per second squared, per frame squared.
pub const DEFAULT_GRAVITY: f32 = -64.0 / 900.0;

/// `PhysicsBehavior` runtime state.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct Physics {
    pub vel: [f32; 3],
    pub accel: [f32; 3],
    pub motive_expires: u32,
    /// `OBJECT_STATUS_BRAKING`: the locomotor moves the body itself.
    pub braking_status: bool,
    /// `m_extraFriction` from the locomotor (`Extra2DFriction`).
    pub extra_friction: f32,
    pub stick_to_ground: bool,
    /// The template's `PhysicsBehavior` data.
    pub data: PhysicsData,
}

/// Per-object `Locomotor` runtime state.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct LocoState {
    pub is_braking: bool,
    pub braking_factor: f32,
    pub moving_backwards: bool,
    pub three_point_turn: bool,
    pub donut_timer: u32,
    pub angle_offset: f32,
    pub offset_increment: f32,
    pub offset_increasing: bool,
}

impl LocoState {
    /// `Locomotor::Locomotor`: draws three logic random values, as the original.
    pub fn new(l: &Locomotor, frame: u32, rng: &mut crate::rng::LogicRng) -> Self {
        let pi = std::f32::consts::PI;
        let angle_offset = rng.real(-pi / 6.0, pi / 6.0);
        let offset_increment = (pi / 40.0) * (rng.real(0.8, 1.2) / l.wander_length_factor);
        let offset_increasing = rng.integer(0, 1) != 0;
        Self {
            is_braking: false,
            braking_factor: 1.0,
            moving_backwards: false,
            three_point_turn: false,
            donut_timer: frame + DONUT_FRAMES,
            angle_offset,
            offset_increment,
            offset_increasing,
        }
    }
}

/// The parts of an object the locomotor and physics read and write.
pub struct Body<'a> {
    pub position: &'a mut [f32; 3],
    pub angle: &'a mut f32,
    pub physics: &'a mut Physics,
    pub loco: &'a mut LocoState,
    pub major_radius: f32,
    pub bounding_radius: f32,
    pub frame: u32,
}

fn dir(angle: f32) -> [f32; 2] {
    [ih_math::cos(angle), ih_math::sin(angle)]
}

fn normalize_angle(a: f32) -> f32 {
    ih_math::angle_delta(0.0, a)
}

impl Body<'_> {
    fn is_motive(&self) -> bool {
        self.physics.motive_expires > self.frame
    }

    /// `applyForce`: while driven, only the lateral part of outside forces counts.
    fn apply_force(&mut self, f: [f32; 3]) {
        if f.iter().any(|v| v.is_nan()) {
            return;
        }
        let mut m = f;
        if self.is_motive() {
            let d = dir(*self.angle);
            let lateral = f[0] * -d[1] + f[1] * d[0];
            m[0] = lateral * -d[1];
            m[1] = lateral * d[0];
        }
        let inv = 1.0 / self.physics.data.mass;
        for (a, v) in self.physics.accel.iter_mut().zip(m) {
            *a += v * inv;
        }
    }

    /// `applyMotiveForce`.
    fn apply_motive_force(&mut self, f: [f32; 3]) {
        self.physics.motive_expires = 0;
        self.apply_force(f);
        self.physics.motive_expires = self.frame + MOTIVE_FRAMES;
    }

    /// `getForwardSpeed2D`, including its per-axis magnitude (not a true projection).
    pub fn forward_speed_2d(&self) -> f32 {
        let d = dir(*self.angle);
        let vx = self.physics.vel[0] * d[0];
        let vy = self.physics.vel[1] * d[1];
        let speed = ih_math::sqrt(vx * vx + vy * vy);
        if vx + vy >= 0.0 { speed } else { -speed }
    }

    fn velocity_magnitude(&self) -> f32 {
        let v = self.physics.vel;
        ih_math::sqrt(v[0] * v[0] + v[1] * v[1] + v[2] * v[2])
    }

    /// `scrubVelocity2D`.
    pub fn scrub_velocity_2d(&mut self, desired: f32) {
        let v = &mut self.physics.vel;
        if desired < 0.001 {
            v[0] = 0.0;
            v[1] = 0.0;
        } else {
            let cur = ih_math::sqrt(v[0] * v[0] + v[1] * v[1]);
            if desired > cur {
                return;
            }
            let k = desired / cur;
            v[0] *= k;
            v[1] *= k;
        }
    }

    fn friction(&self, base: f32) -> f32 {
        (base + self.physics.extra_friction).clamp(MIN_NON_AERO_FRICTION, MAX_FRICTION)
    }

    /// `rotateObjAroundLocoPivot`; returns the signed angle still to turn.
    fn rotate_around_pivot(&mut self, l: &Locomotor, goal: [f32; 2], max_turn: f32) -> f32 {
        let angle = *self.angle;
        let offset = if self.loco.is_braking { 0.0 } else { l.turn_pivot_offset };
        if offset != 0.0 {
            let d = dir(angle);
            let r = self.bounding_radius * offset;
            let turn = [self.position[0] + d[0] * r, self.position[1] + d[1] * r];
            let (dx, dy) = (goal[0] - turn[0], goal[1] - turn[1]);
            if dx.abs() < 0.1 && dy.abs() < 0.1 {
                return 0.0;
            }
            let rel = ih_math::angle_delta(angle, ih_math::atan2(dy, dx));
            let amount = rel.clamp(-max_turn, max_turn);
            // Rotate the object's position about the pivot by `amount`.
            let (s, c) = (ih_math::sin(amount), ih_math::cos(amount));
            let (px, py) = (self.position[0] - turn[0], self.position[1] - turn[1]);
            self.position[0] = turn[0] + px * c - py * s;
            self.position[1] = turn[1] + px * s + py * c;
            *self.angle = normalize_angle(angle + amount);
            rel
        } else {
            let rel =
                ih_math::angle_delta(angle, ih_math::atan2(goal[1] - self.position[1], goal[0] - self.position[0]));
            *self.angle = normalize_angle(angle + rel.clamp(-max_turn, max_turn));
            rel
        }
    }

    /// Accelerate or brake toward `goal_speed` along `along` (`Maintain goal speed`).
    fn drive(&mut self, goal_speed: f32, actual: f32, accel: f32, brake: f32, along: [f32; 2]) {
        let delta = goal_speed - actual;
        if delta == 0.0 {
            return;
        }
        let mass = self.physics.data.mass;
        let a = if delta > 0.0 { accel } else { -brake };
        let mut force = mass * a;
        let needed = mass * delta;
        if force.abs() > needed.abs() {
            force = needed;
        }
        self.apply_motive_force([force * along[0], force * along[1], 0.0]);
    }

    /// `locoUpdate_moveTowardsPosition` for ground locomotors. `valid` is
    /// `validMovementTerrain` on the object's layer. Returns the braking slide to
    /// add to this frame's displacement.
    pub fn move_towards_position(
        &mut self,
        l: &Locomotor,
        goal: [f32; 3],
        mut on_path_dist: f32,
        mut desired_speed: f32,
        blocked: bool,
        valid: &dyn Fn([f32; 2]) -> bool,
    ) -> [f32; 2] {
        let max_speed = l.speed;
        desired_speed = desired_speed.min(max_speed);
        let stop_at_max = (max_speed / l.braking) * max_speed / 2.0;
        if on_path_dist > CELL && on_path_dist > stop_at_max {
            self.loco.is_braking = false;
            self.loco.braking_factor = 1.0;
        }
        let (dx, dy) = (goal[0] - self.position[0], goal[1] - self.position[1]);
        let dist = ih_math::sqrt(dx * dx + dy * dy);
        if dist > on_path_dist {
            if dist > 2.0 * on_path_dist {
                self.loco.is_braking = true;
            }
            on_path_dist = dist;
        }
        self.apply_motive_force([0.0; 3]);
        if blocked {
            // The `*blocked` branch: stop short of the blocker and turn in place toward
            // the goal. The original sets it from collision handling and clears it when
            // slower than desired; collision forces are not ported, so object contact
            // sets it here, and it lasts only while the body still has to turn (the
            // original's "blocked = rotating" result). Facing the goal, it drives on
            // and the contact builds up toward a traffic repath.
            let rel = if l.wander_width_factor == 0.0 {
                self.rotate_around_pivot(l, [goal[0], goal[1]], l.turn_rate)
            } else {
                0.0
            };
            if rel.abs() > l.turn_rate {
                self.scrub_velocity_2d(0.0);
                return [0.0; 2];
            }
        }
        let was_braking = self.physics.braking_status;
        match l.appearance {
            Appearance::Treads => self.treads(l, goal, on_path_dist, desired_speed),
            Appearance::Wheels => self.wheels(l, goal, on_path_dist, desired_speed, valid),
            Appearance::Legs => self.legs(l, goal, on_path_dist, desired_speed),
            Appearance::Other => self.other(l, goal, on_path_dist, desired_speed),
        }
        // Ground ZAxisBehavior: physics keeps the body on the surface.
        self.physics.braking_status = self.loco.is_braking;
        if was_braking && dist > 0.001 {
            // MIN_VEL: one cell per second.
            let vel = self.forward_speed_2d().abs().max(CELL / 30.0).min(dist);
            return [dx / dist * vel, dy / dist * vel];
        }
        [0.0; 2]
    }

    fn braking_goal(&mut self, slow_down: f32, on_path: f32, actual: f32, brake: f32, goal_speed: &mut f32) {
        self.loco.braking_factor = ((slow_down / on_path) * (slow_down / on_path)).min(MAX_BRAKING_FACTOR);
        if slow_down > on_path {
            *goal_speed = (actual - brake).max(0.0);
        } else if slow_down > on_path * 0.75 {
            *goal_speed = (actual - brake / 2.0).max(0.0);
        } else {
            *goal_speed = actual;
        }
    }

    fn treads(&mut self, l: &Locomotor, goal: [f32; 3], on_path: f32, desired: f32) {
        let rel = self.rotate_around_pivot(l, [goal[0], goal[1]], l.turn_rate);
        let angle_coeff = (rel.abs() / (std::f32::consts::PI / 4.0)).min(1.0);
        let (dx, dy) = (self.position[0] - goal[0], self.position[1] - goal[1]);
        let mut goal_speed = (1.0 - angle_coeff) * desired;
        let actual = self.forward_speed_2d();
        let slow_down = (actual / 1.5) * (actual / l.braking);
        if dx * dx + dy * dy < (2.0 * CELL) * (2.0 * CELL) && angle_coeff > 0.05 {
            goal_speed = actual * 0.6;
        }
        if on_path < slow_down && !self.loco.is_braking {
            self.loco.is_braking = true;
            self.loco.braking_factor = 1.1;
        }
        if on_path > CELL && on_path > 2.0 * slow_down {
            self.loco.is_braking = false;
        }
        if self.loco.is_braking {
            self.braking_goal(slow_down, on_path, actual, l.braking, &mut goal_speed);
        }
        let brake = self.loco.braking_factor * l.braking;
        self.drive(goal_speed, actual, l.acceleration, brake, dir(*self.angle));
    }

    fn wheels(
        &mut self,
        l: &Locomotor,
        goal: [f32; 3],
        on_path: f32,
        mut desired: f32,
        valid: &dyn Fn([f32; 2]) -> bool,
    ) {
        let pi = std::f32::consts::PI;
        let max_speed = l.speed;
        let turn_speed = l.min_turn_speed.max(max_speed / 4.0);
        let angle = *self.angle;
        let mut desired_angle = ih_math::atan2(goal[1] - self.position[1], goal[0] - self.position[0]);
        let mut rel = ih_math::angle_delta(angle, desired_angle);
        let mut backwards = false;
        let mut actual = self.forward_speed_2d();
        let mut three_point = false;
        if actual == 0.0 {
            self.loco.moving_backwards = false;
            if l.can_move_backward && rel.abs() > pi / 2.0 {
                self.loco.moving_backwards = true;
                self.loco.three_point_turn = on_path > 5.0 * self.major_radius;
            }
        }
        if self.loco.moving_backwards {
            if rel.abs() < pi / 2.0 {
                self.loco.moving_backwards = false;
            } else {
                backwards = true;
                self.loco.three_point_turn = on_path > 5.0 * self.major_radius;
                three_point = self.loco.three_point_turn;
                if !three_point {
                    desired_angle = ih_math::angle_delta(pi, desired_angle);
                    rel = ih_math::angle_delta(angle, desired_angle);
                }
            }
        }
        if rel.abs() > pi / 20.0 && desired > turn_speed {
            desired = turn_speed;
        }
        let mut goal_speed = desired;
        if backwards {
            actual = -actual;
        }
        let slow_down = (actual / 1.5) * (actual / l.braking + 1.0) + actual;
        let effective = slow_down.max(CELL);
        if rel.abs() > pi / 12.0 {
            // Look half a second ahead along the turn for impassable ground.
            let project = 15.0;
            let distance = project * (goal_speed + actual) / 2.0;
            let turn_factor = (((goal_speed + actual) / 2.0) / turn_speed).min(1.0);
            let amount = project * turn_factor * l.turn_rate / 4.0;
            let target = if rel < 0.0 { angle - amount } else { angle + amount };
            let off = [ih_math::cos(target) * distance, ih_math::sin(target) * distance];
            let next = [self.position[0] + off[0], self.position[1] + off[1]];
            let half = [self.position[0] + off[0] / 2.0, self.position[1] + off[1] / 2.0];
            if !valid(half) || !valid(next) {
                self.rotate_around_pivot(l, [goal[0], goal[1]], l.turn_rate);
                self.apply_motive_force([0.0; 3]);
                return;
            }
        }
        if on_path < effective && !self.loco.is_braking {
            self.loco.is_braking = true;
            self.loco.braking_factor = 1.1;
        }
        if on_path > CELL && on_path > 2.0 * slow_down {
            self.loco.is_braking = false;
        }
        if on_path > DONUT_DISTANCE {
            self.loco.donut_timer = self.frame + DONUT_FRAMES;
        } else if self.loco.donut_timer < self.frame {
            self.loco.is_braking = true;
        }
        if self.loco.is_braking {
            self.braking_goal(slow_down, on_path, actual, l.braking, &mut goal_speed);
            // The wheeled version resets the factor after computing it.
            self.loco.braking_factor = 1.0;
        }
        let turn_amount = (actual / turn_speed).abs().min(1.0) * l.turn_rate;
        if backwards && !three_point {
            let back = [2.0 * self.position[0] - goal[0], 2.0 * self.position[1] - goal[1]];
            self.rotate_around_pivot(l, back, turn_amount);
        } else {
            self.rotate_around_pivot(l, [goal[0], goal[1]], turn_amount);
        }
        let speed_delta = if backwards { -goal_speed + actual } else { goal_speed - actual };
        if speed_delta != 0.0 {
            let mass = self.physics.data.mass;
            let brake = self.loco.braking_factor * l.braking;
            let a = if backwards {
                if speed_delta < 0.0 { -l.acceleration } else { brake }
            } else if speed_delta > 0.0 {
                l.acceleration
            } else {
                -brake
            };
            let mut force = mass * a;
            let needed = mass * speed_delta;
            if force.abs() > needed.abs() {
                force = needed;
            }
            let d = dir(*self.angle);
            self.apply_motive_force([force * d[0], force * d[1], 0.0]);
        }
    }

    fn legs(&mut self, l: &Locomotor, goal: [f32; 3], on_path: f32, desired: f32) {
        if l.downhill_only && self.position[2] < goal[2] {
            return;
        }
        let pi = std::f32::consts::PI;
        let actual = self.forward_speed_2d();
        let angle = *self.angle;
        let mut desired_angle = ih_math::atan2(goal[1] - self.position[1], goal[0] - self.position[0]);
        if l.wander_width_factor != 0.0 {
            let limit = pi / 8.0 * l.wander_width_factor;
            if self.loco.offset_increasing {
                self.loco.angle_offset += self.loco.offset_increment * actual;
                if self.loco.angle_offset > limit {
                    self.loco.offset_increasing = false;
                }
            } else {
                self.loco.angle_offset -= self.loco.offset_increment * actual;
                if self.loco.angle_offset < -limit {
                    self.loco.offset_increasing = true;
                }
            }
            desired_angle = normalize_angle(desired_angle + self.loco.angle_offset);
        }
        let rel = ih_math::angle_delta(angle, desired_angle);
        self.move_towards_angle(l, desired_angle);
        let angle_coeff = (rel.abs() / (pi / 4.0)).min(1.0);
        let mut goal_speed = (1.0 - angle_coeff) * desired;
        if on_path < slow_down_dist(actual, l.min_speed, l.braking) {
            goal_speed = l.min_speed;
        }
        self.drive(goal_speed, actual, l.acceleration, l.braking, dir(*self.angle));
    }

    /// `locoUpdate_moveTowardsAngle` for locomotors without a minimum speed.
    fn move_towards_angle(&mut self, l: &Locomotor, goal_angle: f32) {
        let target = [
            self.position[0] + ih_math::cos(goal_angle) * 1000.0,
            self.position[1] + ih_math::sin(goal_angle) * 1000.0,
        ];
        self.rotate_around_pivot(l, target, l.turn_rate);
    }

    fn other(&mut self, l: &Locomotor, goal: [f32; 3], on_path: f32, desired: f32) {
        let actual = self.forward_speed_2d();
        self.rotate_around_pivot(l, [goal[0], goal[1]], l.turn_rate);
        let mut goal_speed = desired;
        if on_path < slow_down_dist(actual, l.min_speed, l.braking) {
            goal_speed = l.min_speed;
        }
        self.drive(goal_speed, actual, l.acceleration, l.braking, dir(*self.angle));
    }

    /// `locoUpdate_maintainCurrentPosition` for ground locomotors.
    pub fn maintain_current_position(&mut self) {
        self.loco.donut_timer = self.frame + DONUT_FRAMES;
        self.loco.is_braking = false;
        if self.is_motive() {
            self.scrub_velocity_2d(0.0);
        }
    }

    /// `PhysicsBehavior::update` for a body on the ground: gravity and friction,
    /// Euler integration, and the ground clamp. Returns the x/y displacement
    /// (none while braking) and sets z from `ground` at the new position.
    pub fn physics_update(&mut self, gravity: f32, held: bool) -> [f32; 2] {
        let mut step = [0.0; 2];
        if !held {
            self.physics.accel[2] += gravity;
            // applyFrictionalForces, on the ground.
            let v = self.physics.vel;
            if v[0] != 0.0 || v[1] != 0.0 {
                let d = dir(*self.angle);
                let mass = self.physics.data.mass;
                let lateral = v[0] * -d[1] + v[1] * d[0];
                let lf = mass * self.friction(self.physics.data.lateral_friction);
                let mut f = [-(lf * lateral * -d[1]), -(lf * lateral * d[0]), 0.0];
                if !self.is_motive() {
                    let forward = v[0] * d[0] + v[1] * d[1];
                    let ff = mass * self.friction(self.physics.data.forward_friction);
                    f[0] += -(ff * forward * d[0]);
                    f[1] += -(ff * forward * d[1]);
                }
                self.apply_force(f);
            }
            for (v, a) in self.physics.vel.iter_mut().zip(self.physics.accel) {
                *v += a;
                if v.abs() < 0.001 {
                    *v = 0.0;
                }
            }
            if !self.physics.braking_status {
                step = [self.physics.vel[0], self.physics.vel[1]];
            }
            self.position[2] += self.physics.vel[2];
        }
        self.physics.accel = [0.0; 3];
        step
    }

    /// The ground clamp at the end of `PhysicsBehavior::update`.
    pub fn clamp_to_ground(&mut self, ground: f32) {
        if self.position[2] <= ground {
            let dz = ground - self.position[2];
            self.physics.vel[2] = (self.physics.vel[2] + dz).min(0.0);
            self.position[2] = ground;
        } else if self.physics.stick_to_ground {
            self.position[2] = ground;
        }
    }

    /// Current speed, for diagnostics and traffic.
    pub fn speed(&self) -> f32 {
        self.velocity_magnitude()
    }
}

/// `calcSlowDownDist`.
fn slow_down_dist(cur: f32, desired: f32, braking: f32) -> f32 {
    let delta = cur - desired;
    if delta <= 0.0 { 0.0 } else { (delta * delta / braking.abs()) * 0.5 * 1.05 }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn body<'a>(pos: &'a mut [f32; 3], angle: &'a mut f32, p: &'a mut Physics, l: &'a mut LocoState) -> Body<'a> {
        Body { position: pos, angle, physics: p, loco: l, major_radius: 1.0, bounding_radius: 1.0, frame: 0 }
    }

    #[test]
    fn forward_speed_keeps_the_per_axis_magnitude_of_the_original() {
        let (mut pos, mut angle) = ([0.0; 3], std::f32::consts::FRAC_PI_4);
        let (mut p, mut l) = (Physics { vel: [1.0, 0.0, 0.0], ..Default::default() }, LocoState::default());
        let b = body(&mut pos, &mut angle, &mut p, &mut l);
        // sqrt((vx*dx)^2 + (vy*dy)^2), not the dot product (which is 0.7071 too here,
        // but differs for other directions).
        assert!((b.forward_speed_2d() - std::f32::consts::FRAC_1_SQRT_2).abs() < 1e-6);
        let (mut pos, mut angle) = ([0.0; 3], 0.0);
        let mut p = Physics { vel: [1.0, 1.0, 0.0], ..Default::default() };
        let b = body(&mut pos, &mut angle, &mut p, &mut l);
        assert!((b.forward_speed_2d() - 1.0).abs() < 1e-6);
    }

    #[test]
    fn motive_force_takes_only_lateral_part_of_outside_forces() {
        let (mut pos, mut angle) = ([0.0; 3], 0.0);
        let mut p = Physics { data: PhysicsData::default(), ..Default::default() };
        let mut l = LocoState::default();
        let mut b = body(&mut pos, &mut angle, &mut p, &mut l);
        b.apply_motive_force([0.0; 3]);
        b.apply_force([2.0, 3.0, 0.0]);
        assert_eq!(b.physics.accel, [0.0, 3.0, 0.0]);
        // Gravity accumulates in z; resting on the ground clamps it back.
        let step = b.physics_update(DEFAULT_GRAVITY, false);
        assert_eq!(step[0], 0.0);
        b.clamp_to_ground(0.0);
        assert_eq!(b.position[2], 0.0);
        assert!(b.physics.vel[2] <= 0.0);
    }
}
