//! Per-pane bridge wiring. Audio and socket work never runs in the window loop.

use std::ffi::OsStr;
use std::path::Path;

use skelly_term::Terminal;
use skelly_voice::bridge::{Action, Bridge, Delivery, Snapshot, SOCKET_ENV, TOKEN_ENV};
use skelly_voice::conversation::Conversation;
use skelly_voice::speech::{LocalSpeech, Playback};
use winit::event_loop::EventLoopProxy;

use crate::{App, ToastKind, Wakeup};

#[derive(Clone, Copy, PartialEq, Eq)]
pub(crate) enum Phase {
    Opening,
    Recording,
    Transcribing,
    Cancelling,
}

/// One utterance pinned to its original pane/connection/session, never re-targeted.
pub(crate) struct Dictation {
    pub(crate) target: skelly_voice::bridge::Target,
    pub(crate) job: skelly_voice::dictation::Job,
    pub(crate) phase: Phase,
    delivery: Option<Delivery>,
}

pub(crate) struct Speaking {
    pub(crate) job: Playback,
    cancelling: bool,
}

impl Dictation {
    pub(crate) fn start(
        config: &skelly_config::Voice,
        target: skelly_voice::bridge::Target,
        delivery: Option<Delivery>,
        proxy: EventLoopProxy<Wakeup>,
    ) -> Result<Self, skelly_voice::dictation::Error> {
        let job = skelly_voice::dictation::Job::start(
            skelly_voice::audio::Microphone,
            skelly_voice::whisper::Whisper::new(
                &config.whisper_program,
                &config.model_path,
                &config.language,
            ),
            std::time::Duration::from_secs(u64::from(config.max_recording_seconds)),
            move || {
                let _ = proxy.send_event(Wakeup::Voice);
            },
        )?;
        Ok(Self {
            target,
            job,
            phase: Phase::Opening,
            delivery,
        })
    }

    pub(crate) fn label(&self, config: &skelly_config::Voice) -> String {
        let shortcut = if self.delivery.is_some() {
            &config.turn_shortcut
        } else {
            &config.dictation_shortcut
        };
        let status = match self.phase {
            Phase::Opening => "Opening microphone locally · Esc cancels".into(),
            Phase::Recording => format!(
                "Recording · {} stops · Esc cancels",
                if shortcut.is_empty() {
                    "Voice palette"
                } else {
                    shortcut
                }
            ),
            Phase::Transcribing => "Transcribing locally · Esc cancels".into(),
            Phase::Cancelling => "Cancelling dictation…".into(),
        };
        if self.delivery.is_some() {
            format!("Voice turn (stop sends): {status}")
        } else {
            status
        }
    }
}

/// Spawn a shell and an optional private bridge as one pane incarnation.
pub(crate) fn spawn_shell(
    cols: u16,
    rows: u16,
    program: &str,
    cwd: Option<&Path>,
    enabled: bool,
    proxy: EventLoopProxy<Wakeup>,
) -> std::io::Result<(Terminal, Option<Bridge>)> {
    let bridge = if enabled {
        let wake = proxy.clone();
        match Bridge::new(move || {
            let _ = wake.send_event(Wakeup::Voice);
        }) {
            Ok(bridge) => Some(bridge),
            Err(err) => {
                tracing::warn!(%err, "could not start the pane's Pi bridge");
                None
            }
        }
    } else {
        None
    };
    // Mask inherited endpoints even when disabled: a nested Skelly must never
    // register its Pi sessions in the parent app's pane.
    let env = [
        (
            SOCKET_ENV,
            bridge
                .as_ref()
                .map_or(OsStr::new(""), |b| b.path().as_os_str()),
        ),
        (
            TOKEN_ENV,
            bridge
                .as_ref()
                .map_or(OsStr::new(""), |b| OsStr::new(b.token())),
        ),
    ];
    let term =
        Terminal::spawn_shell_in_with_env(cols, rows, Some(program), cwd, &env, move || {
            let _ = proxy.send_event(Wakeup::Shell);
        })?;
    Ok((term, bridge))
}

