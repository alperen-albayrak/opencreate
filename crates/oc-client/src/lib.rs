//! The game client: window, input, menu screens and the frame loop
//! (ARCHITECTURE.md §2). Per-world state lives in [`session::Session`];
//! this shell owns the window, renderer, registry and menu navigation.

mod avatar;
mod camera;
mod craft_menu;
mod entities;
mod far_terrain;
mod hotbar;
mod menu;
mod player;
mod session;
mod settings;
mod sky;
mod streaming;

use std::path::PathBuf;
use std::time::{Duration, Instant};

use anyhow::Result;
use raw_window_handle::{HasDisplayHandle, HasWindowHandle};
use tracing::{error, info};
use winit::application::ApplicationHandler;
use winit::dpi::LogicalSize;
use winit::event::{DeviceEvent, DeviceId, ElementState, MouseButton, WindowEvent};
use winit::event_loop::{ActiveEventLoop, ControlFlow, EventLoop};
use winit::keyboard::{KeyCode, PhysicalKey};
use winit::window::{CursorGrabMode, Window, WindowAttributes, WindowId};

use menu::{CreateScreen, MenuView, SettingsScreen, WorldAction, WorldsScreen};
use oc_assets::Registry;
use oc_protocol::ClientMessage;
use oc_renderer::{FrameCamera, Renderer};
use session::Session;
use settings::Settings;

/// Worlds live in subdirectories of this folder.
const SAVES_ROOT: &str = "saves";

/// Runs the client until the window is closed.
pub fn run() -> Result<()> {
    let event_loop = EventLoop::new()?;
    event_loop.set_control_flow(ControlFlow::Poll);
    let mut app = App::new()?;
    event_loop.run_app(&mut app)?;
    match app.error.take() {
        Some(err) => Err(err),
        None => Ok(()),
    }
}

/// Which screen has the input focus. `InGame`/`Paused`/`Modes` imply a
/// session.
enum Screen {
    Title,
    Worlds(WorldsScreen),
    CreateWorld(CreateScreen),
    /// A world is starting on a worker thread; poll for the session.
    Loading {
        rx: std::sync::mpsc::Receiver<Result<Session>>,
        world: String,
        since: Instant,
    },
    InGame,
    Paused,
    /// The game-mode picker, reached from the pause menu.
    Modes,
    Settings(SettingsScreen),
}

struct App {
    // Field order matters: the renderer (and its surface) must drop before
    // the window it was created from.
    renderer: Option<Renderer>,
    window: Option<Window>,
    error: Option<anyhow::Error>,
    screen: Screen,
    /// The loaded world, if any (None on the title/world screens).
    session: Option<Session>,
    registry: Registry,
    mouse_captured: bool,
    /// Cursor position in physical pixels, for menu hit-testing.
    mouse_pos: (f32, f32),
    last_frame: Instant,
    perf: PerfLog,
    hud_visible: bool,
    /// Exponentially smoothed frame time, for the HUD readout.
    frame_time_ema: f64,
    settings: Settings,
    /// Index of the settings slider being dragged, while the button is down.
    drag_slider: Option<usize>,
    /// App epoch for shader animation time (waves).
    started: Instant,
}

/// Aggregates frame times and logs a summary periodically (§11 budgets).
struct PerfLog {
    window_start: Instant,
    frames: u32,
    worst_frame: Duration,
}

impl PerfLog {
    const INTERVAL: Duration = Duration::from_secs(5);

    fn new() -> Self {
        Self {
            window_start: Instant::now(),
            frames: 0,
            worst_frame: Duration::ZERO,
        }
    }

    fn frame(&mut self, frame_time: Duration, renderer: &Renderer) {
        self.frames += 1;
        self.worst_frame = self.worst_frame.max(frame_time);
        let elapsed = self.window_start.elapsed();
        if elapsed >= Self::INTERVAL {
            let stats = renderer.stats();
            info!(
                fps = (self.frames as f64 / elapsed.as_secs_f64()).round(),
                worst_ms = format!("{:.1}", self.worst_frame.as_secs_f64() * 1e3),
                chunks_drawn = stats.chunks_drawn,
                chunks_resident = stats.chunks_resident,
                "perf"
            );
            *self = Self::new();
        }
    }
}

