//! World queries and actions from ScriptConditions.cpp / ScriptActions.cpp.
use super::*;

impl Simulation {
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
