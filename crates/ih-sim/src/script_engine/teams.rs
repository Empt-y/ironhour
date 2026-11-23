//! ScriptTeams, AIPlayer team work orders and creation scripts.
use super::*;
#[derive(Clone, Debug, Default)]
pub struct Prototype {
    pub player: u32,
    pub units: Vec<(String, u32, u32)>,
    pub priority: i32,
    pub limit: u32,
    pub condition: String,
    pub on_create: String,
    pub recruitable: bool,
}
#[derive(Clone, Debug, Default)]
pub struct Team {
    pub prototype: String,
    pub player: u32,
    pub members: BTreeSet<u32>,
    pub wanted: Vec<(String, u32)>,
    pub pending: BTreeMap<(u32, u32), String>,
    pub ready: bool,
    pub hunt: bool,
    pub priority_set: String,
}
impl ScriptEngine {
    pub(crate) fn hash_teams(&self, h: &mut crate::state_hash::StateHash) {
        h.u32(self.next_team);
        for (set, items) in &self.attack_priorities {
            h.text(set);
            for (unit, priority) in items {
                h.text(unit);
                h.u32(*priority as u32);
            }
        }
        for (name, p) in &self.prototypes {
            h.text(name);
            h.u32(p.player);
            h.u32(p.priority as u32);
            h.u32(p.limit);
            h.text(&p.condition);
            h.text(&p.on_create);
            h.u32(p.recruitable as u32);
            for (unit, min, max) in &p.units {
                h.text(unit);
                h.u32(*min);
                h.u32(*max);
            }
        }
        for (id, t) in &self.teams {
            h.u32(*id);
            h.text(&t.prototype);
            h.u32(t.player);
            h.u32(t.ready as u32);
            h.u32(t.hunt as u32);
            h.text(&t.priority_set);
            h.u64(t.members.len() as u64);
            for member in &t.members {
                h.u32(*member);
            }
            for (unit, n) in &t.wanted {
                h.text(unit);
                h.u32(*n);
            }
            for ((factory, production), unit) in &t.pending {
                h.u32(*factory);
                h.u32(*production);
                h.text(unit);
            }
        }
    }
    pub fn configure_teams(
        &mut self,
        map: &ih_formats::map::Map,
        scb: Option<&ih_formats::scripts::ScbFile>,
        side_of: &dyn Fn(&str) -> Option<String>,
    ) {
        let definitions = scb.map_or(map.sides.teams.as_slice(), |s| s.teams.as_slice());
        for p in &self.players {
            let owner = map
                .sides
                .sides
                .iter()
                .find(|s| {
                    side_of(s.dict.ascii("playerFaction").unwrap_or_default()).as_deref() == Some(p.side.as_str())
                })
                .and_then(|s| s.dict.ascii("playerName"));
            let Some(owner) = owner else { continue };
            for dict in definitions.iter().filter(|d| d.ascii("teamOwner") == Some(owner)) {
                let q = |name: &str| {
                    dict.ascii(name).filter(|s| !s.is_empty()).map(|s| format!("{s}{}", p.start)).unwrap_or_default()
                };
                let name = q("teamName");
                let units = (1..=7)
                    .filter_map(|i| {
                        let unit = dict.ascii(&format!("teamUnitType{i}"))?;
                        let max = dict.int(&format!("teamUnitMaxCount{i}")).unwrap_or(0).max(0) as u32;
                        (max > 0).then(|| {
                            (
                                unit.to_string(),
                                dict.int(&format!("teamUnitMinCount{i}")).unwrap_or(0).max(0) as u32,
                                max,
                            )
                        })
                    })
                    .collect();
                self.prototypes.insert(
                    name,
                    Prototype {
                        player: p.player,
                        units,
                        priority: dict.int("teamProductionPriority").unwrap_or(0),
                        limit: if dict.bool("teamIsSingleton").unwrap_or(false) {
                            1
                        } else {
                            dict.int("teamMaxInstances").unwrap_or(0).max(0) as u32
                        },
                        condition: q("teamProductionCondition"),
                        on_create: q("teamOnCreateScript"),
                        recruitable: dict.bool("teamIsAIRecruitable").unwrap_or(false),
                    },
                );
            }
        }
    }

