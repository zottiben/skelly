//! The GPU renderer.
//!
//! Owns the `wgpu` device, queue, and surface, and paints each frame in two passes:
//! a background/cursor quad pass (clears + fills cells) then a text pass on top.
//! Kept decoupled from the windowing crate - it accepts anything with a raw window
//! handle, so `skelly` never leaks `winit` types into here and the backend stays
//! swappable (ADR-0003).

use std::sync::Arc;

use raw_window_handle::{HasDisplayHandle, HasWindowHandle};
use skelly_config::Appearance;

use crate::cells::{grid_quads, push_outline, Quad, QuadLayer};
use crate::error::RenderError;
use crate::text::{PaneTextInput, TextLayer};
use crate::theme::Theme;
use crate::{GitDockView, OverlayView, PaneView, SettingsView, SidebarView};

/// One chrome layer drawn over the terminal with `LoadOp::Load`: a quad pass then a text
/// pass, gated by whether it is currently shown. The sidebar, git dock, command palette,
/// and settings view are each one of these; only their quad-building and geometry differ,
/// which stays in the per-surface `set_*` methods.
struct ChromeLayer {
    quads: QuadLayer,
    text: TextLayer,
    active: bool,
}

impl ChromeLayer {
    fn new(
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        format: wgpu::TextureFormat,
        width: u32,
        height: u32,
        scale: f64,
        appearance: &Appearance,
    ) -> Self {
        Self {
            quads: QuadLayer::new(device, format),
            text: TextLayer::new(device, queue, format, width, height, scale, appearance),
            active: false,
        }
    }

    /// Re-layout the text for a new surface size.
    fn resize(&mut self, width: u32, height: u32) {
        self.text.resize(width, height);
    }

    /// Update the fallback glyph color after a theme switch.
    fn set_default_fg(&mut self, fg: crate::theme::Srgb) {
        self.text.set_default_fg(fg);
    }

    /// Upload this layer's decorative `quads` and its `text` (rows + position + clip), and
    /// mark it active for the next frame.
    fn set(
        &mut self,
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        surface: (u32, u32),
        quads: &[Quad],
        text: PaneTextInput,
    ) {
        self.active = true;
        self.quads.set(device, queue, surface.0, surface.1, quads);
        self.text.set_panes(&[text]);
    }

    /// Hide the layer (nothing drawn next frame).
    fn clear(&mut self) {
        self.active = false;
    }

    /// Draw the layer over `view` (quads then text, both loading), when active.
    fn draw(
        &mut self,
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        view: &wgpu::TextureView,
        width: u32,
        height: u32,
    ) -> Result<(), RenderError> {
        if !self.active {
            return Ok(());
        }
        self.quads.draw(device, queue, view, None);
        self.text.draw(device, queue, view, width, height)?;
        Ok(())
    }
}

/// Owns the GPU device and surface and presents painted frames.
pub struct Renderer {
    surface: wgpu::Surface<'static>,
    device: wgpu::Device,
    queue: wgpu::Queue,
    config: wgpu::SurfaceConfiguration,
    theme: Theme,
    /// The terminal base layer: this quad pass clears the surface, then the text draws.
    quads: QuadLayer,
    text: TextLayer,
    /// The persistent left sidebar, drawn as base chrome when shown.
    sidebar: ChromeLayer,
    /// The per-repo git diff dock, drawn as base chrome on the right when open.
    gitdock: ChromeLayer,
    /// The command-palette / overlay layer, drawn over the live terminal when active.
    overlay: ChromeLayer,
    /// The full-window settings view, drawn over everything when open.
    settings: ChromeLayer,
}

impl Renderer {
    /// Create a renderer bound to `window`, sized `width` x `height` physical px at
    /// `scale_factor`, using `appearance` for the theme and cell font. Blocks on
    /// GPU init.
    ///
    /// # Panics
    /// Panics if no suitable GPU adapter or device is available, or the surface
    /// cannot be created - all unrecoverable at startup.
    #[must_use]
    pub fn new<W>(
        window: Arc<W>,
        width: u32,
        height: u32,
        scale_factor: f64,
        appearance: &Appearance,
    ) -> Self
    where
        W: HasWindowHandle + HasDisplayHandle + Send + Sync + 'static,
    {
        pollster::block_on(Self::new_async(
            window,
            width,
            height,
            scale_factor,
            appearance,
        ))
    }

