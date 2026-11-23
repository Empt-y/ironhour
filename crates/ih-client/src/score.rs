//! Client match statistics. Initial armies are the baseline, not units produced.
use ih_sim::Simulation;
use std::collections::{BTreeMap, BTreeSet};

#[derive(Clone, Debug, Default)]
pub struct Totals {
    pub units_built: u32,
    pub buildings_built: u32,
    pub units_lost: u32,
    pub buildings_lost: u32,
    pub units_destroyed: u32,
    pub buildings_destroyed: u32,
    pub supplies: u32,
}
#[derive(Default)]
pub struct Scores {
    pub players: BTreeMap<u32, Totals>,
    pub names: BTreeMap<u32, String>,
    observed: BTreeMap<u32, (u32, bool, bool)>,
    initial: bool,
    frame: Option<u32>,
    deaths: BTreeSet<u32>,
}
impl Scores {
    pub fn observe(&mut self, sim: &Simulation) {
        if self.frame == Some(sim.frame) {
            return;
        }
        self.frame = Some(sim.frame);
        let mut live = BTreeSet::new();
        for o in sim.objects.values() {
            let t = &sim.templates.objects[&o.template];
            if o.map_index.is_some()
                || t.has_kind("PROJECTILE")
                || !(t.has_kind("STRUCTURE")
                    || t.has_kind("VEHICLE")
                    || t.has_kind("INFANTRY")
                    || t.has_kind("AIRCRAFT"))
            {
                continue;
            }
            live.insert(o.id);
            let building = t.has_kind("STRUCTURE");
            let complete = !o.dead && o.construction.is_none();
            let old = self.observed.insert(o.id, (o.owner, building, complete));
            let stats = self.players.entry(o.owner).or_default();
            if complete && old.is_none_or(|previous| !previous.2) && self.initial {
                if building { stats.buildings_built += 1 } else { stats.units_built += 1 }
            }
        }
        for notice in &sim.notices {
            match notice {
                ih_sim::notices::Notice::SuppliesDeposited { player, amount } => {
                    self.players.entry(*player).or_default().supplies += amount;
                }
                ih_sim::notices::Notice::ObjectDied { object, player, killer, building, sold }
                    if !sold && self.players.contains_key(player) && self.deaths.insert(*object) =>
                {
                    let stats = self.players.entry(*player).or_default();
                    if *building {
                        stats.buildings_lost += 1
                    } else {
                        stats.units_lost += 1
                    }
                    if let Some(killer) = killer.filter(|p| p != player && *player != 0) {
                        let stats = self.players.entry(killer).or_default();
                        if *building { stats.buildings_destroyed += 1 } else { stats.units_destroyed += 1 }
                    }
                }
                _ => {}
            }
        }
        self.observed.retain(|id, _| live.contains(id));
        self.initial = true;
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn records_removed_deaths_once_and_excludes_starting_armies_from_build_counts() {
        let mut defs = ih_ini::definitions::Definitions::default();
        defs.add_document(ih_ini::parse(b"Object Soldier\n KindOf = INFANTRY\n Body = ActiveBody Body\n MaxHealth = 100\n InitialHealth = 100\n End\n Behavior = DestroyDie Death\n End\nEnd\n"));
        let map = ih_formats::map::Map { width: 32, height: 32, heights: vec![0; 1024], ..Default::default() };
        let mut sim = Simulation::new(
            std::sync::Arc::new(map),
            std::sync::Arc::new(ih_sim::templates::Templates::from_definitions(&defs)),
            1,
        );
        let a = sim.spawn("Soldier", [100.0, 100.0], 1).unwrap();
        let b = sim.spawn("Soldier", [150.0, 150.0], 2).unwrap();
        let mut scores = Scores::default();
        scores.observe(&sim);
        assert_eq!(scores.players[&1].units_built, 0);
        sim.enqueue(
            0,
            ih_sim::Command::Damage {
                object: b,
                source: a,
                amount: 200.0,
                damage_type: "EXPLOSION".into(),
                death_type: "NORMAL".into(),
            },
        )
        .unwrap();
        sim.step();
        assert!(!sim.objects.contains_key(&b));
        scores.observe(&sim);
        scores.observe(&sim);
        assert_eq!(scores.players[&2].units_lost, 1);
        assert_eq!(scores.players[&1].units_destroyed, 1);
        sim.spawn("Soldier", [200.0, 200.0], 1).unwrap();
        sim.step();
        sim.notices.push(ih_sim::notices::Notice::SuppliesDeposited { player: 1, amount: 150 });
        scores.observe(&sim);
        assert_eq!(scores.players[&1].units_built, 1);
        assert_eq!(scores.players[&1].supplies, 150);
        let unfinished = sim.spawn("Soldier", [250.0, 250.0], 1).unwrap();
        sim.objects.get_mut(&unfinished).unwrap().construction = Some(25.0);
        sim.step();
        scores.observe(&sim);
        assert_eq!(scores.players[&1].units_built, 1);
        sim.objects.get_mut(&unfinished).unwrap().construction = None;
        sim.step();
        scores.observe(&sim);
        assert_eq!(scores.players[&1].units_built, 2);
    }
}