    pub(super) fn team_ids(&self, name: &str) -> Vec<u32> {
        if name == ih_formats::scripts::THIS_TEAM {
            return self.current_team.into_iter().collect();
        }
        if let Some(current) = self.current_team
            && self.teams.get(&current).is_some_and(|t| t.prototype == name)
        {
            return vec![current];
        }
        self.teams.iter().filter(|(_, t)| t.prototype == name).map(|(id, _)| *id).collect()
    }
}
impl Simulation {
    pub(super) fn request_script_team(&mut self, e: &mut ScriptEngine, name: &str) {
        let Some(p) = e.prototypes.get(name).cloned() else { return };
        let existing =
            e.teams.values().filter(|t| t.prototype == name && (!t.ready || !t.members.is_empty())).count() as u32;
        if existing >= p.limit || p.units.is_empty() {
            return;
        }
        e.next_team += 1;
        let wanted = p
            .units
            .iter()
            .map(|(unit, min, max)| (unit.clone(), self.rng.integer((*min).min(*max) as i32, *max as i32) as u32))
            .collect();
        e.teams
            .insert(e.next_team, Team { prototype: name.to_string(), player: p.player, wanted, ..Default::default() });
    }

    pub(super) fn update_script_teams(&mut self, e: &mut ScriptEngine) {
        for team in e.teams.values_mut() {
            team.members.retain(|id| self.objects.get(id).is_some_and(|o| !o.dead));
            team.pending.retain(|(factory, production), _| {
                self.objects.get(factory).is_some_and(|o| {
                    !o.dead && o.production.as_ref().is_some_and(|p| p.queue.iter().any(|x| x.id == *production))
                })
            });
        }
        if !self.frame.is_multiple_of(30) {
            return;
        }
        for player in e.players.clone().iter().filter(|p| p.ai.is_some()) {
            if !e.bases.get(&player.player).is_some_and(|b| b.units) {
                continue;
            }
            e.current = e.sides.iter().position(|s| s.player == player.player);
            let candidates: Vec<_> = e
                .prototypes
                .iter()
                .filter(|(_, p)| p.player == player.player && p.priority > 0 && !p.units.is_empty())
                .map(|(n, p)| (n.clone(), p.clone()))
                .collect();
            let mut valid = vec![];
            for (name, proto) in candidates {
                if e.teams.values().filter(|t| t.prototype == name && (!t.ready || !t.members.is_empty())).count()
                    as u32
                    >= proto.limit
                {
                    continue;
                }
                if e.teams.values().any(|t| t.prototype == name && !t.ready) {
                    continue;
                }
                if !proto.condition.is_empty() {
                    let Some(r) = e.find_script(&proto.condition) else { continue };
                    let script = e.get(r).script.clone();
                    let allowed = match player.ai.unwrap() {
                        Difficulty::Easy => script.difficulty[0],
                        Difficulty::Normal => script.difficulty[1],
                        Difficulty::Hard => script.difficulty[2],
                    };
                    if !allowed || !self.evaluate_conditions(e, &script) {
                        continue;
                    }
                }
                if !proto.units.iter().all(|(unit, _, _)| {
                    self.objects.values().any(|o| {
                        o.owner == player.player
                            && !o.dead
                            && self.can_make_unit(o.id, unit) == crate::production::CanMake::Ok
                    })
                }) {
                    continue;
                }
                valid.push((name, proto.priority));
            }
            if let Some(priority) = valid.iter().map(|(_, p)| *p).max() {
                valid.retain(|(_, p)| *p == priority);
                let chosen = self.rng.integer(0, valid.len() as i32 - 1) as usize;
                self.request_script_team(e, &valid[chosen].0);
            }
        }
        let ids: Vec<_> = e.teams.keys().copied().collect();
        for id in ids {
            let mut team = e.teams[&id].clone();
            if !team.ready {
                for (unit, wanted) in &team.wanted {
                    let count = team
                        .members
                        .iter()
                        .filter(|id| self.objects.get(id).is_some_and(|o| o.template == *unit))
                        .count() as u32;
                    let pending = team.pending.values().filter(|u| *u == unit).count() as u32;
                    if count + pending >= *wanted {
                        continue;
                    }
                    let recruited = self
                        .objects
                        .values()
                        .find(|o| {
                            o.owner == team.player
                                && !o.dead
                                && !o.ai.active()
                                && o.dozer.as_ref().is_none_or(|d| {
                                    d.primary == crate::construction::Primary::Idle && d.worker != Some(true)
                                })
                                && o.template == *unit
                                && !e.teams.values().any(|t| t.members.contains(&o.id))
                        })
                        .map(|o| o.id);
                    if let Some(unit) = recruited {
                        team.members.insert(unit);
                    } else if let Some(factory) = self
                        .objects
                        .values()
                        .find(|o| {
                            o.owner == team.player
                                && !o.dead
                                && !self.under_construction(o.id)
                                && o.production.is_some()
                                && self.can_make_unit(o.id, unit) == crate::production::CanMake::Ok
                        })
                        .map(|o| o.id)
                        && let Ok(production) = self.queue_create_unit(factory, unit)
                    {
                        team.pending.insert((factory, production), unit.clone());
                    }
                }
                let complete = team.wanted.iter().all(|(unit, n)| {
                    team.members.iter().filter(|id| self.objects.get(id).is_some_and(|o| o.template == *unit)).count()
                        as u32
                        >= *n
                });
                team.ready = complete;
                e.teams.insert(id, team.clone());
                if complete {
                    let script = e.prototypes[&team.prototype].on_create.clone();
                    if let Some(r) = e.find_script(&script) {
                        let saved = e.get(r).clone();
                        e.current = e.sides.iter().position(|s| s.player == team.player);
                        e.current_team = Some(id);
                        self.execute_script(e, r);
                        *e.get_mut(r) = saved;
                        e.current_team = None;
                    }
                }
            }
            if e.teams[&id].hunt {
                let members: Vec<_> = e.teams[&id].members.iter().copied().collect();
                for unit in members {
                    if self.objects[&unit].ai.active() {
                        continue;
                    }
                    let at = self.objects[&unit].position;
                    let target = self
                        .objects
                        .values()
                        .filter(|o| {
                            o.owner != team.player
                                && o.owner != 0
                                && e.players.iter().any(|p| p.player == o.owner)
                                && !o.dead
                                && self.templates.objects[&o.template].has_kind("SELECTABLE")
                        })
                        .min_by(|a, b| {
                            let dist = |o: &crate::simulation::Object| {
                                (o.position[0] - at[0]).powi(2) + (o.position[1] - at[1]).powi(2)
                            };
                            let priority = |o: &crate::simulation::Object| {
                                e.attack_priorities
                                    .get(&e.teams[&id].priority_set)
                                    .and_then(|p| p.get(&o.template))
                                    .copied()
                                    .unwrap_or(0)
                            };
                            priority(b)
                                .cmp(&priority(a))
                                .then_with(|| dist(a).total_cmp(&dist(b)))
                                .then(a.id.cmp(&b.id))
                        })
                        .map(|o| o.id);
                    if let Some(target) = target {
                        let _ = self.begin_attack(unit, Some(target), self.objects[&target].position);
                    }
                }
            }
        }
        e.current = None;
    }

    pub(crate) fn script_produced_unit(&mut self, factory: u32, production: u32, unit: u32) {
        if let Some(e) = &mut self.script_engine
            && let Some(team) = e.teams.values_mut().find(|t| t.pending.contains_key(&(factory, production)))
        {
            team.members.insert(unit);
            team.pending.remove(&(factory, production));
        }
    }
}
