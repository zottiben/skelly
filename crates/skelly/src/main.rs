//! `skelly` - the binary: window, sidebar, pane tree, command palette, and the
//! wiring that binds the library crates together.
//!
//! At M0 this is a thin harness that proves the config slice end-to-end: it
//! initializes structured logging, loads `~/.config/skelly/config.toml` (or the
//! spec defaults when there is no file), validates it, and reports the resolved
//! settings. The GPU window, PTY, and cell renderer arrive with the M1 walking
//! skeleton. Errors are contextualized at this boundary with `anyhow`.

use anyhow::Context;
use skelly_config::Config;

fn main() -> anyhow::Result<()> {
    init_tracing();

    let config = Config::load_default().context("loading configuration")?;
    let source = match Config::default_path() {
        Some(path) if path.exists() => path.display().to_string(),
        _ => "built-in defaults".to_owned(),
    };

    tracing::info!(
        version = env!("CARGO_PKG_VERSION"),
        %source,
        theme = %config.appearance.theme,
        font_size = config.appearance.font_size,
        max_panes = config.panes.max,
        "config loaded"
    );

    print_summary(&config, &source);
    Ok(())
}

/// Initialize `tracing` with an env filter (`SKELLY_LOG`, default `info`), writing
/// structured logs to stderr. This is the primary debugging tool for the GPU/PTY
/// paths that land later.
fn init_tracing() {
    use tracing_subscriber::{fmt, EnvFilter};
    let filter = EnvFilter::try_from_env("SKELLY_LOG").unwrap_or_else(|_| EnvFilter::new("info"));
    fmt().with_env_filter(filter).with_target(false).init();
}

/// Print the human-facing startup summary to stdout.
fn print_summary(config: &Config, source: &str) {
    println!("skelly {} (M0 scaffold)", env!("CARGO_PKG_VERSION"));
    println!("  config: {source}");
    println!("  theme:  {}", config.appearance.theme);
    println!(
        "  font:   {} @ {}px",
        config.appearance.font_family, config.appearance.font_size
    );
    println!("  panes:  max {}", config.panes.max);
    println!("\nThe GPU window + PTY + cell renderer arrive with the M1 walking skeleton.");
}
