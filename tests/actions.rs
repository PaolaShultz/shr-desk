use shr_desk::model::actions::{self, Action, Inputs, Intent};
use shr_desk::model::{Desk, Edit, Mode, Page, Simulator, Snapshot};

#[test]
fn keyboard_and_midi_produce_identical_protected_commands() {
    for (key, pad) in [("H", 40), ("R", 41), ("M", 39)] {
        let mut a = Desk::new(Snapshot::fixture());
        let mut b = Desk::new(Snapshot::fixture());
        let mut ka = Inputs::default();
        let mut mb = Inputs::default();
        assert!(ka.key(&mut a, key, true).unwrap().is_none());
        assert!(mb.midi(&mut b, &[0x99, pad, 100]).unwrap().is_none());
        assert_eq!(a.confirmed, b.confirmed);
        assert_eq!(a.draft, b.draft);
        let ac = ka.key(&mut a, "Enter", true).unwrap().unwrap();
        let bc = mb.midi(&mut b, &[0x99, 36, 100]).unwrap().unwrap();
        assert_eq!(ac, bc);
    }
}
#[test]
fn mode_picker_requires_choice_and_separate_confirmation() {
    let mut d = Desk::new(Snapshot::fixture());
    let mut i = Inputs::default();
    i.midi(&mut d, &[0x99, 42, 100]).unwrap();
    i.midi(&mut d, &[0x99, 38, 100]).unwrap();
    assert_eq!(d.confirmed.mode, Mode::Auto);
    assert!(matches!(
        d.draft.as_ref().unwrap().intent,
        Intent::Edit(Edit::Mode(Mode::Manual))
    ));
    let c = i.midi(&mut d, &[0x99, 36, 100]).unwrap().unwrap();
    let mut sim = Simulator::default();
    let ack = sim.apply(&c);
    assert!(d.resolve(&ack, sim.state.clone()));
    assert_eq!(d.confirmed.mode, Mode::Manual);
    assert!(d.confirmed.channels.iter().all(|c| c.held));
}
#[test]
fn context_and_authority_changes_cancel_or_refuse_detached_drafts() {
    let mut d = Desk::new(Snapshot::fixture());
    for action in [
        Action::Select(2),
        Action::Page(Page::Analysis),
        Action::Bank(1),
        Action::Cancel,
        Action::Back,
    ] {
        actions::apply(&mut d, Action::Release).unwrap();
        let old = d.confirmed.clone();
        actions::apply(&mut d, action).unwrap();
        assert!(d.draft.is_none());
        assert_eq!(d.confirmed, old);
    }
    actions::apply(&mut d, Action::Hold).unwrap();
    d.confirmed.revision += 1;
    assert_eq!(
        actions::apply(&mut d, Action::Confirm).unwrap_err(),
        "stale draft"
    );
    assert!(d.pending().is_none());
    actions::apply(&mut d, Action::Hold).unwrap();
    d.disconnect();
    assert!(d.draft.is_none());
    d.reconnect(Snapshot::fixture());
    assert!(d.draft.is_none());
}
#[test]
fn key_edges_text_focus_and_focus_loss_do_not_commit() {
    let mut d = Desk::new(Snapshot::fixture());
    let mut i = Inputs::default();
    i.key(&mut d, "+", true).unwrap();
    let draft = d.draft.clone();
    i.key(&mut d, "+", true).unwrap();
    assert_eq!(d.draft, draft);
    i.context_loss(&mut d);
    assert!(d.draft.is_none());
    i.text_focus = true;
    i.key(&mut d, "M", true).unwrap();
    assert!(d.draft.is_none());
    i.context_loss(&mut d);
    assert!(i.midi(&mut d, &[0x99, 40, 100]).unwrap().is_none());
    assert!(d.draft.is_none());
    i.midi(&mut d, &[0x89, 40, 0]).unwrap();
    i.midi(&mut d, &[0x99, 40, 100]).unwrap();
    assert!(d.draft.is_some());
}
#[test]
fn pickup_is_invalidated_even_after_round_trip_selection_and_cancel() {
    let mut d = Desk::new(Snapshot::fixture());
    let mut i = Inputs::default();
    assert!(i.midi(&mut d, &[0xb0, 16, 104]).unwrap().is_some());
    // Drop pending via reconnect; same numeric target must not retain pickup.
    d.reconnect(Snapshot::fixture());
    d.select(13);
    d.select(1);
    assert!(i.midi(&mut d, &[0xb0, 16, 0]).unwrap().is_none());
    i.midi(&mut d, &[0xb0, 16, 104]).unwrap();
    d.reconnect(Snapshot::fixture());
    actions::apply(&mut d, Action::Cancel).unwrap();
    assert!(i.midi(&mut d, &[0xb0, 16, 0]).unwrap().is_none());
}
#[test]
fn busy_and_lost_ack_never_replace_confirmed_state() {
    let mut d = Desk::new(Snapshot::fixture());
    let old = d.confirmed.clone();
    d.begin(Edit::Hold { channel: 1 }).unwrap();
    assert_eq!(actions::apply(&mut d, Action::Release).unwrap_err(), "Busy");
    d.disconnect();
    d.reconnect(old.clone());
    assert_eq!(d.confirmed, old);
    assert!(actions::apply(&mut d, Action::Confirm).is_err());
}

