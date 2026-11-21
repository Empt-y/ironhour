//! Projectiles, from `DumbProjectileBehavior.cpp`, `BezierSegment.cpp`,
//! `BezFwdIterator.cpp`, `WeaponTemplate::fireWeaponTemplate`'s projectile
//! branch, `WeaponStore::handleProjectileDetonation`,
//! `WeaponTemplate::shouldProjectileCollideWith`, `PhysicsBehavior::onCollide`'s
//! projectile hand-off and `PartitionManager::estimateTerrainExtremesAlongLine`.
//!
//! A weapon with a `ProjectileObject` creates that object. A dumb projectile
//! flies a Bezier arc (control points raised above the highest terrain along
//! the line), one segment per frame at the weapon's speed, bending towards a
//! moving victim if allowed, and detonates at the end of the arc, at the end
//! of its lifespan, or when it hits something its weapon collides with; the
//! detonation fires the weapon again at the projectile's position, crediting
//! the launcher.
//!
//! Not yet: launch bones (projectiles start at the launcher's position, as
//! when a model has no launch bone), launcher velocity hand-off, the layer
//! changes under bridges, garrison-clearing hits, tumbling (its random rates are
//! drawn), and other projectile behaviours: a projectile template without a
//! `DumbProjectileBehavior` (missiles) still deals its damage after the flight
//! time, as before.
use crate::{
    Simulation,
    body::{DAMAGE_UNRESISTABLE, DamageInfo},
    death::duration,
    weapons::{self, WeaponTemplate},
};

/// `DumbProjectileBehaviorModuleData`.
#[derive(Clone, Debug, PartialEq)]
pub struct DumbProjectileData {
    pub first_height: f32,
    pub second_height: f32,
    pub first_indent: f32,
    pub second_indent: f32,
    pub max_lifespan: u32,
    pub tumble: bool,
    pub orient: bool,
    pub detonate_calls_kill: bool,
    /// Per frame.
    pub adjust_per_frame: f32,
}
impl DumbProjectileData {
    pub fn parse(block: &ih_ini::Block) -> Self {
        let mut d = Self {
            first_height: 0.0,
            second_height: 0.0,
            first_indent: 0.0,
            second_indent: 0.0,
            max_lifespan: 10 * 30,
            tumble: false,
            orient: true,
            detonate_calls_kill: false,
            adjust_per_frame: 0.0,
        };
        for f in block.fields() {
            let v = f.values();
            let real = || {
                v.first()
                    .and_then(|s| ih_ini::values::scan_real(s.trim_end_matches('%').as_bytes()).ok())
                    .unwrap_or(0.0)
            };
            let yes = || v.first().is_some_and(|s| matches!(s.to_ascii_lowercase().as_str(), "yes" | "true" | "1"));
            match f.name.as_str() {
                "FirstHeight" => d.first_height = real(),
                "SecondHeight" => d.second_height = real(),
                "FirstPercentIndent" => d.first_indent = real() / 100.0,
                "SecondPercentIndent" => d.second_indent = real() / 100.0,
                "MaxLifespan" => d.max_lifespan = duration(real()),
                "TumbleRandomly" => d.tumble = yes(),
                "OrientToFlightPath" => d.orient = yes(),
                "DetonateCallsKill" => d.detonate_calls_kill = yes(),
                "FlightPathAdjustDistPerSecond" => d.adjust_per_frame = real() * (1.0 / 30.0),
                _ => {}
            }
        }
        d
    }
}

/// A dumb projectile's flight.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Flight {
    pub launcher: u32,
    pub victim: u32,
    pub weapon: String,
    pub lifespan_frame: u32,
    pub path: Vec<[f32; 3]>,
    pub start: [f32; 3],
    pub end: [f32; 3],
    pub speed: f32,
    pub segments: i32,
    pub step: usize,
    pub detonated: bool,
}