    async fn new_async<W>(
        window: Arc<W>,
        width: u32,
        height: u32,
        scale_factor: f64,
        appearance: &Appearance,
    ) -> Self
    where
        W: HasWindowHandle + HasDisplayHandle + Send + Sync + 'static,
    {
        let instance = wgpu::Instance::default();
        let surface = instance.create_surface(window).expect("create GPU surface");

        let adapter = instance
            .request_adapter(&wgpu::RequestAdapterOptions {
                power_preference: wgpu::PowerPreference::default(),
                force_fallback_adapter: false,
                compatible_surface: Some(&surface),
                ..Default::default()
            })
            .await
            .expect("no suitable GPU adapter");

        let (device, queue) = adapter
            .request_device(&wgpu::DeviceDescriptor {
                label: Some("skelly-device"),
                required_features: wgpu::Features::empty(),
                required_limits: wgpu::Limits::default(),
                ..Default::default()
            })
            .await
            .expect("failed to create GPU device");

        let mut config = surface
            .get_default_config(&adapter, width.max(1), height.max(1))
            .expect("surface not supported by the selected adapter");
        // Prefer an sRGB swapchain format so the linear clear color displays correctly.
        let caps = surface.get_capabilities(&adapter);
        if let Some(srgb) = caps
            .formats
            .iter()
            .copied()
            .find(wgpu::TextureFormat::is_srgb)
        {
            config.format = srgb;
        }
        surface.configure(&device, &config);

        let quads = QuadLayer::new(&device, config.format);
        let text = TextLayer::new(
            &device,
            &queue,
            config.format,
            config.width,
            config.height,
            scale_factor,
            appearance,
        );
        let chrome = || {
            ChromeLayer::new(
                &device,
                &queue,
                config.format,
                config.width,
                config.height,
                scale_factor,
                appearance,
            )
        };
        // Bind the four chrome layers before the struct literal so the closure's borrows
        // of `device`/`queue`/`config` end (NLL) before those move into `Self`.
        let (sidebar, gitdock, overlay, settings) = (chrome(), chrome(), chrome(), chrome());

        Self {
            theme: Theme::resolve(&appearance.theme),
            quads,
            text,
            sidebar,
            gitdock,
            overlay,
            settings,
            surface,
            device,
            queue,
            config,
        }
    }

    /// Reconfigure the surface and re-layout text for a new size. No-ops on a zero
    /// dimension (minimized window).
    pub fn resize(&mut self, width: u32, height: u32) {
        if width == 0 || height == 0 {
            return;
        }
        self.config.width = width;
        self.config.height = height;
        self.surface.configure(&self.device, &self.config);
        self.text.resize(width, height);
        self.sidebar.resize(width, height);
        self.gitdock.resize(width, height);
        self.overlay.resize(width, height);
        self.settings.resize(width, height);
    }

    /// Cell metrics in physical px: `(width, height, top-left padding)`. Callers use
    /// these to map pixel coordinates (e.g. the mouse) to grid cells.
    #[must_use]
    pub fn cell_metrics(&self) -> (f32, f32, f32) {
        self.text.cell_metrics()
    }

    /// Switch the active UI theme. Re-resolves the semantic tokens and updates the
    /// text layers' fallback color; the next frame repaints every surface in the new
    /// theme (the clear color and all quads read the theme per frame). AGENTS Hard
    /// rule 2: switching theme repaints everything live.
    pub fn set_theme(&mut self, name: &str) {
        self.theme = Theme::resolve(name);
        self.text.set_default_fg(self.theme.fg_primary);
        self.sidebar.set_default_fg(self.theme.fg_primary);
        self.gitdock.set_default_fg(self.theme.fg_primary);
        self.overlay.set_default_fg(self.theme.fg_primary);
        self.settings.set_default_fg(self.theme.fg_primary);
    }

