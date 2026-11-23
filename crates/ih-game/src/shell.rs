//! The menus in the game window: the shell over the map, its input and drawing,
//! and turning a skirmish setup into a game.
//!
//! The game start follows `GameLogic::startNewGame` loosely: each playing slot
//! gets its side's `StartingBuilding` at the map's `Player_N_Start` waypoint and
//! `StartingUnit0` beside it, and the starting money. Random sides come from
//! the setup's seed. Players are numbered after the map's own players (neutral
//! and civilian). Chosen starts are reserved before the other slots are assigned
//! positions; AI players do nothing until M7.
use anyhow::{Context, Result};
use ih_client::{Definitions, World};
use ih_render::{Assets, Renderer, ui::UiPass};
use ih_ui::{
    draw::{DrawCmd, Resources},
    input::Input,
    shell::{GameSetup, Shell, ShellData, ShellEvent, SlotKind},
};
use ih_vfs::Vfs;

pub struct UiLayer {
    pub shell: Shell,
    pub(crate) res: Resources,
    pass: UiPass,
}

fn strings(vfs: &Vfs, language: &str) -> Result<ih_formats::strings::StringTable> {
    if let Some(b) = vfs.read("Data\\Generals.str")? {
        return Ok(ih_formats::strings::parse_str(&b));
    }
    let p = format!("Data\\{language}\\Generals.csf");
    let b = vfs.read(&p)?.with_context(|| format!("{p}: not found"))?;
    Ok(ih_formats::strings::parse_csf(&b)?)
}

impl UiLayer {
    pub fn new(
        vfs: &Vfs,
        language: &str,
        defs: &Definitions,
        renderer: &Renderer,
        size: (u32, u32),
        install: Option<std::path::PathBuf>,
    ) -> Result<Self> {
        let table = std::rc::Rc::new(strings(vfs, language)?);
        let lookup = |t: std::rc::Rc<ih_formats::strings::StringTable>| -> Box<dyn Fn(&str) -> String> {
            Box::new(move |l| t.text(l).map_or_else(|| format!("MISSING: '{l}'"), str::to_string))
        };
        let data = ShellData::load(&defs.data, vfs.read("Maps\\MapCache.ini")?.as_deref());
        let mut layouts = std::collections::HashMap::new();
        for f in vfs.list("Window", "*.wnd", true) {
            if let Some(b) = vfs.read(&f)? {
                let key = f.to_ascii_lowercase().replace('/', "\\");
                layouts.insert(key, b);
            }
        }
        let loader = Box::new(move |name: &str| {
            let key = format!("window\\{}", name.to_ascii_lowercase().replace('/', "\\"));
            layouts.get(&key).and_then(|b| ih_formats::wnd::parse(b).ok())
        });
        let shell = Shell::new([size.0 as i32, size.1 as i32], data, loader, lookup(table.clone()));
        let res = Resources {
            images: ih_ui::draw::mapped_images(&defs.data),
            headers: ih_ui::draw::header_templates(&defs.data),
            strings: lookup(table),
            fonts: ih_ui::text::Fonts::new(ih_ui::text::system_font_source(install)),
            default_font: ih_ui::text::FontSpec { name: "Times New Roman".into(), points: 14, bold: false },
        };
        Ok(Self { shell, res, pass: UiPass::new(renderer) })
    }

    /// Window pixels to the shell's layout pixels.
    pub fn input(&mut self, input: &Input) -> Vec<ShellEvent> {
        self.shell.input(input)
    }

    /// Draws the menus over whatever `view` already holds.
    pub fn draw(
        &mut self,
        renderer: &mut Renderer,
        vfs: &Vfs,
        language: &str,
        enc: &mut wgpu::CommandEncoder,
        view: &wgpu::TextureView,
        size: (u32, u32),
    ) {
        let list = self.shell.ui.draw_list(&mut self.res);
        self.paint(renderer, vfs, language, enc, view, size, &list);
    }

    #[allow(clippy::too_many_arguments)]
    pub(crate) fn paint(
        &mut self,
        renderer: &mut Renderer,
        vfs: &Vfs,
        language: &str,
        enc: &mut wgpu::CommandEncoder,
        view: &wgpu::TextureView,
        size: (u32, u32),
        list: &[DrawCmd],
    ) {
        if self.res.fonts.dirty {
            let s = self.res.fonts.atlas_size;
            renderer.set_texture_rgba(ih_ui::text::ATLAS, s, s, self.res.fonts.atlas.clone());
            self.res.fonts.dirty = false;
        }
        let quads: Vec<ih_render::ui::UiQuad> = list
            .iter()
            .map(|c| match c {
                DrawCmd::Image { texture, uv, rect, color } => ih_render::ui::UiQuad {
                    texture: Some(texture.clone()),
                    rect: *rect,
                    uv: *uv,
                    color: color.map(|c| f32::from(c) / 255.0),
                },
                DrawCmd::Fill { rect, color } => ih_render::ui::UiQuad {
                    texture: None,
                    rect: rect.map(|v| v as f32),
                    uv: [[0.0; 2]; 4],
                    color: color.map(|c| f32::from(c) / 255.0),
                },
            })
            .collect();
        let mut assets = Assets::new(vfs, language);
        self.pass.draw(renderer, &mut assets, enc, view, size, &quads, None);
    }
}

