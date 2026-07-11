//! `skelly` - the binary: window, event loop, and the wiring that binds the library
//! crates together.
//!
//! M1c completes the walking skeleton: load the config, open a native window, spawn
//! the login shell in a PTY (`skelly-term`), and paint its live grid on the GPU
//! (`skelly-render`), forwarding keystrokes to the shell. The reader thread wakes
//! the event loop via an `EventLoopProxy` so we repaint only on new output. Errors
//! are contextualized here with `anyhow`; `wgpu`/`alacritty` types never leak up.

use std::sync::Arc;

use anyhow::Context as _;
use skelly_config::{Appearance, Config};
use skelly_render::Renderer;
use skelly_term::Terminal;
use winit::application::ApplicationHandler;
use winit::dpi::LogicalSize;
use winit::event::{ElementState, KeyEvent, WindowEvent};
use winit::event_loop::{ActiveEventLoop, ControlFlow, EventLoop, EventLoopProxy};
use winit::keyboard::{Key, ModifiersState, NamedKey};
use winit::window::{Window, WindowId};

/// Event the reader thread sends to wake the UI when the shell produces output.
#[derive(Debug, Clone, Copy)]
struct Wakeup;

fn main() -> anyhow::Result<()> {
    init_tracing();

    let config = Config::load_default().context("loading configuration")?;
    tracing::info!(
        version = env!("CARGO_PKG_VERSION"),
        theme = %config.appearance.theme,
        "starting skelly"
    );

    let event_loop = EventLoop::<Wakeup>::with_user_event()
        .build()
        .context("creating the event loop")?;
    event_loop.set_control_flow(ControlFlow::Wait);
    let proxy = event_loop.create_proxy();
    let mut app = App::new(config, proxy);
    event_loop
        .run_app(&mut app)
        .context("running the event loop")?;
    Ok(())
}

/// Application state driven by the winit event loop. The window, renderer, and
/// terminal are `None` until the platform signals `resumed`.
struct App {
    config: Config,
    proxy: EventLoopProxy<Wakeup>,
    window: Option<Arc<Window>>,
    renderer: Option<Renderer>,
    terminal: Option<Terminal>,
    scale: f64,
    modifiers: ModifiersState,
}

impl App {
    fn new(config: Config, proxy: EventLoopProxy<Wakeup>) -> Self {
        Self {
            config,
            proxy,
            window: None,
            renderer: None,
            terminal: None,
            scale: 1.0,
            modifiers: ModifiersState::empty(),
        }
    }

    /// Repaint from the current terminal snapshot.
    fn redraw(&mut self) {
        let content = self
            .terminal
            .as_ref()
            .map(|t| t.snapshot().join("\n"))
            .unwrap_or_default();
        if let Some(renderer) = self.renderer.as_mut() {
            renderer.set_content(&content);
            if let Err(err) = renderer.render() {
                tracing::error!(%err, "frame render failed");
            }
        }
    }
}

impl ApplicationHandler<Wakeup> for App {
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
        self.scale = window.scale_factor();
        let renderer = Renderer::new(
            window.clone(),
            size.width,
            size.height,
            self.scale,
            &self.config.appearance,
        );

        let (cols, rows) = grid_dims(size.width, size.height, &self.config.appearance, self.scale);
        let proxy = self.proxy.clone();
        let terminal = match Terminal::spawn(cols, rows, move || {
            let _ = proxy.send_event(Wakeup);
        }) {
            Ok(terminal) => terminal,
            Err(err) => {
                tracing::error!(%err, "failed to spawn shell");
                event_loop.exit();
                return;
            }
        };

