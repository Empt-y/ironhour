//! Client-side chassis motion, from `Drawable.cpp`: `calcPhysicsXform` and its
//! `Treads`, `Wheels`, `Motorcycle`, `HoverOrWings` and `Thrust` models, applied
//! as `applyPhysicsXform` does. Render-only: it reads the logic object's motion
//! and never feeds back. The original stepped it once per drawn frame, which the
//! retail game locked to the 30 Hz logic; ironhour steps it once per logic frame
//! and interpolates.
//!
//! Not yet: the per-wheel suspension offsets and wheel angles, which only the
//! truck draw modules use, and recoil from damage (M5).
use crate::condition::ClientRng;
use ih_sim::templates::{Chassis, ChassisKind};
use std::f32::consts::PI;

/// `DrawableLocoInfo` (without the wheel offsets).
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct LocoInfo {
    pub pitch: f32,
    pub pitch_rate: f32,
    pub roll: f32,
    pub roll_rate: f32,
    pub yaw: f32,
    pub accel_pitch: f32,
    pub accel_pitch_rate: f32,
    pub accel_roll: f32,
    pub accel_roll_rate: f32,
    pub overlap_z_vel: f32,
    pub overlap_z: f32,
    pub wobble: f32,
    pub yaw_modulator: f32,
    pub pitch_modulator: f32,
}
impl Default for LocoInfo {
    fn default() -> Self {
        Self {
            pitch: 0.0,
            pitch_rate: 0.0,
            roll: 0.0,
            roll_rate: 0.0,
            yaw: 0.0,
            accel_pitch: 0.0,
            accel_pitch_rate: 0.0,
            accel_roll: 0.0,
            accel_roll_rate: 0.0,
            overlap_z_vel: 0.0,
            overlap_z: 0.0,
            wobble: 1.0,
            yaw_modulator: 0.0,
            pitch_modulator: 0.0,
        }
    }
}

/// `PhysicsXformInfo`.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct Xform {
    pub z: f32,
    pub pitch: f32,
    pub roll: f32,
    pub yaw: f32,
}
impl Xform {
    pub fn lerp(self, to: Self, t: f32) -> Self {
        let l = |a: f32, b: f32| a + (b - a) * t;
        Self {
            z: l(self.z, to.z),
            pitch: l(self.pitch, to.pitch),
            roll: l(self.roll, to.roll),
            yaw: l(self.yaw, to.yaw),
        }
    }
    /// `applyPhysicsXform`: translate, then pitch about Y, roll about -X and yaw
    /// about Z, each in the object's local frame.
    pub fn matrix(self) -> glam::Mat4 {
        glam::Mat4::from_translation(glam::Vec3::new(0.0, 0.0, self.z))
            * glam::Mat4::from_rotation_y(self.pitch)
            * glam::Mat4::from_rotation_x(-self.roll)
            * glam::Mat4::from_rotation_z(self.yaw)
    }
}

/// What the models read from the logic object this frame.
#[derive(Clone, Copy, Debug)]
pub struct Motion {
    /// Our position (x, y).
    pub position: [f32; 2],
    /// Unit direction of the facing (`getUnitDirectionVector2D`).
    pub dir: [f32; 2],
    pub vel: [f32; 3],
    /// `getAcceleration` (`m_prevAccel`).
    pub accel: [f32; 3],
    pub motive: bool,
    /// Height of the object above its layer.
    pub height_above: f32,
    /// The layer's normal under the object (`getLayerHeight`).
    pub normal: [f32; 3],
    /// `isSignificantlyAboveTerrain`.
    pub airborne: bool,
    /// `getCurLocomotorSpeed`.
    pub max_speed: f32,
    /// `GeometryMajorRadius` and `GeometryMinorRadius`.
    pub length: f32,
    pub width: f32,
    /// Our bounding circle radius (`getBoundingCircleRadius`).
    pub radius: f32,
    /// The object being driven over now (`getCurrentOverlap`), if not shrubbery.
    pub overlap: Option<Overlap>,
    /// Whether something was overlapped last update (`getPreviousOverlap`).
    pub previous_overlap: bool,
}

/// The overlapped object, as the tread model reads it.
#[derive(Clone, Copy, Debug)]
pub struct Overlap {
    pub position: [f32; 2],
    pub radius: f32,
    /// `getMaxHeightAbovePosition`.
    pub height: f32,
    /// `LOW_OVERLAPPABLE`, infantry, or crushed front and back: driven over flat.
    pub flat: bool,
}

