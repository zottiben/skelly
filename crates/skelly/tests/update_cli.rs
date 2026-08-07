//! Integration test: run the real `skelly` binary's non-windowing subcommands, and drive
//! `skelly update` end-to-end against the repo's own `install.sh`.
//!
//! The update path is exercised without touching the network or the machine: pointing
//! `SKELLY_INSTALL_URL` at a `file://` copy of the script makes curl fetch it locally, and
//! `--check --version <tag>` makes the script report what it would do and exit before it
//! resolves a release or downloads anything. That covers the whole chain - argument
//! parsing, the script fetch, `SKELLY_CURRENT_VERSION` hand-off, and the script's
//! up-to-date comparison - which is where an updater actually breaks.

use std::path::{Path, PathBuf};
use std::process::{Command, Output};

/// The version the built binary reports, i.e. the workspace version.
const VERSION: &str = env!("CARGO_PKG_VERSION");

/// The repo root (`crates/skelly/` is this test's manifest dir).
fn repo_root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../..")
        .canonicalize()
        .expect("repo root")
}

/// Run the built `skelly` binary with `args`, isolated from a real install: the receipt
/// lives under `HOME`, so a temp `HOME` keeps the test off the user's own state.
fn skelly(args: &[&str], home: &Path) -> Output {
    Command::new(env!("CARGO_BIN_EXE_skelly"))
        .args(args)
        .env("HOME", home)
        .env(
            "SKELLY_INSTALL_URL",
            format!("file://{}", repo_root().join("install.sh").display()),
        )
        .output()
        .expect("running skelly")
}

/// Whether an executable is on `PATH` (the update path shells out to curl and sh).
fn have(tool: &str) -> bool {
    std::env::split_paths(&std::env::var_os("PATH").unwrap_or_default())
        .any(|dir| dir.join(tool).is_file())
}

#[test]
fn reports_its_version_and_help_without_opening_a_window() {
    let home = tempfile::tempdir().expect("temp home");
    let out = skelly(&["--version"], home.path());
    assert!(out.status.success());
    assert_eq!(
        String::from_utf8_lossy(&out.stdout).trim(),
        format!("skelly {VERSION}")
    );

    let out = skelly(&["--help"], home.path());
    assert!(out.status.success());
    let help = String::from_utf8_lossy(&out.stdout);
    assert!(
        help.contains("skelly update"),
        "help documents update: {help}"
    );

    // An unknown argument fails loudly instead of falling through to the window.
    let out = skelly(&["--frobnicate"], home.path());
    assert_eq!(out.status.code(), Some(2));
    assert!(String::from_utf8_lossy(&out.stderr).contains("--frobnicate"));
}

#[test]
fn update_check_compares_the_running_version_against_the_release() {
    if !have("curl") || !have("sh") {
        eprintln!("skipping: curl/sh not available");
        return;
    }
    let home = tempfile::tempdir().expect("temp home");

    // Asking for the version we are already running: nothing to do.
    let tag = format!("v{VERSION}");
    let out = skelly(&["update", "--check", "--version", &tag], home.path());
    let stdout = String::from_utf8_lossy(&out.stdout);
    assert!(
        out.status.success(),
        "up-to-date check succeeds: {stdout}{}",
        String::from_utf8_lossy(&out.stderr)
    );
    assert!(
        stdout.contains(&format!("Skelly v{VERSION} is up to date")),
        "reports up to date: {stdout}"
    );

    // A newer release: report the upgrade, and still install nothing under --check.
    let out = skelly(&["update", "--check", "--version", "v99.0.0"], home.path());
    let stdout = String::from_utf8_lossy(&out.stdout);
    assert!(out.status.success());
    assert!(
        stdout.contains(&format!("Update available: v{VERSION} -> v99.0.0")),
        "reports the available update: {stdout}"
    );
    assert!(!home.path().join(".local/bin/skelly").exists());

    // An older release than the running build is reported, never installed over it:
    // `skelly update` must not silently downgrade a development or hand-picked build.
    let out = skelly(&["update", "--check", "--version", "v0.0.1"], home.path());
    let stdout = String::from_utf8_lossy(&out.stdout);
    assert!(out.status.success());
    assert!(
        stdout.contains(&format!("Skelly v{VERSION} is newer than v0.0.1")),
        "refuses to call a downgrade an update: {stdout}"
    );

    // Without --force, an install of the version already installed is a no-op.
    let out = skelly(&["update", "--version", &tag], home.path());
    assert!(out.status.success());
    assert!(String::from_utf8_lossy(&out.stdout).contains("already installed"));

    // Unknown flags are the script's to reject, and its exit code propagates.
    let out = skelly(&["update", "--wat"], home.path());
    assert_eq!(out.status.code(), Some(2));
    assert!(String::from_utf8_lossy(&out.stderr).contains("Unknown option"));
}

#[test]
fn install_script_is_valid_posix_shell() {
    if !have("sh") {
        eprintln!("skipping: sh not available");
        return;
    }
    let out = Command::new("sh")
        .arg("-n")
        .arg(repo_root().join("install.sh"))
        .output()
        .expect("running sh -n");
    assert!(
        out.status.success(),
        "install.sh parses: {}",
        String::from_utf8_lossy(&out.stderr)
    );
}
