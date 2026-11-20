//! Truck wheels, from `W3DTruckDraw.cpp` (and the wheel part of
//! `W3DTankTruckDraw.cpp`): tires spin with speed (backwards when reversing, the
//! rear faster while powersliding), front tires steer and ride their suspension
//! offsets, and a cab and trailer swing into turns. Bones are driven like
//! `Capture_Bone`/`Control_Bone`. Dust, dirt and powerslide effects and the
//! powerslide sound wait for particles and audio.
use crate::chassis::WheelInfo;
use glam::Mat4;
use ih_formats::w3d::Hierarchy;
use ih_ini::Block;

/// `W3DTruckDrawModuleData`.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct TruckDraw {
    pub front: [String; 2],
    pub rear: [String; 2],
    pub mid_front: [String; 2],
    pub mid_rear: [String; 2],
    pub mid_mid: [String; 2],
    pub cab: String,
    pub trailer: String,
    pub rotation_multiplier: f32,
    pub powerslide_addition: f32,
    pub cab_factor: f32,
    pub trailer_factor: f32,
    pub damping: f32,
    /// A `PowerslideSpray` is set (the original only powerslides with one).
    pub powerslide_spray: bool,
}

impl TruckDraw {
    /// For `W3DTruckDraw` and `W3DTankTruckDraw` modules; `None` for others.
    pub fn parse(class: &str, block: &Block) -> Option<Self> {
        if class != "W3DTruckDraw" && class != "W3DTankTruckDraw" {
            return None;
        }
        let text = |key: &str| {
            block
                .fields()
                .filter(|f| f.name.eq_ignore_ascii_case(key))
                .last()
                .and_then(|f| f.values().first().cloned())
                .unwrap_or_default()
        };
        let real = |key: &str| ih_ini::values::scan_real(text(key).as_bytes()).unwrap_or(0.0);
        let pair = |l: &str, r: &str| [text(l), text(r)];
        Some(Self {
            front: pair("LeftFrontTireBone", "RightFrontTireBone"),
            rear: pair("LeftRearTireBone", "RightRearTireBone"),
            mid_front: pair("MidLeftFrontTireBone", "MidRightFrontTireBone"),
            mid_rear: pair("MidLeftRearTireBone", "MidRightRearTireBone"),
            mid_mid: pair("MidLeftMidTireBone", "MidRightMidTireBone"),
            cab: text("CabBone"),
            trailer: text("TrailerBone"),
            rotation_multiplier: real("TireRotationMultiplier"),
            powerslide_addition: real("PowerslideRotationAddition"),
            cab_factor: real("CabRotationMultiplier"),
            trailer_factor: real("TrailerRotationMultiplier"),
            damping: real("RotationDamping"),
            powerslide_spray: !text("PowerslideSpray").is_empty(),
        })
    }
}

/// Per drawable: wheel spin and cab/trailer swing.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct TruckState {
    pub front_rotation: f32,
    pub rear_rotation: f32,
    pub cab_rotation: f32,
    pub trailer_rotation: f32,
    pub powersliding: bool,
}

/// What one update reads from the logic object.
#[derive(Clone, Copy, Debug)]
pub struct TruckMotion {
    /// `getVelocityMagnitude`.
    pub speed: f32,
    pub backwards: bool,
    pub motive: bool,
    pub airborne: bool,
    pub turning: i8,
    /// `getRelativeAngle2D` to the point on the path, if there is a path.
    pub angle_to_path: Option<f32>,
}

/// One `doDrawModule` update of the wheel state.
pub fn step(d: &TruckDraw, s: &mut TruckState, w: &WheelInfo, m: &TruckMotion) {
    // The cab turns with the wheels, limited towards the path; the trailer the other way.
    if !d.cab.is_empty() {
        let mut desired = w.angle * d.cab_factor;
        if let Some(to_goal) = m.angle_to_path {
            if to_goal < 0.0 {
                desired = desired.max(to_goal).min(0.0);
            } else {
                desired = desired.min(to_goal).max(0.0);
            }
        }
        s.cab_rotation += (desired - s.cab_rotation) * d.damping;
        if !d.trailer.is_empty() {
            let desired = -w.angle * d.trailer_factor;
            s.trailer_rotation += (desired - s.trailer_rotation) * d.damping;
        }
    }
    let (mut speed, mut addition) = (m.speed, d.powerslide_addition);
    if m.backwards {
        speed = -speed; // "rotate wheels backwards"
        addition = -addition;
    }
    s.front_rotation += d.rotation_multiplier * speed;
    s.rear_rotation += d.rotation_multiplier * if s.powersliding { speed + addition } else { speed };
    s.powersliding = m.motive && !m.airborne && d.powerslide_spray && m.turning != 0;
}

