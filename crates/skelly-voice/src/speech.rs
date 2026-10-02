//! Local, cancellable speech playback. Text is data, never shell arguments or engine markup.

use std::fs::{self, OpenOptions, Permissions};
use std::io::{self, Write};
use std::os::unix::fs::{OpenOptionsExt, PermissionsExt};
use std::panic::{catch_unwind, AssertUnwindSafe};
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::mpsc;
use std::thread;
use std::time::{Duration, Instant};

use pulldown_cmark::{Event, LinkType, Options, Parser, Tag};
use thiserror::Error;

use crate::bridge::MAX_TEXT;
use crate::dictation::Control;
use crate::process::EngineProcess;

/// Playback failures contain diagnostics, never the spoken text.
#[derive(Debug, Error)]
pub enum Error {
    /// Explicit local interruption, not agent cancellation.
    #[error("Speech stopped")]
    Cancelled,
    /// Local engine or voice settings are unavailable.
    #[error("{0}")]
    Setup(String),
    /// Local filesystem/process failure.
    #[error("Local speech I/O: {0}")]
    Io(#[from] io::Error),
    /// The local process exited unsuccessfully.
    #[error("Local speech failed ({0}); check the installed voice and output device")]
    Engine(String),
    /// Safety deadline exceeded.
    #[error("Local speech timed out")]
    Timeout,
    /// A backend panicked; allow a subsequent playback to start normally.
    #[error("Local speech worker failed; check the installed voice and output device")]
    Worker,
}

/// Convert visible Markdown prose to bounded plain speech.
/// Code, HTML, images, tables, footnotes and URL destinations are not narrated.
/// Brackets are removed to neutralize say/espeak embedded phoneme/speech commands.
#[must_use]
pub fn spoken_text(markdown: &str) -> String {
    if markdown.len() > MAX_TEXT {
        return String::new();
    }
    let mut prose = String::new();
    let mut skip = 0_u32;
    let options = Options::ENABLE_TABLES | Options::ENABLE_FOOTNOTES;
    for event in Parser::new_ext(markdown, options) {
        match event {
            Event::Start(tag) => {
                if skip > 0
                    || matches!(
                        tag,
                        Tag::CodeBlock(_)
                            | Tag::Image { .. }
                            | Tag::Table(_)
                            | Tag::FootnoteDefinition(_)
                            | Tag::Link {
                                link_type: LinkType::Autolink | LinkType::Email,
                                ..
                            }
                    )
                {
                    skip += 1;
                }
            }
            Event::End(_) if skip > 0 => skip -= 1,
            _ if skip > 0 => {}
            Event::Text(text) => prose.push_str(&text),
            Event::End(_) | Event::SoftBreak | Event::HardBreak | Event::Rule => prose.push(' '),
            _ => {}
        }
    }
    let plain: String = prose
        .chars()
        .filter(|c| !matches!(c, '[' | ']') && (!c.is_control() || c.is_whitespace()))
        .collect();
    let clean = plain
        .split_whitespace()
        .filter(|word| {
            let word = word.trim_start_matches(['(', '<', '[']);
            !word.starts_with("https://")
                && !word.starts_with("http://")
                && !word.starts_with("www.")
        })
        .collect::<Vec<_>>()
        .join(" ");
    let mut bounded: String = clean.chars().take(2000).collect();
    if clean.chars().count() > 2000 {
        bounded.push('…');
    }
    bounded
}

/// Speech boundary for offline fixture tests and swappable local engines.
pub trait Speaker: Send + 'static {
    /// Speak visible reply text until completion, cancellation or timeout.
    ///
    /// # Errors
    /// Returns setup, device, process or cancellation failures.
    fn speak(&self, text: &str, control: &Control) -> Result<(), Error>;
}

#[derive(Clone, Copy)]
enum Engine {
    Say,
    Espeak,
}

/// Platform-local speech: macOS say or explicitly installed Linux espeak-ng.
pub struct LocalSpeech {
    engine: Engine,
    program: PathBuf,
    voice: String,
    rate: u16,
    timeout: Duration,
}

