//! Cancellable, single-utterance dictation jobs. Workers own audio and child processes.

use std::panic::{catch_unwind, AssertUnwindSafe};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{mpsc, Arc};
use std::thread;
use std::time::Duration;

use thiserror::Error;

/// Failure in local capture or transcription. No provider credentials are involved.
#[derive(Debug, Error)]
pub enum Error {
    /// User or target lifecycle cancelled the utterance.
    #[error("Dictation cancelled")]
    Cancelled,
    /// Local executable/model configuration is not usable.
    #[error("{0}")]
    Setup(String),
    /// Device opening, permissions, format, disconnect or overflow.
    #[error("Microphone: {0}")]
    Audio(String),
    /// Local filesystem or child-process failure.
    #[error("Local dictation I/O: {0}")]
    Io(#[from] std::io::Error),
    /// Invalid recording or transcript.
    #[error("{0}")]
    Invalid(String),
    /// The local inference process failed; do not surface its potentially private output.
    #[error("whisper-cli failed ({0}); check the model and whisper.cpp >=1.8.2")]
    Engine(String),
    /// Inference exceeded the fixed safety budget.
    #[error("Local transcription timed out")]
    Timeout,
    /// A backend panicked; report completion rather than leaving capture stuck.
    #[error("Local dictation worker failed; retry after checking the audio setup")]
    Worker,
}

/// Bounded mono PCM recording at the input device's native sample rate.
pub struct Audio {
    /// Signed 16-bit mono samples.
    pub samples: Vec<i16>,
    /// Native sample rate; whisper.cpp performs its own resampling.
    pub sample_rate: u32,
}

impl Audio {
    /// Reject empty, very short, silent or oversized audio before invoking inference.
    ///
    /// # Errors
    /// Returns a user-facing recording validation error.
    pub fn validate(&self) -> Result<(), Error> {
        if !(8_000..=192_000).contains(&self.sample_rate)
            || self.samples.len() < self.sample_rate as usize / 4
            || self.samples.len() > self.sample_rate as usize * 120
        {
            return Err(Error::Invalid(
                "Recording is too short, too long or has an unsupported rate".into(),
            ));
        }
        // A conservative low-signal gate prevents the common silence-hallucination case.
        let energy = self
            .samples
            .iter()
            .map(|s| f64::from(*s).powi(2))
            .sum::<f64>();
        let count = u32::try_from(self.samples.len())
            .map_err(|_| Error::Invalid("Recording too large".into()))?;
        if energy / f64::from(count) < 32.0_f64.powi(2) {
            return Err(Error::Invalid(
                "No clear microphone signal; check the input device and permission".into(),
            ));
        }
        Ok(())
    }
}

/// Shared stop/cancel flags. Stop finishes capture; cancel discards everything.
#[derive(Clone, Default)]
pub struct Control {
    stop: Arc<AtomicBool>,
    cancel: Arc<AtomicBool>,
}

impl Control {
    /// Finish capture and transcribe the recording.
    pub fn stop(&self) {
        self.stop.store(true, Ordering::Release);
    }
    /// Discard capture and terminate outstanding local inference.
    pub fn cancel(&self) {
        self.cancel.store(true, Ordering::Release);
    }
    /// Whether the user has stopped recording.
    #[must_use]
    pub fn stopped(&self) -> bool {
        self.stop.load(Ordering::Acquire)
    }
    /// Check cancellation between potentially expensive operations.
    ///
    /// # Errors
    /// Returns the cancellation error when cancelled.
    pub fn check(&self) -> Result<(), Error> {
        if self.cancel.load(Ordering::Acquire) {
            Err(Error::Cancelled)
        } else {
            Ok(())
        }
    }
}

/// Audio input boundary; tests provide recordings without accessing a microphone.
pub trait Recorder: Send + 'static {
    /// Record until stop, cancel, duration limit or device failure.
    ///
    /// # Errors
    /// Returns capture or cancellation failures.
    fn record(&self, control: &Control, limit: Duration, ready: &dyn Fn()) -> Result<Audio, Error>;
}

/// Local transcription boundary, independent from capture, Pi and rendering.
pub trait Transcriber: Send + 'static {
    /// Validate setup *before* asking for microphone access.
    ///
    /// # Errors
    /// Returns setup errors.
    fn validate(&self) -> Result<(), Error>;
    /// Transcribe one utterance.
    ///
    /// # Errors
    /// Returns cancellation, inference or output-validation failures.
    fn transcribe(&self, audio: Audio, control: &Control) -> Result<String, Error>;
}

/// Job events: at most three records per utterance (no per-sample UI traffic).
pub enum Event {
    /// The stream is now running.
    Recording,
    /// Microphone is closed; inference is running locally.
    Transcribing,
    /// The job finished. Text is not submitted automatically by this layer.
    Finished(Result<String, Error>),
}

/// One background utterance. Dropping it cancels without blocking the UI thread.
pub struct Job {
    control: Control,
    events: mpsc::Receiver<Event>,
    worker: Option<thread::JoinHandle<()>>,
}

impl Job {
    /// Start a job using owned backend implementations.
    ///
    /// # Errors
    /// Returns thread-start or invalid duration errors.
    pub fn start(
        recorder: impl Recorder,
        transcriber: impl Transcriber,
        limit: Duration,
        wakeup: impl Fn() + Send + 'static,
    ) -> Result<Self, Error> {
        if !(Duration::from_secs(5)..=Duration::from_mins(2)).contains(&limit) {
            return Err(Error::Invalid(
                "Recording limit must be 5..=120 seconds".into(),
            ));
        }
        let control = Control::default();
        let worker_control = control.clone();
        let (sender, events) = mpsc::channel();
        let worker = thread::Builder::new()
            .name("skelly-dictation".into())
            .spawn(move || {
                let notify = |event| {
                    if sender.send(event).is_ok() {
                        wakeup();
                    }
                };
                let result = catch_unwind(AssertUnwindSafe(|| {
                    worker_control.check()?;
                    transcriber.validate()?;
                    worker_control.check()?;
                    let audio =
                        recorder.record(&worker_control, limit, &|| notify(Event::Recording))?;
                    worker_control.check()?;
                    audio.validate()?;
                    notify(Event::Transcribing);
                    transcriber.transcribe(audio, &worker_control)
                }))
                .unwrap_or(Err(Error::Worker));
                // Even a backend that completed concurrently with cancellation cannot publish text.
                notify(Event::Finished(worker_control.check().and(result)));
            })?;
        Ok(Self {
            control,
            events,
            worker: Some(worker),
        })
    }

    /// Stop recording, retaining audio for transcription.
    pub fn stop(&self) {
        self.control.stop();
    }
    /// Cancel immediately; worker cleanup is asynchronous.
    pub fn cancel(&self) {
        self.control.cancel();
    }
    /// Cancel and wait for cleanup during application shutdown only.
    pub fn shutdown(mut self) {
        self.cancel();
        if let Some(worker) = self.worker.take() {
            let _ = worker.join();
        }
    }
    /// Poll without blocking the event loop.
    #[must_use]
    pub fn poll(&self) -> Option<Event> {
        self.events.try_recv().ok()
    }
}

impl Drop for Job {
    fn drop(&mut self) {
        self.cancel();
    }
}