/// A small deterministic generator for random sides.
fn pick(seed: &mut u32, n: usize) -> usize {
    *seed = seed.wrapping_mul(1_664_525).wrapping_add(1_013_904_223);
    (*seed >> 8) as usize % n.max(1)
}

/// The skirmish as a scenario: starting buildings, units and money.
pub fn scenario_for(
    setup: &GameSetup,
    sides: &[(String, String)],
    defs: &Definitions,
    world: &World,
) -> Result<String> {
    let map = setup.map.as_ref().context("no map chosen")?;
    let mut seed = 0x1234_5678u32;
    let starts: Vec<[f32; 2]> = (1..=8)
        .filter_map(|n| {
            let want = format!("Player_{n}_Start");
            world.map.objects.iter().find(|o| match o.properties.get("waypointName") {
                Some(ih_formats::map::DictValue::Ascii(s)) => s.eq_ignore_ascii_case(&want),
                _ => false,
            })
        })
        .map(|o| [o.position[0], o.position[1]])
        .collect();
    let mut toml = format!("map = {:?}\nseed = {seed}\nframes = 1000000\n", map.folder());
    let field = |template: &str, key: &str| {
        defs.data
            .get("PlayerTemplate", template)
            .and_then(|b| b.fields().filter(|f| f.name == key).last().and_then(|f| f.values().first().cloned()))
    };
    let base = ih_sim::map_objects::players(&world.map).len().max(1) as u32;
    let mut commands = String::new();
    let positions = setup.assigned_starts(starts.len()).context("invalid or insufficient map start positions")?;
    anyhow::ensure!(!sides.is_empty(), "no playable sides");
    for (i, slot) in setup.slots.iter().enumerate() {
        if matches!(slot.kind, SlotKind::Open | SlotKind::Closed) {
            continue;
        }
        // Skirmish players come after the map's own (neutral, civilian), as the
        // original appends them to the player list.
        let owner = base + i as u32;
        let side = match &slot.side {
            Some(s) => s.clone(),
            None => sides[pick(&mut seed, sides.len())].0.clone(),
        };
        let at = starts[positions[i].context("playing slot has no start position")?];
        if let Some(b) = field(&side, "StartingBuilding") {
            toml +=
                &format!("\n[[spawn]]\ntemplate = {b:?}\nposition = [{:.1}, {:.1}]\nowner = {owner}\n", at[0], at[1]);
        }
        if let Some(u) = field(&side, "StartingUnit0") {
            // Beside the building, towards the middle of the map.
            let mid = [world.map.width as f32 * 5.0, world.map.height as f32 * 5.0];
            let d = [mid[0] - at[0], mid[1] - at[1]];
            let len = (d[0] * d[0] + d[1] * d[1]).sqrt().max(1.0);
            let p = [at[0] + d[0] / len * 90.0, at[1] + d[1] / len * 90.0];
            toml += &format!("\n[[spawn]]\ntemplate = {u:?}\nposition = [{:.1}, {:.1}]\nowner = {owner}\n", p[0], p[1]);
        }
        if let Some(sciences) = defs.data.field("PlayerTemplate", &side, "IntrinsicSciences") {
            for science in sciences.into_iter().filter(|s| !s.eq_ignore_ascii_case("None")) {
                commands += &format!(
                    "\n[[commands]]\nframe = 0\nkind = \"grant_science\"\nplayer = {owner}\nscience = {science:?}\n"
                );
            }
        }
        commands += &format!(
            "\n[[commands]]\nframe = 0\nkind = \"deposit\"\nplayer = {owner}\namount = {}\n",
            setup.starting_cash
        );
    }
    Ok(toml + &commands)
}

