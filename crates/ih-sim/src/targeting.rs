//! Picking targets, from `AIIdleState` (`AIStates.cpp`),
//! `AIUpdateInterface::getNextMoodTarget`, `resetNextMoodCheckTime`,
//! `AI::findClosestEnemy` with its partition filters (`LiveMapEnemies`,
//! `RejectBuildings`, `WithinAttackRange`, `PossibleToAttack`,
//! `InsignificantBuildings`), `AI::getAdjustedVisionRangeForObject`,
//! `Object::isAbleToAttack` and `WeaponSet::getAbleToAttackSpecificObject`.
//!
//! An idle unit whose AI has `AutoAcquireEnemiesWhenIdle = YES` looks for the
//! closest enemy within its vision range (scaled by `GuardOuterModifierHuman`)
//! that a weapon can already reach, every `MoodAttackCheckRate` (the first check
//! randomly offset), and attacks it. Such attacks are the AI's own: the unit
//! does not chase a target that leaves its range.
//!
//! Every player counts as a human player until player types exist (so the
//! mood matrix always allows the action and targets must be in range); fog,
//! stealth, line of sight, crates, repulsors, team targets and attack priority
//! lists wait for their systems.
use crate::{
    Simulation,
    weapons::{Relationship, SLOTS},
};

/// `IDLE_COUNTDOWN_DELAY`.
pub const IDLE_COUNTDOWN_DELAY: u32 = 30 * 2;

/// `AIIdleState` and the AI's mood-check timer.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct IdleState {
    pub inited: bool,
    pub sleep_offset: u32,
    pub next_mood_check: u32,
    pub randomly_offset: bool,
}

/// `AutoAcquireEnemiesWhenIdle` bits.
pub const AAS_IDLE: u32 = 0x01;
pub const AAS_NOT_WHILE_ATTACKING: u32 = 0x08;
pub const AAS_ATTACK_BUILDINGS: u32 = 0x10;

/// `TAiData` values for targeting and guarding, with the original defaults.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct AiData {
    pub force_idle_frames: u32,
    pub guard_inner_human: f32,
    pub guard_outer_human: f32,
    pub guard_chase_frames: u32,
    pub guard_scan: u32,
    pub guard_return_scan: u32,
    pub attack_uses_los: bool,
    pub attack_ignore_insignificant: bool,
}
impl Default for AiData {
    fn default() -> Self {
        Self {
            force_idle_frames: 1,
            guard_inner_human: 0.0,
            guard_outer_human: 0.0,
            guard_chase_frames: 0,
            guard_scan: 15,
            guard_return_scan: 30,
            attack_uses_los: true,
            attack_ignore_insignificant: false,
        }
    }
}

/// The AI module's `AutoAcquireEnemiesWhenIdle` and `MoodAttackCheckRate`.
pub fn ai_module_data(t: &crate::templates::Template) -> (u32, u32) {
    let Some(m) = t.modules.iter().find(|m| {
        ih_ini::schema::MODULES
            .iter()
            .find(|d| d.name == m.class)
            .is_some_and(|d| d.tables.iter().any(|t| t.starts_with("AIUpdateModuleData::")))
    }) else {
        return (0, 60);
    };
    let (mut auto, mut rate) = (0, 60);
    let names = ["YES", "STEALTHED", "NO", "NOTWHILEATTACKING", "ATTACK_BUILDINGS"];
    for f in m.block.fields() {
        let v = f.values();
        match f.name.as_str() {
            "AutoAcquireEnemiesWhenIdle" => {
                auto = 0;
                for n in &v {
                    if let Some(i) = names.iter().position(|x| x.eq_ignore_ascii_case(n)) {
                        auto |= 1 << i;
                    }
                }
            }
            "MoodAttackCheckRate" => {
                rate = crate::death::duration(
                    v.first().and_then(|s| ih_ini::values::scan_real(s.as_bytes()).ok()).unwrap_or(0.0),
                )
            }
            _ => {}
        }
    }
    (auto, rate)
}

