//! Sciences, general's ranks, skill points and special powers.
//!
//! Sources: `Common/RTS/Science.cpp` (`ScienceStore`), `GameLogic/System/RankInfo.cpp`,
//! `Player::addSkillPoints`, `addSkillPointsForKill`, `doBountyForKill`,
//! `addScience`, `grantScience`, `attemptToPurchaseScience`,
//! `isCapableOfPurchasingScience`, `resetRank`, `setRankLevel` and the shared
//! special power timers (`Player.cpp`), `Common/RTS/SpecialPower.cpp`,
//! `SpecialPowerModule.cpp`, `OCLSpecialPower.cpp`, `CashHackSpecialPower.cpp`,
//! `CashBountyPower.cpp`, `SpecialPowerStore::canUseSpecialPower` and
//! `AIGroup::groupDoSpecialPower*`.
//!
//! Not ported yet: player templates (intrinsic sciences and purchase points
//! per side), disabled and hidden sciences, the rank limit set by scripts,
//! special abilities (`SpecialAbilityUpdate`: units that walk to their
//! target), spy vision, defectors, cleanup, demoralise, Baikonur and the
//! other power modules, `UpdateModuleStartsAttack`, view objects,
//! `OCLAdjustPositionToPassable`, and sounds, EVA and superweapon timers.
use crate::{SimError, Simulation};
use std::collections::BTreeMap;

/// `ScienceInfo`.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct ScienceInfo {
    pub prereqs: Vec<String>,
    pub cost: i32,
    pub grantable: bool,
}

/// `RankInfo`.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct RankInfo {
    pub skill_points_needed: i32,
    pub sciences_granted: Vec<String>,
    pub purchase_points_granted: i32,
}

/// `SpecialPowerTemplate` (the logic fields).
#[derive(Clone, Debug, Default, PartialEq)]
pub struct SpecialPowerTemplate {
    pub reload: u32,
    pub required_science: Option<String>,
    pub shared: bool,
}

fn int(v: &[String]) -> i32 {
    v.first().and_then(|s| ih_ini::values::scan_int(s.as_bytes()).ok()).unwrap_or(0)
}
fn sciences(v: &[String]) -> Vec<String> {
    v.iter().filter(|s| !s.eq_ignore_ascii_case("NONE")).cloned().collect()
}

/// Science, rank and special power definitions.
pub fn load(
    defs: &ih_ini::definitions::Definitions,
) -> (BTreeMap<String, ScienceInfo>, Vec<RankInfo>, BTreeMap<String, SpecialPowerTemplate>) {
    let mut sci = BTreeMap::new();
    for name in defs.names("Science") {
        let mut s = ScienceInfo { grantable: true, ..Default::default() };
        for b in defs.all("Science", name) {
            for f in b.fields() {
                let v = f.values();
                match f.name.as_str() {
                    "PrerequisiteSciences" => s.prereqs = sciences(&v),
                    "SciencePurchasePointCost" => s.cost = int(&v),
                    "IsGrantable" => s.grantable = v.first().is_some_and(|x| x.eq_ignore_ascii_case("yes")),
                    _ => {}
                }
            }
        }
        sci.insert(name.clone(), s);
    }
    // Rank N, by level.
    let mut ranks: Vec<(i32, RankInfo)> = vec![];
    for name in defs.names("Rank") {
        let level = ih_ini::values::scan_int(name.as_bytes()).unwrap_or(0);
        let mut r = RankInfo::default();
        for b in defs.all("Rank", name) {
            for f in b.fields() {
                let v = f.values();
                match f.name.as_str() {
                    "SkillPointsNeeded" => r.skill_points_needed = int(&v),
                    "SciencesGranted" => r.sciences_granted = sciences(&v),
                    "SciencePurchasePointsGranted" => r.purchase_points_granted = int(&v),
                    _ => {}
                }
            }
        }
        ranks.push((level, r));
    }
    ranks.sort_by_key(|(l, _)| *l);
    let mut powers = BTreeMap::new();
    for name in defs.names("SpecialPower") {
        let mut p = SpecialPowerTemplate::default();
        for b in defs.all("SpecialPower", name) {
            for f in b.fields() {
                let v = f.values();
                let first = v.first().map_or("", |s| s.as_str());
                match f.name.as_str() {
                    "ReloadTime" => {
                        p.reload = crate::death::duration(ih_ini::values::scan_real(first.as_bytes()).unwrap_or(0.0))
                    }
                    "RequiredScience" => {
                        p.required_science =
                            (!first.is_empty() && !first.eq_ignore_ascii_case("NONE")).then(|| first.to_string())
                    }
                    "SharedSyncedTimer" => p.shared = first.eq_ignore_ascii_case("yes"),
                    _ => {}
                }
            }
        }
        powers.insert(name.clone(), p);
    }
    (sci, ranks.into_iter().map(|(_, r)| r).collect(), powers)
}

