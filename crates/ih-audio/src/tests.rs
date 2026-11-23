use super::*;
use kira::backend::mock::{MockBackend, MockBackendSettings};

pub(crate) fn definitions(extra: &str) -> Definitions {
    let mut d = Definitions::default();
    d.add_document(ih_ini::parse(
        format!(
            r#"
AudioSettings
  AudioRoot = Data\Audio
  SoundsFolder = Sounds
  SoundsExtension = wav
  DefaultSoundVolume = 80%
  Relative2DVolume = -10%
  SampleCount2D = 2
End
AudioEvent DefaultSoundEffect
  Volume = 100
  Type = ui player
  Control = random
  Limit = 1
  PitchShift = 0 0
End
AudioEvent Click
  Sounds = click
  Volume = 30
End
{extra}
"#
        )
        .as_bytes(),
    ));
    d
}

fn wav() -> Vec<u8> {
    let frames = 4800u32;
    let mut b = b"RIFF".to_vec();
    b.extend((36 + frames * 2).to_le_bytes());
    b.extend(b"WAVEfmt ");
    b.extend(16u32.to_le_bytes());
    b.extend(1u16.to_le_bytes()); // PCM
    b.extend(1u16.to_le_bytes()); // mono
    b.extend(48000u32.to_le_bytes());
    b.extend(96000u32.to_le_bytes());
    b.extend(2u16.to_le_bytes());
    b.extend(16u16.to_le_bytes());
    b.extend(b"data");
    b.extend((frames * 2).to_le_bytes());
    for _ in 0..frames {
        b.extend(16000i16.to_le_bytes());
    }
    b
}

pub(crate) fn files(localized: bool) -> (tempfile::TempDir, Vfs) {
    let temp = tempfile::tempdir().unwrap();
    let dir = temp.path().join("Data/Audio/Sounds");
    std::fs::create_dir_all(&dir).unwrap();
    std::fs::write(dir.join("click.wav"), wav()).unwrap();
    if localized {
        std::fs::create_dir_all(dir.join("English")).unwrap();
        std::fs::write(dir.join("English/click.wav"), wav()).unwrap();
    }
    let mut vfs = Vfs::new();
    vfs.set_loose_root(temp.path()).unwrap();
    (temp, vfs)
}

#[test]
fn defaults_and_later_overrides_follow_original_parser() {
    let mut d = definitions("");
    d.add_document(ih_ini::parse(b"AudioEvent Click\n Sounds = replacement\nEnd\n"));
    let c = Catalog::load(&d, "English");
    let e = &c.events["Click"];
    assert_eq!(e.sounds, ["replacement"]);
    assert_eq!(e.volume, 1.0); // Replacement starts from defaults, not old Volume=30.
    assert!(e.ui);
    assert_eq!(e.limit, 1);
    assert_eq!(c.sound_volume, 0.8);
    assert_eq!(c.relative_2d, 0.9);
    assert_eq!(c.max_ui_voices, 2);
}

#[test]
fn localized_clip_wins_and_generic_is_the_fallback() {
    let (_temp, vfs) = files(true);
    let d = definitions("");
    let english = Catalog::load(&d, "English");
    assert_eq!(english.resolve(&vfs, "click").unwrap(), "Data\\Audio\\Sounds\\English\\click.wav");
    let french = Catalog::load(&d, "French");
    assert_eq!(french.resolve(&vfs, "click").unwrap(), "Data\\Audio\\Sounds\\click.wav");
    assert!(english.resolve(&vfs, "missing").is_err());
}

#[test]
fn pcm_decodes_from_the_vfs_and_corrupt_audio_reports_an_error() {
    let (_temp, vfs) = files(false);
    let data = decode(&vfs, "Data\\Audio\\Sounds\\click.wav").unwrap();
    assert_eq!(data.sample_rate, 48000);
    assert_eq!(data.frames.len(), 4800);
    assert!((data.duration().as_secs_f64() - 0.1).abs() < 0.00001);
    assert!(data.frames[200].left > 0.4);
    assert!(StaticSoundData::from_cursor(Cursor::new(b"bad file")).is_err());
}