/// `calcPhysicsXform`: `None` for appearances without a chassis model.
pub fn step(c: &Chassis, info: &mut LocoInfo, m: &Motion, rng: &mut ClientRng) -> Option<Xform> {
    let mut x = Xform::default();
    match c.kind {
        ChassisKind::None => return None,
        ChassisKind::Treads => treads(c, info, m, &mut x, rng),
        ChassisKind::Wheels => wheels(c, info, m, &mut x, rng, false),
        ChassisKind::Motorcycle => wheels(c, info, m, &mut x, rng, true),
        ChassisKind::HoverOrWings => hover_or_wings(c, info, m, &mut x),
        ChassisKind::Thrust => thrust(c, info, &mut x),
    }
    // HOTFIX: no denormals back to the caller.
    for v in [&mut x.z, &mut x.pitch, &mut x.roll, &mut x.yaw] {
        if v.abs() < 1e-20 {
            *v = 0.0;
        }
    }
    Some(x)
}

fn cross(a: [f32; 3], b: [f32; 3]) -> [f32; 3] {
    [a[1] * b[2] - a[2] * b[1], a[2] * b[0] - a[0] * b[2], a[0] * b[1] - a[1] * b[0]]
}
fn normalize3(v: [f32; 3]) -> [f32; 3] {
    let l = (v[0] * v[0] + v[1] * v[1] + v[2] * v[2]).sqrt();
    if l > 0.0 { [v[0] / l, v[1] / l, v[2] / l] } else { v }
}

fn ground_pitch_roll(m: &Motion) -> (f32, f32) {
    let perp = [-m.dir[1], m.dir[0]];
    let pitch = (m.normal[0] * m.dir[0] + m.normal[1] * m.dir[1]) * (PI / 2.0);
    let roll = (m.normal[0] * perp[0] + m.normal[1] * perp[1]) * (PI / 2.0);
    (pitch, roll)
}

/// Chassis acceleration dynamics, damped back towards zero.
fn accel_springs(c: &Chassis, i: &mut LocoInfo) {
    i.accel_pitch_rate += -c.pitch_stiffness * i.accel_pitch + -c.pitch_damping * i.accel_pitch_rate;
    i.accel_pitch += i.accel_pitch_rate;
    i.accel_roll_rate += -c.roll_stiffness * i.accel_roll + -c.roll_damping * i.accel_roll_rate;
    i.accel_roll += i.accel_roll_rate;
}

/// Pitch and roll in reaction to acceleration, while a motive force is live.
fn accel_kick(c: &Chassis, i: &mut LocoInfo, m: &Motion) {
    if m.motive {
        let forward = m.dir[0] * m.accel[0] + m.dir[1] * m.accel[1];
        i.accel_pitch_rate += -(c.forward_accel * forward);
        let lateral = -m.dir[1] * m.accel[0] + m.dir[0] * m.accel[1];
        i.accel_roll_rate += -(c.lateral_accel * lateral);
    }
}

/// The acceleration limits (the roll is limited by the pitch limits, as written).
fn limit(c: &Chassis, i: &mut LocoInfo) {
    if i.accel_pitch > c.decel_pitch_limit {
        i.accel_pitch = c.decel_pitch_limit;
    } else if i.accel_pitch < -c.accel_pitch_limit {
        i.accel_pitch = -c.accel_pitch_limit;
    }
    if i.accel_roll > c.decel_pitch_limit {
        i.accel_roll = c.decel_pitch_limit;
    } else if i.accel_roll < -c.accel_pitch_limit {
        i.accel_roll = -c.accel_pitch_limit;
    }
}

