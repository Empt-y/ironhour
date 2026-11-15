//! Native map viewer. Replaces the window/frame-loop portion of `WinMain.cpp`;
//! map loading and drawing use the same client/renderer as the headless tools.
use anyhow::{Context, Result, bail};
use clap::Parser;
use glam::Vec2;
use ih_client::{Definitions, TacticalCamera, World};
use ih_render::{Assets, Renderer, Scene, Targets};
use ih_vfs::{Installs, ModProfile, Vfs};
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
    selected: Option<u32>,
    right_start: Option<Vec2>,
    paused: bool,
}

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
        let mut assets = Assets::new(&self.vfs, &self.language);
        let mut scene = Scene::default();
        self.world.add_terrain(&mut renderer, &mut assets, &self.vfs, &self.defs, &mut scene)?;
        self.world.add_roads(&mut renderer, &mut assets, &self.defs, &mut scene);
        self.world.add_bridges(&mut renderer, &mut assets, &self.defs, &mut scene);
        self.world.add_water(&mut renderer, &mut assets, &self.defs, &mut scene);
        let objects = self.world.add_objects(&mut renderer, &mut assets, &self.defs, &mut scene);
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
            let draw = ih_client::sim_draw::SimDrawables::new(
                &mut renderer,
                &mut assets,
                &templates,
                names,
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
            let half_tan = (camera.fov_y * 0.5).tan() * (config.width as f32 / config.height as f32).min(1.0);
            camera.distance = camera.distance.max((hi - lo).length() * 0.5 / half_tan + 80.0);
        }
        if self.cli.overview {
            camera.overview();
        }
        let targets = renderer.targets((config.width, config.height));
        let now = Instant::now();
        let size = window.inner_size();
        self.state = Some(State {
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
            selected: None,
            right_start: None,
            paused: false,
        });
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
                        draw.update(&s.renderer, &mut s.scene, sim, 1.0, step as f32 * 1000.0, s.selected);
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
    fn render(&mut self, world: &World) -> Result<()> {
        if self.size.width == 0 || self.size.height == 0 {
            return Ok(());
        }
        let now = Instant::now();
        let elapsed = now - self.last_frame;
        let dt = elapsed.as_secs_f32().min(0.1);
        self.last_frame = now;
        if let Some(sim) = &mut self.simulation {
            if !self.paused {
                for _ in 0..self.clock.advance(elapsed) {
                    sim.step();
                }
            }
            if let Some(draw) = &mut self.sim_draw {
                draw.update(
                    &self.renderer,
                    &mut self.scene,
                    sim,
                    self.clock.alpha(),
                    (sim.frame.saturating_sub(1) as f32 + self.clock.alpha()) * 1000.0 / 30.0,
                    self.selected,
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
        match event {
            WindowEvent::CloseRequested => event_loop.exit(),
            WindowEvent::Resized(size) => s.resize(size),
            WindowEvent::RedrawRequested => {
                if let Err(e) = s.render(&self.world) {
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
                                if let (Some(sim), Some(object)) = (&mut s.simulation, s.selected) {
                                    sim.enqueue(sim.frame, ih_sim::Command::Stop { object }).ok();
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
                MouseButton::Left if state.is_pressed() => {
                    if let (Some(sim), Some(cursor)) = (&s.simulation, s.cursor) {
                        let vp = s
                            .camera
                            .camera(&self.world.map)
                            .view_proj(s.size.width as f32 / s.size.height.max(1) as f32);
                        s.selected = sim
                            .objects
                            .values()
                            .filter(|o| o.map_index.is_none())
                            .filter_map(|o| {
                                let p = vp * glam::Vec3::from(o.position).extend(1.0);
                                if p.w <= 0.0 {
                                    return None;
                                }
                                let ndc = p.truncate() / p.w;
                                let screen = Vec2::new(
                                    (ndc.x + 1.0) * 0.5 * s.size.width as f32,
                                    (1.0 - ndc.y) * 0.5 * s.size.height as f32,
                                );
                                let dist = screen.distance_squared(cursor);
                                (dist < 24.0 * 24.0).then_some((dist, o.id))
                            })
                            .min_by(|a, b| a.0.total_cmp(&b.0).then(a.1.cmp(&b.1)))
                            .map(|(_, id)| id);
                        s.window.set_title(&format!("ironhour — M4 movement — selected {:?}", s.selected));
                    }
                }
                MouseButton::Right => {
                    if state.is_pressed() {
                        s.right_start = s.cursor;
                    } else if let (Some(start), Some(cursor)) = (s.right_start, s.cursor)
                        && start.distance(cursor) <= 5.0
                        && let (Some(object), Some(sim)) = (s.selected, &mut s.simulation)
                        && let Some(target) =
                            s.camera.surface_at_pixel(&self.world.map, &sim.grid, [s.size.width, s.size.height], cursor)
                    {
                        sim.enqueue(sim.frame, ih_sim::Command::Move { object, target: target.to_array() }).ok();
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
                if let Some(sim) = &mut s.simulation {
                    let now = Instant::now();
                    if !s.paused {
                        for _ in 0..s.clock.advance(now - s.last_frame) {
                            sim.step();
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
