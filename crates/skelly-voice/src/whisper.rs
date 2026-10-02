//! Offline whisper.cpp CLI adapter. Never invokes a shell or downloads a model.

use std::fs::{self, File, OpenOptions, Permissions};
use std::io::Read;
use std::os::unix::fs::{OpenOptionsExt, PermissionsExt};
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::time::{Duration, Instant};

use crate::bridge::MAX_TEXT;
use crate::dictation::{Audio, Control, Error, Transcriber};
use crate::process::EngineProcess;

/// Explicit local whisper.cpp installation. No executable arguments are taken from config.
pub struct Whisper {
    program: PathBuf,
    model: PathBuf,
    language: String,
    timeout: Duration,
}

impl Whisper {
    /// Configure a local backend; setup is checked on the job thread before capture.
    #[must_use]
    pub fn new(program: &str, model: &str, language: &str) -> Self {
        Self {
            program: expand_home(program),
            model: expand_home(model),
            language: language.into(),
            timeout: Duration::from_mins(2),
        }
    }

    fn executable(&self) -> Result<PathBuf, Error> {
        let usable = |p: &Path| {
            p.metadata()
                .is_ok_and(|m| m.is_file() && m.permissions().mode() & 0o111 != 0)
        };
        if self.program.components().count() > 1 || self.program.is_absolute() {
            if usable(&self.program) {
                return Ok(fs::canonicalize(&self.program)?);
            }
        } else if let Some(path) = std::env::var_os("PATH") {
            for directory in std::env::split_paths(&path) {
                let candidate = directory.join(&self.program);
                if usable(&candidate) {
                    return Ok(fs::canonicalize(candidate)?);
                }
            }
        }
        Err(Error::Setup(
            "Set Voice → Whisper executable to a locally installed whisper-cli (>=1.8.2)".into(),
        ))
    }
}

impl Transcriber for Whisper {
    fn validate(&self) -> Result<(), Error> {
        self.executable()?;
        if !self
            .model
            .metadata()
            .is_ok_and(|m| m.is_file() && m.len() > 0)
        {
            return Err(Error::Setup("Set Voice → Model path to a downloaded whisper.cpp ggml model; Skelly never downloads one".into()));
        }
        File::open(&self.model)?;
        if self.language != "auto"
            && (!(2..=3).contains(&self.language.len())
                || !self.language.bytes().all(|b| b.is_ascii_lowercase()))
        {
            return Err(Error::Setup(
                "Whisper language must be a lowercase language code or auto".into(),
            ));
        }
        Ok(())
    }

    fn transcribe(&self, audio: Audio, control: &Control) -> Result<String, Error> {
        control.check()?;
        self.validate()?;
        audio.validate()?;
        let directory = tempfile::Builder::new()
            .prefix("skelly-audio-")
            .permissions(Permissions::from_mode(0o700))
            .tempdir()?;
        let input = directory.path().join("utterance.wav");
        let output = directory.path().join("transcript");
        let text_path = output.with_extension("txt");
        let wav = OpenOptions::new()
            .write(true)
            .create_new(true)
            .mode(0o600)
            .open(&input)?;
        let mut writer = hound::WavWriter::new(
            wav,
            hound::WavSpec {
                channels: 1,
                sample_rate: audio.sample_rate,
                bits_per_sample: 16,
                sample_format: hound::SampleFormat::Int,
            },
        )
        .map_err(|error| wav_error(&error))?;
        for chunk in audio.samples.chunks(8192) {
            control.check()?;
            for sample in chunk {
                writer
                    .write_sample(*sample)
                    .map_err(|error| wav_error(&error))?;
            }
        }
        writer.finalize().map_err(|error| wav_error(&error))?;
        drop(audio);
        // Precreate with private permissions; whisper's ofstream truncates this file.
        OpenOptions::new()
            .write(true)
            .create_new(true)
            .mode(0o600)
            .open(&text_path)?;
        control.check()?;
        let mut child = EngineProcess::spawn(
            Command::new(self.executable()?)
                .args(["-np", "-nt", "-otxt", "-of"])
                .arg(&output)
                .arg("-m")
                .arg(fs::canonicalize(&self.model)?)
                .arg("-f")
                .arg(&input)
                .arg("-l")
                .arg(&self.language)
                .stdin(Stdio::null())
                .stdout(Stdio::null())
                .stderr(Stdio::null()),
        )?;
        let deadline = Instant::now() + self.timeout;
        loop {
            control.check()?;
            if Instant::now() >= deadline {
                return Err(Error::Timeout);
            }
            if text_path.metadata()?.len() > MAX_TEXT as u64 {
                return Err(Error::Invalid(
                    "Transcript exceeds the 8 KiB safety limit".into(),
                ));
            }
            if let Some(status) = child.try_wait()? {
                if !status.success() {
                    return Err(Error::Engine(status.to_string()));
                }
                break;
            }
            std::thread::sleep(Duration::from_millis(20));
        }
        let mut text = String::new();
        File::open(text_path)?
            .take(MAX_TEXT as u64 + 1)
            .read_to_string(&mut text)?;
        control.check()?;
        validate_text(&text)
    }
}

fn expand_home(value: &str) -> PathBuf {
    if let Some(suffix) = value.strip_prefix("~/") {
        if let Some(home) = std::env::var_os("HOME") {
            return PathBuf::from(home).join(suffix);
        }
    }
    PathBuf::from(value)
}

fn wav_error(error: &hound::Error) -> Error {
    Error::Invalid(format!("Could not write microphone audio: {error}"))
}