/// Existing world names: the subdirectories of `saves/`, sorted.
fn list_worlds() -> Vec<String> {
    let mut worlds: Vec<String> = std::fs::read_dir(SAVES_ROOT)
        .into_iter()
        .flatten()
        .flatten()
        .filter(|entry| entry.path().is_dir())
        .filter_map(|entry| entry.file_name().into_string().ok())
        .collect();
    worlds.sort();
    worlds
}

/// A seed nobody typed: from the clock ("leave it blank for random").
fn random_seed() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_nanos() as u64)
        .unwrap_or(1)
}

impl App {
    fn new() -> Result<Self> {
        Ok(Self {
            renderer: None,
            window: None,
            error: None,
            screen: Screen::Title,
            session: None,
            registry: Registry::load_default()?,
            mouse_captured: false,
            mouse_pos: (0.0, 0.0),
            last_frame: Instant::now(),
            perf: PerfLog::new(),
            hud_visible: true,
            frame_time_ema: 1.0 / 60.0,
            settings: Settings::load(),
            drag_slider: None,
            started: Instant::now(),
        })
    }

    /// Effective UI scale: the display's DPI factor times the user's
    /// UI-scale setting, so 4K monitors and TVs are both tunable.
    fn ui(&self) -> f32 {
        let dpi = self.window.as_ref().map_or(1.0, |w| w.scale_factor() as f32);
        dpi * self.settings.ui_scale
    }

    /// Pushes the current settings into the live session and the camera.
    fn apply_settings(&mut self) {
        if let Some(renderer) = &mut self.renderer {
            renderer.set_resolution_scale(self.settings.resolution_scale);
        }
        if let Some(session) = &mut self.session {
            session.camera.fov_y = self.settings.fov.to_radians();
            session.camera.sensitivity = self.settings.mouse_sensitivity;
            session.streamer.set_radius(self.settings.render_distance);
        }
    }

    /// Saves + applies the settings screen's values and returns to where
    /// it was opened from.
    fn leave_settings(&mut self) {
        self.drag_slider = None;
        let back_to_pause = if let Screen::Settings(screen) = &self.screen {
            screen.apply(&mut self.settings);
            screen.back_to_pause
        } else {
            return;
        };
        self.settings.save();
        self.apply_settings();
        self.screen = if back_to_pause { Screen::Paused } else { Screen::Title };
    }

    /// Live-applies slider values while dragging. The UI scale is held
    /// back until release so the screen doesn't re-lay-out under the
    /// cursor mid-drag.
    fn drag_apply(&mut self) {
        let ui_scale = self.settings.ui_scale;
        if let Screen::Settings(screen) = &self.screen {
            screen.apply(&mut self.settings);
        }
        self.settings.ui_scale = ui_scale;
        self.apply_settings();
    }

    fn window_size(&self) -> (f32, f32) {
        self.window.as_ref().map_or((1280.0, 720.0), |window| {
            let size = window.inner_size();
            (size.width.max(1) as f32, size.height.max(1) as f32)
        })
    }

    fn init(&mut self, event_loop: &ActiveEventLoop) -> Result<()> {
        let window = event_loop.create_window(
            WindowAttributes::default()
                .with_title("OpenCreate")
                .with_inner_size(LogicalSize::new(1280, 720)),
        )?;
        let size = window.inner_size();
        // SAFETY: the window is stored in `self` and declared after the
        // renderer, so it outlives the renderer's surface.
        let renderer = unsafe {
            Renderer::new(
                window.display_handle()?.as_raw(),
                window.window_handle()?.as_raw(),
                size.width,
                size.height,
            )?
        };
        info!("renderer initialized");
        self.window = Some(window);
        self.renderer = Some(renderer);
        self.apply_settings(); // resolution scale etc. from settings.ron
        // Dev hook: OC_WORLD=<name> skips the menus into a world (used by
        // graphics verification; harmless in normal play).
        if let Ok(name) = std::env::var("OC_WORLD") {
            let seed = std::env::var("OC_SEED")
                .ok()
                .and_then(|s| s.parse().ok())
                .unwrap_or_else(random_seed);
            self.start_session(&menu::sanitize_name(&name), seed, None, Some(true));
        }
        self.last_frame = Instant::now();
        Ok(())
    }