impl LocalSpeech {
    /// Use an installed OS voice; this never installs or downloads voices.
    #[must_use]
    pub fn system(voice: &str, rate: u16) -> Self {
        let (engine, program) = if cfg!(target_os = "macos") {
            (Engine::Say, PathBuf::from("/usr/bin/say"))
        } else {
            (Engine::Espeak, PathBuf::from("espeak-ng"))
        };
        Self {
            engine,
            program,
            voice: voice.into(),
            rate,
            timeout: Duration::from_mins(5),
        }
    }

    fn executable(&self) -> Result<PathBuf, Error> {
        let usable = |p: &Path| {
            p.metadata()
                .is_ok_and(|m| m.is_file() && m.permissions().mode() & 0o111 != 0)
        };
        if self.program.is_absolute() && usable(&self.program) {
            return Ok(self.program.clone());
        }
        if let Some(path) = std::env::var_os("PATH") {
            for directory in std::env::split_paths(&path) {
                let candidate = directory.join(&self.program);
                if usable(&candidate) {
                    return Ok(fs::canonicalize(candidate)?);
                }
            }
        }
        Err(Error::Setup("Local speech unavailable; macOS requires say, Linux requires an explicit espeak-ng installation".into()))
    }
}

impl Speaker for LocalSpeech {
    fn speak(&self, text: &str, control: &Control) -> Result<(), Error> {
        check(control)?;
        if !(80..=400).contains(&self.rate)
            || self.voice.len() > 128
            || self.voice.chars().any(char::is_control)
        {
            return Err(Error::Setup("Invalid local voice or speech rate".into()));
        }
        let text = spoken_text(text);
        if text.is_empty() {
            return Ok(());
        }
        let mut command = Command::new(self.executable()?);
        let directory = tempfile::Builder::new()
            .prefix("skelly-speech-")
            .permissions(Permissions::from_mode(0o700))
            .tempdir()?;
        let input = directory.path().join("speech.txt");
        let mut file = OpenOptions::new()
            .write(true)
            .create_new(true)
            .mode(0o600)
            .open(&input)?;
        file.write_all(text.as_bytes())?;
        drop(file);
        command.arg("-f").arg(&input);
        if matches!(self.engine, Engine::Espeak) {
            command.args(["-b", "1"]);
        }
        command
            .arg(match self.engine {
                Engine::Say => "-r",
                Engine::Espeak => "-s",
            })
            .arg(self.rate.to_string());
        if !self.voice.is_empty() {
            command.arg("-v").arg(&self.voice);
        }
        command
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null());
        check(control)?;
        let mut child = EngineProcess::spawn(&mut command)?;
        let deadline = Instant::now() + self.timeout;
        loop {
            check(control)?;
            if Instant::now() >= deadline {
                return Err(Error::Timeout);
            }
            if let Some(status) = child.try_wait()? {
                return if status.success() {
                    Ok(())
                } else {
                    Err(Error::Engine(status.to_string()))
                };
            }
            thread::sleep(Duration::from_millis(20));
        }
    }
}

fn check(control: &Control) -> Result<(), Error> {
    control.check().map_err(|_| Error::Cancelled)
}

/// One playback worker. Keep it owned until completion; shutdown waits for reaping.
pub struct Playback {
    control: Control,
    result: mpsc::Receiver<Result<(), Error>>,
    worker: Option<thread::JoinHandle<()>>,
}

impl Playback {
    /// Begin local playback off the window loop.
    ///
    /// # Errors
    /// Returns thread-start failures.
    pub fn start(
        speaker: impl Speaker,
        text: String,
        wakeup: impl Fn() + Send + 'static,
    ) -> io::Result<Self> {
        let control = Control::default();
        let worker_control = control.clone();
        let (sender, result) = mpsc::channel();
        let worker = thread::Builder::new()
            .name("skelly-speech".into())
            .spawn(move || {
                let result =
                    catch_unwind(AssertUnwindSafe(|| speaker.speak(&text, &worker_control)))
                        .unwrap_or(Err(Error::Worker));
                let result = check(&worker_control).and(result);
                let _ = sender.send(result);
                wakeup();
            })?;
        Ok(Self {
            control,
            result,
            worker: Some(worker),
        })
    }
    /// Stop only local audio, never the agent.
    pub fn cancel(&self) {
        self.control.cancel();
    }
    /// Nonblocking completion poll.
    #[must_use]
    pub fn poll(&self) -> Option<Result<(), Error>> {
        self.result.try_recv().ok()
    }
    /// Cancel and reap before the application exits.
    pub fn shutdown(mut self) {
        self.cancel();
        if let Some(worker) = self.worker.take() {
            let _ = worker.join();
        }
    }
}