/// `BezierSegment` with its forward-difference iterator.
#[derive(Clone, Copy, Debug)]
struct Bezier([[f32; 3]; 4]);
impl Bezier {
    fn sub(a: [f32; 3], b: [f32; 3]) -> [f32; 3] {
        [a[0] - b[0], a[1] - b[1], a[2] - b[2]]
    }
    fn len(v: [f32; 3]) -> f32 {
        ih_math::sqrt(v[0] * v[0] + v[1] * v[1] + v[2] * v[2])
    }
    /// The cubic's coefficients (`s_bezBasisMatrix`): t³, t², t, 1.
    fn coefficients(&self, axis: usize) -> [f32; 4] {
        let p = self.0.map(|c| c[axis]);
        [-p[0] + 3.0 * p[1] - 3.0 * p[2] + p[3], 3.0 * p[0] - 6.0 * p[1] + 3.0 * p[2], -3.0 * p[0] + 3.0 * p[1], p[0]]
    }
    fn at(&self, t: f32) -> [f32; 3] {
        let tv = [t * t * t, t * t, t, 1.0];
        let mut out = [0.0; 3];
        for (axis, o) in out.iter_mut().enumerate() {
            // The original multiplies t by the basis and dots with the points.
            let p = self.0.map(|c| c[axis]);
            let r = [
                -tv[0] + 3.0 * tv[1] - 3.0 * tv[2] + tv[3],
                3.0 * tv[0] - 6.0 * tv[1] + 3.0 * tv[2],
                -3.0 * tv[0] + 3.0 * tv[1],
                tv[0],
            ];
            *o = p[0] * r[0] + p[1] * r[1] + p[2] * r[2] + p[3] * r[3];
        }
        out
    }
    /// `splitSegmentAtT`.
    fn split(&self, t: f32) -> (Self, Self) {
        let c = self.0;
        let lerp =
            |a: [f32; 3], b: [f32; 3]| [a[0] + (b[0] - a[0]) * t, a[1] + (b[1] - a[1]) * t, a[2] + (b[2] - a[2]) * t];
        let (p01, p12, p23) = (lerp(c[0], c[1]), lerp(c[1], c[2]), lerp(c[2], c[3]));
        let (left, right) = (lerp(p01, p12), lerp(p12, p23));
        let mid = self.at(t);
        (Self([c[0], p01, left, mid]), Self([mid, right, p23, c[3]]))
    }
    /// `getApproximateLength(USUAL_TOLERANCE)`.
    fn length(&self) -> f32 {
        let c = self.0;
        let l0 = Self::len(Self::sub(c[3], c[0]));
        let l1 = Self::len(Self::sub(c[1], c[0])) + Self::len(Self::sub(c[2], c[1])) + Self::len(Self::sub(c[3], c[2]));
        if l1 - l0 > 1.0 {
            let (a, b) = self.split(0.5);
            return a.length() + b.length();
        }
        (l0 + l1) / 2.0
    }
    /// `getSegmentPoints` (`BezFwdIterator`).
    fn points(&self, n: i32) -> Vec<[f32; 3]> {
        let n = n.max(0) as usize;
        let mut out = Vec::with_capacity(n);
        let mut cur = self.0[0];
        let (mut dq, mut ddq, mut dddq) = ([0.0f32; 3], [0.0f32; 3], [0.0f32; 3]);
        if n > 1 {
            let d = 1.0 / (n as f32 - 1.0);
            let (d2, d3) = (d * d, d * d * d);
            for axis in 0..3 {
                let [a, b, c, _] = self.coefficients(axis);
                dq[axis] = a * d3 + b * d2 + c * d;
                ddq[axis] = 6.0 * a * d3 + 2.0 * b * d2;
                dddq[axis] = 6.0 * a * d3;
            }
        }
        for _ in 0..n {
            out.push(cur);
            for axis in 0..3 {
                cur[axis] += dq[axis];
                dq[axis] += ddq[axis];
                ddq[axis] += dddq[axis];
            }
        }
        out
    }
}

