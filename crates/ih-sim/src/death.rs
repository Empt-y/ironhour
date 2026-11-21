//! Slow deaths, from `SlowDeathBehavior.cpp` and `AIDeadState`
//! (`AIUpdateInterface::markAsDead`): a dying object rolls among its
//! applicable slow-death modules by probability (plus an overkill bonus), the
//! chosen one sinks the body after a delay, runs its midpoint and final phases
//! and destroys the object. Units enter the dead AI state: they stop, and
//! infantry slide to a halt.
//!
//! The phases' FX, object creation lists and weapons wait for those systems;
//! their random choices are drawn as the original draws them. The game LOD's
//! slow-death scale is always 1 (`getSlowDeathScale` at full detail), hulk
//! lifetime overrides and tree snags are not ported, and a flung body is given
//! its force but no bounce model. Subclasses (helicopter, jet, battle bus,
//! neutron missile, ...) remain stubs.
use crate::{
    Simulation,
    body::{DamageInfo, DieKind, DieMux},
    scheduler::FOREVER,
};

/// `BEGIN_MIDPOINT_RATIO`, `END_MIDPOINT_RATIO`.
const BEGIN_MIDPOINT_RATIO: f32 = 0.35;
const END_MIDPOINT_RATIO: f32 = 0.65;
pub const PHASES: usize = 3;

/// `SlowDeathBehaviorModuleData`.
#[derive(Clone, Debug, PartialEq)]
pub struct SlowDeathData {
    pub mux: DieMux,
    /// Per frame (`parseVelocityReal`).
    pub sink_rate: f32,
    pub probability: i32,
    pub overkill_bonus: f32,
    pub sink_delay: u32,
    pub sink_delay_variance: u32,
    pub destruction_delay: u32,
    pub destruction_delay_variance: u32,
    pub destruction_altitude: f32,
    /// List lengths per phase: FX, OCL, weapons (entries may name nothing).
    pub fx: [usize; PHASES],
    pub ocl: [usize; PHASES],
    pub weapons: [usize; PHASES],
    /// `m_maskOfLoadedEffects` is nonzero: some entry names a real effect.
    pub any_effect: bool,
    pub fling_force: f32,
    pub fling_force_variance: f32,
    pub fling_pitch: f32,
    pub fling_pitch_variance: f32,
}

/// `INI::parseDurationUnsignedInt`.
pub fn duration(ms: f32) -> u32 {
    (ms * (30.0f32 / 1000.0)).ceil() as u32
}

impl SlowDeathData {
    pub fn parse(block: &ih_ini::Block, known: &dyn Fn(&str, &str) -> bool) -> Self {
        let mut d = Self {
            mux: DieMux::parse(block),
            sink_rate: 0.0,
            probability: 10,
            overkill_bonus: 0.0,
            sink_delay: 0,
            sink_delay_variance: 0,
            destruction_delay: 0,
            destruction_delay_variance: 0,
            destruction_altitude: -10.0,
            fx: [0; PHASES],
            ocl: [0; PHASES],
            weapons: [0; PHASES],
            any_effect: false,
            fling_force: 0.0,
            fling_force_variance: 0.0,
            fling_pitch: 0.0,
            fling_pitch_variance: 0.0,
        };
        let real = |v: &[String]| {
            v.first().and_then(|s| ih_ini::values::scan_real(s.trim_end_matches('%').as_bytes()).ok()).unwrap_or(0.0)
        };
        for f in block.fields() {
            let v = f.values();
            match f.name.as_str() {
                "SinkRate" => d.sink_rate = real(&v) * (1.0 / 30.0),
                "ProbabilityModifier" => d.probability = real(&v) as i32,
                "ModifierBonusPerOverkillPercent" => d.overkill_bonus = real(&v) / 100.0,
                "SinkDelay" => d.sink_delay = duration(real(&v)),
                "SinkDelayVariance" => d.sink_delay_variance = duration(real(&v)),
                "DestructionDelay" => d.destruction_delay = duration(real(&v)),
                "DestructionDelayVariance" => d.destruction_delay_variance = duration(real(&v)),
                "DestructionAltitude" => d.destruction_altitude = real(&v),
                "FlingForce" => d.fling_force = real(&v),
                "FlingForceVariance" => d.fling_force_variance = real(&v),
                "FlingPitch" => d.fling_pitch = real(&v).to_radians(),
                "FlingPitchVariance" => d.fling_pitch_variance = real(&v).to_radians(),
                "FX" | "OCL" | "Weapon" => {
                    let Some(phase) = v
                        .first()
                        .and_then(|p| ["INITIAL", "MIDPOINT", "FINAL"].iter().position(|n| n.eq_ignore_ascii_case(p)))
                    else {
                        continue;
                    };
                    let kind = match f.name.as_str() {
                        "FX" => "FXList",
                        "OCL" => "ObjectCreationList",
                        _ => "Weapon",
                    };
                    let list = match f.name.as_str() {
                        "FX" => &mut d.fx,
                        "OCL" => &mut d.ocl,
                        _ => &mut d.weapons,
                    };
                    for name in &v[1..] {
                        list[phase] += 1;
                        if known(kind, name) {
                            d.any_effect = true;
                        }
                    }
                }
                _ => {}
            }
        }
        d
    }
    /// `getProbabilityModifier`.
    fn probability(&self, info: &DamageInfo, max_health: f32) -> i32 {
        let overkill = (info.dealt - info.clipped) as i32;
        let percent = overkill as f32 / max_health;
        let modifier = (percent * self.overkill_bonus) as i32;
        (self.probability + modifier).max(1)
    }
}

