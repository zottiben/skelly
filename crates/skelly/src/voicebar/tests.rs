use super::*;
use crate::contextmenu::{ContextMenu, MenuAction};

fn controls(activity: Activity) -> Controls {
    Controls::new(Ok(false), activity, &skelly_config::Voice::default(), false)
}

fn pane(w: f32, scale: f32) -> PxRect {
    PxRect {
        x: 20.0 * scale,
        y: 30.0 * scale,
        w: w * scale,
        h: 300.0 * scale,
    }
}

#[test]
fn separate_buttons_explain_draft_vs_send_and_configured_shortcuts() {
    let config = skelly_config::Voice {
        dictation_shortcut: "alt+d".into(),
        turn_shortcut: String::new(),
        ..Default::default()
    };
    let controls = Controls::new(Ok(false), Activity::Idle, &config, false);
    assert_eq!(controls.buttons[0].action, Some(Action::Dictate));
    assert!(controls.buttons[0].hint.contains("never sends"));
    assert!(controls.buttons[0].hint.contains("alt+d"));
    assert_eq!(controls.buttons[1].action, Some(Action::Turn));
    assert!(controls.buttons[1].hint.contains("stopping sends"));
    assert!(!controls.buttons[1].hint.contains("ctrl+"));
}

#[test]
fn unavailable_or_modal_pi_never_offers_capture_or_replies_only() {
    for reason in [
        "No Pi bridge connected",
        "Pi is not in the foreground",
        "Pi is waiting for a modal interaction",
    ] {
        let controls = Controls::new(
            Err(reason.into()),
            Activity::Idle,
            &skelly_config::Voice::default(),
            false,
        );
        assert!(controls.buttons[..2]
            .iter()
            .all(|b| b.action.is_none() && b.hint == reason));
        assert_eq!(controls.status, reason);
        let actions: Vec<_> = controls.buttons.iter().filter_map(|b| b.action).collect();
        assert_eq!(actions, [Action::Settings]);
    }
}

#[test]
fn busy_pi_disables_send_unless_the_existing_delivery_policy_allows_it() {
    for delivery in [
        skelly_config::VoiceDelivery::Idle,
        skelly_config::VoiceDelivery::Steer,
        skelly_config::VoiceDelivery::FollowUp,
    ] {
        let config = skelly_config::Voice {
            busy_delivery: delivery,
            ..Default::default()
        };
        let controls = Controls::new(Ok(true), Activity::Idle, &config, false);
        assert_eq!(controls.buttons[0].action, Some(Action::Dictate));
        assert_eq!(
            controls.buttons[1].action,
            (delivery != skelly_config::VoiceDelivery::Idle).then_some(Action::Turn)
        );
    }
}

#[test]
fn every_capture_phase_offers_only_safe_current_actions() {
    for spoken in [false, true] {
        for phase in [
            Phase::Opening,
            Phase::Recording,
            Phase::Transcribing,
            Phase::Cancelling,
        ] {
            let controls = controls(Activity::Capture { phase, spoken });
            let action = if spoken {
                Action::Turn
            } else {
                Action::Dictate
            };
            assert_eq!(
                controls.buttons[0].action,
                (phase == Phase::Recording).then_some(action)
            );
            assert_eq!(
                controls.buttons[1].action,
                (phase != Phase::Cancelling).then_some(Action::Cancel)
            );
            if phase == Phase::Recording {
                assert_eq!(
                    controls.buttons[0].label,
                    if spoken {
                        "Stop & send"
                    } else {
                        "Stop & insert"
                    }
                );
            }
            assert!(controls
                .buttons
                .iter()
                .all(|b| b.action != Some(Action::Conversation)));
        }
    }
}

#[test]
fn playback_and_end_controls_are_local_and_wait_for_cleanup() {
    let speaking = controls(Activity::Speaking);
    assert_eq!(speaking.buttons[0].action, Some(Action::StopPlayback));
    assert_eq!(speaking.buttons[1].action, Some(Action::End));
    assert!(speaking
        .buttons
        .iter()
        .all(|b| b.action != Some(Action::Turn)));
    let stopping = controls(Activity::Stopping);
    assert!(stopping.buttons[0].action.is_none());
    assert_eq!(stopping.buttons[1].action, Some(Action::End));
    let config = skelly_config::Voice {
        spoken_replies: false,
        ..Default::default()
    };
    let armed = Controls::new(Ok(false), Activity::Armed, &config, true);
    assert_eq!(armed.status, "Voice muted");
    assert!(armed
        .buttons
        .iter()
        .any(|b| b.label == "Unmute replies" && b.action == Some(Action::Mute)));
    assert!(armed
        .buttons
        .iter()
        .any(|b| b.action == Some(Action::CopyTranscript)));
}