    fn fail(&mut self, event_loop: &ActiveEventLoop, err: anyhow::Error) {
        error!("fatal: {err:#}");
        self.error = Some(err);
        event_loop.exit();
    }

    fn set_mouse_captured(&mut self, captured: bool) {
        let Some(window) = &self.window else { return };
        let grab = if captured {
            window
                .set_cursor_grab(CursorGrabMode::Locked)
                .or_else(|_| window.set_cursor_grab(CursorGrabMode::Confined))
        } else {
            window.set_cursor_grab(CursorGrabMode::None)
        };
        if let Err(err) = grab {
            error!("cursor grab failed: {err}");
            return;
        }
        window.set_cursor_visible(!captured);
        self.mouse_captured = captured;
    }

    /// Opens a world: the embedded server starts on a worker thread while
    /// the loading screen animates; the frame loop polls for the result.
    fn start_session(&mut self, name: &str, seed: u64, mode: Option<String>, cheats: Option<bool>) {
        info!(world = name, "loading world");
        let (tx, rx) = std::sync::mpsc::channel();
        let dir = PathBuf::from(SAVES_ROOT).join(name);
        std::thread::spawn(move || {
            let _ = tx.send(Session::start(dir, seed, mode, cheats));
        });
        self.screen = Screen::Loading { rx, world: name.to_owned(), since: Instant::now() };
    }

    /// Polls a pending world start; returns true when handled.
    fn poll_loading(&mut self) {
        let Screen::Loading { rx, world, .. } = &self.screen else {
            return;
        };
        match rx.try_recv() {
            Ok(Ok(mut session)) => {
                // Dev hook companion to OC_WORLD: OC_POS=x,y,z places the
                // camera (graphics verification; harmless in normal play).
                if let Ok(pos) = std::env::var("OC_POS") {
                    let v: Vec<f64> = pos.split(',').filter_map(|s| s.trim().parse().ok()).collect();
                    if let [x, y, z] = v[..] {
                        session.player.position = glam::DVec3::new(x, y, z);
                        session.camera.position = session.player.eye();
                    }
                }
                if let Ok(cam) = std::env::var("OC_CAM") {
                    session.camera_mode = match cam.as_str() {
                        "back" => session::CameraMode::ThirdBack,
                        "front" => session::CameraMode::ThirdFront,
                        _ => session::CameraMode::FirstPerson,
                    };
                }
                if let Ok(look) = std::env::var("OC_LOOK") {
                    let v: Vec<f32> = look.split(',').filter_map(|s| s.trim().parse().ok()).collect();
                    if let [yaw, pitch] = v[..] {
                        session.camera.yaw = yaw;
                        session.camera.pitch = pitch;
                    }
                }
                self.session = Some(session);
                self.apply_settings();
                self.screen = Screen::InGame;
                self.set_mouse_captured(true);
            }
            Ok(Err(err)) => {
                error!("failed to start world {world:?}: {err:#}");
                self.screen = Screen::Title;
            }
            Err(std::sync::mpsc::TryRecvError::Empty) => {}
            Err(std::sync::mpsc::TryRecvError::Disconnected) => {
                error!("world startup thread vanished");
                self.screen = Screen::Title;
            }
        }
    }

    /// Leaves the current world (final save included) for the title screen.
    fn quit_to_title(&mut self) {
        if let Some(mut session) = self.session.take() {
            session.shutdown();
        }
        if let Some(renderer) = &mut self.renderer {
            renderer.clear_chunks();
        }
        self.set_mouse_captured(false);
        self.screen = Screen::Title;
    }

    /// The menu view for the current screen, if it shows one. (Settings
    /// and Loading render themselves; see `frame_with`.)
    fn menu_view(&self, w: f32, h: f32) -> Option<MenuView> {
        let ui = self.ui();
        match &self.screen {
            Screen::Title => self
                .registry
                .menu("oc:title")
                .map(|def| MenuView::from_def(def, &self.registry, w, h, ui, false)),
            Screen::Paused => self.registry.menu("oc:pause").map(|def| {
                let mut view = MenuView::from_def(def, &self.registry, w, h, ui, true);
                // State-dependent labels resolve at render time.
                if let Some(button) =
                    view.buttons.iter_mut().find(|b| b.action == "oc:toggle_cheats")
                {
                    let on = self.session.as_ref().is_some_and(|s| s.cheats);
                    let state = self.registry.text(if on { "menu.on" } else { "menu.off" });
                    button.label = format!("{}: {state}", button.label);
                }
                view
            }),
            Screen::Worlds(worlds) => Some(worlds.view(&self.registry, w, h, ui)),
            Screen::CreateWorld(create) => Some(create.view(&self.registry, w, h, ui)),
            Screen::Modes => {
                let (current, cheats) =
                    self.session.as_ref().map_or((0, false), |s| (s.mode.0, s.cheats));
                Some(menu::modes_view(&self.registry, current, cheats, w, h, ui))
            }
            Screen::InGame | Screen::Settings(_) | Screen::Loading { .. } => None,
        }
    }

