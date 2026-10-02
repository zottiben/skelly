//! Real ALSA failure path in a child with an isolated config; never opens hardware.

#![cfg(target_os = "linux")]

use std::process::Command;
use std::time::Duration;

use skelly_voice::audio::Microphone;
use skelly_voice::dictation::{Control, Error, Recorder};

#[test]
fn unavailable_alsa_input_is_an_error_before_recording() {
    if std::env::var_os("SKELLY_TEST_ALSA_CHILD").is_some() {
        let result = Microphone.record(&Control::default(), Duration::from_secs(5), &|| {
            panic!("missing device must never report recording");
        });
        assert!(matches!(result, Err(Error::Audio(_))));
        return;
    }
    let dir = tempfile::tempdir().unwrap();
    let config = dir.path().join("alsa.conf");
    std::fs::write(&config, "pcm.!default { type hw card 9999 }\n").unwrap();
    let output = Command::new(std::env::current_exe().unwrap())
        .args([
            "--exact",
            "unavailable_alsa_input_is_an_error_before_recording",
            "--nocapture",
        ])
        .env("ALSA_CONFIG_PATH", config)
        .env("SKELLY_TEST_ALSA_CHILD", "1")
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "{}{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
}
