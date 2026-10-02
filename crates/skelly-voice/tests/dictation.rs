//! Worker tests never open a microphone or call a speech/provider service.

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{mpsc, Arc};
use std::time::{Duration, Instant};

use skelly_voice::dictation::{Audio, Control, Error, Event, Job, Recorder, Transcriber};

struct FixtureInput(Arc<AtomicBool>);
impl Recorder for FixtureInput {
    fn record(&self, control: &Control, _: Duration, ready: &dyn Fn()) -> Result<Audio, Error> {
        self.0.store(true, Ordering::Release);
        ready();
        while !control.stopped() {
            control.check()?;
            std::thread::sleep(Duration::from_millis(2));
        }
        Ok(Audio {
            samples: vec![1000; 24000],
            sample_rate: 48000,
        })
    }
}

struct FixtureEngine {
    configured: bool,
    invoked: Arc<AtomicBool>,
}
impl Transcriber for FixtureEngine {
    fn validate(&self) -> Result<(), Error> {
        if self.configured {
            Ok(())
        } else {
            Err(Error::Setup("install a local model".into()))
        }
    }
    fn transcribe(&self, audio: Audio, control: &Control) -> Result<String, Error> {
        control.check()?;
        audio.validate()?;
        self.invoked.store(true, Ordering::Release);
        Ok("editable draft, never Enter".into())
    }
}

fn event(job: &Job) -> Event {
    let deadline = Instant::now() + Duration::from_secs(5);
    loop {
        if let Some(event) = job.poll() {
            return event;
        }
        assert!(
            Instant::now() < deadline,
            "worker failed to publish an event"
        );
        std::thread::sleep(Duration::from_millis(2));
    }
}

#[test]
fn setup_is_checked_before_microphone_access() {
    let opened = Arc::new(AtomicBool::new(false));
    let invoked = Arc::new(AtomicBool::new(false));
    let job = Job::start(
        FixtureInput(Arc::clone(&opened)),
        FixtureEngine {
            configured: false,
            invoked: Arc::clone(&invoked),
        },
        Duration::from_secs(5),
        || {},
    )
    .unwrap();
    assert!(matches!(event(&job), Event::Finished(Err(Error::Setup(_)))));
    assert!(!opened.load(Ordering::Acquire));
    assert!(!invoked.load(Ordering::Acquire));
}

#[test]
fn stop_transcribes_but_cancel_never_delivers_a_draft() {
    for cancel in [false, true] {
        let invoked = Arc::new(AtomicBool::new(false));
        let job = Job::start(
            FixtureInput(Arc::new(AtomicBool::new(false))),
            FixtureEngine {
                configured: true,
                invoked: Arc::clone(&invoked),
            },
            Duration::from_secs(5),
            || {},
        )
        .unwrap();
        assert!(matches!(event(&job), Event::Recording));
        if cancel {
            job.cancel();
            assert!(matches!(
                event(&job),
                Event::Finished(Err(Error::Cancelled))
            ));
        } else {
            job.stop();
            assert!(matches!(event(&job), Event::Transcribing));
            match event(&job) {
                Event::Finished(Ok(text)) => assert_eq!(text, "editable draft, never Enter"),
                _ => panic!("expected transcript after stop"),
            }
        }
        assert_eq!(invoked.load(Ordering::Acquire), !cancel);
        assert!(job.poll().is_none());
    }
}

struct LateEngine(mpsc::Receiver<()>);
impl Transcriber for LateEngine {
    fn validate(&self) -> Result<(), Error> {
        Ok(())
    }
    fn transcribe(&self, _: Audio, _: &Control) -> Result<String, Error> {
        self.0.recv_timeout(Duration::from_secs(5)).unwrap();
        Ok("late output from an uncooperative backend".into())
    }
}

#[test]
fn cancellation_wins_over_a_concurrent_transcript() {
    let (release, wait) = mpsc::channel();
    let job = Job::start(
        FixtureInput(Arc::new(AtomicBool::new(false))),
        LateEngine(wait),
        Duration::from_secs(5),
        || {},
    )
    .unwrap();
    assert!(matches!(event(&job), Event::Recording));
    job.stop();
    assert!(matches!(event(&job), Event::Transcribing));
    job.cancel();
    release.send(()).unwrap();
    assert!(matches!(
        event(&job),
        Event::Finished(Err(Error::Cancelled))
    ));
}

#[test]
fn device_denial_or_loss_never_reaches_inference() {
    struct FailedInput(bool);
    impl Recorder for FailedInput {
        fn record(&self, _: &Control, _: Duration, ready: &dyn Fn()) -> Result<Audio, Error> {
            if self.0 {
                ready();
            }
            Err(Error::Audio(
                "fixture permission denial or device loss".into(),
            ))
        }
    }
    for after_open in [false, true] {
        let invoked = Arc::new(AtomicBool::new(false));
        let job = Job::start(
            FailedInput(after_open),
            FixtureEngine {
                configured: true,
                invoked: Arc::clone(&invoked),
            },
            Duration::from_secs(5),
            || {},
        )
        .unwrap();
        if after_open {
            assert!(matches!(event(&job), Event::Recording));
        }
        assert!(matches!(event(&job), Event::Finished(Err(Error::Audio(_)))));
        assert!(!invoked.load(Ordering::Acquire));
        assert!(job.poll().is_none());
        job.shutdown();
    }
}

struct PanickingEngine;
impl Transcriber for PanickingEngine {
    fn validate(&self) -> Result<(), Error> {
        panic!("fixture backend failure");
    }
    fn transcribe(&self, _: Audio, _: &Control) -> Result<String, Error> {
        unreachable!("validation failed");
    }
}

#[test]
fn backend_panic_finishes_and_wakes_instead_of_sticking_the_hud() {
    let (wake, ready) = mpsc::channel();
    let job = Job::start(
        FixtureInput(Arc::new(AtomicBool::new(false))),
        PanickingEngine,
        Duration::from_secs(5),
        move || {
            let _ = wake.send(());
        },
    )
    .unwrap();
    assert!(
        ready.recv_timeout(Duration::from_secs(2)).is_ok(),
        "panic must wake the UI"
    );
    assert!(matches!(event(&job), Event::Finished(Err(_))));
    assert!(job.poll().is_none());
    job.shutdown();
}

#[test]
fn shutdown_waits_for_capture_to_be_released() {
    let job = Job::start(
        FixtureInput(Arc::new(AtomicBool::new(false))),
        FixtureEngine {
            configured: true,
            invoked: Arc::new(AtomicBool::new(false)),
        },
        Duration::from_secs(5),
        || {},
    )
    .unwrap();
    assert!(matches!(event(&job), Event::Recording));
    let start = Instant::now();
    job.shutdown();
    assert!(start.elapsed() < Duration::from_secs(2));
}