    /// Set the panes to display next frame. Each pane's grid is filled at its
    /// `origin` and clipped to its `rect`; with more than one pane a `border` divider
    /// is drawn around each and a `border.strong` ring around the focused one. Only
    /// the focused pane draws a cursor.
    pub fn set_panes(&mut self, panes: &[PaneView]) {
        let (cell_w, cell_h, _) = self.text.cell_metrics();
        let scale = self.text.scale();
        let mut quads = Vec::new();
        let mut text_inputs = Vec::with_capacity(panes.len());
        for pane in panes {
            let cursor = pane.focused.then_some(pane.cursor);
            quads.extend(grid_quads(
                cell_w,
                cell_h,
                pane.origin,
                pane.rows,
                cursor,
                self.theme.accent,
                pane.selection,
            ));
            text_inputs.push(PaneTextInput {
                rows: pane.rows,
                left: pane.origin.0,
                top: pane.origin.1,
                clip: (pane.rect.x, pane.rect.y, pane.rect.w, pane.rect.h),
            });
        }
        // A lone pane stays borderless (just the window margin), like a single shell.
        // With splits, draw the dividers, then the focused ring on top.
        if panes.len() > 1 {
            let divider = self.theme.border.to_linear();
            for pane in panes {
                push_outline(
                    &mut quads,
                    pane.rect.x,
                    pane.rect.y,
                    pane.rect.w,
                    pane.rect.h,
                    scale,
                    divider,
                );
            }
            if let Some(pane) = panes.iter().find(|p| p.focused) {
                push_outline(
                    &mut quads,
                    pane.rect.x,
                    pane.rect.y,
                    pane.rect.w,
                    pane.rect.h,
                    2.0 * scale,
                    self.theme.border_strong.to_linear(),
                );
            }
        }
        self.quads.set(
            &self.device,
            &self.queue,
            self.config.width,
            self.config.height,
            &quads,
        );
        self.text.set_panes(&text_inputs);
    }

    /// Set the persistent left sidebar to draw next frame, or clear it with `None`
    /// (hidden). Builds the active-tab highlight + right-edge divider quads; the tab
    /// labels come baked into `sidebar.rows`. Drawn as base chrome beneath any overlay.
    pub fn set_sidebar(&mut self, sidebar: Option<&SidebarView>) {
        let Some(view) = sidebar else {
            self.sidebar.clear();
            return;
        };
        let scale = self.sidebar.text.scale();
        let (cell_w, cell_h, _) = self.sidebar.text.cell_metrics();
        let quads = crate::cells::sidebar_quads(view, &self.theme, cell_w, cell_h, scale);
        self.sidebar.set(
            &self.device,
            &self.queue,
            (self.config.width, self.config.height),
            &quads,
            PaneTextInput {
                rows: view.rows,
                left: view.text_origin.0,
                top: view.text_origin.1,
                clip: (view.panel.x, view.panel.y, view.panel.w, view.panel.h),
            },
        );
    }

    /// Set the git diff dock to draw next frame, or clear it with `None` (closed).
    /// Builds the left-edge divider, the selected-file highlight, and the translucent
    /// add/del/hunk line backgrounds; the diff text comes baked into `dock.rows`. Drawn
    /// as base chrome (beneath the palette/settings overlays) on the right edge, with the
    /// pane viewport inset to its left.
    pub fn set_git_dock(&mut self, dock: Option<&GitDockView>) {
        let Some(view) = dock else {
            self.gitdock.clear();
            return;
        };
        let scale = self.gitdock.text.scale();
        let (cell_w, cell_h, _) = self.gitdock.text.cell_metrics();
        let quads = crate::cells::gitdock_quads(view, &self.theme, cell_w, cell_h, scale);
        self.gitdock.set(
            &self.device,
            &self.queue,
            (self.config.width, self.config.height),
            &quads,
            PaneTextInput {
                rows: view.rows,
                left: view.text_origin.0,
                top: view.text_origin.1,
                clip: (view.panel.x, view.panel.y, view.panel.w, view.panel.h),
            },
        );
    }

