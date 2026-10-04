use shr_desk::{
    console::{Console, Envelope, Event, HeadlessRenderer, LedMailbox, LedState},
    midi::Color,
    model::{Desk, Snapshot},
};
fn console(capacity: usize) -> Console {
    let mut c = Console::new(
        Desk::new(Snapshot::fixture()),
        HeadlessRenderer::default(),
        capacity,
    );
    c.assign(true);
    c
}
fn send(c: &mut Console, event: Event) {
    c.enqueue(Envelope {
        generation: c.generation(),
        event,
    })
    .unwrap();
}
fn key(c: &mut Console, name: &str, pressed: bool) {
    send(
        c,
        Event::Key {
            key: name.into(),
            pressed,
        },
    );
}
#[test]
fn event_pump_returns_commands_without_applying_them_and_leds_coalesce() {
    let mut c = console(8);
    let old = c.desk.confirmed.clone();
    key(&mut c, "H", false);
    key(&mut c, "H", true);
    key(&mut c, "Enter", false);
    key(&mut c, "Enter", true);
    let result = c.pump();
    assert_eq!(result.commands.len(), 1);
    assert_eq!(c.desk.confirmed, old);
    assert!(result.frame.is_some());
    assert!(c.pump().frame.is_none());
    let mut mailbox = LedMailbox::default();
    mailbox.offer(LedState {
        generation: 1,
        colors: [Color::Red; 8],
    });
    mailbox.offer(LedState {
        generation: 1,
        colors: [Color::Blue; 8],
    });
    assert_eq!(mailbox.take(1).unwrap().colors, [Color::Blue; 8]);
    assert!(mailbox.take(1).is_none());
}
#[test]
fn saturation_discards_queued_edits_and_requires_fresh_release() {
    let mut c = console(2);
    key(&mut c, "H", false);
    key(&mut c, "H", true);
    c.pump();
    assert!(c.desk.draft.is_some());
    key(&mut c, "Enter", false);
    key(&mut c, "Enter", true);
    assert!(
        c.enqueue(Envelope {
            generation: c.generation(),
            event: Event::Midi(vec![0x99, 40, 100])
        })
        .is_err()
    );
    assert_eq!(c.queued(), 0);
    assert!(c.desk.draft.is_none());
    key(&mut c, "H", true);
    c.pump();
    assert!(c.desk.draft.is_none());
    key(&mut c, "H", false);
    key(&mut c, "H", true);
    c.pump();
    assert!(c.desk.draft.is_some());
}
#[test]
fn revoke_reassign_and_reconnect_never_replay_old_input() {
    let mut c = console(8);
    let old = c.generation();
    key(&mut c, "H", false);
    key(&mut c, "H", true);
    c.assign(false);
    c.assign(true);
    assert!(
        c.enqueue(Envelope {
            generation: old,
            event: Event::Midi(vec![0x99, 40, 100])
        })
        .is_err()
    );
    assert!(c.pump().commands.is_empty());
    assert!(c.desk.draft.is_none());
    c.disconnect();
    c.reconnect(Snapshot::fixture());
    send(&mut c, Event::Midi(vec![0x99, 40, 100]));
    c.pump();
    assert!(c.desk.draft.is_none());
    send(&mut c, Event::Midi(vec![0x89, 40, 0]));
    send(&mut c, Event::Midi(vec![0x99, 40, 100]));
    c.pump();
    assert!(c.desk.draft.is_some());
    c.desk.draft = None;
    send(&mut c, Event::Midi(vec![0xb0, 16, 0]));
    c.pump();
    assert!(c.desk.pending().is_none());
}
#[test]
fn lifecycle_loss_cancels_gestures_preserves_state_and_restores_scaled_frame() {
    let mut c = console(8);
    let old = c.desk.confirmed.clone();
    key(&mut c, "H", false);
    key(&mut c, "H", true);
    c.pump();
    send(&mut c, Event::DeviceLost);
    assert!(c.pump().frame.is_none());
    assert!(c.desk.draft.is_none());
    assert_eq!(c.desk.confirmed, old);
    send(&mut c, Event::DeviceRestored);
    send(
        &mut c,
        Event::Resize {
            width: 960,
            height: 540,
        },
    );
    assert!(
        c.pump()
            .frame
            .unwrap()
            .contains("width=\"960\" height=\"540\"")
    );
    send(
        &mut c,
        Event::Resize {
            width: 0,
            height: 0,
        },
    );
    assert!(c.pump().frame.is_none());
}
#[test]
fn text_and_focus_events_share_the_input_dispatch_gate() {
    let mut c = console(8);
    send(&mut c, Event::TextFocus(true));
    key(&mut c, "H", false);
    key(&mut c, "H", true);
    c.pump();
    assert!(c.desk.draft.is_none());
    send(&mut c, Event::Focus(false));
    c.pump();
    send(&mut c, Event::Focus(true));
    key(&mut c, "H", true);
    c.pump();
    assert!(c.desk.draft.is_none());
    key(&mut c, "H", false);
    key(&mut c, "H", true);
    c.pump();
    assert!(c.desk.draft.is_some());
}

#[test]
fn text_and_keyboard_dispatch_identical_commands() {
    let mut a = console(8);
    let mut b = console(8);
    send(&mut a, Event::Text("hold".into()));
    send(&mut a, Event::Text("confirm".into()));
    key(&mut b, "H", false);
    key(&mut b, "H", true);
    key(&mut b, "Enter", false);
    key(&mut b, "Enter", true);
    assert_eq!(a.pump().commands, b.pump().commands);
}

#[test]
fn same_batch_loss_discards_edits_but_retains_recovery() {
    for device in [false, true] {
        let mut c = console(16);
        let old = c.desk.confirmed.clone();
        send(
            &mut c,
            if device {
                Event::DeviceLost
            } else {
                Event::Focus(false)
            },
        );
        send(&mut c, Event::Text("hold".into()));
        send(&mut c, Event::Text("confirm".into()));
        send(
            &mut c,
            if device {
                Event::DeviceRestored
            } else {
                Event::Focus(true)
            },
        );
        let result = c.pump();
        assert!(result.frame.is_some());
        assert!(result.commands.is_empty());
        assert!(c.desk.draft.is_none());
        assert_eq!(c.desk.confirmed, old);
        key(&mut c, "H", false);
        key(&mut c, "H", true);
        c.pump();
        assert!(c.desk.draft.is_some());
    }
}

#[test]
fn text_focus_transition_cancels_release_resets_pickup_and_allows_new_text() {
    let mut c = console(16);
    // Arm K01 at the confirmed gain, then open a protected release draft.
    let value = ((c.desk.confirmed.channels[0].gain_mdb + 90000) * 127 / 102000) as u8;
    send(&mut c, Event::Midi(vec![0xb0, 16, value]));
    let command = c.pump().commands.pop().unwrap();
    let mut sim = shr_desk::model::Simulator::default();
    let ack = sim.apply(&command);
    c.desk.resolve(&ack, sim.state.clone());
    send(&mut c, Event::Text("release".into()));
    c.pump();
    assert!(c.desk.draft.is_some());
    send(&mut c, Event::TextFocus(true));
    c.pump();
    assert!(c.desk.draft.is_none());
    send(&mut c, Event::Midi(vec![0xb0, 16, 0]));
    assert!(c.pump().commands.is_empty());
    // A new intended editor can type through the shared semantic text path.
    send(&mut c, Event::Text("hold".into()));
    send(&mut c, Event::Text("confirm".into()));
    assert_eq!(c.pump().commands.len(), 1);
}
