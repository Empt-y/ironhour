//! Client audio, following `INIAudioEventInfo.cpp`, `AudioEventRTS.cpp`
//! (`generateFilename`, localization and play information) and AudioSettings.
//!
//! Files come from the mounted VFS, never an extracted copy. Kira mixes decoded
//! Symphonia samples. This client-only slice does not consume simulation RNG.
//! Positional FX, unit speech, EVA and streaming music stay outside simulation.
//! Original looping attack/body/decay and full ambient emitters remain pending.
mod emitters;
pub mod music;
pub mod preferences;
pub mod speech;
#[cfg(test)]
mod tests;

use std::{collections::BTreeMap, io::Cursor, time::Duration};

use anyhow::{Context, Result, ensure};
use ih_ini::{Block, definitions::Definitions};
use ih_vfs::Vfs;
use kira::{
    AudioManager, AudioManagerSettings, Decibels, DefaultBackend,
    backend::Backend,
    sound::{
        PlaybackState,
        static_sound::{StaticSoundData, StaticSoundHandle},
    },
};

fn percent(s: &str) -> Option<f32> {
    s.trim_end_matches('%').parse::<f32>().ok().filter(|n| n.is_finite()).map(|n| n / 100.0)
}

#[derive(Clone, Debug)]
pub struct Event {
    pub sounds: Vec<String>,
    pub volume: f32,
    pub volume_shift: f32,
    pub pitch: [f64; 2],
    pub delay_ms: [u64; 2],
    pub limit: usize,
    pub ui: bool,
    pub range: [f32; 2],
    global: bool,
    random: bool,
    looping: bool,
    attack: Vec<String>,
    decay: Vec<String>,
    loop_count: u32,
    priority: u8,
    interrupt: bool,
    all: bool,
    post_delay: bool,
    variants: BTreeMap<String, Vec<String>>,
}

impl Default for Event {
    fn default() -> Self {
        Self {
            sounds: vec![],
            volume: 1.0,
            volume_shift: 0.0,
            pitch: [1.0; 2],
            delay_ms: [0; 2],
            limit: 0,
            ui: false,
            range: [175.0, 800.0],
            global: false,
            random: false,
            looping: false,
            attack: vec![],
            decay: vec![],
            loop_count: 1,
            priority: 2,
            interrupt: false,
            all: false,
            post_delay: false,
            variants: BTreeMap::new(),
        }
    }
}

impl Event {
    fn apply(&mut self, block: &Block) {
        for f in block.fields() {
            let values = f.values();
            let Some(first) = values.first() else { continue };
            match f.name.to_ascii_lowercase().as_str() {
                "sounds" => self.sounds = values,
                "soundsnight" | "soundsevening" | "soundsmorning" => {
                    self.variants.insert(f.name.to_ascii_lowercase(), values);
                }
                "attack" => self.attack = values,
                "decay" => self.decay = values,
                "loopcount" => self.loop_count = first.parse().unwrap_or(self.loop_count),
                "priority" => {
                    self.priority = match first.to_ascii_lowercase().as_str() {
                        "lowest" => 0,
                        "low" => 1,
                        "high" => 3,
                        "critical" => 4,
                        _ => 2,
                    }
                }
                "volume" => self.volume = percent(first).unwrap_or(self.volume).max(0.0),
                "volumeshift" => self.volume_shift = percent(first).unwrap_or(self.volume_shift),
                "pitchshift" => {
                    let p: Vec<_> = values.iter().filter_map(|s| percent(s)).collect();
                    if p.len() == 2 && p[0] > -1.0 && p[1] >= p[0] {
                        self.pitch = [1.0 + p[0] as f64, 1.0 + p[1] as f64];
                    }
                }
                "delay" => {
                    let d: Vec<_> = values.iter().filter_map(|s| s.parse::<u64>().ok()).collect();
                    if d.len() == 2 && d[1] >= d[0] {
                        self.delay_ms = [d[0], d[1]];
                    }
                }
                "limit" => self.limit = first.parse().unwrap_or(self.limit),
                "minrange" => self.range[0] = first.parse().unwrap_or(self.range[0]),
                "maxrange" => self.range[1] = first.parse().unwrap_or(self.range[1]),
                "type" => {
                    self.ui = values.iter().any(|s| s.eq_ignore_ascii_case("ui"));
                    self.global = values.iter().any(|s| s.eq_ignore_ascii_case("global"));
                }
                "control" => {
                    self.random = values.iter().any(|s| s.eq_ignore_ascii_case("random"));
                    self.looping = values.iter().any(|s| s.eq_ignore_ascii_case("loop"));
                    self.interrupt = values.iter().any(|s| s.eq_ignore_ascii_case("interrupt"));
                    self.all = values.iter().any(|s| s.eq_ignore_ascii_case("all"));
                    self.post_delay = values.iter().any(|s| s.eq_ignore_ascii_case("postdelay"));
                }
                _ => {}
            }
        }
    }
}

