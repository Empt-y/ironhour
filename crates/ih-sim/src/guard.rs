//! Guarding, from `AIGuard.cpp` (`AIGuardMachine` and its inner, outer,
//! return, idle, crate and attack-aggressor states, `lookForInnerTarget`,
//! `hasAttackedMeAndICanReturnFire`, `ExitConditions`) and `AIGuardState`
//! (`AIStates.cpp`).
//!
//! A guarding unit returns to its post, then scans for enemies within its inner
//! guard range (`GuardInnerModifierHuman` of its vision) every
//! `GuardEnemyScanRate`; it attacks one until it dies or leaves that range,
//! pursues it within the outer range for `GuardChaseUnitsDuration`, then goes
//! back. Whatever shoots it is attacked back (and followed) while the guard
//! idles, returns or attacks within the inner range.
//!
//! Only guarding a position is ported (objects and areas, flying-only and
//! no-pursuit modes, entering and hijacking guards and crate pick-up wait for
//! containers, polygon areas and crates).
use crate::{
    Simulation,
    ai::{AiState, Outcome},
    attack::{ExitConditions, Ret},
    weapons::Relationship,
};

/// `AIGuardMachine` states.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
#[repr(u8)]
pub enum GuardSub {
    #[default]
    Inner = 0,
    Return = 1,
    Idle = 2,
    Outer = 3,
    GetCrate = 4,
    AttackAggressor = 5,
}

/// `AIGuardMachine`.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct GuardMachine {
    pub position: [f32; 3],
    pub nemesis: u32,
    pub sub: GuardSub,
    /// The idle and return states' scan timers.
    pub next_scan: u32,
}

impl Simulation {
    /// `aiGuardPosition` (`AIGuardState::onEnter`).
    pub(crate) fn begin_guard(&mut self, id: u32, position: [f32; 2]) -> Result<(), crate::SimError> {
        let o = self.objects.get(&id).ok_or_else(|| crate::SimError(format!("object {id} is missing")))?;
        if o.dead {
            return Err(crate::SimError(format!("object {id} is dead")));
        }
        let z = self.grid.height_at(&self.map, crate::terrain::Waypoint::ground(position));
        let frame = self.frame;
        let o = self.objects.get_mut(&id).unwrap();
        o.ai.begin(AiState::Guard, frame);
        o.ai.cmd_from_ai = false;
        o.target = None;
        o.path.clear();
        o.ai.guard = Some(GuardMachine { position: [position[0], position[1], z], ..Default::default() });
        self.wake_ai(id);
        // initDefaultState (the inner state, which falls through to the return),
        // then setState(AI_GUARD_RETURN).
        let mut ret = self.guard_enter(id, GuardSub::Inner, 0);
        if ret != Ret::Failure {
            self.guard_exit(id);
            ret = self.guard_enter(id, GuardSub::Return, 0);
        }
        if ret == Ret::Failure {
            self.objects.get_mut(&id).unwrap().ai.guard = None;
            self.end_order(id, Outcome::Failure);
        }
        Ok(())
    }

    fn guard(&self, id: u32) -> GuardMachine {
        self.objects[&id].ai.guard.clone().unwrap_or_default()
    }
    fn set_guard(&mut self, id: u32, g: GuardMachine) {
        if let Some(o) = self.objects.get_mut(&id) {
            o.ai.guard = Some(g);
        }
    }

    /// `getStdGuardRange`: the inner guard range.
    fn std_guard_range(&self, id: u32) -> f32 {
        self.adjusted_vision(id, true)
    }

    /// `lookForInnerTarget`: the closest attackable enemy to the post within the
    /// inner range (centre distance).
    fn look_for_inner_target(&mut self, id: u32) -> bool {
        if !self.able_to_attack(id) {
            return false;
        }
        let g = self.guard(id);
        let range = self.std_guard_range(id);
        let candidates: Vec<u32> = self.objects.keys().copied().filter(|o| *o != id).collect();
        let mut best: Option<(f32, u32)> = None;
        for other in candidates {
            if self.relationship(id, other) != Relationship::Enemies {
                continue;
            }
            let p = self.objects[&other].position;
            let d2 = (p[0] - g.position[0]).powi(2) + (p[1] - g.position[1]).powi(2);
            if d2 > range * range {
                continue;
            }
            if self.can_attack(id, other, false).is_none() {
                continue;
            }
            if best.is_none_or(|(bd, _)| d2 < bd) {
                best = Some((d2, other));
            }
        }
        match best {
            Some((_, n)) => {
                let mut g = self.guard(id);
                g.nemesis = n;
                self.set_guard(id, g);
                true
            }
            None => false,
        }
    }

