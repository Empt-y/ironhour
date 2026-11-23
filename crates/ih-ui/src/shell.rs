//! The shell: the stack of menu screens and the menus' callbacks.
//!
//! Ported from `GameClient/GUI/Shell/Shell.cpp` (`push`, `pop`, hiding the
//! screen below), the menu callbacks in `GUICallbacks/Menus/` (`MainMenu.cpp`:
//! `MainMenuInit` hiding the drop-downs, faction windows and save buttons,
//! `MainMenuSystem` switching drop-downs and pushing menus;
//! `SkirmishGameOptionsMenu.cpp`: the slot, faction, colour, team and money
//! combo boxes, the map and start; `SkirmishMapSelectMenu.cpp`;
//! `OptionsMenu.cpp` and `CreditsMenu.cpp` back buttons), `GUIUtil.cpp`
//! (`PopulateColorComboBox`, `PopulatePlayerTemplateComboBox`) and the map
//! cache (`MapUtil.cpp`, `Maps\MapCache.ini`).
//!
//! Menus answer by name, as the original's function lexicon does: a screen is
//! handled by its layout's `LAYOUTINIT` and its root window's `SYSTEMCALLBACK`.
//! Menu transitions (`TheTransitionHandler`), the shell map, menu schemes,
//! campaign, challenge, network, replay and save screens, preferences and the
//! options themselves are not ported yet; their buttons do nothing.
use crate::input::{Input, Message, UiEvent};
use crate::{WindowId, WindowManager, state};
use ih_formats::wnd;

/// A map from `MapCache.ini`.
#[derive(Clone, Debug, PartialEq)]
pub struct MapInfo {
    /// `maps\tournament desert\tournament desert.map`.
    pub path: String,
    /// `nameLookupTag`, a string label (`MAP:TournamentDesert`), or the folder name.
    pub name_label: String,
    pub players: u32,
    pub multiplayer: bool,
    pub official: bool,
    /// `extentMin` / `extentMax` x and y: lo x, lo y, hi x, hi y.
    pub extent: [f32; 4],
    /// `Player_N_Start` waypoints, in order.
    pub starts: Vec<[f32; 2]>,
}
impl MapInfo {
    /// The map's folder name, as `World::load` takes it.
    pub fn folder(&self) -> String {
        let p = self.path.replace('/', "\\");
        let mut parts = p.split('\\').filter(|s| !s.is_empty());
        let _ = parts.next();
        parts.next().unwrap_or(&p).to_string()
    }
}

fn bname(b: &ih_ini::Block) -> String {
    b.args().first().cloned().unwrap_or_default()
}

/// Decodes the cache's escaped names (`_5C` is `\`, `_20` a space).
fn unescape(s: &str) -> String {
    let b = s.as_bytes();
    let mut out = Vec::with_capacity(b.len());
    let mut i = 0;
    while i < b.len() {
        if b[i] == b'_'
            && i + 2 < b.len() + 1
            && let Some(h) = s.get(i + 1..i + 3).and_then(|h| u8::from_str_radix(h, 16).ok())
        {
            out.push(h);
            i += 3;
        } else {
            out.push(b[i]);
            i += 1;
        }
    }
    String::from_utf8_lossy(&out).into_owned()
}

/// The maps in `MapCache.ini`, in file order.
pub fn map_cache(bytes: &[u8]) -> Vec<MapInfo> {
    let doc = ih_ini::parse(bytes);
    let mut out = vec![];
    for b in doc.blocks.iter().filter(|b| b.kind.eq_ignore_ascii_case("MapCache")) {
        let get = |k: &str| b.fields().filter(|f| f.name.eq_ignore_ascii_case(k)).last().map(|f| f.values());
        let first = |k: &str| get(k).and_then(|v| v.first().cloned()).unwrap_or_default();
        let path = unescape(&bname(b));
        let label = first("nameLookupTag");
        let xy = |k: &str| {
            let v = get(k).unwrap_or_default();
            let c =
                |a: &str| v.iter().find_map(|t| t.strip_prefix(a).and_then(|n| n.parse::<f32>().ok())).unwrap_or(0.0);
            [c("X:"), c("Y:")]
        };
        let (lo, hi) = (xy("extentMin"), xy("extentMax"));
        let mut starts = vec![];
        for n in 1..=8 {
            let k = format!("Player_{n}_Start");
            if get(&k).is_none() {
                break;
            }
            starts.push(xy(&k));
        }
        let m = MapInfo {
            name_label: if label.is_empty() { path.clone() } else { label },
            players: first("numPlayers").parse().unwrap_or(0),
            multiplayer: first("isMultiplayer").eq_ignore_ascii_case("yes"),
            official: first("isOfficial").eq_ignore_ascii_case("yes"),
            extent: [lo[0], lo[1], hi[0], hi[1]],
            starts,
            path,
        };
        out.push(m);
    }
    out
}

