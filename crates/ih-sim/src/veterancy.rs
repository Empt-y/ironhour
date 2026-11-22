//! Weapon bonuses and veterancy.
//!
//! Sources: `GameLogic/Object/Weapon.cpp` (`WeaponBonus`, `WeaponBonusSet`,
//! `Weapon::computeBonus` and the bonus-aware `WeaponTemplate` getters),
//! `GameLogic/Object/ExperienceTracker.cpp`, `Object::scoreTheKill`,
//! `Object::onVeterancyLevelChanged`, `ActiveBody::onVeterancyLevelChanged`,
//! `UpgradeCenter::init` (veterancy upgrades) and `ExperienceScalarUpgrade`.
//!
//! Not ported yet: skill points and bounties for kills (sciences), passing
//! bonuses from containers to passengers, horde, nationalism, enthusiasm,
//! battle plans and continuous fire bonuses (their conditions exist but
//! nothing sets them yet), and the promotion sounds and animation.
use crate::{SimError, Simulation};

/// `WeaponBonus::Field`: damage, radius, range, rate of fire, pre-attack delay.
pub type Bonus = [f32; 5];
pub const DAMAGE: usize = 0;
pub const RADIUS: usize = 1;
pub const RANGE: usize = 2;
pub const RATE_OF_FIRE: usize = 3;
pub const PRE_ATTACK: usize = 4;
const NO_BONUS: Bonus = [1.0; 5];

/// `TheVeterancyNames`.
pub const LEVELS: [&str; 4] = ["REGULAR", "VETERAN", "ELITE", "HEROIC"];

fn bonus_names() -> &'static [&'static str] {
    ih_ini::schema::schema().name_list("TheWeaponBonusNames").unwrap_or(&[])
}
/// A `WeaponBonusConditionType` bit by name.
pub fn bonus_flag(name: &str) -> u32 {
    bonus_names().iter().position(|n| n.eq_ignore_ascii_case(name)).map_or(0, |i| 1 << i)
}

/// `WeaponBonusSet`: a bonus per condition.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct BonusSet(pub Vec<Bonus>);
impl BonusSet {
    /// `parseWeaponBonusSet`: `CONDITION FIELD PERCENT`, a line at a time.
    pub fn add_line(&mut self, v: &[String]) {
        let (Some(c), Some(f), Some(p)) = (v.first(), v.get(1), v.get(2)) else { return };
        let Some(c) = bonus_names().iter().position(|n| n.eq_ignore_ascii_case(c)) else { return };
        let fields = ["DAMAGE", "RADIUS", "RANGE", "RATE_OF_FIRE", "PRE_ATTACK"];
        let Some(f) = fields.iter().position(|n| n.eq_ignore_ascii_case(f)) else { return };
        let pct = ih_ini::values::scan_real(p.trim_end_matches('%').as_bytes()).unwrap_or(100.0) / 100.0;
        if self.0.len() <= c {
            self.0.resize(c + 1, NO_BONUS);
        }
        self.0[c][f] = pct;
    }
    /// `WeaponBonusSet::appendBonuses`.
    fn append(&self, flags: u32, bonus: &mut Bonus) {
        for (i, b) in self.0.iter().enumerate() {
            if flags & (1 << i) != 0 {
                for f in 0..5 {
                    bonus[f] += b[f] - 1.0;
                }
            }
        }
    }
}

/// An object's `ExperienceTracker`.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Experience {
    pub level: u8,
    pub points: i32,
    pub scalar: f32,
    /// `m_experienceSink`: experience goes to this object instead.
    pub sink: Option<u32>,
}
impl Default for Experience {
    fn default() -> Self {
        Self { level: 0, points: 0, scalar: 1.0, sink: None }
    }
}

fn levels(t: &crate::templates::Template, key: &str) -> [i32; 4] {
    let mut out = [0; 4];
    for (i, v) in t.values(key).iter().take(4).enumerate() {
        out[i] = ih_ini::values::scan_int(v.as_bytes()).unwrap_or(0);
    }
    out
}

impl Simulation {
    /// `Weapon::computeBonus`: the global bonuses plus the weapon's own for the
    /// object's conditions.
    pub(crate) fn weapon_bonus(&self, id: u32, wt: &crate::weapons::WeaponTemplate) -> Bonus {
        let mut bonus = NO_BONUS;
        let Some(o) = self.objects.get(&id) else { return bonus };
        let flags = o.bonus_flags;
        if flags == 0 {
            return bonus;
        }
        self.templates.weapon_bonus.append(flags, &mut bonus);
        wt.extra_bonus.append(flags, &mut bonus);
        bonus
    }

