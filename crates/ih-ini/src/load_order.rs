//! Which INI files the game loads, and in what order.
//!
//! Ported from the startup sequence in `GameEngine/Source/Common/GameEngine.cpp`
//! (`GameEngine::init`) and the `init()` of each subsystem it creates. A
//! subsystem's `init()` runs before its own INI files are loaded
//! (`SubsystemInterfaceList::initSubsystem`), so files loaded inside `init()` come
//! first. Every file in [`STARTUP`] is required: the original aborts if one is
//! missing (`INI_CANT_OPEN_FILE`).
//!
//! Files that ship in the archives but are never loaded, so their contents have no
//! effect: `Data\INI\Default\ButtonSets.ini` (no `ButtonSet` block exists) and
//! `Data\INI\ControlBarResizer.ini` (the code that loads it is commented out in
//! `ControlBar.cpp`).

use ih_vfs::Vfs;

/// How a file's blocks are applied (`INILoadType`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Mode {
    /// Normal load; later definitions replace earlier ones.
    Overwrite,
    /// Per-map overrides (`map.ini`, `solo.ini`): definitions become overrides that are
    /// discarded when the map ends.
    CreateOverrides,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Source {
    File(&'static str),
    /// `INI::loadDirectory`: every `*.ini` below the folder, files directly in it first,
    /// then files in subfolders, each group in case-insensitive order.
    Dir(&'static str),
    /// A file under the language folder; `{lang}` is replaced (e.g. `English`).
    LangFile(&'static str),
    /// A folder in the user's data directory (outside the game's file system).
    UserDir(&'static str),
}

#[derive(Debug, Clone, Copy)]
pub struct Step {
    pub subsystem: &'static str,
    pub source: Source,
    /// Included in the INI checksum used to detect mismatched data in multiplayer.
    pub crc: bool,
    /// Where the original does this.
    pub cpp: &'static str,
}

const fn f(subsystem: &'static str, path: &'static str, crc: bool, cpp: &'static str) -> Step {
    Step { subsystem, source: Source::File(path), crc, cpp }
}

/// Number of [`STARTUP`] steps that run before `-mod` archives are mounted
/// (`parseCommandLine` runs right after GameData). A `-mod` cannot change GameData.
pub const BEFORE_MODS: usize = 2;

/// Files loaded at startup, in order.
pub static STARTUP: &[Step] = &[
    f("GlobalData", "Data\\INI\\Default\\GameData.ini", true, "GameEngine.cpp:363"),
    f("GlobalData", "Data\\INI\\GameData.ini", true, "GameEngine.cpp:363"),
    // -- parseCommandLine(): -mod archives are mounted here --
    f("GameLOD", "Data\\INI\\GameLOD.ini", false, "GameLOD.cpp:272"),
    f("GameLOD", "Data\\INI\\GameLODPresets.ini", false, "GameLOD.cpp:275"),
    f("Water", "Data\\INI\\Default\\Water.ini", true, "GameEngine.cpp:394"),
    f("Water", "Data\\INI\\Water.ini", true, "GameEngine.cpp:395"),
    f("Weather", "Data\\INI\\Default\\Weather.ini", true, "GameEngine.cpp:396"),
    f("Weather", "Data\\INI\\Weather.ini", true, "GameEngine.cpp:397"),
    f("ScienceStore", "Data\\INI\\Default\\Science.ini", true, "GameEngine.cpp:422"),
    f("ScienceStore", "Data\\INI\\Science.ini", true, "GameEngine.cpp:422"),
    f("MultiplayerSettings", "Data\\INI\\Default\\Multiplayer.ini", true, "GameEngine.cpp:423"),
    f("MultiplayerSettings", "Data\\INI\\Multiplayer.ini", true, "GameEngine.cpp:423"),
    f("TerrainTypes", "Data\\INI\\Default\\Terrain.ini", true, "GameEngine.cpp:424"),
    f("TerrainTypes", "Data\\INI\\Terrain.ini", true, "GameEngine.cpp:424"),
    f("TerrainRoads", "Data\\INI\\Default\\Roads.ini", true, "GameEngine.cpp:425"),
    f("TerrainRoads", "Data\\INI\\Roads.ini", true, "GameEngine.cpp:425"),
    Step {
        subsystem: "GlobalLanguage",
        source: Source::LangFile("Data\\{lang}\\Language.ini"),
        crc: false,
        cpp: "GlobalLanguage.cpp:156",
    },
    f("Audio", "Data\\INI\\AudioSettings.ini", false, "GameAudio.cpp:219"),
    f("Audio", "Data\\INI\\Default\\Music.ini", false, "GameAudio.cpp:221"),
    f("Audio", "Data\\INI\\Music.ini", false, "GameAudio.cpp:222"),
    f("Audio", "Data\\INI\\Default\\SoundEffects.ini", false, "GameAudio.cpp:224"),
    f("Audio", "Data\\INI\\SoundEffects.ini", false, "GameAudio.cpp:225"),
    f("Audio", "Data\\INI\\Default\\Speech.ini", false, "GameAudio.cpp:227"),
    f("Audio", "Data\\INI\\Speech.ini", false, "GameAudio.cpp:228"),
    f("Audio", "Data\\INI\\Default\\Voice.ini", false, "GameAudio.cpp:230"),
    f("Audio", "Data\\INI\\Voice.ini", false, "GameAudio.cpp:231"),
    f("Audio", "Data\\INI\\MiscAudio.ini", false, "GameAudio.cpp:234"),
    f("RankInfoStore", "Data\\INI\\Rank.ini", true, "GameEngine.cpp:451"),
    f("PlayerTemplateStore", "Data\\INI\\Default\\PlayerTemplate.ini", true, "GameEngine.cpp:452"),
    f("PlayerTemplateStore", "Data\\INI\\PlayerTemplate.ini", true, "GameEngine.cpp:452"),
    f("ParticleSystemManager", "Data\\INI\\ParticleSystem.ini", false, "ParticleSys.cpp:2871"),
    f("FXListStore", "Data\\INI\\Default\\FXList.ini", true, "GameEngine.cpp:463"),
    f("FXListStore", "Data\\INI\\FXList.ini", true, "GameEngine.cpp:463"),
    f("WeaponStore", "Data\\INI\\Weapon.ini", true, "GameEngine.cpp:464"),
    f("ObjectCreationListStore", "Data\\INI\\Default\\ObjectCreationList.ini", true, "GameEngine.cpp:465"),
    f("ObjectCreationListStore", "Data\\INI\\ObjectCreationList.ini", true, "GameEngine.cpp:465"),
    f("LocomotorStore", "Data\\INI\\Locomotor.ini", true, "GameEngine.cpp:466"),
    f("SpecialPowerStore", "Data\\INI\\Default\\SpecialPower.ini", true, "GameEngine.cpp:467"),
    f("SpecialPowerStore", "Data\\INI\\SpecialPower.ini", true, "GameEngine.cpp:467"),
    f("DamageFXStore", "Data\\INI\\DamageFX.ini", true, "GameEngine.cpp:468"),
    f("ArmorStore", "Data\\INI\\Armor.ini", true, "GameEngine.cpp:469"),
    f("ThingFactory", "Data\\INI\\Default\\Object.ini", true, "GameEngine.cpp:482"),
    Step { subsystem: "ThingFactory", source: Source::Dir("Data\\INI\\Object"), crc: true, cpp: "GameEngine.cpp:482" },
    f("UpgradeCenter", "Data\\INI\\Default\\Upgrade.ini", true, "GameEngine.cpp:492"),
    f("UpgradeCenter", "Data\\INI\\Upgrade.ini", true, "GameEngine.cpp:492"),
    // -- GameClient::init (GameClient.cpp) --
    f("GameClient", "Data\\INI\\DrawGroupInfo.ini", false, "GameClient.cpp:256"),
    Step { subsystem: "MappedImages", source: Source::UserDir("INI\\MappedImages"), crc: false, cpp: "Image.cpp:245" },
    Step {
        subsystem: "MappedImages",
        source: Source::Dir("Data\\INI\\MappedImages\\TextureSize_512"),
        crc: false,
        cpp: "Image.cpp:254",
    },
    Step {
        subsystem: "MappedImages",
        source: Source::Dir("Data\\INI\\MappedImages\\HandCreated"),
        crc: false,
        cpp: "Image.cpp:256",
    },
    f("Anim2DCollection", "Data\\INI\\Animation2D.ini", false, "Anim2D.cpp:763"),
    f("Mouse", "Data\\INI\\Mouse.ini", false, "Mouse.cpp:559"),
    Step {
        subsystem: "HeaderTemplateManager",
        source: Source::LangFile("Data\\{lang}\\HeaderTemplate.ini"),
        crc: false,
        cpp: "HeaderTemplate.cpp:159",
    },
    f("WindowManager", "Data\\INI\\WindowTransitions.ini", false, "GameWindowTransitions.cpp:390"),
    // Shell::init loads the schemes, then ShellMenuSchemeManager::init loads them again.
    f("Shell", "Data\\INI\\Default\\ShellMenuScheme.ini", false, "Shell.cpp:148"),
    f("Shell", "Data\\INI\\ShellMenuScheme.ini", false, "Shell.cpp:149"),
    f("ShellMenuSchemeManager", "Data\\INI\\Default\\ShellMenuScheme.ini", false, "ShellMenuScheme.cpp:285"),
    f("ShellMenuSchemeManager", "Data\\INI\\ShellMenuScheme.ini", false, "ShellMenuScheme.cpp:286"),
    f("InGameUI", "Data\\INI\\InGameUI.ini", false, "InGameUI.cpp:1103"),
    f("ControlBar", "Data\\INI\\Default\\CommandButton.ini", false, "ControlBar.cpp:1055"),
    f("ControlBar", "Data\\INI\\CommandButton.ini", false, "ControlBar.cpp:1056"),
    f("ControlBar", "Data\\INI\\CommandSet.ini", false, "ControlBar.cpp:1059"),
    f("ControlBarSchemeManager", "Data\\INI\\Default\\ControlBarScheme.ini", false, "ControlBarScheme.cpp:1036"),
    f("ControlBarSchemeManager", "Data\\INI\\ControlBarScheme.ini", false, "ControlBarScheme.cpp:1037"),
    f("ChallengeGenerals", "Data\\INI\\ChallengeMode.ini", false, "ChallengeGenerals.cpp:63"),
    f("VideoPlayer", "Data\\INI\\Default\\Video.ini", false, "VideoPlayer.cpp:185"),
    f("VideoPlayer", "Data\\INI\\Video.ini", false, "VideoPlayer.cpp:186"),
    f("CampaignManager", "Data\\INI\\Campaign.ini", false, "CampaignManager.cpp:254"),
    f("Eva", "Data\\INI\\Eva.ini", false, "Eva.cpp:263"),
    // -- back in GameEngine::init --
    f("AI", "Data\\INI\\Default\\AIData.ini", true, "GameEngine.cpp:504"),
    f("AI", "Data\\INI\\AIData.ini", true, "GameEngine.cpp:504"),
    f("CrateSystem", "Data\\INI\\Default\\Crate.ini", true, "GameEngine.cpp:507"),
    f("CrateSystem", "Data\\INI\\Crate.ini", true, "GameEngine.cpp:507"),
    Step {
        subsystem: "MetaMap",
        source: Source::LangFile("Data\\{lang}\\CommandMap.ini"),
        crc: false,
        cpp: "GameEngine.cpp:525",
    },
    f("MetaMap", "Data\\INI\\CommandMap.ini", false, "GameEngine.cpp:525"),
];

/// Files loaded later, when a feature is first used.
pub static ON_DEMAND: &[Step] = &[
    f("Credits", "Data\\INI\\Credits.ini", false, "Credits.cpp:163"),
    f("WebBrowser", "Data\\INI\\Webpages.ini", false, "WebBrowser.cpp:175"),
    f("MapCache", "Maps\\MapCache.ini", false, "MapUtil.cpp:516"),
];

/// Per-map files, loaded with [`Mode::CreateOverrides`] when a map starts
/// (`GameLogic.cpp:2432`, `2439`).
pub const MAP_FILES: &[&str] = &["map.ini", "solo.ini"];

/// One concrete file to load.
#[derive(Debug, Clone)]
pub struct Planned {
    pub path: String,
    pub step: &'static Step,
    pub mode: Mode,
}

/// Expands the steps into concrete files, in load order. Missing required files
/// are returned separately; the original would abort on the first one.
pub fn plan(vfs: &Vfs, language: &str, steps: &'static [Step]) -> (Vec<Planned>, Vec<&'static Step>) {
    let mut files = Vec::new();
    let mut missing = Vec::new();
    for step in steps {
        match step.source {
            Source::File(p) => {
                if vfs.exists(p) {
                    files.push(Planned { path: p.to_string(), step, mode: Mode::Overwrite });
                } else {
                    missing.push(step);
                }
            }
            Source::LangFile(p) => {
                let path = p.replace("{lang}", language);
                if vfs.exists(&path) {
                    files.push(Planned { path, step, mode: Mode::Overwrite });
                } else {
                    missing.push(step);
                }
            }
            Source::Dir(d) => {
                for path in load_directory(vfs, d) {
                    files.push(Planned { path, step, mode: Mode::Overwrite });
                }
            }
            // The user data folder is outside the game's file system; callers that
            // know it mount it separately.
            Source::UserDir(_) => {}
        }
    }
    (files, missing)
}

/// `INI::loadDirectory`: files directly in `dir` first, then files in subfolders.
pub fn load_directory(vfs: &Vfs, dir: &str) -> Vec<String> {
    let prefix = format!("{dir}\\");
    let all = vfs.list(&prefix, "*.ini", true);
    let in_sub = |p: &String| p.get(prefix.len()..).is_some_and(|rest| rest.contains(['\\', '/']));
    let mut out: Vec<String> = all.iter().filter(|p| !in_sub(p)).cloned().collect();
    out.extend(all.iter().filter(|p| in_sub(p)).cloned());
    out
}

/// Every per-map INI file (`map.ini`, `solo.ini`) under `Maps`, sorted.
pub fn map_files(vfs: &Vfs) -> Vec<String> {
    let mut out = Vec::new();
    for name in MAP_FILES {
        out.extend(vfs.list("Maps", name, true));
    }
    out.sort_by(|a, b| ih_vfs::path::cmp_nocase(a, b));
    out
}