impl Simulation {
    /// `Object::isAbleToAttack` (no containers, turrets or construction yet).
    pub fn able_to_attack(&self, id: u32) -> bool {
        let o = &self.objects[&id];
        // Nothing under construction can fire.
        let sold = crate::construction::status_bits(&["SOLD"]);
        if o.dead
            || o.disabled.has(crate::disabled::DisabledReason::Subdued)
            || self.under_construction(id)
            || o.status & sold != 0
        {
            return false;
        }
        let t = &self.templates.objects[&o.template];
        t.has_kind("CAN_ATTACK") || o.weapon_set.weapons.iter().any(Option::is_some)
    }

    /// `getAbleToAttackSpecificObject(ATTACK_NEW_TARGET)`: possible now, after
    /// moving, or not at all.
    pub(crate) fn can_attack(&self, id: u32, victim: u32, from_player: bool) -> Option<bool> {
        let (Some(s), Some(v)) = (self.objects.get(&id), self.objects.get(&victim)) else { return None };
        if s.dead || v.dead || id == victim || v.status & crate::body::status::DESTROYED != 0 {
            return None;
        }
        let vt = &self.templates.objects[&v.template];
        if vt.has_kind("UNATTACKABLE") {
            return None;
        }
        let r = self.relationship(id, victim);
        if r != Relationship::Enemies && !(vt.has_kind("MINE") && r != Relationship::Allies) && from_player {
            return None;
        }
        // getAbleToUseWeaponAgainstTarget: the current weapon's range (the
        // original tests it for each slot but always reads the current one).
        let anti = self.victim_anti_mask(victim);
        let cur = s.weapon_set.current as usize;
        let total_anti = s
            .weapon_set
            .weapons
            .iter()
            .flatten()
            .filter_map(|w| self.templates.weapons.get(&w.template))
            .fold(0, |m, w| m | w.anti);
        let has_weapon = s.weapon_set.weapons[cur].is_some();
        let pos = v.position;
        let in_range =
            has_weapon && total_anti & anti != 0 && self.within_attack_range(id, cur, None, Some(victim), pos);
        let st = &self.templates.objects[&s.template];
        if (st.has_kind("IMMOBILE") || st.has_kind("SPAWNS_ARE_THE_WEAPONS")) && has_weapon && !in_range {
            return None;
        }
        if total_anti & anti == 0 {
            return None;
        }
        for i in (0..SLOTS).rev() {
            if self.objects[&id].weapon_set.weapons[i].is_some() && self.estimate_weapon_damage(id, i, victim) != 0.0 {
                return Some(in_range);
            }
        }
        None
    }

    /// `getAdjustedVisionRangeForObject(OWNERTYPE | MOOD [| GUARDINNER])` for a
    /// human player's unit.
    pub(crate) fn adjusted_vision(&self, id: u32, inner: bool) -> f32 {
        let t = &self.templates.objects[&self.objects[&id].template];
        let range = t.number("VisionRange", 0.0);
        let d = &self.templates.ai;
        range * if inner { d.guard_inner_human } else { d.guard_outer_human }
    }

    /// `AI::findClosestEnemy` (`CAN_ATTACK`, `WITHIN_ATTACK_RANGE` for human
    /// players, buildings only with `attack_buildings`): the closest by the 2D
    /// boundary distance, lowest object id on ties.
    pub(crate) fn find_closest_enemy(&mut self, id: u32, range: f32, attack_buildings: bool) -> Option<u32> {
        if !self.able_to_attack(id) {
            return None;
        }
        let candidates: Vec<u32> = self.objects.keys().copied().filter(|o| *o != id).collect();
        let mut best: Option<(f32, u32)> = None;
        for other in candidates {
            let o = &self.objects[&other];
            // LiveMapEnemies
            if o.dead || self.relationship(id, other) != Relationship::Enemies {
                continue;
            }
            let d2 = self.attack_distance_sqr(id, None, Some(other), o.position);
            if d2 > range * range {
                continue;
            }
            let ot = &self.templates.objects[&o.template];
            // RejectBuildings: enemy base defences only, unless buildings are wanted.
            if !attack_buildings && ot.has_kind("STRUCTURE") && !ot.has_kind("FS_BASE_DEFENSE") {
                continue;
            }
            // WithinAttackRange: any weapon.
            let in_range = (0..SLOTS).any(|i| {
                self.objects[&id].weapon_set.weapons[i].is_some()
                    && self.within_attack_range(id, i, None, Some(other), self.objects[&other].position)
            });
            if !in_range {
                continue;
            }
            if self.can_attack(id, other, false).is_none() {
                continue;
            }
            // InsignificantBuildings (with AttackIgnoreInsignificantBuildings).
            if self.templates.ai.attack_ignore_insignificant
                && ot.has_kind("STRUCTURE")
                && !ot.values("KindOf").iter().any(|k| k.starts_with("FS_"))
            {
                continue;
            }
            if best.is_none_or(|(bd, _)| d2 < bd) {
                best = Some((d2, other));
            }
        }
        best.map(|(_, o)| o)
    }