impl App {
    fn send_voice_action(
        &self,
        target: &skelly_voice::bridge::Target,
        action: Action,
    ) -> std::io::Result<u64> {
        let tab = self.active_tab();
        tab.voice_bridges
            .get(&tab.tree.focused())
            .ok_or_else(|| std::io::Error::other("Pi disconnected"))?
            .send(target, action)
    }

    pub(crate) fn toggle_conversation(&mut self) {
        if self.conversation.is_some() {
            self.end_conversation();
            self.show_toast("Voice conversation ended; Pi continues", ToastKind::Info);
        } else if self.dictation.is_some() {
            self.show_toast("Finish or cancel capture first", ToastKind::Info);
        } else {
            match self.voice_snapshot() {
                Ok(snapshot) => {
                    self.conversation = Conversation::new(&snapshot);
                    self.show_toast(
                        "Voice on: new Pi replies will be spoken unless muted",
                        ToastKind::Info,
                    );
                }
                Err(error) => self.show_toast(error.to_string(), ToastKind::Error),
            }
        }
        self.request_redraw();
    }

    /// Local voice ends independently from work already dispatched to Pi.
    pub(crate) fn end_conversation(&mut self) {
        if self
            .dictation
            .as_ref()
            .is_some_and(|d| d.delivery.is_some())
        {
            self.cancel_dictation("Spoken turn cancelled");
        }
        if self.conversation.take().is_some() {
            self.show_toast("Voice conversation ended; Pi continues", ToastKind::Info);
        }
        self.cancel_playback_job();
        self.request_redraw();
    }

    fn cancel_playback_job(&mut self) {
        if let Some(playback) = &mut self.playback {
            playback.job.cancel();
            playback.cancelling = true;
        }
    }

    pub(crate) fn stop_playback(&mut self) {
        if let Ok(snapshot) = self.voice_snapshot() {
            if let Some(conversation) = &mut self.conversation {
                conversation.silence(&snapshot);
            }
        }
        self.cancel_playback_job();
        self.request_redraw();
    }

    pub(crate) fn toggle_voice_mute(&mut self) {
        self.config.voice.spoken_replies = !self.config.voice.spoken_replies;
        self.stop_playback();
        self.apply_setting_change("voice.spoken_replies");
        self.show_toast(
            if self.config.voice.spoken_replies {
                "Spoken replies on (new replies only)"
            } else {
                "Spoken replies muted; Pi continues"
            },
            ToastKind::Info,
        );
    }

    /// Only this explicit command requests agent cancellation; never called by Esc/end/mute.
    pub(crate) fn abort_voice_agent(&mut self) {
        self.cancel_dictation("Capture cancelled");
        self.end_conversation();
        let result = self
            .voice_snapshot()
            .and_then(|s| {
                s.target
                    .ok_or_else(|| std::io::Error::other("Pi disconnected"))
            })
            .and_then(|target| self.send_voice_action(&target, Action::Abort));
        if let Err(error) = result {
            self.show_toast(error.to_string(), ToastKind::Error);
        }
    }

    pub(crate) fn on_voice_escape(&mut self, key: &winit::keyboard::Key) -> bool {
        if !matches!(
            key,
            winit::keyboard::Key::Named(winit::keyboard::NamedKey::Escape)
        ) {
            return false;
        }
        if self.dictation.is_some() {
            self.cancel_dictation("Capture cancelled");
        } else if self.playback.is_some() {
            self.stop_playback();
        } else if self.conversation.is_some() {
            self.end_conversation();
        } else {
            return false;
        }
        true
    }

    pub(crate) fn voice_activity_label(&self) -> Option<String> {
        if let Some(dictation) = &self.dictation {
            return Some(dictation.label(&self.config.voice));
        }
        self.playback.as_ref().map(|p| {
            if p.cancelling {
                "Stopping local speech…".into()
            } else {
                "Speaking locally · Esc stops audio (Pi continues)".into()
            }
        })
    }

