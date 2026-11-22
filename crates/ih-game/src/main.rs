//! Native map viewer. Replaces the window/frame-loop portion of `WinMain.cpp`;
//! map loading and drawing use the same client/renderer as the headless tools.
use anyhow::{Context, Result, bail};
use clap::Parser;
use glam::Vec2;
use ih_client::{Definitions, TacticalCamera, World};
use ih_render::{Assets, Renderer, Scene, Targets};
use ih_vfs::{Installs, ModProfile, Vfs};

mod audio;
mod shell;
use std::{
    collections::HashSet,
    path::PathBuf,
    sync::Arc,
    time::{Duration, Instant},
};
use winit::{
    application::ApplicationHandler,
    dpi::PhysicalSize,
    event::{MouseButton, MouseScrollDelta, WindowEvent},
    event_loop::{ActiveEventLoop, ControlFlow, EventLoop},
    keyboard::{KeyCode, PhysicalKey},
    window::{Window, WindowId},
};

#[derive(Parser)]
#[command(version, about)]
struct Cli {
    /// Map name or game-data path.
    #[arg(default_value = "Tournament Desert")]
    map: String,
    #[arg(long)]
    profile: Option<PathBuf>,
    #[arg(long, default_value_t = 1280)]
    width: u32,
    #[arg(long, default_value_t = 800)]
    height: u32,
    /// Start with the whole map in view.
    #[arg(long)]
    overview: bool,
    /// Verify window/surface setup, resize and rendering without displaying a window.
    #[arg(long)]
    smoke: bool,
    /// Load a recorded M4 movement scenario; its map replaces the positional map.
    #[arg(long)]
    scenario: Option<PathBuf>,
    /// Start in the menus (drawn over the map); Start Game in skirmish plays it.
    #[arg(long)]
    shell: bool,
    /// Disable audio output (smoke checks are always silent).
    #[arg(long)]
    no_audio: bool,
    /// Keep sound effects and speech, but disable background music.
    #[arg(long)]
    no_music: bool,
    /// Override the per-user audio preferences file.
    #[arg(long)]
    audio_preferences: Option<PathBuf>,
}

fn main() -> Result<()> {
    let mut cli = Cli::parse();
    let scenario: Option<ih_sim::scenario::Scenario> = cli
        .scenario
        .as_ref()
        .map(|p| -> Result<_> { Ok(toml::from_str(&std::fs::read_to_string(p)?)?) })
        .transpose()?;
    if let Some(s) = &scenario {
        cli.map = s.map.clone();
    }
    if cli.width == 0 || cli.height == 0 {
        bail!("window dimensions must be positive");
    }
    let installs = Installs::locate();
    if installs.zero_hour.is_none() {
        bail!("Zero Hour install not found; set IH_ZH_INSTALL and IH_GENERALS_INSTALL");
    }
    let profile = cli.profile.as_ref().map(ModProfile::load).transpose()?.unwrap_or_else(ModProfile::stock);
    let vfs = Vfs::for_game(&installs, &profile)?;
    for w in vfs.warnings() {
        eprintln!("warning: {}", w.0);
    }
    let mut defs = Definitions::load(&vfs, &installs.language);
    let world = World::load(&vfs, &cli.map)?;
    defs.load_map_overrides(&vfs, &world.path)?;
    let event_loop = EventLoop::new()?;
    let mut app = App { cli, vfs, language: installs.language, defs, world, scenario, state: None, error: None };
    event_loop.run_app(&mut app)?;
    if let Some(e) = app.error {
        return Err(e);
    }
    Ok(())
}

struct App {
    cli: Cli,
    vfs: Vfs,
    language: String,
    defs: Definitions,
    world: World,
    scenario: Option<ih_sim::scenario::Scenario>,
    state: Option<State>,
    error: Option<anyhow::Error>,
}
struct State {
    window: Arc<Window>,
    instance: wgpu::Instance,
    surface: wgpu::Surface<'static>,
    config: wgpu::SurfaceConfiguration,
    renderer: Renderer,
    scene: Scene,
    targets: Targets,
    camera: TacticalCamera,
    reset_camera: TacticalCamera,
    keys: HashSet<KeyCode>,
    cursor: Option<Vec2>,
    pan_drag: bool,
    orbit_drag: bool,
    focused: bool,
    size: PhysicalSize<u32>,
    started: Instant,
    last_frame: Instant,
    next_redraw: Instant,
    simulation: Option<ih_sim::Simulation>,
    sim_draw: Option<ih_client::sim_draw::SimDrawables>,
    clock: ih_sim::LogicClock,
    selected: Vec<u32>,
    right_start: Option<Vec2>,
    /// Where a left press began (drag-box selection).
    left_start: Option<Vec2>,
    paused: bool,
    /// The menus, while they are up.
    ui: Option<shell::UiLayer>,
    audio: Option<ih_audio::Audio>,
    local_player: Option<u32>,
    feedback: audio::Feedback,
    preferences: ih_audio::preferences::Preferences,
    preferences_path: PathBuf,
}

