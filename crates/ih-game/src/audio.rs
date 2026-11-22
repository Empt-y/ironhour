//! Dispatch immediately after each tick: the next tick clears FX events.
use ih_audio::{Audio, Listener};
use ih_client::TacticalCamera;
use ih_formats::map::Map;
use ih_sim::{Simulation, fx::FxEvent};
use ih_vfs::Vfs;
use std::{collections::BTreeMap, time::Duration};

#[derive(Default)]
pub struct Feedback {
    previous: BTreeMap<u32, (f32, bool)>,
    rank: Option<i32>,
    side: Option<String>,
    emitters: BTreeMap<u64, (String, u32)>,
    seen: BTreeMap<u32, u32>,
    moving: std::collections::BTreeSet<u32>,
    powers: BTreeMap<(u32, String), bool>,
}

fn superweapon(name: &str) -> Option<&'static str> {
    let name = name.to_ascii_lowercase();
    if name.contains("particle") {
        Some("ParticleCannon")
    } else if name.contains("nuclearmissile") || name.contains("neutronmissile") || name.contains("nukemissile") {
        Some("Nuke")
    } else if name.contains("scudstorm") {
        Some("ScudStorm")
    } else if name.contains("gpsscrambler") {
        Some("GPS_Scrambler")
    } else if name.contains("sneakattack") {
        Some("Sneak_Attack")
    } else {
        None
    }
}

fn side(sim: &Simulation, player: u32) -> Option<&str> {
    sim.objects
        .values()
        .filter(|o| o.owner == player && !o.dead)
        .find_map(|o| sim.templates.objects.get(&o.template)?.values("Side").first().map(String::as_str))
}

pub fn start_music(audio: &mut Option<Audio>, vfs: &Vfs, sim: Option<&Simulation>, player: Option<u32>, shell: bool) {
    let Some(audio) = audio else { return };
    let result = if shell {
        audio.play_music(vfs, "Shell")
    } else {
        let faction = sim.zip(player).and_then(|(sim, p)| side(sim, p)).unwrap_or("").to_ascii_lowercase();
        let prefix = if faction.starts_with("america") {
            "Game_USA_"
        } else if faction.starts_with("china") {
            "Game_CHI_"
        } else if faction.starts_with("gla") {
            "Game_GLA_"
        } else {
            audio.stop_music();
            return;
        };
        let tracks = audio.catalog.music.keys().filter(|n| n.starts_with(prefix)).cloned().collect();
        audio.play_playlist(vfs, tracks)
    };
    if let Err(e) = result {
        eprintln!("music: {e:#}");
    }
}

pub fn unit_voice(
    audio: &mut Option<Audio>,
    sim: &Simulation,
    selected: &[u32],
    player: Option<u32>,
    field: &str,
    now: Duration,
) {
    let Some(audio) = audio else { return };
    let voice = selected
        .iter()
        .filter_map(|id| sim.objects.get(id))
        .filter(|o| !o.dead && Some(o.owner) == player)
        .find_map(|o| sim.templates.objects.get(&o.template)?.values(field).first());
    if let Some(name) = voice.filter(|n| !n.eq_ignore_ascii_case("None") && !n.eq_ignore_ascii_case("NoSound")) {
        audio.queue_speech(name, 1, now, Duration::from_secs(2));
    }
}