/// The bone controls for this state (`Control_Bone` relative transforms).
pub fn controls(d: &TruckDraw, s: &TruckState, w: &WheelInfo, h: &Hierarchy) -> Vec<(usize, Mat4)> {
    let bone = |name: &str| {
        (!name.is_empty()).then(|| h.pivots.iter().position(|p| p.name.eq_ignore_ascii_case(name))).flatten()
    };
    let mut out = Vec::new();
    if let Some(cab) = bone(&d.cab) {
        out.push((cab, Mat4::from_rotation_z(s.cab_rotation)));
        if let Some(trailer) = bone(&d.trailer) {
            out.push((trailer, Mat4::from_rotation_z(s.trailer_rotation)));
        }
    }
    // Front tires: lift, steer, spin. Rear tires: spin, lift.
    let front = |z: f32, spin: f32| {
        Mat4::from_translation(glam::Vec3::new(0.0, 0.0, z))
            * Mat4::from_rotation_z(w.angle)
            * Mat4::from_rotation_y(spin)
    };
    let rear = |z: f32, spin: f32| Mat4::from_translation(glam::Vec3::new(0.0, 0.0, z)) * Mat4::from_rotation_y(spin);
    let mut pair = |names: &[String; 2], make: &dyn Fn(f32, f32) -> Mat4, offsets: [f32; 2], spin: f32, both: bool| {
        let (l, r) = (bone(&names[0]), bone(&names[1]));
        // The mid pairs need both bones (`if (!right) left = 0`).
        if both && (l.is_none() || r.is_none()) {
            return;
        }
        if let Some(l) = l {
            out.push((l, make(offsets[0], spin)));
            if let Some(r) = r {
                out.push((r, make(offsets[1], spin)));
            }
        }
    };
    pair(&d.front, &front, [w.front_left, w.front_right], s.front_rotation, false);
    pair(&d.rear, &rear, [w.rear_left, w.rear_right], s.rear_rotation, false);
    pair(&d.mid_front, &front, [w.front_left, w.front_right], s.front_rotation, true);
    pair(&d.mid_rear, &rear, [w.rear_left, w.rear_right], s.rear_rotation, true);
    pair(&d.mid_mid, &rear, [w.rear_left, w.rear_right], s.rear_rotation, true);
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn truck() -> TruckDraw {
        TruckDraw {
            front: ["TIRE01".into(), "TIRE02".into()],
            rear: ["TIRE03".into(), "TIRE04".into()],
            cab: "CAB".into(),
            rotation_multiplier: 0.2,
            powerslide_addition: 1.0,
            cab_factor: 1.0,
            damping: 0.5,
            powerslide_spray: true,
            ..Default::default()
        }
    }
    fn motion(speed: f32) -> TruckMotion {
        TruckMotion { speed, backwards: false, motive: true, airborne: false, turning: 0, angle_to_path: None }
    }

    #[test]
    fn tires_spin_with_speed_and_backwards_when_reversing() {
        let (d, w) = (truck(), WheelInfo::default());
        let mut s = TruckState::default();
        step(&d, &mut s, &w, &motion(2.0));
        assert!((s.front_rotation - 0.4).abs() < 1e-6 && (s.rear_rotation - 0.4).abs() < 1e-6);
        step(&d, &mut s, &w, &TruckMotion { backwards: true, ..motion(2.0) });
        assert!(s.front_rotation.abs() < 1e-6);
    }

    #[test]
    fn powersliding_spins_the_rear_faster_and_the_cab_follows_the_wheels() {
        let d = truck();
        let w = WheelInfo { angle: 0.3, ..Default::default() };
        let mut s = TruckState::default();
        step(&d, &mut s, &w, &TruckMotion { turning: 1, ..motion(1.0) });
        assert!(s.powersliding);
        let before = (s.front_rotation, s.rear_rotation);
        step(&d, &mut s, &w, &TruckMotion { turning: 1, ..motion(1.0) });
        assert!((s.rear_rotation - before.1) - (s.front_rotation - before.0) > 0.19, "{s:?}");
        // Damped halfway towards the wheel angle each update.
        assert!((s.cab_rotation - 0.225).abs() < 1e-6, "{s:?}");
        // Limited towards the path: the path is straight ahead, so the cab returns.
        step(&d, &mut s, &w, &TruckMotion { angle_to_path: Some(0.0), ..motion(1.0) });
        assert!(s.cab_rotation < 0.225);
    }
}