/// What `App::load_game` builds for a map.
type Game = (Scene, Option<ih_sim::Simulation>, Option<ih_client::sim_draw::SimDrawables>, TacticalCamera);

impl App {
    fn initialize(&mut self, event_loop: &ActiveEventLoop) -> Result<()> {
        let window = Arc::new(
            event_loop.create_window(
                Window::default_attributes()
                    .with_title(format!("ironhour — {}", self.cli.map))
                    .with_inner_size(PhysicalSize::new(self.cli.width, self.cli.height))
                    .with_visible(!self.cli.smoke),
            )?,
        );
        let instance = wgpu::Instance::new(wgpu::InstanceDescriptor::new_without_display_handle_from_env());
        let surface = instance.create_surface(window.clone())?;
        let adapter = pollster::block_on(instance.request_adapter(&wgpu::RequestAdapterOptions {
            power_preference: wgpu::PowerPreference::HighPerformance,
            compatible_surface: Some(&surface),
            force_fallback_adapter: false,
            apply_limit_buckets: false,
        }))?;
        let mut config = surface
            .get_default_config(&adapter, self.cli.width, self.cli.height)
            .context("no surface configuration")?;
        // The original fixed-function colours are already display encoded.
        // Prefer a linear UNORM attachment to avoid an extra sRGB encode.
        if let Some(format) = surface.get_capabilities(&adapter).formats.iter().find(|f| !f.is_srgb()) {
            config.format = *format;
        }
        config.present_mode = wgpu::PresentMode::AutoVsync;
        let mut renderer = Renderer::with_adapter(&adapter, config.format)?;
        surface.configure(&renderer.device, &config);
        let (scene, simulation, sim_draw, camera) = self.load_game(&mut renderer, (config.width, config.height))?;
        let targets = renderer.targets((config.width, config.height));
        let now = Instant::now();
        let size = window.inner_size();
        let preferences_path =
            self.cli.audio_preferences.clone().unwrap_or_else(ih_audio::preferences::Preferences::path);
        let preferences = ih_audio::preferences::Preferences::load(&preferences_path).unwrap_or_else(|e| {
            eprintln!("audio preferences: {e:#}");
            Default::default()
        });
        self.state = Some(State {
            preferences,
            preferences_path,
            window,
            instance,
            surface,
            config,
            renderer,
            scene,
            targets,
            reset_camera: camera.clone(),
            camera,
            keys: HashSet::new(),
            cursor: None,
            pan_drag: false,
            orbit_drag: false,
            focused: false,
            size,
            started: now,
            last_frame: now,
            next_redraw: now,
            simulation,
            sim_draw,
            clock: ih_sim::LogicClock::default(),
            selected: Vec::new(),
            right_start: None,
            left_start: None,
            paused: false,
            feedback: audio::Feedback::default(),
            local_player: self.scenario.as_ref().and_then(|s| s.spawn.first()).map(|s| s.owner),
            ui: None,
            audio: if self.cli.no_audio || self.cli.smoke {
                None
            } else {
                match ih_audio::Audio::new(ih_audio::Catalog::load(&self.defs.data, &self.language)) {
                    Ok(mut audio) => {
                        audio.set_preferences(preferences)?;
                        audio.set_time_of_day(match self.world.map.lighting.as_ref().map(|l| l.time_of_day) {
                            Some(1) => "morning",
                            Some(3) => "evening",
                            Some(4) => "night",
                            _ => "afternoon",
                        });
                        Some(audio)
                    }
                    Err(e) => {
                        eprintln!("audio disabled: {e:#}");
                        None
                    }
                }
            },
        });
        if self.cli.shell {
            let st = self.state.as_mut().unwrap();
            let size = (st.config.width, st.config.height);
            st.ui = Some(shell::UiLayer::new(
                &self.vfs,
                &self.language,
                &self.defs,
                &st.renderer,
                size,
                Installs::locate().zero_hour.clone(),
            )?);
            st.ui.as_mut().unwrap().shell.audio_volumes =
                [st.preferences.effects, st.preferences.speech, st.preferences.music];
        }
        if !self.cli.no_music {
            let st = self.state.as_mut().unwrap();
            audio::start_music(&mut st.audio, &self.vfs, st.simulation.as_ref(), st.local_player, self.cli.shell);
        }
        if self.cli.smoke && self.cli.shell {
            // Click through the real menus to Start Game, play the skirmish briefly.
            let ui = self.state.as_mut().unwrap().ui.as_mut().unwrap();
            let mut started = None;
            for name in [
                "MainMenu.wnd:ButtonSinglePlayer",
                "MainMenu.wnd:ButtonSkirmish",
                "SkirmishGameOptionsMenu.wnd:ButtonStart",
            ] {
                for e in ui.shell.click(name) {
                    if let ih_ui::shell::ShellEvent::StartGame(setup) = e {
                        started = Some(setup);
                    }
                }
            }
            let setup = started.context("the menus did not start a game")?;
            self.start_skirmish(&setup)?;
            let s = self.state.as_mut().unwrap();
            let sim = s.simulation.as_mut().context("no simulation")?;
            for _ in 0..60 {
                sim.step();
            }
            let owned: Vec<(u32, String)> =
                sim.objects.values().filter(|o| o.map_index.is_none()).map(|o| (o.owner, o.template.clone())).collect();
            let money: Vec<(u32, u32)> = sim.players.keys().map(|&p| (p, sim.money(p))).collect();
            let rgba = s.renderer.render_scene(
                &s.scene,
                &s.camera.camera(&self.world.map),
                &self.world.object_lights(),
                (320, 240),
                [0.0, 0.0, 0.0, 1.0],
                0.0,
            );
            if !rgba.as_chunks::<4>().0.iter().any(|p| p[..3] != [0, 0, 0]) {
                bail!("skirmish render is empty");
            }
            eprintln!(
                "shell smoke passed: {} on {}; players' objects {owned:?}; money {money:?}",
                setup.player_name, self.cli.map
            );
            event_loop.exit();
            return Ok(());
        }
        if self.cli.smoke {
            let s = self.state.as_mut().unwrap();
            for (step, size) in [(0, (320, 240)), (1, (640, 360)), (2, (512, 512))] {
                s.resize(PhysicalSize::new(size.0, size.1));
                s.camera.pan(Vec2::new(0.2, 0.3), 0.1, false);
                s.camera.zoom(0.25);
                s.camera.orbit(0.1, 0.02);
                if let Some(sim) = &mut s.simulation {
                    if let Some(scenario) = &self.scenario {
                        scenario.verify_frame(sim)?;
                    }
                    for _ in 0..60 {
                        sim.step();
                        if let Some(scenario) = &self.scenario {
                            scenario.verify_frame(sim)?;
                        }
                        if self.scenario.as_ref().is_some_and(|s| s.require_no_overlaps) && !sim.overlaps().is_empty() {
                            bail!("simulation smoke overlap at frame {}: {:?}", sim.frame, sim.overlaps());
                        }
                    }
                    if !sim.events.is_empty() {
                        bail!("simulation smoke rejected commands: {:?}", sim.events);
                    }
                    if let Some(draw) = &mut s.sim_draw {
                        draw.update(&s.renderer, &mut s.scene, sim, 1.0, step as f32 * 1000.0, &s.selected);
                    }
                }
                let rgba = s.renderer.render_scene(
                    &s.scene,
                    &s.camera.camera(&self.world.map),
                    &self.world.object_lights(),
                    size,
                    [0.0, 0.0, 0.0, 1.0],
                    step as f32 * 1000.0,
                );
                if !rgba.as_chunks::<4>().0.iter().any(|p| p[..3] != [0, 0, 0]) {
                    bail!("smoke render {step} is empty");
                }
            }
            eprintln!(
                "window smoke passed: surface configured; three resize/camera/animated render checks; window stayed hidden"
            );
            event_loop.exit();
        } else {
            eprintln!(
                "WASD/arrows pan; wheel zoom; right drag pan; middle drag orbit; Q/E rotate; R/F pitch; Shift faster; Home reset; O overview; Esc quit"
            );
            if self.scenario.is_some() {
                eprintln!(
                    "Left click selects a spawned unit; right click moves it; X stops it; Space pauses/resumes. Recorded orders continue at their scheduled times."
                );
            }
            self.state.as_ref().unwrap().window.request_redraw();
        }
        Ok(())
    }
    /// The world, the scenario's simulation and drawables, and a camera, for
    /// the current map.
    fn load_game(&mut self, renderer: &mut Renderer, size: (u32, u32)) -> Result<Game> {
        let renderer = &mut *renderer;
        let mut assets = Assets::new(&self.vfs, &self.language);
        let mut scene = Scene::default();
        self.world.add_terrain(renderer, &mut assets, &self.vfs, &self.defs, &mut scene)?;
        self.world.add_roads(renderer, &mut assets, &self.defs, &mut scene);
        self.world.add_bridges(renderer, &mut assets, &self.defs, &mut scene);
        self.world.add_water(renderer, &mut assets, &self.defs, &mut scene);
        let objects = self.world.add_objects(renderer, &mut assets, &self.defs, &mut scene);
        let (simulation, sim_draw) = if let Some(scenario) = &self.scenario {
            let templates = Arc::new(ih_sim::templates::Templates::for_map(
                &self.defs.data,
                &self.world.map,
                &self.vfs,
                &self.language,
            )?);
            for w in &templates.warnings {
                eprintln!("template: {w}");
            }
            let sim = scenario.instantiate(Arc::new(self.world.map.clone()), templates.clone())?;
            let names =
                scenario.spawn.iter().map(|s| s.template.clone()).chain(scenario.commands.iter().filter_map(|c| {
                    match &c.command {
                        ih_sim::Command::Spawn { template, .. } => Some(template.clone()),
                        _ => None,
                    }
                }));
            // Their weapons' projectiles are drawn too.
            let mut names: Vec<String> = names.collect();
            for n in names.clone() {
                if let Some(t) = templates.objects.get(&n) {
                    for set in &t.weapon_sets {
                        for w in set.weapons.iter().flatten() {
                            if let Some(p) = templates.weapons.get(w).and_then(|w| w.projectile.clone())
                                && !names.contains(&p)
                            {
                                names.push(p);
                            }
                        }
                    }
                }
            }
            let draw = ih_client::sim_draw::SimDrawables::new(
                renderer,
                &mut assets,
                &templates,
                names.into_iter(),
                &self.world.condition_flags(),
                scene.instances.len(),
            );
            eprintln!("M4 movement slice: {} objects; explicit stub modules: {:?}", sim.objects.len(), sim.stub_counts);
            (Some(sim), Some(draw))
        } else {
            (None, None)
        };
        for w in self.world.warnings.iter().chain(&assets.warnings) {
            eprintln!("warning: {w}");
        }
        eprintln!(
            "{}: {objects} objects; {} road types; {} bridges; {}",
            self.world.path,
            scene.roads.len(),
            scene.bridges.len(),
            renderer.adapter_name
        );
        let mut camera = TacticalCamera::new(self.world.playable_bounds(), &self.defs);
        if let Some(sim) = &simulation
            && sim.objects.values().any(|o| o.map_index.is_none())
        {
            let (mut lo, mut hi) = (Vec2::splat(f32::INFINITY), Vec2::splat(f32::NEG_INFINITY));
            for o in sim.objects.values().filter(|o| o.map_index.is_none()) {
                let p = Vec2::new(o.position[0], o.position[1]);
                lo = lo.min(p);
                hi = hi.max(p);
            }
            camera.center = (lo + hi) * 0.5;
            let half_tan = (camera.fov_y * 0.5).tan() * (size.0 as f32 / size.1.max(1) as f32).min(1.0);
            camera.distance = camera.distance.max((hi - lo).length() * 0.5 / half_tan + 80.0);
        }
        if self.cli.overview {
            camera.overview();
        }
        Ok((scene, simulation, sim_draw, camera))
    }

