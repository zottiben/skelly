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

use crate::cells::{
    chrome_quad, grid_quads, logo_quads, push_outline, scrim_quad, Quad, QuadLayer,
    LOGO_WATERMARK_OPACITY,
};
use crate::error::RenderError;
use crate::prose::{ProseLabel, ProseLayer};
use crate::text::{PaneTextInput, TextLayer};
use crate::theme::Theme;
use crate::{
    DeadPaneView, GitDockView, OverlayView, PaneView, PxRect, SettingsView, SidebarView,
    TimelineView,
};

/// One chrome layer drawn over the terminal with `LoadOp::Load`: a quad pass then a text
/// pass, gated by whether it is currently shown. The sidebar, git dock, command palette,
/// and settings view are each one of these; only their quad-building and geometry differ,
/// which stays in the per-surface `set_*` methods.
struct ChromeLayer {
    quads: QuadLayer,
    text: TextLayer,
    /// Proportional text for surfaces built in the guide's fonts (the sidebar today, the
    /// other surfaces as they migrate). Empty for layers still drawn as a monospace grid.
    prose: ProseLayer,
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
            prose: ProseLayer::new(device, queue, format, scale_f32(scale)),
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

    /// Upload this layer's decorative `quads` and its monospace `text` (rows + position +
    /// clip), and mark it active for the next frame. (Monospace surfaces; the sidebar uses
    /// [`set_paint`](Self::set_paint).)
    fn set(
        &mut self,
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        surface: (u32, u32),
        quads: &[Quad],
        text: PaneTextInput,
    ) {
        self.set_all(device, queue, surface, quads, &[text]);
    }

    /// Like [`set`](Self::set) but for a layer made of several independently positioned
    /// text grids (e.g. one exit message per pane); active only when there is something
    /// to draw.
    fn set_all(
        &mut self,
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        surface: (u32, u32),
        quads: &[Quad],
        texts: &[PaneTextInput],
    ) {
        self.active = !texts.is_empty() || !quads.is_empty();
        self.quads.set(device, queue, surface.0, surface.1, quads);
        self.text.set_panes(texts);
        self.prose.clear();
    }

    /// Upload a proportional-chrome display list: decorative `quads` plus positioned prose
    /// `labels` clipped to `clip`. The monospace text is cleared (a paint layer never mixes
    /// the two). Active whenever there is anything to draw.
    fn set_paint(
        &mut self,
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        surface: (u32, u32),
        quads: &[Quad],
        labels: &[ProseLabel],
        clip: PxRect,
    ) {
        self.active = !quads.is_empty() || !labels.is_empty();
        self.quads.set(device, queue, surface.0, surface.1, quads);
        self.text.set_panes(&[]);
        self.prose.set_labels(labels, clip);
    }

    /// Hide the layer (nothing drawn next frame).
    fn clear(&mut self) {
        self.active = false;
        self.prose.clear();
    }

    /// Draw the layer over `view` (quads, then monospace text, then prose, all loading),
    /// when active.
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
        self.prose.draw(device, queue, view, width, height)?;
        Ok(())
    }
}

