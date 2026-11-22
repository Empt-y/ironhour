//! Client narration arbitration. EVA uses Eva.ini's side, priority, expiry and
//! repeat interval; unit responses share one voice channel with announcements.
use super::*;
use kira::sound::{
    FromFileError,
    streaming::{StreamingSoundData, StreamingSoundHandle},
};

#[derive(Clone, Debug)]
pub struct EvaEvent {
    pub priority: u32,
    pub interval: Duration,
    pub expiry: Duration,
    pub sides: BTreeMap<String, Vec<String>>,
}

impl EvaEvent {
    pub(crate) fn parse(block: &Block) -> Self {
        let number = |key: &str, default| {
            block
                .fields()
                .filter(|f| f.name.eq_ignore_ascii_case(key))
                .last()
                .and_then(|f| f.values().first()?.parse::<u64>().ok())
                .unwrap_or(default)
        };
        let mut sides = BTreeMap::new();
        for b in block.blocks().filter(|b| b.kind.eq_ignore_ascii_case("SideSounds")) {
            let value = |key: &str| {
                b.fields().filter(|f| f.name.eq_ignore_ascii_case(key)).last().map(|f| f.values()).unwrap_or_default()
            };
            if let Some(side) = value("Side").first() {
                sides.insert(side.to_ascii_lowercase(), value("Sounds"));
            }
        }
        Self {
            priority: number("Priority", 1) as u32,
            interval: Duration::from_millis(number("TimeBetweenChecksMS", 30000)),
            expiry: Duration::from_millis(number("ExpirationTimeMS", 5000)),
            sides,
        }
    }
}

#[derive(Clone, Debug)]
struct Request {
    name: String,
    priority: u32,
    expires: Duration,
}

pub(super) enum Narration {
    Sample(StaticSoundHandle),
    Stream(StreamingSoundHandle<FromFileError>),
}
impl Narration {
    fn state(&self) -> PlaybackState {
        match self {
            Self::Sample(h) => h.state(),
            Self::Stream(h) => h.state(),
        }
    }
    fn stop(&mut self, tween: kira::Tween) {
        match self {
            Self::Sample(h) => h.stop(tween),
            Self::Stream(h) => h.stop(tween),
        }
    }
    pub(super) fn set_volume(&mut self, db: Decibels, tween: kira::Tween) {
        match self {
            Self::Sample(h) => h.set_volume(db, tween),
            Self::Stream(h) => h.set_volume(db, tween),
        }
    }
}

#[derive(Default)]
pub(crate) struct Speech {
    queue: Vec<Request>,
    pub(super) current: Option<(String, u32, Narration)>,
    cooldown: BTreeMap<String, Duration>,
}

impl Catalog {
    pub fn dialog_stream(&self, vfs: &Vfs, name: &str) -> Result<StreamingSoundData<FromFileError>> {
        let event = self.dialog.get(name).with_context(|| format!("unknown speech event: {name}"))?;
        let path = self.resolve_file(vfs, &self.speech_folder, &event.filename)?;
        let bytes = vfs.read(&path)?.with_context(|| format!("speech missing: {path}"))?;
        Ok(StreamingSoundData::from_cursor(Cursor::new(bytes))?.volume(decibels(event.volume * self.speech_volume)))
    }
    pub fn dialog_data(&self, vfs: &Vfs, name: &str) -> Result<StaticSoundData> {
        let event = self.dialog.get(name).with_context(|| format!("unknown speech event: {name}"))?;
        let path = self.resolve_file(vfs, &self.speech_folder, &event.filename)?;
        Ok(decode(vfs, &path)?.volume(decibels(event.volume * self.speech_volume)))
    }
}

impl<B: Backend> Audio<B> {
    /// Bounded FIFO within priority; duplicates and expired messages are discarded.
    /// A new unit response replaces an older unit response, never an EVA announcement.
    pub fn queue_speech(&mut self, name: &str, priority: u32, now: Duration, lifetime: Duration) -> bool {
        self.speech.queue.retain(|r| r.expires > now);
        if priority == 0
            || lifetime.is_zero()
            || self.speech.queue.len() >= 32
            || self.speech.queue.iter().any(|r| r.name == name)
            || self.speech.current.as_ref().is_some_and(|(n, _, h)| n == name && h.state() != PlaybackState::Stopped)
        {
            return false;
        }
        if priority == 1 {
            self.speech.queue.retain(|r| r.priority != 1);
        }
        if priority == 1
            && let Some((_, 1, handle)) = &mut self.speech.current
        {
            handle.stop(kira::Tween::default());
            self.speech.queue.retain(|r| r.priority != 1);
        }
        self.speech.queue.push(Request { name: name.into(), priority, expires: now.saturating_add(lifetime) });
        true
    }