#[test]
fn pad_menu_banks_and_pages_match_keyboard_navigation() {
    let mut a = Desk::new(Snapshot::fixture());
    let mut b = Desk::new(Snapshot::fixture());
    let mut ka = Inputs::default();
    let mut mb = Inputs::default();
    ka.key(&mut a, "PageDown", true).unwrap();
    mb.midi(&mut b, &[0x99, 43, 100]).unwrap();
    mb.midi(&mut b, &[0x99, 37, 100]).unwrap();
    assert_eq!(a.selected, b.selected);
    assert_eq!(a.selected, 13);
    assert!(b.draft.is_none());
    assert_eq!(a.confirmed, b.confirmed);
}

#[test]
fn held_confirm_needs_release_after_focus_loss() {
    let mut d = Desk::new(Snapshot::fixture());
    let mut i = Inputs::default();
    assert!(i.key(&mut d, "Enter", true).is_err());
    i.context_loss(&mut d);
    actions::apply(&mut d, Action::Hold).unwrap();
    assert!(i.key(&mut d, "Enter", true).unwrap().is_none());
    assert!(d.pending().is_none());
    i.key(&mut d, "Enter", false).unwrap();
    assert!(i.key(&mut d, "Enter", true).unwrap().is_some());
}

#[test]
fn scalar_keyboard_and_rotary_use_the_same_command_path() {
    let mut a = Desk::new(Snapshot::fixture());
    let mut b = Desk::new(Snapshot::fixture());
    let mut input = Inputs::default();
    let value = 104;
    actions::apply(
        &mut a,
        Action::Edit(Edit::Gain {
            channel: 1,
            mdb: i32::from(value) * 102000 / 127 - 90000,
        }),
    )
    .unwrap();
    let ac = actions::apply(&mut a, Action::Confirm).unwrap().unwrap();
    let bc = input.midi(&mut b, &[0xb0, 16, value]).unwrap().unwrap();
    assert_eq!(ac, bc);
}

#[test]
fn protected_drafts_refuse_replacement_and_picker_survives_enter() {
    let mut d = Desk::new(Snapshot::fixture());
    actions::apply(&mut d, Action::Release).unwrap();
    let original = d.draft.clone();
    for action in [
        Action::Hold,
        Action::Mute,
        Action::Menu,
        Action::ModePicker,
        Action::ChooseMode(Mode::Auto),
    ] {
        assert!(actions::apply(&mut d, action).is_err());
        assert_eq!(d.draft, original);
    }
    actions::apply(&mut d, Action::Cancel).unwrap();
    actions::apply(&mut d, Action::ModePicker).unwrap();
    let original = d.draft.clone();
    assert!(actions::apply(&mut d, Action::Confirm).is_err());
    assert_eq!(d.draft, original);
}

#[test]
fn direct_reconnect_fences_every_pad_until_release() {
    let mut d = Desk::new(Snapshot::fixture());
    let mut input = Inputs::default();
    d.disconnect();
    d.reconnect(Snapshot::fixture());
    assert!(input.midi(&mut d, &[0x99, 40, 100]).unwrap().is_none());
    assert!(d.draft.is_none());
    input.midi(&mut d, &[0x89, 40, 0]).unwrap();
    input.midi(&mut d, &[0x99, 40, 100]).unwrap();
    assert!(d.draft.is_some());
}

#[test]
fn actual_keyboard_pan_matches_actual_midi_pan() {
    let mut a = Desk::new(Snapshot::fixture());
    let mut b = Desk::new(Snapshot::fixture());
    let mut ka = Inputs::default();
    let mut mb = Inputs::default();
    ka.key(&mut a, "[", true).unwrap();
    let ac = ka.key(&mut a, "Enter", true).unwrap().unwrap();
    let bc = mb.midi(&mut b, &[0xb0, 29, 63]).unwrap().unwrap();
    assert_eq!(ac, bc);
}
