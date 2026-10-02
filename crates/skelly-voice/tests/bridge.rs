//! Transport tests use real local sockets, never audio or a model provider.

use std::fs;
use std::io::{BufRead, BufReader, Write};
use std::os::unix::fs::PermissionsExt;
use std::os::unix::net::UnixStream;
use std::thread;
use std::time::{Duration, Instant};

use serde_json::{json, Value};
use skelly_voice::bridge::{Action, Bridge, Delivery, Target, MAX_FRAME};

fn wait<T>(mut f: impl FnMut() -> Option<T>) -> T {
    let start = Instant::now();
    loop {
        if let Some(value) = f() {
            return value;
        }
        assert!(
            start.elapsed() < Duration::from_secs(3),
            "bridge condition timed out"
        );
        thread::sleep(Duration::from_millis(5));
    }
}

fn frame(stream: &mut UnixStream, value: &Value) {
    serde_json::to_writer(&mut *stream, value).unwrap();
    stream.write_all(b"\n").unwrap();
}

fn connect(bridge: &Bridge, session: &str) -> (UnixStream, Target) {
    let mut stream = UnixStream::connect(bridge.path()).unwrap();
    stream
        .set_read_timeout(Some(Duration::from_secs(2)))
        .unwrap();
    frame(
        &mut stream,
        &json!({"type":"hello","version":1,"token":bridge.token(),"session":session,"pid":123}),
    );
    let target = wait(|| bridge.snapshot().unwrap().target);
    assert_eq!(
        read_request(&stream),
        json!({"type":"welcome","version":1,"session":session})
    );
    (stream, target)
}

fn read_request(stream: &UnixStream) -> Value {
    let mut line = String::new();
    BufReader::new(stream).read_line(&mut line).unwrap();
    serde_json::from_str(&line).unwrap()
}

#[test]
fn registration_requires_a_process_identity() {
    let bridge = Bridge::new(|| {}).unwrap();
    let mut stream = UnixStream::connect(bridge.path()).unwrap();
    stream
        .set_read_timeout(Some(Duration::from_secs(2)))
        .unwrap();
    frame(
        &mut stream,
        &json!({
            "type":"hello", "version":1, "token":bridge.token(), "session":"s", "pid":0
        }),
    );
    let mut line = String::new();
    assert_eq!(BufReader::new(&stream).read_line(&mut line).unwrap(), 0);

    let mut stream = UnixStream::connect(bridge.path()).unwrap();
    frame(
        &mut stream,
        &json!({
            "type":"hello", "version":1, "token":bridge.token(), "session":"s", "pid":123
        }),
    );
    let target = wait(|| bridge.snapshot().unwrap().target);
    assert_eq!(target.session(), "s");
    assert!(target.is_foreground(Some(123)));
    assert!(!target.is_foreground(Some(456)));
    assert!(!target.is_foreground(None));
}

#[test]
fn local_dictation_reaches_the_real_pi_extension_without_a_model() {
    struct ChildGuard(std::process::Child);
    impl Drop for ChildGuard {
        fn drop(&mut self) {
            let _ = self.0.kill();
            let _ = self.0.wait();
        }
    }
    let bridge = Bridge::new(|| {}).unwrap();
    let child = std::process::Command::new("node")
        .arg(concat!(env!("CARGO_MANIFEST_DIR"), "/tests/pi-peer.mjs"))
        .env("SKELLY_VOICE_SOCKET", bridge.path())
        .env("SKELLY_VOICE_TOKEN", bridge.token())
        .stdout(std::process::Stdio::piped())
        .spawn()
        .expect("Node >=22.19 is required for the Pi bridge integration test");
    let mut child = ChildGuard(child);
    let stdout = BufReader::new(child.0.stdout.take().unwrap());
    let (sender, output) = std::sync::mpsc::sync_channel(2);
    thread::spawn(move || {
        for line in stdout.lines() {
            if sender.send(line).is_err() {
                break;
            }
        }
    });
    let target = wait(|| bridge.snapshot().unwrap().target);
    assert_eq!(target.session(), "fixture-session");
    assert!(target.is_foreground(Some(child.0.id())));
    bridge
        .send(
            &target,
            Action::Insert {
                text: dictated_fixture_text(),
            },
        )
        .unwrap();
    assert!(wait(|| bridge.take_reply().unwrap()).accepted);
    let line = output
        .recv_timeout(Duration::from_secs(2))
        .unwrap()
        .unwrap();
    assert_eq!(
        serde_json::from_str::<Value>(&line).unwrap(),
        json!({
            "type":"draft","text":"draft: 你好\nsecond line"
        })
    );
    assert!(
        output.recv_timeout(Duration::from_millis(100)).is_err(),
        "dictation must not submit"
    );
    bridge
        .send(
            &target,
            Action::Prompt {
                text: "/new is literal".into(),
                delivery: Delivery::Idle,
            },
        )
        .unwrap();
    let reply = wait(|| bridge.take_reply().unwrap());
    assert!(reply.accepted);
    assert_eq!(reply.operation, skelly_voice::bridge::Operation::Prompt);
    let line = output
        .recv_timeout(Duration::from_secs(2))
        .unwrap()
        .unwrap();
    assert_eq!(
        serde_json::from_str::<Value>(&line).unwrap(),
        json!({
            "type":"prompt","text":"/new is literal","options":{"expandPromptTemplates":false}
        })
    );
    assert_eq!(wait(|| bridge.snapshot().unwrap().answer), "Fixture reply.");
}

