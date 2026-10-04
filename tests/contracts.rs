use shr_desk::{midi::*, model::*, render};

fn send(d: &mut Desk, s: &mut Simulator, e: Edit) -> Ack {
    let c = d.begin(e).unwrap();
    let a = s.apply(&c);
    assert!(d.resolve(&a, s.state.clone()));
    a
}

#[test]
fn selection_and_octaves_never_edit_parameters() {
    let snapshot = Snapshot::fixture();
    let mut d = Desk::new(snapshot.clone());
    let mut decoder = Decoder::new(Profile::fixture()).unwrap();
    for (note, id, bank) in [
        (48, 1, 0),
        (59, 12, 0),
        (60, 13, 12),
        (71, 24, 12),
        (72, 25, 24),
        (83, 36, 24),
    ] {
        assert_eq!(decoder.decode(&[0x90, note, 100]), Some(Action::Select(id)));
        assert!(d.select(id));
        assert_eq!(d.bank_start(), bank);
        assert_eq!(d.confirmed, snapshot);
    }
    assert!(!d.select(37));
    assert_eq!(decoder.decode(&[0x90, 84, 100]), None);
    assert_eq!(decoder.decode(&[0x90, 47, 100]), None);
}

#[test]
fn human_hold_blocks_automation_and_release_is_explicit() {
    let mut s = Simulator::default();
    let mut d = Desk::new(s.state.clone());
    send(
        &mut d,
        &mut s,
        Edit::Gain {
            channel: 1,
            mdb: -3000,
        },
    );
    s.propose(1, -12000).unwrap();
    d.reconnect(s.state.clone());
    assert_eq!(d.selected().unwrap().gain_mdb, -3000);
    assert!(d.selected().unwrap().held);
    send(&mut d, &mut s, Edit::Release { channel: 1 });
    assert_eq!(d.selected().unwrap().gain_mdb, -12000);
    assert!(!d.selected().unwrap().held);
}

#[test]
fn mode_transitions_preserve_levels_and_protection_is_not_a_surface_parameter() {
    let mut s = Simulator::default();
    let mut d = Desk::new(s.state.clone());
    for mode in [Mode::Manual, Mode::Assist, Mode::Auto] {
        let gains: Vec<_> = s.state.channels.iter().map(|c| c.gain_mdb).collect();
        send(&mut d, &mut s, Edit::Mode(mode));
        s.propose(1, 12000).unwrap();
        d.reconnect(s.state.clone());
        assert_eq!(
            gains,
            s.state
                .channels
                .iter()
                .map(|c| c.gain_mdb)
                .collect::<Vec<_>>()
        );
        if mode != Mode::Auto {
            assert_eq!(
                send(&mut d, &mut s, Edit::Release { channel: 1 }).result,
                Err(Rejection::Manual)
            );
        }
    }
}

#[test]
fn pending_is_not_applied_and_capacity_is_explicit() {
    let mut s = Simulator::default();
    let mut d = Desk::new(s.state.clone());
    let old = d.confirmed.clone();
    let c = d
        .begin(Edit::Gain {
            channel: 1,
            mdb: 1000,
        })
        .unwrap();
    assert_eq!(d.confirmed, old);
    assert_eq!(d.begin(Edit::Hold { channel: 1 }), Err(Rejection::Busy));
    let a = s.apply(&c);
    assert!(d.resolve(&a, s.state.clone()));
    assert_eq!(d.selected().unwrap().gain_mdb, 1000);
}

#[test]
fn stale_duplicate_reused_and_evicted_commands_do_not_reapply() {
    let mut s = Simulator::default();
    let c = Command {
        id: 1,
        epoch: 1,
        expected_revision: 0,
        edit: Edit::Gain { channel: 1, mdb: 0 },
    };
    let a = s.apply(&c);
    assert_eq!(s.apply(&c), a);
    assert_eq!(s.state.revision, 1);
    let mut reused = c.clone();
    reused.edit = Edit::Mute {
        channel: 1,
        value: true,
    };
    assert_eq!(s.apply(&reused).result, Err(Rejection::ReusedId));
    assert_eq!(
        s.apply(&Command { id: 2, ..c.clone() }).result,
        Err(Rejection::Revision)
    );
    for id in 3..80 {
        let revision = s.state.revision;
        s.apply(&Command {
            id,
            expected_revision: revision,
            ..c.clone()
        });
    }
    assert_eq!(s.apply(&c).result, Err(Rejection::ReusedId));
    assert!(!s.state.channels[0].muted);
}

#[test]
fn reconnect_discards_pending_and_refuses_old_epoch_ack() {
    let mut s = Simulator::default();
    let mut d = Desk::new(s.state.clone());
    let c = d
        .begin(Edit::Gain {
            channel: 1,
            mdb: 12000,
        })
        .unwrap();
    let old = s.state.clone();
    d.disconnect();
    assert!(d.pending().is_none());
    assert_eq!(
        d.begin(Edit::Hold { channel: 1 }),
        Err(Rejection::Disconnected)
    );
    d.reconnect(old.clone());
    assert_eq!(d.confirmed, old);
    let ack = s.apply(&c);
    assert!(!d.resolve(&ack, s.state.clone()));
    let command = d.begin(Edit::Hold { channel: 1 }).unwrap();
    let mut snapshot = old;
    snapshot.epoch = 2;
    assert!(!d.resolve(
        &Ack {
            id: command.id,
            epoch: 2,
            result: Ok(1)
        },
        snapshot.clone()
    ));
    d.reconnect(snapshot);
    assert!(d.pending().is_none());
}

