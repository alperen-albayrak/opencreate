//! The game client: window, input, and the frame loop (ARCHITECTURE.md §2).

mod camera;
mod craft_menu;
mod hotbar;
mod player;
mod sky;
mod streaming;

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

use camera::Camera;
use hotbar::Hotbar;
use oc_protocol::{ClientMessage, InProcEnd, ServerMessage, Transport, in_proc_channel};
use oc_renderer::{FrameCamera, Renderer};
use oc_server::{ServerConfig, ServerHandle};
use oc_assets::Registry;
use oc_world::raycast::{RayHit, raycast};
use oc_world::BlockId;
use player::{MoveInput, Player};
use streaming::ChunkStreamer;

/// Fixed world seed until there is a world-selection UI.
const WORLD_SEED: u64 = 20260611;
/// Save location, relative to the working directory (proper platform dirs
/// come with the launcher/UI work).
const SAVE_DIR: &str = "saves/world";
/// How far the player can reach to break/place blocks.
const REACH: f64 = 6.0;

type ClientEnd = InProcEnd<ClientMessage, ServerMessage>;

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

struct App {
    // Field order matters: the renderer (and its surface) must drop before
    // the window it was created from.
    renderer: Option<Renderer>,
    window: Option<Window>,
    error: Option<anyhow::Error>,
    streamer: ChunkStreamer,
    camera: Camera,
    player: Player,
    input: MoveInput,
    hotbar: Hotbar,
    /// Click edges captured by the event loop, consumed by the next frame.
    break_clicked: bool,
    place_clicked: bool,
    mouse_captured: bool,
    last_frame: Instant,
    /// Time of day in [0, 1); locally advanced, corrected by server Time.
    day_fraction: f64,
    perf: PerfLog,
    /// Connection to the (embedded) server; None after shutdown.
    transport: Option<ClientEnd>,
    server: Option<ServerHandle>,
    /// Messages queued for the server this frame.
    outbox: Vec<ClientMessage>,
    hud_visible: bool,
    craft_open: bool,
    /// Exponentially smoothed frame time, for the HUD readout.
    frame_time_ema: f64,
    /// Server-authoritative survival stats (health, hunger, stamina, oxygen).
    stats: [f32; 4],
    registry: Registry,
    /// Server-authoritative item counts, keyed by per-load item id.
    inventory: std::collections::HashMap<u16, u32>,
}

/// Aggregates frame times and logs a summary periodically (§11 budgets,
/// until the in-game HUD exists).
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

impl App {
    fn new() -> Result<Self> {
        // Offline play is still client-server (§1): an embedded server on
        // its own thread, connected by the in-proc transport.
        let (mut transport, server_end) = in_proc_channel();
        let server = oc_server::start(
            ServerConfig { seed: WORLD_SEED, save_dir: SAVE_DIR.into() },
            server_end,
        )?;

        // The Welcome carries the seed, spawn and time of day.
        let deadline = Instant::now() + Duration::from_secs(5);
        let (seed, spawn, day_fraction) = loop {
            match transport
                .try_recv()
                .map_err(|_| anyhow::anyhow!("server disconnected during startup"))?
            {
                Some(ServerMessage::Welcome { seed, spawn, day_fraction }) => {
                    break (seed, spawn, day_fraction);
                }
                Some(_) => {}
                None if Instant::now() > deadline => {
                    anyhow::bail!("timed out waiting for the server welcome")
                }
                None => std::thread::sleep(Duration::from_millis(1)),
            }
        };
        info!(seed, "connected to embedded server");

        let player = Player::new(spawn);
        Ok(Self {
            renderer: None,
            window: None,
            error: None,
            streamer: ChunkStreamer::new(seed),
            camera: Camera::new(player.eye()),
            player,
            input: MoveInput::default(),
            hotbar: Hotbar::new(),
            break_clicked: false,
            place_clicked: false,
            mouse_captured: false,
            last_frame: Instant::now(),
            day_fraction,
            perf: PerfLog::new(),
            transport: Some(transport),
            server: Some(server),
            outbox: Vec::new(),
            hud_visible: true,
            craft_open: false,
            frame_time_ema: 1.0 / 60.0,
            stats: [10.0; 4],
            registry: Registry::load_default()?,
            inventory: std::collections::HashMap::new(),
        })
    }