    /// `hasAttackedMeAndICanReturnFire` (it clears the last attacker).
    fn attacked_and_can_return_fire(&mut self, id: u32) -> bool {
        let Some(h) = self.objects.get_mut(&id).and_then(|o| o.health.as_mut()) else { return false };
        let Some(attacker) = h.last_damage.filter(|_| !h.last_damage_cleared).map(|d| d.source) else {
            return false;
        };
        h.last_damage_cleared = true;
        if attacker == 0 || !self.objects.get(&attacker).is_some_and(|o| !o.dead) {
            return false;
        }
        if self.relationship(id, attacker) != Relationship::Enemies || !self.able_to_attack(id) {
            return false;
        }
        self.can_attack(id, attacker, false).is_some()
    }

    /// Enter a guard state, following immediate results through the table.
    fn guard_enter(&mut self, id: u32, sub: GuardSub, depth: u32) -> Ret {
        let mut g = self.guard(id);
        g.sub = sub;
        self.set_guard(id, g.clone());
        let now = self.frame;
        let ret = match sub {
            GuardSub::Inner => {
                if !self.objects.contains_key(&g.nemesis) {
                    Ret::Success
                } else {
                    let r = self.std_guard_range(id);
                    let exit = ExitConditions {
                        center: g.position,
                        radius_sqr: r * r,
                        if_outside_radius: true,
                        if_no_unit: true,
                        ..Default::default()
                    };
                    let pos = self.objects[&g.nemesis].position;
                    match self.attack_enter(id, Some(g.nemesis), pos, false, Some(exit)) {
                        Ret::Continue => Ret::Continue,
                        _ => Ret::Success,
                    }
                }
            }
            GuardSub::Outer => {
                if !self.objects.contains_key(&g.nemesis) {
                    Ret::Success
                } else {
                    let r = self.adjusted_vision(id, false);
                    let exit = ExitConditions {
                        center: g.position,
                        radius_sqr: r * r,
                        give_up_frame: now + self.templates.ai.guard_chase_frames,
                        if_expired: true,
                        if_outside_radius: true,
                        if_no_unit: true,
                    };
                    let pos = self.objects[&g.nemesis].position;
                    match self.attack_enter(id, Some(g.nemesis), pos, false, Some(exit)) {
                        Ret::Continue => Ret::Continue,
                        _ => Ret::Success,
                    }
                }
            }
            GuardSub::AttackAggressor => {
                let mut g = self.guard(id);
                if let Some(src) = self.objects[&id].health.as_ref().and_then(|h| h.last_damage).map(|d| d.source)
                    && src != 0
                {
                    g.nemesis = src;
                    self.set_guard(id, g.clone());
                }
                if !self.objects.contains_key(&g.nemesis) {
                    Ret::Success
                } else {
                    let r = self.std_guard_range(id);
                    let exit = ExitConditions {
                        center: g.position,
                        radius_sqr: r * r,
                        give_up_frame: now + self.templates.ai.guard_chase_frames,
                        if_expired: true,
                        if_outside_radius: true,
                        if_no_unit: true,
                    };
                    let pos = self.objects[&g.nemesis].position;
                    match self.attack_enter(id, Some(g.nemesis), pos, true, Some(exit)) {
                        Ret::Continue => Ret::Continue,
                        _ => Ret::Success,
                    }
                }
            }
            // No crates yet.
            GuardSub::GetCrate => Ret::Success,
            GuardSub::Return => {
                let mut g = self.guard(id);
                g.next_scan = now + self.rng.integer(0, self.templates.ai.guard_return_scan as i32) as u32;
                self.set_guard(id, g.clone());
                // adjustDestination, then AIInternalMoveToState::onEnter.
                let goal = self.choose_goal(id, [g.position[0], g.position[1]], AiState::MoveTo, true);
                match self.route_to_waypoint(id, goal, true) {
                    Ok((path, to)) => {
                        let o = self.objects.get_mut(&id).unwrap();
                        o.path = path;
                        o.path.push_front(crate::terrain::Waypoint {
                            position: [o.position[0], o.position[1]],
                            layer: o.layer,
                        });
                        o.target = Some(to);
                        o.last_repath = now;
                        self.move_allies(id);
                        Ret::Continue
                    }
                    Err(_) => Ret::Failure,
                }
            }
            GuardSub::Idle => {
                let mut g = self.guard(id);
                g.next_scan = now + self.rng.integer(0, self.templates.ai.guard_scan as i32) as u32;
                self.set_guard(id, g);
                Ret::Continue
            }
        };
        self.guard_check(id, sub, ret, depth + 1)
    }