/// What a skirmish slot holds.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SlotKind {
    Human,
    Open,
    Closed,
    EasyAi,
    MediumAi,
    HardAi,
}

/// A slot of the skirmish setup.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Slot {
    pub kind: SlotKind,
    /// A `PlayerTemplate` name; `None` is random.
    pub side: Option<String>,
    /// An index into the multiplayer colours; `None` is random.
    pub color: Option<usize>,
    /// 1 to 4; `None` is no team.
    pub team: Option<u8>,
    /// A start position (0-based `Player_N_Start`); `None` is given out at the start.
    pub start: Option<u8>,
}

/// The skirmish being set up (`SkirmishGameInfo`).
#[derive(Clone, Debug, PartialEq)]
pub struct GameSetup {
    pub map: Option<MapInfo>,
    pub slots: Vec<Slot>,
    pub starting_cash: u32,
    pub limit_superweapons: bool,
    pub player_name: String,
}

impl GameSetup {
    /// Reserve explicit positions first, then give each remaining player the
    /// first free one. Inactive slots cannot reserve a position. Reject invalid
    /// or duplicate choices rather than spawning players on top of one another.
    pub fn assigned_starts(&self, count: usize) -> Option<Vec<Option<usize>>> {
        let mut used = vec![false; count];
        let mut positions = vec![None; self.slots.len()];
        for (i, slot) in self.slots.iter().enumerate() {
            if matches!(slot.kind, SlotKind::Open | SlotKind::Closed) {
                continue;
            }
            if let Some(p) = slot.start {
                let p = usize::from(p);
                if *used.get(p)? {
                    return None;
                }
                used[p] = true;
                positions[i] = Some(p);
            }
        }
        for (i, slot) in self.slots.iter().enumerate() {
            if matches!(slot.kind, SlotKind::Open | SlotKind::Closed) || positions[i].is_some() {
                continue;
            }
            let p = used.iter().position(|taken| !taken)?;
            used[p] = true;
            positions[i] = Some(p);
        }
        Some(positions)
    }
}

/// What the shell asks of the game.
#[derive(Clone, Debug, PartialEq)]
pub enum ShellEvent {
    StartGame(GameSetup),
    Quit,
    ReturnToMain,
    /// An interface sound to play.
    Sound(String),
    AudioPreview([u8; 3]),
    AudioSave([u8; 3]),
    InterfacePreview([u32; 3]),
    InterfaceSave([u32; 3]),
}

/// Data the menus offer.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct ShellData {
    /// Playable sides: (`PlayerTemplate` name, display label).
    pub sides: Vec<(String, String)>,
    /// `MultiplayerColor`s: (tooltip label, RGB).
    pub colors: Vec<(String, [u8; 3])>,
    /// `StartingMoneyChoice`s, and the default.
    pub cash: Vec<u32>,
    pub default_cash: u32,
    pub maps: Vec<MapInfo>,
}
impl ShellData {
    /// Sides, colours and money from the definitions (`PlayerTemplate.ini`,
    /// `Multiplayer.ini`), maps from the map cache.
    pub fn load(defs: &ih_ini::definitions::Definitions, map_cache_bytes: Option<&[u8]>) -> Self {
        let mut d = ShellData::default();
        for b in defs.ordered("PlayerTemplate") {
            let get = |k: &str| b.fields().filter(|f| f.name == k).last().and_then(|f| f.values().first().cloned());
            if get("PlayableSide").is_some_and(|v| v.eq_ignore_ascii_case("yes"))
                && get("Side").is_some_and(|s| !s.eq_ignore_ascii_case("Observer"))
            {
                d.sides.push((bname(b), get("DisplayName").unwrap_or_else(|| bname(b))));
            }
        }
        for b in defs.ordered("MultiplayerColor") {
            let get = |k: &str| b.fields().filter(|f| f.name == k).last().map(|f| f.values());
            let rgb = get("RGBColor").unwrap_or_default();
            let c = |k: &str| {
                rgb.iter()
                    .find_map(|t| t.strip_prefix(&format!("{k}:")).and_then(|v| v.parse::<u8>().ok()))
                    .unwrap_or(255)
            };
            let label = get("TooltipName").and_then(|v| v.first().cloned()).unwrap_or_else(|| bname(b));
            d.colors.push((label, [c("R"), c("G"), c("B")]));
        }
        for b in defs.ordered("MultiplayerSettings") {
            for f in b.fields().filter(|f| f.name == "StartingMoneyChoice") {
                let v = f.values();
                if let Some(n) = v.first().and_then(|s| s.parse().ok()) {
                    d.cash.push(n);
                    if v.iter().any(|t| t.eq_ignore_ascii_case("Default")) {
                        d.default_cash = n;
                    }
                }
            }
        }
        if d.cash.is_empty() {
            d.cash = vec![5000, 10000, 20000, 50000];
        }
        if d.default_cash == 0 {
            d.default_cash = 10000;
        }
        if let Some(b) = map_cache_bytes {
            d.maps = map_cache(b).into_iter().filter(|m| m.multiplayer && m.players >= 2).collect();
        }
        d
    }
}