/// The kind of special power module.
#[derive(Clone, Debug, PartialEq)]
pub enum PowerKind {
    /// `OCLSpecialPower`: (science, OCL) upgrades, the default OCL, where to create it.
    Ocl { upgrades: Vec<(String, String)>, ocl: Option<String>, create: CreateLocation },
    /// `CashHackSpecialPower`: (science, amount) upgrades and the default amount.
    CashHack { upgrades: Vec<(String, u32)>, amount: u32 },
    /// `CashBountyPower`: the bounty fraction.
    CashBounty { bounty: f32 },
    /// Any other `SpecialPowerModule`: the timer only.
    Other,
}

/// `OCLCreateLocType`.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum CreateLocation {
    #[default]
    EdgeNearSource,
    EdgeNearTarget,
    AtLocation,
    UseOwnerObject,
    AboveLocation,
    EdgeFarthestFromTarget,
}

/// A `SpecialPowerModule`.
#[derive(Clone, Debug, PartialEq)]
pub struct Power {
    pub template: String,
    pub kind: PowerKind,
    pub starts_paused: bool,
    pub script_only: bool,
    pub available_on: u32,
    pub paused_count: u32,
    pub paused_on: u32,
}

impl Power {
    /// A special power module's data, if the module is one.
    pub fn from_module(class: &str, b: &ih_ini::Block) -> Option<Self> {
        let is_power = ih_ini::schema::MODULES
            .iter()
            .find(|d| d.name == class)
            .is_some_and(|d| d.tables.iter().any(|t| t.starts_with("SpecialPowerModuleData::")));
        if !is_power {
            return None;
        }
        let mut p = Power {
            template: String::new(),
            kind: PowerKind::Other,
            starts_paused: false,
            script_only: false,
            available_on: 0,
            paused_count: 0,
            paused_on: 0,
        };
        let (mut ocl_up, mut ocl, mut create) = (vec![], None, CreateLocation::default());
        let (mut cash_up, mut amount, mut bounty) = (vec![], 0, 0.0);
        let names = [
            "CREATE_AT_EDGE_NEAR_SOURCE",
            "CREATE_AT_EDGE_NEAR_TARGET",
            "CREATE_AT_LOCATION",
            "USE_OWNER_OBJECT",
            "CREATE_ABOVE_LOCATION",
            "CREATE_AT_EDGE_FARTHEST_FROM_TARGET",
        ];
        for f in b.fields() {
            let v = f.values();
            let first = v.first().cloned().unwrap_or_default();
            let yes = first.eq_ignore_ascii_case("yes");
            match f.name.as_str() {
                "SpecialPowerTemplate" => p.template = first,
                "StartsPaused" => p.starts_paused = yes,
                "ScriptedSpecialPowerOnly" => p.script_only = yes,
                "OCL" => ocl = Some(first),
                "UpgradeOCL" => {
                    if let Some(o) = v.get(1) {
                        ocl_up.push((first, o.clone()));
                    }
                }
                "CreateLocation" => {
                    create = match names.iter().position(|n| n.eq_ignore_ascii_case(&first)) {
                        Some(1) => CreateLocation::EdgeNearTarget,
                        Some(2) => CreateLocation::AtLocation,
                        Some(3) => CreateLocation::UseOwnerObject,
                        Some(4) => CreateLocation::AboveLocation,
                        Some(5) => CreateLocation::EdgeFarthestFromTarget,
                        _ => CreateLocation::EdgeNearSource,
                    }
                }
                "MoneyAmount" => amount = int(&v).max(0) as u32,
                "UpgradeMoneyAmount" => {
                    if let Some(a) = v.get(1) {
                        cash_up.push((first, ih_ini::values::scan_int(a.as_bytes()).unwrap_or(0).max(0) as u32));
                    }
                }
                // INI::parsePercentToReal
                "Bounty" => {
                    bounty = ih_ini::values::scan_real(first.trim_end_matches('%').as_bytes()).unwrap_or(0.0) / 100.0
                }
                _ => {}
            }
        }
        p.kind = match class {
            "OCLSpecialPower" => PowerKind::Ocl { upgrades: ocl_up, ocl, create },
            "CashHackSpecialPower" => PowerKind::CashHack { upgrades: cash_up, amount },
            "CashBountyPower" => PowerKind::CashBounty { bounty },
            _ => PowerKind::Other,
        };
        Some(p)
    }
}

