//! Dispatch immediately after each tick: the next tick clears FX events.
use ih_audio::{Audio, Listener};
use ih_client::TacticalCamera;
use ih_formats::map::Map;
use ih_sim::{Simulation, fx::FxEvent};
use ih_vfs::Vfs;

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