#[test]
fn footer_reservation_is_independent_of_connection_and_status_visibility() {
    assert!(reserved_height(false, false, 2.0).abs() < f32::EPSILON);
    assert!((reserved_height(false, true, 2.0) - 48.0).abs() < f32::EPSILON);
    assert!((reserved_height(true, false, 2.0) - 64.0).abs() < f32::EPSILON);
    assert!((reserved_height(true, true, 2.0) - 112.0).abs() < f32::EPSILON);
    let rect = skelly_pane::Rect::new(0.0, 0.0, 800.0, 600.0);
    let bare = crate::pane_dims(rect, 10.0, 16.0, 8.0, 0.0);
    let voice = crate::pane_dims(rect, 10.0, 16.0, 8.0, reserved_height(true, false, 1.0));
    assert_eq!(voice, (bare.0, bare.1 - 2));
}

#[test]
fn paint_and_hit_share_geometry_at_both_scales_and_narrow_widths() {
    for scale in [1.0, 2.0] {
        let mut measure = TextMeasure::new(scale);
        for width in [8.0, 90.0, 180.0, 500.0] {
            for status_h in [0.0, crate::statusline::HEIGHT * scale] {
                let pane = pane(width, scale);
                let bar = Bar::layout(
                    controls(Activity::Idle),
                    pane,
                    status_h,
                    scale,
                    &mut measure,
                );
                assert!((bar.rect.y + bar.rect.h - (pane.y + pane.h - status_h)).abs() < 0.01);
                for placed in &bar.buttons {
                    assert!(placed.rect.x >= pane.x);
                    assert!(placed.rect.x + placed.rect.w <= pane.x + pane.w);
                    let point = (
                        placed.rect.x + placed.rect.w * 0.5,
                        placed.rect.y + placed.rect.h * 0.5,
                    );
                    if placed.rect.w > 0.0 {
                        assert_eq!(bar.hit(point).unwrap().action, placed.button.action);
                    }
                }
                assert!(
                    bar.hit((pane.x, pane.y)).is_none(),
                    "terminal cells cannot trigger buttons"
                );
                if width <= 90.0 {
                    assert_eq!(bar.buttons.len(), 1);
                    assert_eq!(bar.buttons[0].button.action, Some(Action::More));
                } else if width >= 500.0 {
                    assert_eq!(bar.buttons.len(), 3);
                }
                for name in ["ossein-dark", "ossein-light"] {
                    let theme = Theme::resolve(name);
                    let (quads, labels) = bar.paint(None, scale, &theme, &mut measure);
                    assert_eq!(quads[0].color, theme.bg_inset);
                    for label in labels {
                        assert!(
                            label.x >= pane.x
                                && label.x + measure.width(&label.text, label.role, None)
                                    <= pane.x + pane.w + 0.01
                        );
                    }
                }
            }
        }
    }
}

#[test]
fn narrow_menu_contains_actions_and_rejects_stale_stop_semantics() {
    let pane = skelly_pane::PaneTree::new().focused();
    let menu = ContextMenu::for_voice((0.0, 0.0), pane, None, controls(Activity::Idle));
    assert_eq!(
        menu.selected_action(),
        Some(MenuAction::Voice(Action::Dictate))
    );
    assert!(menu.voice_action_is_current(Action::Turn, &controls(Activity::Idle)));
    assert!(menu.voice_action_is_current(Action::Conversation, &controls(Activity::Idle)));
    assert!(!menu.voice_action_is_current(Action::Cancel, &controls(Activity::Idle)));
    let recording = Activity::Capture {
        phase: Phase::Recording,
        spoken: true,
    };
    let menu = ContextMenu::for_voice((0.0, 0.0), pane, None, controls(recording));
    assert!(menu.voice_action_is_current(Action::Turn, &controls(recording)));
    assert!(
        !menu.voice_action_is_current(Action::Turn, &controls(Activity::Idle)),
        "auto-stop cannot turn an old Stop menu item into a new recording"
    );
    assert!(!menu.voice_action_is_current(
        Action::Turn,
        &controls(Activity::Capture {
            phase: Phase::Transcribing,
            spoken: true
        })
    ));
}