        self.window = Some(window);
        self.renderer = Some(renderer);
        self.terminal = Some(terminal);
        tracing::info!(cols, rows, "window, GPU, and shell ready");
    }

    fn user_event(&mut self, _event_loop: &ActiveEventLoop, _event: Wakeup) {
        // New shell output arrived; ask the window to repaint.
        if let Some(window) = self.window.as_ref() {
            window.request_redraw();
        }
    }

    fn window_event(&mut self, event_loop: &ActiveEventLoop, _id: WindowId, event: WindowEvent) {
        match event {
            WindowEvent::CloseRequested => event_loop.exit(),
            WindowEvent::ModifiersChanged(modifiers) => self.modifiers = modifiers.state(),
            WindowEvent::KeyboardInput {
                event: key_event, ..
            } => {
                if key_event.state != ElementState::Pressed {
                    return;
                }
                // Quit on the platform combo (Cmd/Super + Q) without stealing it
                // from the shell in any other case - the terminal owns every key.
                if self.modifiers.super_key() {
                    if let Key::Character(ch) = key_event.logical_key.as_ref() {
                        if ch.eq_ignore_ascii_case("q") {
                            event_loop.exit();
                            return;
                        }
                    }
                }
                if let Some(bytes) = key_to_bytes(&key_event, self.modifiers) {
                    if let Some(terminal) = self.terminal.as_mut() {
                        terminal.write(&bytes);
                    }
                }
            }
            WindowEvent::Resized(size) => {
                if let Some(renderer) = self.renderer.as_mut() {
                    renderer.resize(size.width, size.height);
                }
                let (cols, rows) =
                    grid_dims(size.width, size.height, &self.config.appearance, self.scale);
                if let Some(terminal) = self.terminal.as_mut() {
                    terminal.resize(cols, rows);
                }
            }
            WindowEvent::RedrawRequested => self.redraw(),
            _ => {}
        }
    }
}

/// Translate a key press into the bytes a terminal expects, or `None` if it has no
/// terminal representation.
fn key_to_bytes(event: &KeyEvent, modifiers: ModifiersState) -> Option<Vec<u8>> {
    match &event.logical_key {
        Key::Named(NamedKey::Enter) => Some(vec![b'\r']),
        Key::Named(NamedKey::Backspace) => Some(vec![0x7f]),
        Key::Named(NamedKey::Tab) => Some(vec![b'\t']),
        Key::Named(NamedKey::Escape) => Some(vec![0x1b]),
        Key::Named(NamedKey::ArrowUp) => Some(b"\x1b[A".to_vec()),
        Key::Named(NamedKey::ArrowDown) => Some(b"\x1b[B".to_vec()),
        Key::Named(NamedKey::ArrowRight) => Some(b"\x1b[C".to_vec()),
        Key::Named(NamedKey::ArrowLeft) => Some(b"\x1b[D".to_vec()),
        // Ctrl + letter -> the corresponding control byte (Ctrl+A = 0x01, etc.).
        Key::Character(ch) if modifiers.control_key() => {
            let upper = ch.chars().next()?.to_ascii_uppercase();
            if upper.is_ascii_alphabetic() {
                u8::try_from(upper).ok().map(|byte| vec![byte - b'@'])
            } else {
                event.text.as_ref().map(|text| text.as_bytes().to_vec())
            }
        }
        _ => event.text.as_ref().map(|text| text.as_bytes().to_vec()),
    }
}

/// Estimate the terminal grid size (cols, rows) from the window's physical size and
/// the cell metrics. Approximate for now - the exact cell size comes from font
/// shaping in M2.
#[allow(
    clippy::cast_possible_truncation,
    clippy::cast_sign_loss,
    reason = "cols/rows are clamped to a small positive range before the cast"
)]
fn grid_dims(width: u32, height: u32, appearance: &Appearance, scale: f64) -> (u16, u16) {
    let font_px = f64::from(appearance.font_size) * scale;
    let cell_w = (font_px * 0.6).max(1.0);
    let cell_h = (font_px * f64::from(appearance.line_height)).max(1.0);
    let cols = (f64::from(width) / cell_w).floor().clamp(1.0, 1000.0) as u16;
    let rows = (f64::from(height) / cell_h).floor().clamp(1.0, 1000.0) as u16;
    (cols, rows)
}

/// Initialize `tracing` with an env filter (`SKELLY_LOG`, default `info`), writing
/// structured logs to stderr.
fn init_tracing() {
    use tracing_subscriber::{fmt, EnvFilter};
    let filter = EnvFilter::try_from_env("SKELLY_LOG").unwrap_or_else(|_| EnvFilter::new("info"));
    fmt().with_env_filter(filter).with_target(false).init();
}