/// A player's science state (`Player`'s rank and science members).
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Generals {
    /// `m_rankLevel` (0 until `reset_rank` runs).
    pub rank: i32,
    pub skill_points: i32,
    pub level_up: i32,
    pub level_down: i32,
    pub purchase_points: i32,
    /// `m_skillPointsModifier`.
    pub skill_modifier: f32,
    /// `m_cashBountyPercent`.
    pub cash_bounty: f32,
    /// `m_specialPowerReadyTimerList`: shared timers by power.
    pub shared_timers: Vec<(String, u32)>,
}

impl Simulation {
    fn rank_info(&self, level: i32) -> Option<&RankInfo> {
        (level >= 1).then(|| self.templates.ranks.get(level as usize - 1)).flatten()
    }

    /// `Player::resetRank`, on a player's first use.
    pub(crate) fn reset_rank(&mut self, player: u32) {
        let next = self.rank_info(2).map_or(i32::MAX, |r| r.skill_points_needed);
        let granted = self.rank_info(1).map_or(0, |r| r.purchase_points_granted);
        let sciences = self.rank_info(1).map(|r| r.sciences_granted.clone()).unwrap_or_default();
        let p = self.player(player);
        p.generals = Generals {
            rank: 1,
            skill_points: 0,
            level_up: next,
            level_down: 0,
            purchase_points: granted,
            skill_modifier: 1.0,
            ..p.generals.clone()
        };
        p.sciences.clear();
        // resetSciences: the sciences of every rank so far.
        for s in sciences {
            self.add_science(player, &s);
        }
    }

    /// `Player::addScience`: special powers that needed it start their timers.
    pub fn add_science(&mut self, player: u32, science: &str) -> bool {
        if !self.player(player).sciences.insert(science.to_string()) {
            return false;
        }
        let now = self.frame;
        let ids: Vec<u32> = self.objects.values().filter(|o| o.owner == player).map(|o| o.id).collect();
        for id in ids {
            for i in 0..self.objects[&id].powers.len() {
                let t = self.objects[&id].powers[i].template.clone();
                let needs = self.templates.special_powers.get(&t).and_then(|t| t.required_science.clone());
                if needs.as_deref() == Some(science) {
                    self.on_power_creation(id, i);
                    self.objects.get_mut(&id).unwrap().powers[i].available_on = now;
                }
            }
        }
        true
    }

    /// `Player::grantScience`.
    pub fn grant_science(&mut self, player: u32, science: &str) -> bool {
        if !self.templates.sciences.get(science).is_some_and(|s| s.grantable) {
            return false;
        }
        self.add_science(player, science)
    }

    /// `Player::isCapableOfPurchasingScience`.
    pub fn can_purchase_science(&mut self, player: u32, science: &str) -> bool {
        let Some(info) = self.templates.sciences.get(science).cloned() else { return false };
        let p = self.player(player);
        if p.sciences.contains(science) || !info.prereqs.iter().all(|s| p.sciences.contains(s)) {
            return false;
        }
        info.cost != 0 && info.cost <= p.generals.purchase_points
    }

