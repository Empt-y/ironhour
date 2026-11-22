//! Interface audio, following `INIAudioEventInfo.cpp`, `AudioEventRTS.cpp`
//! (`generateFilename`, localization and play information) and AudioSettings.
//!
//! Files come from the mounted VFS, never an extracted copy. Kira mixes decoded
//! Symphonia samples. This client-only slice does not consume simulation RNG.
//! World sounds, looping attack/body/decay, speech, music and EVA follow later.
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
    random: bool,
    looping: bool,
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
            random: false,
            looping: false,
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
                "type" => self.ui = values.iter().any(|s| s.eq_ignore_ascii_case("ui")),
                "control" => {
                    self.random = values.iter().any(|s| s.eq_ignore_ascii_case("random"));
                    self.looping = values.iter().any(|s| s.eq_ignore_ascii_case("loop"));
                }
                _ => {}
            }
        }
    }
}

/// Audio definitions are inspectable without opening an output device.
pub struct Catalog {
    pub events: BTreeMap<String, Event>,
    root: String,
    folder: String,
    extension: String,
    language: String,
    pub sound_volume: f32,
    pub relative_2d: f32,
    pub max_ui_voices: usize,
}

impl Catalog {
    pub fn load(defs: &Definitions, language: &str) -> Self {
        let mut c = Self {
            events: BTreeMap::new(),
            root: "Data\\Audio".into(),
            folder: "Sounds".into(),
            extension: "wav".into(),
            language: language.into(),
            sound_volume: 0.8,
            relative_2d: 1.0,
            max_ui_voices: 4,
        };
        for block in defs.ordered("AudioSettings") {
            for f in block.fields() {
                let values = f.values();
                let Some(v) = values.first() else { continue };
                match f.name.to_ascii_lowercase().as_str() {
                    "audioroot" => c.root = v.clone(),
                    "soundsfolder" => c.folder = v.clone(),
                    "soundsextension" => c.extension = v.trim_start_matches('.').into(),
                    "defaultsoundvolume" => c.sound_volume = percent(v).unwrap_or(c.sound_volume).clamp(0.0, 1.0),
                    "relative2dvolume" => c.relative_2d = 1.0 + percent(v).unwrap_or(0.0).clamp(-1.0, 0.0),
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
        let prefix = format!("{}\\{}", self.root, self.folder);
        let filename = format!("{sound}.{}", self.extension);
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

/// A bounded UI voice pool; a missing device can be handled by the game host.
pub struct Audio<B: Backend = DefaultBackend> {
    manager: AudioManager<B>,
    pub catalog: Catalog,
    cache: BTreeMap<String, StaticSoundData>,
    active: Vec<(String, StaticSoundHandle)>,
    sequence: BTreeMap<String, usize>,
    rng: u32,
}

impl Audio {
    pub fn new(catalog: Catalog) -> Result<Self> {
        let manager = AudioManager::new(AudioManagerSettings::default()).context("open audio output")?;
        Ok(Self::with_manager(catalog, manager))
    }
}

impl<B: Backend> Audio<B> {
    pub fn with_manager(catalog: Catalog, manager: AudioManager<B>) -> Self {
        Self { manager, catalog, cache: BTreeMap::new(), active: vec![], sequence: BTreeMap::new(), rng: 0x91ab_3d57 }
    }

    fn random(&mut self) -> f64 {
        self.rng ^= self.rng << 13;
        self.rng ^= self.rng >> 17;
        self.rng ^= self.rng << 5;
        self.rng as f64 / u32::MAX as f64
    }

    /// False means the event was culled by its limit or the UI voice budget.
    pub fn play_ui(&mut self, vfs: &Vfs, name: &str) -> Result<bool> {
        self.active.retain(|(_, h)| h.state() != PlaybackState::Stopped);
        let event = self.catalog.events.get(name).with_context(|| format!("unknown audio event: {name}"))?.clone();
        ensure!(event.ui, "{name}: world audio is not implemented yet");
        ensure!(!event.looping, "{name}: looping audio is not implemented yet");
        ensure!(!event.sounds.is_empty(), "{name}: no sounds");
        if self.active.len() >= self.catalog.max_ui_voices
            || (event.limit > 0 && self.active.iter().filter(|(n, _)| n == name).count() >= event.limit)
        {
            return Ok(false);
        }
        let previous = self.sequence.get(name).copied();
        let len = event.sounds.len();
        let mut index = if event.random {
            (self.random() * len as f64) as usize % len
        } else {
            previous.map_or(0, |n| (n + 1) % len)
        };
        if event.random && len > 2 && previous == Some(index) {
            index = (index + 1) % len;
        }
        let path = self.catalog.resolve(vfs, &event.sounds[index])?;
        if !self.cache.contains_key(&path) {
            self.cache.insert(path.clone(), decode(vfs, &path)?);
        }
        let volume_shift = 1.0 + event.volume_shift as f64 * (1.0 - self.random());
        let volume =
            (event.volume as f64 * volume_shift * self.catalog.sound_volume as f64 * self.catalog.relative_2d as f64)
                .max(0.0);
        let db = if volume > 0.0 { Decibels((20.0 * volume.log10()) as f32) } else { Decibels::SILENCE };
        let pitch = event.pitch[0] + (event.pitch[1] - event.pitch[0]) * self.random();
        let delay = event.delay_ms[0] as f64 + (event.delay_ms[1] - event.delay_ms[0]) as f64 * self.random();
        let sound =
            self.cache[&path].volume(db).playback_rate(pitch).start_time(Duration::from_secs_f64(delay / 1000.0));
        let handle = self.manager.play(sound).map_err(|e| anyhow::anyhow!("play {name}: {e:?}"))?;
        self.sequence.insert(name.into(), index);
        self.active.push((name.into(), handle));
        Ok(true)
    }
}