    /// `resetNextMoodCheckTime`.
    fn reset_mood_check(&mut self, id: u32) {
        let now = self.frame;
        let f = self.templates.ai.force_idle_frames;
        let idle = &mut self.objects.get_mut(&id).unwrap().ai.idle;
        idle.next_mood_check = now + f;
        idle.randomly_offset = true;
    }

    /// `AIIdleState::onEnter`.
    pub(crate) fn idle_enter(&mut self, id: u32) {
        if !self
            .objects
            .get(&id)
            .is_some_and(|o| o.modules.iter().any(|m| m.kind == crate::simulation::ModuleKind::Steering))
        {
            return;
        }
        self.reset_mood_check(id);
        let offset = self.rng.integer(0, IDLE_COUNTDOWN_DELAY as i32) as u32;
        let idle = &mut self.objects.get_mut(&id).unwrap().ai.idle;
        idle.inited = true;
        idle.sleep_offset = offset;
    }

    /// `getNextMoodTarget(calledByAI, calledDuringIdle)`.
    pub(crate) fn next_mood_target(&mut self, id: u32, during_idle: bool) -> Option<u32> {
        let o = &self.objects[&id];
        if o.dead {
            return None;
        }
        let (auto, rate) = ai_module_data(&self.templates.objects[&o.template]);
        if during_idle && auto & AAS_IDLE == 0 {
            return None;
        }
        let attacking = matches!(o.ai.state, crate::ai::AiState::AttackObject | crate::ai::AiState::AttackPosition);
        if attacking && auto & AAS_NOT_WHILE_ATTACKING != 0 {
            return None;
        }
        let now = self.frame;
        if now < o.ai.idle.next_mood_check {
            return None;
        }
        let mut next = now + rate;
        if o.ai.idle.randomly_offset {
            let half = (rate >> 1) as i32;
            next = (next as i32 + self.rng.integer(-half, half)) as u32;
        }
        let idle = &mut self.objects.get_mut(&id).unwrap().ai.idle;
        idle.next_mood_check = next;
        idle.randomly_offset = false;
        let range = self.adjusted_vision(id, false);
        if range <= 0.0 {
            return None;
        }
        self.find_closest_enemy(id, range, auto & AAS_ATTACK_BUILDINGS != 0)
    }

    /// `AIIdleState::update`: look for a target, else sleep.
    pub(crate) fn idle_update(&mut self, id: u32) -> u32 {
        let o = &self.objects[&id];
        let mut sleep = IDLE_COUNTDOWN_DELAY + o.ai.idle.sleep_offset;
        let old_offset = o.ai.idle.sleep_offset;
        self.objects.get_mut(&id).unwrap().ai.idle.sleep_offset = 0;
        self.objects.get_mut(&id).unwrap().ai.idle.inited = false;
        use crate::disabled::DisabledReason as D;
        let d = &self.objects[&id].disabled;
        if !(d.has(D::Paralyzed) || d.has(D::Unmanned) || d.has(D::Emp) || d.has(D::Subdued) || d.has(D::Hacked))
            && let Some(enemy) = self.next_mood_target(id, true)
        {
            // aiAttackObject(CMD_FROM_AI).
            let pos = self.objects[&enemy].position;
            self.objects.get_mut(&id).unwrap().ai.cmd_from_ai = true;
            let _ = self.begin_attack(id, Some(enemy), pos);
            return 1;
        }
        let now = self.frame;
        let next = self.objects[&id].ai.idle.next_mood_check;
        if next > now && next - now < sleep {
            sleep = next - now;
            self.objects.get_mut(&id).unwrap().ai.idle.sleep_offset = old_offset;
        }
        sleep
    }
}
