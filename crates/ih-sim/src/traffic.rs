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
    /// `KINDOF_INFANTRY`: infantry pass through infantry.
    pub infantry: bool,
    pub position: [f32; 3],
    pub radius: f32,
    /// The geometry for narrow-phase tests (`radius` is its bounding circle).
    pub geom: Geom,
    pub major: f32,
    pub minor: f32,
    pub bottom: f32,
    pub top: f32,
    pub angle: f32,
    pub priority: u8,
    pub moving: bool,
    pub enabled: bool,
    /// `KINDOF_IMMOBILE` or a structure: kept out of by obstacle cells, not by clipping.
    pub immobile: bool,
}
/// `GeometryType`.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum Geom {
    #[default]
    Sphere,
    Cylinder,
    Box,
}

/// The template-derived part of a collider, worked out once when the object is
/// created (`GeometryInfo::calcBoundingStuff` and the collision filters).
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct BodyShape {
    pub infantry: bool,
    pub radius: f32,
    pub geom: Geom,
    pub major: f32,
    pub minor: f32,
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
        let geom = match t.values("Geometry").first().map(|g| g.to_ascii_uppercase()).as_deref() {
            Some("BOX") => Geom::Box,
            Some("CYLINDER") => Geom::Cylinder,
            _ => Geom::Sphere,
        };
        let major = t.number("GeometryMajorRadius", 5.0).max(0.0) * scale;
        let minor = t.number("GeometryMinorRadius", 0.0).max(0.0) * scale;
        Self {
            infantry: t.has_kind("INFANTRY"),
            radius,
            geom,
            major,
            minor: if geom == Geom::Box { minor } else { major },
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
            infantry: s.infantry,
            position: o.position,
            radius: s.radius,
            geom: s.geom,
            major: s.major,
            minor: s.minor,
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
/// Fraction of a proposed step before first contact. Two round bodies are swept
/// exactly; with a box involved the step is bisected against the narrow phase
/// (steps are small next to bodies, so the midpoint guards against tunnelling).
/// Already overlapping bodies may move apart but not closer.
/// Infantry pass through infantry, as in the original, where AI units get no
/// collision push and `moveAllies` lets "infantry walk through other infantry".
pub fn step_fraction(body: Collider, velocity: [f32; 2], other: Collider) -> f32 {
    if !body.interacts(other) || (body.infantry && other.infantry) {
        return 1.0;
    }
    if body.geom == Geom::Box || other.geom == Geom::Box {
        let at = |t: f32| {
            body.with_position([
                body.position[0] + velocity[0] * t,
                body.position[1] + velocity[1] * t,
                body.position[2],
            ])
        };
        let hit = |t: f32| shapes_touch(&at(t), &other, 0.0);
        if hit(0.0) {
            let away = (body.position[0] - other.position[0]) * velocity[0]
                + (body.position[1] - other.position[1]) * velocity[1];
            return if away >= 0.0 { 1.0 } else { 0.0 };
        }
        if !hit(1.0) && !hit(0.5) {
            return 1.0;
        }
        let (mut lo, mut hi) = (0.0f32, if hit(0.5) { 0.5 } else { 1.0 });
        for _ in 0..12 {
            let mid = (lo + hi) * 0.5;
            if hit(mid) {
                hi = mid;
            } else {
                lo = mid;
            }
        }
        let speed = ih_math::length(velocity);
        return if speed > 0.0 { (lo - CONTACT_GAP / speed).max(0.0) } else { lo };
    }
    step_fraction_round(body, velocity, other)
}
/// The exact sweep of two round bodies.
fn step_fraction_round(body: Collider, velocity: [f32; 2], other: Collider) -> f32 {
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
/// `rectToFourPoints`: a rect's corners (tl, tr, bl, br) at its facing.
fn corners(c: &Collider, grow: f32) -> [[f32; 2]; 4] {
    let (major, minor) = (c.major + grow, c.minor + grow);
    let (cs, sn) = (ih_math::cos(c.angle), ih_math::sin(c.angle));
    let (exc, eyc, exs, eys) = (major * cs, minor * cs, major * sn, minor * sn);
    let [x, y, _] = c.position;
    [
        [x - exc - eys, y + eyc - exs],
        [x + exc - eys, y + eyc + exs],
        [x - exc + eys, y - eyc - exs],
        [x + exc + eys, y - eyc + exs],
    ]
}
/// `testRotatedPointsAgainstRect`: how many of the points lie inside the rect.
fn points_inside(pts: &[[f32; 2]; 4], c: &Collider, grow: f32) -> usize {
    let (major, minor) = (c.major + grow, c.minor + grow);
    let (cs, sn) = (ih_math::cos(-c.angle), ih_math::sin(-c.angle));
    pts.iter()
        .filter(|p| {
            let (x, y) = (p[0] - c.position[0], p[1] - c.position[1]);
            (x * cs - y * sn).abs() <= major && (x * sn + y * cs).abs() <= minor
        })
        .count()
}
/// The 2D narrow phase (`PartitionManager.cpp`): `xy_collideTest_Rect_Rect` (any
/// corner of one inside the other), circles against rects as squares at their own
/// facing (`xy_collideTest_Rect_Circle`), circle against circle by radii, and
/// non-spheres whose centres are closer than the smallest radius always touch
/// (`z_collideTest_Nonsphere_Nonsphere`). `grow` widens `a` (contact range) or,
/// negative, shrinks it.
pub fn shapes_touch(a: &Collider, b: &Collider, grow: f32) -> bool {
    let d2 = (a.position[0] - b.position[0]).powi(2) + (a.position[1] - b.position[1]).powi(2);
    let round = |g: Geom| g != Geom::Box;
    if round(a.geom) && round(b.geom) {
        let r = (a.major + grow + b.major).max(0.0);
        return d2 <= r * r;
    }
    if a.geom != Geom::Sphere && b.geom != Geom::Sphere {
        let min = (a.major + grow).min(a.minor + grow).min(b.major).min(b.minor);
        if min > 0.0 && min * min > d2 {
            return true;
        }
    }
    // Circles become squares (minor = major) at their own facing.
    points_inside(&corners(a, grow), b, 0.0) > 0 || points_inside(&corners(b, 0.0), a, grow) > 0
}

/// The no-overlap invariant. Round bodies may not overlap at all. With a box
/// involved, turning lets corners overlap a little, as the original allows; a
/// breach is one body's centre inside the other.
pub fn overlaps(a: Collider, b: Collider) -> bool {
    if !a.interacts(b) || (a.infantry && b.infantry) {
        return false;
    }
    if a.geom != Geom::Box && b.geom != Geom::Box {
        // Float rounding at exact contact does not count as penetration.
        return shapes_touch(&a, &b, -0.005);
    }
    let centre = |c: &Collider| Collider { geom: Geom::Cylinder, major: 0.0, minor: 0.0, ..*c };
    shapes_touch(&centre(&a), &b, 0.0) || shapes_touch(&centre(&b), &a, 0.0)
}
/// Touching within `CONTACT_RANGE` (bodies never overlap here, so contact means close).
pub fn contact(a: Collider, b: Collider) -> bool {
    a.interacts(b) && shapes_touch(&a, &b, CONTACT_RANGE)
}

#[cfg(test)]
mod tests {
    use super::*;
    fn body(id: u32, x: f32) -> Collider {
        Collider {
            id,
            infantry: false,
            position: [x, 0.0, 0.0],
            radius: 5.0,
            geom: Geom::Cylinder,
            major: 5.0,
            minor: 5.0,
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
        let small = Collider { radius: 1.0, major: 1.0, minor: 1.0, ..a };
        let distant = Collider { radius: 1.0, major: 1.0, minor: 1.0, ..body(2, 2500.0) };
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