/// `calcPhysicsXformTreads`.
fn treads(c: &Chassis, i: &mut LocoInfo, m: &Motion, x: &mut Xform, rng: &mut ClientRng) {
    const OVERLAP_SHRINK_FACTOR: f32 = 0.8;
    const FLATTENED_OBJECT_HEIGHT: f32 = 0.5;
    const LEAVE_OVERLAP_PITCH_KICK: f32 = PI / 128.0;
    const OVERLAP_ROUGH_VIBRATION_FACTOR: f32 = 5.0;
    const MAX_ROUGH_VIBRATION: f32 = 0.5;
    let mut normal = m.normal;
    let mut overlap_z = 0.0;
    if let Some(o) = m.overlap {
        let (dx, dy) = (o.position[0] - m.position[0], o.position[1] - m.position[1]);
        let center2 = dx * dx + dy * dy;
        let max = (o.radius + m.radius) * OVERLAP_SHRINK_FACTOR;
        if center2 < max * max {
            let dist = center2.sqrt();
            let amount = (1.0 - dist / max).clamp(0.0, 1.0);
            // Rough vibrations in proportion to speed when driving over something.
            let rough =
                ((m.vel[0] * m.vel[0] + m.vel[1] * m.vel[1]) * OVERLAP_ROUGH_VIBRATION_FACTOR).min(MAX_ROUGH_VIBRATION);
            let height = if o.flat { FLATTENED_OBJECT_HEIGHT } else { o.height };
            if amount < FLATTENED_OBJECT_HEIGHT && !o.flat {
                overlap_z = height * 2.0 * amount;
                // A vector along the "surface", then the normal across it.
                let v = [dx / dist, dy / dist, 0.2];
                let up = normalize3([rng.real(-rough, rough), rng.real(-rough, rough), 1.0]);
                let prp = cross(v, up);
                normal = normalize3(cross(prp, v));
            } else {
                // Sitting on top of it.
                overlap_z = height;
                normal = normalize3([rng.real(-rough, rough), rng.real(-rough, rough), 1.0]);
            }
        }
    } else if m.previous_overlap && i.overlap_z > 0.0 {
        // Leaving an overlap while up in the air: a kick to the pitch.
        i.pitch_rate += LEAVE_OVERLAP_PITCH_KICK;
    }
    let (ground_pitch, ground_roll) = ground_pitch_roll(&Motion { normal, ..*m });
    // The ground can only push back while touching it.
    if m.overlap.is_some() || i.overlap_z <= 0.0 {
        i.pitch_rate += -c.pitch_stiffness * (i.pitch - ground_pitch) + -c.pitch_damping * i.pitch_rate;
        if i.pitch_rate > 0.0 {
            i.pitch_rate *= 0.5;
        }
        i.roll_rate += -c.roll_stiffness * (i.roll - ground_roll) + -c.roll_damping * i.roll_rate;
    }
    i.pitch += i.pitch_rate * c.uniform_axial_damping;
    i.roll += i.roll_rate * c.uniform_axial_damping;
    accel_springs(c, i);
    x.pitch = i.pitch + i.accel_pitch;
    x.roll = i.roll + i.accel_roll;
    accel_kick(c, i, m);
    limit(c, i);
    if overlap_z > i.overlap_z {
        i.overlap_z = overlap_z;
        i.overlap_z_vel = 0.0;
    }
    // Fake Z physics for driving over things.
    let half = i.overlap_z / 2.0;
    if i.overlap_z > 0.0 {
        i.overlap_z_vel -= 0.2;
        i.overlap_z += i.overlap_z_vel;
    }
    if i.overlap_z <= 0.0 {
        i.overlap_z = 0.0;
        i.overlap_z_vel = 0.0;
    }
    x.z = half;
}

/// `calcPhysicsXformWheels` and `calcPhysicsXformMotorcycle`, chassis only.
fn wheels(c: &Chassis, i: &mut LocoInfo, m: &Motion, x: &mut Xform, rng: &mut ClientRng, motorcycle: bool) {
    let (ground_pitch, ground_roll) = ground_pitch_roll(m);
    if m.airborne {
        let pitch_height = m.length * ih_math::sin(i.pitch + i.accel_pitch - ground_pitch);
        let roll_height = m.width * ih_math::sin(i.roll + i.accel_roll - ground_roll);
        if !motorcycle {
            // "maintain the same orientation while we fly through the air"
            x.z = pitch_height.abs() / 4.0 + roll_height.abs() / 4.0;
            return;
        }
        x.z = pitch_height.abs() / 4.0;
    }
    // Bouncy.
    let speed = (m.vel[0] * m.vel[0] + m.vel[1] * m.vel[1] + m.vel[2] * m.vel[2]).sqrt();
    if !m.airborne && m.max_speed > 0.0 && speed > m.max_speed / 10.0 {
        let factor = speed / m.max_speed;
        let kick = c.bounce_kick;
        if i.pitch_rate.abs() < factor * kick / 4.0 && i.roll_rate.abs() < factor * kick / 8.0 {
            let (p, r) = match rng.int(0, 3) {
                0 => (-1.0, -1.0),
                1 => (1.0, -1.0),
                2 => (-1.0, 1.0),
                _ => (1.0, 1.0),
            };
            i.pitch_rate += p * kick * factor;
            i.roll_rate += r * kick * factor / 2.0;
        }
    }
    if !m.airborne {
        i.pitch_rate += -c.pitch_stiffness * (i.pitch - ground_pitch) + -c.pitch_damping * i.pitch_rate;
        if !motorcycle && i.pitch_rate > 0.0 {
            i.pitch_rate *= 0.5;
        }
        i.roll_rate += -c.roll_stiffness * (i.roll - ground_roll) + -c.roll_damping * i.roll_rate;
    } else {
        // Motorcycles auto-level in the air.
        i.pitch_rate += -c.pitch_stiffness * i.pitch + -c.pitch_damping * i.pitch_rate;
        i.roll_rate += -c.roll_stiffness * i.roll + -c.roll_damping * i.roll_rate;
    }
    i.pitch += i.pitch_rate * c.uniform_axial_damping;
    i.roll += i.roll_rate * c.uniform_axial_damping;
    accel_springs(c, i);
    x.pitch = i.pitch + i.accel_pitch;
    // Motorcycles keep `unclampedRoll > 0.5f && unclampedRoll < -0.5f ? unclampedRoll
    // : 0.0f`, which is never true: they never roll.
    x.roll = if motorcycle { 0.0 } else { i.roll + i.accel_roll };
    accel_kick(c, i, m);
    limit(c, i);
    x.z = 0.0;
    let pitch_height = m.length * ih_math::sin(x.pitch - ground_pitch);
    let roll_height = m.width * ih_math::sin(x.roll - ground_roll);
    // Above 22 degrees the chassis rises.
    let pitch = (x.pitch - ground_pitch).abs();
    let mut divisor = 4.0;
    if pitch > PI / 8.0 {
        divisor = ((4.0 * PI / 8.0) + (pitch - PI / 8.0)) / pitch;
    }
    x.z += pitch_height.abs() / divisor;
    x.z += roll_height.abs() / divisor;
}