impl Simulation {
    /// `PartitionManager::estimateTerrainExtremesAlongLine` (highest terrain):
    /// the partition cells along the line, each the highest of its ground
    /// samples a grid step apart. `None` off the map.
    fn highest_terrain_along(&self, a: [f32; 3], b: [f32; 3]) -> Option<f32> {
        let size = self.templates.partition_cell_size.max(1.0);
        let (wx, wy) =
            (self.grid.width as f32 * crate::navigation::CELL, self.grid.height as f32 * crate::navigation::CELL);
        let (cx, cy) = ((wx / size).ceil() as i32, (wy / size).ceil() as i32);
        let cell = |p: [f32; 3]| ((p[0] / size).floor() as i32, (p[1] / size).floor() as i32);
        let (s, e) = (cell(a), cell(b));
        let mut best: Option<f32> = None;
        let steps = (size / 10.0).ceil();
        let step = size / steps;
        let mut visit = |x: i32, y: i32| {
            if x < 0 || y < 0 || x >= cx || y >= cy {
                return;
            }
            let (bx, by) = (x as f32 * size, y as f32 * size);
            let mut hi = f32::MIN;
            let mut yy = 0.0;
            while yy <= size {
                let mut xx = 0.0;
                while xx <= size {
                    hi = hi.max(self.map.ground_height(bx + xx, by + yy));
                    xx += step;
                }
                yy += step;
            }
            best = Some(best.map_or(hi, |b: f32| b.max(hi)));
        };
        crate::pathfind::bresenham(s, e, &mut |x, y| {
            visit(x, y);
            true
        });
        best
    }

    /// `calcFlightPath`.
    fn calc_flight_path(&self, d: &DumbProjectileData, f: &mut Flight, recalc_segments: bool) -> bool {
        let (p0, p3) = (f.start, f.end);
        let Some(mut highest) = self.highest_terrain_along(p0, p3) else { return false };
        let v = Bezier::sub(p3, p0);
        let dist = Bezier::len(v);
        let n = if dist > 0.0 { [v[0] / dist, v[1] / dist, v[2] / dist] } else { [0.0; 3] };
        highest = highest.max(p0[2]).max(p3[2]);
        let p1 = [p0[0] + n[0] * dist * d.first_indent, p0[1] + n[1] * dist * d.first_indent, highest + d.first_height];
        let p2 =
            [p0[0] + n[0] * dist * d.second_indent, p0[1] + n[1] * dist * d.second_indent, highest + d.second_height];
        let curve = Bezier([p0, p1, p2, p3]);
        if recalc_segments {
            f.segments = (curve.length() / f.speed).ceil() as i32;
        }
        f.path = curve.points(f.segments);
        true
    }

    /// The fire branch for a weapon with a projectile object. Returns false when
    /// the projectile has no dumb-projectile behaviour (the caller falls back).
    pub(crate) fn launch_projectile(
        &mut self,
        launcher: u32,
        wt: &WeaponTemplate,
        victim: Option<u32>,
        dest: [f32; 3],
    ) -> bool {
        let Some(name) = &wt.projectile else { return false };
        let Some(t) = self.templates.objects.get(name) else { return false };
        let Some(m) = t.modules.iter().find(|m| m.class == "DumbProjectileBehavior") else { return false };
        let d = DumbProjectileData::parse(&m.block);
        let lo = &self.objects[&launcher];
        let (owner, start, layer) = (lo.owner, lo.position, lo.layer);
        let Ok(id) = self.create_object(name, start, 0.0, layer, owner) else { return false };
        // projectileLaunchAtObjectOrPosition, then projectileFireAtObjectOrPosition.
        let mut f = Flight {
            launcher,
            victim: victim.unwrap_or(0),
            weapon: wt.name.clone(),
            lifespan_frame: self.frame + d.max_lifespan,
            ..Default::default()
        };
        let target = victim.map_or(dest, |v| {
            let vo = &self.objects[&v];
            let dz = Self::bounding(&self.templates.objects[&vo.template]).2;
            [vo.position[0], vo.position[1], vo.position[2] + dz]
        });
        f.speed = if wt.scale_speed {
            let range = ih_math::sqrt((target[0] - start[0]).powi(2) + (target[1] - start[1]).powi(2));
            let ratio = (range - wt.minimum_range()) / (wt.attack_range - wt.minimum_range());
            ratio * (wt.speed - wt.min_speed) + wt.min_speed
        } else {
            wt.speed
        };
        if d.tumble {
            // setPitchRate, setYawRate, setRollRate (rotation is not modelled yet).
            for _ in 0..3 {
                let _ = self.rng.real(-1.0 / std::f32::consts::PI, 1.0 / std::f32::consts::PI);
            }
        }
        f.start = start;
        f.end = target;
        if !self.calc_flight_path(&d, &mut f, true) {
            self.destroy_object(id);
            return true;
        }
        f.step = 0;
        let o = self.objects.get_mut(&id).unwrap();
        o.flight = Some(f);
        true
    }

