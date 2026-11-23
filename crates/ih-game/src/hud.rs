//! In-game control bar: original layout and data, translated into simulation commands.
use crate::shell::UiLayer;
use anyhow::{Context, Result};
use ih_client::{
    Definitions,
    control::{Button, Controls},
};
use ih_render::Renderer;
use ih_sim::{Command, Simulation};
use ih_ui::{
    WindowManager,
    draw::DrawCmd,
    input::{Input, Message, UiEvent},
};
use ih_vfs::Vfs;

struct Scheme {
    side: String,
    parts: Vec<(i32, String, [f32; 4])>,
    icons: std::collections::BTreeMap<String, String>,
    res: [f32; 2],
}
pub struct Hud {
    schemes: Vec<Scheme>,
    scheme: usize,
    pub layer: UiLayer,
    pub controls: Controls,
    pub pending: Option<String>,
    pub angle: f32,
    pub ground: Option<[f32; 2]>,
    pub view_projection: glam::Mat4,
    pub visible: bool,
    pub menu: bool,
    pub score: bool,
    pub action: Option<String>,
    pub camera_target: Option<[f32; 2]>,
    map_frame: u32,
    pub generals: bool,
    general_page: usize,
    general_pages: usize,
    pub groups: [Vec<u32>; 10],
    pub views: [Option<ih_client::TacticalCamera>; 10],
    pub camera_keys: std::collections::BTreeMap<String, String>,
    slots: Vec<Option<String>>,
    cursor_images: std::collections::BTreeMap<String, String>,
    science_sets: std::collections::BTreeMap<String, Vec<String>>,
    pub toast: String,
    pub sounds: Vec<String>,
    tooltip: String,
    size: (u32, u32),
    layout: ih_formats::wnd::Layout,
}
impl Hud {
    pub fn new(vfs: &Vfs, language: &str, defs: &Definitions, renderer: &Renderer, size: (u32, u32)) -> Result<Self> {
        let layer = UiLayer::new(vfs, language, defs, renderer, size, ih_vfs::Installs::locate().zero_hour)?;
        let bytes = vfs.read("Window\\ControlBar.wnd")?.context("ControlBar.wnd missing")?;
        let layout = ih_formats::wnd::parse(&bytes)?;
        let cursor_images = defs
            .data
            .ordered("MouseCursor")
            .filter_map(|b| {
                Some((
                    b.args().first()?.clone(),
                    b.fields().filter(|f| f.name == "Image").last()?.values().first()?.clone(),
                ))
            })
            .collect();
        let science_sets = defs
            .data
            .ordered("PlayerTemplate")
            .filter_map(|b| {
                let side = b.fields().filter(|f| f.name == "Side").last()?.values().first()?.clone();
                let sets = b
                    .fields()
                    .filter(|f| f.name.starts_with("PurchaseScienceCommandSetRank"))
                    .filter_map(|f| f.values().first().cloned())
                    .collect();
                Some((side, sets))
            })
            .collect();
        let mut schemes = vec![];
        for b in defs.data.ordered("ControlBarScheme") {
            let field = |name: &str| {
                b.fields().filter(|f| f.name.eq_ignore_ascii_case(name)).last().map(|f| f.values()).unwrap_or_default()
            };
            let xy = |v: Vec<String>| {
                let num = |k: &str, d: f32| v.iter().find_map(|s| s.strip_prefix(k)?.parse().ok()).unwrap_or(d);
                [num("X:", 800.0), num("Y:", 600.0)]
            };
            let res = xy(field("ScreenCreationRes"));
            let side = field("Side").first().cloned().unwrap_or_default();
            let mut parts = vec![];
            b.walk(&mut |part, _| {
                if part.kind == "ImagePart" {
                    let get = |key: &str| {
                        part.fields().filter(|f| f.name == key).last().map(|f| f.values()).unwrap_or_default()
                    };
                    let p = xy(get("Position"));
                    let s = xy(get("Size"));
                    if let Some(name) = get("ImageName").first() {
                        let layer = get("Layer").first().and_then(|s| s.parse().ok()).unwrap_or(0);
                        parts.push((layer, name.clone(), [p[0], p[1], p[0] + s[0], p[1] + s[1]]));
                    }
                }
            });
            parts.sort_by_key(|p| p.0);
            let icons = b.fields().filter_map(|f| Some((f.name.clone(), f.values().first()?.clone()))).collect();
            schemes.push(Scheme { side, parts, icons, res });
        }
        let mut h = Self {
            schemes,
            scheme: 0,
            layer,
            controls: Controls::load(&defs.data),
            pending: None,
            angle: 0.0,
            ground: None,
            view_projection: glam::Mat4::IDENTITY,
            visible: true,
            menu: false,
            score: false,
            action: None,
            camera_target: None,
            map_frame: u32::MAX,
            generals: false,
            general_page: 0,
            general_pages: 1,
            groups: Default::default(),
            views: Default::default(),
            camera_keys: Default::default(),
            slots: vec![None; 14],
            cursor_images,
            science_sets,
            toast: String::new(),
            sounds: vec![],
            tooltip: String::new(),
            size,
            layout,
        };
        h.resize(size);
        Ok(h)
    }
    pub fn resize(&mut self, size: (u32, u32)) {
        if size.0 == 0 || size.1 == 0 {
            return;
        }
        self.size = size;
        self.map_frame = u32::MAX;
        let ui = &mut self.layer.shell.ui;
        *ui = WindowManager::new([size.0 as i32, size.1 as i32]);
        ui.load(&self.layout);
        ui.init_gadgets();
        for name in [
            "BeaconWindow",
            "ObserverPlayerInfoWindow",
            "ObserverPlayerListWindow",
            "OCLTimerWindow",
            "UnderConstructionWindow",
            "WinUnitSelected",
            "PopupCommunicator",
            "ButtonPlaceBeacon",
            "ButtonSmall",
            "ButtonLarge",
            "ButtonMedium",
            "Munkee",
        ] {
            if let Some(id) = ui.find(&format!("ControlBar.wnd:{name}")) {
                ui.hide(id, true);
            }
        }
    }
    fn rect(&self, name: &str) -> Option<[f32; 4]> {
        let ui = &self.layer.shell.ui;
        let id = ui.find(&format!("ControlBar.wnd:{name}"))?;
        let p = ui.screen_position(id);
        let s = ui.windows[id].size;
        Some([p[0] as f32, p[1] as f32, (p[0] + s[0]) as f32, (p[1] + s[1]) as f32])
    }
    pub fn update(&mut self, sim: &Simulation, selected: &[u32], player: u32) {
        let side = sim
            .objects
            .values()
            .find(|o| o.owner == player && !o.dead)
            .and_then(|o| sim.templates.objects[&o.template].values("Side").first());
        if let Some(side) = side {
            self.scheme = self
                .schemes
                .iter()
                .position(|s| s.side.eq_ignore_ascii_case(side))
                .or_else(|| {
                    self.schemes
                        .iter()
                        .position(|s| side.to_ascii_lowercase().starts_with(&s.side.to_ascii_lowercase()))
                })
                .unwrap_or(0);
        }
        self.slots = self.controls.selection(sim, selected, player);
        if self.generals {
            self.slots = vec![None; 14];
            let side = sim
                .objects
                .values()
                .find(|o| o.owner == player && !o.dead)
                .and_then(|o| sim.templates.objects[&o.template].values("Side").first());
            if let Some(sets) = side.and_then(|s| self.science_sets.get(s)) {
                let choices: Vec<_> = sets
                    .iter()
                    .filter_map(|s| self.controls.sets.get(s))
                    .flat_map(|s| s.iter().filter_map(Clone::clone))
                    .collect();
                self.general_pages = choices.len().div_ceil(14).max(1);
                self.general_page %= self.general_pages;
                self.slots = choices.into_iter().skip(self.general_page * 14).take(14).map(Some).collect();
                self.slots.resize(14, None);
            }
        }
        if let Some(id) = self.layer.shell.ui.find("ControlBar.wnd:CommandWindow") {
            self.layer.shell.ui.hide(id, false);
        }
        for i in 0..14 {
            let name = format!("ControlBar.wnd:ButtonCommand{:02}", i + 1);
            if let Some(id) = self.layer.shell.ui.find(&name) {
                self.layer.shell.ui.hide(id, self.slots[i].is_none());
                self.layer.shell.ui.enable(
                    id,
                    self.slots[i]
                        .as_ref()
                        .and_then(|n| self.controls.buttons.get(n))
                        .is_some_and(|b| self.controls.available(b, sim, selected, player)),
                );
            }
        }
        if let Some(id) = self.layer.shell.ui.find("ControlBar.wnd:MoneyDisplay") {
            self.layer.shell.ui.windows[id].text = Some(format!("${}", sim.money(player)));
        }
    }
    pub fn activate(&mut self, index: usize, sim: &mut Simulation, selected: &[u32], player: u32) {
        let Some(button) =
            self.slots.get(index).and_then(|n| n.as_ref()).and_then(|n| self.controls.buttons.get(n)).cloned()
        else {
            return;
        };
        if !self.controls.available(&button, sim, selected, player) {
            self.toast = "Command unavailable".into();
            return;
        }
        if button.targeted() {
            self.pending = Some(button.name);
            self.angle = 0.0;
        } else {
            self.send(&button, sim, selected, player, None, None);
        }
    }
    fn send(
        &mut self,
        b: &Button,
        sim: &mut Simulation,
        selected: &[u32],
        player: u32,
        point: Option<[f32; 2]>,
        target: Option<u32>,
    ) {
        let orders = self.controls.orders(b, sim, selected, player, point, target, self.angle);
        if orders.is_empty() {
            self.toast = "Choose a valid target".into();
            return;
        }
        for command in orders {
            if let Err(e) = sim.enqueue(sim.frame, command) {
                self.toast = e.to_string();
            }
        }
    }
    pub fn target(
        &mut self,
        sim: &mut Simulation,
        selected: &[u32],
        player: u32,
        point: [f32; 2],
        target: Option<u32>,
        append: bool,
    ) -> bool {
        let Some(b) = self.pending.as_ref().and_then(|n| self.controls.buttons.get(n)).cloned() else { return false };
        if b.command == "DOZER_CONSTRUCT"
            && sim.location_legal_to_build(&b.object, point, self.angle, selected.first().copied())
                != ih_sim::construction::LegalBuild::Ok
        {
            self.toast = "Cannot build here".into();
            return true;
        }
        self.send(&b, sim, selected, player, Some(point), target);
        if !append {
            self.pending = None;
        }
        true
    }
    pub fn input(&mut self, input: &Input, sim: &mut Simulation, selected: &[u32], player: u32) -> bool {
        if self.menu || self.score {
            if let Input::LeftUp(p) = input {
                let x = p[0] as f32 / self.size.0 as f32;
                let y = p[1] as f32 / self.size.1 as f32;
                if (0.3..0.7).contains(&x) && (0.3..0.75).contains(&y) {
                    let row = ((y - 0.3) / 0.075) as usize;
                    self.action = Some(
                        if self.score {
                            "main"
                        } else {
                            ["resume", "options", "score", "main", "quit", "resume"][row.min(5)]
                        }
                        .into(),
                    );
                }
            }
            return true;
        }
        if !self.visible {
            return false;
        }
        let p = match input {
            Input::MouseMove(p) | Input::LeftDown(p) | Input::LeftUp(p) => *p,
            _ => return false,
        };
        let inside = p[1] as f32 >= self.size.1 as f32 * 416.0 / 600.0;
        if !inside && self.layer.shell.ui.input_state.captor.is_none() {
            return false;
        }
        if let Input::LeftDown(p) = input
            && sim.has_radar(player)
            && let Some(r) = self.rect("LeftHUD")
            && p[0] as f32 >= r[0]
            && (p[0] as f32) < r[2]
            && p[1] as f32 >= r[1]
            && (p[1] as f32) < r[3]
        {
            self.camera_target = Some([
                (p[0] as f32 - r[0]) / (r[2] - r[0]) * sim.map.width as f32 * 10.0,
                (1.0 - (p[1] as f32 - r[1]) / (r[3] - r[1])) * sim.map.height as f32 * 10.0,
            ]);
            return true;
        }
        let events = self.layer.shell.ui.input(input);
        for event in events {
            if let UiEvent::Sound(name) = &event {
                self.sounds.push(name.clone());
            }
            if let UiEvent::System { message: Message::Selected(id), .. } = event {
                let name = self.layer.shell.ui.windows[id].name.clone().unwrap_or_default();
                if let Some(i) = name.strip_prefix("ControlBar.wnd:ButtonCommand").and_then(|s| s.parse::<usize>().ok())
                {
                    self.activate(i - 1, sim, selected, player);
                } else if let Some(i) =
                    name.strip_prefix("ControlBar.wnd:ButtonQueue").and_then(|s| s.parse::<usize>().ok())
                {
                    if let Some((&factory, entry)) = selected.first().and_then(|id| {
                        sim.objects
                            .get(id)
                            .and_then(|o| o.production.as_ref())
                            .and_then(|q| q.queue.get(i - 1))
                            .map(|e| (id, e.clone()))
                    }) {
                        let command = if entry.upgrade {
                            Command::CancelResearch { factory, upgrade: entry.template }
                        } else {
                            Command::CancelProduction { factory, production: entry.id }
                        };
                        sim.enqueue(sim.frame, command).ok();
                    }
                } else if name.ends_with(":ButtonOptions") {
                    self.menu = true;
                } else if name.ends_with(":ButtonGeneral") {
                    if !self.generals {
                        self.generals = true;
                        self.general_page = 0;
                    } else if self.general_page + 1 < self.general_pages {
                        self.general_page += 1;
                    } else {
                        self.generals = false;
                    }
                } else if name.ends_with(":ButtonIdleWorker") {
                    self.action = Some("worker".into());
                }
            }
        }
        if let Some(id) = self.layer.shell.ui.window_at(p) {
            self.tooltip = self.layer.shell.ui.windows[id]
                .name
                .as_deref()
                .and_then(|n| n.strip_prefix("ControlBar.wnd:ButtonCommand"))
                .and_then(|s| s.parse::<usize>().ok())
                .and_then(|i| self.slots.get(i - 1))
                .and_then(|n| n.as_ref())
                .and_then(|n| self.controls.buttons.get(n))
                .map(|b| {
                    let t = &self.layer.res.strings;
                    let cost = if !b.object.is_empty() {
                        sim.templates.objects.get(&b.object).map(|o| sim.cost_to_build(o, player))
                    } else {
                        None
                    };
                    format!(
                        "{}{}\n{}",
                        t(&b.label),
                        cost.map(|c| format!("  ${c}")).unwrap_or_default(),
                        t(&b.description)
                    )
                })
                .unwrap_or_default();
        }
        true
    }
    pub fn button_hotkey(&mut self, key: char, sim: &mut Simulation, selected: &[u32], player: u32) -> bool {
        let index = self.slots.iter().position(|n| {
            n.as_ref().and_then(|n| self.controls.buttons.get(n)).is_some_and(|b| {
                let label = (self.layer.res.strings)(&b.label);
                label.split_once('&').and_then(|(_, s)| s.chars().next()).is_some_and(|c| c.eq_ignore_ascii_case(&key))
            })
        });
        if let Some(i) = index {
            self.activate(i, sim, selected, player);
            true
        } else {
            false
        }
    }
    fn text(&mut self, out: &mut Vec<DrawCmd>, text: &str, x: f32, y: f32, width: f32, color: [u8; 4]) {
        let font = ih_ui::text::FontSpec { name: "Arial".into(), points: 10, bold: false };
        let px = (self.size.0 as f32 / 800.0 * 13.0).max(10.0) as u32;
        let layout = self.layer.res.fonts.layout(&font, px, text, width);
        for q in self.layer.res.fonts.draw(&font, px, &layout, [x, y], false, color, Some([0, 0, 0, 255])) {
            out.push(DrawCmd::Image { texture: ih_ui::text::ATLAS.into(), uv: q.uv, rect: q.rect, color: q.color });
        }
    }
    fn mapped(&self, out: &mut Vec<DrawCmd>, name: &str, rect: [f32; 4], color: [u8; 4]) {
        if let Some(image) = self.layer.res.images.get(&name.to_ascii_lowercase()) {
            out.push(DrawCmd::Image { texture: image.texture.clone(), uv: image.uv_corners(), rect, color });
        }
    }
    #[allow(clippy::too_many_arguments)]
    pub fn draw(
        &mut self,
        renderer: &mut Renderer,
        vfs: &Vfs,
        language: &str,
        enc: &mut wgpu::CommandEncoder,
        view: &wgpu::TextureView,
        sim: &Simulation,
        selected: &[u32],
        player: u32,
        cursor: Option<[f32; 2]>,
    ) {
        let mut list = vec![];
        if self.visible {
            list.push(DrawCmd::Fill {
                rect: [0, (self.size.1 as f32 * 416.0 / 600.0) as i32, self.size.0 as i32, self.size.1 as i32],
                color: [12, 20, 30, 245],
            });
            if let Some(scheme) = self.schemes.get(self.scheme) {
                for (_, name, r) in &scheme.parts {
                    let r = [
                        r[0] / scheme.res[0] * self.size.0 as f32,
                        r[1] / scheme.res[1] * self.size.1 as f32,
                        r[2] / scheme.res[0] * self.size.0 as f32,
                        r[3] / scheme.res[1] * self.size.1 as f32,
                    ];
                    self.mapped(&mut list, name, r, [255; 4]);
                }
                for (window, key) in [
                    ("ButtonOptions", "OptionsButtonEnable"),
                    ("ButtonIdleWorker", "IdleWorkerButtonEnable"),
                    ("ButtonGeneral", "GeneralButtonEnable"),
                ] {
                    if let Some(name) = scheme.icons.get(key)
                        && let Some(r) = self.rect(window)
                    {
                        self.mapped(&mut list, name, r, [255; 4]);
                    }
                }
            }
            self.text(
                &mut list,
                &format!("${}", sim.money(player)),
                self.size.0 as f32 * 0.45,
                self.size.1 as f32 * 0.73,
                130.0,
                [255; 4],
            );
            // The original layout supplies hit regions; command cameos are runtime images.
            for i in 0..14 {
                if let Some(b) =
                    self.slots.get(i).and_then(|n| n.as_ref()).and_then(|n| self.controls.buttons.get(n)).cloned()
                    && let Some(r) = self.rect(&format!("ButtonCommand{:02}", i + 1))
                {
                    self.mapped(
                        &mut list,
                        &b.image,
                        r,
                        if self.controls.available(&b, sim, selected, player) { [255; 4] } else { [90, 90, 90, 255] },
                    );
                }
            }
            if let Some(factory) = selected.first().and_then(|id| sim.objects.get(id)) {
                if let Some(p) = &factory.production {
                    for (i, e) in p.queue.iter().take(9).enumerate() {
                        if let Some(r) = self.rect(&format!("ButtonQueue{:02}", i + 1)) {
                            list.push(DrawCmd::Fill { rect: r.map(|v| v as i32), color: [20, 35, 55, 240] });
                            self.text(
                                &mut list,
                                &format!(
                                    "{}\n{:.0}%",
                                    sim.templates
                                        .objects
                                        .get(&e.template)
                                        .and_then(|t| t.values("DisplayName").first())
                                        .map(|n| (self.layer.res.strings)(n))
                                        .unwrap_or_else(|| e.template.clone()),
                                    e.percent * 100.0
                                ),
                                r[0],
                                r[1],
                                r[2] - r[0],
                                [240; 4],
                            );
                        }
                    }
                }
                let label = sim.templates.objects[&factory.template]
                    .values("DisplayName")
                    .first()
                    .map(|n| (self.layer.res.strings)(n))
                    .unwrap_or_else(|| factory.template.clone());
                let health =
                    factory.health.as_ref().map(|h| format!("  {:.0}/{:.0}", h.current, h.max)).unwrap_or_default();
                self.text(
                    &mut list,
                    &format!(
                        "{}{}{}",
                        label,
                        health,
                        factory.construction.map(|p| format!("\nConstruction: {:.0}%", p.max(0.0))).unwrap_or_default()
                    ),
                    self.size.0 as f32 * 0.69,
                    self.size.1 as f32 * 0.90,
                    self.size.0 as f32 * 0.30,
                    [235, 240, 245, 255],
                );
            }
            let (supply, demand) = sim.energy(player);
            let points = sim.players.get(&player).map_or(0, |p| p.generals.purchase_points);
            self.text(
                &mut list,
                &format!("Power {supply}/{demand}   General points: {points}"),
                self.size.0 as f32 * 0.27,
                self.size.1 as f32 * 0.79,
                self.size.0 as f32 * 0.50,
                if supply < demand { [255, 90, 70, 255] } else { [140, 240, 160, 255] },
            );
            if !self.tooltip.is_empty() {
                let text = self.tooltip.clone();
                self.text(
                    &mut list,
                    &text,
                    12.0,
                    self.size.1 as f32 * 0.63,
                    self.size.0 as f32 * 0.7,
                    [255, 240, 180, 255],
                );
            }
            if !self.toast.is_empty() {
                let text = self.toast.clone();
                self.text(&mut list, &text, 12.0, 45.0, self.size.0 as f32 * 0.85, [255, 170, 100, 255]);
            }
        }
        if self.visible
            && sim.has_radar(player)
            && let Some(r) = self.rect("LeftHUD")
        {
            if self.map_frame == u32::MAX || sim.frame.saturating_sub(self.map_frame) >= 3 {
                let mut pixels = vec![0u8; 128 * 128 * 4];
                for y in 0..128 {
                    for x in 0..128 {
                        let world = [
                            x as f32 / 127.0 * sim.map.width as f32 * 10.0,
                            (1.0 - y as f32 / 127.0) * sim.map.height as f32 * 10.0,
                        ];
                        let visible = sim.shroud.status_at(player, world);
                        let h = sim.map.ground_height(world[0], world[1]);
                        let shade = (h * 0.2).clamp(0.0, 80.0) as u8;
                        let color = match visible {
                            ih_sim::shroud::Visibility::Shrouded => [0, 0, 0, 255],
                            ih_sim::shroud::Visibility::Fogged => [30, 35, 20, 255],
                            _ => [65 + shade, 75 + shade, 45 + shade, 255],
                        };
                        pixels[(y * 128 + x) * 4..(y * 128 + x) * 4 + 4].copy_from_slice(&color);
                    }
                }
                for o in sim.objects.values().filter(|o| {
                    !o.dead
                        && (o.owner == player || sim.shrouded_status(o.id, player) == ih_sim::shroud::Visibility::Clear)
                }) {
                    let x = (o.position[0] / (sim.map.width as f32 * 10.0) * 127.0).clamp(0.0, 127.0) as usize;
                    let y = ((1.0 - o.position[1] / (sim.map.height as f32 * 10.0)) * 127.0).clamp(0.0, 127.0) as usize;
                    pixels[(y * 128 + x) * 4..(y * 128 + x) * 4 + 4].copy_from_slice(if o.owner == player {
                        &[60, 255, 100, 255]
                    } else {
                        &[255, 80, 65, 255]
                    });
                }
                renderer.set_texture_rgba("__ih_radar", 128, 128, pixels);
                self.map_frame = sim.frame;
            }
            list.push(DrawCmd::Image {
                texture: "__ih_radar".into(),
                uv: [[0.0, 0.0], [1.0, 0.0], [1.0, 1.0], [0.0, 1.0]],
                rect: r,
                color: [255; 4],
            });
        }
        if self.visible
            && !sim.has_radar(player)
            && let Some(r) = self.rect("LeftHUD")
        {
            list.push(DrawCmd::Fill { rect: r.map(|v| v as i32), color: [5, 10, 15, 255] });
            self.text(&mut list, "Radar offline", r[0] + 5.0, r[1] + 20.0, r[2] - r[0] - 10.0, [145, 155, 160, 255]);
        }
        if self.generals {
            let text = format!(
                "General powers {}/{} - click General to change page",
                self.general_page + 1,
                self.general_pages
            );
            self.text(
                &mut list,
                &text,
                self.size.0 as f32 * 0.28,
                self.size.1 as f32 * 0.64,
                self.size.0 as f32 * 0.70,
                [255, 240, 180, 255],
            );
        }
        if let Some(button) = self.pending.as_ref().and_then(|n| self.controls.buttons.get(n))
            && button.command == "DOZER_CONSTRUCT"
            && let Some(at) = self.ground
            && let Some(t) = sim.templates.objects.get(&button.object)
        {
            let legal = sim.location_legal_to_build(&button.object, at, self.angle, selected.first().copied())
                == ih_sim::construction::LegalBuild::Ok;
            let rx = t.number("GeometryMajorRadius", 20.0);
            let ry = t.number("GeometryMinorRadius", rx);
            let (s, c) = self.angle.sin_cos();
            let corners = [[-rx, -ry], [rx, -ry], [rx, ry], [-rx, ry]];
            let points: Vec<_> = corners
                .iter()
                .filter_map(|p| {
                    let x = at[0] + p[0] * c - p[1] * s;
                    let y = at[1] + p[0] * s + p[1] * c;
                    let q = self.view_projection * glam::Vec4::new(x, y, sim.map.ground_height(x, y) + 2.0, 1.0);
                    (q.w > 0.0).then(|| {
                        [
                            ((q.x / q.w + 1.0) * 0.5 * self.size.0 as f32) as i32,
                            ((1.0 - q.y / q.w) * 0.5 * self.size.1 as f32) as i32,
                        ]
                    })
                })
                .collect();
            if points.len() == 4 {
                for i in 0..4 {
                    let a = points[i];
                    let b = points[(i + 1) % 4];
                    let steps = ((b[0] - a[0]).abs().max((b[1] - a[1]).abs())).max(1);
                    for j in 0..=steps {
                        let x = a[0] + (b[0] - a[0]) * j / steps;
                        let y = a[1] + (b[1] - a[1]) * j / steps;
                        list.push(DrawCmd::Fill {
                            rect: [x - 1, y - 1, x + 2, y + 2],
                            color: if legal { [60, 255, 100, 210] } else { [255, 70, 60, 210] },
                        });
                    }
                }
            }
        }
        if self.menu || self.score {
            list.push(DrawCmd::Fill { rect: [0, 0, self.size.0 as i32, self.size.1 as i32], color: [0, 0, 0, 180] });
            let labels = if self.score {
                vec!["Match ended", "Return to main menu"]
            } else {
                vec!["Resume", "Options", "End match / scores", "Main menu", "Exit game"]
            };
            for (i, label) in labels.iter().enumerate() {
                let y = self.size.1 as f32 * (0.3 + i as f32 * 0.075);
                list.push(DrawCmd::Fill {
                    rect: [
                        (self.size.0 as f32 * 0.3) as i32,
                        y as i32,
                        (self.size.0 as f32 * 0.7) as i32,
                        (y + self.size.1 as f32 * 0.06) as i32,
                    ],
                    color: [20, 40, 65, 255],
                });
                self.text(&mut list, label, self.size.0 as f32 * 0.32, y + 5.0, self.size.0 as f32 * 0.36, [245; 4]);
            }
        }
        if let Some(p) = cursor {
            let hover = if p[1] < self.size.1 as f32 * 416.0 / 600.0 {
                sim.objects
                    .values()
                    .filter(|o| !o.dead)
                    .filter_map(|o| {
                        let q = self.view_projection * glam::Vec3::from(o.position).extend(1.0);
                        if q.w <= 0.0 {
                            return None;
                        }
                        let ndc = q.truncate() / q.w;
                        let dx = (ndc.x + 1.0) * 0.5 * self.size.0 as f32 - p[0];
                        let dy = (1.0 - ndc.y) * 0.5 * self.size.1 as f32 - p[1];
                        Some((dx * dx + dy * dy, o))
                    })
                    .filter(|(d, _)| *d < 24.0 * 24.0)
                    .min_by(|a, b| a.0.total_cmp(&b.0))
                    .map(|(_, o)| o)
            } else {
                None
            };
            let cursor_name = if self.menu || self.score || p[1] >= self.size.1 as f32 * 416.0 / 600.0 {
                "Arrow"
            } else if let Some(b) = self.pending.as_ref().and_then(|n| self.controls.buttons.get(n)) {
                if b.command == "DOZER_CONSTRUCT"
                    && self.ground.is_some_and(|point| {
                        sim.location_legal_to_build(&b.object, point, self.angle, selected.first().copied())
                            != ih_sim::construction::LegalBuild::Ok
                    })
                {
                    "GenericInvalid"
                } else if b.cursor.is_empty() {
                    "Target"
                } else {
                    &b.cursor
                }
            } else if selected.iter().any(|id| sim.objects.get(id).is_some_and(|o| o.owner == player)) {
                if hover.is_some_and(|o| o.owner != player && o.owner != 0) {
                    "AttackObj"
                } else if hover.is_some() {
                    "Select"
                } else if self.ground.is_some() {
                    "Move"
                } else {
                    "GenericInvalid"
                }
            } else if hover.is_some() {
                "Select"
            } else {
                "Arrow"
            };
            if let Some(image) = self.cursor_images.get(cursor_name).or_else(|| self.cursor_images.get("Arrow")) {
                self.mapped(&mut list, image, [p[0], p[1], p[0] + 32.0, p[1] + 32.0], [255; 4]);
            }
        }
        self.layer.paint(renderer, vfs, language, enc, view, self.size, &list);
    }
}