    /// Tells the (singleplayer) server to freeze or resume simulation.
    fn send_paused(&mut self, paused: bool) {
        if let Some(session) = &mut self.session {
            session.queue(ClientMessage::SetPaused(paused));
        }
    }

    /// Fires a menu action (from menus.ron or a screen-internal one).
    fn run_action(&mut self, action: &str, event_loop: &ActiveEventLoop) {
        match action {
            "oc:open_worlds" => self.screen = Screen::Worlds(WorldsScreen::new(list_worlds())),
            "oc:quit_app" => {
                if let Some(mut session) = self.session.take() {
                    session.shutdown();
                }
                event_loop.exit();
            }
            "oc:resume" => {
                self.send_paused(false);
                self.screen = Screen::InGame;
                self.set_mouse_captured(true);
            }
            "oc:open_modes" => self.screen = Screen::Modes,
            "oc:open_settings" => {
                let from_pause = matches!(self.screen, Screen::Paused);
                self.screen =
                    Screen::Settings(SettingsScreen::from_settings(&self.settings, from_pause));
            }
            "settings_back" => self.leave_settings(),
            "oc:toggle_cheats" => {
                // The server decides (we're the owner in singleplayer);
                // its Cheats reply updates the label.
                if let Some(session) = &mut self.session {
                    let next = !session.cheats;
                    session.queue(ClientMessage::SetCheats(next));
                }
            }
            "oc:quit_world" => self.quit_to_title(),
            "back" => self.screen = Screen::Title,
            "back_worlds" => self.screen = Screen::Worlds(WorldsScreen::new(list_worlds())),
            "back_pause" => self.screen = Screen::Paused,
            "create_screen" => self.screen = Screen::CreateWorld(CreateScreen::new()),
            "cycle_create_mode" => {
                if let Screen::CreateWorld(create) = &mut self.screen {
                    create.cycle_mode(&self.registry);
                }
            }
            "toggle_create_cheats" => {
                if let Screen::CreateWorld(create) = &mut self.screen {
                    create.cheats = !create.cheats;
                }
            }
            "create" => {
                if let Screen::CreateWorld(create) = &self.screen {
                    let mut name = menu::sanitize_name(&create.name.value);
                    while list_worlds().contains(&name) {
                        name.push('2');
                    }
                    let seed = menu::parse_seed(&create.seed.value, random_seed());
                    let mode = create.mode_id(&self.registry);
                    let cheats = create.cheats;
                    self.start_session(&name, seed, Some(mode), Some(cheats));
                }
            }
            _ if action.starts_with("focus:") => {
                if let Screen::CreateWorld(create) = &mut self.screen {
                    create.focus(&action["focus:".len()..]);
                }
            }
            _ if action.starts_with("mode:") => {
                // Stay on the picker: the [x] marker moves only when the
                // server's GameMode confirmation arrives, so the player
                // sees the change land before going back themselves.
                if let Some(session) = &mut self.session
                    && let Ok(mode) = action["mode:".len()..].parse::<u16>()
                {
                    session.queue(ClientMessage::SetGameMode(mode));
                }
            }
            _ if action.starts_with("world:") => {
                let world = action["world:".len()..].to_owned();
                let (w, _) = self.window_size();
                let ui = self.ui();
                if let Screen::Worlds(worlds) = &mut self.screen {
                    match worlds.world_click(&world, self.mouse_pos.0, w, ui) {
                        WorldAction::Play => self.start_session(&world, random_seed(), None, None),
                        WorldAction::ArmDelete => {}
                        WorldAction::Delete => {
                            let path = PathBuf::from(SAVES_ROOT).join(&world);
                            if let Err(err) = std::fs::remove_dir_all(&path) {
                                error!("deleting {path:?}: {err}");
                            }
                            info!(world, "world deleted");
                            self.screen = Screen::Worlds(WorldsScreen::new(list_worlds()));
                        }
                    }
                }
            }
            _ => {}
        }
    }