/// Loads a layout by its shell name (`Menus/MainMenu.wnd`).
pub type LayoutLoader = Box<dyn Fn(&str) -> Option<wnd::Layout>>;

/// A screen on the stack: its layout and its top-level windows.
#[derive(Clone, Debug)]
struct Screen {
    init: String,
    windows: Vec<WindowId>,
}

/// `TheShell`.
pub struct Shell {
    pub audio_volumes: [u8; 3],
    pub interface_options: [u32; 3],
    interface_original: [u32; 3],
    resolutions: Vec<[u32; 2]>,
    options_original: [u8; 3],
    pub ui: WindowManager,
    pub setup: GameSetup,
    pub data: ShellData,
    stack: Vec<Screen>,
    loader: LayoutLoader,
    strings: Box<dyn Fn(&str) -> String>,
    /// Screens the original shows but are not ported, by layout name (for reports).
    pub missing: Vec<String>,
}

const MAIN: &str = "Menus/MainMenu.wnd";
const SKIRMISH: &str = "Menus/SkirmishGameOptionsMenu.wnd";
const MAP_SELECT: &str = "Menus/SkirmishMapSelectMenu.wnd";

impl Shell {
    pub fn new(display: [i32; 2], data: ShellData, loader: LayoutLoader, strings: Box<dyn Fn(&str) -> String>) -> Self {
        let slot = |kind| Slot { kind, side: None, color: None, team: None, start: None };
        let mut slots = vec![slot(SlotKind::Human), slot(SlotKind::EasyAi)];
        slots.resize(8, slot(SlotKind::Closed));
        let map = data
            .maps
            .iter()
            .find(|m| m.path.to_ascii_lowercase().contains("tournament desert"))
            .or(data.maps.first())
            .cloned();
        let setup = GameSetup {
            map,
            slots,
            starting_cash: data.default_cash,
            limit_superweapons: false,
            player_name: strings("GUI:Player"),
        };
        let mut s = Shell {
            audio_volumes: [80, 70, 55],
            interface_options: [1280, 800, 50],
            interface_original: [1280, 800, 50],
            resolutions: vec![
                [800, 600],
                [1024, 768],
                [1280, 720],
                [1280, 800],
                [1600, 900],
                [1920, 1080],
                [2560, 1440],
            ],
            options_original: [80, 70, 55],
            ui: WindowManager::new(display),
            setup,
            data,
            stack: vec![],
            loader,
            strings,
            missing: vec![],
        };
        s.push(MAIN);
        s
    }

    /// The layout on top (`Menus/MainMenu.wnd`).
    pub fn resize(&mut self, display: [i32; 2]) {
        self.ui.resize(display);
        if self.top() == Some(SKIRMISH) {
            self.skirmish_show_map();
        }
    }

    pub fn top(&self) -> Option<&str> {
        self.stack.last().map(|s| s.init.as_str())
    }

    fn find(&self, name: &str) -> Option<WindowId> {
        self.ui.find(name)
    }
    fn hide(&mut self, name: &str, hidden: bool) {
        if let Some(id) = self.find(name) {
            self.ui.hide(id, hidden);
        }
    }

    /// `Shell::push`: hides the screen below and shows a new one.
    pub fn push(&mut self, layout: &str) {
        let Some(l) = (self.loader)(layout) else {
            self.missing.push(layout.to_string());
            return;
        };
        if let Some(top) = self.stack.last() {
            for &w in &top.windows.clone() {
                self.ui.hide(w, true);
            }
        }
        let windows = self.ui.load(&l);
        self.ui.init_gadgets();
        self.stack.push(Screen { init: layout.to_string(), windows });
        self.init(layout);
    }

    /// `Shell::pop`: removes the top screen and shows the one below.
    pub fn pop(&mut self) {
        let Some(top) = self.stack.pop() else { return };
        self.ui.unload(&top.windows);
        if let Some(below) = self.stack.last() {
            for &w in &below.windows.clone() {
                self.ui.hide(w, false);
            }
            let name = below.init.clone();
            self.refresh(&name);
        }
    }

