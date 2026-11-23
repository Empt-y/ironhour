//! Command bar data and order translation, independent of the window backend.
//! Uses the original CommandSet/CommandButton and localized CommandMap definitions.
use ih_ini::definitions::Definitions;
use ih_sim::{Command, Simulation};
use std::collections::BTreeMap;

#[derive(Clone, Debug, Default)]
pub struct Button {
    pub name: String,
    pub command: String,
    pub object: String,
    pub upgrade: String,
    pub power: String,
    pub science: Vec<String>,
    pub options: Vec<String>,
    pub label: String,
    pub description: String,
    pub image: String,
    pub cursor: String,
}
impl Button {
    pub fn targeted(&self) -> bool {
        matches!(self.command.as_str(), "DOZER_CONSTRUCT" | "DOZER_REPAIR" | "SET_RALLY_POINT" | "GUARD" | "ENTER")
            || self.options.iter().any(|v| {
                matches!(
                    v.as_str(),
                    "NEED_TARGET_POS"
                        | "NEED_TARGET_ENEMY_OBJECT"
                        | "NEED_TARGET_ALLY_OBJECT"
                        | "NEED_TARGET_NEUTRAL_OBJECT"
                )
            })
    }
}
#[derive(Clone, Debug)]
pub struct Binding {
    pub action: String,
    pub key: String,
    pub modifiers: String,
    pub down: bool,
}
#[derive(Default)]
pub struct Controls {
    pub buttons: BTreeMap<String, Button>,
    pub sets: BTreeMap<String, Vec<Option<String>>>,
    pub bindings: Vec<Binding>,
}
impl Controls {
    pub fn load(defs: &Definitions) -> Self {
        let mut out = Self::default();
        for b in defs.ordered("CommandButton") {
            let Some(name) = b.args().first().cloned() else { continue };
            let value = |key: &str| {
                b.fields().filter(|f| f.name.eq_ignore_ascii_case(key)).last().map(|f| f.values()).unwrap_or_default()
            };
            let first = |key: &str| value(key).first().cloned().unwrap_or_default();
            out.buttons.insert(
                name.clone(),
                Button {
                    name,
                    command: first("Command"),
                    object: first("Object"),
                    upgrade: first("Upgrade"),
                    power: first("SpecialPower"),
                    science: value("Science"),
                    options: value("Options"),
                    label: first("TextLabel"),
                    description: first("DescriptLabel"),
                    image: first("ButtonImage"),
                    cursor: first("CursorName"),
                },
            );
        }
        for b in defs.ordered("CommandSet") {
            let Some(name) = b.args().first().cloned() else { continue };
            let mut slots = vec![None; 14];
            for f in b.fields() {
                if let Ok(i @ 1..=14) = f.name.parse::<usize>() {
                    slots[i - 1] = f.values().first().cloned().filter(|n| !n.eq_ignore_ascii_case("None"));
                }
            }
            out.sets.insert(name, slots);
        }
        for b in defs.ordered("CommandMap") {
            let Some(action) = b.args().first().cloned() else { continue };
            let first = |key: &str| {
                b.fields()
                    .filter(|f| f.name == key)
                    .last()
                    .and_then(|f| f.values().first().cloned())
                    .unwrap_or_default()
            };
            out.bindings.retain(|entry| entry.action != action);
            out.bindings.push(Binding {
                action,
                key: first("Key"),
                modifiers: first("Modifiers"),
                down: first("Transition") != "UP",
            });
        }
        out
    }
    pub fn hotkey(&self, key: &str, shift: bool, ctrl: bool, alt: bool, down: bool) -> Option<&str> {
        self.bindings
            .iter()
            .rev()
            .find(|b| {
                b.key.eq_ignore_ascii_case(key)
                    && b.down == down
                    && b.modifiers.contains("SHIFT") == shift
                    && b.modifiers.contains("CTRL") == ctrl
                    && b.modifiers.contains("ALT") == alt
            })
            .map(|b| b.action.as_str())
    }
    pub fn selection(&self, sim: &Simulation, selected: &[u32], player: u32) -> Vec<Option<String>> {
        let mut sets =
            selected.iter().filter_map(|id| sim.objects.get(id)).filter(|o| o.owner == player && !o.dead).map(|o| {
                let t = &sim.templates.objects[&o.template];
                o.upgrades
                    .command_set
                    .as_ref()
                    .or_else(|| t.values("CommandSet").first())
                    .and_then(|n| self.sets.get(n))
                    .cloned()
                    .unwrap_or_else(|| vec![None; 14])
            });
        let Some(mut common) = sets.next() else { return vec![None; 14] };
        for other in sets {
            for (a, b) in common.iter_mut().zip(other) {
                if *a != b {
                    *a = None;
                }
            }
        }
        common
    }
    pub fn supported(button: &Button) -> bool {
        matches!(
            button.command.as_str(),
            "UNIT_BUILD"
                | "DOZER_CONSTRUCT"
                | "PLAYER_UPGRADE"
                | "OBJECT_UPGRADE"
                | "SPECIAL_POWER"
                | "SPECIAL_POWER_FROM_SHORTCUT"
                | "PURCHASE_SCIENCE"
                | "STOP"
                | "SELL"
                | "EVACUATE"
                | "EXIT_CONTAINER"
                | "SET_RALLY_POINT"
                | "DOZER_REPAIR"
                | "GUARD"
                | "ENTER"
        )
    }
    pub fn available(&self, b: &Button, sim: &Simulation, selected: &[u32], player: u32) -> bool {
        if !Self::supported(b) || b.options.iter().any(|s| s == "SCRIPT_ONLY") {
            return false;
        }
        if b.command == "PURCHASE_SCIENCE" {
            return b.science.first().and_then(|s| sim.templates.sciences.get(s).map(|info| (s, info))).is_some_and(
                |(s, info)| {
                    sim.players.get(&player).is_some_and(|p| {
                        !p.sciences.contains(s)
                            && p.generals.purchase_points >= info.cost
                            && info.prereqs.iter().all(|s| s.eq_ignore_ascii_case("None") || p.sciences.contains(s))
                    })
                },
            );
        }
        let Some(o) = selected.iter().filter_map(|id| sim.objects.get(id)).find(|o| o.owner == player && !o.dead)
        else {
            return false;
        };
        if o.disabled.any() {
            return false;
        }
        match b.command.as_str() {
            "UNIT_BUILD" | "DOZER_CONSTRUCT" => sim.can_make_unit(o.id, &b.object) == ih_sim::production::CanMake::Ok,
            "PLAYER_UPGRADE" | "OBJECT_UPGRADE" => sim.templates.upgrades.get(&b.upgrade).is_some_and(|u| {
                sim.money(player) >= u.cost_to_build()
                    && !o.upgrades.completed.contains(&b.upgrade)
                    && !sim.players.get(&player).is_some_and(|p| {
                        p.upgrades.completed.contains(&b.upgrade) || p.upgrades.in_progress.contains(&b.upgrade)
                    })
            }),
            "SPECIAL_POWER" | "SPECIAL_POWER_FROM_SHORTCUT" => {
                o.powers
                    .iter()
                    .find(|p| p.template == b.power)
                    .is_some_and(|p| p.available_on <= sim.frame && p.paused_count == 0 && !p.script_only)
                    && sim
                        .templates
                        .special_powers
                        .get(&b.power)
                        .and_then(|p| p.required_science.as_ref())
                        .is_none_or(|s| sim.players.get(&player).is_some_and(|p| p.sciences.contains(s)))
            }
            "SELL" => sim.templates.objects[&o.template].has_kind("STRUCTURE"),
            _ => true,
        }
    }