#[test]
fn ui_voice_limits_release_after_playback_without_an_output_device() {
    let (_temp, vfs) = files(false);
    let d = definitions(
        "AudioEvent Other\n Sounds = click\n Limit = 0\nEnd\nAudioEvent World\n Type = world\n Sounds = click\nEnd\n",
    );
    let manager = AudioManager::<MockBackend>::new(AudioManagerSettings {
        backend_settings: MockBackendSettings { sample_rate: 48000 },
        ..Default::default()
    })
    .unwrap();
    let mut audio = Audio::with_manager(Catalog::load(&d, "English"), manager);
    assert!(audio.play_ui(&vfs, "Click").unwrap());
    assert!(!audio.play_ui(&vfs, "Click").unwrap()); // Per-event limit.
    assert!(audio.play_ui(&vfs, "Other").unwrap());
    assert!(!audio.play_ui(&vfs, "Other").unwrap()); // Total UI budget.
    assert!(audio.play_ui(&vfs, "World").is_err());
    assert!(audio.play_ui(&vfs, "Missing").is_err());
    let sound = &audio.cache["Data\\Audio\\Sounds\\click.wav"];
    assert_eq!(sound.frames.len(), 4800);
    // Process actual mixer buffers to complete both sounds; no speaker is opened.
    for _ in 0..100 {
        audio.manager.backend_mut().on_start_processing();
        audio.manager.backend_mut().process();
    }
    assert!(audio.play_ui(&vfs, "Click").unwrap());
    assert_eq!(audio.cache.len(), 1);
}

#[test]
fn world_attenuation_pans_with_the_camera_and_cuts_off_at_max_range() {
    let listener = Listener::default();
    assert_eq!(listener.spatial([50.0, 0.0, 0.0], [100.0, 400.0]), (1.0, 1.0));
    assert_eq!(listener.spatial([-200.0, 0.0, 0.0], [100.0, 400.0]), (0.5, 0.0));
    assert_eq!(listener.spatial([400.0, 0.0, 0.0], [100.0, 400.0]).0, 0.0);
    let rotated = Listener { right: [0.0, 1.0], ..listener };
    assert_eq!(rotated.spatial([0.0, 200.0, 0.0], [100.0, 400.0]), (0.5, 1.0));
    assert_eq!(listener.spatial([0.0, 0.0, 200.0], [100.0, 400.0]), (0.5, 0.5));
}

#[test]
fn fx_sounds_use_world_budget_without_consuming_ui_voices() {
    let (_temp, vfs) = files(false);
    let d = definitions(
        "AudioSettings\n SampleCount3D = 1\nEnd\nAudioEvent Shot\n Type = world\n Sounds = click\n MinRange = 100\n MaxRange = 400\nEnd\nFXList Fire\n Sound\n Name = Shot\n End\n ParticleSystem\n Name = Smoke\n End\nEnd\n",
    );
    let manager = AudioManager::<MockBackend>::new(AudioManagerSettings::default()).unwrap();
    let mut audio = Audio::with_manager(Catalog::load(&d, "English"), manager);
    assert_eq!(audio.catalog.fx_sounds["Fire"], ["Shot"]);
    assert!(!audio.play_world(&vfs, "Shot", [500.0, 0.0, 0.0]).unwrap());
    assert!(audio.cache.is_empty(), "out-of-range sounds are culled before decoding");
    assert!(audio.play_fx(&vfs, "Fire", [50.0, 0.0, 0.0]).is_empty());
    assert_eq!(audio.active.len(), 1);
    assert!(!audio.play_world(&vfs, "Shot", [50.0, 0.0, 0.0]).unwrap());
    assert!(audio.play_ui(&vfs, "Click").unwrap());
    assert_eq!(audio.active.len(), 2);
}

fn finish_buffer(audio: &mut Audio<MockBackend>, vfs: &Vfs) {
    for _ in 0..100 {
        audio.manager.backend_mut().on_start_processing();
        audio.manager.backend_mut().process();
    }
    audio.advance_voices(vfs).unwrap();
}