    pub fn queue_eva(&mut self, event: &str, side: &str, now: Duration) -> bool {
        let key = event.to_ascii_lowercase();
        let Some(info) = self.catalog.eva.get(&key).cloned() else { return false };
        if info.priority == 0 {
            return false;
        }
        if self.speech.cooldown.get(&key).is_some_and(|until| now < *until) {
            return false;
        }
        let Some(sounds) = info.sides.get(&side.to_ascii_lowercase()).filter(|s| !s.is_empty()) else { return false };
        let index = (self.random() * sounds.len() as f64) as usize % sounds.len();
        if !self.queue_speech(&sounds[index], info.priority.saturating_add(1).max(2), now, info.expiry) {
            return false;
        }
        self.speech.cooldown.insert(key, now.saturating_add(info.interval));
        true
    }

    /// Script/campaign hook: explicit interruption never displaces a higher-priority speaker.
    pub fn request_dialog(
        &mut self,
        name: &str,
        priority: u32,
        interrupt: bool,
        now: Duration,
        lifetime: Duration,
    ) -> bool {
        if interrupt
            && let Some((_, current_priority, handle)) = &mut self.speech.current
            && priority >= *current_priority
        {
            handle.stop(kira::Tween::default());
        }
        self.queue_speech(name, priority, now, lifetime)
    }
    pub fn stop_speech(&mut self) {
        if let Some((_, _, mut handle)) = self.speech.current.take() {
            handle.stop(kira::Tween::default());
        }
        self.speech.queue.clear();
    }

