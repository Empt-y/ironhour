//! Streaming decode from compressed VFS bytes. No decoded whole-track cache.
use super::*;
use kira::sound::{
    FromFileError,
    streaming::{StreamingSoundData, StreamingSoundHandle},
};

#[derive(Default)]
pub(crate) struct Music {
    current: Option<(String, StreamingSoundHandle<FromFileError>)>,
    playlist: Vec<String>,
    next: usize,
}

impl Catalog {
    pub fn music_data(&self, vfs: &Vfs, name: &str) -> Result<StreamingSoundData<FromFileError>> {
        let event = self.music.get(name).with_context(|| format!("unknown music track: {name}"))?;
        let path = self.resolve_file(vfs, &self.music_folder, &event.filename)?;
        let bytes = vfs.read(&path)?.with_context(|| format!("music not found: {path}"))?;
        Ok(StreamingSoundData::from_cursor(Cursor::new(bytes))
            .with_context(|| format!("stream {path}"))?
            .volume(decibels(event.volume * self.music_volume)))
    }
}

impl<B: Backend> Audio<B> {
    /// Replace music only once the replacement has loaded and started successfully.
    pub fn play_music(&mut self, vfs: &Vfs, name: &str) -> Result<()> {
        if self.music.current.as_ref().is_some_and(|(n, h)| n == name && h.state() != PlaybackState::Stopped) {
            return Ok(());
        }
        let data = self.catalog.music_data(vfs, name)?.loop_region(..);
        let handle = self.manager.play(data).map_err(|e| anyhow::anyhow!("music {name}: {e:?}"))?;
        self.stop_music();
        self.music.current = Some((name.into(), handle));
        Ok(())
    }

    pub fn play_playlist(&mut self, vfs: &Vfs, tracks: Vec<String>) -> Result<()> {
        ensure!(!tracks.is_empty(), "empty music playlist");
        let data = self.catalog.music_data(vfs, &tracks[0])?;
        let handle = self.manager.play(data).map_err(|e| anyhow::anyhow!("music: {e:?}"))?;
        self.stop_music();
        self.music.current = Some((tracks[0].clone(), handle));
        self.music.next = 1 % tracks.len();
        self.music.playlist = tracks;
        Ok(())
    }

    pub fn stop_music(&mut self) {
        self.music.playlist.clear();
        self.music.next = 0;
        if let Some((_, mut handle)) = self.music.current.take() {
            handle.stop(kira::Tween::default());
        }
    }

    pub fn update_media(&mut self, vfs: &Vfs, now: Duration) -> Vec<String> {
        let mut errors = vec![];
        if let Err(e) = self.advance_voices(vfs) {
            errors.push(format!("{e:#}"));
        }
        if let Err(e) = self.update_speech(vfs, now) {
            errors.push(format!("{e:#}"));
        }
        if let Some((name, handle)) = &mut self.music.current {
            while let Some(e) = handle.pop_error() {
                errors.push(format!("stream {name}: {e}"));
            }
        }
        if !self.music.playlist.is_empty()
            && self.music.current.as_ref().is_some_and(|(_, h)| h.state() == PlaybackState::Stopped)
        {
            let name = self.music.playlist[self.music.next].clone();
            self.music.next = (self.music.next + 1) % self.music.playlist.len();
            match self
                .catalog
                .music_data(vfs, &name)
                .and_then(|data| self.manager.play(data).map_err(|e| anyhow::anyhow!("music: {e:?}")))
            {
                Ok(handle) => self.music.current = Some((name, handle)),
                Err(e) => {
                    errors.push(format!("{e:#}"));
                    self.stop_music();
                }
            }
        }
        errors
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::tests::{definitions, files};
    #[test]
    fn music_probes_vfs_bytes_without_an_audio_device() {
        let (temp, _) = files(false);
        let dir = temp.path().join("Data/Audio/Tracks/English");
        std::fs::create_dir_all(&dir).unwrap();
        let source = temp.path().join("Data/Audio/Sounds/click.wav");
        std::fs::copy(source, dir.join("theme.wav")).unwrap();
        let mut vfs = Vfs::new();
        vfs.set_loose_root(temp.path()).unwrap();
        let d = definitions(
            "MusicTrack DefaultMusicTrack\n Volume = 75\nEnd\nMusicTrack Shell\n Filename = theme.wav\nEnd\n",
        );
        let catalog = Catalog::load(&d, "English");
        assert_eq!(catalog.music["Shell"].volume, 0.75);
        let data = catalog.music_data(&vfs, "Shell").unwrap();
        assert!((data.duration().as_secs_f64() - 0.1).abs() < 0.00001);
        assert!(catalog.music_data(&vfs, "Missing").is_err());
        let manager = AudioManager::<kira::backend::mock::MockBackend>::new(AudioManagerSettings::default()).unwrap();
        let mut audio = Audio::with_manager(catalog, manager);
        audio.play_playlist(&vfs, vec!["Shell".into()]).unwrap();
        assert_eq!(audio.music.playlist, ["Shell"]);
        assert!(audio.play_music(&vfs, "Missing").is_err());
        assert_eq!(audio.music.current.as_ref().unwrap().0, "Shell", "failed replacement preserves playback");
        audio.stop_music();
        assert!(audio.music.current.is_none());
        assert!(audio.music.playlist.is_empty());
    }
}