    /// `DumbProjectileBehavior::update`.
    pub(crate) fn dumb_projectile_update(&mut self, id: u32, index: usize) -> u32 {
        let o = &self.objects[&id];
        let d = DumbProjectileData::parse(&self.templates.objects[&o.template].modules_block(&o.modules[index].tag));
        let Some(mut f) = o.flight.clone() else {
            // Never launched: the empty path detonates it at once.
            self.detonate(id, &d);
            return 1;
        };
        if f.lifespan_frame != 0 && self.frame >= f.lifespan_frame {
            self.detonate(id, &d);
            return 1;
        }
        if f.step >= f.path.len() {
            self.detonate(id, &d);
            return 1;
        }
        if f.victim != 0
            && d.adjust_per_frame > 0.0
            && let Some(v) = self.objects.get(&f.victim)
        {
            let dz = Self::bounding(&self.templates.objects[&v.template]).2;
            let now = [v.position[0], v.position[1], v.position[2] + dz];
            let delta = Bezier::sub(now, f.end);
            let moved2 = delta[0] * delta[0] + delta[1] * delta[1] + delta[2] * delta[2];
            if moved2 > 0.1 {
                let moved = ih_math::sqrt(moved2);
                let step = moved.min(d.adjust_per_frame);
                for (e, dl) in f.end.iter_mut().zip(delta) {
                    *e += step * dl / moved;
                }
                if !self.calc_flight_path(&d, &mut f, false) {
                    self.objects.get_mut(&id).unwrap().flight = Some(f);
                    self.detonate(id, &d);
                    return 1;
                }
            }
        }
        let p = f.path[f.step];
        let o = self.objects.get_mut(&id).unwrap();
        if d.orient && !d.tumble {
            let next = if f.step > 0 { (f.path[f.step - 1], p) } else { (f.path[0], *f.path.get(1).unwrap_or(&p)) };
            let dir = Bezier::sub(next.1, next.0);
            if dir[0] != 0.0 || dir[1] != 0.0 {
                o.angle = ih_math::atan2(dir[1], dir[0]);
            }
        }
        o.position = p;
        f.step += 1;
        o.flight = Some(f);
        1
    }

    /// `DumbProjectileBehavior::detonate`.
    fn detonate(&mut self, id: u32, d: &DumbProjectileData) {
        let Some(o) = self.objects.get(&id) else { return };
        if o.flight.as_ref().is_some_and(|f| f.detonated) {
            return;
        }
        let weapon = o.flight.as_ref().and_then(|f| self.templates.weapons.get(&f.weapon).cloned());
        let pos = o.position;
        let kill = |s: &mut Self| {
            let max = s.objects.get(&id).and_then(|o| o.health.as_ref()).map_or(0.0, |h| h.max);
            let death = crate::body::death_type("DETONATED").unwrap_or(10);
            s.attempt_damage(id, DamageInfo::new(0, DAMAGE_UNRESISTABLE, death, max));
        };
        if let Some(wt) = weapon {
            self.projectile_detonation(id, &wt, pos);
            if d.detonate_calls_kill {
                kill(self);
            } else {
                self.destroy_object(id);
            }
        } else {
            kill(self);
        }
        if let Some(f) = self.objects.get_mut(&id).and_then(|o| o.flight.as_mut()) {
            f.detonated = true;
        }
    }

    /// `WeaponStore::handleProjectileDetonation`: a fresh weapon with a full clip
    /// fires once as a detonation from the projectile at `pos`.
    pub(crate) fn projectile_detonation(&mut self, source: u32, wt: &WeaponTemplate, pos: [f32; 3]) {
        self.fire_weapon_template(source, wt, None, pos, true);
        // The temporary weapon's bookkeeping after the shot: with ammo left it
        // draws its delay between shots.
        let ammo_left = wt.clip_size != 1;
        if ammo_left && wt.min_delay != wt.max_delay {
            let _ = self.rng.integer(wt.min_delay as i32, wt.max_delay as i32);
        }
    }

