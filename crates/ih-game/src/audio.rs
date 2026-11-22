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

pub fn listener(camera: &TacticalCamera, map: &Map) -> Listener {
    let (s, c) = camera.yaw.sin_cos();
    Listener {
        position: [camera.center.x, camera.center.y, map.ground_height(camera.center.x, camera.center.y) + 50.0],
        right: [c, s],
    }
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