    #[allow(clippy::too_many_arguments)]
    pub fn orders(
        &self,
        b: &Button,
        sim: &Simulation,
        selected: &[u32],
        player: u32,
        point: Option<[f32; 2]>,
        target: Option<u32>,
        angle: f32,
    ) -> Vec<Command> {
        let objects: Vec<_> = selected
            .iter()
            .copied()
            .filter(|id| sim.objects.get(id).is_some_and(|o| o.owner == player && !o.dead))
            .collect();
        if b.command == "PURCHASE_SCIENCE" {
            return b
                .science
                .first()
                .map(|s| vec![Command::PurchaseScience { player, science: s.clone() }])
                .unwrap_or_default();
        }
        objects
            .into_iter()
            .take(
                if matches!(
                    b.command.as_str(),
                    "UNIT_BUILD"
                        | "DOZER_CONSTRUCT"
                        | "PLAYER_UPGRADE"
                        | "OBJECT_UPGRADE"
                        | "SPECIAL_POWER"
                        | "SPECIAL_POWER_FROM_SHORTCUT"
                ) {
                    1
                } else {
                    usize::MAX
                },
            )
            .filter_map(|object| {
                Some(match b.command.as_str() {
                    "UNIT_BUILD" => Command::Produce { factory: object, template: b.object.clone() },
                    "DOZER_CONSTRUCT" => {
                        Command::Construct { dozer: object, template: b.object.clone(), position: point?, angle }
                    }
                    "PLAYER_UPGRADE" | "OBJECT_UPGRADE" => {
                        Command::Research { factory: object, upgrade: b.upgrade.clone() }
                    }
                    "SPECIAL_POWER" | "SPECIAL_POWER_FROM_SHORTCUT" => {
                        Command::SpecialPower { object, power: b.power.clone(), target, location: point }
                    }
                    "STOP" => Command::Stop { object },
                    "SELL" => Command::Sell { object },
                    "EVACUATE" => Command::Evacuate { object },
                    "EXIT_CONTAINER" => Command::Exit { object },
                    "SET_RALLY_POINT" => Command::SetRallyPoint { object, position: point? },
                    "DOZER_REPAIR" => Command::Repair { dozer: object, target: target? },
                    "ENTER" => Command::Enter { object, target: target? },
                    "GUARD" => Command::Guard { object, position: point? },
                    "ATTACK_MOVE" => {
                        if let Some(target) = target {
                            Command::Attack { object, target }
                        } else {
                            Command::Move { object, target: point? }
                        }
                    }
                    _ => return None,
                })
            })
            .collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn orders_keep_simulation_authority_and_never_command_enemy_selections() {
        let mut defs = Definitions::default();
        defs.add_document(ih_ini::parse(b"Object Factory\n KindOf = STRUCTURE IMMOBILE\n CommandSet = FactorySet\nEnd\nCommandButton Build\n Command = UNIT_BUILD\n Object = Soldier\nEnd\nCommandSet FactorySet\n 1 = Build\nEnd\n"));
        let map = ih_formats::map::Map { width: 32, height: 32, heights: vec![0; 1024], ..Default::default() };
        let mut sim = Simulation::new(
            std::sync::Arc::new(map),
            std::sync::Arc::new(ih_sim::templates::Templates::from_definitions(&defs)),
            1,
        );
        let own = sim.spawn("Factory", [100.0, 100.0], 1).unwrap();
        let enemy = sim.spawn("Factory", [200.0, 200.0], 2).unwrap();
        let c = Controls::load(&defs);
        assert_eq!(c.selection(&sim, &[own], 1)[0].as_deref(), Some("Build"));
        assert!(c.selection(&sim, &[enemy], 1).iter().all(Option::is_none));
        let orders = c.orders(&c.buttons["Build"], &sim, &[enemy, own], 1, None, None, 0.0);
        assert!(matches!(&orders[..],[Command::Produce{factory,template}] if *factory==own&&template=="Soldier"));
    }

    #[test]
    fn definitions_override_bindings_and_keep_command_slot_order() {
        let mut defs = Definitions::default();
        defs.add_document(ih_ini::parse(b"CommandButton Build\n Command = DOZER_CONSTRUCT\n Object = Power\nEnd\nCommandSet Dozer\n 2 = Build\nEnd\nCommandMap STOP\n Key = KEY_S\n Modifiers = NONE\nEnd\nCommandMap STOP\n Key = KEY_X\n Modifiers = CTRL\nEnd\n"));
        let c = Controls::load(&defs);
        assert!(c.buttons["Build"].targeted());
        assert_eq!(c.sets["Dozer"][1].as_deref(), Some("Build"));
        assert_eq!(c.hotkey("KEY_X", false, true, false, true), Some("STOP"));
        assert!(c.hotkey("KEY_S", false, false, false, true).is_none());
    }
}