    /// A left click on a menu screen: resolve the button under the cursor.
    fn menu_click(&mut self, event_loop: &ActiveEventLoop) {
        let (w, h) = self.window_size();
        let action = self
            .menu_view(w, h)
            .and_then(|view| view.hit(self.mouse_pos).map(str::to_owned));
        if let Some(action) = action {
            self.run_action(&action, event_loop);
        }
    }

    fn handle_key(&mut self, code: KeyCode, pressed: bool, text: Option<&str>) {
        match &mut self.screen {
            Screen::InGame => self.handle_game_key(code, pressed),
            Screen::Paused => {
                if code == KeyCode::Escape && pressed {
                    self.send_paused(false);
                    self.screen = Screen::InGame;
                    self.set_mouse_captured(true);
                }
            }
            Screen::Modes => {
                if code == KeyCode::Escape && pressed {
                    self.screen = Screen::Paused;
                }
            }
            Screen::Settings(_) => {
                if code == KeyCode::Escape && pressed {
                    self.leave_settings();
                }
            }
            Screen::Loading { .. } => {}
            Screen::Worlds(_) => {
                if code == KeyCode::Escape && pressed {
                    self.screen = Screen::Title;
                }
            }
            Screen::CreateWorld(create) => {
                if !pressed {
                    return;
                }
                match code {
                    KeyCode::Escape => {
                        self.screen = Screen::Worlds(WorldsScreen::new(list_worlds()));
                    }
                    KeyCode::Backspace => create.backspace(),
                    _ => {
                        for c in text.unwrap_or("").chars() {
                            create.type_char(c);
                        }
                    }
                }
            }
            Screen::Title => {}
        }
    }

    fn handle_game_key(&mut self, code: KeyCode, pressed: bool) {
        let Some(session) = self.session.as_mut() else {
            return;
        };
        match code {
            KeyCode::KeyW => session.input.forward = pressed,
            KeyCode::KeyS => session.input.backward = pressed,
            KeyCode::KeyA => session.input.left = pressed,
            KeyCode::KeyD => session.input.right = pressed,
            KeyCode::Space => session.input.up = pressed,
            KeyCode::ShiftLeft => session.input.down = pressed,
            KeyCode::ControlLeft => session.input.fast = pressed,
            KeyCode::KeyF if pressed => {
                let caps = session.caps(&self.registry);
                if caps.can_fly && !caps.noclip {
                    session.player.flying = !session.player.flying;
                    info!(flying = session.player.flying, "movement mode toggled");
                }
            }
            KeyCode::F5 if pressed => session.cycle_camera(),
            KeyCode::KeyC if pressed => session.craft_open = !session.craft_open,
            KeyCode::KeyE if pressed => session.eat(&self.registry),
            KeyCode::Digit1 if pressed => session.digit(&self.registry, 0),
            KeyCode::Digit2 if pressed => session.digit(&self.registry, 1),
            KeyCode::Digit3 if pressed => session.digit(&self.registry, 2),
            KeyCode::Digit4 if pressed => session.digit(&self.registry, 3),
            KeyCode::Digit5 if pressed => session.digit(&self.registry, 4),
            KeyCode::Digit6 if pressed => session.digit(&self.registry, 5),
            KeyCode::Digit7 if pressed => session.digit(&self.registry, 6),
            KeyCode::Digit8 if pressed => session.digit(&self.registry, 7),
            KeyCode::Digit9 if pressed => session.digit(&self.registry, 8),
            KeyCode::F3 if pressed => self.hud_visible = !self.hud_visible,
            KeyCode::Escape if pressed => {
                // Stop moving, drop the mouse, show the pause menu, and
                // freeze the singleplayer simulation (a multiplayer
                // server would ignore the request and keep the world on).
                session.input = Default::default();
                session.queue(ClientMessage::SetPaused(true));
                self.screen = Screen::Paused;
                self.set_mouse_captured(false);
            }
            _ => {}
        }
    }