/// `calcPhysicsXformHoverOrWings`.
fn hover_or_wings(c: &Chassis, i: &mut LocoInfo, m: &Motion, x: &mut Xform) {
    i.pitch_rate += -c.pitch_stiffness * i.pitch + -c.pitch_damping * i.pitch_rate;
    i.roll_rate += -c.roll_stiffness * i.roll + -c.roll_damping * i.roll_rate;
    i.pitch += i.pitch_rate * c.uniform_axial_damping;
    i.roll += i.roll_rate * c.uniform_axial_damping;
    accel_springs(c, i);
    x.pitch = i.pitch + i.accel_pitch;
    x.roll = i.roll + i.accel_roll;
    if m.motive {
        if c.pitch_by_z_vel != 0.0 && m.vel[2].abs() > 0.001 {
            let pitch = ih_math::atan2(m.vel[2], (m.vel[0] * m.vel[0] + m.vel[1] * m.vel[1]).sqrt());
            i.pitch -= c.pitch_by_z_vel * pitch;
        }
        // Pitch and roll in reaction to the current speed.
        let forward = m.dir[0] * m.vel[0] + m.dir[1] * m.vel[1];
        i.pitch += -(c.forward_vel * forward);
        let lateral = -m.dir[1] * m.vel[0] + m.dir[0] * m.vel[1];
        i.roll += -(c.lateral_vel * lateral);
    }
    accel_kick(c, i, m);
    limit(c, i);
    i.yaw_modulator += c.rudder_rate;
    x.yaw = c.rudder_degree * ih_math::sin(i.yaw_modulator);
    i.pitch_modulator += c.elevator_rate;
    x.pitch += c.elevator_degree * ih_math::cos(i.pitch_modulator);
    x.z = 0.0;
}

