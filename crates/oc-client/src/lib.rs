//! The game client: window, input, and the frame loop (ARCHITECTURE.md §2).

use anyhow::Result;
use raw_window_handle::{HasDisplayHandle, HasWindowHandle};
use tracing::{error, info};
use winit::application::ApplicationHandler;
use winit::dpi::LogicalSize;
use winit::event::WindowEvent;
use winit::event_loop::{ActiveEventLoop, ControlFlow, EventLoop};
use winit::window::{Window, WindowAttributes, WindowId};

use oc_renderer::Renderer;

/// Runs the client until the window is closed.
pub fn run() -> Result<()> {
    let event_loop = EventLoop::new()?;
    event_loop.set_control_flow(ControlFlow::Poll);
    let mut app = App::default();
    event_loop.run_app(&mut app)?;
    match app.error.take() {
        Some(err) => Err(err),
        None => Ok(()),
    }
}

#[derive(Default)]
struct App {
    // Field order matters: the renderer (and its surface) must drop before
    // the window it was created from.
    renderer: Option<Renderer>,
    window: Option<Window>,
    error: Option<anyhow::Error>,
}

impl App {
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
        Ok(())
    }

    fn fail(&mut self, event_loop: &ActiveEventLoop, err: anyhow::Error) {
        error!("fatal: {err:#}");
        self.error = Some(err);
        event_loop.exit();
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
            WindowEvent::RedrawRequested => {
                if let Some(renderer) = &mut self.renderer
                    && let Err(err) = renderer.draw()
                {
                    self.fail(event_loop, err);
                }
            }
            _ => {}
        }
    }

    fn about_to_wait(&mut self, _event_loop: &ActiveEventLoop) {
        if let Some(window) = &self.window {
            window.request_redraw();
        }
    }
}
