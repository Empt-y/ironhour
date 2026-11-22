//! Attacking, from `AIStates.cpp`: `AIAttackState` and its `AttackStateMachine`
//! (`AIAttackAimAtTargetState`, `AIAttackFireWeaponState`,
//! `AIAttackApproachTargetState`, the `outOfWeaponRangeObject` and
//! `outOfWeaponRangePosition` transitions). An attacker aims at its victim,
//! fires when the weapon is ready, aims again while it reloads, and closes in
//! when the victim leaves its range; the attack succeeds when the victim dies
//! and fails when the attacker runs out of ammo.
//!
//! Stand-ins and gaps: the approach paths to the victim's cell with an
//! adjusted goal and stops once in range (`findAttackPath`'s search for a
//! firing cell is not ported); chasing is the approach (`AIAttackPursueTargetState`
//! is not ported); aiming turns the body at the locomotor's turn rate as the
//! face orders do; turrets, firing line of sight, mood, retaliation, continued
//! attacks, weapon locks and the "can attack" checks beyond weapon choice wait
//! for their systems.
use crate::{
    Simulation,
    ai::{AiState, Outcome},
    scheduler::FOREVER,
    terrain::Waypoint,
};

/// `AttackStateMachine` states.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
#[repr(u8)]
pub enum Sub {
    Chase = 0,
    Approach = 1,
    #[default]
    Aim = 2,
    Fire = 3,
}

/// The attack sub-machine.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct AttackMachine {
    pub victim: Option<u32>,
    pub position: [f32; 3],
    pub sub: Sub,
    pub approach_frame: u32,
    pub prev_victim_pos: [f32; 3],
    pub stop_if_in_range: bool,
    pub can_turn_in_place: bool,
    /// `m_follow`: keep after a mobile victim once in range.
    pub follow: bool,
    /// `m_attackParameters` (the guard states' exit conditions).
    pub exit: Option<ExitConditions>,
}