    /// Start Game from the skirmish menu: load its map and play the setup.
    fn start_skirmish(&mut self, setup: &ih_ui::shell::GameSetup) -> Result<()> {
        let map = setup.map.as_ref().context("no map chosen")?.folder();
        let world = World::load(&self.vfs, &map)?;
        let mut defs = Definitions::load(&self.vfs, &self.language);
        defs.load_map_overrides(&self.vfs, &world.path)?;
        let sides =
            self.state.as_ref().and_then(|s| s.ui.as_ref()).map(|u| u.shell.data.sides.clone()).unwrap_or_default();
        let text = shell::scenario_for(setup, &sides, &defs, &world)?;
        let scenario: ih_sim::scenario::Scenario = toml::from_str(&text)?;
        self.world = world;
        self.defs = defs;
        self.scenario = Some(scenario);
        self.cli.map = map;
        let mut st = self.state.take().context("no window")?;
        let size = (st.config.width, st.config.height);
        let built = self.load_game(&mut st.renderer, size);
        let (scene, simulation, sim_draw, camera) = match built {
            Ok(g) => g,
            Err(e) => {
                self.state = Some(st);
                return Err(e);
            }
        };
        st.scene = scene;
        st.simulation = simulation;
        st.local_player = self.scenario.as_ref().and_then(|s| s.spawn.first()).map(|s| s.owner);
        st.sim_draw = sim_draw;
        st.reset_camera = camera.clone();
        st.camera = camera;
        st.clock = ih_sim::LogicClock::default();
        st.selected.clear();
        st.ui = None;
        st.feedback = audio::Feedback::default();
        if let Some(audio) = &mut st.audio {
            audio.set_time_of_day(match self.world.map.lighting.as_ref().map(|l| l.time_of_day) {
                Some(1) => "morning",
                Some(3) => "evening",
                Some(4) => "night",
                _ => "afternoon",
            });
        }
        if !self.cli.no_music {
            audio::start_music(&mut st.audio, &self.vfs, st.simulation.as_ref(), st.local_player, false);
        }
        st.window.set_title(&format!("ironhour — {}", self.cli.map));
        self.state = Some(st);
        Ok(())
    }