/// Synthetic native-rate audio traverses the real job/CLI adapter before the wire test.
fn dictated_fixture_text() -> String {
    use skelly_voice::dictation::{Audio, Control, Error, Event, Job, Recorder};
    struct Input;
    impl Recorder for Input {
        fn record(&self, _: &Control, _: Duration, ready: &dyn Fn()) -> Result<Audio, Error> {
            ready();
            Ok(Audio {
                samples: vec![1000; 22050],
                sample_rate: 44100,
            })
        }
    }
    let dir = tempfile::tempdir().unwrap();
    let executable = dir.path().join("whisper-fixture");
    fs::write(
        &executable,
        r#"#!/bin/sh
set -eu
while [ "$#" -gt 0 ]; do
  case "$1" in -of) shift; output="$1";; esac
  shift
done
printf '你好\nsecond line' > "$output.txt"
"#,
    )
    .unwrap();
    fs::set_permissions(&executable, fs::Permissions::from_mode(0o700)).unwrap();
    let model = dir.path().join("fixture-model.bin");
    fs::write(&model, b"fixture").unwrap();
    let engine = skelly_voice::whisper::Whisper::new(
        executable.to_str().unwrap(),
        model.to_str().unwrap(),
        "en",
    );
    let job = Job::start(Input, engine, Duration::from_secs(5), || {}).unwrap();
    wait(|| match job.poll() {
        Some(Event::Finished(result)) => Some(result.unwrap()),
        _ => None,
    })
}

fn settle(
    stream: &mut UnixStream,
    bridge: &Bridge,
    revision: u64,
    text: &str,
) -> skelly_voice::bridge::Snapshot {
    frame(
        stream,
        &json!({"type":"settled", "session":"s", "text":text}),
    );
    wait(|| {
        let snapshot = bridge.snapshot().unwrap();
        (snapshot.answer_revision == revision).then_some(snapshot)
    })
}

#[test]
fn conversation_reads_new_revisions_once_and_never_replays_muted_or_modal_answers() {
    use skelly_voice::conversation::Conversation;
    let bridge = Bridge::new(|| {}).unwrap();
    let (mut stream, _) = connect(&bridge, "s");
    let cached = settle(&mut stream, &bridge, 1, "same answer");
    let mut conversation = Conversation::new(&cached).unwrap();
    assert!(conversation.observe(&cached, true));
    assert!(
        conversation.take_pending().is_none(),
        "arming must not replay cached speech"
    );
    for revision in [2, 3] {
        let snapshot = settle(&mut stream, &bridge, revision, "same answer");
        assert!(conversation.observe(&snapshot, true));
        assert_eq!(conversation.take_pending().as_deref(), Some("same answer"));
        conversation.observe(&snapshot, true);
        assert!(conversation.take_pending().is_none());
    }
    let muted = settle(&mut stream, &bridge, 4, "muted");
    conversation.observe(&muted, false);
    conversation.observe(&muted, true);
    assert!(
        conversation.take_pending().is_none(),
        "unmuting is not replay"
    );
    frame(
        &mut stream,
        &json!({"type":"state","session":"s","busy":false,"blocked":true}),
    );
    let modal = settle(&mut stream, &bridge, 5, "modal");
    conversation.observe(&modal, true);
    assert!(conversation.take_pending().is_none());
    frame(
        &mut stream,
        &json!({"type":"state","session":"s","busy":false,"blocked":false}),
    );
    wait(|| (!bridge.snapshot().unwrap().blocked).then_some(()));
    conversation.observe(&bridge.snapshot().unwrap(), true);
    assert!(conversation.take_pending().is_none());
    let fresh = settle(&mut stream, &bridge, 6, "interrupted");
    conversation.observe(&fresh, true);
    conversation.silence(&fresh);
    conversation.observe(&fresh, true);
    assert!(
        conversation.take_pending().is_none(),
        "stop does not replay"
    );
    let other = Bridge::new(|| {}).unwrap();
    let (_other_stream, _) = connect(&other, "s");
    assert!(
        !conversation.observe(&other.snapshot().unwrap(), true),
        "same session in another pane is not the captured target"
    );
}

