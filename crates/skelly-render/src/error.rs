//! Renderer error type.

/// Anything that can go wrong painting a frame.
#[derive(Debug, thiserror::Error)]
pub enum RenderError {
    /// The GPU surface failed in a way we could not silently recover from.
    #[error("surface error: {0}")]
    Surface(String),
    /// Shaping/preparing or drawing the text failed.
    #[error("text error: {0}")]
    Text(String),
}