    pub(crate) fn drain_speech(&mut self) {
        if let Some(result) = self.playback.as_ref().and_then(|p| p.job.poll()) {
            let cancelled = self.playback.take().is_some_and(|p| p.cancelling);
            if !cancelled {
                if let Err(error) = result {
                    self.show_toast(error.to_string(), ToastKind::Error);
                }
            }
            self.request_redraw();
        }
        if self.conversation.is_none() {
            return;
        }
        let Ok(snapshot) = self.voice_snapshot() else {
            self.end_conversation();
            return;
        };
        let permitted = self.config.voice.spoken_replies && self.dictation.is_none();
        let Some(conversation) = self.conversation.as_mut() else {
            return;
        };
        if !conversation.observe(&snapshot, permitted) {
            self.end_conversation();
            return;
        }
        if !permitted || snapshot.busy || snapshot.blocked {
            self.cancel_playback_job();
            return;
        }
        if conversation.has_pending() && self.playback.is_some() {
            self.cancel_playback_job();
            return;
        }
        if self.playback.is_none() {
            let text = self
                .conversation
                .as_mut()
                .and_then(Conversation::take_pending);
            if let Some(text) = text {
                let proxy = self.proxy.clone();
                match Playback::start(
                    LocalSpeech::system(
                        &self.config.voice.speech_voice,
                        self.config.voice.speech_rate,
                    ),
                    text,
                    move || {
                        let _ = proxy.send_event(Wakeup::Voice);
                    },
                ) {
                    Ok(job) => {
                        self.playback = Some(Speaking {
                            job,
                            cancelling: false,
                        });
                    }
                    Err(error) => self.show_toast(error.to_string(), ToastKind::Error),
                }
                self.request_redraw();
            }
        }
    }

    pub(crate) fn on_dictation_shortcut(&mut self, event: &winit::event::KeyEvent) -> bool {
        let matches = |chord: &str| {
            crate::parse_leader(chord).is_some_and(|(mods, code)| {
                mods == self.modifiers
                    && event.physical_key == winit::keyboard::PhysicalKey::Code(code)
            })
        };
        let dictation = matches(&self.config.voice.dictation_shortcut);
        let turn = matches(&self.config.voice.turn_shortcut);
        if !event.repeat {
            if dictation {
                self.toggle_dictation();
            } else if turn {
                self.toggle_spoken_turn();
            }
        }
        dictation || turn
    }

    fn voice_snapshot(&self) -> std::io::Result<Snapshot> {
        if !self.config.voice.enabled {
            return Err(std::io::Error::other(
                "Enable Voice and open a new pane first",
            ));
        }
        let tab = self.active_tab();
        let pane = tab.tree.focused();
        let bridge = tab
            .voice_bridges
            .get(&pane)
            .ok_or_else(|| std::io::Error::other("Enable Voice and open a new pane first"))?;
        let state = bridge.snapshot()?;
        let target = state
            .target
            .as_ref()
            .ok_or_else(|| std::io::Error::other("No Pi bridge connected in this pane"))?;
        if !target.is_foreground(tab.panes.get(&pane).and_then(Terminal::cwd_pid)) {
            return Err(std::io::Error::other(
                "The connected Pi is not this pane's foreground process",
            ));
        }
        Ok(state)
    }

    pub(crate) fn voice_target(&self) -> std::io::Result<skelly_voice::bridge::Target> {
        let snapshot = self.voice_snapshot()?;
        if snapshot.blocked {
            return Err(std::io::Error::other(
                "Pi is waiting for a modal interaction",
            ));
        }
        snapshot
            .target
            .ok_or_else(|| std::io::Error::other("Pi disconnected"))
    }

    pub(crate) fn toggle_dictation(&mut self) {
        self.toggle_capture(None);
    }

    pub(crate) fn toggle_spoken_turn(&mut self) {
        let delivery = match self.config.voice.busy_delivery {
            skelly_config::VoiceDelivery::Idle => Delivery::Idle,
            skelly_config::VoiceDelivery::Steer => Delivery::Steer,
            skelly_config::VoiceDelivery::FollowUp => Delivery::FollowUp,
        };
        self.toggle_capture(Some(delivery));
    }