#[test]
fn invalid_commands_leave_state_unchanged() {
    let mut s = Simulator::default();
    let old = s.state.clone();
    for (i, e) in [
        Edit::Gain {
            channel: 1,
            mdb: 12001,
        },
        Edit::Gain {
            channel: 1,
            mdb: -90001,
        },
        Edit::Pan {
            channel: 1,
            value: 101,
        },
        Edit::Mute {
            channel: 900,
            value: true,
        },
    ]
    .into_iter()
    .enumerate()
    {
        assert!(
            s.apply(&Command {
                id: i as u64 + 1,
                epoch: 1,
                expected_revision: 0,
                edit: e
            })
            .result
            .is_err()
        );
        assert_eq!(s.state, old);
    }
    assert_eq!(s.propose(1, i32::MAX), Err(Rejection::Range));
    assert_eq!(s.state, old);
}

#[test]
fn pads_trigger_once_per_press_and_ignore_pressure_and_other_channels() {
    let mut d = Decoder::new(Profile::fixture()).unwrap();
    assert_eq!(d.decode(&[0x99, 36, 90]), Some(Action::Pad(0)));
    assert_eq!(d.decode(&[0x99, 36, 100]), None);
    assert_eq!(d.decode(&[0xa9, 36, 100]), None);
    assert_eq!(d.decode(&[0x99, 36, 0]), None);
    assert_eq!(d.decode(&[0x99, 36, 90]), Some(Action::Pad(0)));
    assert_eq!(d.decode(&[0x89, 36, 90]), None);
    assert_eq!(d.decode(&[0x98, 36, 90]), None);
    assert_eq!(d.decode(&[0x90, 60, 0]), None);
    assert_eq!(d.decode(&[0x90, 60, 128]), None);
    assert_eq!(d.decode(&[0x90, 60]), None);
    assert_eq!(
        d.decode(&[0xb0, 16, 42]),
        Some(Action::Rotary {
            index: 0,
            value: 42
        })
    );
    d.reset();
    assert_eq!(d.decode(&[0x99, 36, 90]), Some(Action::Pad(0)));
}

#[test]
fn ambiguous_profiles_are_rejected() {
    let mut p = Profile::fixture();
    p.pad_channel = 0;
    p.pad_notes[0] = 60;
    assert!(Decoder::new(p).is_none());
    let mut p = Profile::fixture();
    p.rotary_ccs[1] = p.rotary_ccs[0];
    assert!(!p.validate());
    let mut p = Profile::fixture();
    p.anchor_note = 127;
    assert!(!p.validate());
}

#[test]
fn absolute_pickup_rearms_after_bank_or_automation_change() {
    let mut p = Pickup::default();
    assert!(!p.accept((1, 0, 0), 64, 0));
    assert!(!p.accept((1, 0, 0), 64, 40));
    assert!(p.accept((1, 0, 0), 64, 80));
    assert!(p.accept((1, 0, 0), 64, 90));
    assert!(!p.accept((13, 0, 0), 64, 90));
    assert!(!p.accept((13, 0, 1000), 20, 30));
    assert!(p.accept((13, 0, 1000), 20, 10));
}

#[test]
fn relative_modes_and_led_palette_are_explicit() {
    assert_eq!(Relative::TwosComplement.delta(127), Some(-1));
    assert_eq!(Relative::TwosComplement.delta(64), Some(0));
    assert_eq!(Relative::BinaryOffset.delta(65), Some(1));
    assert_eq!(Relative::SignedBit.delta(65), Some(-1));
    assert_eq!(Relative::TwosComplement.delta(63), Some(8));
    assert_eq!(Relative::SignedBit.delta(128), None);
    assert_eq!(
        pad_packet(0, 0, Color::Green),
        Some([0xf0, 0, 0x20, 0x6b, 0x7f, 0x42, 2, 0, 0x10, 0x70, 4, 0xf7])
    );
    assert_eq!(pad_packet(1, 7, Color::White).unwrap()[9], 0x7f);
    assert!(pad_packet(2, 0, Color::Red).is_none());
    assert!(pad_packet(0, 8, Color::Red).is_none());
}

#[test]
fn feedback_distinguishes_pending_confirmed_and_disconnected() {
    let mut s = Simulator::default();
    let mut d = Desk::new(s.state.clone());
    assert_eq!(pad_colors(&d)[0], Color::Cyan);
    assert_eq!(pad_colors(&d)[3], Color::Blue);
    let c = d
        .begin(Edit::Mute {
            channel: 1,
            value: true,
        })
        .unwrap();
    assert_eq!(pad_colors(&d)[3], Color::Yellow);
    assert!(!d.selected().unwrap().muted);
    let a = s.apply(&c);
    assert!(d.resolve(&a, s.state.clone()));
    assert_eq!(pad_colors(&d)[3], Color::Red);
    d.disconnect();
    assert_eq!(pad_colors(&d)[3], Color::Off);
    assert_eq!(pad_colors(&d)[7], Color::Yellow);
}

#[test]
fn full_hd_pages_fit_and_use_bundled_glyphs() {
    let mut d = Desk::new(Snapshot::fixture());
    let font = render::Font::default();
    for connected in [true, false] {
        d.connected = connected;
        for page in [Page::Mix, Page::Channel, Page::Analysis] {
            d.page = page;
            for id in [1, 12, 13, 36] {
                d.select(id);
                let scene = render::scene(&d);
                assert!(scene.in_bounds(), "{page:?} {id}");
                for p in &scene.primitives {
                    if let render::Primitive::Text { value, .. } = p {
                        assert!(value.chars().all(|c| font.supports(c)), "{value}");
                    }
                }
                let svg = render::svg(&scene);
                assert!(svg.contains("1920 1080"));
                assert!(svg.contains("SIMULATION"));
            }
        }
    }
}