impl Drop for Playback {
    fn drop(&mut self) {
        self.cancel();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn fixture(script: &str, engine: Engine) -> (tempfile::TempDir, LocalSpeech) {
        let dir = tempfile::tempdir().unwrap();
        let program = dir.path().join("speech fixture");
        fs::write(&program, format!("#!/bin/sh\nset -eu\n{script}")).unwrap();
        fs::set_permissions(&program, Permissions::from_mode(0o700)).unwrap();
        let speech = LocalSpeech {
            engine,
            program,
            voice: "installed voice".into(),
            rate: 180,
            timeout: Duration::from_secs(10),
        };
        (dir, speech)
    }

    fn wait(mut ready: impl FnMut() -> bool) {
        let deadline = Instant::now() + Duration::from_secs(5);
        while !ready() {
            assert!(Instant::now() < deadline);
            thread::sleep(Duration::from_millis(5));
        }
    }

    #[test]
    fn reads_prose_not_code_html_images_destinations_or_engine_commands() {
        let text = spoken_text("# Hello **world**\n\nUse `danger()` carefully.\n\n```sh\nsecret command\n```\n\n[Docs](https://secret.invalid/token) ![secret alt](image.png)\n\n<script>secret html</script>\n\n[[slnc 999999]] done.");
        assert!(text.contains("Hello world"));
        assert!(text.contains("carefully"));
        for omitted in [
            "danger()",
            "secret",
            "https://",
            "image.png",
            "[",
            "]",
            "```",
        ] {
            assert!(!text.contains(omitted), "unexpected narration: {omitted}");
        }
        assert_eq!(
            spoken_text("https://secret.invalid/a https://secret.invalid/b"),
            ""
        );
        assert_eq!(spoken_text("```rust\nfn main() {}\n```"), "");
        assert_eq!(spoken_text("你好 **世界**"), "你好 世界");
        assert!(spoken_text(&"好".repeat(2200)).chars().count() <= 2001);
        assert!(spoken_text(&"x".repeat(MAX_TEXT + 1)).is_empty());
    }

    #[test]
    fn local_engines_take_private_files_not_text_arguments_and_cleanup() {
        for (engine, rate_flag) in [(Engine::Say, "-r"), (Engine::Espeak, "-s")] {
            let (dir, speech) = fixture(
                r#"
printf '%s\n' "$@" > "$(dirname "$0")/args"
while [ "$#" -gt 0 ]; do
  case "$1" in -f) shift; input="$1";; esac
  shift
done
cp "$input" "$(dirname "$0")/text"
printf '%s' "$input" > "$(dirname "$0")/input-path"
"#,
                engine,
            );
            speech
                .speak(
                    "Sensitive **reply** [[slnc 999999]] --help",
                    &Control::default(),
                )
                .unwrap();
            let args = fs::read_to_string(dir.path().join("args")).unwrap();
            assert!(!args.contains("Sensitive"));
            assert!(!args.contains("--help"));
            assert!(args.lines().any(|arg| arg == rate_flag));
            assert!(args.contains("installed voice"));
            let copy = dir.path().join("text");
            assert_eq!(copy.metadata().unwrap().permissions().mode() & 0o777, 0o600);
            let text = fs::read_to_string(copy).unwrap();
            assert!(text.contains("Sensitive reply"));
            assert!(!text.contains('['));
            let input = fs::read_to_string(dir.path().join("input-path")).unwrap();
            assert!(!Path::new(&input).parent().unwrap().exists());
        }
    }