    fn frame(&mut self) -> Result<()> {
        // Take the renderer out so `self` stays borrowable for game logic.
        let Some(mut renderer) = self.renderer.take() else {
            return Ok(());
        };
        let result = self.frame_with(&mut renderer);
        self.renderer = Some(renderer);
        result
    }

    fn frame_with(&mut self, renderer: &mut Renderer) -> Result<()> {
        let frame_start = Instant::now();
        let dt = (frame_start - self.last_frame).as_secs_f64().min(0.1);
        self.last_frame = frame_start;
        self.frame_time_ema = self.frame_time_ema * 0.95 + dt * 0.05;

        self.poll_loading();

        let (w, h) = self.window_size();
        let ui = self.ui();
        let in_game = matches!(self.screen, Screen::InGame);
        // Wraps hourly: keeps f32 wave phase precise on long sessions
        // (every wave speed is periodic well within 3600 s).
        let time = (self.started.elapsed().as_secs_f64() % 3600.0) as f32;

        let mut camera = if let Some(session) = &mut self.session {
            session.update(renderer, &self.registry, dt, in_game, self.settings.far_terrain)?;
            session.frame_camera(
                renderer,
                &self.registry,
                (w, h),
                ui,
                time,
                if self.settings.far_terrain {
                    far_terrain::fog_distance()
                } else {
                    (self.settings.render_distance as f32) * 16.0
                },
                self.settings.clouds,
                self.settings.water_reflections,
                self.settings.far_terrain,
                self.frame_time_ema,
                self.hud_visible && in_game,
                in_game,
            )
        } else {
            // Menu screens without a world: a fixed late-morning sky.
            let sky = sky::sky_at(0.30);
            FrameCamera {
                view_proj: glam::Mat4::IDENTITY,
                position: glam::DVec3::ZERO,
                highlight: None,
                sun: sky.sun,
                sky_color: sky.sky_color,
                sky_zenith: [sky.zenith[0], sky.zenith[1], sky.zenith[2], sky.stars],
                sky_sun: sky.sun_dir,
                sky_away: [
                    sky.horizon_away[0],
                    sky.horizon_away[1],
                    sky.horizon_away[2],
                    sky.moon_phase,
                ],
                sky_angle: sky.angle,
                fog_distance: 1000.0,
                clouds: true,
                shadows: false,
                water_reflections: false,
                far_terrain: false,
                far_cut: [0.0; 4],
                cloud_color: sky.clouds,
                entities: Vec::new(),
                hud: String::new(),
                hud_scale: ui,
                time,
                ui_texts: Vec::new(),
                ui_quads: Vec::new(),
            }
        };

        if let Some(view) = self.menu_view(w, h) {
            camera.ui_quads.extend(view.quads(w, h, self.mouse_pos));
            camera.ui_texts.extend(view.texts(w, h));
        }
        match &self.screen {
            Screen::Settings(screen) => {
                camera
                    .ui_quads
                    .extend(screen.quads(&self.registry, w, h, ui, self.mouse_pos));
                camera.ui_texts.extend(screen.texts(&self.registry, w, h, ui));
            }
            Screen::Loading { world, since, .. } => {
                // "Loading <world>" with marching dots.
                let dots = ".".repeat(1 + (since.elapsed().as_millis() / 350 % 3) as usize);
                let text = format!("{} {world}{dots}", self.registry.text("menu.loading"));
                let width = text.len() as f32 * 6.0 * 2.0 * ui;
                camera.ui_texts.push(oc_renderer::UiText {
                    text,
                    x: (w - width) / 2.0,
                    y: h * 0.45,
                    scale: 2.0 * ui,
                });
            }
            _ => {}
        }

        renderer.draw(&camera)?;
        self.perf.frame(frame_start.elapsed(), renderer);

        // Frame limiter: sleep off the remainder of the frame budget.
        if self.settings.max_fps > 0 {
            let budget = Duration::from_secs_f64(1.0 / self.settings.max_fps as f64);
            let spent = frame_start.elapsed();
            if spent < budget {
                std::thread::sleep(budget - spent);
            }
        }
        Ok(())
    }
}

impl ApplicationHandler for App {
    fn resumed(&mut self, event_loop: &ActiveEventLoop) {
        if self.window.is_none()
            && let Err(err) = self.init(event_loop)
        {
            self.fail(event_loop, err);
        }
    }