fn validate_text(text: &str) -> Result<String, Error> {
    let clean = text.trim();
    if clean.is_empty()
        || text.len() > MAX_TEXT
        || clean
            .chars()
            .any(|c| c.is_control() && !matches!(c, '\n' | '\r' | '\t'))
    {
        return Err(Error::Invalid(
            "No usable transcript (empty, too long or invalid control characters)".into(),
        ));
    }
    Ok(clean.into())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn fixture(script: &str) -> (tempfile::TempDir, Whisper) {
        let dir = tempfile::tempdir().unwrap();
        let program = dir.path().join("whisper fixture");
        fs::write(&program, format!("#!/bin/sh\nset -eu\n{script}\n")).unwrap();
        fs::set_permissions(&program, Permissions::from_mode(0o700)).unwrap();
        let model = dir.path().join("model file.bin");
        fs::write(&model, b"fixture").unwrap();
        let engine = Whisper::new(program.to_str().unwrap(), model.to_str().unwrap(), "en");
        (dir, engine)
    }

    fn audio() -> Audio {
        Audio {
            samples: vec![1024; 8000],
            sample_rate: 16000,
        }
    }

    #[test]
    fn cli_uses_private_wav_and_literal_paths_then_cleans_up() {
        let (dir, engine) = fixture(
            r#"
while [ "$#" -gt 0 ]; do
  case "$1" in -of) shift; output="$1";; -f) shift; input="$1";; esac
  shift
done
printf '%s' "$input" > "$(dirname "$0")/input-path"
[ -f "$input" ]
cp "$input" "$(dirname "$0")/captured.wav"
printf '  edit this draft, do not submit\n' > "$output.txt"
"#,
        );
        assert_eq!(
            engine.transcribe(audio(), &Control::default()).unwrap(),
            "edit this draft, do not submit"
        );
        let input = fs::read_to_string(dir.path().join("input-path")).unwrap();
        assert!(!Path::new(&input).parent().unwrap().exists());
        let saved = dir.path().join("captured.wav");
        assert_eq!(
            saved.metadata().unwrap().permissions().mode() & 0o777,
            0o600
        );
        let reader = hound::WavReader::open(saved).unwrap();
        assert_eq!(reader.spec().sample_rate, 16000);
        assert_eq!(reader.spec().channels, 1);
        assert_eq!(reader.spec().bits_per_sample, 16);
        assert_eq!(reader.len(), 8000);
    }

    #[test]
    fn missing_installations_are_actionable_and_silence_is_rejected() {
        let (_dir, mut engine) = fixture("exit 0");
        let program = engine.program.clone();
        engine.program = "/no-skelly-whisper".into();
        assert!(matches!(engine.validate(), Err(Error::Setup(_))));
        engine.program = program;
        engine.model = "/no-skelly-model".into();
        assert!(engine
            .validate()
            .unwrap_err()
            .to_string()
            .contains("Model path"));
        assert!(Audio {
            samples: vec![0; 16000],
            sample_rate: 16000
        }
        .validate()
        .is_err());
        assert!(Audio {
            samples: vec![1024; 10],
            sample_rate: 16000
        }
        .validate()
        .is_err());
        assert!(audio().validate().is_ok());
    }

    #[test]
    fn failed_empty_and_invalid_transcripts_are_not_delivered() {
        let (_dir, engine) = fixture("exit 9");
        assert!(matches!(
            engine.transcribe(audio(), &Control::default()),
            Err(Error::Engine(_))
        ));
        let (_dir, engine) = fixture("exit 0");
        assert!(matches!(
            engine.transcribe(audio(), &Control::default()),
            Err(Error::Invalid(_))
        ));
        for text in [
            String::new(),
            "\u{1b}[31mno".into(),
            "x".repeat(MAX_TEXT + 1),
        ] {
            assert!(validate_text(&text).is_err());
        }
    }

    #[test]
    fn deadline_exhaustion_is_an_error() {
        let (_dir, mut engine) = fixture("exec sleep 20");
        engine.timeout = Duration::ZERO;
        assert!(matches!(
            engine.transcribe(audio(), &Control::default()),
            Err(Error::Timeout)
        ));
    }

    #[test]
    fn cancellation_reaps_the_process_and_deletes_audio() {
        let (dir, engine) = fixture(
            r#"
while [ "$#" -gt 0 ]; do
  case "$1" in -f) shift; input="$1";; esac
  shift
done
printf '%s' "$input" > "$(dirname "$0")/input-path"
echo $$ > "$(dirname "$0")/pid"
exec sleep 20
"#,
        );
        let control = Control::default();
        let worker_control = control.clone();
        let worker = std::thread::spawn(move || engine.transcribe(audio(), &worker_control));
        let deadline = Instant::now() + Duration::from_secs(10);
        while !dir.path().join("pid").exists() {
            assert!(
                !worker.is_finished(),
                "fixture exited before publishing its PID: {:?}",
                worker.join().unwrap()
            );
            assert!(Instant::now() < deadline);
            std::thread::sleep(Duration::from_millis(5));
        }
        control.cancel();
        assert!(matches!(worker.join().unwrap(), Err(Error::Cancelled)));
        let input = fs::read_to_string(dir.path().join("input-path")).unwrap();
        assert!(!Path::new(&input).parent().unwrap().exists());
        let pid = fs::read_to_string(dir.path().join("pid")).unwrap();
        assert!(!Command::new("kill")
            .args(["-0", pid.trim()])
            .stderr(Stdio::null())
            .status()
            .unwrap()
            .success());
    }
}