    fn init(&mut self, layout: &str) {
        match layout {
            MAIN => {
                // MainMenuInit: only the main drop-down; no faction art or save buttons.
                for d in ["MapBorder", "MapBorder1", "MapBorder3", "MapBorder4"] {
                    self.hide(&format!("MainMenu.wnd:{d}"), true);
                }
                self.hide("MainMenu.wnd:MapBorder2", false);
                let names: Vec<String> = self
                    .ui
                    .windows
                    .iter()
                    .filter_map(|w| w.name.clone())
                    .filter(|n| {
                        let n = n.to_ascii_lowercase();
                        n.starts_with("mainmenu.wnd:winfaction")
                            || n == "mainmenu.wnd:wingrowmarker"
                            || n.ends_with("recentsave")
                            || n.ends_with("loadgame") && n != "mainmenu.wnd:buttonloadgame"
                    })
                    .collect();
                for n in names {
                    self.hide(&n, true);
                }
            }
            SKIRMISH => self.skirmish_init(),
            MAP_SELECT => self.map_select_init(),
            "Menus/OptionsMenu.wnd" => {
                self.options_original = self.audio_volumes;
                self.interface_original = self.interface_options;
                let current = [self.interface_options[0], self.interface_options[1]];
                if !self.resolutions.contains(&current) {
                    self.resolutions.push(current);
                }
                let row = self.resolutions.iter().position(|r| *r == current).unwrap_or(0);
                self.rows(
                    "OptionsMenu.wnd:ComboBoxResolution",
                    self.resolutions.iter().map(|r| format!("{} x {}", r[0], r[1])).collect(),
                    row,
                );
                if let Some(id) = self.find("OptionsMenu.wnd:SliderScrollSpeed") {
                    let g = &mut self.ui.windows[id].gadget;
                    g.min = 0;
                    g.max = 100;
                    g.value = self.interface_options[2] as i32;
                }
                if let Some(id) = self.find("OptionsMenu.wnd:NetworkParent") {
                    self.ui.hide(id, true);
                }
                for w in &mut self.ui.windows {
                    if w.name.as_deref().is_some_and(|n| n.starts_with("OptionsMenu.wnd:"))
                        && matches!(
                            w.kind.as_str(),
                            "HORZSLIDER"
                                | "VERTSLIDER"
                                | "COMBOBOX"
                                | "CHECKBOX"
                                | "RADIOBUTTON"
                                | "PUSHBUTTON"
                                | "ENTRYFIELD"
                        )
                    {
                        let n = w.name.as_deref().unwrap();
                        let enabled = [
                            "SliderVoiceVolume",
                            "SliderMusicVolume",
                            "SliderSFXVolume",
                            "SliderScrollSpeed",
                            "ComboBoxResolution",
                            "ButtonAccept",
                            "ButtonBack",
                            "ButtonDefaults",
                        ]
                        .iter()
                        .any(|key| n.ends_with(&format!(":{key}")));
                        if !enabled {
                            w.status &= !crate::status::ENABLED;
                        }
                    }
                }
                for (name, value) in
                    ["SliderSFXVolume", "SliderVoiceVolume", "SliderMusicVolume"].into_iter().zip(self.audio_volumes)
                {
                    if let Some(id) = self.find(&format!("OptionsMenu.wnd:{name}")) {
                        let g = &mut self.ui.windows[id].gadget;
                        g.min = 0;
                        g.max = 100;
                        g.value = value as i32;
                    }
                }
            }
            _ => {}
        }
    }

    /// A screen shown again after the one above it closed.
    fn refresh(&mut self, layout: &str) {
        if layout == SKIRMISH {
            self.skirmish_show_map();
        }
    }

    fn rows(&mut self, name: &str, rows: Vec<String>, selected: usize) {
        if let Some(id) = self.find(name) {
            let g = &mut self.ui.windows[id].gadget;
            g.rows = rows.into_iter().map(|r| vec![r]).collect();
            g.selected = (selected < g.rows.len()).then_some(selected);
        }
    }

    fn skirmish_init(&mut self) {
        let t = |s: &Shell, l: &str| (s.strings)(l);
        if let Some(e) = self.find("SkirmishGameOptionsMenu.wnd:TextEntryPlayerName") {
            self.ui.windows[e].gadget.entry = self.setup.player_name.clone();
        }
        let sides: Vec<String> =
            std::iter::once(t(self, "GUI:Random")).chain(self.data.sides.iter().map(|(_, l)| t(self, l))).collect();
        let colors: Vec<String> =
            std::iter::once(t(self, "GUI:Random")).chain(self.data.colors.iter().map(|(l, _)| t(self, l))).collect();
        let teams: Vec<String> = std::iter::once(t(self, "GUI:None"))
            .chain((1..=4).map(|i| format!("{} {i}", t(self, "GUI:Team"))))
            .collect();
        let kinds = ["GUI:Open", "GUI:Closed", "GUI:EasyAI", "GUI:MediumAI", "GUI:HardAI"];
        for i in 0..8 {
            let slot = self.setup.slots[i].clone();
            if i > 0 {
                let rows = kinds.iter().map(|k| t(self, k)).collect();
                let sel = match slot.kind {
                    SlotKind::Open | SlotKind::Human => 0,
                    SlotKind::Closed => 1,
                    SlotKind::EasyAi => 2,
                    SlotKind::MediumAi => 3,
                    SlotKind::HardAi => 4,
                };
                self.rows(&format!("SkirmishGameOptionsMenu.wnd:ComboBoxPlayer{i}"), rows, sel);
            }
            let side =
                slot.side.as_ref().and_then(|s| self.data.sides.iter().position(|(n, _)| n == s)).map_or(0, |p| p + 1);
            self.rows(&format!("SkirmishGameOptionsMenu.wnd:ComboBoxPlayerTemplate{i}"), sides.clone(), side);
            self.rows(
                &format!("SkirmishGameOptionsMenu.wnd:ComboBoxColor{i}"),
                colors.clone(),
                slot.color.map_or(0, |c| c + 1),
            );
            self.rows(
                &format!("SkirmishGameOptionsMenu.wnd:ComboBoxTeam{i}"),
                teams.clone(),
                slot.team.map_or(0, usize::from),
            );
        }
        let cash: Vec<String> = self.data.cash.iter().map(|c| format!("${c}")).collect();
        let sel = self.data.cash.iter().position(|&c| c == self.setup.starting_cash).unwrap_or(0);
        self.rows("SkirmishGameOptionsMenu.wnd:ComboBoxStartingCash", cash, sel);
        self.skirmish_show_map();
    }

