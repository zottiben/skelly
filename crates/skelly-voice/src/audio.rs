//! Native default-input capture. The realtime callback only converts into a lock-free ring.

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::time::{Duration, Instant};

use cpal::traits::{DeviceTrait, HostTrait, StreamTrait};
use cpal::{FromSample, Sample, SampleFormat, SizedSample};
use rtrb::RingBuffer;

use crate::dictation::{Audio, Control, Error, Recorder};

/// Records the OS-selected default input; device selection remains in OS settings for MVP.
pub struct Microphone;

impl Recorder for Microphone {
    fn record(&self, control: &Control, limit: Duration, ready: &dyn Fn()) -> Result<Audio, Error> {
        control.check()?;
        if limit.is_zero() || limit > Duration::from_mins(2) {
            return Err(Error::Invalid(
                "Recording duration must be positive and at most two minutes".into(),
            ));
        }
        let host = cpal::default_host();
        let device = host
            .default_input_device()
            .ok_or_else(|| Error::Audio("No default input device".into()))?;
        let config = device
            .default_input_config()
            .map_err(|error| audio_error(&error))?;
        let rate = config.sample_rate();
        if !(8_000..=192_000).contains(&rate) || !(1..=8).contains(&config.channels()) {
            return Err(Error::Audio(
                "Unsupported input rate or channel count".into(),
            ));
        }
        match config.sample_format() {
            SampleFormat::F32 => capture::<f32>(&device, config.into(), control, limit, ready),
            SampleFormat::F64 => capture::<f64>(&device, config.into(), control, limit, ready),
            SampleFormat::I16 => capture::<i16>(&device, config.into(), control, limit, ready),
            SampleFormat::I32 => capture::<i32>(&device, config.into(), control, limit, ready),
            SampleFormat::U16 => capture::<u16>(&device, config.into(), control, limit, ready),
            format => Err(Error::Audio(format!(
                "Unsupported microphone sample format {format}"
            ))),
        }
    }
}

fn audio_error(error: &cpal::Error) -> Error {
    Error::Audio(format!("{error}. Check OS microphone privacy settings."))
}

fn capture<T>(
    device: &cpal::Device,
    config: cpal::StreamConfig,
    control: &Control,
    limit: Duration,
    ready: &dyn Fn(),
) -> Result<Audio, Error>
where
    T: SizedSample,
    f32: FromSample<T>,
{
    control.check()?;
    let rate = config.sample_rate;
    let channels = config.channels;
    let (mut producer, mut consumer) = RingBuffer::new(rate as usize / 2);
    let fault = Arc::new(AtomicBool::new(false));
    let overflow = Arc::clone(&fault);
    let device_error = Arc::clone(&fault);
    let stream = device
        .build_input_stream(
            config,
            move |data: &[T], _| {
                for frame in data.chunks_exact(usize::from(channels)) {
                    let mono = frame.iter().map(|s| s.to_sample::<f32>()).sum::<f32>()
                        / f32::from(channels);
                    if !mono.is_finite()
                        || producer
                            .push(mono.clamp(-1.0, 1.0).to_sample::<i16>())
                            .is_err()
                    {
                        overflow.store(true, Ordering::Release);
                        break;
                    }
                }
            },
            move |_| {
                device_error.store(true, Ordering::Release);
            },
            Some(Duration::from_secs(5)),
        )
        .map_err(|error| audio_error(&error))?;
    control.check()?;
    stream.play().map_err(|error| audio_error(&error))?;
    ready();
    let deadline = Instant::now() + limit;
    let max_samples = usize::try_from(u128::from(rate) * limit.as_nanos() / 1_000_000_000)
        .map_err(|_| Error::Invalid("Recording duration exceeds capture capacity".into()))?;
    let mut samples = Vec::with_capacity(max_samples);
    loop {
        control.check()?;
        if fault.load(Ordering::Acquire) {
            return Err(Error::Audio(
                "Input device changed, disconnected or capture overflowed".into(),
            ));
        }
        while samples.len() < max_samples {
            match consumer.pop() {
                Ok(sample) => samples.push(sample),
                Err(_) => break,
            }
        }
        if control.stopped() || Instant::now() >= deadline || samples.len() == max_samples {
            break;
        }
        std::thread::sleep(Duration::from_millis(10));
    }
    drop(stream); // Release the microphone before inference or disk I/O.
    Ok(Audio {
        samples,
        sample_rate: rate,
    })
}
