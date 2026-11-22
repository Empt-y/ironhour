//! The menus in the game window: the shell over the map, its input and drawing,
//! and turning a skirmish setup into a game.
//!
//! The game start follows `GameLogic::startNewGame` loosely: each playing slot
//! gets its side's `StartingBuilding` at the map's `Player_N_Start` waypoint and
//! `StartingUnit0` beside it, and the starting money. Random sides come from
//! the setup's seed. Players are numbered after the map's own players (neutral
//! and civilian). Start positions are given out in slot order (the original
//! lets players choose or shuffles them), and AI players do nothing until M7.
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
    res: Resources,
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
    let mut start = 0;
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
        let Some(&at) = starts.get(start) else { break };
        start += 1;
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
        commands += &format!(
            "\n[[commands]]\nframe = 0\nkind = \"deposit\"\nplayer = {owner}\namount = {}\n",
            setup.starting_cash
        );
    }
    Ok(toml + &commands)
}
