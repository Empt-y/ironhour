//! Finding the installed game.
//!
//! The original reads `InstallPath` and `Language` from
//! `HKLM\SOFTWARE\Electronic Arts\EA Games\<game>` (`Common/System/registry.cpp`).
//! Environment variables take precedence so tests, CI and non-Windows systems can
//! point at a copy anywhere.

use std::path::PathBuf;

pub const ENV_ZERO_HOUR: &str = "IH_ZH_INSTALL";
pub const ENV_GENERALS: &str = "IH_GENERALS_INSTALL";
pub const ENV_LANGUAGE: &str = "IH_LANGUAGE";

/// Registry key names used by the original game.
pub const REG_ZERO_HOUR: &str = r"SOFTWARE\Electronic Arts\EA Games\Command and Conquer Generals Zero Hour";
pub const REG_GENERALS: &str = r"SOFTWARE\Electronic Arts\EA Games\Generals";

#[derive(Debug, Clone, Default)]
pub struct Installs {
    pub zero_hour: Option<PathBuf>,
    pub generals: Option<PathBuf>,
    /// Language folder name under `Data\` (`GetRegistryLanguage`), e.g. `english`.
    pub language: String,
}

impl Installs {
    /// Environment variables first, then the registry.
    pub fn locate() -> Self {
        Self {
            zero_hour: from_env(ENV_ZERO_HOUR).or_else(|| registry_path(REG_ZERO_HOUR)),
            generals: from_env(ENV_GENERALS).or_else(|| registry_path(REG_GENERALS)),
            language: std::env::var(ENV_LANGUAGE)
                .ok()
                .or_else(|| registry_string(REG_ZERO_HOUR, "Language"))
                .unwrap_or_else(|| "english".to_string()),
        }
    }
}

fn from_env(var: &str) -> Option<PathBuf> {
    let v = std::env::var_os(var)?;
    let p = PathBuf::from(v);
    p.is_dir().then_some(p)
}

fn registry_path(key: &str) -> Option<PathBuf> {
    let s = registry_string(key, "InstallPath")?;
    let p = PathBuf::from(s.trim_end_matches(['\\', '/']));
    p.is_dir().then_some(p)
}

#[cfg(windows)]
fn registry_string(key: &str, value: &str) -> Option<String> {
    use winreg::RegKey;
    use winreg::enums::{HKEY_LOCAL_MACHINE, KEY_READ, KEY_WOW64_32KEY};
    // The game is 32-bit, so its keys live in the 32-bit registry view
    // (WOW6432Node on 64-bit Windows).
    let hklm = RegKey::predef(HKEY_LOCAL_MACHINE);
    let k = hklm.open_subkey_with_flags(key, KEY_READ | KEY_WOW64_32KEY).ok()?;
    k.get_value(value).ok()
}

#[cfg(not(windows))]
fn registry_string(_key: &str, _value: &str) -> Option<String> {
    None
}