    fn map_name(&self, m: &MapInfo) -> String {
        if m.name_label.contains(':') { (self.strings)(&m.name_label) } else { m.folder() }
    }

    fn skirmish_show_map(&mut self) {
        let name = self.setup.map.as_ref().map(|m| self.map_name(m)).unwrap_or_default();
        // A static text despite its name (GadgetStaticTextSetText).
        if let Some(e) = self.find("SkirmishGameOptionsMenu.wnd:TextEntryMapDisplay") {
            self.ui.windows[e].text = Some(name);
        }
        self.position_start_spots();
    }

    /// `positionStartSpots`: the preview letterboxed to the map's extent
    /// (`findDrawPositions`), and a marker per `Player_N_Start`, numbered with
    /// the slot that has it.
    fn position_start_spots(&mut self) {
        let Some(map_win) = self.find("SkirmishGameOptionsMenu.wnd:MapWindow") else { return };
        let Some(m) = self.setup.map.clone() else { return };
        let size = self.ui.windows[map_win].size;
        let (w, h) = ((m.extent[2] - m.extent[0]).max(1.0), (m.extent[3] - m.extent[1]).max(1.0));
        let (rw, rh) = (w / size[0].max(1) as f32, h / size[1].max(1) as f32);
        let (ul, lr) = if rw >= rh {
            let ry = h / rw;
            let uy = ((size[1] as f32 - ry) / 2.0) as i32;
            ([0, uy], [(w / rw) as i32, size[1] - uy])
        } else {
            let rx = w / rh;
            let ux = ((size[0] as f32 - rx) / 2.0) as i32;
            ([ux, 0], [size[0] - ux, (h / rh) as i32])
        };
        let folder = m.folder();
        self.ui.windows[map_win].image = Some(format!("Maps\\{folder}\\{folder}.tga"));
        self.ui.windows[map_win].image_rect = [ul[0], ul[1], lr[0], lr[1]];
        let origin = self.ui.screen_position(map_win);
        for i in 0..8 {
            let Some(b) = self.find(&format!("SkirmishGameOptionsMenu.wnd:ButtonMapStartPosition{i}")) else {
                continue;
            };
            let Some(p) = m.starts.get(i).filter(|_| m.multiplayer) else {
                self.ui.hide(b, true);
                continue;
            };
            self.ui.hide(b, false);
            let gs = self.ui.windows[b].size;
            let fx = (p[0] - m.extent[0]) / w.max(1.0);
            let fy = (p[1] - m.extent[1]) / h.max(1.0);
            let pos = [
                (fx * (lr[0] - ul[0]) as f32) as i32 - gs[0] / 2 + ul[0],
                ((1.0 - fy) * (lr[1] - ul[1]) as f32) as i32 - gs[1] / 2 + ul[1],
            ];
            // Relative to the marker's parent.
            let parent = self.ui.windows[b].parent.map_or([0, 0], |q| self.ui.screen_position(q));
            self.ui.windows[b].position = [origin[0] + pos[0] - parent[0], origin[1] + pos[1] - parent[1]];
            let holder = self.setup.slots.iter().position(|s| s.start == Some(i as u8));
            self.ui.windows[b].text = Some(holder.map_or(String::new(), |h| (h + 1).to_string()));
        }
    }

    /// `getNextSelectablePlayer`: the first playing slot from `start` without a position.
    fn next_selectable(&self, start: usize) -> Option<usize> {
        (start..self.setup.slots.len()).find(|&j| {
            let s = &self.setup.slots[j];
            s.start.is_none() && !matches!(s.kind, SlotKind::Open | SlotKind::Closed)
        })
    }

    /// A start position marker was clicked: a free one goes to the next player
    /// without a position; a taken one passes to the next such player after its holder.
    fn start_spot_clicked(&mut self, i: u8) {
        if let Some(holder) = self.setup.slots.iter().position(|s| s.start == Some(i)) {
            let next = self.next_selectable(holder + 1);
            self.setup.slots[holder].start = None;
            if let Some(n) = next {
                self.setup.slots[n].start = Some(i);
            }
        } else if let Some(n) = self.next_selectable(0).or(Some(0)) {
            self.setup.slots[n].start = Some(i);
        }
        self.position_start_spots();
    }