/// Initial EVA sources. Snapshots stay in the client and never change game state.
pub fn feedback(
    audio: &mut Option<Audio>,
    sim: &Simulation,
    vfs: &Vfs,
    previous: &mut Feedback,
    player: Option<u32>,
    now: Duration,
) {
    let Some(audio) = audio else { return };
    let Some(player) = player else { return };
    if let Some(side) = side(sim, player) {
        previous.side = Some(side.into());
    }
    let Some(side) = previous.side.as_deref() else { return };
    let current: BTreeMap<u32, (f32, bool)> = sim
        .objects
        .values()
        .filter(|o| o.owner == player && !o.dead)
        .filter_map(|o| {
            Some((
                o.id,
                (
                    o.health.as_ref()?.current,
                    sim.templates.objects[&o.template].values("KindOf").iter().any(|k| k == "STRUCTURE"),
                ),
            ))
        })
        .collect();
    let mut live_emitters = std::collections::BTreeSet::new();
    let mut moving_now = std::collections::BTreeSet::new();
    let mut desired = vec![];
    for o in sim.objects.values() {
        let template = &sim.templates.objects[&o.template];
        let state = o.health.as_ref().map(|h| h.state);
        let key = u64::from(o.id) << 1;
        if !o.dead && o.speed.abs() > 0.01 {
            moving_now.insert(o.id);
            let damaged = state == Some(ih_sim::body::DamageState::ReallyDamaged);
            let start = template.values(if damaged { "SoundMoveStartDamaged" } else { "SoundMoveStart" }).first();
            let repeat = template.values(if damaged { "SoundMoveLoopDamaged" } else { "SoundMoveLoop" }).first();
            let first_move = !previous.moving.contains(&o.id);
            if let Some(name) = start.filter(|n| !n.eq_ignore_ascii_case("None") && !n.eq_ignore_ascii_case("NoSound"))
            {
                if first_move || audio.has_emitter(key + 1) {
                    desired.push((key + 1, name, o.position));
                }
            } else if let Some(name) = repeat {
                desired.push((key + 1, name, o.position));
            }
        }
        let field = if o.dead {
            "SoundAmbientRubble"
        } else {
            match state {
                Some(ih_sim::body::DamageState::ReallyDamaged) => "SoundAmbientReallyDamaged",
                Some(ih_sim::body::DamageState::Damaged) => "SoundAmbientDamaged",
                _ => "SoundAmbient",
            }
        };
        if let Some(name) = template
            .values(field)
            .first()
            .or_else(|| if !o.dead { template.values("SoundAmbient").first() } else { None })
        {
            desired.push((key, name, o.position));
        }
    }
    previous.moving = moving_now;
    for (id, name, position) in desired {
        if name.eq_ignore_ascii_case("None") || name.eq_ignore_ascii_case("NoSound") {
            continue;
        }
        live_emitters.insert(id);
        let entry = previous.emitters.entry(id).or_insert((name.clone(), 0));
        if entry.0 != *name {
            entry.0 = name.clone();
            entry.1 = 0;
        }
        if sim.frame >= entry.1 || audio.has_emitter(id) {
            if let Err(e) = audio.emitter(vfs, id, name, position) {
                eprintln!("emitter: {e:#}");
            }
            entry.1 = sim.frame.saturating_add(30);
        }
    }
    for id in previous.emitters.keys().filter(|id| !live_emitters.contains(id)).copied().collect::<Vec<_>>() {
        if let Err(e) = audio.stop_emitter(vfs, id, true) {
            eprintln!("ambient stop: {e:#}");
        }
        previous.emitters.remove(&id);
    }
    audio.set_logic_time(Duration::from_secs_f64(sim.frame as f64 / 30.0));
    if sim.frame >= 2 {
        for notice in &sim.notices {
            use ih_sim::notices::Notice;
            let message = match notice {
                Notice::InsufficientFunds { player: owner } if *owner == player => {
                    Some("InsufficientFunds".to_string())
                }
                Notice::UpgradeComplete { player: owner } if *owner == player => Some("UpgradeComplete".to_string()),
                Notice::CashStolen { player: owner } if *owner == player => Some("CashStolen".to_string()),
                Notice::PowerUsed { player: owner, power } => superweapon(power).map(|kind| {
                    format!("SuperweaponLaunched_{}_{kind}", if *owner == player { "Own" } else { "Enemy" })
                }),
                _ => None,
            };
            if let Some(message) = message {
                audio.queue_eva(&message, side, now);
            }
        }
        for o in sim.objects.values().filter(|o| !o.dead) {
            let visible = o.owner == player
                || (sim.shrouded_status(o.id, player) == ih_sim::shroud::Visibility::Clear
                    && !sim.stealthed_and_undetected(o.id));
            if !visible {
                continue;
            }
            let template = &sim.templates.objects[&o.template];
            if let Some(old) = previous.seen.insert(o.id, o.owner) {
                if old == player && o.owner != player {
                    audio.queue_eva(
                        if template.has_kind("STRUCTURE") { "BuildingStolen" } else { "VehicleStolen" },
                        side,
                        now,
                    );
                }
            } else {
                let relation = if o.owner == player { "Own" } else { "Enemy" };
                if template.has_kind("STRUCTURE")
                    && let Some(kind) = superweapon(&o.template)
                {
                    audio.queue_eva(&format!("SuperweaponDetected_{relation}_{kind}"), side, now);
                }
                for (needle, hero) in
                    [("blacklotus", "BlackLotus"), ("jarmenkell", "JarmenKell"), ("colonelburton", "ColonelBurton")]
                {
                    if o.template.to_ascii_lowercase().contains(needle) {
                        audio.queue_eva(&format!("{relation}{hero}Detected"), side, now);
                    }
                }
            }
            for power in &o.powers {
                let ready = power.available_on <= sim.frame && power.paused_count == 0 && !o.disabled.any();
                let old = previous.powers.insert((o.id, power.template.clone()), ready);
                if ready
                    && old != Some(true)
                    && let Some(kind) = superweapon(&power.template)
                {
                    audio.queue_eva(
                        &format!("SuperweaponReady_{}_{kind}", if o.owner == player { "Own" } else { "Enemy" }),
                        side,
                        now,
                    );
                }
            }
        }
    }
    if sim.frame >= 2 {
        let (supply, demand) = sim.energy(player);
        if demand > supply {
            audio.queue_eva("LowPower", side, now);
        }
        for (id, (health, structure)) in &previous.previous {
            if !current.contains_key(id) {
                audio.queue_eva(if *structure { "BuildingLost" } else { "UnitLost" }, side, now);
            } else if *structure && current[id].0 < *health {
                audio.queue_eva("BaseUnderAttack", side, now);
            }
        }
        if let Some(rank) = sim.players.get(&player).map(|p| p.generals.rank) {
            if previous.rank.is_some_and(|old| rank > old) {
                audio.queue_eva("GeneralLevelUp", side, now);
            }
            previous.rank = Some(rank);
        }
    }
    previous.previous = current;
}