/// Audio definitions are inspectable without opening an output device.
#[derive(Clone, Debug, Default)]
pub struct FileEvent {
    pub filename: String,
    pub volume: f32,
}

pub struct Catalog {
    pub events: BTreeMap<String, Event>,
    pub dialog: BTreeMap<String, FileEvent>,
    pub music: BTreeMap<String, FileEvent>,
    pub eva: BTreeMap<String, speech::EvaEvent>,
    pub speech_volume: f32,
    pub music_volume: f32,
    speech_folder: String,
    music_folder: String,
    root: String,
    folder: String,
    extension: String,
    language: String,
    pub sound_volume: f32,
    pub relative_2d: f32,
    pub max_ui_voices: usize,
    pub max_world_voices: usize,
    pub world_volume: f32,
    relative_world: f32,
    global_range: [f32; 2],
    pub fx_sounds: BTreeMap<String, Vec<String>>,
}

impl Catalog {
    pub fn load(defs: &Definitions, language: &str) -> Self {
        let mut c = Self {
            events: BTreeMap::new(),
            dialog: BTreeMap::new(),
            music: BTreeMap::new(),
            eva: BTreeMap::new(),
            speech_volume: 0.7,
            music_volume: 0.55,
            speech_folder: "Speech".into(),
            music_folder: "Tracks".into(),
            root: "Data\\Audio".into(),
            folder: "Sounds".into(),
            extension: "wav".into(),
            language: language.into(),
            sound_volume: 0.8,
            relative_2d: 1.0,
            max_ui_voices: 4,
            max_world_voices: 25,
            world_volume: 0.8,
            relative_world: 1.0,
            global_range: [5000.0, 500000.0],
            fx_sounds: BTreeMap::new(),
        };
        for block in defs.ordered("AudioSettings") {
            for f in block.fields() {
                let values = f.values();
                let Some(v) = values.first() else { continue };
                match f.name.to_ascii_lowercase().as_str() {
                    "audioroot" => c.root = v.clone(),
                    "soundsfolder" => c.folder = v.clone(),
                    "streamingfolder" => c.speech_folder = v.clone(),
                    "musicfolder" => c.music_folder = v.clone(),
                    "defaultspeechvolume" => c.speech_volume = percent(v).unwrap_or(c.speech_volume).clamp(0.0, 1.0),
                    "defaultmusicvolume" => c.music_volume = percent(v).unwrap_or(c.music_volume).clamp(0.0, 1.0),
                    "soundsextension" => c.extension = v.trim_start_matches('.').into(),
                    "defaultsoundvolume" => c.sound_volume = percent(v).unwrap_or(c.sound_volume).clamp(0.0, 1.0),
                    "relative2dvolume" => {
                        let relative = percent(v).unwrap_or(0.0).clamp(-1.0, 1.0);
                        c.relative_2d = 1.0 + relative.min(0.0);
                        c.relative_world = 1.0 - relative.max(0.0);
                    }
                    "default3dsoundvolume" => c.world_volume = percent(v).unwrap_or(c.world_volume).clamp(0.0, 1.0),
                    "samplecount3d" => c.max_world_voices = v.parse().unwrap_or(c.max_world_voices),
                    "globalminrange" => c.global_range[0] = v.parse().unwrap_or(c.global_range[0]),
                    "globalmaxrange" => c.global_range[1] = v.parse().unwrap_or(c.global_range[1]),
                    "samplecount2d" => c.max_ui_voices = v.parse().unwrap_or(c.max_ui_voices),
                    _ => {}
                }
            }
        }
        // Each definition starts with DefaultSoundEffect, as the original
        // parser does, including when a later file replaces a named event.
        for block in defs.ordered("AudioEvent") {
            let Some(name) = block.args().first().cloned() else { continue };
            let mut event = c.events.get("DefaultSoundEffect").cloned().unwrap_or_default();
            event.apply(block);
            c.events.insert(name, event);
        }
        for (kind, default, target) in
            [("MusicTrack", "DefaultMusicTrack", &mut c.music), ("DialogEvent", "DefaultDialog", &mut c.dialog)]
        {
            for block in defs.ordered(kind) {
                let Some(name) = block.args().first().cloned() else { continue };
                let mut event =
                    target.get(default).cloned().unwrap_or(FileEvent { filename: String::new(), volume: 1.0 });
                for f in block.fields() {
                    let v = f.values();
                    let Some(first) = v.first() else { continue };
                    if f.name.eq_ignore_ascii_case("Filename") {
                        event.filename = first.clone();
                    }
                    if f.name.eq_ignore_ascii_case("Volume") {
                        event.volume = percent(first).unwrap_or(event.volume).max(0.0);
                    }
                }
                target.insert(name, event);
            }
        }
        for block in defs.ordered("EvaEvent") {
            if let Some(name) = block.args().first() {
                c.eva.insert(name.to_ascii_lowercase(), speech::EvaEvent::parse(block));
            }
        }
        for block in defs.ordered("FXList") {
            let Some(name) = block.args().first().cloned() else { continue };
            let sounds = block
                .blocks()
                .filter(|b| b.kind.eq_ignore_ascii_case("Sound"))
                .filter_map(|b| b.fields().find(|f| f.name.eq_ignore_ascii_case("Name"))?.values().first().cloned())
                .filter(|n| !n.eq_ignore_ascii_case("None"))
                .collect();
            c.fx_sounds.insert(name, sounds);
        }
        c
    }

