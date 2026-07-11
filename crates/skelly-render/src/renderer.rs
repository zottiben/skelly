//! The GPU renderer.
//!
//! M1a: owns the `wgpu` device, queue, and surface, and clears the surface to the
//! resolved theme background each frame. Kept decoupled from the windowing crate -
//! it accepts anything with a raw window handle, so `skelly` never leaks `winit`
//! types into here and the backend stays swappable (ADR-0003).

use std::sync::Arc;

use raw_window_handle::{HasDisplayHandle, HasWindowHandle};

use crate::theme::Theme;

/// Errors the renderer surfaces to its caller.
#[derive(Debug, thiserror::Error)]
pub enum RenderError {
    /// The GPU surface failed in a way we could not silently recover from.
    #[error("surface error: {0}")]
    Surface(String),
}

/// Owns the GPU device and surface and paints frames.
pub struct Renderer {
    surface: wgpu::Surface<'static>,
    device: wgpu::Device,
    queue: wgpu::Queue,
    config: wgpu::SurfaceConfiguration,
    theme: Theme,
}

impl Renderer {
    /// Create a renderer bound to `window`, sized `width` x `height`, clearing to
    /// the background of the theme named `theme_name`. Blocks on GPU init.
    ///
    /// # Panics
    /// Panics if no suitable GPU adapter or device is available, or the surface
    /// cannot be created - all unrecoverable at startup.
    #[must_use]
    pub fn new<W>(window: Arc<W>, width: u32, height: u32, theme_name: &str) -> Self
    where
        W: HasWindowHandle + HasDisplayHandle + Send + Sync + 'static,
    {
        pollster::block_on(Self::new_async(window, width, height, theme_name))
    }

    async fn new_async<W>(window: Arc<W>, width: u32, height: u32, theme_name: &str) -> Self
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

        Self {
            surface,
            device,
            queue,
            config,
            theme: Theme::resolve(theme_name),
        }
    }

    /// Reconfigure the surface for a new size. No-ops on a zero dimension
    /// (minimized window).
    pub fn resize(&mut self, width: u32, height: u32) {
        if width > 0 && height > 0 {
            self.config.width = width;
            self.config.height = height;
            self.surface.configure(&self.device, &self.config);
        }
    }

    /// Paint one frame: clear the surface to the theme background.
    ///
    /// Recovers from a lost/outdated swapchain by reconfiguring and skipping the
    /// frame; transient states (timeout, occluded) skip the frame; only an
    /// unrecoverable failure is returned.
    ///
    /// # Errors
    /// Returns [`RenderError::Surface`] on an unrecoverable surface validation
    /// failure.
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
        let mut encoder = self
            .device
            .create_command_encoder(&wgpu::CommandEncoderDescriptor {
                label: Some("skelly-frame"),
            });

        let bg = self.theme.bg_base;
        {
            let _pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
                label: Some("clear"),
                color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                    view: &view,
                    depth_slice: None,
                    resolve_target: None,
                    ops: wgpu::Operations {
                        load: wgpu::LoadOp::Clear(wgpu::Color {
                            r: bg.r,
                            g: bg.g,
                            b: bg.b,
                            a: bg.a,
                        }),
                        store: wgpu::StoreOp::Store,
                    },
                })],
                depth_stencil_attachment: None,
                timestamp_writes: None,
                occlusion_query_set: None,
                multiview_mask: None,
            });
        }

        self.queue.submit(std::iter::once(encoder.finish()));
        self.queue.present(frame);
        Ok(())
    }
}
