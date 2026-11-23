//! World queries and actions from ScriptConditions.cpp / ScriptActions.cpp.
use super::*;

impl Simulation {
    pub(super) fn script_area<'a>(
        &'a self,
        e: &ScriptEngine,
        name: &str,
    ) -> Option<&'a ih_formats::map::PolygonTrigger> {
        let mut name = name.to_string();
        if name.starts_with("[Skirmish]") {
            let player =
                e.player_named(if name.contains("Enemy") { "<This Player's Enemy>" } else { "<This Player>" })?;
            let start = e.players.iter().find(|p| p.player == player)?.start + 1;
            name = format!("{}Perimeter{start}", if name.contains("Outer") { "Outer" } else { "Inner" });
        }
        self.map.triggers.iter().find(|t| t.name == name)
    }

    pub(super) fn script_area_center(area: &ih_formats::map::PolygonTrigger) -> [f32; 2] {
        let mut lo = [f32::INFINITY; 2];
        let mut hi = [f32::NEG_INFINITY; 2];
        for p in &area.points {
            for i in 0..2 {
                lo[i] = lo[i].min(p[i] as f32);
                hi[i] = hi[i].max(p[i] as f32);
            }
        }
        [(lo[0] + hi[0]) * 0.5, (lo[1] + hi[1]) * 0.5]
    }

    fn script_inside(area: &ih_formats::map::PolygonTrigger, p: [f32; 3]) -> bool {
        let mut inside = false;
        for (a, b) in area.points.iter().zip(area.points.iter().cycle().skip(1)).take(area.points.len()) {
            if (a[1] as f32 > p[1]) != (b[1] as f32 > p[1])
                && p[0] < (b[0] - a[0]) as f32 * (p[1] - a[1] as f32) / (b[1] - a[1]) as f32 + a[0] as f32
            {
                inside = !inside;
            }
        }
        inside
    }
    fn script_count(&self, e: &ScriptEngine, player: u32, what: &str, unfinished: bool) -> i32 {
        self.objects
            .values()
            .filter(|o| {
                o.owner == player
                    && (unfinished || !self.under_construction(o.id))
                    && e.object_lists.get(what).map_or(o.template == what, |list| list.contains(&o.template))
            })
            .count() as i32
    }

    fn script_named(&self, name: &str) -> Option<&crate::simulation::Object> {
        self.objects
            .values()
            .find(|o| o.map_index.is_some_and(|i| self.map.objects[i].properties.ascii("objectName") == Some(name)))
    }

    pub(super) fn script_world_condition(&self, e: &mut ScriptEngine, c: &Call) -> Option<bool> {
        let s = |i| param_str(c, i);
        let n = |i| param_int(c, i);
        let player = |i| e.player_named(s(i));
        Some(match name_of(c) {
            "START_POSITION_IS" => {
                player(0).is_some_and(|p| e.players.iter().any(|x| x.player == p && x.start as i32 + 1 == n(1)))
            }
            "SKIRMISH_NAMED_AREA_EXIST" => self.script_area(e, s(1)).is_some(),
            "SKIRMISH_VALUE_IN_AREA" => player(0).is_some_and(|p| {
                self.script_area(e, s(3)).is_some_and(|area| {
                    let cost = self
                        .objects
                        .values()
                        .filter(|o| {
                            o.owner == p
                                && !o.dead
                                && !self.templates.objects[&o.template].has_kind("INERT")
                                && Self::script_inside(area, o.position)
                        })
                        .map(|o| self.templates.objects[&o.template].number("BuildCost", 0.0) as i32)
                        .sum();
                    compare(cost, n(1), n(2))
                })
            }),
            "SKIRMISH_TECH_BUILDING_WITHIN_DISTANCE" => player(0).is_some_and(|p| {
                self.script_area(e, s(2)).is_some_and(|area| {
                    let at = Self::script_area_center(area);
                    let radius = area
                        .points
                        .iter()
                        .map(|v| ih_math::length([v[0] as f32 - at[0], v[1] as f32 - at[1]]))
                        .fold(0.0, f32::max)
                        + param_real(c, 1);
                    self.objects.values().any(|o| {
                        o.owner != p
                            && !o.dead
                            && self.templates.objects[&o.template].has_kind("TECH_BUILDING")
                            && ih_math::length([o.position[0] - at[0], o.position[1] - at[1]]) <= radius
                    })
                })
            }),
            "PLAYER_HAS_COMPARISON_UNIT_TYPE_IN_TRIGGER_AREA" => player(0).is_some_and(|p| {
                self.script_area(e, s(4)).is_some_and(|area| {
                    let count = self
                        .objects
                        .values()
                        .filter(|o| {
                            o.owner == p
                                && !o.dead
                                && e.object_lists
                                    .get(s(3))
                                    .map_or(o.template == s(3), |list| list.contains(&o.template))
                                && Self::script_inside(area, o.position)
                        })
                        .count() as i32;
                    compare(count, n(1), n(2))
                })
            }),
            "TEAM_DESTROYED" => e.team_ids(s(0)).iter().all(|id| {
                e.teams[id].ready && e.teams[id].members.iter().all(|id| self.objects.get(id).is_none_or(|o| o.dead))
            }),
            "TEAM_CREATED" => !e.team_ids(s(0)).is_empty(),
            "TEAM_HAS_UNITS" => e
                .team_ids(s(0))
                .iter()
                .any(|id| e.teams[id].members.iter().any(|id| self.objects.get(id).is_some_and(|o| !o.dead))),
            "SKIRMISH_PLAYER_FACTION" => {
                player(0).is_some_and(|p| e.players.iter().any(|x| x.player == p && x.side == s(1)))
            }
            "BUILT_BY_PLAYER" => player(1).is_some_and(|p| self.script_count(e, p, s(0), false) > 0),
            "PLAYER_HAS_OBJECT_COMPARISON" => {
                player(0).is_some_and(|p| compare(self.script_count(e, p, s(3), false), n(1), n(2)))
            }
            "PLAYER_LOST_OBJECT_TYPE" => {
                let p = player(0)?;
                let count = self.script_count(e, p, s(1), true);
                let previous = e.object_counts.insert((p, s(1).to_string()), count).unwrap_or(0);
                count < previous
            }
            // The original compares the amount parameter to the balance, in this order.
            "PLAYER_HAS_CREDITS" => player(2).is_some_and(|p| compare(n(0), n(1), self.money(p) as i32)),
            "PLAYER_HAS_POWER" | "PLAYER_HAS_NO_POWER" => player(0).is_some_and(|p| {
                let (supply, demand) = self.energy(p);
                (supply >= demand) == (name_of(c) == "PLAYER_HAS_POWER")
            }),
            "PLAYER_POWER_COMPARE_PERCENT" | "PLAYER_EXCESS_POWER_COMPARE_VALUE" => player(0).is_some_and(|p| {
                let (supply, demand) = self.energy(p);
                let value = if name_of(c) == "PLAYER_POWER_COMPARE_PERCENT" {
                    if demand == 0 { supply as f32 * 100.0 } else { supply as f32 / demand as f32 * 100.0 }
                } else {
                    (supply - demand) as f32
                };
                match n(1) {
                    0 => value < n(2) as f32,
                    1 => value <= n(2) as f32,
                    2 => value == n(2) as f32,
                    3 => value >= n(2) as f32,
                    4 => value > n(2) as f32,
                    5 => value != n(2) as f32,
                    _ => false,
                }
            }),
            "PLAYER_BUILT_UPGRADE" => {
                player(0).is_some_and(|p| self.players.get(&p).is_some_and(|x| x.upgrades.completed.contains(s(1))))
            }
            "PLAYER_ACQUIRED_SCIENCE" => {
                player(0).is_some_and(|p| self.players.get(&p).is_some_and(|x| x.sciences.contains(s(1))))
            }
            "SKIRMISH_SPECIAL_POWER_READY" => player(0).is_some_and(|p| {
                self.objects.values().any(|o| {
                    o.owner == p
                        && !o.dead
                        && !o.disabled.any()
                        && !self.under_construction(o.id)
                        && o.powers.iter().any(|power| {
                            power.template == s(1) && power.available_on <= self.frame && power.paused_count == 0
                        })
                        && self.templates.special_powers.get(s(1)).is_some_and(|power| {
                            power.required_science.as_ref().is_none_or(|science| {
                                self.players.get(&p).is_some_and(|x| x.sciences.contains(science))
                            })
                        })
                })
            }),
            "NAMED_NOT_DESTROYED" => self.script_named(s(0)).is_some_and(|o| !o.dead),
            "NAMED_DESTROYED" | "NAMED_TOTALLY_DEAD" => self.script_named(s(0)).is_none_or(|o| o.dead),
            "NAMED_DYING" => self.script_named(s(0)).is_some_and(|o| o.dead),
            "NAMED_CREATED" => self.script_named(s(0)).is_some(),
            "NAMED_OWNED_BY_PLAYER" => self.script_named(s(0)).is_some_and(|o| Some(o.owner) == player(1)),
            "PLAYER_ALL_DESTROYED" => player(0).is_none_or(|p| !self.objects.values().any(|o| o.owner == p && !o.dead)),
            "PLAYER_HAS_N_OR_FEWER_BUILDINGS" | "PLAYER_HAS_N_OR_FEWER_FACTION_BUILDINGS" => {
                player(0).is_some_and(|p| {
                    self.objects
                        .values()
                        .filter(|o| {
                            o.owner == p
                                && !o.dead
                                && self.templates.objects[&o.template].has_kind("STRUCTURE")
                                && (name_of(c) != "PLAYER_HAS_N_OR_FEWER_FACTION_BUILDINGS"
                                    || !self.templates.objects[&o.template].has_kind("TECH_BUILDING"))
                        })
                        .count() as i32
                        <= n(1)
                })
            }
            _ => return None,
        })
    }

    pub(super) fn script_world_action(&mut self, e: &mut ScriptEngine, c: &Call) -> bool {
        match name_of(c) {
            "SET_ATTACK_PRIORITY_THING" => {
                let units = e
                    .object_lists
                    .get(param_str(c, 1))
                    .cloned()
                    .unwrap_or_else(|| [param_str(c, 1).to_string()].into_iter().collect());
                let list = e.attack_priorities.entry(param_str(c, 0).to_string()).or_default();
                for unit in units {
                    list.insert(unit, param_int(c, 2));
                }
            }
            "TEAM_APPLY_ATTACK_PRIORITY_SET" => {
                for team in e.team_ids(param_str(c, 0)) {
                    e.teams.get_mut(&team).unwrap().priority_set = param_str(c, 1).to_string();
                }
            }
            "SKIRMISH_FIRE_SPECIAL_POWER_AT_MOST_COST" => {
                if let Some(player) = e.player_named(param_str(c, 0)) {
                    let power = param_str(c, 1);
                    let caster = self
                        .objects
                        .values()
                        .find(|o| {
                            o.owner == player
                                && !o.dead
                                && !self.under_construction(o.id)
                                && !o.disabled.any()
                                && o.powers
                                    .iter()
                                    .any(|p| p.template == power && p.available_on <= self.frame && p.paused_count == 0)
                        })
                        .map(|o| o.id);
                    let target = self
                        .objects
                        .values()
                        .filter(|o| {
                            o.owner != player
                                && o.owner != 0
                                && !o.dead
                                && e.players.iter().any(|p| p.player == o.owner)
                        })
                        .max_by_key(|o| self.templates.objects[&o.template].number("BuildCost", 0.0) as i32)
                        .map(|o| o.id);
                    if let (Some(caster), Some(target)) = (caster, target) {
                        let _ = self.do_special_power(caster, power, Some(target), None);
                    }
                }
            }
            "TEAM_USE_COMMANDBUTTON_ABILITY" => {
                let button = param_str(c, 1);
                for team in e.team_ids(param_str(c, 0)) {
                    for unit in e.teams[&team].members.clone() {
                        if let Some(power) = self.templates.command_button_powers.get(button).cloned() {
                            let _ = self.do_special_power(unit, &power, None, None);
                        } else if let Some(upgrade) = self.templates.command_button_upgrades.get(button).cloned() {
                            let _ = self.queue_upgrade(unit, &upgrade);
                        }
                    }
                }
            }
            "TEAM_GUARD_AREA" => {
                if let Some(area) = self.script_area(e, param_str(c, 1)) {
                    let point = Self::script_area_center(area);
                    for id in e.team_ids(param_str(c, 0)) {
                        e.teams.get_mut(&id).unwrap().hunt = false;
                        for unit in e.teams[&id].members.clone() {
                            let _ = self.begin_guard(unit, point);
                        }
                    }
                }
            }
            "TEAM_ENTER_NAMED" | "TEAM_LOAD_TRANSPORTS" | "TEAM_GARRISON_NEAREST_BUILDING" => {
                for id in e.team_ids(param_str(c, 0)) {
                    let team = e.teams[&id].clone();
                    for unit in team.members.iter().copied() {
                        let at = self.objects[&unit].position;
                        let target = if name_of(c) == "TEAM_ENTER_NAMED" {
                            self.script_named(param_str(c, 1)).map(|o| o.id)
                        } else {
                            self.objects
                                .values()
                                .filter(|o| {
                                    !o.dead
                                        && self.can_enter_object(unit, o.id, true)
                                        && if name_of(c) == "TEAM_LOAD_TRANSPORTS" {
                                            team.members.contains(&o.id)
                                        } else {
                                            self.templates.objects[&o.template].has_kind("STRUCTURE")
                                        }
                                })
                                .min_by(|a, b| {
                                    ih_math::length([a.position[0] - at[0], a.position[1] - at[1]])
                                        .total_cmp(&ih_math::length([b.position[0] - at[0], b.position[1] - at[1]]))
                                })
                                .map(|o| o.id)
                        };
                        if let Some(target) = target {
                            let _ = self.begin_enter(unit, target, true);
                        }
                    }
                }
            }
            "TEAM_EXIT_ALL" => {
                for id in e.team_ids(param_str(c, 0)) {
                    for unit in e.teams[&id].members.clone() {
                        let _ = self.evacuate(unit, true);
                        let _ = self.begin_exit(unit, true);
                    }
                }
            }
            "TEAM_AVAILABLE_FOR_RECRUITMENT" => {
                for id in e.team_ids(param_str(c, 0)) {
                    let name = e.teams[&id].prototype.clone();
                    if let Some(p) = e.prototypes.get_mut(&name) {
                        p.recruitable = param_int(c, 1) != 0;
                    }
                }
            }
            "AI_PLAYER_BUILD_UPGRADE" => {
                if let Some(player) = e.player_named(param_str(c, 0)) {
                    let factories: Vec<_> = self
                        .objects
                        .values()
                        .filter(|o| {
                            o.owner == player && !o.dead && !self.under_construction(o.id) && o.production.is_some()
                        })
                        .map(|o| o.id)
                        .collect();
                    for factory in factories {
                        if self.queue_upgrade(factory, param_str(c, 1)).is_ok() {
                            break;
                        }
                    }
                }
            }
            "AI_PLAYER_BUILD_SUPPLY_CENTER" => {
                if let Some(player) = e.player_named(param_str(c, 0)) {
                    self.request_supply_building(e, player, param_str(c, 1), param_int(c, 2));
                }
            }
            "BUILD_TEAM" => self.request_script_team(e, param_str(c, 0)),
            "TEAM_HUNT" => {
                for id in e.team_ids(param_str(c, 0)) {
                    e.teams.get_mut(&id).unwrap().hunt = true;
                }
            }
            "TEAM_STOP" | "TEAM_STOP_AND_DISBAND" => {
                for id in e.team_ids(param_str(c, 0)) {
                    let team = e.teams.get_mut(&id).unwrap();
                    team.hunt = false;
                    for unit in &team.members {
                        let _ = self.enqueue(self.frame, crate::Command::Stop { object: *unit });
                    }
                    if name_of(c) == "TEAM_STOP_AND_DISBAND" {
                        team.members.clear();
                    }
                }
            }
            "TEAM_INCREASE_PRIORITY" | "TEAM_DECREASE_PRIORITY" => {
                if let Some(proto) = e.prototypes.get_mut(param_str(c, 0)) {
                    proto.priority +=
                        if name_of(c) == "TEAM_INCREASE_PRIORITY" { param_int(c, 1) } else { -param_int(c, 1) };
                }
            }
            "PLAYER_ENABLE_BASE_CONSTRUCTION"
            | "PLAYER_ENABLE_UNIT_CONSTRUCTION"
            | "PLAYER_DISABLE_BASE_CONSTRUCTION"
            | "PLAYER_DISABLE_UNIT_CONSTRUCTION" => {
                if let Some(player) = e.player_named(param_str(c, 0))
                    && let Some(base) = e.bases.get_mut(&player)
                {
                    if name_of(c).contains("BASE") {
                        base.enabled = name_of(c).contains("ENABLE");
                    } else {
                        base.units = name_of(c).contains("ENABLE");
                    }
                }
            }
            "SKIRMISH_BUILD_BUILDING" => {
                if let Some(player) = e.current.map(|i| e.sides[i].player)
                    && let Some(base) = e.bases.get_mut(&player)
                    && let Some(building) = base.buildings.iter_mut().find(|b| {
                        b.template == param_str(c, 0) && b.object.is_none() && !b.priority && b.remaining != 0
                    })
                {
                    building.priority = true;
                    base.next_frame = self.frame;
                }
            }
            "OBJECTLIST_ADDOBJECTTYPE" => {
                e.object_lists.entry(param_str(c, 0).to_string()).or_default().insert(param_str(c, 1).to_string());
            }
            "OBJECTLIST_REMOVEOBJECTTYPE" => {
                if let Some(list) = e.object_lists.get_mut(param_str(c, 0)) {
                    list.remove(param_str(c, 1));
                }
                e.object_lists.retain(|_, list| !list.is_empty());
            }
            "PLAYER_GIVE_MONEY" | "PLAYER_SET_MONEY" => {
                if let Some(player) = e.player_named(param_str(c, 0)) {
                    let amount = param_int(c, 1).max(0) as u32;
                    if name_of(c) == "PLAYER_SET_MONEY" {
                        self.player(player).money = amount;
                    } else {
                        self.player(player).deposit(amount);
                    }
                }
            }
            "PLAYER_SELL_EVERYTHING" => {
                if let Some(player) = e.player_named(param_str(c, 0)) {
                    let ids: Vec<_> =
                        self.objects.values().filter(|o| o.owner == player && !o.dead).map(|o| o.id).collect();
                    for id in ids {
                        let _ = self.sell(id);
                    }
                }
            }
            _ => return false,
        }
        true
    }
}