    /// `Player::attemptToPurchaseScience` (`MSG_PURCHASE_SCIENCE`).
    pub(crate) fn purchase_science(&mut self, player: u32, science: &str) -> Result<(), SimError> {
        if !self.can_purchase_science(player, science) {
            return Err(SimError(format!("player {player} cannot purchase {science}")));
        }
        let cost = self.templates.sciences[science].cost;
        let p = self.player(player);
        p.generals.purchase_points = (p.generals.purchase_points - cost).max(0);
        self.add_science(player, science);
        Ok(())
    }

    /// `Player::addSkillPoints`: true when a rank was gained.
    pub fn add_skill_points(&mut self, player: u32, delta: i32) -> bool {
        let modifier = self.player(player).generals.skill_modifier;
        let delta = (modifier * delta as f32).ceil() as i32;
        if delta == 0 {
            return false;
        }
        let cap_level = self.templates.ranks.len() as i32;
        let cap = self.rank_info(cap_level).map_or(i32::MAX, |r| r.skill_points_needed);
        let g = &mut self.player(player).generals;
        g.skill_points = cap.min(g.skill_points + delta);
        let mut gained = false;
        while self.player(player).generals.skill_points >= self.player(player).generals.level_up {
            let next = self.player(player).generals.rank + 1;
            if !self.set_rank_level(player, next) {
                break;
            }
            gained = true;
        }
        gained
    }

    /// `Player::setRankLevel` (upwards; lowering resets first).
    pub fn set_rank_level(&mut self, player: u32, level: i32) -> bool {
        let level = level.clamp(1, (self.templates.ranks.len() as i32).max(1));
        if level == self.player(player).generals.rank {
            return false;
        }
        if level < self.player(player).generals.rank {
            self.reset_rank(player);
        }
        let from = self.player(player).generals.rank + 1;
        for i in from..=level {
            let Some(r) = self.rank_info(i).cloned() else { continue };
            let g = &mut self.player(player).generals;
            g.purchase_points = (g.purchase_points + r.purchase_points_granted).max(0);
            g.skill_points = g.skill_points.max(r.skill_points_needed);
            g.level_down = r.skill_points_needed;
            for s in &r.sciences_granted {
                self.add_science(player, s);
            }
        }
        let next = self.rank_info(level + 1).map_or(i32::MAX, |r| r.skill_points_needed);
        let g = &mut self.player(player).generals;
        g.level_up = next;
        g.rank = level;
        true
    }

    /// `Player::addSkillPointsForKill` and `doBountyForKill`, from `scoreTheKill`.
    pub(crate) fn kill_rewards(&mut self, killer: u32, victim: u32) {
        let (Some(k), Some(v)) = (self.objects.get(&killer), self.objects.get(&victim)) else { return };
        if v.status & crate::construction::under_construction_bit() != 0 {
            return;
        }
        let player = k.owner;
        let t = &self.templates.objects[&v.template];
        let level = v.experience.level as usize;
        let value = t
            .values("SkillPointValue")
            .get(level)
            .and_then(|s| ih_ini::values::scan_int(s.as_bytes()).ok())
            // USE_EXP_VALUE_FOR_SKILL_VALUE: the experience value.
            .unwrap_or_else(|| {
                t.values("ExperienceValue")
                    .get(level)
                    .and_then(|s| ih_ini::values::scan_int(s.as_bytes()).ok())
                    .unwrap_or(0)
            });
        let cost = self.cost_to_build(t, v.owner);
        self.add_skill_points(player, value);
        let bounty = (cost as f32 * self.player(player).generals.cash_bounty).ceil() as u32;
        if bounty > 0 {
            self.player(player).deposit(bounty);
        }
    }

    // ----- special powers -----

    /// The special power modules' constructors.
    pub(crate) fn powers_created(&mut self, id: u32) {
        let building = self.under_construction(id);
        for i in 0..self.objects[&id].powers.len() {
            let p = &self.objects[&id].powers[i];
            let shared = self.templates.special_powers.get(&p.template).is_some_and(|t| t.shared);
            if !building && !shared {
                self.start_power_recharge(id, i);
            }
            if self.objects[&id].powers[i].starts_paused {
                self.pause_power(id, i, true);
            }
            // CashBountyPower::onObjectCreated
            self.raise_bounty(id, i, true);
        }
    }

