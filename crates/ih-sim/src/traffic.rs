//! Initial live ground traffic. AIUpdate.cpp supplies the priority rules and
//! blocked/follow/repath structure; Geometry.cpp supplies bounding circles.
//! Swept disc clipping is an ironhour guard for the initial corridor integrator,
//! not a port of the original rigid-body/crushing collision implementation.
use crate::{
    simulation::{ModuleKind, Object},
    templates::{Template, Templates},
};

pub const REPATH_DELAY: u32 = 30;
pub const CONTACT_GAP: f32 = 0.02;

#[derive(Clone, Copy, Debug)]
pub struct Collider {
    pub id: u32,
    pub position: [f32; 3],
    pub radius: f32,
    pub bottom: f32,
    pub top: f32,
    pub angle: f32,
    pub priority: u8,
    pub moving: bool,
    pub enabled: bool,
    /// `KINDOF_IMMOBILE` or a structure: kept out of by obstacle cells, not by clipping.
    pub immobile: bool,
}
/// The template-derived part of a collider, worked out once when the object is
/// created (`GeometryInfo::calcBoundingStuff` and the collision filters).
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct BodyShape {
    pub radius: f32,
    pub bottom: f32,
    pub top: f32,
    pub priority: u8,
    pub collides: bool,
    pub immobile: bool,
}
impl BodyShape {
    pub fn new(t: &Template, modules: &[crate::simulation::Module], templates: &Templates) -> Self {
        let scale = t.number("Scale", 1.0).max(0.0);
        let radius = t.collision_radius();
        let sphere = t.values("Geometry").first().is_some_and(|v| v == "SPHERE");
        let height = if sphere { radius } else { t.number("GeometryHeight", 5.0).max(0.01) * scale };
        let immobile = t.has_kind("IMMOBILE") || t.has_kind("STRUCTURE");
        Self {
            radius,
            bottom: if sphere { -radius } else { 0.0 },
            top: height,
            priority: if t.has_kind("DOZER") {
                2
            } else if t.has_kind("VEHICLE") {
                1
            } else {
                0
            },
            collides: !t.has_kind("NO_COLLIDE")
                && (immobile
                    || (modules.iter().any(|m| m.kind == ModuleKind::Physics)
                        && t.locomotor()
                            .and_then(|n| templates.locomotors.get(n))
                            .is_some_and(|l| l.surface_movement()))),
            immobile,
        }
    }
}
impl Collider {
    pub fn object(o: &Object) -> Self {
        let s = o.body;
        Self {
            id: o.id,
            position: o.position,
            radius: s.radius,
            bottom: s.bottom,
            top: s.top,
            angle: o.angle,
            priority: s.priority,
            moving: o.target.is_some() && !o.disabled.any(),
            enabled: s.collides,
            immobile: s.immobile,
        }
    }
    pub fn with_position(self, position: [f32; 3]) -> Self {
        Self { position, ..self }
    }
    pub fn interacts(self, other: Self) -> bool {
        self.enabled
            && other.enabled
            && self.id != other.id
            && self.position[2] + self.bottom < other.position[2] + other.top
            && other.position[2] + other.bottom < self.position[2] + self.top
    }
    /// Ungrouped AIUpdate::hasHigherPathPriority: dozer, vehicle over infantry,
    /// front unit along combined headings, then object ID for a tie/head-on.
    pub fn higher_priority(self, other: Self) -> bool {
        if self.priority != other.priority {
            return self.priority > other.priority;
        }
        let a = [ih_math::cos(self.angle), ih_math::sin(self.angle)];
        let b = [ih_math::cos(other.angle), ih_math::sin(other.angle)];
        if a[0] * b[0] + a[1] * b[1] > 0.0 {
            let dot = (a[0] + b[0]) * (other.position[0] - self.position[0])
                + (a[1] + b[1]) * (other.position[1] - self.position[1]);
            if dot != 0.0 {
                return dot < 0.0;
            }
        }
        self.id < other.id
    }
}