    pub fn ui_events(&self) -> impl Iterator<Item = (&str, &Event)> {
        self.events
            .iter()
            .filter(|(n, e)| n.as_str() != "DefaultSoundEffect" && e.ui && !e.sounds.is_empty())
            .map(|(n, e)| (n.as_str(), e))
    }

    /// Localized clips win over generic clips; VFS mount priority still applies.
    pub fn resolve(&self, vfs: &Vfs, sound: &str) -> Result<String> {
        self.resolve_file(vfs, &self.folder, &format!("{sound}.{}", self.extension))
    }

    fn resolve_file(&self, vfs: &Vfs, folder: &str, filename: &str) -> Result<String> {
        let prefix = format!("{}\\{}", self.root, folder);
        let localized = format!("{prefix}\\{}\\{filename}", self.language);
        if vfs.exists(&localized) {
            return Ok(localized);
        }
        let generic = format!("{prefix}\\{filename}");
        ensure!(vfs.exists(&generic), "sound not found: {localized} or {generic}");
        Ok(generic)
    }
}

/// Decode without a device, also used by the headless asset check.
pub fn decode(vfs: &Vfs, path: &str) -> Result<StaticSoundData> {
    let bytes = vfs.read(path)?.with_context(|| format!("sound not found: {path}"))?;
    StaticSoundData::from_cursor(Cursor::new(bytes)).with_context(|| format!("decode {path}"))
}

/// Listener at the camera's ground target, with a screen-right unit vector.
#[derive(Clone, Copy, Debug)]
pub struct Listener {
    pub position: [f32; 3],
    pub right: [f32; 2],
}
impl Default for Listener {
    fn default() -> Self {
        Self { position: [0.0; 3], right: [1.0, 0.0] }
    }
}
impl Listener {
    /// Miles' effective-volume inverse-distance rule and hard maximum cutoff.
    /// Panning is a stereo approximation to the original 3D backend.
    pub fn spatial(&self, at: [f32; 3], range: [f32; 2]) -> (f32, f32) {
        let d = [at[0] - self.position[0], at[1] - self.position[1], at[2] - self.position[2]];
        let distance = (d[0] * d[0] + d[1] * d[1] + d[2] * d[2]).sqrt();
        let gain = if distance >= range[1] {
            0.0
        } else if distance <= range[0] {
            1.0
        } else {
            range[0].max(0.0) / distance.max(0.001)
        };
        let horizontal = d[0].hypot(d[1]);
        let pan = if horizontal > 0.001 { (d[0] * self.right[0] + d[1] * self.right[1]) / horizontal } else { 0.0 };
        (gain, ((pan.clamp(-1.0, 1.0) + 1.0) * 0.5))
    }
}
fn decibels(volume: f32) -> Decibels {
    if volume > 0.0 { Decibels(20.0 * volume.log10()) } else { Decibels::SILENCE }
}
#[derive(Clone, Copy, Debug, PartialEq)]
enum Phase {
    Attack,
    Body,
    Decay,
}
struct Voice {
    name: String,
    handle: StaticSoundHandle,
    spatial: Option<([f32; 3], [f32; 2])>,
    gain: f32,
    phase: Phase,
    remaining: u32,
    body_index: usize,
    emitter: Option<u32>,
    priority: u8,
}

