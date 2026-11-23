use anyhow::Result;
use std::{io::Write, path::Path};
#[derive(Clone, Debug, serde::Serialize, serde::Deserialize)]
pub struct Interface {
    pub width: u32,
    pub height: u32,
    pub scroll: u32,
}
impl Default for Interface {
    fn default() -> Self {
        Self { width: 1280, height: 800, scroll: 50 }
    }
}
impl Interface {
    pub fn load(cli: &crate::Cli) -> Self {
        let path = cli
            .audio_preferences
            .clone()
            .unwrap_or_else(ih_audio::preferences::Preferences::path)
            .with_file_name("interface.toml");
        std::fs::read_to_string(path)
            .ok()
            .and_then(|text| toml::from_str::<Self>(&text).ok())
            .filter(|p| (640..=8192).contains(&p.width) && (480..=8192).contains(&p.height) && p.scroll <= 100)
            .unwrap_or_default()
    }
    pub fn save(&self, path: &Path) -> Result<()> {
        anyhow::ensure!(
            (640..=8192).contains(&self.width) && (480..=8192).contains(&self.height) && self.scroll <= 100,
            "invalid interface settings"
        );
        let parent = path.parent().unwrap_or(Path::new("."));
        std::fs::create_dir_all(parent)?;
        let mut file = tempfile::NamedTempFile::new_in(parent)?;
        file.write_all(toml::to_string(self)?.as_bytes())?;
        file.as_file().sync_all()?;
        file.persist(path).map_err(|e| e.error)?;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use clap::Parser;
    #[test]
    fn persists_resolution_and_speed_and_rejects_invalid_replacement() {
        let root = tempfile::tempdir().unwrap();
        let audio = root.path().join("audio.toml");
        let path = root.path().join("interface.toml");
        let cli = crate::Cli::parse_from(["ironhour", "--audio-preferences", audio.to_str().unwrap()]);
        let prefs = Interface { width: 1600, height: 900, scroll: 75 };
        prefs.save(&path).unwrap();
        let loaded = Interface::load(&cli);
        assert_eq!((loaded.width, loaded.height, loaded.scroll), (1600, 900, 75));
        assert!(Interface { width: 0, ..prefs }.save(&path).is_err());
        assert_eq!(Interface::load(&cli).width, 1600);
    }
}