    #[test]
    fn cancellation_reaps_playback_and_removes_private_text() {
        let (dir, speech) = fixture(
            r#"
while [ "$#" -gt 0 ]; do
  case "$1" in -f) shift; input="$1";; esac
  shift
done
printf '%s' "$input" > "$(dirname "$0")/input-path"
echo $$ > "$(dirname "$0")/pid"
exec sleep 20
"#,
            Engine::Say,
        );
        let playback = Playback::start(speech, "private answer".into(), || {}).unwrap();
        wait(|| dir.path().join("pid").exists());
        playback.cancel();
        let mut result = None;
        wait(|| {
            result = playback.poll();
            result.is_some()
        });
        assert!(matches!(result, Some(Err(Error::Cancelled))));
        let pid = fs::read_to_string(dir.path().join("pid")).unwrap();
        assert!(!Command::new("kill")
            .args(["-0", pid.trim()])
            .stderr(Stdio::null())
            .status()
            .unwrap()
            .success());
        let input = fs::read_to_string(dir.path().join("input-path")).unwrap();
        assert!(!Path::new(&input).parent().unwrap().exists());
    }

    #[test]
    fn backend_panic_finishes_and_wakes_instead_of_sticking_playback() {
        struct PanickingSpeaker;
        impl Speaker for PanickingSpeaker {
            fn speak(&self, _: &str, _: &Control) -> Result<(), Error> {
                panic!("fixture backend failure");
            }
        }
        let (wake, ready) = mpsc::channel();
        let playback = Playback::start(PanickingSpeaker, "reply".into(), move || {
            let _ = wake.send(());
        })
        .unwrap();
        assert!(
            ready.recv_timeout(Duration::from_secs(2)).is_ok(),
            "panic must wake the UI"
        );
        assert!(matches!(playback.poll(), Some(Err(_))));
        assert!(playback.poll().is_none());
        playback.shutdown();
    }

    #[test]
    fn cancellation_stops_engine_helpers_too() {
        let (dir, speech) = fixture(
            r#"
sleep 20 &
echo $! > "$(dirname "$0")/helper"
wait
"#,
            Engine::Espeak,
        );
        let playback = Playback::start(speech, "reply".into(), || {}).unwrap();
        let mut helper = String::new();
        wait(|| {
            helper = fs::read_to_string(dir.path().join("helper")).unwrap_or_default();
            helper.trim().parse::<u32>().is_ok()
        });
        playback.shutdown();
        let deadline = Instant::now() + Duration::from_secs(2);
        let running = loop {
            let status = Command::new("ps")
                .args(["-o", "stat=", "-p", helper.trim()])
                .output()
                .unwrap();
            let state = String::from_utf8_lossy(&status.stdout);
            let running = !state.trim().is_empty() && !state.trim().starts_with('Z');
            if !running || Instant::now() >= deadline {
                break running;
            }
            thread::sleep(Duration::from_millis(5));
        };
        // Even the red regression run must not leave its fixture behind.
        if running {
            let _ = Command::new("kill")
                .args(["-KILL", helper.trim()])
                .stderr(Stdio::null())
                .status();
        }
        assert!(!running, "cancelling an engine must also stop its helper");
    }

    #[test]
    fn errors_timeouts_and_pre_cancel_are_not_successful_playback() {
        let (_dir, mut speech) = fixture("exit 7", Engine::Espeak);
        assert!(matches!(
            speech.speak("reply", &Control::default()),
            Err(Error::Engine(_))
        ));
        speech.timeout = Duration::ZERO;
        assert!(matches!(
            speech.speak("reply", &Control::default()),
            Err(Error::Timeout)
        ));
        let control = Control::default();
        control.cancel();
        assert!(matches!(
            speech.speak("reply", &control),
            Err(Error::Cancelled)
        ));
        speech.program = "/no-skelly-speech-engine".into();
        assert!(matches!(
            speech.speak("reply", &Control::default()),
            Err(Error::Setup(_))
        ));
        assert!(speech
            .speak("```\ncode only\n```", &Control::default())
            .is_ok());
    }
}