/// A bounded UI and world voice pool; a missing device can be handled by the game host.
pub struct Audio<B: Backend = DefaultBackend> {
    manager: AudioManager<B>,
    pub catalog: Catalog,
    cache: BTreeMap<String, StaticSoundData>,
    active: Vec<Voice>,
    listener: Listener,
    speech: speech::Speech,
    music: music::Music,
    sequence: BTreeMap<String, usize>,
    rng: u32,
    time_of_day: String,
}

impl Audio {
    pub fn new(catalog: Catalog) -> Result<Self> {
        let manager = AudioManager::new(AudioManagerSettings::default()).context("open audio output")?;
        Ok(Self::with_manager(catalog, manager))
    }
}

impl<B: Backend> Audio<B> {
    pub fn with_manager(catalog: Catalog, manager: AudioManager<B>) -> Self {
        Self {
            manager,
            catalog,
            cache: BTreeMap::new(),
            active: vec![],
            sequence: BTreeMap::new(),
            rng: 0x91ab_3d57,
            time_of_day: String::new(),
            listener: Listener::default(),
            speech: speech::Speech::default(),
            music: music::Music::default(),
        }
    }

    fn random(&mut self) -> f64 {
        self.rng ^= self.rng << 13;
        self.rng ^= self.rng >> 17;
        self.rng ^= self.rng << 5;
        self.rng as f64 / u32::MAX as f64
    }

    pub fn update_listener(&mut self, listener: Listener) {
        self.listener = listener;
        for voice in &mut self.active {
            let category = if voice.spatial.is_some() {
                self.catalog.world_volume * self.catalog.relative_world
            } else {
                self.catalog.sound_volume * self.catalog.relative_2d
            };
            if let Some((at, range)) = voice.spatial {
                let (gain, pan) = listener.spatial(at, range);
                voice.handle.set_volume(decibels(voice.gain * gain * category), kira::Tween::default());
                voice.handle.set_panning(pan, kira::Tween::default());
            } else {
                voice.handle.set_volume(decibels(voice.gain * category), kira::Tween::default());
            }
        }
    }

    pub fn play_ui(&mut self, vfs: &Vfs, name: &str) -> Result<bool> {
        self.play(vfs, name, None)
    }

    pub fn play_world(&mut self, vfs: &Vfs, name: &str, position: [f32; 3]) -> Result<bool> {
        self.play(vfs, name, Some(position))
    }

    /// Dispatch only the Sound nuggets. Particles and bone transforms are separate client work.
    pub fn play_fx(&mut self, vfs: &Vfs, name: &str, position: [f32; 3]) -> Vec<String> {
        let sounds = self.catalog.fx_sounds.get(name).cloned().unwrap_or_default();
        sounds
            .into_iter()
            .filter_map(|sound| self.play_world(vfs, &sound, position).err().map(|e| format!("{sound}: {e:#}")))
            .collect()
    }

