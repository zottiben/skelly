//! Pane-local voice controls. Pure state, layout and hit testing; clicks never write to a PTY.

use skelly_render::{ChromeQuad, FontRole, ProseLabel, PxRect, TextMeasure, Theme};

use crate::voice::Phase;

pub(crate) const HEIGHT: f32 = 32.0;
const PAD: f32 = 6.0;
const GAP: f32 = 6.0;
const BUTTON_PAD: f32 = 9.0;

#[cfg(test)]
mod tests;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Action {
    Dictate,
    Turn,
    Cancel,
    End,
    StopPlayback,
    Conversation,
    Mute,
    CopyTranscript,
    Settings,
    More,
}

#[derive(Clone, Copy)]
pub(crate) enum Activity {
    Idle,
    Armed,
    Speaking,
    Stopping,
    Capture { phase: Phase, spoken: bool },
}

pub(crate) struct Button {
    pub(crate) label: &'static str,
    pub(crate) action: Option<Action>,
    pub(crate) hint: String,
    active: bool,
}

impl Button {
    fn new(label: &'static str, action: Action, hint: impl Into<String>) -> Self {
        Self {
            label,
            action: Some(action),
            hint: hint.into(),
            active: false,
        }
    }

    pub(crate) fn menu_label(&self) -> &'static str {
        match self.label {
            "Dictate" => "Dictate (draft only)",
            "Voice" => "Voice (record / send)",
            label => label,
        }
    }

    fn disabled(mut self, reason: &str) -> Self {
        self.action = None;
        self.hint = reason.into();
        self
    }
}

pub(crate) struct Controls {
    pub(crate) status: String,
    /// First two are the direct buttons; remaining actions live in the overflow menu.
    pub(crate) buttons: Vec<Button>,
}

impl Controls {
    pub(crate) fn new(
        availability: Result<bool, String>,
        activity: Activity,
        config: &skelly_config::Voice,
        has_transcript: bool,
    ) -> Self {
        let hint = |text: &str, shortcut: &str| {
            if shortcut.is_empty() {
                text.into()
            } else {
                format!("{text} · {shortcut}")
            }
        };
        let dictate = Button::new(
            "Dictate",
            Action::Dictate,
            hint(
                "Record into Pi's draft; never sends",
                &config.dictation_shortcut,
            ),
        );
        let turn = Button::new(
            "Voice",
            Action::Turn,
            hint(
                "Record a spoken turn; stopping sends it to Pi",
                &config.turn_shortcut,
            ),
        );
        let (dictate, turn, idle_status) = match availability {
            Ok(busy) => {
                let turn = if busy && config.busy_delivery == skelly_config::VoiceDelivery::Idle {
                    turn.disabled("Pi is busy; choose Steer or Follow up in Speech settings")
                } else {
                    turn
                };
                (
                    dictate,
                    turn,
                    if busy { "Pi busy" } else { "Pi connected" }.to_owned(),
                )
            }
            Err(reason) => (dictate.disabled(&reason), turn.disabled(&reason), reason),
        };
        let end = || Button::new("End voice", Action::End, "End local voice; Pi continues");
        let mut controls = match activity {
            Activity::Capture { phase, spoken } => Self::capture(phase, spoken),
            Activity::Speaking | Activity::Stopping => {
                Self::playback(matches!(activity, Activity::Stopping))
            }

            Activity::Armed => {
                let mut turn = turn;
                turn.label = "Talk / send";
                turn.active = true;
                Self {
                    status: if config.spoken_replies {
                        "Voice on"
                    } else {
                        "Voice muted"
                    }
                    .into(),
                    buttons: vec![turn, end(), dictate],
                }
            }
            Activity::Idle => Self {
                status: idle_status,
                buttons: vec![dictate, turn],
            },
        };
        if matches!(activity, Activity::Idle) && controls.buttons.iter().any(|b| b.action.is_some())
        {
            controls.buttons.push(Button::new(
                "Replies only",
                Action::Conversation,
                "Arm spoken replies without recording a turn",
            ));
        }
        if matches!(
            activity,
            Activity::Armed | Activity::Speaking | Activity::Stopping
        ) {
            controls.buttons.push(Button::new(
                if config.spoken_replies {
                    "Mute replies"
                } else {
                    "Unmute replies"
                },
                Action::Mute,
                "Toggle local spoken replies; Pi continues",
            ));
        }
        if has_transcript {
            controls.buttons.push(Button::new(
                "Copy transcript",
                Action::CopyTranscript,
                "Copy the last transcript; inspect Pi before resending",
            ));
        }
        controls.buttons.push(Button::new(
            "Voice settings",
            Action::Settings,
            "Open local engine and model settings",
        ));
        controls
    }