/// Narrow a DPI scale factor to `f32` for the prose layer (sub-pixel precision loss is
/// irrelevant for glyph sizing, matching the terminal text layer).
#[allow(
    clippy::cast_possible_truncation,
    reason = "scale-factor precision loss does not matter for glyph sizing"
)]
fn scale_f32(value: f64) -> f32 {
    value as f32
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
    /// The dim "shell exited" overlay drawn over any pane whose shell ended, above the
    /// terminal text but beneath every other chrome layer.
    pane_overlay: ChromeLayer,
    /// The persistent left sidebar, drawn as base chrome when shown.
    sidebar: ChromeLayer,
    /// The per-repo git diff dock, drawn as base chrome on the right when open.
    gitdock: ChromeLayer,
    /// The session-timeline dock, drawn as base chrome on the right when open (mutually
    /// exclusive with the git dock - only one right-dock surface at a time, Hard rule 4).
    timeline: ChromeLayer,
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
        // Bind the six chrome layers before the struct literal so the closure's borrows
        // of `device`/`queue`/`config` end (NLL) before those move into `Self`.
        let (pane_overlay, sidebar, gitdock, timeline, overlay, settings) =
            (chrome(), chrome(), chrome(), chrome(), chrome(), chrome());

        Self {
            theme: Theme::resolve(&appearance.theme),
            quads,
            text,
            pane_overlay,
            sidebar,
            gitdock,
            timeline,
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
        self.pane_overlay.resize(width, height);
        self.sidebar.resize(width, height);
        self.gitdock.resize(width, height);
        self.timeline.resize(width, height);
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
        self.pane_overlay.set_default_fg(self.theme.fg_primary);
        self.sidebar.set_default_fg(self.theme.fg_primary);
        self.gitdock.set_default_fg(self.theme.fg_primary);
        self.timeline.set_default_fg(self.theme.fg_primary);
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
            // The empty-state brand watermark (a pristine tab), beneath the glyphs.
            if let Some(bounds) = pane.logo {
                quads.extend(logo_quads(bounds, &self.theme, LOGO_WATERMARK_OPACITY));
            }
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

    /// Set the dim "shell exited" overlays to draw next frame (one per pane whose shell
    /// ended), or clear them with an empty slice. Each draws a translucent `bg.base` scrim
    /// over the pane's rect - dimming its preserved grid - then its centered message on top.
    /// Drawn above the terminal text but beneath every other chrome layer (Hard rule 4 - a
    /// layer; the panes never unmount, so a restart just respawns the shell in place).
    pub fn set_pane_overlays(&mut self, overlays: &[DeadPaneView]) {
        if overlays.is_empty() {
            self.pane_overlay.clear();
            return;
        }
        let mut quads = Vec::with_capacity(overlays.len());
        let mut texts = Vec::with_capacity(overlays.len());
        for view in overlays {
            quads.push(scrim_quad(view.rect, &self.theme));
            texts.push(PaneTextInput {
                rows: view.rows,
                left: view.text_origin.0,
                top: view.text_origin.1,
                clip: (view.rect.x, view.rect.y, view.rect.w, view.rect.h),
            });
        }
        self.pane_overlay.set_all(
            &self.device,
            &self.queue,
            (self.config.width, self.config.height),
            &quads,
            &texts,
        );
    }

    /// Set the persistent left sidebar to draw next frame, or clear it with `None`
    /// (hidden). Proportional chrome: the binary hands over the finished display list
    /// (decorative `quads` + positioned prose `labels`); the renderer paints them clipped
    /// to the panel. Drawn as base chrome beneath any overlay.
    pub fn set_sidebar(&mut self, sidebar: Option<&SidebarView>) {
        let Some(view) = sidebar else {
            self.sidebar.clear();
            return;
        };
        let quads: Vec<Quad> = view.quads.iter().map(chrome_quad).collect();
        self.sidebar.set_paint(
            &self.device,
            &self.queue,
            (self.config.width, self.config.height),
            &quads,
            view.labels,
            view.panel,
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

    /// Set the session-timeline dock to draw next frame, or clear it with `None` (closed).
    /// Builds the left-edge divider, the selected event's `accent.subtle` fill, and - when
    /// rewound - the viewed event's `accent` bar; the text comes baked into `dock.rows`.
    /// Drawn as base chrome on the right edge (mutually exclusive with the git dock), with
    /// the pane viewport inset to its left.
    pub fn set_timeline(&mut self, dock: Option<&TimelineView>) {
        let Some(view) = dock else {
            self.timeline.clear();
            return;
        };
        let scale = self.timeline.text.scale();
        let mut quads = crate::cells::dock_frame_quads(view.panel, &self.theme, scale);
        quads.extend(view.quads.iter().map(chrome_quad));
        self.timeline.set_paint(
            &self.device,
            &self.queue,
            (self.config.width, self.config.height),
            &quads,
            view.labels,
            view.panel,
        );
    }

    /// Set the command-palette / modal overlay to draw over the terminal next frame, or
    /// clear it with `None`. Draws the floating card (shadow + `border.strong` ring +
    /// `bg.elevated` fill) from the panel, then the binary's content quads (selected pill,
    /// caret) and proportional labels on top.
    pub fn set_overlay(&mut self, overlay: Option<&OverlayView>) {
        let Some(view) = overlay else {
            self.overlay.clear();
            return;
        };
        let scale = self.overlay.text.scale();
        let mut quads = crate::cells::card_quads(view.panel, &self.theme, scale);
        quads.extend(view.quads.iter().map(chrome_quad));
        self.overlay.set_paint(
            &self.device,
            &self.queue,
            (self.config.width, self.config.height),
            &quads,
            view.labels,
            view.panel,
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
        let mut quads =
            crate::cells::settings_frame_quads(view.panel, view.nav_divider_x, &self.theme, scale);
        quads.extend(view.quads.iter().map(chrome_quad));
        self.settings.set_paint(
            &self.device,
            &self.queue,
            (self.config.width, self.config.height),
            &quads,
            view.labels,
            view.panel,
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
        // The "shell exited" scrims + messages, loaded over the terminal text so the dimmed
        // scrollback shows through; beneath the sidebar/docks/overlays so those stay on top.
        self.pane_overlay
            .draw(&self.device, &self.queue, &view, w, h)?;
        // Passes 3-4: the sidebar chrome, loaded over the cleared left strip.
        self.sidebar.draw(&self.device, &self.queue, &view, w, h)?;
        // Passes 5-6: the git diff dock, loaded over the cleared right strip (base
        // chrome, like the sidebar; the pane viewport insets to its left).
        self.gitdock.draw(&self.device, &self.queue, &view, w, h)?;
        // The session-timeline dock shares the right strip and is mutually exclusive with
        // the git dock (only one right-dock surface open at a time), so at most one of
        // these two draws.
        self.timeline.draw(&self.device, &self.queue, &view, w, h)?;
        // Passes 7-8: the command palette / overlay, loaded over everything.
        self.overlay.draw(&self.device, &self.queue, &view, w, h)?;
        // Passes 9-10: the full-window settings view, loaded over everything else. It is
        // mutually exclusive with the palette, so the two never draw together.
        self.settings.draw(&self.device, &self.queue, &view, w, h)?;
        self.queue.present(frame);
        Ok(())
    }
}
