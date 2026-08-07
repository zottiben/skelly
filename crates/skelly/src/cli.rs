//! The command line in front of the window: bare `skelly` opens the terminal, while
//! `skelly update` installs the latest release in place, so nobody has to remember the
//! `curl … | sh` one-liner to upgrade.
//!
//! `update` deliberately owns no install logic of its own - it fetches the published
//! `install.sh` and runs it. That script already knows every platform detail (the macOS
//! app bundle, the Linux binary + desktop entry, checksum verification, the sudo
//! fallbacks), and keeping one copy of that knowledge means an update can never drift
//! from a fresh install. Skelly only contributes what the script cannot know on its own:
//! the running binary's version, passed as `SKELLY_CURRENT_VERSION` so the script can
//! answer "already up to date" without downloading a release.

use std::env;
use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;

use anyhow::{bail, Context as _};

/// Where the install/update script is published. `SKELLY_INSTALL_URL` overrides it (any
/// URL curl accepts, including `file://`, which is how the tests run this offline).
const INSTALL_URL: &str = "https://zottiben.github.io/skelly/install.sh";

/// What the invocation asked for.
#[derive(Clone, PartialEq, Eq, Debug)]
pub(crate) enum Cli {
    /// No arguments: open the terminal window (the normal path).
    Run,
    /// `skelly update [args…]` - the args are forwarded verbatim to the install script.
    Update(Vec<String>),
    /// `skelly --version`.
    Version,
    /// `skelly --help`.
    Help,
}

/// Parse the arguments after the executable name. Returns the offending argument as the
/// error when the invocation is not understood; there is no partial guessing.
pub(crate) fn parse<I>(args: I) -> Result<Cli, String>
where
    I: IntoIterator<Item = String>,
{
    // LaunchServices appends `-psn_0_12345` when macOS opens the .app bundle from Finder;
    // it is not a user argument and must not be treated as one.
    let mut args = args
        .into_iter()
        .filter(|a| !a.starts_with("-psn_"))
        .peekable();
    let Some(first) = args.next() else {
        return Ok(Cli::Run);
    };
    match first.as_str() {
        "update" => Ok(Cli::Update(args.collect())),
        "-V" | "--version" => Ok(Cli::Version),
        "-h" | "--help" => Ok(Cli::Help),
        other => Err(other.to_string()),
    }
}

/// `skelly --help`.
pub(crate) fn help() -> String {
    format!(
        "skelly {} - a barebones, keyboard-driven terminal emulator.\n\
         \n\
         Usage:\n  \
           skelly                 open the terminal\n  \
           skelly update          install the latest release\n  \
           skelly update --check  report whether an update is available\n  \
           skelly --version       print the version\n  \
           skelly --help          print this help\n\
         \n\
         Settings live in ~/.config/skelly/config.toml; press ⌘K in the app for commands.",
        env!("CARGO_PKG_VERSION")
    )
}

/// Download the published install script and run it, forwarding `args` and inheriting
/// stdio so the script's own progress output is what the user sees. Returns the script's
/// exit code.
pub(crate) fn update(args: &[String]) -> anyhow::Result<i32> {
    if which("curl").is_none() {
        bail!("`skelly update` needs curl on your PATH. Install curl, or download a build from https://github.com/zottiben/skelly/releases/latest");
    }
    if let Some(exe) = env::current_exe().ok().filter(|p| is_build_dir(p)) {
        // A `cargo run` build updates the *installed* copy, not this one - say so rather
        // than let the version afterwards look like nothing happened.
        eprintln!(
            "Note: {} is a development build; this updates the installed copy of Skelly.",
            exe.display()
        );
    }

    let url = env::var("SKELLY_INSTALL_URL").unwrap_or_else(|_| INSTALL_URL.to_string());
    let script = TempScript::path();
    let fetch = Command::new("curl")
        .args(["-fsSL", &url, "-o"])
        .arg(&script.0)
        .status()
        .context("running curl to fetch the install script")?;
    if !fetch.success() {
        bail!("could not download the install script from {url}");
    }

    let status = Command::new("sh")
        .arg(&script.0)
        .args(args)
        .env("SKELLY_CURRENT_VERSION", env!("CARGO_PKG_VERSION"))
        .status()
        .context("running the install script")?;
    Ok(status.code().unwrap_or(1))
}

/// The downloaded script, removed when it goes out of scope (including on an error path).
struct TempScript(PathBuf);

impl TempScript {
    fn path() -> Self {
        Self(env::temp_dir().join(format!("skelly-install-{}.sh", std::process::id())))
    }
}

impl Drop for TempScript {
    fn drop(&mut self) {
        let _ = fs::remove_file(&self.0);
    }
}

/// Whether `exe` sits in a cargo build directory (`…/target/debug|release/skelly`, or a
/// cross-compiled `…/target/<triple>/debug/skelly`).
fn is_build_dir(exe: &Path) -> bool {
    let mut parents = exe.ancestors().skip(1);
    let profile = parents.next().and_then(Path::file_name);
    if !matches!(profile.and_then(|p| p.to_str()), Some("debug" | "release")) {
        return false;
    }
    parents
        .take(2)
        .any(|p| p.file_name().and_then(|n| n.to_str()) == Some("target"))
}

/// The first `PATH` entry holding an executable named `name`.
fn which(name: &str) -> Option<PathBuf> {
    env::split_paths(&env::var_os("PATH")?).find_map(|dir| {
        let candidate = dir.join(name);
        candidate.is_file().then_some(candidate)
    })
}

#[cfg(test)]
mod tests {
    use super::{is_build_dir, parse, Cli};
    use std::path::Path;

    fn args(list: &[&str]) -> Vec<String> {
        list.iter().map(|s| (*s).to_string()).collect()
    }

    #[test]
    fn bare_invocation_runs_the_terminal() {
        assert_eq!(parse(args(&[])), Ok(Cli::Run));
        // macOS passes a process-serial-number argument when opening the .app from Finder.
        assert_eq!(parse(args(&["-psn_0_774516"])), Ok(Cli::Run));
    }

    #[test]
    fn update_forwards_its_arguments_to_the_script() {
        assert_eq!(parse(args(&["update"])), Ok(Cli::Update(vec![])));
        assert_eq!(
            parse(args(&["update", "--check", "--version", "v0.1.8"])),
            Ok(Cli::Update(args(&["--check", "--version", "v0.1.8"])))
        );
    }

    #[test]
    fn version_help_and_unknown_arguments() {
        assert_eq!(parse(args(&["--version"])), Ok(Cli::Version));
        assert_eq!(parse(args(&["-V"])), Ok(Cli::Version));
        assert_eq!(parse(args(&["--help"])), Ok(Cli::Help));
        assert_eq!(parse(args(&["-h"])), Ok(Cli::Help));
        // An unrecognized argument is reported, never silently ignored.
        assert_eq!(parse(args(&["--nope"])), Err("--nope".to_string()));
        assert_eq!(parse(args(&["upgrade"])), Err("upgrade".to_string()));
    }

    #[test]
    fn recognizes_cargo_build_directories() {
        assert!(is_build_dir(Path::new("/w/skelly/target/debug/skelly")));
        assert!(is_build_dir(Path::new("/w/skelly/target/release/skelly")));
        assert!(is_build_dir(Path::new(
            "/w/target/aarch64-apple-darwin/release/skelly"
        )));
        assert!(!is_build_dir(Path::new("/usr/local/bin/skelly")));
        assert!(!is_build_dir(Path::new(
            "/Applications/Skelly.app/Contents/MacOS/skelly"
        )));
    }
}