    /// The slot settings as the combo boxes show them.
    fn skirmish_read(&mut self) {
        for i in 0..8 {
            let sel = |s: &Shell, n: String| s.find(&n).and_then(|id| s.ui.windows[id].gadget.selected).unwrap_or(0);
            if i > 0 {
                self.setup.slots[i].kind = match sel(self, format!("SkirmishGameOptionsMenu.wnd:ComboBoxPlayer{i}")) {
                    0 => SlotKind::Open,
                    2 => SlotKind::EasyAi,
                    3 => SlotKind::MediumAi,
                    4 => SlotKind::HardAi,
                    _ => SlotKind::Closed,
                };
            }
            if matches!(self.setup.slots[i].kind, SlotKind::Open | SlotKind::Closed) {
                self.setup.slots[i].start = None;
            }
            let side = sel(self, format!("SkirmishGameOptionsMenu.wnd:ComboBoxPlayerTemplate{i}"));
            self.setup.slots[i].side = side.checked_sub(1).and_then(|s| self.data.sides.get(s)).map(|(n, _)| n.clone());
            let color = sel(self, format!("SkirmishGameOptionsMenu.wnd:ComboBoxColor{i}"));
            self.setup.slots[i].color = color.checked_sub(1);
            let team = sel(self, format!("SkirmishGameOptionsMenu.wnd:ComboBoxTeam{i}"));
            self.setup.slots[i].team = (team > 0).then_some(team as u8);
        }
        if let Some(id) = self.find("SkirmishGameOptionsMenu.wnd:ComboBoxStartingCash")
            && let Some(c) = self.ui.windows[id].gadget.selected.and_then(|i| self.data.cash.get(i))
        {
            self.setup.starting_cash = *c;
        }
        if let Some(id) = self.find("SkirmishGameOptionsMenu.wnd:CheckboxLimitSuperweapons") {
            self.setup.limit_superweapons = self.ui.windows[id].state & state::SELECTED != 0;
        }
        if let Some(e) = self.find("SkirmishGameOptionsMenu.wnd:TextEntryPlayerName") {
            self.setup.player_name = self.ui.windows[e].gadget.entry.clone();
        }
    }

    fn map_select_init(&mut self) {
        let names: Vec<String> =
            self.data.maps.iter().map(|m| format!("{} ({})", self.map_name(m), m.players)).collect();
        let sel = self.setup.map.as_ref().and_then(|m| self.data.maps.iter().position(|x| x == m)).unwrap_or(0);
        if let Some(id) = self.find("SkirmishMapSelectMenu.wnd:ListboxMap") {
            let g = &mut self.ui.windows[id].gadget;
            g.rows = names.into_iter().map(|n| vec![n]).collect();
            g.selected = Some(sel).filter(|&s| s < g.rows.len());
        }
    }

    fn read_interface_options(&mut self) -> [u32; 3] {
        if let Some(id) = self.find("OptionsMenu.wnd:SliderScrollSpeed") {
            self.interface_options[2] = self.ui.windows[id].gadget.value.clamp(0, 100) as u32;
        }
        if let Some(id) = self.find("OptionsMenu.wnd:ComboBoxResolution")
            && let Some(r) = self.ui.windows[id].gadget.selected.and_then(|i| self.resolutions.get(i))
        {
            self.interface_options[0] = r[0];
            self.interface_options[1] = r[1];
        }
        self.interface_options
    }

    fn read_audio_options(&mut self) -> [u8; 3] {
        for (i, name) in ["SliderSFXVolume", "SliderVoiceVolume", "SliderMusicVolume"].into_iter().enumerate() {
            if let Some(id) = self.find(&format!("OptionsMenu.wnd:{name}")) {
                self.audio_volumes[i] = self.ui.windows[id].gadget.value.clamp(0, 100) as u8;
            }
        }
        self.audio_volumes
    }

    /// Feeds input to the window system and the menus; returns what the game must do.
    pub fn input(&mut self, input: &Input) -> Vec<ShellEvent> {
        let mut out = vec![];
        for e in self.ui.input(input) {
            match e {
                UiEvent::Sound(s) => out.push(ShellEvent::Sound(s)),
                UiEvent::System { message: Message::SliderTrack(..), .. }
                    if self.top() == Some("Menus/OptionsMenu.wnd") =>
                {
                    let values = self.read_audio_options();
                    out.push(ShellEvent::AudioPreview(values));
                    let values = self.read_interface_options();
                    out.push(ShellEvent::InterfacePreview(values));
                }
                UiEvent::System { message: Message::Selected(control), .. } => {
                    if let Some(name) = self.ui.windows[control].name.clone() {
                        self.selected(&name, &mut out);
                    }
                }
                UiEvent::System { message: Message::ListSelected(..), .. } if self.top() == Some(SKIRMISH) => {
                    self.skirmish_read();
                    self.position_start_spots();
                }
                _ => {}
            }
        }
        out
    }