    fn capture(phase: Phase, spoken: bool) -> Self {
        let mut stop = Button::new(
            if spoken {
                "Stop & send"
            } else {
                "Stop & insert"
            },
            if spoken {
                Action::Turn
            } else {
                Action::Dictate
            },
            if spoken {
                "Transcribe and send this turn to Pi"
            } else {
                "Transcribe into the editable draft; never sends"
            },
        );
        stop.active = true;
        let status = match phase {
            Phase::Opening => {
                stop.label = "Opening…";
                stop = stop.disabled("Opening the microphone; Cancel discards this capture");
                "Opening microphone"
            }
            Phase::Recording => {
                if spoken {
                    "Recording · sends to Pi"
                } else {
                    "Recording · draft only"
                }
            }
            Phase::Transcribing => {
                stop.label = "Transcribing…";
                stop = stop.disabled("Transcribing locally; Cancel discards the result");
                "Transcribing locally"
            }
            Phase::Cancelling => {
                stop.label = "Cancelling…";
                stop = stop.disabled("Waiting for local capture cleanup");
                "Cancelling capture"
            }
        };
        let mut cancel = Button::new(
            "Cancel",
            Action::Cancel,
            "Discard capture; Pi continues · Esc",
        );
        if phase == Phase::Cancelling {
            cancel = cancel.disabled("Waiting for local capture cleanup");
        }
        Self {
            status: status.into(),
            buttons: vec![stop, cancel],
        }
    }

    fn playback(stopping: bool) -> Self {
        let mut stop = Button::new(
            "Stop audio",
            Action::StopPlayback,
            "Stop local playback, not Pi · Esc",
        );
        stop.active = true;
        if stopping {
            stop.label = "Stopping…";
            stop = stop.disabled("Waiting for local playback cleanup");
        }
        Self {
            status: "Local speech".into(),
            buttons: vec![
                stop,
                Button::new("End voice", Action::End, "End local voice; Pi continues"),
            ],
        }
    }
}

pub(crate) struct PlacedButton {
    pub(crate) rect: PxRect,
    pub(crate) button: Button,
}

pub(crate) struct Bar {
    pub(crate) rect: PxRect,
    pub(crate) buttons: Vec<PlacedButton>,
    status: String,
}

/// Reserve the footer even when the ordinary status line is hidden. A stable config-based
/// height avoids resizing the PTY on every connection/activity transition.
pub(crate) fn reserved_height(voice: bool, status: bool, scale: f32) -> f32 {
    (if voice { HEIGHT } else { 0.0 }
        + if status {
            crate::statusline::HEIGHT
        } else {
            0.0
        })
        * scale
}

impl Bar {
    pub(crate) fn layout(
        controls: Controls,
        pane: PxRect,
        status_height: f32,
        scale: f32,
        measure: &mut TextMeasure,
    ) -> Self {
        let h = (HEIGHT * scale).min((pane.h - status_height).max(0.0));
        let rect = PxRect {
            y: (pane.y + pane.h - status_height - h).max(pane.y),
            h,
            ..pane
        };
        let pad = (PAD * scale).min(pane.w.max(0.0) * 0.5).min(h * 0.5);
        let button_h = (h - 2.0 * pad).max(0.0);
        let more_w = 28.0 * scale;
        let widths: Vec<_> = controls
            .buttons
            .iter()
            .take(2)
            .map(|b| measure.width(b.label, FontRole::Caption, None) + 2.0 * BUTTON_PAD * scale)
            .collect();
        let fits = widths.iter().sum::<f32>() + 2.0 * GAP * scale + more_w + 2.0 * pad <= pane.w;
        let mut buttons = Vec::new();
        let mut x = rect.x + pad;
        if fits {
            for (button, w) in controls.buttons.into_iter().take(2).zip(widths) {
                buttons.push(PlacedButton {
                    rect: PxRect {
                        x,
                        y: rect.y + pad,
                        w,
                        h: button_h,
                    },
                    button,
                });
                x += w + GAP * scale;
            }
        }
        let label = if fits { "···" } else { "Voice…" };
        let w = if fits {
            more_w
        } else {
            (measure.width(label, FontRole::Caption, None) + 2.0 * BUTTON_PAD * scale)
                .min((pane.w - 2.0 * pad).max(0.0))
        };
        buttons.push(PlacedButton {
            rect: PxRect {
                x,
                y: rect.y + pad,
                w,
                h: button_h,
            },
            button: Button::new(
                label,
                Action::More,
                format!("Voice controls · {}", controls.status),
            ),
        });
        Self {
            rect,
            buttons,
            status: controls.status,
        }
    }