    /// Set the command-palette overlay to draw over the terminal next frame, or clear
    /// it with `None`. Builds the panel surface (`bg.elevated`), its `border.strong`
    /// outline, the selected-row highlight, and the input caret; the text colors come
    /// baked into `overlay.rows`.
    pub fn set_overlay(&mut self, overlay: Option<&OverlayView>) {
        let Some(view) = overlay else {
            self.overlay.clear();
            return;
        };
        let scale = self.overlay.text.scale();
        let (cell_w, cell_h, _) = self.overlay.text.cell_metrics();
        let quads = crate::cells::overlay_quads(view, &self.theme, cell_w, cell_h, scale);
        self.overlay.set(
            &self.device,
            &self.queue,
            (self.config.width, self.config.height),
            &quads,
            PaneTextInput {
                rows: view.rows,
                left: view.text_origin.0,
                top: view.text_origin.1,
                clip: (view.panel.x, view.panel.y, view.panel.w, view.panel.h),
            },
        );
    }

    /// Set the full-window settings view to draw over everything next frame, or clear
    /// it with `None` (closed). Builds the nav/content panel fills, the active-category
    /// and focused-control highlights, and the divider; the text colors come baked into
    /// `settings.rows`. Drawn on top so it never unmounts the panes beneath (AGENTS Hard
    /// rule 4).
    pub fn set_settings(&mut self, settings: Option<&SettingsView>) {
        let Some(view) = settings else {
            self.settings.clear();
            return;
        };
        let scale = self.settings.text.scale();
        let (cell_w, cell_h, _) = self.settings.text.cell_metrics();
        let quads = crate::cells::settings_quads(view, &self.theme, cell_w, cell_h, scale);
        self.settings.set(
            &self.device,
            &self.queue,
            (self.config.width, self.config.height),
            &quads,
            PaneTextInput {
                rows: view.rows,
                left: view.text_origin.0,
                top: view.text_origin.1,
                clip: (view.panel.x, view.panel.y, view.panel.w, view.panel.h),
            },
        );
    }

    /// Acquire the next surface frame, paint it, and present.
    ///
    /// Recovers from a lost/outdated swapchain by reconfiguring and skipping the
    /// frame; transient states (timeout, occluded) skip the frame.
    ///
    /// # Errors
    /// Returns [`RenderError`] if painting fails or the surface fails
    /// unrecoverably.
    pub fn render(&mut self) -> Result<(), RenderError> {
        use wgpu::CurrentSurfaceTexture as Cst;

        let frame = match self.surface.get_current_texture() {
            Cst::Success(frame) | Cst::Suboptimal(frame) => frame,
            Cst::Outdated | Cst::Lost => {
                self.surface.configure(&self.device, &self.config);
                return Ok(());
            }
            Cst::Timeout | Cst::Occluded => return Ok(()),
            Cst::Validation => {
                return Err(RenderError::Surface("surface validation error".to_owned()))
            }
        };

        let view = frame
            .texture
            .create_view(&wgpu::TextureViewDescriptor::default());
        let bg = self.theme.bg_base;
        // Pass 1: clear to the background and fill cell backgrounds + cursor.
        self.quads.draw(
            &self.device,
            &self.queue,
            &view,
            Some(wgpu::Color {
                r: bg.r,
                g: bg.g,
                b: bg.b,
                a: bg.a,
            }),
        );
        // Pass 2: draw the glyphs on top.
        self.text.draw(
            &self.device,
            &self.queue,
            &view,
            self.config.width,
            self.config.height,
        )?;
        let (w, h) = (self.config.width, self.config.height);
        // Passes 3-4: the sidebar chrome, loaded over the cleared left strip.
        self.sidebar.draw(&self.device, &self.queue, &view, w, h)?;
        // Passes 5-6: the git diff dock, loaded over the cleared right strip (base
        // chrome, like the sidebar; the pane viewport insets to its left).
        self.gitdock.draw(&self.device, &self.queue, &view, w, h)?;
        // Passes 7-8: the command palette / overlay, loaded over everything.
        self.overlay.draw(&self.device, &self.queue, &view, w, h)?;
        // Passes 9-10: the full-window settings view, loaded over everything else. It is
        // mutually exclusive with the palette, so the two never draw together.
        self.settings.draw(&self.device, &self.queue, &view, w, h)?;
        self.queue.present(frame);
        Ok(())
    }
}