    fn play(&mut self, vfs: &Vfs, name: &str, position: Option<[f32; 3]>) -> Result<bool> {
        self.advance_voices(vfs)?;
        let mut event = self.catalog.events.get(name).with_context(|| format!("unknown audio event: {name}"))?.clone();
        ensure!(event.ui || position.is_some(), "{name}: world audio needs a position");
        if let Some(sounds) = event.variants.get(&format!("sounds{}", self.time_of_day)).filter(|s| !s.is_empty()) {
            event.sounds = sounds.clone();
        }
        ensure!(!event.sounds.is_empty(), "{name}: no sounds");
        let spatial = if event.ui {
            None
        } else {
            position.map(|p| (p, if event.global { self.catalog.global_range } else { event.range }))
        };
        let (attenuation, _) = spatial.map_or((1.0, 0.5), |(at, range)| self.listener.spatial(at, range));
        if attenuation <= 0.0 {
            return Ok(false);
        }
        let budget = if spatial.is_some() { self.catalog.max_world_voices } else { self.catalog.max_ui_voices };
        let same: Vec<usize> = self.active.iter().enumerate().filter(|(_, v)| v.name == name).map(|(i, _)| i).collect();
        let victim = if event.limit > 0 && same.len() >= event.limit {
            if event.interrupt {
                same.first().copied()
            } else {
                return Ok(false);
            }
        } else if self.active.iter().filter(|v| v.spatial.is_some() == spatial.is_some()).count() >= budget {
            self.active
                .iter()
                .enumerate()
                .filter(|(_, v)| v.spatial.is_some() == spatial.is_some() && v.priority < event.priority)
                .min_by_key(|(i, v)| (v.priority, *i))
                .map(|(i, _)| i)
        } else {
            None
        };
        if victim.is_none() && self.active.iter().filter(|v| v.spatial.is_some() == spatial.is_some()).count() >= budget
        {
            return Ok(false);
        }
        let phase = if event.attack.is_empty() { Phase::Body } else { Phase::Attack };
        let clips = if phase == Phase::Attack {
            &event.attack[..]
        } else if event.all {
            &event.sounds[..1]
        } else {
            &event.sounds[..]
        };
        let (handle, gain) = self.segment(vfs, name, &event, clips, spatial, true)?;
        if let Some(index) = victim {
            let mut old = self.active.remove(index);
            old.handle.stop(kira::Tween::default());
        }
        self.active.push(Voice {
            name: name.into(),
            handle,
            spatial,
            gain,
            phase,
            remaining: if event.looping { event.loop_count } else { 1 },
            body_index: 0,
            emitter: None,
            priority: event.priority,
        });
        Ok(true)
    }

    fn segment(
        &mut self,
        vfs: &Vfs,
        name: &str,
        event: &Event,
        clips: &[String],
        spatial: Option<([f32; 3], [f32; 2])>,
        delay: bool,
    ) -> Result<(StaticSoundHandle, f32)> {
        let (attenuation, pan) = spatial.map_or((1.0, 0.5), |(at, range)| self.listener.spatial(at, range));
        let previous = self.sequence.get(name).copied();
        let len = clips.len();
        let mut index = if event.random {
            (self.random() * len as f64) as usize % len
        } else {
            previous.map_or(0, |n| (n + 1) % len)
        };
        if event.random && len > 2 && previous == Some(index) {
            index = (index + 1) % len;
        }
        let path = self.catalog.resolve(vfs, &clips[index])?;
        if !self.cache.contains_key(&path) {
            self.cache.insert(path.clone(), decode(vfs, &path)?);
        }
        let volume_shift = 1.0 + event.volume_shift as f64 * (1.0 - self.random());
        let category = if spatial.is_some() {
            self.catalog.world_volume * self.catalog.relative_world
        } else {
            self.catalog.sound_volume * self.catalog.relative_2d
        };
        let base_gain = (event.volume as f64 * volume_shift).max(0.0) as f32;
        let volume = base_gain * category;
        let db = decibels(volume * attenuation);
        let pitch = event.pitch[0] + (event.pitch[1] - event.pitch[0]) * self.random();
        let delay = if delay {
            event.delay_ms[0] as f64 + (event.delay_ms[1] - event.delay_ms[0]) as f64 * self.random()
        } else {
            0.0
        };
        let sound = self.cache[&path]
            .volume(db)
            .panning(pan)
            .playback_rate(pitch)
            .start_time(Duration::from_secs_f64(delay / 1000.0));
        let handle = self.manager.play(sound).map_err(|e| anyhow::anyhow!("play {name}: {e:?}"))?;
        self.sequence.insert(name.into(), index);
        Ok((handle, base_gain))
    }
}