    /// The menus' `GBM_SELECTED` handling, by control name.
    fn selected(&mut self, name: &str, out: &mut Vec<ShellEvent>) {
        let n = name.to_ascii_lowercase();
        let drop = |s: &mut Shell, show: &str, hide: &str| {
            s.hide(&format!("MainMenu.wnd:{show}"), false);
            s.hide(&format!("MainMenu.wnd:{hide}"), true);
        };
        match n.as_str() {
            "mainmenu.wnd:buttonsingleplayer" => drop(self, "MapBorder", "MapBorder2"),
            "mainmenu.wnd:buttonsingleback" => drop(self, "MapBorder2", "MapBorder"),
            "mainmenu.wnd:buttonmultiplayer" => drop(self, "MapBorder1", "MapBorder2"),
            "mainmenu.wnd:buttonmultiback" => drop(self, "MapBorder2", "MapBorder1"),
            "mainmenu.wnd:buttonloadreplay" => drop(self, "MapBorder3", "MapBorder2"),
            "mainmenu.wnd:buttonloadreplayback" => drop(self, "MapBorder2", "MapBorder3"),
            "mainmenu.wnd:buttonskirmish" => self.push(SKIRMISH),
            "mainmenu.wnd:buttonoptions" => self.push("Menus/OptionsMenu.wnd"),
            "mainmenu.wnd:buttoncredits" => self.push("Menus/CreditsMenu.wnd"),
            "mainmenu.wnd:buttonexit" => out.push(ShellEvent::Quit),
            "scorescreen.wnd:buttonok" | "scorescreen.wnd:buttoncontinue" => out.push(ShellEvent::ReturnToMain),
            "optionsmenu.wnd:buttondefaults" => {
                self.audio_volumes = [80, 70, 55];
                self.interface_options = [1280, 800, 50];
                let original = self.options_original;
                let interface = self.interface_original;
                self.init("Menus/OptionsMenu.wnd");
                self.options_original = original;
                self.interface_original = interface;
                out.push(ShellEvent::AudioPreview(self.audio_volumes));
                out.push(ShellEvent::InterfacePreview(self.interface_options));
            }
            "optionsmenu.wnd:buttonaccept" => {
                let values = self.read_audio_options();
                out.push(ShellEvent::AudioSave(values));
                let values = self.read_interface_options();
                out.push(ShellEvent::InterfaceSave(values));
                self.pop();
            }
            "optionsmenu.wnd:buttonback" => {
                self.audio_volumes = self.options_original;
                self.interface_options = self.interface_original;
                out.push(ShellEvent::InterfacePreview(self.interface_options));
                out.push(ShellEvent::AudioPreview(self.audio_volumes));
                self.pop();
            }
            "skirmishgameoptionsmenu.wnd:buttonback"
            | "skirmishmapselectmenu.wnd:buttonback"
            | "creditsmenu.wnd:buttonback" => self.pop(),
            n if n.starts_with("skirmishgameoptionsmenu.wnd:buttonmapstartposition") => {
                if let Ok(i) = n.trim_start_matches("skirmishgameoptionsmenu.wnd:buttonmapstartposition").parse::<u8>()
                {
                    self.skirmish_read();
                    self.start_spot_clicked(i);
                }
            }
            "skirmishgameoptionsmenu.wnd:buttonselectmap" => {
                self.skirmish_read();
                self.push(MAP_SELECT);
            }
            "skirmishmapselectmenu.wnd:buttonok" => {
                if let Some(id) = self.find("SkirmishMapSelectMenu.wnd:ListboxMap")
                    && let Some(m) = self.ui.windows[id].gadget.selected.and_then(|i| self.data.maps.get(i))
                {
                    if self.setup.map.as_ref() != Some(m) {
                        for s in &mut self.setup.slots {
                            s.start = None;
                        }
                    }
                    self.setup.map = Some(m.clone());
                }
                self.pop();
            }
            "skirmishgameoptionsmenu.wnd:buttonstart" => {
                self.skirmish_read();
                let playing =
                    self.setup.slots.iter().filter(|s| !matches!(s.kind, SlotKind::Open | SlotKind::Closed)).count();
                let fits =
                    self.setup.map.as_ref().is_some_and(|m| self.setup.assigned_starts(m.players as usize).is_some());
                if playing >= 2 && fits {
                    out.push(ShellEvent::StartGame(self.setup.clone()));
                }
            }
            _ => {}
        }
    }