/// A slow-death module's state.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct SlowDeathState {
    pub sink_frame: u32,
    pub midpoint_frame: u32,
    pub destruction_frame: u32,
    pub activated: bool,
    pub midpoint_done: bool,
    pub flung: bool,
    pub bounced: bool,
}

impl Simulation {
    fn slow_death_data(&self, id: u32, index: usize) -> Option<SlowDeathData> {
        match &self.objects[&id].modules[index].die {
            Some(DieKind::SlowDeath(d)) => Some((**d).clone()),
            _ => None,
        }
    }

    /// `SlowDeathBehavior::onDie` for the module at `index`.
    pub(crate) fn slow_death_on_die(&mut self, id: u32, index: usize, info: DamageInfo) {
        let Some(d) = self.slow_death_data(id, index) else { return };
        let o = &self.objects[&id];
        if !d.mux.applies(info.death_type, o.status) {
            return;
        }
        if self.has_ai(id) {
            if o.ai.dead {
                return;
            }
            // markAsDead
            let o = self.objects.get_mut(&id).unwrap();
            o.ai.dead = true;
            o.dead = true;
            self.wake_ai(id);
        }
        let o = &self.objects[&id];
        let max = o.health.as_ref().map_or(1.0, |h| h.max);
        let status = o.status;
        let candidates: Vec<(usize, i32)> = o
            .modules
            .iter()
            .enumerate()
            .filter_map(|(i, m)| match &m.die {
                Some(DieKind::SlowDeath(s)) if s.mux.applies(info.death_type, status) => {
                    Some((i, s.probability(&info, max)))
                }
                _ => None,
            })
            .collect();
        let total: i32 = candidates.iter().map(|c| c.1).sum();
        let mut roll = self.rng.integer(1, total);
        for (i, p) in candidates {
            roll -= p;
            if roll <= 0 {
                self.begin_slow_death(id, i);
                return;
            }
        }
    }