#[test]
fn latest_reply_replaces_backlog_and_stopping_playback_never_sends_abort() {
    use skelly_voice::conversation::Conversation;
    use skelly_voice::dictation::Control;
    use skelly_voice::speech::{Error, Playback, Speaker};
    struct UntilCancelled(std::sync::mpsc::SyncSender<()>);
    impl Speaker for UntilCancelled {
        fn speak(&self, _: &str, control: &Control) -> Result<(), Error> {
            self.0.send(()).unwrap();
            while control.check().is_ok() {
                thread::sleep(Duration::from_millis(2));
            }
            Err(Error::Cancelled)
        }
    }
    let bridge = Bridge::new(|| {}).unwrap();
    let (mut stream, target) = connect(&bridge, "s");
    let mut conversation = Conversation::new(&bridge.snapshot().unwrap()).unwrap();
    conversation.observe(&settle(&mut stream, &bridge, 1, "older"), true);
    conversation.observe(&settle(&mut stream, &bridge, 2, "newest"), true);
    assert_eq!(conversation.take_pending().as_deref(), Some("newest"));
    let (started, ready) = std::sync::mpsc::sync_channel(1);
    let playback = Playback::start(UntilCancelled(started), "answer".into(), || {}).unwrap();
    ready.recv_timeout(Duration::from_secs(2)).unwrap();
    playback.cancel();
    assert!(matches!(wait(|| playback.poll()), Err(Error::Cancelled)));
    stream
        .set_read_timeout(Some(Duration::from_millis(100)))
        .unwrap();
    let mut line = String::new();
    assert!(
        BufReader::new(&stream).read_line(&mut line).is_err(),
        "audio interruption must not touch Pi"
    );

    // An explicit abort remains distinct, including while Pi is showing a dialog.
    frame(
        &mut stream,
        &json!({"type":"state","session":"s","busy":true,"blocked":true}),
    );
    wait(|| bridge.snapshot().unwrap().blocked.then_some(()));
    let id = bridge.send(&target, Action::Abort).unwrap();
    assert_eq!(read_request(&stream)["type"], "abort");
    frame(
        &mut stream,
        &json!({"type":"reply","session":"s","id":id,"accepted":true,"error":null}),
    );
    assert_eq!(
        wait(|| bridge.take_reply().unwrap()).operation,
        skelly_voice::bridge::Operation::Abort
    );
}

#[test]
fn endpoint_is_private_unique_and_revoked_on_drop() {
    let first = Bridge::new(|| {}).unwrap();
    let second = Bridge::new(|| {}).unwrap();
    assert_ne!(first.path(), second.path());
    assert_ne!(first.token(), second.token());
    assert_eq!(first.token().len(), 64);
    let path = first.path().to_owned();
    let dir = path.parent().unwrap().to_owned();
    assert_eq!(
        fs::metadata(&dir).unwrap().permissions().mode() & 0o777,
        0o700
    );
    assert_eq!(
        fs::metadata(&path).unwrap().permissions().mode() & 0o777,
        0o600
    );
    drop(first);
    assert!(!path.exists(), "socket revoked synchronously");
    wait(|| (!dir.exists()).then_some(()));
}

#[test]
fn authenticates_and_inserts_literal_text_without_terminal_submission() {
    let bridge = Bridge::new(|| {}).unwrap();
    let (mut stream, target) = connect(&bridge, "session-a");
    let text = "existing context\n/new is literal\u{2028}中文";
    let id = bridge
        .send(&target, Action::Insert { text: text.into() })
        .unwrap();
    assert_eq!(
        read_request(&stream),
        json!({
            "type":"insert","id":id,"session":"session-a","text":text,
        })
    );
    frame(
        &mut stream,
        &json!({"type":"reply","session":"session-a","id":id,"accepted":true,"error":null}),
    );
    assert!(wait(|| bridge.take_reply().unwrap()).accepted);
}

#[test]
fn rejects_wrong_token_version_and_events_before_hello() {
    let bridge = Bridge::new(|| {}).unwrap();
    for message in [
        json!({"type":"hello","version":1,"token":"wrong","session":"s","pid":123}),
        json!({"type":"hello","version":2,"token":bridge.token(),"session":"s","pid":123}),
        json!({"type":"state","session":"s","busy":false,"blocked":false}),
    ] {
        let mut stream = UnixStream::connect(bridge.path()).unwrap();
        stream
            .set_read_timeout(Some(Duration::from_secs(2)))
            .unwrap();
        frame(&mut stream, &message);
        let mut line = String::new();
        assert_eq!(BufReader::new(&stream).read_line(&mut line).unwrap(), 0);
        assert!(bridge.snapshot().unwrap().target.is_none());
    }
}

