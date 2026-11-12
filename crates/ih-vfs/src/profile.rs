//! Mod profiles: which archives to mount on top of the stock game.
//!
//! The original only knows `-mod <file or folder>` (`Common/CommandLine.cpp`,
//! `parseMod`). Profiles cover that and the schemes mod launchers use, such as
//! Contra's `.ctr` archives that its launcher enables selectively.
//!
//! ```toml
//! name = "Contra X Beta 2"
//! extensions = ["ctr"]          # extra archive extensions in the Zero Hour folder
//! exclude = ["*_ControlBarPro.ctr"]
//! mod_dir = []                  # like -mod <folder>: every .big inside, overwrite
//! mod_big = []                  # like -mod <file.big>
//! ```

use std::path::{Path, PathBuf};

use serde::Deserialize;

use crate::VfsError;

#[derive(Debug, Clone, Default, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct ModProfile {
    pub name: String,
    /// Extra extensions treated as archives in the Zero Hour folder, on top of `big`.
    pub extensions: Vec<String>,
    /// Archive file names to skip (case-insensitive wildcards).
    pub exclude: Vec<String>,
    /// Folders mounted like `-mod <folder>`.
    pub mod_dir: Vec<PathBuf>,
    /// Archives mounted like `-mod <file.big>`.
    pub mod_big: Vec<PathBuf>,
}

impl ModProfile {
    /// The unmodified game.
    pub fn stock() -> Self {
        Self { name: "Stock Zero Hour".into(), ..Self::default() }
    }

    pub fn load(path: impl AsRef<Path>) -> Result<Self, VfsError> {
        let path = path.as_ref();
        let text = std::fs::read_to_string(path).map_err(|e| VfsError::Io(path.to_path_buf(), e))?;
        let mut p: Self = toml::from_str(&text).map_err(|e| VfsError::Profile(path.to_path_buf(), e.to_string()))?;
        // Relative mod paths are relative to the profile file.
        let base = path.parent().unwrap_or(Path::new("."));
        for d in p.mod_dir.iter_mut().chain(p.mod_big.iter_mut()) {
            if d.is_relative() {
                *d = base.join(&*d);
            }
        }
        if p.name.is_empty() {
            p.name = path.file_stem().map(|s| s.to_string_lossy().into_owned()).unwrap_or_default();
        }
        Ok(p)
    }

    pub fn archive_extensions(&self) -> Vec<String> {
        let mut v = vec!["big".to_string()];
        v.extend(self.extensions.iter().cloned());
        v
    }
}