#[test]
fn overflow_menu_is_mouse_operable_and_omits_unavailable_actions() {
    let pane = skelly_pane::PaneTree::new().focused();
    let mut menu = ContextMenu::for_voice(
        (180.0, 100.0),
        pane,
        None,
        Controls::new(
            Err("Connect Pi first".into()),
            Activity::Idle,
            &skelly_config::Voice::default(),
            false,
        ),
    );
    let mut measure = TextMeasure::new(1.0);
    let size = menu.natural_size(1.0, &mut measure);
    let panel = menu.place(size, (300.0, 160.0));
    let hit = menu
        .hit(panel, 1.0, panel.x + 10.0, panel.y + 16.0)
        .unwrap();
    assert_eq!(
        menu.action_at(hit),
        Some(MenuAction::Voice(Action::Settings))
    );
    assert!(panel.x + panel.w <= 300.0 && panel.y + panel.h <= 160.0);
    assert_eq!(menu.voice_context().unwrap().0, pane);
}

const CAPTURE_FIXTURES: [(&str, Activity, bool); 8] = [
    ("Connected Pi", Activity::Idle, false),
    ("Not connected", Activity::Idle, true),
    (
        "Editable dictation",
        Activity::Capture {
            phase: Phase::Recording,
            spoken: false,
        },
        false,
    ),
    (
        "Spoken turn",
        Activity::Capture {
            phase: Phase::Recording,
            spoken: true,
        },
        false,
    ),
    (
        "Local transcription",
        Activity::Capture {
            phase: Phase::Transcribing,
            spoken: false,
        },
        false,
    ),
    ("Voice conversation", Activity::Armed, false),
    ("Reply playback", Activity::Speaking, false),
    ("Narrow split", Activity::Idle, false),
];

#[test]
#[ignore = "manual GPU capture: writes /tmp/skelly-voice-controls-<theme>.png"]
fn capture_voice_controls() {
    for name in ["ossein-dark", "ossein-light"] {
        let appearance = skelly_config::Appearance {
            theme: name.into(),
            ..Default::default()
        };
        let theme = Theme::resolve(name);
        let mut measure = TextMeasure::new(1.0);
        let mut chrome = skelly_render::Chrome::default();
        let mut panes = Vec::new();
        for (i, (title, activity, unavailable)) in CAPTURE_FIXTURES.iter().enumerate() {
            let pane = PxRect {
                x: if i % 2 == 0 { 10.0 } else { 560.0 },
                y: 10.0 + f32::from(u16::try_from(i / 2).unwrap()) * 210.0,
                w: if i == 7 { 120.0 } else { 530.0 },
                h: 190.0,
            };
            let available = if *unavailable {
                Err("No Pi bridge connected in this pane".into())
            } else {
                Ok(false)
            };
            let bar = Bar::layout(
                Controls::new(
                    available,
                    *activity,
                    &skelly_config::Voice::default(),
                    false,
                ),
                pane,
                crate::statusline::HEIGHT,
                1.0,
                &mut measure,
            );
            let (q, l) = bar.paint(None, 1.0, &theme, &mut measure);
            chrome.pane_overlay.quads.extend(q);
            chrome.pane_overlay.labels.extend(l);
            let (q, l) = crate::statusline::paint(
                &crate::statusline::Info {
                    cwd: "~/skelly",
                    branch: Some("main"),
                    dirty: None,
                    mode: None,
                    filetype: None,
                    shell: "zsh",
                    cursor: (0, 0),
                },
                pane,
                1.0,
                &theme,
                &mut measure,
            );
            chrome.pane_overlay.quads.extend(q);
            chrome.pane_overlay.labels.extend(l);
            chrome.pane_overlay.labels.push(ProseLabel {
                text: if i == 7 {
                    (*title).into()
                } else {
                    format!("{title} · UI fixture")
                },
                x: pane.x + 14.0,
                y: pane.y + 18.0,
                role: FontRole::Body,
                color: theme.fg_secondary,
                weight: None,
                max_w: pane.w - 28.0,
            });
            panes.push(skelly_render::CapturePane {
                rect: pane,
                origin: (pane.x + 14.0, pane.y + 44.0),
                rows: vec![],
                cursor: (0, 0),
                cursor_shape: skelly_render::CursorShape::Hidden,
                focused: i == 0,
                logo: None,
            });
        }
        let rgba = skelly_render::capture_panes_rgba(&appearance, 1100, 850, 1.0, &panes, &chrome);
        let file = std::fs::File::create(format!("/tmp/skelly-voice-controls-{name}.png")).unwrap();
        let mut encoder = png::Encoder::new(std::io::BufWriter::new(file), 1100, 850);
        encoder.set_color(png::ColorType::Rgba);
        encoder.set_depth(png::BitDepth::Eight);
        encoder
            .write_header()
            .unwrap()
            .write_image_data(&rgba)
            .unwrap();
    }
}