    /// `friend_checkForTransitions` for the guard machine.
    fn guard_check(&mut self, id: u32, sub: GuardSub, ret: Ret, depth: u32) -> Ret {
        // The original gives up after twenty nested transitions.
        if depth >= 20 {
            return Ret::Failure;
        }
        if ret == Ret::Continue {
            if matches!(sub, GuardSub::Inner | GuardSub::Return | GuardSub::Idle)
                && self.attacked_and_can_return_fire(id)
            {
                self.guard_exit(id);
                return self.guard_enter(id, GuardSub::AttackAggressor, depth);
            }
            return Ret::Continue;
        }
        self.guard_exit(id);
        let next = match (sub, ret) {
            (GuardSub::Inner, _) => GuardSub::Outer,
            (GuardSub::Return, Ret::Success) => GuardSub::Idle,
            (GuardSub::Return, _) => GuardSub::Inner,
            (GuardSub::Idle, Ret::Success) => GuardSub::Inner,
            (GuardSub::Idle, _) => GuardSub::Return,
            (GuardSub::Outer, _) => GuardSub::GetCrate,
            (GuardSub::GetCrate, _) => GuardSub::Return,
            (GuardSub::AttackAggressor, _) => GuardSub::Inner,
        };
        self.guard_enter(id, next, depth)
    }

    fn guard_exit(&mut self, id: u32) {
        let Some(o) = self.objects.get_mut(&id) else { return };
        o.ai.attack = None;
        o.target = None;
        o.path.clear();
    }

    /// `AIGuardState::update`. Returns the sleep.
    pub(crate) fn update_guard(&mut self, id: u32) -> u32 {
        if self.objects[&id].ai.guard.is_none() {
            self.end_order(id, Outcome::Failure);
            return crate::scheduler::FOREVER;
        }
        let g = self.guard(id);
        let now = self.frame;
        let (ret, sleep) = match g.sub {
            GuardSub::Inner | GuardSub::Outer | GuardSub::AttackAggressor => {
                if g.sub == GuardSub::Outer
                    && let Some(v) = self.objects[&id].ai.attack.as_ref().and_then(|m| m.victim)
                    && let Some(vo) = self.objects.get(&v)
                {
                    // Close to the post again: the chase timer restarts.
                    let d = [
                        g.position[0] - vo.position[0],
                        g.position[1] - vo.position[1],
                        g.position[2] - vo.position[2],
                    ];
                    let r = self.std_guard_range(id);
                    if d[0] * d[0] + d[1] * d[1] + d[2] * d[2] <= r * r {
                        let chase = self.templates.ai.guard_chase_frames;
                        if let Some(e) =
                            self.objects.get_mut(&id).unwrap().ai.attack.as_mut().and_then(|m| m.exit.as_mut())
                        {
                            e.give_up_frame = now + chase;
                        }
                    }
                }
                let ret = if self.objects[&id].ai.attack.is_some() { self.attack_tick(id) } else { Ret::Success };
                (ret, 1)
            }
            GuardSub::GetCrate => (Ret::Success, 1),
            GuardSub::Return => {
                if now >= g.next_scan {
                    let mut gg = g.clone();
                    gg.next_scan = now + self.templates.ai.guard_return_scan;
                    self.set_guard(id, gg);
                    if self.look_for_inner_target(id) {
                        let r = self.guard_check(id, GuardSub::Return, Ret::Failure, 0);
                        return self.guard_result(id, r, 1);
                    }
                }
                self.repath_traffic(id);
                self.drive(id);
                if self.objects[&id].target.is_none() { (Ret::Success, 1) } else { (Ret::Continue, 1) }
            }
            GuardSub::Idle => {
                if now < g.next_scan {
                    (Ret::Continue, g.next_scan - now)
                } else {
                    let mut gg = g.clone();
                    gg.next_scan = now + self.templates.ai.guard_scan;
                    self.set_guard(id, gg.clone());
                    if self.look_for_inner_target(id) { (Ret::Success, 1) } else { (Ret::Continue, gg.next_scan - now) }
                }
            }
        };
        let r = self.guard_check(id, g.sub, ret, 0);
        self.guard_result(id, r, sleep)
    }

    fn guard_result(&mut self, id: u32, r: Ret, sleep: u32) -> u32 {
        if r == Ret::Failure {
            self.objects.get_mut(&id).unwrap().ai.guard = None;
            self.end_order(id, Outcome::Failure);
            return crate::scheduler::FOREVER;
        }
        // Anything but idling needs every frame.
        if self.guard(id).sub == GuardSub::Idle { sleep.max(1) } else { 1 }
    }
}