/// Fraction of a proposed step before first contact. Already overlapping units
/// may move out, but cannot deepen the overlap. This also catches a single fast
/// step that would otherwise start and end on opposite sides of another unit.
pub fn step_fraction(body: Collider, velocity: [f32; 2], other: Collider) -> f32 {
    if !body.interacts(other) {
        return 1.0;
    }
    let p = [body.position[0] - other.position[0], body.position[1] - other.position[1]];
    let a = velocity[0] * velocity[0] + velocity[1] * velocity[1];
    if a == 0.0 {
        return 1.0;
    }
    let dot = p[0] * velocity[0] + p[1] * velocity[1];
    if dot >= 0.0 {
        return 1.0;
    }
    let radius = body.radius + other.radius;
    let c = p[0] * p[0] + p[1] * p[1] - radius * radius;
    if c <= 0.0 {
        return 0.0;
    }
    // Closest approach avoids cancellation between large squared products.
    let closest = -dot / a;
    let q = [p[0] + velocity[0] * closest, p[1] + velocity[1] * closest];
    let inside = radius * radius - (q[0] * q[0] + q[1] * q[1]);
    if inside <= 0.0 {
        return 1.0;
    }
    let t = closest - ih_math::sqrt(inside / a);
    if !(0.0..=1.0).contains(&t) {
        return 1.0;
    }
    (t - CONTACT_GAP / ih_math::sqrt(a)).max(0.0)
}
pub fn overlaps(a: Collider, b: Collider) -> bool {
    if !a.interacts(b) {
        return false;
    }
    let d = [a.position[0] - b.position[0], a.position[1] - b.position[1]];
    // Float rounding at exact contact does not count as penetration.
    let r = (a.radius + b.radius - 0.005).max(0.0);
    d[0] * d[0] + d[1] * d[1] < r * r
}

#[cfg(test)]
mod tests {
    use super::*;
    fn body(id: u32, x: f32) -> Collider {
        Collider {
            id,
            position: [x, 0.0, 0.0],
            radius: 5.0,
            bottom: 0.0,
            top: 10.0,
            angle: 0.0,
            priority: 1,
            moving: true,
            enabled: true,
            immobile: false,
        }
    }
    #[test]
    fn sweep_catches_tunnelling_and_allows_overlap_escape() {
        let a = body(1, 0.0);
        let b = body(2, 50.0);
        let fraction = step_fraction(a, [100.0, 0.0], b);
        assert!(fraction > 0.39 && fraction < 0.4);
        assert!(!overlaps(a.with_position([100.0 * fraction, 0.0, 0.0]), b));
        assert_eq!(step_fraction(body(1, 45.0), [1.0, 0.0], b), 0.0);
        assert_eq!(step_fraction(body(1, 45.0), [-1.0, 0.0], b), 1.0);
        assert_eq!(step_fraction(a, [0.0, 100.0], b), 1.0);
        let small = Collider { radius: 1.0, ..a };
        let distant = Collider { radius: 1.0, ..body(2, 2500.0) };
        let f = step_fraction(small, [5000.0, 0.0], distant);
        assert!(f > 0.499 && f < 0.5);
        assert!(!overlaps(small.with_position([5000.0 * f, 0.0, 0.0]), distant));
    }
    #[test]
    fn deck_clearance_no_collide_and_source_priority_rules() {
        let a = body(1, 0.0);
        let mut b = body(2, 0.0);
        b.position[2] = 20.0;
        assert!(!overlaps(a, b));
        b.position[2] = 5.0;
        assert!(overlaps(a, b));
        b.enabled = false;
        assert_eq!(step_fraction(a, [100.0, 0.0], b), 1.0);
        b = body(2, 20.0);
        assert!(!a.higher_priority(b)); // the front unit wins
        b.angle = std::f32::consts::PI;
        assert!(a.higher_priority(b)); // head-on tie uses ID
        b.priority = 2;
        assert!(b.higher_priority(a));
    }
}

/// `FAST_AS_POSSIBLE`: no speed limit.
pub const FAST_AS_POSSIBLE: f32 = 999_999.0;
/// Bodies within this gap of touching are reported as colliding. Swept clipping
/// keeps ground bodies from overlapping, so the original's overlap test is widened
/// to contact.
pub const CONTACT_RANGE: f32 = 1.0;

/// `AIUpdateInterface` traffic state (`m_isBlocked`, `m_isBlockedAndStuck`,
/// `m_curMaxBlockedSpeed`, `m_bumpSpeedLimit`); `Object::blocked_frames` is
/// `m_blockedFrames`.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct TrafficState {
    pub is_blocked: bool,
    pub stuck: bool,
    pub max_blocked_speed: f32,
    pub bump_speed_limit: f32,
    /// `m_ignoreCollisionsUntil`.
    pub ignore_until: u32,
}
impl Default for TrafficState {
    fn default() -> Self {
        Self {
            is_blocked: false,
            stuck: false,
            max_blocked_speed: FAST_AS_POSSIBLE,
            bump_speed_limit: FAST_AS_POSSIBLE,
            ignore_until: 0,
        }
    }
}