    fn raise_bounty(&mut self, id: u32, i: usize, needs_science: bool) {
        let p = &self.objects[&id].powers[i];
        let PowerKind::CashBounty { bounty } = p.kind else { return };
        let owner = self.objects[&id].owner;
        let required = self.templates.special_powers.get(&p.template).and_then(|t| t.required_science.clone());
        // hasScience(SCIENCE_INVALID) is false.
        if needs_science && !required.is_some_and(|s| self.player(owner).sciences.contains(&s)) {
            return;
        }
        let g = &mut self.player(owner).generals;
        if bounty > g.cash_bounty {
            g.cash_bounty = bounty;
        }
    }

    /// `SpecialPowerModule::onSpecialPowerCreation`.
    fn on_power_creation(&mut self, id: u32, i: usize) {
        self.start_power_recharge(id, i);
        let p = &self.objects[&id].powers[i];
        let template = p.template.clone();
        if self.templates.special_powers.get(&template).is_some_and(|t| t.shared) {
            let owner = self.objects[&id].owner;
            let now = self.frame;
            self.set_shared_timer(owner, &template, now);
            let ready = self.shared_ready_frame(owner, &template);
            self.objects.get_mut(&id).unwrap().powers[i].available_on = ready;
        }
        if self.objects[&id].powers[i].starts_paused {
            self.pause_power(id, i, true);
        }
        self.raise_bounty(id, i, false);
    }

    fn set_shared_timer(&mut self, player: u32, template: &str, frame: u32) {
        let timers = &mut self.player(player).generals.shared_timers;
        match timers.iter_mut().find(|(t, _)| t == template) {
            Some(t) => t.1 = frame,
            None => timers.push((template.to_string(), frame)),
        }
    }
    /// `Player::getOrStartSpecialPowerReadyFrame`.
    fn shared_ready_frame(&mut self, player: u32, template: &str) -> u32 {
        let now = self.frame;
        let timers = &mut self.player(player).generals.shared_timers;
        if let Some((_, f)) = timers.iter().find(|(t, _)| t == template) {
            return *f;
        }
        timers.push((template.to_string(), now));
        now
    }

    /// `SpecialPowerModule::startPowerRecharge`.
    fn start_power_recharge(&mut self, id: u32, i: usize) {
        let template = self.objects[&id].powers[i].template.clone();
        let Some(t) = self.templates.special_powers.get(&template).cloned() else { return };
        let now = self.frame;
        if t.shared {
            let owner = self.objects[&id].owner;
            self.set_shared_timer(owner, &template, now + t.reload);
        } else {
            self.objects.get_mut(&id).unwrap().powers[i].available_on = now + t.reload;
        }
    }

    /// `SpecialPowerModule::pauseCountdown`.
    pub fn pause_power(&mut self, id: u32, i: usize, pause: bool) {
        let now = self.frame;
        let p = &mut self.objects.get_mut(&id).unwrap().powers[i];
        if pause {
            if p.paused_count == 0 {
                p.paused_on = now;
            }
            p.paused_count += 1;
        } else if p.paused_count > 0 {
            p.paused_count -= 1;
            if p.paused_count == 0 {
                p.available_on += now - p.paused_on;
            }
        }
    }

    /// `SpecialPowerModule::isReady`.
    pub fn power_ready(&mut self, id: u32, i: usize) -> bool {
        let p = self.objects[&id].powers[i].clone();
        if self.templates.special_powers.get(&p.template).is_some_and(|t| t.shared) {
            let owner = self.objects[&id].owner;
            return self.frame >= self.shared_ready_frame(owner, &p.template);
        }
        p.paused_count == 0 && self.frame >= p.available_on
    }