    /// Disconnects from the server and waits for its final save.
    fn shutdown(&mut self) {
        drop(self.transport.take());
        if let Some(server) = self.server.take() {
            server.join();
        }
    }

    fn hud_text(&self, renderer: &Renderer) -> String {
        if !self.hud_visible {
            return String::new();
        }
        let stats = renderer.stats();
        let p = self.player.position;
        format!(
            "fps {:>3.0}  {:>5.2} ms\nchunks {} / {}\npos {:.1} / {:.1} / {:.1}\nday {:.2}  {}  holding {}\n[f3] hud  [f] {}",
            (1.0 / self.frame_time_ema).round(),
            self.frame_time_ema * 1e3,
            stats.chunks_drawn,
            stats.chunks_resident,
            p.x,
            p.y,
            p.z,
            self.day_fraction,
            if self.player.flying { "flying" } else { "walking" },
            hotbar::block_name(self.hotbar.block()),
            if self.player.flying { "walk" } else { "fly" },
        )
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
        info!("renderer initialized — click to capture the mouse, Esc to release");
        self.window = Some(window);
        self.renderer = Some(renderer);
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

    fn handle_key(&mut self, code: KeyCode, pressed: bool) {
        match code {
            KeyCode::KeyW => self.input.forward = pressed,
            KeyCode::KeyS => self.input.backward = pressed,
            KeyCode::KeyA => self.input.left = pressed,
            KeyCode::KeyD => self.input.right = pressed,
            KeyCode::Space => self.input.up = pressed,
            KeyCode::ShiftLeft => self.input.down = pressed,
            KeyCode::ControlLeft => self.input.fast = pressed,
            KeyCode::KeyF if pressed => {
                self.player.flying = !self.player.flying;
                info!(flying = self.player.flying, "movement mode toggled");
            }
            KeyCode::KeyC if pressed => self.craft_open = !self.craft_open,
            KeyCode::Digit1 if pressed => self.digit(0),
            KeyCode::Digit2 if pressed => self.digit(1),
            KeyCode::Digit3 if pressed => self.digit(2),
            KeyCode::Digit4 if pressed => self.digit(3),
            KeyCode::Digit5 if pressed => self.digit(4),
            KeyCode::Digit6 if pressed => self.digit(5),
            KeyCode::Digit7 if pressed => self.digit(6),
            KeyCode::Digit8 if pressed => self.digit(7),
            KeyCode::Digit9 if pressed => self.digit(8),
            KeyCode::F3 if pressed => self.hud_visible = !self.hud_visible,
            KeyCode::Escape if pressed => self.set_mouse_captured(false),
            _ => {}
        }
    }

    /// How many of a block's item the player carries.
    fn count_of(&self, block: BlockId) -> u32 {
        self.registry
            .item_for_block(block)
            .and_then(|item| self.inventory.get(&item.0).copied())
            .unwrap_or(0)
    }

    fn hotbar_counts(&self) -> [u32; hotbar::ITEMS.len()] {
        std::array::from_fn(|i| self.count_of(hotbar::ITEMS[i]))
    }

    /// Number keys: hotbar slots normally, recipes while the book is open.
    fn digit(&mut self, n: usize) {
        if self.craft_open {
            if self.registry.craftable(n, |item| {
                self.inventory.get(&item.0).copied().unwrap_or(0)
            }) {
                self.outbox.push(ClientMessage::Craft { recipe: n as u32 });
            }
        } else {
            self.hotbar.select(n);
        }
    }

    fn target(&self) -> Option<RayHit> {
        raycast(
            self.streamer.world(),
            self.camera.position,
            self.camera.forward().as_dvec3(),
            REACH,
        )
    }

    /// Applies an edit locally (prediction) and tells the server. The
    /// server's BlockChanged echo is a no-op when it matches.
    fn apply_block_edits(&mut self, renderer: &mut Renderer) -> Result<()> {
        if std::mem::take(&mut self.break_clicked)
            && let Some(hit) = self.target()
        {
            let broken = self.streamer.world().block(hit.block);
            if self.streamer.world_mut().set_block(hit.block, BlockId::AIR) {
                self.streamer.remesh_after_edit(renderer, hit.block)?;
                self.outbox
                    .push(ClientMessage::SetBlock { pos: hit.block, block: BlockId::AIR });
                // Predict the pickup; the server's Inventory message confirms.
                if let Some(item) = self.registry.item_for_block(broken) {
                    *self.inventory.entry(item.0).or_insert(0) += 1;
                }
            }
        }
        if std::mem::take(&mut self.place_clicked)
            && let Some(hit) = self.target()
            // normal == 0 means the camera is inside the block: nowhere to place.
            && hit.normal != glam::IVec3::ZERO
        {
            let pos = hit.block + hit.normal;
            // Water is replaceable, like Minecraft.
            let free = !self.streamer.world().block(pos).is_solid()
                && !self.player.aabb().intersects_block(pos)
                && self.count_of(self.hotbar.block()) > 0;
            if free && self.streamer.world_mut().set_block(pos, self.hotbar.block()) {
                self.streamer.remesh_after_edit(renderer, pos)?;
                self.outbox
                    .push(ClientMessage::SetBlock { pos, block: self.hotbar.block() });
                if let Some(item) = self.registry.item_for_block(self.hotbar.block()) {
                    self.inventory
                        .entry(item.0)
                        .and_modify(|n| *n = n.saturating_sub(1));
                }
            }
        }
        Ok(())
    }

    /// Integrates everything the server sent since last frame.
    fn drain_server_messages(&mut self, renderer: &mut Renderer) -> Result<()> {
        let Some(transport) = &mut self.transport else {
            return Ok(());
        };
        loop {
            let msg = transport
                .try_recv()
                .map_err(|_| anyhow::anyhow!("server disconnected"))?;
            match msg {
                Some(ServerMessage::Column(column)) => self.streamer.insert_column(column),
                Some(ServerMessage::BlockChanged { pos, block }) => {
                    self.streamer.apply_block_change(renderer, pos, block)?;
                }
                Some(ServerMessage::Time { day_fraction }) => self.day_fraction = day_fraction,
                Some(ServerMessage::Stats { health, hunger, stamina, oxygen }) => {
                    self.stats = [health, hunger, stamina, oxygen];
                }
                Some(ServerMessage::Inventory { counts }) => {
                    self.inventory = counts.into_iter().collect();
                }
                Some(ServerMessage::Respawn { position }) => {
                    info!("you died; respawning");
                    self.player.position = position;
                    self.player.velocity = glam::DVec3::ZERO;
                    self.stats = [10.0; 4];
                }
                Some(ServerMessage::Welcome { .. }) => {} // already consumed at startup
                None => return Ok(()),
            }
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
        self.day_fraction = (self.day_fraction + dt / sky::DAY_LENGTH_SECS).fract();

        self.drain_server_messages(renderer)?;

        // Out of stamina: no sprinting (the server drains/regens it).
        let mut input = MoveInput { ..self.input };
        if self.stats[2] <= 0.05 && !self.player.flying {
            input.fast = false;
        }
        let moving = input.forward || input.backward || input.left || input.right;
        let sprinting = input.fast && moving && !self.player.flying;

        // Hold physics until the column under the player has terrain, so
        // nobody falls through a world that hasn't streamed in yet.
        let feet_chunk =
            oc_core::coords::block_to_chunk(self.player.position.floor().as_ivec3());
        if self.streamer.world().is_generated(feet_chunk) {
            self.player
                .update(self.streamer.world(), &input, self.camera.yaw, dt);
        }
        self.camera.position = self.player.eye();

        self.apply_block_edits(renderer)?;
        self.streamer
            .update(renderer, self.camera.position, &mut self.outbox)?;

        // Flush this frame's messages, plus the player state the server
        // persists (and will reconcile in phase 4).
        self.outbox.push(ClientMessage::PlayerState {
            position: self.player.position,
            yaw: self.camera.yaw,
            pitch: self.camera.pitch,
            sprinting,
            flying: self.player.flying,
        });
        if let Some(transport) = &mut self.transport {
            for msg in self.outbox.drain(..) {
                transport
                    .send(msg)
                    .map_err(|_| anyhow::anyhow!("server disconnected"))?;
            }
        }

        let Some(window) = &self.window else {
            return Ok(());
        };
        let size = window.inner_size();
        let aspect = size.width.max(1) as f32 / size.height.max(1) as f32;
        let sky = sky::sky_at(self.day_fraction);
        renderer.draw(&FrameCamera {
            view_proj: self.camera.view_proj(aspect),
            position: self.camera.position,
            highlight: self.target().map(|hit| hit.block),
            sun: sky.sun,
            sky_color: sky.sky_color,
            hud: self.hud_text(renderer),
            ui_texts: {
                let (w, h) = (size.width.max(1) as f32, size.height.max(1) as f32);
                let mut texts = self.hotbar.count_labels(w, h, &self.hotbar_counts());
                if self.craft_open {
                    let lines = craft_menu::lines(&self.registry, |item| {
                        self.inventory.get(&item.0).copied().unwrap_or(0)
                    });
                    texts.extend(craft_menu::panel(&lines, w).1);
                }
                texts
            },
            ui_quads: {
                let (w, h) = (size.width.max(1) as f32, size.height.max(1) as f32);
                let counts = self.hotbar_counts();
                let mut quads = self.hotbar.quads(w, h, &counts);
                if self.craft_open {
                    let lines = craft_menu::lines(&self.registry, |item| {
                        self.inventory.get(&item.0).copied().unwrap_or(0)
                    });
                    quads.extend(craft_menu::panel(&lines, w).0);
                }
                quads.extend(hotbar::stat_bars(
                    w, h, self.stats[0], self.stats[1], self.stats[2], self.stats[3],
                ));
                // Crosshair: a small plus at screen center.
                let cross = [0.95, 0.95, 0.95, 0.8];
                quads.push(oc_renderer::UiQuad {
                    x: w / 2.0 - 12.0, y: h / 2.0 - 2.0, w: 24.0, h: 4.0, color: cross,
                });
                quads.push(oc_renderer::UiQuad {
                    x: w / 2.0 - 2.0, y: h / 2.0 - 12.0, w: 4.0, h: 24.0, color: cross,
                });
                quads
            },
        })?;
        self.perf.frame(frame_start.elapsed(), renderer);
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
                self.shutdown(); // the server runs the final save
                event_loop.exit();
            }
            WindowEvent::Resized(size) => {
                if let Some(renderer) = &mut self.renderer {
                    renderer.resize(size.width, size.height);
                }
            }
            WindowEvent::KeyboardInput { event, .. } => {
                if let PhysicalKey::Code(code) = event.physical_key {
                    self.handle_key(code, event.state == ElementState::Pressed);
                }
            }
            WindowEvent::MouseWheel { delta, .. } if self.mouse_captured => {
                let amount = match delta {
                    winit::event::MouseScrollDelta::LineDelta(_, y) => y as f64,
                    winit::event::MouseScrollDelta::PixelDelta(p) => p.y / 40.0,
                };
                self.hotbar.scroll(amount);
            }
            WindowEvent::MouseInput {
                state: ElementState::Pressed,
                button,
                ..
            } => {
                if !self.mouse_captured {
                    self.set_mouse_captured(true);
                } else {
                    match button {
                        MouseButton::Left => self.break_clicked = true,
                        MouseButton::Right => self.place_clicked = true,
                        _ => {}
                    }
                }
            }
            WindowEvent::Focused(false) => self.set_mouse_captured(false),
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
            && let DeviceEvent::MouseMotion { delta: (dx, dy) } = event
        {
            self.camera.look(dx, dy);
        }
    }

    fn about_to_wait(&mut self, _event_loop: &ActiveEventLoop) {
        if let Some(window) = &self.window {
            window.request_redraw();
        }
    }
}
