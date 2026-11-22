use super::*;
use std::{
    io::Write,
    path::{Path, PathBuf},
};

#[derive(Clone, Copy, Debug, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct Preferences {
    pub effects: u8,
    pub speech: u8,
    pub music: u8,
}
impl Default for Preferences {
    fn default() -> Self {
        Self { effects: 80, speech: 70, music: 55 }
    }
}
impl Preferences {
    pub fn path() -> PathBuf {
        let root = if cfg!(windows) {
            std::env::var_os("APPDATA").map(PathBuf::from)
        } else {
            std::env::var_os("XDG_CONFIG_HOME")
                .map(PathBuf::from)
                .or_else(|| std::env::var_os("HOME").map(|p| PathBuf::from(p).join(".config")))
        };
        root.unwrap_or_else(std::env::temp_dir).join("ironhour").join("audio.toml")
    }
    pub fn load(path: &Path) -> Result<Self> {
        match std::fs::read_to_string(path) {
            Ok(text) => {
                let p: Self = toml::from_str(&text).context("parse audio preferences")?;
                p.validate()?;
                Ok(p)
            }
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(Self::default()),
            Err(e) => Err(e).context("read audio preferences"),
        }
    }
    fn validate(self) -> Result<()> {
        ensure!(self.effects <= 100 && self.speech <= 100 && self.music <= 100, "audio volumes must be 0..100");
        Ok(())
    }
    pub fn save(self, path: &Path) -> Result<()> {
        self.validate()?;
        let parent = path.parent().filter(|p| !p.as_os_str().is_empty()).unwrap_or(Path::new("."));
        std::fs::create_dir_all(parent)?;
        let mut file = tempfile::NamedTempFile::new_in(parent)?;
        file.write_all(toml::to_string(&self)?.as_bytes())?;
        file.as_file().sync_all()?;
        file.persist(path).map_err(|e| e.error).context("save audio preferences")?;
        Ok(())
    }
}

impl<B: Backend> Audio<B> {
    pub fn set_preferences(&mut self, p: Preferences) -> Result<()> {
        p.validate()?;
        self.catalog.sound_volume = p.effects as f32 / 100.0;
        self.catalog.world_volume = self.catalog.sound_volume;
        self.catalog.speech_volume = p.speech as f32 / 100.0;
        self.catalog.music_volume = p.music as f32 / 100.0;
        self.update_listener(self.listener);
        self.refresh_media_volumes();
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn saves_replace_atomically_and_invalid_values_do_not_overwrite() {
        let temp = tempfile::tempdir().unwrap();
        let path = temp.path().join("settings/audio.toml");
        let p = Preferences { effects: 0, speech: 100, music: 33 };
        p.save(&path).unwrap();
        assert_eq!(Preferences::load(&path).unwrap(), p);
        Preferences::default().save(&path).unwrap();
        assert!(Preferences { music: 101, ..p }.save(&path).is_err());
        assert_eq!(Preferences::load(&path).unwrap(), Preferences::default());
    }
}
