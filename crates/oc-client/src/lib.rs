//! The game client: window, input, and the frame loop (ARCHITECTURE.md §2).

mod camera;
mod player;
mod sky;
mod streaming;

use std::time::{Duration, Instant};

use anyhow::Result;
use glam::DVec3;
use raw_window_handle::{HasDisplayHandle, HasWindowHandle};
use tracing::{error, info};
use winit::application::ApplicationHandler;
use winit::dpi::LogicalSize;
use winit::event::{DeviceEvent, DeviceId, ElementState, MouseButton, WindowEvent};
use winit::event_loop::{ActiveEventLoop, ControlFlow, EventLoop};
use winit::keyboard::{KeyCode, PhysicalKey};
use winit::window::{CursorGrabMode, Window, WindowAttributes, WindowId};

use camera::Camera;
use oc_renderer::{FrameCamera, Renderer};
use oc_world::raycast::{RayHit, raycast};
use oc_world::{BlockId, blocks};
use player::{MoveInput, Player};
use streaming::ChunkStreamer;

/// Fixed world seed until there is a world-selection UI.
const WORLD_SEED: u64 = 20260611;
/// How far the player can reach to break/place blocks.
const REACH: f64 = 6.0;

/// Nearest dry land to the origin (outward ring search over the pure
/// heightmap), standing just above the surface.
fn find_spawn(world: &oc_world::World) -> DVec3 {
    const STEP: i32 = 8;
    for radius in 0..256 {
        let r = radius * STEP;
        for z in (-r..=r).step_by(STEP as usize) {
            for x in (-r..=r).step_by(STEP as usize) {
                // Ring only: skip the interior already searched.
                if x.abs() != r && z.abs() != r {
                    continue;
                }
                let h = world.surface_height(x, z);
                if h > 1 {
                    return DVec3::new(x as f64 + 0.5, h as f64 + 2.0, z as f64 + 0.5);
                }
            }
        }
    }
    // No land within 2048 blocks: float above the ocean at the origin.
    DVec3::new(0.5, 24.0, 0.5)
}

/// Runs the client until the window is closed.
pub fn run() -> Result<()> {
    let event_loop = EventLoop::new()?;
    event_loop.set_control_flow(ControlFlow::Poll);
    let mut app = App::new();
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
    /// Block placed on right click; selected with the 1/2/3 keys.
    selected_block: BlockId,
    /// Click edges captured by the event loop, consumed by the next frame.
    break_clicked: bool,
    place_clicked: bool,
    mouse_captured: bool,
    last_frame: Instant,
    /// Time of day in [0, 1); see `sky::sky_at` for the phase convention.
    day_fraction: f64,
    perf: PerfLog,
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
    fn new() -> Self {
        let streamer = ChunkStreamer::new(WORLD_SEED);
        let player = Player::new(find_spawn(streamer.world()));
        Self {
            renderer: None,
            window: None,
            error: None,
            streamer,
            camera: Camera::new(player.eye()),
            player,
            input: MoveInput::default(),
            selected_block: blocks::STONE,
            break_clicked: false,
            place_clicked: false,
            mouse_captured: false,
            last_frame: Instant::now(),
            // Start mid-morning so the first impression is a lit world.
            day_fraction: 0.15,
            perf: PerfLog::new(),
        }
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
            KeyCode::Digit1 if pressed => self.selected_block = blocks::STONE,
            KeyCode::Digit2 if pressed => self.selected_block = blocks::DIRT,
            KeyCode::Digit3 if pressed => self.selected_block = blocks::GRASS,
            KeyCode::Escape if pressed => self.set_mouse_captured(false),
            _ => {}
        }
    }

    /// The solid block the camera looks at, within reach.
    fn target(&self) -> Option<RayHit> {
        raycast(
            self.streamer.world(),
            self.camera.position,
            self.camera.forward().as_dvec3(),
            REACH,
        )
    }

    fn apply_block_edits(&mut self, renderer: &mut Renderer) -> Result<()> {
        if std::mem::take(&mut self.break_clicked)
            && let Some(hit) = self.target()
            && self.streamer.world_mut().set_block(hit.block, BlockId::AIR)
        {
            self.streamer.remesh_after_edit(renderer, hit.block)?;
        }
        if std::mem::take(&mut self.place_clicked)
            && let Some(hit) = self.target()
            // normal == 0 means the camera is inside the block: nowhere to place.
            && hit.normal != glam::IVec3::ZERO
        {
            let pos = hit.block + hit.normal;
            // Water is replaceable, like Minecraft.
            let free = !self.streamer.world().block(pos).is_solid()
                && !self.player.aabb().intersects_block(pos);
            if free && self.streamer.world_mut().set_block(pos, self.selected_block) {
                self.streamer.remesh_after_edit(renderer, pos)?;
            }
        }
        Ok(())
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
        self.day_fraction = (self.day_fraction + dt / sky::DAY_LENGTH_SECS).fract();

        self.player
            .update(self.streamer.world(), &self.input, self.camera.yaw, dt);
        self.camera.position = self.player.eye();

        self.apply_block_edits(renderer)?;
        self.streamer.update(renderer, self.camera.position)?;

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
            WindowEvent::CloseRequested => event_loop.exit(),
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