    fn fail(&mut self, event_loop: &ActiveEventLoop, e: anyhow::Error) {
        self.error = Some(e);
        event_loop.exit();
    }
}

impl State {
    fn resize(&mut self, size: PhysicalSize<u32>) {
        self.size = size;
        if size.width == 0 || size.height == 0 {
            return;
        }
        self.config.width = size.width;
        self.config.height = size.height;
        self.surface.configure(&self.renderer.device, &self.config);
        self.targets = self.renderer.targets((size.width, size.height));
    }
    fn render(&mut self, world: &World, vfs: &Vfs, language: &str) -> Result<()> {
        if self.size.width == 0 || self.size.height == 0 {
            return Ok(());
        }
        let now = Instant::now();
        let elapsed = now - self.last_frame;
        let dt = elapsed.as_secs_f32().min(0.1);
        self.last_frame = now;
        if let Some(audio) = &mut self.audio {
            audio.update_listener(audio::listener(&self.camera, &world.map));
            for error in audio.update_media(vfs, self.started.elapsed()) {
                eprintln!("audio: {error}");
            }
        }
        if let Some(sim) = &mut self.simulation {
            if !self.paused {
                for _ in 0..self.clock.advance(elapsed) {
                    sim.step();
                    audio::dispatch(&mut self.audio, sim, vfs, self.local_player);
                    audio::feedback(
                        &mut self.audio,
                        sim,
                        vfs,
                        &mut self.feedback,
                        self.local_player,
                        self.started.elapsed(),
                    );
                }
            }
            if let Some(draw) = &mut self.sim_draw {
                draw.update(
                    &self.renderer,
                    &mut self.scene,
                    sim,
                    self.clock.alpha(),
                    (sim.frame.saturating_sub(1) as f32 + self.clock.alpha()) * 1000.0 / 30.0,
                    &self.selected,
                );
            }
            for event in sim.events.drain(..) {
                eprintln!("rejected: {event}");
            }
        }
        let key = |keys: &[KeyCode]| f32::from(u8::from(keys.iter().any(|k| self.keys.contains(k))));
        let mut pan = Vec2::new(
            key(&[KeyCode::KeyD, KeyCode::ArrowRight]) - key(&[KeyCode::KeyA, KeyCode::ArrowLeft]),
            key(&[KeyCode::KeyW, KeyCode::ArrowUp]) - key(&[KeyCode::KeyS, KeyCode::ArrowDown]),
        );
        if self.focused
            && !self.pan_drag
            && !self.orbit_drag
            && let Some(cursor) = self.cursor
        {
            if cursor.x < 12.0 {
                pan.x -= 1.0;
            } else if cursor.x > self.size.width as f32 - 12.0 {
                pan.x += 1.0;
            }
            if cursor.y < 12.0 {
                pan.y += 1.0;
            } else if cursor.y > self.size.height as f32 - 12.0 {
                pan.y -= 1.0;
            }
        }
        let fast = self.keys.contains(&KeyCode::ShiftLeft) || self.keys.contains(&KeyCode::ShiftRight);
        let yaw = (key(&[KeyCode::KeyE]) - key(&[KeyCode::KeyQ])) * dt;
        let pitch = (key(&[KeyCode::KeyR]) - key(&[KeyCode::KeyF])) * dt * 0.5;
        self.camera.pan(pan, dt, fast);
        self.camera.orbit(yaw, pitch);
        let (frame, suboptimal) = match self.surface.get_current_texture() {
            wgpu::CurrentSurfaceTexture::Success(f) => (f, false),
            wgpu::CurrentSurfaceTexture::Suboptimal(f) => (f, true),
            wgpu::CurrentSurfaceTexture::Outdated => {
                self.resize(self.size);
                return Ok(());
            }
            wgpu::CurrentSurfaceTexture::Lost => {
                self.surface = self.instance.create_surface(self.window.clone())?;
                self.resize(self.size);
                return Ok(());
            }
            wgpu::CurrentSurfaceTexture::Timeout | wgpu::CurrentSurfaceTexture::Occluded => return Ok(()),
            wgpu::CurrentSurfaceTexture::Validation => bail!("surface acquisition failed validation"),
        };
        let view = frame.texture.create_view(&Default::default());
        let mut enc = self.renderer.device.create_command_encoder(&Default::default());
        let camera = self.camera.camera(&world.map);
        self.renderer.draw(
            &mut enc,
            &self.scene,
            &camera,
            &world.object_lights(),
            &self.targets,
            &view,
            [0.0, 0.0, 0.0, 1.0],
            self.started.elapsed().as_secs_f32() * 1000.0,
        );
        if let Some(ui) = &mut self.ui {
            let size = (self.config.width, self.config.height);
            ui.draw(&mut self.renderer, vfs, language, &mut enc, &view, size);
        }
        self.renderer.queue.submit([enc.finish()]);
        self.window.pre_present_notify();
        self.renderer.queue.present(frame);
        if suboptimal {
            self.resize(self.size);
        }
        Ok(())
    }
}