/// `calcPhysicsXformThrust`: "a kind of quick thrust implementation cause we need
/// scud missiles to wobble *now*".
fn thrust(c: &Chassis, i: &mut LocoInfo, x: &mut Xform) {
    let rate = c.wobble_rate;
    if rate != 0.0 {
        if i.wobble >= 1.0 {
            let step = if i.pitch < c.max_wobble - rate * 2.0 { rate } else { rate / 2.0 };
            i.pitch += step;
            i.yaw += step;
            if i.pitch >= c.max_wobble {
                i.wobble = -1.0;
            }
        } else {
            let step = if i.pitch >= c.min_wobble + rate * 2.0 { rate } else { rate / 2.0 };
            i.pitch -= step;
            i.yaw -= step;
            if i.pitch <= c.min_wobble {
                i.wobble = 1.0;
            }
        }
        x.pitch = i.pitch;
        x.yaw = i.yaw;
    }
    if c.thrust_roll != 0.0 {
        i.roll += c.thrust_roll;
        x.roll = i.roll;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn motion(normal: [f32; 3]) -> Motion {
        Motion {
            dir: [1.0, 0.0],
            vel: [0.0; 3],
            accel: [0.0; 3],
            motive: false,
            height_above: 0.0,
            normal,
            airborne: false,
            max_speed: 1.0,
            length: 10.0,
            width: 5.0,
            position: [0.0, 0.0],
            radius: 10.0,
            overlap: None,
            previous_overlap: false,
        }
    }

    #[test]
    fn treads_settle_to_the_slope_under_them() {
        let c = Chassis { kind: ChassisKind::Treads, ..Default::default() };
        let mut i = LocoInfo::default();
        let mut rng = ClientRng::new(1);
        // A slope rising ahead: the normal leans back (-x), so the nose pitches up (negative).
        let n = [-0.2f32, 0.0, (1.0f32 - 0.04).sqrt()];
        let mut x = Xform::default();
        for _ in 0..300 {
            x = step(&c, &mut i, &motion(n), &mut rng).unwrap();
        }
        assert!((x.pitch - (-0.2 * PI / 2.0)).abs() < 1e-3, "{x:?}");
        assert!(x.roll.abs() < 1e-6);
    }

    #[test]
    fn acceleration_pitches_the_chassis_back_within_its_limit() {
        let c = Chassis {
            kind: ChassisKind::Treads,
            forward_accel: 10.0,
            accel_pitch_limit: 0.1,
            decel_pitch_limit: 0.2,
            ..Default::default()
        };
        let mut i = LocoInfo::default();
        let mut rng = ClientRng::new(1);
        let mut m = motion([0.0, 0.0, 1.0]);
        m.motive = true;
        m.accel = [0.5, 0.0, 0.0];
        let mut lowest = 0.0f32;
        for _ in 0..30 {
            let x = step(&c, &mut i, &m, &mut rng).unwrap();
            lowest = lowest.min(x.pitch);
        }
        assert!(lowest < -0.05, "pitched back: {lowest}");
        assert!(i.accel_pitch >= -0.1 - 1e-6, "limited: {}", i.accel_pitch);
    }

    #[test]
    fn treads_rise_over_an_overlapped_object_and_settle_after() {
        let c = Chassis { kind: ChassisKind::Treads, ..Default::default() };
        let mut i = LocoInfo::default();
        let mut rng = ClientRng::new(1);
        let mut m = motion([0.0, 0.0, 1.0]);
        // A crushable car 3 tall right ahead, half way under the tank.
        m.overlap = Some(Overlap { position: [8.0, 0.0], radius: 6.0, height: 3.0, flat: false });
        let x = step(&c, &mut i, &m, &mut rng).unwrap();
        assert!(i.overlap_z > 0.0 && x.pitch < 0.0, "{x:?} {i:?}");
        // Flat things (infantry) only lift by half a unit.
        let mut flat = LocoInfo::default();
        m.overlap = Some(Overlap { position: [2.0, 0.0], radius: 3.0, height: 3.0, flat: true });
        // The drawn lift is half the height before this frame's fall (0.5 - 0.2).
        assert_eq!(step(&c, &mut flat, &m, &mut rng).unwrap().z, 0.25);
        assert!((flat.overlap_z - 0.3).abs() < 1e-6);
        // Clear of it, the fake z falls back to the ground.
        m.overlap = None;
        m.previous_overlap = true;
        for _ in 0..30 {
            step(&c, &mut i, &m, &mut rng).unwrap();
        }
        assert_eq!(i.overlap_z, 0.0);
    }

    #[test]
    fn thrust_wobbles_between_its_limits_and_motorcycles_never_roll() {
        let c = Chassis {
            kind: ChassisKind::Thrust,
            wobble_rate: 0.01,
            min_wobble: -0.05,
            max_wobble: 0.05,
            ..Default::default()
        };
        let mut i = LocoInfo::default();
        let mut rng = ClientRng::new(1);
        let m = motion([0.0, 0.0, 1.0]);
        let pitches: Vec<f32> = (0..40).map(|_| step(&c, &mut i, &m, &mut rng).unwrap().pitch).collect();
        let (lo, hi) = pitches.iter().fold((f32::MAX, f32::MIN), |(a, b), p| (a.min(*p), b.max(*p)));
        assert!(hi >= 0.05 - 1e-6 && lo <= -0.05 + 1e-6, "{lo} {hi}");
        let c = Chassis { kind: ChassisKind::Motorcycle, ..Default::default() };
        let mut i = LocoInfo::default();
        let tilted = motion([0.0, 0.3, 0.95]);
        for _ in 0..50 {
            assert_eq!(step(&c, &mut i, &tilted, &mut rng).unwrap().roll, 0.0);
        }
    }
}