/// Populate the original score layout from client-side match counters.
pub fn populate_scores(layer: &mut UiLayer, scores: &ih_client::score::Scores) {
    let ui = &mut layer.shell.ui;
    for w in &mut ui.windows {
        if w.name.as_deref().is_some_and(|n| {
            n.starts_with("ScoreScreen.wnd:StaticTextObserver") || n.starts_with("ScoreScreen.wnd:GameWindowWinner")
        }) {
            w.status |= ih_ui::status::HIDDEN;
        }
    }
    for name in [
        "ButtonBuddy",
        "ButtonSaveReplay",
        "ButtonEmote",
        "TextEntryChat",
        "ListboxChatWindowScoreScreen",
        "ListboxWarschoolAdvice",
        "StaticTextWarSchool",
        "BigPortrait",
        "GeneralRemarks",
        "ChallengeWinLossText",
        "StaticTextGameSaveComplete",
    ] {
        if let Some(id) = ui.find(&format!("ScoreScreen.wnd:{name}")) {
            ui.hide(id, true);
        }
    }
    let mut rows: Vec<_> = ui
        .windows
        .iter()
        .filter_map(|w| {
            let n = w.name.as_deref()?.strip_prefix("ScoreScreen.wnd:StaticTextPlayer")?;
            let index = n.parse::<usize>().ok()?;
            Some((ui.screen_position(w.id)[1], index))
        })
        .collect();
    rows.sort();
    let players: Vec<_> = scores.players.iter().filter(|(id, _)| **id > 0).collect();
    for (row, (_, suffix)) in rows.iter().enumerate() {
        let stats = players.get(row);
        for (prefix, value) in [
            (
                "StaticTextPlayer",
                stats.map(|(id, _)| scores.names.get(id).cloned().unwrap_or_else(|| format!("Army {}", row + 1))),
            ),
            ("StaticTextUnitsBuilt", stats.map(|(_, s)| s.units_built.to_string())),
            ("StaticTextUnitsLost", stats.map(|(_, s)| s.units_lost.to_string())),
            ("StaticTextUnitsDestroyed", stats.map(|(_, s)| s.units_destroyed.to_string())),
            ("StaticTextBuildingsBuilt", stats.map(|(_, s)| s.buildings_built.to_string())),
            ("StaticTextBuildingsLost", stats.map(|(_, s)| s.buildings_lost.to_string())),
            ("StaticTextBuildingsDestroyed", stats.map(|(_, s)| s.buildings_destroyed.to_string())),
            ("StaticTextResources", stats.map(|(_, s)| s.supplies.to_string())),
        ] {
            if let Some(id) = ui.find(&format!("ScoreScreen.wnd:{prefix}{suffix}")) {
                ui.hide(id, value.is_none());
                ui.windows[id].text = value;
            }
        }
    }
}

pub fn loading_progress(layer: &mut UiLayer, setup: &GameSetup, value: i32) {
    let ui = &mut layer.shell.ui;
    for w in &mut ui.windows {
        if w.kind == "STATICTEXT" && w.text.is_none() && w.script.text().is_some_and(|t| !t.contains(':')) {
            w.text = Some(if w.script.rect.upper_left[1] < 100 { "Loading game".into() } else { String::new() });
        }
    }
    if let Some(id) = ui.find("MultiplayerLoadScreen.wnd:StaticTextProgressTitle") {
        ui.windows[id].text = Some(format!("Loading: {value}%"));
    }
    for (i, slot) in setup.slots.iter().enumerate() {
        let active = !matches!(slot.kind, SlotKind::Open | SlotKind::Closed);
        for prefix in ["ProgressLoad", "StaticTextPlayer", "StaticTextTeam", "StaticTextSide"] {
            if let Some(id) = ui.find(&format!("MultiplayerLoadScreen.wnd:{prefix}{i}")) {
                ui.hide(id, !active);
                if prefix == "ProgressLoad" {
                    ui.windows[id].gadget.value = value;
                } else if prefix == "StaticTextPlayer" {
                    ui.windows[id].text =
                        Some(if i == 0 { setup.player_name.clone() } else { format!("Army {}", i + 1) });
                } else if prefix == "StaticTextSide" {
                    ui.windows[id].text = Some(slot.side.clone().unwrap_or_else(|| "Random".into()));
                } else {
                    ui.windows[id].text = Some(slot.team.map(|t| t.to_string()).unwrap_or_else(|| "None".into()));
                }
            }
        }
    }
    if let Some(map) = &setup.map
        && let Some(id) = ui.find("MultiplayerLoadScreen.wnd:WinMapPreview")
    {
        let size = ui.windows[id].size;
        let origin = ui.screen_position(id);
        let extent = [(map.extent[2] - map.extent[0]).max(1.0), (map.extent[3] - map.extent[1]).max(1.0)];
        let scale = (size[0] as f32 / extent[0]).min(size[1] as f32 / extent[1]);
        let w = extent[0] * scale;
        let h = extent[1] * scale;
        let left = (size[0] as f32 - w) * 0.5;
        let top = (size[1] as f32 - h) * 0.5;
        ui.windows[id].image = Some(format!("Maps\\{}\\{}.tga", map.folder(), map.folder()));
        ui.windows[id].image_rect = [left as i32, top as i32, (left + w) as i32, (top + h) as i32];
        let assignments = setup.assigned_starts(map.players as usize).unwrap_or_default();
        for n in 0..8 {
            if let Some(marker) = ui.find(&format!("MultiplayerLoadScreen.wnd:ButtonMapStartPosition{n}")) {
                if let Some(p) = map.starts.get(n) {
                    ui.hide(marker, false);
                    let parent = ui.windows[marker].parent.map_or([0, 0], |p| ui.screen_position(p));
                    let s = ui.windows[marker].size;
                    ui.windows[marker].position = [
                        origin[0] + left as i32 + ((p[0] - map.extent[0]) * scale) as i32 - parent[0] - s[0] / 2,
                        origin[1] + top as i32 + (h - (p[1] - map.extent[1]) * scale) as i32 - parent[1] - s[1] / 2,
                    ];
                    ui.windows[marker].text = Some(
                        assignments.iter().position(|p| *p == Some(n)).map(|i| (i + 1).to_string()).unwrap_or_default(),
                    );
                } else {
                    ui.hide(marker, true);
                }
            }
        }
    }
}