    /// `PhysicsBehavior::onCollide` handing a projectile's collisions to
    /// `projectileHandleCollision`: flying dumb projectiles that touch
    /// something their weapon collides with detonate.
    pub(crate) fn projectile_collisions(&mut self) {
        let flying: Vec<u32> = self
            .objects
            .values()
            .filter(|o| {
                (o.flight.as_ref().is_some_and(|f| !f.detonated) || o.missile.as_ref().is_some_and(|m| m.armed))
                    && o.status & crate::body::status::NO_COLLISIONS == 0
            })
            .filter(|o| o.modules.iter().any(|m| m.kind == crate::simulation::ModuleKind::Physics))
            .map(|o| o.id)
            .collect();
        for p in flying {
            let Some(po) = self.objects.get(&p) else { continue };
            let pt = &self.templates.objects[&po.template];
            let (_, pr, pdz) = Self::bounding(pt);
            let pc = [po.position[0], po.position[1], po.position[2] + pdz];
            let others: Vec<u32> = self
                .objects
                .values()
                .filter(|o| o.id != p && o.status & crate::body::status::NO_COLLISIONS == 0)
                .filter(|o| {
                    let (_, r, dz) = Self::bounding(&self.templates.objects[&o.template]);
                    let d = [o.position[0] - pc[0], o.position[1] - pc[1], o.position[2] + dz - pc[2]];
                    d[0] * d[0] + d[1] * d[1] + d[2] * d[2] < (r + pr) * (r + pr)
                })
                .map(|o| o.id)
                .collect();
            for other in others {
                let hit = if self.objects.get(&p).is_some_and(|o| o.missile.is_some()) {
                    self.missile_hit(p, Some(other))
                } else {
                    self.projectile_hit(p, other)
                };
                if hit {
                    break;
                }
            }
        }
    }

    /// `projectileHandleCollision`: true once it detonated.
    fn projectile_hit(&mut self, p: u32, other: u32) -> bool {
        let Some(f) = self.objects.get(&p).and_then(|o| o.flight.clone()) else { return false };
        let Some(wt) = self.templates.weapons.get(&f.weapon).cloned() else { return false };
        if !self.should_collide(&wt, f.launcher, p, other, f.victim) {
            return false;
        }
        let o = &self.objects[&p];
        let tag = o.modules.iter().find(|m| m.class == "DumbProjectileBehavior").map(|m| m.tag.clone());
        let Some(tag) = tag else { return false };
        let d = DumbProjectileData::parse(&self.templates.objects[&o.template].modules_block(&tag));
        self.detonate(p, &d);
        if let Some(o) = self.objects.get_mut(&p) {
            o.status |= crate::body::status::NO_COLLISIONS;
        }
        true
    }

    /// `WeaponTemplate::shouldProjectileCollideWith`.
    pub(crate) fn should_collide(
        &self,
        wt: &WeaponTemplate,
        launcher: u32,
        projectile: u32,
        other: u32,
        victim: u32,
    ) -> bool {
        if victim == other {
            return true;
        }
        if launcher == other {
            return false;
        }
        let ot = &self.templates.objects[&self.objects[&other].template];
        let mut required = match self.relationship(projectile, other) {
            weapons::Relationship::Allies => weapons::COLLIDE_ALLIES,
            weapons::Relationship::Enemies => weapons::COLLIDE_ENEMIES,
            weapons::Relationship::Neutral => 0,
        };
        if ot.has_kind("STRUCTURE") {
            required |= if self.objects[&other].owner == self.objects[&projectile].owner {
                weapons::COLLIDE_CONTROLLED_STRUCTURES
            } else {
                weapons::COLLIDE_STRUCTURES
            };
        }
        if ot.has_kind("SHRUBBERY") {
            required |= weapons::COLLIDE_SHRUBBERY;
        }
        if ot.has_kind("PROJECTILE") {
            required |= weapons::COLLIDE_PROJECTILE;
        }
        if ot.number("FenceWidth", 0.0) > 0.0 {
            required |= weapons::COLLIDE_WALLS;
        }
        if ot.has_kind("SMALL_MISSILE") {
            required |= weapons::COLLIDE_SMALL_MISSILES;
        }
        if ot.has_kind("BALLISTIC_MISSILE") {
            required |= weapons::COLLIDE_BALLISTIC_MISSILES;
        }
        wt.collides & required != 0
    }
}