pub fn listener(camera: &TacticalCamera, map: &Map, catalog: &ih_audio::Catalog) -> Listener {
    let (s, c) = camera.yaw.sin_cos();
    let view = camera.camera(map);
    catalog.camera_listener(view.target.to_array(), view.eye.to_array(), [c, s])
}

fn permitted(event: &FxEvent, local_player: Option<u32>) -> bool {
    // Team relationships are not implemented yet. Private FX are audible only
    // to their owner, never to an opponent or an unspecified observer.
    event.allies_only.is_none() || event.allies_only == local_player
}

pub fn dispatch(audio: &mut Option<Audio>, sim: &Simulation, vfs: &Vfs, local_player: Option<u32>) {
    let Some(audio) = audio else { return };
    for event in &sim.fx_events {
        if permitted(event, local_player) {
            for error in audio.play_fx(vfs, &event.fx, event.position) {
                eprintln!("FX sound: {error}");
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn private_fx_never_leak_to_an_enemy_or_observer() {
        let mut event =
            FxEvent { fx: "Fire".into(), position: [0.0; 3], object: None, target: None, allies_only: Some(4) };
        assert!(permitted(&event, Some(4)));
        assert!(!permitted(&event, Some(5)));
        assert!(!permitted(&event, None));
        event.allies_only = None;
        assert!(permitted(&event, None));
    }
}