#[test]
fn targets_do_not_survive_disconnect_or_cross_panes() {
    let bridge = Bridge::new(|| {}).unwrap();
    let other = Bridge::new(|| {}).unwrap();
    let (stream, old) = connect(&bridge, "same-session");
    let (_other_stream, _) = connect(&other, "same-session");
    assert!(other.send(&old, Action::Abort).is_err());
    drop(stream);
    wait(|| bridge.snapshot().unwrap().target.is_none().then_some(()));
    let (_stream, new) = connect(&bridge, "same-session");
    assert_ne!(old, new);
    assert!(bridge.send(&old, Action::Abort).is_err());
}

#[test]
fn honors_modal_and_busy_state_with_explicit_delivery() {
    let bridge = Bridge::new(|| {}).unwrap();
    let (mut stream, target) = connect(&bridge, "s");
    frame(
        &mut stream,
        &json!({"type":"state","session":"s","busy":true,"blocked":true}),
    );
    wait(|| bridge.snapshot().unwrap().blocked.then_some(()));
    assert!(bridge
        .send(&target, Action::Insert { text: "yes".into() })
        .is_err());
    frame(
        &mut stream,
        &json!({"type":"state","session":"s","busy":true,"blocked":false}),
    );
    wait(|| (!bridge.snapshot().unwrap().blocked).then_some(()));
    assert!(bridge
        .send(
            &target,
            Action::Prompt {
                text: "hello".into(),
                delivery: Delivery::Idle
            }
        )
        .is_err());
    let id = bridge
        .send(
            &target,
            Action::Prompt {
                text: "correction".into(),
                delivery: Delivery::Steer,
            },
        )
        .unwrap();
    assert_eq!(read_request(&stream)["delivery"], "steer");
    assert!(
        bridge.send(&target, Action::Abort).is_err(),
        "one in-flight request"
    );
    frame(
        &mut stream,
        &json!({"type":"reply","session":"s","id":id,"accepted":true,"error":null}),
    );
    wait(|| bridge.take_reply().unwrap());
}

#[test]
fn discards_unknown_outcomes_and_never_replays_on_reconnect() {
    let bridge = Bridge::new(|| {}).unwrap();
    let (stream, target) = connect(&bridge, "s");
    let id = bridge
        .send(
            &target,
            Action::Prompt {
                text: "do work".into(),
                delivery: Delivery::Idle,
            },
        )
        .unwrap();
    assert_eq!(read_request(&stream)["id"], id);
    drop(stream);
    let reply = wait(|| bridge.take_reply().unwrap());
    assert!(!reply.accepted);
    assert!(reply.error.unwrap().contains("not retried"));
    assert_eq!(reply.operation, skelly_voice::bridge::Operation::Prompt);
    let (stream, _) = connect(&bridge, "s");
    stream
        .set_read_timeout(Some(Duration::from_millis(80)))
        .unwrap();
    let mut line = String::new();
    assert!(BufReader::new(&stream).read_line(&mut line).is_err());
}

#[test]
fn rejects_control_sequences_and_oversized_or_empty_text() {
    let bridge = Bridge::new(|| {}).unwrap();
    let (_stream, target) = connect(&bridge, "s");
    for text in [
        "\x1b[201~rm -rf\n".into(),
        "  ".into(),
        "\u{009b}".into(),
        "a".repeat(8193),
    ] {
        assert!(bridge.send(&target, Action::Insert { text }).is_err());
    }
}

#[test]
fn bounds_partial_frames_and_rejects_wrong_session_events() {
    for oversized in [true, false] {
        let bridge = Bridge::new(|| {}).unwrap();
        let (mut stream, _) = connect(&bridge, "s");
        if oversized {
            stream.write_all(&vec![b'x'; MAX_FRAME + 1]).unwrap();
        } else {
            frame(
                &mut stream,
                &json!({"type":"settled","session":"other","text":"wrong pane"}),
            );
        }
        wait(|| bridge.snapshot().unwrap().target.is_none().then_some(()));
    }
}

#[test]
fn handles_fragmented_unicode_and_coalesced_frames() {
    let bridge = Bridge::new(|| {}).unwrap();
    let (mut stream, _) = connect(&bridge, "s");
    let data = concat!(
        "{\"type\":\"state\",\"session\":\"s\",\"busy\":true,\"blocked\":false}\n",
        "{\"type\":\"settled\",\"session\":\"s\",\"text\":\"🙂你好\u{2028}done\"}\n"
    );
    for byte in data.as_bytes().chunks(2) {
        stream.write_all(byte).unwrap();
    }
    let text = wait(|| bridge.snapshot().unwrap().answer);
    assert_eq!(text, "🙂你好\u{2028}done");
    assert!(!bridge.snapshot().unwrap().busy);
}