#[test]
fn finite_and_infinite_emitters_follow_attack_body_decay_and_move() {
    let (_temp, vfs) = files(false);
    let d = definitions(
        "AudioEvent Engine\n Type = world\n Attack = click\n Sounds = click\n Decay = click\n Control = loop\n LoopCount = 2\nEnd\nAudioEvent Infinite\n Type = world\n Sounds = click\n Decay = click\n Control = loop\n LoopCount = 0\nEnd\n",
    );
    let manager = AudioManager::<MockBackend>::new(AudioManagerSettings::default()).unwrap();
    let mut a = Audio::with_manager(Catalog::load(&d, "English"), manager);
    a.emitter(&vfs, 7, "Engine", [0.0; 3]).unwrap();
    assert_eq!(a.active[0].phase, Phase::Attack);
    finish_buffer(&mut a, &vfs);
    assert_eq!(a.active[0].phase, Phase::Body);
    a.emitter(&vfs, 7, "Engine", [20.0, 0.0, 0.0]).unwrap();
    assert_eq!(a.active[0].spatial.unwrap().0[0], 20.0);
    finish_buffer(&mut a, &vfs);
    assert_eq!(a.active[0].remaining, 1);
    finish_buffer(&mut a, &vfs);
    assert_eq!(a.active[0].phase, Phase::Decay);
    finish_buffer(&mut a, &vfs);
    assert!(a.active.is_empty());
    a.emitter(&vfs, 8, "Infinite", [0.0; 3]).unwrap();
    for _ in 0..3 {
        finish_buffer(&mut a, &vfs);
        assert!(a.has_emitter(8));
    }
    a.stop_emitter(&vfs, 8, true).unwrap();
    assert_eq!(a.active[0].phase, Phase::Decay);
    finish_buffer(&mut a, &vfs);
    assert!(a.active.is_empty());
}

#[test]
fn higher_priority_steals_and_interrupt_replaces_the_oldest_matching_voice() {
    let (_temp, vfs) = files(false);
    let d = definitions(
        "AudioSettings\n SampleCount2D = 1\nEnd\nAudioEvent Quiet\n Sounds = click\n Priority = low\nEnd\nAudioEvent Important\n Sounds = click\n Priority = critical\n Control = interrupt\nEnd\n",
    );
    let manager = AudioManager::<MockBackend>::new(AudioManagerSettings::default()).unwrap();
    let mut a = Audio::with_manager(Catalog::load(&d, "English"), manager);
    assert!(a.play_ui(&vfs, "Quiet").unwrap());
    assert!(a.play_ui(&vfs, "Important").unwrap());
    assert_eq!(a.active[0].name, "Important");
    assert!(!a.play_ui(&vfs, "Quiet").unwrap());
    assert!(a.play_ui(&vfs, "Important").unwrap());
    assert_eq!(a.active.len(), 1);
}

#[test]
fn microphone_moves_towards_camera_and_zoom_gain_matches_settings() {
    let c = Catalog::load(&definitions(""), "English");
    let far = c.camera_listener([0.0; 3], [0.0, -300.0, 400.0], [1.0, 0.0]);
    assert_eq!(far.position, [0.0, -37.5, 50.0]);
    assert!((far.world_gain - 0.8).abs() < 0.00001);
    let near = c.camera_listener([0.0; 3], [0.0, -30.0, 40.0], [1.0, 0.0]);
    assert!(near.position[2] < 14.0 && near.world_gain == 1.0);
}

#[test]
fn map_registry_reload_releases_voices_and_does_not_reuse_old_samples() {
    let (_temp, vfs) = files(false);
    let manager = AudioManager::<MockBackend>::new(AudioManagerSettings::default()).unwrap();
    let mut audio = Audio::with_manager(Catalog::load(&definitions(""), "English"), manager);
    audio.play_ui(&vfs, "Click").unwrap();
    assert!(!audio.cache.is_empty());
    audio.reload_catalog(Catalog::load(&definitions(""), "English"), preferences::Preferences::default()).unwrap();
    assert!(audio.active.is_empty() && audio.cache.is_empty());
    assert!(audio.play_ui(&vfs, "Click").unwrap());
}

#[test]
fn a_clip_missing_from_the_game_data_plays_nothing_without_an_error() {
    // Retail Voice.ini names clips that were never shipped (irancl, vsupspe...).
    let (_temp, vfs) = files(false);
    let d = definitions("AudioEvent Gone\n Sounds = neverShipped\nEnd\n");
    let manager = AudioManager::<kira::backend::mock::MockBackend>::new(AudioManagerSettings::default()).unwrap();
    let mut audio = Audio::with_manager(Catalog::load(&d, "English"), manager);
    assert!(!audio.play_ui(&vfs, "Gone").unwrap(), "silent, not an error");
    assert!(audio.active.is_empty());
    assert_eq!(audio.missing.len(), 1);
    assert!(audio.missing.iter().next().unwrap().contains("neverShipped.wav"));
    // A real decode or lookup problem is still an error.
    assert!(audio.play_ui(&vfs, "NoSuchEvent").is_err());
}
