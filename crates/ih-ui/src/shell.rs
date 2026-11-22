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
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct MapInfo {
    /// `maps\tournament desert\tournament desert.map`.
    pub path: String,
    /// `nameLookupTag`, a string label (`MAP:TournamentDesert`), or the folder name.
    pub name_label: String,
    pub players: u32,
    pub multiplayer: bool,
    pub official: bool,
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
        let m = MapInfo {
            name_label: if label.is_empty() { path.clone() } else { label },
            players: first("numPlayers").parse().unwrap_or(0),
            multiplayer: first("isMultiplayer").eq_ignore_ascii_case("yes"),
            official: first("isOfficial").eq_ignore_ascii_case("yes"),
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
}

/// The skirmish being set up (`SkirmishGameInfo`).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct GameSetup {
    pub map: Option<MapInfo>,
    pub slots: Vec<Slot>,
    pub starting_cash: u32,
    pub limit_superweapons: bool,
    pub player_name: String,
}

/// What the shell asks of the game.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ShellEvent {
    StartGame(GameSetup),
    Quit,
    /// An interface sound to play.
    Sound(String),
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
        let slot = |kind| Slot { kind, side: None, color: None, team: None };
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
        let mut s =
            Shell { ui: WindowManager::new(display), setup, data, stack: vec![], loader, strings, missing: vec![] };
        s.push(MAIN);
        s
    }

    /// The layout on top (`Menus/MainMenu.wnd`).
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

    /// Feeds input to the window system and the menus; returns what the game must do.
    pub fn input(&mut self, input: &Input) -> Vec<ShellEvent> {
        let mut out = vec![];
        for e in self.ui.input(input) {
            match e {
                UiEvent::Sound(s) => out.push(ShellEvent::Sound(s)),
                UiEvent::System { message: Message::Selected(control), .. } => {
                    if let Some(name) = self.ui.windows[control].name.clone() {
                        self.selected(&name, &mut out);
                    }
                }
                UiEvent::System { message: Message::ListSelected(..), .. } if self.top() == Some(SKIRMISH) => {
                    self.skirmish_read();
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
            "skirmishgameoptionsmenu.wnd:buttonback"
            | "skirmishmapselectmenu.wnd:buttonback"
            | "optionsmenu.wnd:buttonback"
            | "optionsmenu.wnd:buttonaccept"
            | "creditsmenu.wnd:buttonback" => self.pop(),
            "skirmishgameoptionsmenu.wnd:buttonselectmap" => {
                self.skirmish_read();
                self.push(MAP_SELECT);
            }
            "skirmishmapselectmenu.wnd:buttonok" => {
                if let Some(id) = self.find("SkirmishMapSelectMenu.wnd:ListboxMap")
                    && let Some(m) = self.ui.windows[id].gadget.selected.and_then(|i| self.data.maps.get(i))
                {
                    self.setup.map = Some(m.clone());
                }
                self.pop();
            }
            "skirmishgameoptionsmenu.wnd:buttonstart" => {
                self.skirmish_read();
                let playing =
                    self.setup.slots.iter().filter(|s| !matches!(s.kind, SlotKind::Open | SlotKind::Closed)).count();
                let fits = self.setup.map.as_ref().is_some_and(|m| m.players as usize >= playing);
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
    fn map_cache_names_unescape() {
        let ini = b"MapCache maps_5Ctournament_20desert_5Ctournament_20desert_2Emap\n  isMultiplayer = yes\n  isOfficial = yes\n  numPlayers = 2\n  nameLookupTag = MAP:TournamentDesert\nEND\n";
        let m = map_cache(ini);
        assert_eq!(m.len(), 1);
        assert_eq!(m[0].path, "maps\\tournament desert\\tournament desert.map");
        assert_eq!(m[0].folder(), "tournament desert");
        assert_eq!((m[0].players, m[0].multiplayer, m[0].name_label.as_str()), (2, true, "MAP:TournamentDesert"));
    }
}
