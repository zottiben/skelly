//! The GPU renderer.
//!
//! M1b: owns the `wgpu` device, queue, and surface, and drives a [`TextLayer`] to
//! paint each frame (clear to the theme background + shaped text). Kept decoupled
//! from the windowing crate - it accepts anything with a raw window handle, so
//! `skelly` never leaks `winit` types into here and the backend stays swappable
//! (ADR-0003).

use std::sync::Arc;

use raw_window_handle::{HasDisplayHandle, HasWindowHandle};
use skelly_config::Appearance;

use crate::error::RenderError;
use crate::text::TextLayer;

/// Owns the GPU device and surface and presents painted frames.
pub struct Renderer {
    surface: wgpu::Surface<'static>,
    device: wgpu::Device,
    queue: wgpu::Queue,
    config: wgpu::SurfaceConfiguration,
    text: TextLayer,
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

        let text = TextLayer::new(
            &device,
            &queue,
            config.format,
            config.width,
            config.height,
            scale_factor,
            appearance,
        );

        Self {
            surface,
            device,
            queue,
            config,
            text,
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
    }

    /// Set the text to display next frame (e.g. the live terminal grid snapshot).
    pub fn set_content(&mut self, text: &str) {
        self.text.set_content(text);
    }

    /// Set a colored grid to display next frame: each cell is `(char, fg)`.
    pub fn set_content_rgb(&mut self, rows: &[Vec<(char, crate::theme::Srgb)>]) {
        self.text.set_cells(rows);
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
        self.text.draw(
            &self.device,
            &self.queue,
            &view,
            self.config.width,
            self.config.height,
        )?;
        self.queue.present(frame);
        Ok(())
    }
}
