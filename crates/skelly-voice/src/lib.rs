//! Local speech integration, independent of the terminal parser and GPU.
//!
//! The first slice is a private, per-pane bridge to the Pi process already running
//! in that pane. It never writes terminal input or starts a second agent.

pub mod audio;
pub mod bridge;
pub mod conversation;
pub mod dictation;
mod process;
pub mod speech;
pub mod whisper;