    fn toggle_capture(&mut self, delivery: Option<Delivery>) {
        if let Some(dictation) = &mut self.dictation {
            if dictation.delivery.is_some() != delivery.is_some() {
                self.show_toast(
                    "Finish or cancel the current capture before switching input mode",
                    ToastKind::Info,
                );
                return;
            }
            match dictation.phase {
                Phase::Recording => {
                    dictation.job.stop();
                    dictation.phase = Phase::Transcribing;
                }
                Phase::Opening => {
                    dictation.job.cancel();
                    dictation.phase = Phase::Cancelling;
                }
                Phase::Transcribing | Phase::Cancelling => {}
            }
        } else {
            // Never capture our own output. Keep cancelled playback owned until reaped.
            if self.playback.is_some() {
                self.stop_playback();
                self.show_toast("Playback stopping; press again to record", ToastKind::Info);
                return;
            }
            if delivery.is_none() {
                self.end_conversation();
            }
            let snapshot = match self.voice_snapshot() {
                Ok(snapshot) => snapshot,
                Err(error) => {
                    self.show_toast(error.to_string(), ToastKind::Error);
                    return;
                }
            };
            if matches!(delivery, Some(Delivery::Idle)) && snapshot.busy {
                self.show_toast(
                    "Pi is busy; choose Steer or Follow up in Speech settings",
                    ToastKind::Error,
                );
                return;
            }
            if delivery.is_some() && self.conversation.is_none() {
                self.conversation = Conversation::new(&snapshot);
            }
            let result = self
                .voice_target()
                .map_err(|e| e.to_string())
                .and_then(|target| {
                    Dictation::start(&self.config.voice, target, delivery, self.proxy.clone())
                        .map_err(|e| e.to_string())
                });
            match result {
                Ok(dictation) => {
                    self.voice_transcript = None;
                    self.dictation = Some(dictation);
                }
                Err(error) => self.show_toast(error, ToastKind::Error),
            }
        }
        self.request_redraw();
    }

    pub(crate) fn cancel_dictation(&mut self, reason: &str) {
        if let Some(dictation) = &mut self.dictation {
            if dictation.phase != Phase::Cancelling {
                dictation.job.cancel();
                dictation.phase = Phase::Cancelling;
                self.show_toast(reason, ToastKind::Info);
            }
        }
    }

    pub(crate) fn copy_voice_transcript(&mut self) {
        let result = self
            .voice_transcript
            .as_ref()
            .ok_or("No completed transcript to copy")
            .and_then(|text| {
                self.clipboard
                    .as_mut()
                    .ok_or("Clipboard unavailable")
                    .and_then(|clipboard| {
                        clipboard
                            .set_text(text.clone())
                            .map_err(|_| "Clipboard unavailable")
                    })
            });
        match result {
            Ok(()) => self.show_toast(
                "Transcript copied; inspect Pi before resending",
                ToastKind::Success,
            ),
            Err(error) => self.show_toast(error, ToastKind::Error),
        }
    }

    pub(crate) fn drain_dictation(&mut self) {
        use skelly_voice::dictation::Event;
        self.drain_speech();
        let Some(dictation) = &self.dictation else {
            return;
        };
        if self.voice_target().as_ref().ok() != Some(&dictation.target) {
            self.cancel_dictation("Dictation cancelled: Pi target changed or became unavailable");
        }
        while let Some(event) = self.dictation.as_ref().and_then(|d| d.job.poll()) {
            let Some(dictation) = &mut self.dictation else {
                break;
            };
            let cancelled = dictation.phase == Phase::Cancelling;
            match event {
                Event::Recording if !cancelled => dictation.phase = Phase::Recording,
                Event::Transcribing if !cancelled => dictation.phase = Phase::Transcribing,
                Event::Recording | Event::Transcribing => {}
                Event::Finished(result) => {
                    let Some(dictation) = self.dictation.take() else {
                        break;
                    };
                    if !cancelled {
                        match result {
                            Ok(text) => {
                                self.voice_transcript = Some(text.clone());
                                let action = match dictation.delivery {
                                    Some(delivery) => Action::Prompt { text, delivery },
                                    None => Action::Insert { text },
                                };
                                let result = self.send_voice_action(&dictation.target, action);
                                if let Err(error) = result {
                                    self.show_toast(
                                        format!("{error}; Voice: copy last transcript to recover"),
                                        ToastKind::Error,
                                    );
                                }
                            }
                            Err(error) => self.show_toast(error.to_string(), ToastKind::Error),
                        }
                    }
                }
            }
            self.request_redraw();
        }
    }
}