    /// `beginSlowDeath`.
    fn begin_slow_death(&mut self, id: u32, index: usize) {
        let Some(d) = self.slow_death_data(id, index) else { return };
        if self.objects[&id].modules[index].death.activated {
            return;
        }
        let mut s = SlowDeathState {
            sink_frame: d.sink_delay + self.rng.integer(0, d.sink_delay_variance as i32) as u32,
            destruction_frame: d.destruction_delay + self.rng.integer(0, d.destruction_delay_variance as i32) as u32,
            ..Default::default()
        };
        s.midpoint_frame = self.rng.integer(
            (BEGIN_MIDPOINT_RATIO * s.destruction_frame as f32) as i32,
            (END_MIDPOINT_RATIO * s.destruction_frame as f32) as i32,
        ) as u32;
        let now = self.frame;

        let wake = if d.fling_force > 0.0 {
            // calcRandomForce, then the body is thrown (no bounce model yet).
            let angle = self.rng.real(-std::f32::consts::PI, std::f32::consts::PI);
            let pitch = self.rng.real(d.fling_pitch, d.fling_pitch + d.fling_pitch_variance);
            let mag = self.rng.real(d.fling_force, d.fling_force + d.fling_force_variance);
            let (cp, sp) = (ih_math::cos(pitch), ih_math::sin(pitch));
            let (ca, sa) = (ih_math::cos(angle), ih_math::sin(angle));
            let force = [mag * cp * ca, mag * cp * sa, mag * sp];
            let ground = self.grid.height_at(
                &self.map,
                crate::terrain::Waypoint {
                    position: [self.objects[&id].position[0], self.objects[&id].position[1]],
                    layer: self.objects[&id].layer,
                },
            );
            let o = self.objects.get_mut(&id).unwrap();
            if o.position[2] - ground < 1.0 {
                o.position[2] += 1.0;
            }
            let inv = 1.0 / o.physics.data.mass;
            for (a, f) in o.physics.accel.iter_mut().zip(force) {
                *a += f * inv;
            }
            o.physics.woken = true;
            o.angle = ih_math::atan2(force[1], force[0]);
            s.flung = true;
            1
        } else {
            s.sink_frame.min(s.destruction_frame).min(s.midpoint_frame)
        };
        s.sink_frame += now;
        s.destruction_frame += now;
        s.midpoint_frame += now;
        s.activated = true;
        self.objects.get_mut(&id).unwrap().modules[index].death = s;
        self.scheduler.awaken((id, index), now, wake);
        self.slow_death_phase(&d, 0);
    }

    /// `doPhaseStuff`: one random FX, OCL and weapon of the phase (not run yet).
    fn slow_death_phase(&mut self, d: &SlowDeathData, phase: usize) {
        if !d.any_effect {
            return;
        }
        for n in [d.fx[phase], d.ocl[phase], d.weapons[phase]] {
            if n > 0 {
                let _ = self.rng.integer(0, n as i32 - 1);
            }
        }
    }

    /// `SlowDeathBehavior::update`. Returns the sleep.
    pub(crate) fn slow_death_update(&mut self, id: u32, index: usize) -> u32 {
        let Some(d) = self.slow_death_data(id, index) else { return FOREVER };
        let mut s = self.objects[&id].modules[index].death;
        if !s.activated {
            return FOREVER;
        }
        let now = self.frame;
        if s.flung && !s.bounced {
            s.sink_frame += 1;
            s.midpoint_frame += 1;
            s.destruction_frame += 1;
            let o = &self.objects[&id];
            let ground = self.grid.height_at(
                &self.map,
                crate::terrain::Waypoint { position: [o.position[0], o.position[1]], layer: o.layer },
            );
            if o.position[2] <= ground {
                s.bounced = true;
            }
        }
        if now >= s.sink_frame && d.sink_rate > 0.0 {
            let o = self.objects.get_mut(&id).unwrap();
            o.disabled.set(crate::disabled::DisabledReason::Held, now, None);
            o.position[2] -= d.sink_rate;
        }
        if now >= s.midpoint_frame && !s.midpoint_done {
            self.slow_death_phase(&d, 1);
            s.midpoint_done = true;
        }
        self.objects.get_mut(&id).unwrap().modules[index].death = s;
        if now >= s.destruction_frame {
            self.slow_death_phase(&d, 2);
            self.destroy_object(id);
        }
        1
    }

    /// `AIDeadState`: entered once the AI is marked dead; the locomotor stops and
    /// infantry slow by a fifth each frame.
    pub(crate) fn update_dead(&mut self, id: u32) -> u32 {
        let infantry = self.templates.objects[&self.objects[&id].template].has_kind("INFANTRY");
        let frame = self.frame;
        let o = self.objects.get_mut(&id).unwrap();
        if o.ai.state != crate::ai::AiState::Dead {
            o.ai.begin(crate::ai::AiState::Dead, frame);
        }
        o.target = None;
        o.path.clear();
        o.dead = true;
        if infantry {
            let v = &mut o.physics.vel;
            let cur = ih_math::sqrt(v[0] * v[0] + v[1] * v[1]);
            let desired = cur * 0.8;
            if desired < 0.001 {
                v[0] = 0.0;
                v[1] = 0.0;
            } else if desired < cur {
                let k = desired / cur;
                v[0] *= k;
                v[1] *= k;
            }
        }
        1
    }
}