    /// Called every client frame, including frames with no simulation tick.
    pub fn update_speech(&mut self, vfs: &Vfs, now: Duration) -> Result<()> {
        if let Some((_, _, Narration::Stream(handle))) = &mut self.speech.current
            && let Some(error) = handle.pop_error()
        {
            handle.stop(kira::Tween::default());
            return Err(anyhow::anyhow!("speech stream: {error}"));
        }
        if self.speech.current.as_ref().is_some_and(|(_, _, h)| h.state() != PlaybackState::Stopped) {
            return Ok(());
        }
        self.speech.current = None;
        self.speech.queue.retain(|r| r.expires > now);
        // Reverse priority, stable queue position for ties.
        let Some(index) = self
            .speech
            .queue
            .iter()
            .enumerate()
            .max_by_key(|(i, r)| (r.priority, std::cmp::Reverse(*i)))
            .map(|(i, _)| i)
        else {
            return Ok(());
        };
        let request = self.speech.queue.remove(index);
        let handle = if let Some(event) = self.catalog.events.get(&request.name).cloned() {
            ensure!(!event.looping && !event.sounds.is_empty(), "{}: invalid speech event", request.name);
            let index = (self.random() * event.sounds.len() as f64) as usize % event.sounds.len();
            let path = self.catalog.resolve(vfs, &event.sounds[index])?;
            if !self.cache.contains_key(&path) {
                self.cache.insert(path.clone(), decode(vfs, &path)?);
            }
            let pitch = event.pitch[0] + (event.pitch[1] - event.pitch[0]) * self.random();
            let volume =
                event.volume * self.catalog.speech_volume * (1.0 + event.volume_shift * (1.0 - self.random() as f32));
            let data = self.cache[&path].volume(decibels(volume.max(0.0))).playback_rate(pitch);
            Narration::Sample(self.manager.play(data).map_err(|e| anyhow::anyhow!("speech: {e:?}"))?)
        } else {
            let data = self.catalog.dialog_stream(vfs, &request.name)?;
            Narration::Stream(self.manager.play(data).map_err(|e| anyhow::anyhow!("speech: {e:?}"))?)
        };
        self.speech.current = Some((request.name, request.priority, handle));
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::tests::{definitions, files};
    use kira::backend::mock::MockBackend;
    #[test]
    fn announcements_take_priority_and_keep_fifo_order_without_unit_interruptions() {
        let (_temp, vfs) = files(false);
        let d = definitions(
            "AudioEvent Unit\n Sounds = click\nEnd\nAudioEvent AlertA\n Sounds = click\nEnd\nAudioEvent AlertB\n Sounds = click\nEnd\n",
        );
        let manager = AudioManager::<MockBackend>::new(AudioManagerSettings::default()).unwrap();
        let mut audio = Audio::with_manager(Catalog::load(&d, "English"), manager);
        let ttl = Duration::from_secs(3);
        assert!(audio.queue_speech("Unit", 1, Duration::ZERO, ttl));
        assert!(audio.queue_speech("AlertA", 3, Duration::ZERO, ttl));
        assert!(audio.queue_speech("AlertB", 3, Duration::ZERO, ttl));
        audio.update_speech(&vfs, Duration::ZERO).unwrap();
        assert_eq!(audio.speech.current.as_ref().unwrap().0, "AlertA");
        // A unit response cannot replace an announcement already playing.
        audio.queue_speech("Click", 1, Duration::ZERO, ttl);
        assert_eq!(audio.speech.current.as_ref().unwrap().0, "AlertA");
        for _ in 0..100 {
            audio.manager.backend_mut().on_start_processing();
            audio.manager.backend_mut().process();
        }
        audio.update_speech(&vfs, Duration::from_millis(100)).unwrap();
        assert_eq!(audio.speech.current.as_ref().unwrap().0, "AlertB");
    }

    #[test]
    fn dialog_filename_and_default_volume_use_the_speech_folder() {
        let (temp, _) = files(false);
        let folder = temp.path().join("Data/Audio/Speech/English");
        std::fs::create_dir_all(&folder).unwrap();
        std::fs::copy(temp.path().join("Data/Audio/Sounds/click.wav"), folder.join("line.wav")).unwrap();
        let mut vfs = Vfs::new();
        vfs.set_loose_root(temp.path()).unwrap();
        let d =
            definitions("DialogEvent DefaultDialog\n Volume = 50\nEnd\nDialogEvent Alert\n Filename = line.wav\nEnd\n");
        let catalog = Catalog::load(&d, "English");
        assert_eq!(catalog.dialog["Alert"].volume, 0.5);
        assert_eq!(catalog.dialog_data(&vfs, "Alert").unwrap().frames.len(), 4800);
        assert!(catalog.dialog_data(&vfs, "Missing").is_err());
        assert_eq!(catalog.dialog_stream(&vfs, "Alert").unwrap().duration(), Duration::from_millis(100));
        let manager = AudioManager::<MockBackend>::new(AudioManagerSettings::default()).unwrap();
        let mut audio = Audio::with_manager(catalog, manager);
        audio.request_dialog("Alert", 5, true, Duration::ZERO, Duration::from_secs(5));
        audio.update_speech(&vfs, Duration::ZERO).unwrap();
        assert!(matches!(audio.speech.current.as_ref().unwrap().2, Narration::Stream(_)));
        assert!(audio.cache.is_empty(), "dialogue must not enter the whole-PCM cache");
        audio.stop_speech();
        assert!(audio.speech.current.is_none());
    }
    #[test]
    fn eva_uses_side_priority_expiry_cooldown_and_duplicate_suppression() {
        let (_temp, vfs) = files(false);
        let d = definitions(
            "EvaEvent LowPower\n Priority = 2\n TimeBetweenChecksMS = 1000\n ExpirationTimeMS = 500\n SideSounds\n Side = America\n Sounds = Click\n End\nEnd\nEvaEvent Disabled\n Priority = 0\n SideSounds\n Side = America\n Sounds = Click\n End\nEnd\n",
        );
        let manager = AudioManager::<MockBackend>::new(AudioManagerSettings::default()).unwrap();
        let mut audio = Audio::with_manager(Catalog::load(&d, "English"), manager);
        assert!(!audio.queue_eva("LowPower", "China", Duration::ZERO));
        assert!(!audio.queue_eva("Disabled", "America", Duration::ZERO));
        assert!(audio.speech.queue.is_empty());
        assert!(audio.queue_eva("LOWPOWER", "america", Duration::ZERO));
        assert!(!audio.queue_eva("LowPower", "America", Duration::from_millis(200)));
        audio.update_speech(&vfs, Duration::from_millis(600)).unwrap();
        assert!(audio.speech.current.is_none(), "expired announcement is not played");
        assert!(audio.queue_eva("LowPower", "America", Duration::from_millis(1001)));
        audio.update_speech(&vfs, Duration::from_millis(1001)).unwrap();
        assert_eq!(audio.speech.current.as_ref().unwrap().1, 3);
        assert!(!audio.queue_speech("Click", 1, Duration::from_millis(1002), Duration::from_secs(1)));
    }
}