    fn window_event(&mut self, event_loop: &ActiveEventLoop, _id: WindowId, event: WindowEvent) {
        match event {
            WindowEvent::CloseRequested => {
                if let Some(mut session) = self.session.take() {
                    session.shutdown(); // the server runs the final save
                }
                event_loop.exit();
            }
            WindowEvent::Resized(size) => {
                if let Some(renderer) = &mut self.renderer {
                    renderer.resize(size.width, size.height);
                }
            }
            WindowEvent::KeyboardInput { event, .. } => {
                if let PhysicalKey::Code(code) = event.physical_key {
                    let pressed = event.state == ElementState::Pressed;
                    self.handle_key(code, pressed, event.text.as_deref());
                }
            }
            WindowEvent::CursorMoved { position, .. } => {
                self.mouse_pos = (position.x as f32, position.y as f32);
                if let Some(index) = self.drag_slider {
                    let (w, h) = self.window_size();
                    let ui = self.ui();
                    let mouse_x = self.mouse_pos.0;
                    if let Screen::Settings(screen) = &mut self.screen {
                        screen.drag(index, mouse_x, w, h, ui);
                    }
                    self.drag_apply();
                }
            }
            WindowEvent::MouseWheel { delta, .. } if self.mouse_captured => {
                if let Some(session) = &mut self.session {
                    let amount = match delta {
                        winit::event::MouseScrollDelta::LineDelta(_, y) => y as f64,
                        winit::event::MouseScrollDelta::PixelDelta(p) => p.y / 40.0,
                    };
                    session.hotbar.scroll(amount);
                }
            }
            WindowEvent::MouseInput { state: ElementState::Pressed, button, .. } => {
                let (w, h) = self.window_size();
                let ui = self.ui();
                match (&mut self.screen, button) {
                    (Screen::InGame, _) if !self.mouse_captured => self.set_mouse_captured(true),
                    (Screen::InGame, MouseButton::Left) => {
                        if let Some(session) = &mut self.session {
                            session.break_clicked = true;
                        }
                    }
                    (Screen::InGame, MouseButton::Right) => {
                        if let Some(session) = &mut self.session {
                            session.place_clicked = true;
                        }
                    }
                    (Screen::Settings(screen), MouseButton::Left) => {
                        if let Some(index) = screen.slider_at(self.mouse_pos, w, h, ui) {
                            screen.drag(index, self.mouse_pos.0, w, h, ui);
                            self.drag_slider = Some(index);
                            self.drag_apply();
                        } else if let Some(action) =
                            screen.button_hit(&self.registry, self.mouse_pos, w, h, ui)
                        {
                            if let Some(tab) = action.strip_prefix("tab:") {
                                if let Ok(tab) = tab.parse() {
                                    screen.tab = tab;
                                }
                            } else {
                                self.leave_settings();
                            }
                        }
                    }
                    (_, MouseButton::Left) => self.menu_click(event_loop),
                    _ => {}
                }
            }
            WindowEvent::MouseInput { state: ElementState::Released, .. } => {
                if self.drag_slider.take().is_some() {
                    // The UI scale applies on release (held back during
                    // the drag); persist the whole set.
                    if let Screen::Settings(screen) = &self.screen {
                        screen.apply(&mut self.settings);
                    }
                    self.settings.save();
                    self.apply_settings();
                }
            }
            WindowEvent::Focused(false) => {
                if matches!(self.screen, Screen::InGame) {
                    self.send_paused(true);
                    self.screen = Screen::Paused;
                }
                self.set_mouse_captured(false);
            }
            WindowEvent::RedrawRequested => {
                if let Err(err) = self.frame() {
                    self.fail(event_loop, err);
                }
            }
            _ => {}
        }
    }

    fn device_event(&mut self, _el: &ActiveEventLoop, _id: DeviceId, event: DeviceEvent) {
        if self.mouse_captured
            && matches!(self.screen, Screen::InGame)
            && let Some(session) = &mut self.session
            && let DeviceEvent::MouseMotion { delta: (dx, dy) } = event
        {
            session.camera.look(dx, dy);
        }
    }

    fn about_to_wait(&mut self, _event_loop: &ActiveEventLoop) {
        if let Some(window) = &self.window {
            window.request_redraw();
        }
    }
}