    pub(crate) fn hit(&self, point: (f32, f32)) -> Option<&Button> {
        self.buttons
            .iter()
            .find(|b| contains(b.rect, point))
            .map(|b| &b.button)
    }

    pub(crate) fn paint(
        &self,
        pointer: Option<(f32, f32)>,
        scale: f32,
        theme: &Theme,
        measure: &mut TextMeasure,
    ) -> (Vec<ChromeQuad>, Vec<ProseLabel>) {
        let mut quads = vec![
            ChromeQuad::fill(self.rect, theme.bg_inset),
            ChromeQuad::fill(
                PxRect {
                    h: scale.max(1.0),
                    ..self.rect
                },
                theme.border_subtle,
            ),
        ];
        let mut labels = Vec::new();
        let line = measure.line_height(FontRole::Caption);
        for placed in &self.buttons {
            let button = &placed.button;
            let rect = placed.rect;
            let hover = pointer.is_some_and(|p| contains(rect, p)) && button.action.is_some();
            let bg = if button.active || hover {
                theme.accent_subtle_on(theme.bg_inset)
            } else {
                theme.bg_surface
            };
            quads.push(ChromeQuad::rounded(rect, bg, 5.0 * scale));
            let text = fit_label(button.label, rect.w, measure);
            let w = measure.width(&text, FontRole::Caption, None);
            labels.push(ProseLabel {
                text,
                x: rect.x + ((rect.w - w) * 0.5).max(0.0),
                y: rect.y + (rect.h - line) * 0.5,
                role: FontRole::Caption,
                color: if button.action.is_none() {
                    theme.fg_faint
                } else if button.active || hover {
                    theme.accent
                } else {
                    theme.fg_primary
                },
                weight: None,
                max_w: rect.w,
            });
        }
        if let Some(last) = self.buttons.last() {
            let x = last.rect.x + last.rect.w + GAP * scale;
            let avail = self.rect.x + self.rect.w - PAD * scale - x;
            if avail > 0.0 {
                labels.push(ProseLabel {
                    text: fit_label(&self.status, avail, measure),
                    x,
                    y: self.rect.y + (self.rect.h - line) * 0.5,
                    role: FontRole::Caption,
                    color: theme.fg_muted,
                    weight: None,
                    max_w: avail,
                });
            }
        }
        (quads, labels)
    }
}

// Pane-overlay labels share a window-wide clip; ProseLabel::max_w is layout metadata,
// not a per-label scissor. Fit the actual string so it cannot bleed into the next pane.
fn fit_label(text: &str, width: f32, measure: &mut TextMeasure) -> String {
    if measure.width(text, FontRole::Caption, None) <= width {
        return text.into();
    }
    if measure.width("…", FontRole::Caption, None) > width {
        return String::new();
    }
    let boundaries: Vec<_> = text
        .char_indices()
        .map(|(i, _)| i)
        .chain(std::iter::once(text.len()))
        .collect();
    let (mut low, mut high) = (0, boundaries.len() - 1);
    while low < high {
        let mid = (low + high).div_ceil(2);
        let candidate = format!("{}…", &text[..boundaries[mid]]);
        if measure.width(&candidate, FontRole::Caption, None) <= width {
            low = mid;
        } else {
            high = mid - 1;
        }
    }
    format!("{}…", &text[..boundaries[low]])
}

pub(crate) fn contains(rect: PxRect, point: (f32, f32)) -> bool {
    point.0 >= rect.x && point.0 < rect.x + rect.w && point.1 >= rect.y && point.1 < rect.y + rect.h
}