    /// Unnamed helper for tests and tools: click a window by name (press and release at its centre).
    pub fn click(&mut self, name: &str) -> Vec<ShellEvent> {
        let Some(id) = self.find(name) else { return vec![] };
        let o = self.ui.screen_position(id);
        let s = self.ui.windows[id].size;
        let p = [o[0] + s[0] / 2, o[1] + s[1] / 2];
        let mut out = self.input(&Input::MouseMove(p));
        out.extend(self.input(&Input::LeftDown(p)));
        out.extend(self.input(&Input::LeftUp(p)));
        out
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn audio_options_preview_cancel_and_accept() {
        let source = br#"WINDOW
WINDOWTYPE = USER;
SCREENRECT = UPPERLEFT: 0 0, BOTTOMRIGHT: 800 600, CREATIONRESOLUTION: 800 600;
NAME = "OptionsMenu.wnd:Root";
STATUS = ENABLED;
SYSTEMCALLBACK = "OptionsMenuSystem";
CHILD
WINDOW
WINDOWTYPE = HORZSLIDER;
SCREENRECT = UPPERLEFT: 10 10, BOTTOMRIGHT: 110 30, CREATIONRESOLUTION: 800 600;
NAME = "OptionsMenu.wnd:SliderSFXVolume";
STATUS = ENABLED;
END
ENDALLCHILDREN
END
"#;
        let mut shell = Shell::new([800, 600], ShellData::default(), Box::new(|_| None), Box::new(str::to_string));
        shell.loader = Box::new(move |_| Some(wnd::parse(source).unwrap()));
        shell.push("Menus/OptionsMenu.wnd");
        let out = shell.click("OptionsMenu.wnd:SliderSFXVolume");
        assert!(out.contains(&ShellEvent::AudioPreview([50, 70, 55])));
        let mut out = vec![];
        shell.selected("OptionsMenu.wnd:ButtonBack", &mut out);
        assert!(out.contains(&ShellEvent::AudioPreview([80, 70, 55])));
        shell.push("Menus/OptionsMenu.wnd");
        shell.click("OptionsMenu.wnd:SliderSFXVolume");
        out.clear();
        shell.selected("OptionsMenu.wnd:ButtonAccept", &mut out);
        assert!(out.contains(&ShellEvent::AudioSave([50, 70, 55])));
    }

    fn setup() -> GameSetup {
        GameSetup {
            map: None,
            slots: [SlotKind::Human, SlotKind::EasyAi, SlotKind::Closed]
                .into_iter()
                .map(|kind| Slot { kind, side: None, color: None, team: None, start: None })
                .collect(),
            starting_cash: 10000,
            limit_superweapons: false,
            player_name: "Player".into(),
        }
    }

    #[test]
    fn explicit_starts_are_reserved_before_unassigned_players() {
        let mut s = setup();
        s.slots[1].start = Some(0);
        s.slots[2].start = Some(1); // A closed slot must not reserve this start.
        assert_eq!(s.assigned_starts(2), Some(vec![Some(1), Some(0), None]));
        s.slots[0].start = Some(0);
        assert_eq!(s.assigned_starts(2), None); // Duplicate choice.
        s.slots[0].start = Some(2);
        assert_eq!(s.assigned_starts(2), None); // Outside the map.
        s.slots[0].start = None;
        assert_eq!(s.assigned_starts(1), None); // Too many players.
    }

    #[test]
    fn start_marker_cycles_between_players_then_clears() {
        let mut shell = Shell::new([800, 600], ShellData::default(), Box::new(|_| None), Box::new(str::to_string));
        shell.start_spot_clicked(1);
        assert_eq!(shell.setup.slots[0].start, Some(1));
        shell.start_spot_clicked(1);
        assert_eq!(shell.setup.slots[0].start, None);
        assert_eq!(shell.setup.slots[1].start, Some(1));
        shell.start_spot_clicked(1);
        assert!(shell.setup.slots.iter().all(|s| s.start.is_none()));
    }

    #[test]
    fn map_cache_has_preview_extents_and_start_waypoints() {
        let maps = map_cache(b"MapCache maps_5Cx_5Cx_2Emap\n extentMin = X:10 Y:20 Z:0\n extentMax = X:410 Y:220 Z:0\n Player_1_Start = X:50 Y:75 Z:0\n Player_2_Start = X:350 Y:175 Z:0\nEND\n");
        assert_eq!(maps[0].extent, [10.0, 20.0, 410.0, 220.0]);
        assert_eq!(maps[0].starts, vec![[50.0, 75.0], [350.0, 175.0]]);
    }

    #[test]
    fn map_cache_names_unescape() {
        let ini = b"MapCache maps_5Ctournament_20desert_5Ctournament_20desert_2Emap\n  isMultiplayer = yes\n  isOfficial = yes\n  numPlayers = 2\n  nameLookupTag = MAP:TournamentDesert\nEND\n";
        let m = map_cache(ini);
        assert_eq!(m.len(), 1);
        assert_eq!(m[0].path, "maps\\tournament desert\\tournament desert.map");
        assert_eq!(m[0].folder(), "tournament desert");
        assert_eq!((m[0].players, m[0].multiplayer, m[0].name_label.as_str()), (2, true, "MAP:TournamentDesert"));
    }
}