/// `ExitConditions` (`AIGuard.cpp`).
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct ExitConditions {
    pub center: [f32; 3],
    pub radius_sqr: f32,
    pub give_up_frame: u32,
    pub if_expired: bool,
    pub if_outside_radius: bool,
    pub if_no_unit: bool,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Ret {
    Continue,
    Success,
    Failure,
}

/// `MIN_RECOMPUTE_TIME`.
const MIN_RECOMPUTE_TIME: u32 = 10;
/// `REL_THRESH` (about two degrees).
const REL_THRESH: f32 = 0.035;

/// `isSamePosition`: the target moved less than a tenth of the distance to it.
fn same_position(ours: [f32; 3], prev: [f32; 3], cur: [f32; 3]) -> bool {
    let diff = [cur[0] - prev[0], cur[1] - prev[1]];
    let to = [cur[0] - ours[0], cur[1] - ours[1]];
    let tol = (to[0] * to[0] + to[1] * to[1]) * (1.0 / 100.0);
    diff[0] * diff[0] + diff[1] * diff[1] <= tol
}

impl Simulation {
    /// `aiAttackObject` / `aiAttackPosition` (`AIAttackState::onEnter`).
    pub(crate) fn begin_attack(
        &mut self,
        id: u32,
        victim: Option<u32>,
        position: [f32; 3],
    ) -> Result<(), crate::SimError> {
        let o = self.objects.get(&id).ok_or_else(|| crate::SimError(format!("object {id} is missing")))?;
        if o.dead {
            return Err(crate::SimError(format!("object {id} is dead")));
        }
        if o.weapon_set.weapons.iter().all(Option::is_none) {
            return Err(crate::SimError(format!("{} has no weapon", o.template)));
        }
        if let Some(v) = victim
            && !self.objects.contains_key(&v)
        {
            return Err(crate::SimError(format!("attack target {v} is missing")));
        }
        let frame = self.frame;
        let o = self.objects.get_mut(&id).unwrap();
        o.ai.begin(if victim.is_some() { AiState::AttackObject } else { AiState::AttackPosition }, frame);
        o.target = None;
        o.path.clear();
        o.ai.attack = None;
        self.wake_ai(id);
        let ret = self.attack_enter(id, victim, position, false, None);
        if ret != Ret::Continue {
            self.finish_attack(id, ret);
        }
        Ok(())
    }

    /// `ExitConditions::shouldExit` for the attack machine's victim.
    fn should_exit(&self, m: &AttackMachine) -> bool {
        let Some(e) = m.exit else { return false };
        let Some(v) = m.victim.and_then(|v| self.objects.get(&v)) else { return e.if_no_unit };
        if e.if_expired && self.frame >= e.give_up_frame {
            return true;
        }
        if e.if_outside_radius {
            let d = [v.position[0] - e.center[0], v.position[1] - e.center[1]];
            if d[0] * d[0] + d[1] * d[1] > e.radius_sqr {
                return true;
            }
        }
        false
    }

    fn finish_attack(&mut self, id: u32, ret: Ret) {
        self.objects.get_mut(&id).unwrap().ai.attack = None;
        self.end_order(id, if ret == Ret::Success { Outcome::Success } else { Outcome::Failure });
    }

    /// `AIAttackState::onEnter`.
    pub(crate) fn attack_enter(
        &mut self,
        id: u32,
        victim: Option<u32>,
        position: [f32; 3],
        follow: bool,
        exit: Option<ExitConditions>,
    ) -> Ret {
        let probe = AttackMachine { victim, exit, ..Default::default() };
        if self.should_exit(&probe) {
            return Ret::Success;
        }
        let projectile = self.templates.objects[&self.objects[&id].template].has_kind("PROJECTILE");
        if self.out_of_ammo(id) && !projectile {
            return Ret::Failure;
        }
        if let Some(v) = victim
            && self.objects.get(&v).is_none_or(|o| o.dead)
        {
            return Ret::Failure;
        }
        if !self.choose_weapon(id, victim) {
            return Ret::Failure;
        }
        let cur = self.objects[&id].weapon_set.current as usize;
        if let Some(w) = self.objects.get_mut(&id).unwrap().weapon_set.weapons[cur].as_mut() {
            w.max_shots = crate::weapons::NO_MAX_SHOTS;
        }
        let pos = victim.map_or(position, |v| self.objects[&v].position);
        self.objects.get_mut(&id).unwrap().ai.attack =
            Some(AttackMachine { victim, position: pos, follow, exit, ..Default::default() }.with_default());
        self.enter_sub(id, Sub::Aim)
    }

    /// The attack order (`AI_ATTACK_OBJECT`, `AI_ATTACK_POSITION`).
    pub(crate) fn update_attack(&mut self, id: u32) -> u32 {
        if self.objects[&id].ai.attack.is_none() {
            self.end_order(id, Outcome::Failure);
            return FOREVER;
        }
        let ret = self.attack_tick(id);
        if ret != Ret::Continue {
            self.finish_attack(id, ret);
        }
        1
    }

    /// `AIAttackState::update`.
    pub(crate) fn attack_tick(&mut self, id: u32) -> Ret {
        let Some(m) = self.objects[&id].ai.attack.clone() else { return Ret::Failure };
        if self.should_exit(&m) {
            return Ret::Success;
        }
        let projectile = self.templates.objects[&self.objects[&id].template].has_kind("PROJECTILE");

        if self.out_of_ammo(id) && !projectile {
            Ret::Failure
        } else if m.victim.is_some_and(|v| self.objects.get(&v).is_none_or(|o| o.dead)) {
            Ret::Success
        } else if !self.choose_weapon(id, m.victim) {
            Ret::Failure
        } else {
            let cur = self.objects[&id].weapon_set.current as usize;
            match &self.objects[&id].weapon_set.weapons[cur] {
                Some(w) if w.max_shots > 0 => self.update_attack_machine(id),
                _ => Ret::Failure,
            }
        }
    }

    fn machine(&self, id: u32) -> AttackMachine {
        self.objects[&id].ai.attack.clone().unwrap_or_default()
    }
    fn set_machine(&mut self, id: u32, m: AttackMachine) {
        self.objects.get_mut(&id).unwrap().ai.attack = Some(m);
    }
    fn victim_pos(&self, m: &AttackMachine) -> [f32; 3] {
        m.victim.and_then(|v| self.objects.get(&v)).map_or(m.position, |o| o.position)
    }
    fn in_range(&self, id: u32, m: &AttackMachine) -> bool {
        let cur = self.objects[&id].weapon_set.current as usize;
        // attemptBestFirePointPosition: garrisoned units fire from a fire point.
        let from = self.garrison_fire_position(id, self.victim_pos(m));
        self.within_attack_range(id, cur, from, m.victim, self.victim_pos(m))
    }

    /// `StateMachine::updateStateMachine` for the attack machine: the state's
    /// update, then (`friend_checkForTransitions`) its table on success or
    /// failure, or its conditions while it continues.
    fn update_attack_machine(&mut self, id: u32) -> Ret {
        let sub = self.machine(id).sub;
        let ret = match sub {
            Sub::Aim => self.aim_update(id),
            Sub::Fire => self.fire_update(id),
            Sub::Chase | Sub::Approach => self.approach_update(id),
        };
        self.check_transitions(id, sub, ret)
    }

    fn check_transitions(&mut self, id: u32, sub: Sub, ret: Ret) -> Ret {
        if ret != Ret::Continue {
            return self.transition(id, sub, ret);
        }
        // Aim and fire leave for the chase when the victim is out of range.
        let m = self.machine(id);
        if matches!(sub, Sub::Aim | Sub::Fire) && !self.in_range(id, &m) {
            self.exit_sub(id, sub);
            return self.enter_sub(id, Sub::Chase);
        }
        Ret::Continue
    }

    /// The machine's transition table.
    fn transition(&mut self, id: u32, from: Sub, ret: Ret) -> Ret {
        if ret == Ret::Continue {
            return Ret::Continue;
        }
        self.exit_sub(id, from);
        let immobile = self.templates.objects[&self.objects[&id].template].has_kind("IMMOBILE");
        let next = match (from, ret) {
            (Sub::Aim, Ret::Success) => Some(Sub::Fire),
            (Sub::Aim, _) => None,
            (Sub::Fire, _) => Some(Sub::Aim),
            // CHASE is a FailureState for immobile objects.
            (Sub::Chase, _) if immobile => None,
            (Sub::Chase, _) => Some(Sub::Approach),
            (Sub::Approach, Ret::Success) => Some(Sub::Aim),
            (Sub::Approach, _) => None,
        };
        match next {
            Some(s) => self.enter_sub(id, s),
            None => Ret::Failure,
        }
    }

    /// Enter a state, following immediate successes and failures (`internalSetState`).
    fn enter_sub(&mut self, id: u32, sub: Sub) -> Ret {
        let mut m = self.machine(id);
        m.sub = sub;
        self.set_machine(id, m);
        let ret = match sub {
            Sub::Aim => self.aim_enter(id),
            Sub::Fire => self.fire_enter(id),
            // AIAttackPursueTargetState stand-in: straight on to the approach.
            Sub::Chase => {
                if self.templates.objects[&self.objects[&id].template].has_kind("IMMOBILE") {
                    Ret::Failure
                } else {
                    Ret::Success
                }
            }
            Sub::Approach => self.approach_enter(id),
        };
        self.check_transitions(id, sub, ret)
    }

    fn exit_sub(&mut self, id: u32, sub: Sub) {
        match sub {
            Sub::Fire => {
                // A pre-attack that never fired is cancelled.
                let o = self.objects.get_mut(&id).unwrap();
                let cur = o.weapon_set.current as usize;
                if let Some(w) = o.weapon_set.weapons[cur].as_mut()
                    && w.status == crate::weapons::Status::PreAttack
                {
                    w.pre_attack_finished = 0;
                }
            }
            Sub::Approach | Sub::Chase => {
                let o = self.objects.get_mut(&id).unwrap();
                o.target = None;
                o.path.clear();
            }
            Sub::Aim => {}
        }
    }

    fn aim_enter(&mut self, id: u32) -> Ret {
        let m = self.machine(id);
        let o = &self.objects[&id];
        let t = &self.templates.objects[&o.template];
        let can_turn = t.locomotor().and_then(|n| self.templates.locomotors.get(n)).is_some_and(|l| l.min_speed == 0.0);
        let cur = o.weapon_set.current as usize;
        let contact = o.weapon_set.weapons[cur]
            .as_ref()
            .and_then(|w| self.templates.weapons.get(&w.template))
            .is_some_and(|w| w.is_contact());
        let in_range = self.in_range(id, &m);
        self.set_machine(id, AttackMachine { can_turn_in_place: can_turn, ..m });
        if contact && in_range { Ret::Success } else { Ret::Continue }
    }

    fn aim_update(&mut self, id: u32) -> Ret {
        let m = self.machine(id);
        if m.victim.is_some_and(|v| self.objects.get(&v).is_none_or(|o| o.dead)) {
            return Ret::Failure;
        }
        let target = self.victim_pos(&m);
        let o = &self.objects[&id];
        let rel = ih_math::angle_delta(o.angle, ih_math::atan2(target[1] - o.position[1], target[0] - o.position[0]));
        let cur = o.weapon_set.current as usize;
        let aim_delta = o.weapon_set.weapons[cur]
            .as_ref()
            .and_then(|w| self.templates.weapons.get(&w.template))
            .map_or(0.0, |w| w.aim_delta)
            .max(REL_THRESH);
        let t = &self.templates.objects[&o.template];
        let turn_rate = t.locomotor().and_then(|n| self.templates.locomotors.get(n)).map_or(0.0, |l| l.turn_rate);
        if rel.abs() < aim_delta {
            return Ret::Success;
        }
        if m.can_turn_in_place {
            let o = self.objects.get_mut(&id).unwrap();
            o.angle += rel.clamp(-turn_rate, turn_rate);
        } else {
            // setLocomotorGoalPositionExplicit: drive at the target.
            let o = self.objects.get_mut(&id).unwrap();
            let here = Waypoint { position: [o.position[0], o.position[1]], layer: o.layer };
            o.path = [here, Waypoint { position: [target[0], target[1]], layer: o.layer }].into();
            o.target = Some([target[0], target[1]]);
            self.drive(id);
        }
        Ret::Continue
    }

    fn fire_enter(&mut self, id: u32) -> Ret {
        let m = self.machine(id);
        self.pre_fire(id, m.victim);
        Ret::Continue
    }

    fn fire_update(&mut self, id: u32) -> Ret {
        let m = self.machine(id);
        if m.victim.is_some_and(|v| self.objects.get(&v).is_none_or(|o| o.dead)) {
            return Ret::Failure;
        }
        match self.current_status(id) {
            None => return Ret::Failure,
            Some(crate::weapons::Status::PreAttack) => return Ret::Continue,
            Some(crate::weapons::Status::ReadyToFire) => {}
            Some(_) => return Ret::Failure,
        }
        let pos = self.victim_pos(&m);
        self.fire_current_weapon(id, m.victim, pos);
        Ret::Success
    }

    fn approach_enter(&mut self, id: u32) -> Ret {
        let mut m = self.machine(id);
        if m.victim.is_some_and(|v| !self.objects.contains_key(&v)) {
            return Ret::Success;
        }
        if m.victim.is_some() && self.in_range(id, &m) {
            return Ret::Success;
        }
        // A human player's unit does not chase a target it picked itself
        // (`isAllowedToChase` is off) unless its weapon is a contact weapon.
        let o = &self.objects[&id];
        let cur = o.weapon_set.current as usize;
        let contact = o.weapon_set.weapons[cur]
            .as_ref()
            .and_then(|w| self.templates.weapons.get(&w.template))
            .is_some_and(|w| w.is_contact());
        if m.victim.is_some() && o.ai.cmd_from_ai && !contact {
            return Ret::Failure;
        }
        m.prev_victim_pos = [0.0; 3];
        m.approach_frame = self.frame.wrapping_sub(MIN_RECOMPUTE_TIME);
        self.set_machine(id, m);
        if !self.approach_path(id, true) {
            return Ret::Failure;
        }
        Ret::Continue
    }

    /// `computePath` (stand-in for `requestAttackPath`).
    fn approach_path(&mut self, id: u32, force: bool) -> bool {
        let mut m = self.machine(id);
        let o = &self.objects[&id];
        let force = force || (o.target.is_none() && o.path.is_empty());
        if !force && self.frame.wrapping_sub(m.approach_frame) < MIN_RECOMPUTE_TIME {
            return true;
        }
        m.approach_frame = self.frame;
        let target = self.victim_pos(&m);
        if m.victim.is_some() && !force && same_position(o.position, m.prev_victim_pos, target) {
            self.set_machine(id, m);
            return true;
        }
        if m.victim.is_none() && !force {
            self.set_machine(id, m);
            return true;
        }
        m.prev_victim_pos = target;
        m.stop_if_in_range = true;
        self.set_machine(id, m);
        let goal = self.choose_goal(id, [target[0], target[1]], AiState::MoveTo, true);
        match self.route_to_waypoint(id, goal, true) {
            Ok((path, to)) => {
                let o = self.objects.get_mut(&id).unwrap();
                o.path = path;
                o.path.push_front(Waypoint { position: [o.position[0], o.position[1]], layer: o.layer });
                o.target = Some(to);
                o.last_repath = self.frame;
                self.move_allies(id);
                true
            }
            Err(_) => false,
        }
    }

    /// `AIAttackApproachTargetState::update`: with `m_follow`, a mobile victim is
    /// followed whatever the approach returns.
    fn approach_update(&mut self, id: u32) -> Ret {
        let m = self.machine(id);
        let ret = self.approach_update_internal(id);
        if m.follow
            && let Some(v) = m.victim.and_then(|v| self.objects.get(&v))
            && !self.templates.objects[&v.template].has_kind("IMMOBILE")
            && !self.templates.objects[&self.objects[&id].template].has_kind("IMMOBILE")
        {
            return Ret::Continue;
        }
        ret
    }

    fn approach_update_internal(&mut self, id: u32) -> Ret {
        let m = self.machine(id);
        if m.victim.is_some_and(|v| !self.objects.contains_key(&v)) {
            return Ret::Failure;
        }
        if m.stop_if_in_range && self.in_range(id, &m) {
            return Ret::Success;
        }
        if !self.approach_path(id, false) {
            return Ret::Success;
        }
        self.repath_traffic(id);
        self.drive(id);
        if self.objects[&id].target.is_none() {
            // The move ended: always a success, so the attack aims again.
            return Ret::Success;
        }
        Ret::Continue
    }
}

impl AttackMachine {
    fn with_default(self) -> Self {
        Self { sub: Sub::Aim, ..self }
    }
}
