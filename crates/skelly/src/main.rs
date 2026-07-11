//! `skelly` - the binary: window, event loop, and the wiring that binds the library
//! crates together.
//!
//! M1a (walking skeleton, first slice): load the config, open a native window, and
//! render a GPU-cleared surface in the resolved theme background, quitting cleanly
//! on window-close, Escape, or `q`. The PTY, terminal grid, and text rendering
//! arrive with the next slices. Errors are contextualized at this boundary with
//! `anyhow`; `wgpu` types never leak up here (the renderer owns them).

use std::sync::Arc;

use anyhow::Context as _;
use skelly_config::Config;
use skelly_render::Renderer;
use winit::application::ApplicationHandler;
use winit::dpi::LogicalSize;
use winit::event::{ElementState, KeyEvent, WindowEvent};
use winit::event_loop::{ActiveEventLoop, ControlFlow, EventLoop};
use winit::keyboard::{Key, NamedKey};
use winit::window::{Window, WindowId};

fn main() -> anyhow::Result<()> {
    init_tracing();

    let config = Config::load_default().context("loading configuration")?;
    tracing::info!(
        version = env!("CARGO_PKG_VERSION"),
        theme = %config.appearance.theme,
        "starting skelly"
    );

    let event_loop = EventLoop::new().context("creating the event loop")?;
    event_loop.set_control_flow(ControlFlow::Wait);
    let mut app = App::new(config);
    event_loop
        .run_app(&mut app)
        .context("running the event loop")?;
    Ok(())
}

/// Application state driven by the winit event loop. The window and renderer are
/// `None` until the platform signals `resumed`.
struct App {
    config: Config,
    window: Option<Arc<Window>>,
    renderer: Option<Renderer>,
}

impl App {
    fn new(config: Config) -> Self {
        Self {
            config,
            window: None,
            renderer: None,
        }
    }
}

impl ApplicationHandler for App {
    fn resumed(&mut self, event_loop: &ActiveEventLoop) {
        if self.window.is_some() {
            return;
        }
        let attributes = Window::default_attributes()
            .with_title("skelly")
            .with_inner_size(LogicalSize::new(960.0, 600.0));
        let window = match event_loop.create_window(attributes) {
            Ok(window) => Arc::new(window),
            Err(err) => {
                tracing::error!(%err, "failed to create window");
                event_loop.exit();
                return;
            }
        };

        let size = window.inner_size();
        let renderer = Renderer::new(
            window.clone(),
            size.width,
            size.height,
            &self.config.appearance.theme,
        );

        self.window = Some(window);
        self.renderer = Some(renderer);
        tracing::info!("window and GPU surface ready");
        if let Some(window) = self.window.as_ref() {
            window.request_redraw();
        }
    }

    fn window_event(&mut self, event_loop: &ActiveEventLoop, _id: WindowId, event: WindowEvent) {
        match event {
            WindowEvent::CloseRequested => {
                tracing::info!("close requested");
                event_loop.exit();
            }
            WindowEvent::KeyboardInput {
                event:
                    KeyEvent {
                        state: ElementState::Pressed,
                        logical_key,
                        ..
                    },
                ..
            } => {
                let quit = matches!(logical_key, Key::Named(NamedKey::Escape))
                    || matches!(logical_key.as_ref(), Key::Character("q"));
                if quit {
                    event_loop.exit();
                }
            }
            WindowEvent::Resized(size) => {
                if let Some(renderer) = self.renderer.as_mut() {
                    renderer.resize(size.width, size.height);
                }
            }
            WindowEvent::RedrawRequested => {
                if let Some(renderer) = self.renderer.as_mut() {
                    if let Err(err) = renderer.render() {
                        tracing::error!(%err, "frame render failed");
                    }
                }
            }
            _ => {}
        }
    }
}

/// Initialize `tracing` with an env filter (`SKELLY_LOG`, default `info`), writing
/// structured logs to stderr.
fn init_tracing() {
    use tracing_subscriber::{fmt, EnvFilter};
    let filter = EnvFilter::try_from_env("SKELLY_LOG").unwrap_or_else(|_| EnvFilter::new("info"));
    fmt().with_env_filter(filter).with_target(false).init();
}