    /// `MSG_DO_SPECIAL_POWER*`: the science and readiness checks, then the power.
    pub(crate) fn do_special_power(
        &mut self,
        id: u32,
        power: &str,
        target: Option<u32>,
        location: Option<[f32; 2]>,
    ) -> Result<(), SimError> {
        let o = self.objects.get(&id).ok_or_else(|| SimError(format!("object {id} is missing")))?;
        let i = o
            .powers
            .iter()
            .position(|p| p.template == power)
            .ok_or_else(|| SimError(format!("object {id} has no special power {power}")))?;
        let owner = o.owner;
        let required = self.templates.special_powers.get(power).and_then(|t| t.required_science.clone());
        if let Some(s) = required
            && !self.player(owner).sciences.contains(&s)
        {
            return Err(SimError(format!("player {owner} lacks {s}")));
        }
        if self.objects[&id].powers[i].script_only || !self.power_ready(id, i) {
            return Err(SimError(format!("special power {power} is not ready")));
        }
        if self.objects[&id].disabled.any() || self.objects[&id].powers[i].paused_count > 0 {
            return Ok(());
        }
        let target_pos = match (target, location) {
            (Some(t), _) => self.objects.get(&t).map(|o| o.position),
            (None, Some(p)) => {
                let z = self.map.ground_height(p[0], p[1]);
                Some([p[0], p[1], z])
            }
            (None, None) => None,
        };
        let kind = self.objects[&id].powers[i].kind.clone();
        let has = |sim: &mut Simulation, s: &str| sim.player(owner).sciences.contains(s);
        match kind {
            PowerKind::Ocl { upgrades, ocl, create } => {
                let ocl = upgrades.iter().find(|(s, _)| has(self, s)).map(|(_, o)| o.clone()).or(ocl);
                let me = self.objects[&id].position;
                let at = target_pos.unwrap_or(me);
                // triggerSpecialPower comes first (doSpecialPowerAtLocation's base call).
                self.start_power_recharge(id, i);
                if let Some(ocl) = ocl {
                    let from = match (target_pos, create) {
                        (None, _) | (_, CreateLocation::AtLocation | CreateLocation::UseOwnerObject) => at,
                        (_, CreateLocation::EdgeNearSource) => self.closest_edge_point(me),
                        (_, CreateLocation::EdgeNearTarget) => self.closest_edge_point(at),
                        (_, CreateLocation::EdgeFarthestFromTarget) => {
                            let mut p = self.farthest_edge_point(at);
                            p[2] += 300.0;
                            p
                        }
                        (_, CreateLocation::AboveLocation) => [at[0], at[1], at[2] + 300.0],
                    };
                    self.create_ocl_at(&ocl, id, from, Some(at), None);
                }
            }
            PowerKind::CashHack { upgrades, amount } => {
                let Some(victim) = target else {
                    return Err(SimError("a cash hack needs a target".into()));
                };
                self.start_power_recharge(id, i);
                let amount = upgrades.iter().find(|(s, _)| has(self, s)).map_or(amount, |(_, a)| *a);
                let victim_owner = self.objects.get(&victim).map_or(0, |o| o.owner);
                let cash = self.player(victim_owner).withdraw(amount);
                self.player(owner).deposit(cash);
            }
            PowerKind::CashBounty { .. } | PowerKind::Other => self.start_power_recharge(id, i),
        }
        Ok(())
    }

    /// `TerrainLogic::findClosestEdgePoint`.
    fn closest_edge_point(&self, p: [f32; 3]) -> [f32; 3] {
        let (lo, hi) = self.extent();
        let d = [(p[1] - lo[1]).abs(), (p[0] - hi[0]).abs(), (p[1] - hi[1]).abs(), (p[0] - lo[0]).abs()];
        let mut best = 0;
        for i in 1..4 {
            if d[i] < d[best] {
                best = i;
            }
        }
        let mut r = p;
        match best {
            0 => r[1] = lo[1],
            1 => r[0] = hi[0],
            2 => r[1] = hi[1],
            _ => r[0] = lo[0],
        }
        r[2] = self.map.ground_height(r[0], r[1]);
        r
    }
    /// `TerrainLogic::findFarthestEdgePoint`.
    fn farthest_edge_point(&self, p: [f32; 3]) -> [f32; 3] {
        let (lo, hi) = self.extent();
        let x = if p[0] < (hi[0] - lo[0]) / 2.0 { hi[0] } else { lo[0] };
        let y = if p[1] < (hi[1] - lo[1]) / 2.0 { hi[1] } else { lo[1] };
        [x, y, self.map.ground_height(x, y)]
    }
}