    fn trainable(&self, id: u32) -> bool {
        self.templates.objects[&self.objects[&id].template]
            .values("IsTrainable")
            .first()
            .is_some_and(|v| v.eq_ignore_ascii_case("yes"))
    }

    /// `Object::scoreTheKill` (the experience part; skill points and bounties
    /// come with sciences).
    pub(crate) fn score_the_kill(&mut self, killer: u32, victim: u32) {
        let (Some(k), Some(v)) = (self.objects.get(&killer), self.objects.get(&victim)) else { return };
        // Neutral and civilian objects are not a playable side.
        if v.owner == 0 || self.templates.objects[&v.template].has_kind("IGNORED_IN_GUI") {
            return;
        }
        if self.relationship(killer, victim) != crate::weapons::Relationship::Enemies || k.owner == v.owner {
            return;
        }
        let accepting = self.trainable(killer) || k.experience.sink.is_some();
        if accepting && !self.under_construction(victim) {
            let value = levels(&self.templates.objects[&v.template], "ExperienceValue")[v.experience.level as usize];
            self.add_experience(killer, value, true);
        }
    }

    /// `ExperienceTracker::addExperiencePoints`.
    pub fn add_experience(&mut self, id: u32, gain: i32, scale: bool) {
        let Some(o) = self.objects.get(&id) else { return };
        let e = o.experience;
        if let Some(sink) = e.sink.filter(|s| self.objects.contains_key(s)) {
            self.add_experience(sink, (gain as f32 * e.scalar) as i32, scale);
            return;
        }
        if !self.trainable(id) {
            return;
        }
        let amount = if scale { (gain as f32 * e.scalar) as i32 } else { gain };
        let required = levels(&self.templates.objects[&o.template], "ExperienceRequired");
        let points = e.points + amount;
        let mut level = 0;
        while level + 1 < 4 && points >= required[level + 1] {
            level += 1;
        }
        let o = self.objects.get_mut(&id).unwrap();
        o.experience.points = points;
        let old = o.experience.level;
        o.experience.level = level as u8;
        if old != level as u8 {
            self.on_veterancy_changed(id, old, level as u8);
        }
    }

    /// `ExperienceTracker::setVeterancyLevel` (scripts, `set_veterancy`).
    pub(crate) fn set_veterancy(&mut self, id: u32, level: u8) -> Result<(), SimError> {
        let o = self.objects.get(&id).ok_or_else(|| SimError(format!("object {id} is missing")))?;
        let level = level.min(3);
        let old = o.experience.level;
        if old == level {
            return Ok(());
        }
        let required = levels(&self.templates.objects[&o.template], "ExperienceRequired");
        let o = self.objects.get_mut(&id).unwrap();
        o.experience.level = level;
        o.experience.points = required[level as usize];
        self.on_veterancy_changed(id, old, level);
        Ok(())
    }

    /// `Object::onVeterancyLevelChanged` and `ActiveBody::onVeterancyLevelChanged`.
    fn on_veterancy_changed(&mut self, id: u32, old: u8, new: u8) {
        self.update_upgrade_modules(id);
        let upgrade = format!("Upgrade_Veterancy_{}", LEVELS[new as usize]);
        if self.templates.upgrades.contains_key(&upgrade) {
            self.give_object_upgrade(id, &upgrade);
        }
        // HealthBonus_*: the maximum scales, keeping the ratio.
        let b = self.templates.health_bonus;
        let mult = b[new as usize] / b[old as usize];
        if let Some(max) = self.objects[&id].health.as_ref().map(|h| h.max) {
            self.set_max_health(id, max * mult - max, crate::upgrades::HealthChange::PreserveRatio);
        }
        let set_flags = ["VETERAN", "ELITE", "HERO"].map(crate::upgrades::weapon_set_flag);
        let bonus_flags = ["VETERAN", "ELITE", "HERO"].map(bonus_flag);
        let o = self.objects.get_mut(&id).unwrap();
        for i in 0..3 {
            o.upgrades.weapon_set_flags &= !set_flags[i];
            o.bonus_flags &= !bonus_flags[i];
        }
        if new > 0 {
            o.upgrades.weapon_set_flags |= set_flags[new as usize - 1];
            o.bonus_flags |= bonus_flags[new as usize - 1];
        }
        self.update_weapon_set(id);
    }
}