impl ApplicationHandler for App {
    fn resumed(&mut self, event_loop: &ActiveEventLoop) {
        if self.state.is_none()
            && let Err(e) = self.initialize(event_loop)
        {
            self.fail(event_loop, e);
        }
    }
    fn window_event(&mut self, event_loop: &ActiveEventLoop, id: WindowId, event: WindowEvent) {
        let Some(s) = &mut self.state else { return };
        if id != s.window.id() {
            return;
        }
        if let Some(ui) = &mut s.ui {
            let p = |v: Vec2| [v.x as i32, v.y as i32];
            let input = match &event {
                WindowEvent::CursorMoved { position, .. } => {
                    s.cursor = Some(Vec2::new(position.x as f32, position.y as f32));
                    Some(ih_ui::input::Input::MouseMove(p(s.cursor.unwrap())))
                }
                WindowEvent::MouseInput { state, button: MouseButton::Left, .. } => s.cursor.map(|c| {
                    if state.is_pressed() {
                        ih_ui::input::Input::LeftDown(p(c))
                    } else {
                        ih_ui::input::Input::LeftUp(p(c))
                    }
                }),
                WindowEvent::KeyboardInput { event, .. } if event.state.is_pressed() => match event.physical_key {
                    PhysicalKey::Code(KeyCode::Backspace) => Some(ih_ui::input::Input::Backspace),
                    PhysicalKey::Code(KeyCode::Enter) => Some(ih_ui::input::Input::Enter),
                    _ => event
                        .text
                        .as_ref()
                        .and_then(|t| t.chars().next())
                        .filter(|c| !c.is_control())
                        .map(ih_ui::input::Input::Char),
                },
                _ => None,
            };
            if let Some(input) = input {
                let events = ui.input(&input);
                for e in events {
                    match e {
                        ih_ui::shell::ShellEvent::Quit => event_loop.exit(),
                        ih_ui::shell::ShellEvent::StartGame(setup) => {
                            if let Err(e) = self.start_skirmish(&setup) {
                                eprintln!("could not start the skirmish: {e:#}");
                            }
                            return;
                        }
                        ih_ui::shell::ShellEvent::AudioPreview(values)
                        | ih_ui::shell::ShellEvent::AudioSave(values) => {
                            let save = matches!(e, ih_ui::shell::ShellEvent::AudioSave(_));
                            let st = self.state.as_mut().unwrap();
                            st.preferences = ih_audio::preferences::Preferences {
                                effects: values[0],
                                speech: values[1],
                                music: values[2],
                            };
                            if let Some(audio) = &mut st.audio
                                && let Err(e) = audio.set_preferences(st.preferences)
                            {
                                eprintln!("volume: {e:#}");
                            }
                            if save && let Err(e) = st.preferences.save(&st.preferences_path) {
                                eprintln!("save audio settings: {e:#}");
                            }
                        }
                        ih_ui::shell::ShellEvent::Sound(name) => {
                            if let Some(audio) = self.state.as_mut().and_then(|s| s.audio.as_mut())
                                && let Err(e) = audio.play_ui(&self.vfs, &name)
                            {
                                eprintln!("interface sound: {e:#}");
                            }
                        }
                    }
                }
                return;
            }
        }
        let Some(s) = &mut self.state else { return };
        match event {
            WindowEvent::CloseRequested => event_loop.exit(),
            WindowEvent::Resized(size) => s.resize(size),
            WindowEvent::RedrawRequested => {
                if let Err(e) = s.render(&self.world, &self.vfs, &self.language) {
                    self.fail(event_loop, e);
                }
            }
            WindowEvent::Focused(f) => {
                s.focused = f;
                if !f {
                    s.keys.clear();
                    s.pan_drag = false;
                    s.orbit_drag = false;
                }
            }
            WindowEvent::KeyboardInput { event, .. } => {
                if let PhysicalKey::Code(code) = event.physical_key {
                    if event.state.is_pressed() {
                        s.keys.insert(code);
                        match code {
                            KeyCode::Escape => event_loop.exit(),
                            KeyCode::Home => s.camera = s.reset_camera.clone(),
                            KeyCode::KeyO => s.camera.overview(),
                            KeyCode::Space if !event.repeat => s.paused = !s.paused,
                            KeyCode::KeyX if !event.repeat => {
                                if let Some(sim) = &mut s.simulation {
                                    for &object in &s.selected {
                                        sim.enqueue(sim.frame, ih_sim::Command::Stop { object }).ok();
                                    }
                                }
                            }
                            _ => {}
                        }
                    } else {
                        s.keys.remove(&code);
                    }
                }
            }
            WindowEvent::CursorMoved { position, .. } => {
                let current = Vec2::new(position.x as f32, position.y as f32);
                if let Some(previous) = s.cursor {
                    let delta = current - previous;
                    if s.pan_drag
                        && (s.simulation.is_none() || s.right_start.is_some_and(|p| p.distance(current) > 5.0))
                    {
                        s.camera.drag_pan(delta, s.size.height as f32);
                    }
                    if s.orbit_drag {
                        s.camera.orbit(-delta.x * 0.006, -delta.y * 0.006);
                    }
                }
                s.cursor = Some(current);
            }
            WindowEvent::CursorLeft { .. } => s.cursor = None,
            WindowEvent::MouseInput { state, button, .. } => match button {
                MouseButton::Left if state.is_pressed() => s.left_start = s.cursor,
                MouseButton::Left => {
                    if let (Some(sim), Some(cursor), Some(start)) = (&s.simulation, s.cursor, s.left_start.take()) {
                        let vp = s
                            .camera
                            .camera(&self.world.map)
                            .view_proj(s.size.width as f32 / s.size.height.max(1) as f32);
                        let size = Vec2::new(s.size.width as f32, s.size.height as f32);
                        // The map's own objects are scenery here; scenario units can be selected.
                        let on_screen: Vec<(u32, Vec2)> = sim
                            .objects
                            .values()
                            .filter(|o| o.map_index.is_none())
                            .filter_map(|o| {
                                let p = vp * glam::Vec3::from(o.position).extend(1.0);
                                (p.w > 0.0).then(|| {
                                    let ndc = p.truncate() / p.w;
                                    (o.id, Vec2::new((ndc.x + 1.0) * 0.5 * size.x, (1.0 - ndc.y) * 0.5 * size.y))
                                })
                            })
                            .collect();
                        s.selected = if start.distance(cursor) > 5.0 {
                            // Drag box: everything inside it.
                            let (lo, hi) = (start.min(cursor), start.max(cursor));
                            on_screen
                                .iter()
                                .filter(|(_, p)| p.x >= lo.x && p.y >= lo.y && p.x <= hi.x && p.y <= hi.y)
                                .map(|(id, _)| *id)
                                .collect()
                        } else {
                            on_screen
                                .iter()
                                .map(|(id, p)| (p.distance_squared(cursor), *id))
                                .filter(|(d, _)| *d < 24.0 * 24.0)
                                .min_by(|a, b| a.0.total_cmp(&b.0).then(a.1.cmp(&b.1)))
                                .map(|(_, id)| id)
                                .into_iter()
                                .collect()
                        };
                        s.window.set_title(&format!(
                            "ironhour — M4 movement — {} selected {:?}",
                            s.selected.len(),
                            s.selected
                        ));
                    }
                }
                MouseButton::Right => {
                    if state.is_pressed() {
                        s.right_start = s.cursor;
                    } else if let (Some(start), Some(cursor)) = (s.right_start, s.cursor)
                        && start.distance(cursor) <= 5.0
                        && !s.selected.is_empty()
                        && let Some(sim) = &mut s.simulation
                        && let Some(target) =
                            s.camera.surface_at_pixel(&self.world.map, &sim.grid, [s.size.width, s.size.height], cursor)
                    {
                        let target = target.to_array();
                        let vp = s
                            .camera
                            .camera(&self.world.map)
                            .view_proj(s.size.width as f32 / s.size.height.max(1) as f32);
                        let size = Vec2::new(s.size.width as f32, s.size.height as f32);
                        // The object under the cursor, map objects included.
                        let under = sim
                            .objects
                            .values()
                            .filter(|o| !o.dead && !s.selected.contains(&o.id))
                            .filter_map(|o| {
                                let p = vp * glam::Vec3::from(o.position).extend(1.0);
                                (p.w > 0.0).then(|| {
                                    let ndc = p.truncate() / p.w;
                                    let screen = Vec2::new((ndc.x + 1.0) * 0.5 * size.x, (1.0 - ndc.y) * 0.5 * size.y);
                                    (screen.distance_squared(cursor), o.id)
                                })
                            })
                            .filter(|(d, _)| *d < 24.0 * 24.0)
                            .min_by(|a, b| a.0.total_cmp(&b.0).then(a.1.cmp(&b.1)))
                            .map(|(_, id)| id);
                        let held = |a: KeyCode, b: KeyCode| s.keys.contains(&a) || s.keys.contains(&b);
                        let force = held(KeyCode::ControlLeft, KeyCode::ControlRight);
                        let guard = held(KeyCode::AltLeft, KeyCode::AltRight);
                        let owner = sim.objects.get(&s.selected[0]).map_or(0, |o| o.owner);
                        let enemy =
                            under.filter(|u| sim.objects.get(u).is_some_and(|o| o.owner != owner && o.owner != 0));
                        let armed = |o: &u32| {
                            sim.objects.get(o).is_some_and(|o| o.weapon_set.weapons.iter().any(Option::is_some))
                        };
                        let commands: Vec<ih_sim::Command> = if guard {
                            // Alt: guard the point.
                            s.selected
                                .iter()
                                .map(|&object| ih_sim::Command::Guard { object, position: target })
                                .collect()
                        } else if let Some(victim) = enemy.or(under.filter(|_| force)) {
                            // An enemy (or anything, with Ctrl): attack it; the unarmed move there.
                            s.selected
                                .iter()
                                .map(|&object| {
                                    if armed(&object) {
                                        ih_sim::Command::Attack { object, target: victim }
                                    } else {
                                        ih_sim::Command::Move { object, target }
                                    }
                                })
                                .collect()
                        } else if force {
                            // Ctrl on the ground: force-fire there.
                            s.selected
                                .iter()
                                .filter(|o| armed(o))
                                .map(|&object| ih_sim::Command::AttackPosition { object, target })
                                .collect()
                        } else if let [object] = s.selected[..] {
                            vec![ih_sim::Command::Move { object, target }]
                        } else {
                            vec![ih_sim::Command::GroupMove { objects: s.selected.clone(), target }]
                        };
                        let attack = commands.iter().any(|c| {
                            matches!(c, ih_sim::Command::Attack { .. } | ih_sim::Command::AttackPosition { .. })
                        });
                        audio::unit_voice(
                            &mut s.audio,
                            sim,
                            &s.selected,
                            s.local_player,
                            if attack { "VoiceAttack" } else { "VoiceMove" },
                            s.started.elapsed(),
                        );
                        for command in commands {
                            sim.enqueue(sim.frame, command).ok();
                        }
                    }
                    s.pan_drag = state.is_pressed();
                    if !state.is_pressed() {
                        s.right_start = None;
                    }
                }
                MouseButton::Middle => s.orbit_drag = state.is_pressed(),
                _ => {}
            },
            WindowEvent::MouseWheel { delta, .. } => s.camera.zoom(match delta {
                MouseScrollDelta::LineDelta(_, y) => y,
                MouseScrollDelta::PixelDelta(p) => p.y as f32 / 60.0,
            }),
            _ => {}
        }
    }
    fn about_to_wait(&mut self, event_loop: &ActiveEventLoop) {
        if self.cli.smoke {
            return;
        }
        if let Some(s) = &mut self.state {
            if s.size.width == 0 || s.size.height == 0 {
                if let Some(audio) = &mut s.audio {
                    for error in audio.update_media(&self.vfs, s.started.elapsed()) {
                        eprintln!("audio: {error}");
                    }
                }
                if let Some(sim) = &mut s.simulation {
                    let now = Instant::now();
                    if !s.paused {
                        for _ in 0..s.clock.advance(now - s.last_frame) {
                            sim.step();
                            audio::dispatch(&mut s.audio, sim, &self.vfs, s.local_player);
                            audio::feedback(
                                &mut s.audio,
                                sim,
                                &self.vfs,
                                &mut s.feedback,
                                s.local_player,
                                s.started.elapsed(),
                            );
                        }
                    }
                    s.last_frame = now;
                    event_loop.set_control_flow(ControlFlow::WaitUntil(now + Duration::from_millis(33)));
                } else {
                    event_loop.set_control_flow(ControlFlow::Wait);
                }
                return;
            }
            let now = Instant::now();
            if now >= s.next_redraw {
                s.window.request_redraw();
                s.next_redraw = now + Duration::from_millis(if s.focused { 16 } else { 100 });
            }
            event_loop.set_control_flow(ControlFlow::WaitUntil(s.next_redraw));
        }
    }
}
