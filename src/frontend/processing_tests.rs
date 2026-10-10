use super::*;
pub(super) fn master_surface() -> (Frontend, Receiver<Request>) {
    let (mut f, rx) = brain_surface();
    f.scope = "pa_configuration".into();
    let mut snapshot = crate::structure::decode_snapshot(include_bytes!(
        "../../tests/fixtures/gp14/v1/structure-16.json"
    ))
    .unwrap();
    // Altered bus map is a UI test input, not a new accepted producer fixture.
    snapshot.pa_program_buses = vec![0, 1];
    snapshot.outputs_quiesced = true;
    let u = f.state.as_mut().unwrap();
    u.snapshot.as_mut().unwrap().authority.revision = snapshot.revision.clone();
    u.structural = Some(snapshot);
    u.structural_fresh = true;
    u.structural_age_ms = Some(0);
    u.received = Instant::now();
    (f, rx)
}
#[test]
fn master_eq_keyboard_edits_are_local_and_review_uses_existing_owner_transaction() {
    let (mut f, rx) = master_surface();
    f.key("F11").unwrap();
    assert!(!f.brain_page);
    let original = f.structural_draft.as_ref().unwrap().document.clone();
    assert!(f.scene().in_bounds());
    f.key("B").unwrap();
    f.key("I").unwrap();
    f.key("F3").unwrap();
    for key in ["-", "2", ".", "5", "Enter"] {
        f.key(key).unwrap();
    }
    let draft = f.structural_draft.as_ref().unwrap();
    for side in 0..2 {
        assert_eq!(
            draft.document["configuration"]["inputs"][side]["geq_db"][0],
            json!(-2.5)
        );
    }
    assert_eq!(
        draft.document["configuration"]["outputs"],
        original["configuration"]["outputs"]
    );
    assert!(rx.try_recv().is_err(), "field editing sends nothing");
    assert!(f.scene().in_bounds());
    for _ in 0..31 {
        f.key("I").unwrap();
        assert!(f.scene().in_bounds());
    }
    let body = f.structural_draft.as_ref().unwrap().body().unwrap();
    f.state.as_mut().unwrap().received = Instant::now();
    f.key("F4").unwrap();
    let request = rx.try_recv().unwrap();
    assert!(
        matches!(request.operation, Operation::ReviewStructure{kind,body:b} if kind=="pa_set" && b==body)
    );
    assert!(f.structural_draft.is_none());
}
#[test]
fn master_eq_refuses_unquiesced_changed_or_stale_context_without_losing_draft() {
    for reason in [
        "scope",
        "stale",
        "revision",
        "generation",
        "epoch",
        "unquiesced",
        "raw",
        "authority",
        "changed-readback",
        "changed-map",
        "changed-scope",
    ] {
        let (mut f, rx) = master_surface();
        if reason == "scope" {
            f.scope = "foh".into();
            assert!(f.key("F11").is_err());
            continue;
        }
        f.key("F11").unwrap();
        let original = f.structural_draft.as_ref().unwrap().document.clone();
        let u = f.state.as_mut().unwrap();
        match reason {
            "stale" => u.structural_age_ms = Some(251),
            "revision" => u.structural.as_mut().unwrap().revision = "900".into(),
            "generation" => {
                f.provider.generation.fetch_add(1, Ordering::AcqRel);
            }
            "epoch" => u.structural.as_mut().unwrap().epoch = "900".into(),
            "unquiesced" => u.structural.as_mut().unwrap().outputs_quiesced = false,
            "raw" => u.snapshot.as_mut().unwrap().authority.revision = "900".into(),
            "authority" => u.writer_lease_remaining_ms = Some(0),
            "changed-readback" => {
                let current = u.structural.as_mut().unwrap();
                let mut c: Value =
                    serde_json::from_str(current.pa_configuration_json.as_ref().unwrap()).unwrap();
                c["inputs"][0]["gain_db"] = json!(1.);
                current.pa_configuration_json = Some(c.to_string());
            }
            "changed-map" => u.structural.as_mut().unwrap().pa_program_buses = vec![1, 0],
            "changed-scope" => f.scope = "output_routes".into(),
            _ => unreachable!(),
        }
        assert!(f.key("F4").is_err(), "{reason}");
        assert_eq!(f.structural_draft.as_ref().unwrap().document, original);
        assert!(rx.try_recv().is_err());
    }
}
#[test]
fn master_eq_detached_text_survives_focus_loss_but_old_context_cannot_apply() {
    let (mut f, rx) = master_surface();
    f.key("F11").unwrap();
    f.key("F3").unwrap();
    f.key("t").unwrap();
    let mut fresh = f.state.clone().unwrap();
    f.enqueue(Event::Focus(false)).unwrap();
    assert_eq!(f.processing_entry, "t");
    assert!(f.structural_draft.as_ref().unwrap().master_eq.is_some());
    // Clear explicit text entry; neither focus recovery nor a fresh timestamp
    // restores the old generation's permission to replace PA settings.
    f.key("Esc").unwrap();
    fresh.received = Instant::now();
    f.state = Some(fresh);
    assert!(f.key("F4").is_err());
    assert!(
        !rx.try_iter()
            .any(|r| matches!(r.operation, Operation::ReviewStructure { .. }))
    );
}
#[test]
fn master_eq_open_requires_quiescence_and_reconnect_never_replays_or_rearms() {
    let (mut f, rx) = master_surface();
    f.state
        .as_mut()
        .unwrap()
        .structural
        .as_mut()
        .unwrap()
        .outputs_quiesced = false;
    assert!(f.key("F11").is_err());
    assert!(f.structural_draft.is_none());
    f.state
        .as_mut()
        .unwrap()
        .structural
        .as_mut()
        .unwrap()
        .outputs_quiesced = true;
    f.key("F11").unwrap();
    f.key("I").unwrap();
    f.action(Action::StructureText("high_shelf".into()))
        .unwrap();
    let original = f.structural_draft.as_ref().unwrap().document.clone();
    let mut fresh = f.state.clone().unwrap();
    f.key("F5").unwrap();
    assert!(matches!(
        rx.try_recv().unwrap().operation,
        Operation::Reconnect
    ));
    fresh.received = Instant::now();
    f.state = Some(fresh);
    assert_eq!(f.structural_draft.as_ref().unwrap().document, original);
    assert!(f.key("F4").is_err());
    assert!(rx.try_recv().is_err());
    let mut after_cancel = f.state.clone().unwrap();
    f.key("Esc").unwrap();
    // Cancel may send Cancel, but it cannot send PA settings or rearm.
    assert!(
        rx.try_iter()
            .all(|r| matches!(r.operation, Operation::Cancel))
    );
    after_cancel.received = Instant::now();
    after_cancel.generation = f.provider.generation();
    after_cancel.attachment_generation = f.attachment_fence.unwrap();
    f.accept_update(after_cancel);
    f.key("F11").unwrap();
    f.key("F4").unwrap();
    assert!(
        matches!(rx.try_recv().unwrap().operation, Operation::ReviewStructure { kind, .. } if kind == "pa_set")
    );
    assert!(rx.try_recv().is_err(), "PA Apply never queues rearm");
}
#[test]
fn master_eq_complete_owner_body_must_be_presented_before_confirmation() {
    let (mut f, rx) = master_surface();
    f.key("F11").unwrap();
    let body = f.structural_draft.as_ref().unwrap().body().unwrap();
    f.key("F4").unwrap();
    assert!(matches!(
        rx.try_recv().unwrap().operation,
        Operation::ReviewStructure { .. }
    ));
    // Match the existing operator's GP14 review format, including the exact
    // escaped owner JSON. Presentation must cover every character of it.
    let text = format!("pa_set {body} / revision 1 / scope pa_configuration / show test / epoch 9");
    f.state.as_mut().unwrap().review = Some((42, text.clone()));
    f.synchronize_review();
    assert!(f.review_pages() > 1);
    assert!(f.key("Enter").is_err());
    let mut recovered = String::new();
    for page in 0..f.review_pages() {
        f.state.as_mut().unwrap().received = Instant::now();
        for primitive in f.scene().primitives {
            if let Primitive::Text { y, value, .. } = primitive
                && (96..864).contains(&y)
            {
                recovered.push_str(&value);
            }
        }
        f.mark_presented();
        if page + 1 < f.review_pages() {
            assert!(f.key("Enter").is_err());
            f.key("PageDown").unwrap();
        }
    }
    assert_eq!(recovered, text);
    f.state.as_mut().unwrap().received = Instant::now();
    f.key("Enter").unwrap();
    assert!(matches!(
        rx.try_recv().unwrap().operation,
        Operation::Confirm(42)
    ));
    assert!(rx.try_recv().is_err());
}
pub(super) fn brain_surface() -> (Frontend, Receiver<Request>) {
    let mut f = surface();
    let (tx, rx) = mpsc::sync_channel(8);
    f.provider = Provider {
        tx,
        pending_edits: Arc::new(AtomicUsize::new(0)),
        latest: Arc::new(Latest::default()),
        generation: Arc::new(AtomicU64::new(1)),
        stop: Arc::new(AtomicBool::new(false)),
        child: None,
        authorization: Arc::new(AtomicBool::new(true)),
        brain_signal: Arc::new(crate::brain::HoldSignal::default()),
    };
    f.module_config.wire_version = 2;
    f.brain_enabled = true;
    f.brain_page = true;
    let b = crate::brain::decode_reply(include_bytes!(
        "../../tests/fixtures/gp15/v1/hold-final.json"
    ))
    .unwrap();
    let u = f.state.as_mut().unwrap();
    u.brain = b.snapshot;
    u.brain_fresh = true;
    u.brain_age_ms = Some(0);
    u.held_baseline_ready = true;
    u.held_transport_authenticated = true;
    u.writer_lease_remaining_ms = Some(2000);
    u.received = Instant::now();
    (f, rx)
}
#[test]
fn brain_keyboard_controller_edges_and_same_batch_keyup_do_not_resurrect() {
    let (mut f, rx) = brain_surface();
    f.enqueue(Event::Key {
        key: "t".into(),
        pressed: true,
    })
    .unwrap();
    f.enqueue(Event::Key {
        key: "t".into(),
        pressed: false,
    })
    .unwrap();
    f.pump();
    assert!(!f.ptt_pressed);
    assert!(!f.provider.brain_signal.live());
    assert!(
        rx.try_iter()
            .all(|r| !matches!(r.operation, Operation::BrainPress(_)))
    );
    f.enqueue(Event::Key {
        key: "t".into(),
        pressed: true,
    })
    .unwrap();
    f.pump();
    let first = rx
        .try_iter()
        .find_map(|r| {
            if let Operation::BrainPress(id) = r.operation {
                Some(id)
            } else {
                None
            }
        })
        .unwrap();
    assert!(f.provider.brain_signal.live_id(first));
    f.enqueue(Event::Key {
        key: "t".into(),
        pressed: true,
    })
    .unwrap();
    f.pump();
    assert!(
        rx.try_iter()
            .all(|r| !matches!(r.operation, Operation::BrainPress(_)))
    );
    f.enqueue(Event::Key {
        key: "t".into(),
        pressed: false,
    })
    .unwrap();
    assert!(!f.provider.brain_signal.live_id(first));
    f.pump();
    f.inject_controller(Action::TalkbackPress).unwrap();
    f.pump();
    let second = rx
        .try_iter()
        .find_map(|r| {
            if let Operation::BrainPress(id) = r.operation {
                Some(id)
            } else {
                None
            }
        })
        .unwrap();
    assert_ne!(first, second);
    assert!(!f.provider.brain_signal.live_id(first));
    f.inject_controller(Action::TalkbackRelease).unwrap();
    assert!(!f.provider.brain_signal.live());
    f.controller_removed();
    assert!(!f.ptt_pressed);
}
#[test]
fn brain_focus_loss_and_lost_ui_pump_close_without_waiting_for_queue() {
    let (mut f, _rx) = brain_surface();
    f.action(Action::TalkbackPress).unwrap();
    assert!(f.ptt_pressed);
    f.enqueue(Event::Focus(false)).unwrap();
    assert!(!f.provider.brain_signal.live());
    assert!(!f.ptt_pressed);
    assert!(f.action(Action::TalkbackPress).is_err());
    let (mut f, _rx) = brain_surface();
    f.action(Action::TalkbackPress).unwrap();
    std::thread::sleep(Duration::from_millis(105));
    f.pump();
    assert!(!f.provider.brain_signal.live());
}
#[test]
fn every_armed_monitor_action_pins_the_ui_device_before_queueing() {
    for action in [
        Action::BrainArm,
        Action::BrainGain(-1800),
        Action::BrainDim,
        Action::BrainMute,
    ] {
        let (mut f, rx) = brain_surface();
        let crate::brain_device::Message::Snapshot(d) = crate::brain_device::decode(
            include_bytes!("../../tests/fixtures/gp15/device-v1/snapshot-unarmed.json"),
        )
        .unwrap() else {
            panic!()
        };
        let pin = crate::audio::MonitorDevicePin::capture(Some(&d)).unwrap();
        let u = f.state.as_mut().unwrap();
        u.device = Some(d);
        u.device_fresh = false; // Worker refreshes; the queued identity must not change.
        u.brain.as_mut().unwrap().monitor_armed = true;
        f.action(action).unwrap();
        let request = rx.try_recv().unwrap();
        let Operation::ReviewBrain { body, device, .. } = request.operation else {
            panic!()
        };
        assert_eq!(body["armed"], true);
        assert_eq!(device.as_deref(), Some(&pin));
        f.state
            .as_mut()
            .unwrap()
            .device
            .as_mut()
            .unwrap()
            .observation
            .as_mut()
            .unwrap()
            .brain_epoch += 1;
        assert_ne!(
            device.as_deref(),
            Some(
                &crate::audio::MonitorDevicePin::capture(f.state.as_ref().unwrap().device.as_ref())
                    .unwrap()
            )
        );
    }
    let (mut f, rx) = brain_surface();
    assert!(f.action(Action::BrainArm).is_err());
    assert!(rx.try_recv().is_err());
}
#[test]
fn device_restart_and_map_change_retain_content_but_require_new_identity_review() {
    for change_epoch in [true, false] {
        let (mut f, rx) = brain_surface();
        let crate::brain_device::Message::Snapshot(d) = crate::brain_device::decode(
            include_bytes!("../../tests/fixtures/gp15/device-v1/snapshot-unarmed.json"),
        )
        .unwrap() else {
            panic!()
        };
        let u = f.state.as_mut().unwrap();
        u.device = Some(d);
        u.device_fresh = true;
        f.action(Action::DeviceEdit).unwrap();
        let retained_endpoint = f.device_draft.as_ref().unwrap().endpoint.clone();
        let o = f
            .state
            .as_mut()
            .unwrap()
            .device
            .as_mut()
            .unwrap()
            .observation
            .as_mut()
            .unwrap();
        if change_epoch {
            o.brain_epoch += 1;
        } else {
            o.brain_map += 1;
        }
        o.config.endpoint = "fake:replacement".into();
        assert!(
            f.action(Action::DeviceApply)
                .unwrap_err()
                .contains("context changed")
        );
        assert!(rx.try_recv().is_err());
        f.pump();
        assert_eq!(f.device_draft.as_ref().unwrap().endpoint, retained_endpoint);
        f.action(Action::DeviceEdit).unwrap();
        assert_eq!(f.device_draft.as_ref().unwrap().endpoint, retained_endpoint);
        f.action(Action::DeviceApply).unwrap();
        assert!(
            rx.try_iter()
                .any(|r| matches!(r.operation, Operation::ReviewDevice(..)))
        );
    }
}
#[test]
fn brain_actual_snapshot_scene_and_device_draft_are_truthful_and_fit() {
    let (mut f, rx) = brain_surface();
    let crate::brain_device::Message::Snapshot(mut d) = crate::brain_device::decode(
        include_bytes!("../../tests/fixtures/gp15/device-v1/snapshot-unarmed.json"),
    )
    .unwrap() else {
        panic!()
    };
    d.observation.as_mut().unwrap().status["capture_queue_dropped"] = serde_json::json!(17);
    let u = f.state.as_mut().unwrap();
    u.device = Some(d);
    u.device_fresh = true;
    u.brain_fresh = false;
    u.fresh = false;
    u.brain_age_ms = Some(7);
    u.held_status = Some(crate::held_proof::Status {
        generation: Some("1".into()),
        source_frame: "480".into(),
        revision: "9".into(),
        talkback_path_ready: true,
        media_authorized: true,
        observed: Instant::now(),
        valid_until: Instant::now() + Duration::from_millis(50),
    });
    let scene = f.scene();
    assert!(scene.primitives.iter().any(
        |p| matches!(p,Primitive::Text{value,..} if value.contains("compact true / ready true"))
    ));
    assert!(
        scene
            .primitives
            .iter()
            .any(|p| matches!(p,Primitive::Text{value,..} if value.contains("observation STALE")))
    );
    assert!(scene.primitives.iter().any(|p| matches!(p,Primitive::Text{value,..} if value.contains("Sample peaks") && value.contains("age"))));
    f.state.as_mut().unwrap().fresh = true;

    assert!(scene.primitives.iter().any(
        |p| matches!(p,Primitive::Text{value,..}if value.contains("ratio")&&value.contains("ppb"))
    ));
    assert!(
        scene.primitives.iter().any(
            |p| matches!(p, Primitive::Text { value, .. } if value.contains("capture drops 17"))
        )
    );
    f.state
        .as_mut()
        .unwrap()
        .device
        .as_mut()
        .unwrap()
        .observation
        .as_mut()
        .unwrap()
        .status
        .as_object_mut()
        .unwrap()
        .remove("capture_queue_dropped");
    assert!(
        f.scene().primitives.iter().any(
            |p| matches!(p, Primitive::Text { value, .. } if value.contains("capture drops --"))
        )
    );
    f.action(Action::DeviceEdit).unwrap();
    assert_eq!(f.device_draft.as_ref().unwrap().endpoint, "fake:brain");
    assert!(rx.try_recv().is_err());
    let text = f.device_draft.as_ref().unwrap().review().unwrap();
    f.action(Action::DeviceText(text)).unwrap();
    f.action(Action::DeviceApply).unwrap();
    assert!(
        rx.try_iter()
            .any(|r| matches!(r.operation, Operation::ReviewDevice(..)))
    );
    #[cfg(feature = "native")]
    if std::env::var_os("VK_DRIVER_FILES").is_some() {
        for (w, h) in [(1920, 1080), (960, 540), (540, 960), (3840, 2160)] {
            crate::native::offscreen_at(&scene, w, h).unwrap();
        }
    }
}
fn surface() -> Frontend {
    let mut f = Frontend::new(Config {
        wire_version: 1,
        remote: None,
        endpoint: "/nonexistent/gp07-layout.sock".into(),
        show: "11111111-1111-4111-8111-111111111111".into(),
        epoch: 9,
        writer: "gp07-layout".into(),
        scope: "foh".into(),
    });
    let corpus: Value = serde_json::from_str(include_str!(
        "../../tests/fixtures/gp03/v1/e03-rendered.json"
    ))
    .unwrap();
    let mut raw =
        crate::audio::decode_snapshot(&serde_json::to_vec(&corpus["initial"]).unwrap()).unwrap();
    raw.authority.revision = "0".into();
    let processing = crate::processing::decode_reply(include_bytes!(
        "../../tests/fixtures/gp07/v2/snapshot-reply.json"
    ))
    .unwrap()
    .snapshot;
    f.state = Some(Update {
        device: None,
        device_final: None,
        device_fresh: false,
        device_age_ms: Some(0),
        brain: None,
        brain_final: None,
        brain_fresh: false,
        brain_age_ms: None,
        held_status: None,
        held_baseline_ready: false,
        held_transport_authenticated: false,
        brain_status: "disabled".into(),
        generation: 1,
        attachment_generation: 1,
        last_operation: None,
        writer_lease_remaining_ms: None,
        snapshot: Some(raw),
        fx: Default::default(),
        fx_age_ms: None,
        measurement: Default::default(),
        live_eq: None,
        live_eq_age_ms: None,
        live_eq_final: None,
        sends: None,
        sends_age_ms: None,
        sends_final: None,
        processing,
        processing_age_ms: Some(0),
        processing_status: "fixture layout only".into(),
        structural: None,
        structural_final: None,
        structural_fresh: false,
        structural_age_ms: Some(0),
        processing_final: None,
        fresh: true,
        snapshot_age_ms: Some(0),
        status: "fixture layout only".into(),
        review: None,
        received: Instant::now(),
    });
    f.page = Page::Channel;
    f
}
fn exact_surface() -> (Frontend, Receiver<Request>) {
    let (mut f, rx) = brain_surface();
    f.brain_page = false;
    f.brain_enabled = false;
    f.page = Page::Mix;
    (f, rx)
}
#[test]
fn exact_keyboard_semantic_review_and_exclusivity() {
    use crate::exact_value::Parameter;
    for parameter in [Parameter::Fader, Parameter::Pan] {
        let (mut f, rx) = exact_surface();
        f.key(if parameter == Parameter::Fader {
            "d"
        } else {
            "c"
        })
        .unwrap();
        let text = if parameter == Parameter::Fader {
            "-6.1"
        } else {
            "+31"
        };
        for c in text.chars() {
            f.key(&c.to_string()).unwrap();
        }
        f.key("Enter").unwrap();
        assert!(rx.try_recv().is_err());
        let d = f.exact_draft.as_ref().unwrap();
        assert_eq!(d.value, Some(parameter.parse(text).unwrap()));
        assert!(f.scene().in_bounds());
        for action in [
            Action::ProcessingEdit,
            Action::SendLevelEdit,
            Action::DeviceEdit,
            Action::StructureEdit,
            Action::MasterEqEdit,
            Action::LiveEqEdit,
            Action::Mute,
            Action::Hold,
            Action::Release,
            Action::Adjust(1000),
            Action::AdjustPan(1),
            Action::ModePicker,
        ] {
            assert!(f.action(action).is_err());
        }
        assert!(f.key("Enter").is_ok()); // accepting twice never sends
        assert!(rx.try_recv().is_err());
        f.key("F4").unwrap();
        let r = rx.try_recv().unwrap();
        assert_eq!(r.revision.as_deref(), Some("0"));
        assert!(
            matches!(r.operation,Operation::ReviewSet{ref target,ref value} if target["input"]=="input-01" && target["parameter"]==parameter.name() && value==&json!(parameter.parse(text).unwrap()))
        );
        assert!(f.key("F4").is_err());
        assert!(f.action(Action::ExactText("0".into())).is_err());
        assert!(f.key("Enter").is_err()); // no displayed review
        assert!(rx.try_recv().is_err());
        let (mut semantic, sr) = exact_surface();
        semantic.action(Action::ExactEdit(parameter)).unwrap();
        semantic.action(Action::ExactText(text.into())).unwrap();
        semantic.action(Action::ExactApply).unwrap();
        assert!(
            matches!(sr.try_recv().unwrap().operation,Operation::ReviewSet{ref target,ref value} if target["parameter"]==parameter.name() && value==&json!(parameter.parse(text).unwrap()))
        );
    }
}
#[test]
fn exact_accepted_dynamic_inventory_and_controller_fence() {
    let (mut f, rx) = exact_surface();
    let u = f.state.as_mut().unwrap();
    u.snapshot = Some(
        crate::audio::decode_snapshot(include_bytes!(
            "../../tests/fixtures/gp18/v1-corrected/raw-baseline.json"
        ))
        .unwrap(),
    );
    u.received = Instant::now();
    f.selected = 16;
    f.page = Page::Channel;
    f.key("C").unwrap();
    f.action(Action::ExactText("-100".into())).unwrap();
    assert_eq!(f.exact_draft.as_ref().unwrap().input, "input-17");
    f.inject_controller(Action::ExactApply).unwrap();
    f.controller_removed();
    assert!(f.queue.is_empty());
    assert_eq!(f.exact_draft.as_ref().unwrap().value, Some(-100));
    assert!(rx.try_recv().is_err());
}
#[test]
fn exact_open_refuses_each_existing_draft_and_review() {
    for kind in 0..5 {
        let (mut f, _rx) = exact_surface();
        match kind {
            0 => {
                f.processing_draft = Some(ProcessingDraft {
                    input: "input-01".into(),
                    config: f
                        .state
                        .as_ref()
                        .unwrap()
                        .processing
                        .as_ref()
                        .unwrap()
                        .channels[0]
                        .target
                        .clone(),
                    revision: "0".into(),
                    generation: 1,
                })
            }
            1 => {
                f.send_draft = Some(SendDraft {
                    input: "input-01".into(),
                    monitor: "monitor-1".into(),
                    value: SendDraftValue::Level(0),
                    revision: "0".into(),
                    generation: 1,
                })
            }
            2 => {
                f.structural_draft = Some(
                    crate::structure::Draft::new(
                        &crate::structure::decode_snapshot(include_bytes!(
                            "../../tests/fixtures/gp14/v1/structure-16.json"
                        ))
                        .unwrap(),
                        "output_routes",
                        1,
                    )
                    .unwrap(),
                )
            }
            3 => {
                let crate::brain_device::Message::Snapshot(s) = crate::brain_device::decode(
                    include_bytes!("../../tests/fixtures/gp15/device-v1/snapshot-unarmed.json"),
                )
                .unwrap() else {
                    panic!()
                };
                f.device_draft = s.observation.map(|o| o.config);
            }
            4 => f.state.as_mut().unwrap().review = Some((1, "existing review".into())),
            _ => unreachable!(),
        }
        assert!(f.key("D").is_err());
        assert!(f.exact_draft.is_none());
    }
}
#[test]
fn exact_presented_confirm_once_and_cancel_never_claims_submitted_undo() {
    let (mut f, rx) = exact_surface();
    f.key("D").unwrap();
    f.action(Action::ExactText("-6.1".into())).unwrap();
    f.key("F4").unwrap();
    drop(rx.try_recv().unwrap());
    f.state.as_mut().unwrap().review = Some((
        19,
        "input-01 FOH fader proposed -6.1 dB / complete request".into(),
    ));
    f.synchronize_review();
    assert!(f.key("Enter").is_err());
    f.mark_presented();
    f.key("Enter").unwrap();
    assert!(f.key("Enter").is_err());
    assert!(
        f.action(Action::ExactEdit(crate::exact_value::Parameter::Fader))
            .is_err()
    );
    f.key("Esc").unwrap();
    assert!(f.message.contains("submitted mutation is NOT cancelled"));
    assert_eq!(
        rx.try_iter()
            .filter(|r| matches!(r.operation, Operation::Confirm(19)))
            .count(),
        1
    );
}
#[test]
fn exact_unshifted_editor_keyup_rearms_after_controller_fence() {
    for (key, parameter, text) in [
        ("d", crate::exact_value::Parameter::Fader, "-6.1"),
        ("c", crate::exact_value::Parameter::Pan, "-31"),
    ] {
        let (mut f, rx) = exact_surface();
        f.action(Action::ExactEdit(parameter)).unwrap();
        f.action(Action::ExactText(text.into())).unwrap();
        let update = f.state.clone().unwrap();
        f.controller_removed();
        f.state = Some(Update {
            generation: f.provider.generation(),
            received: Instant::now(),
            ..update
        });
        f.enqueue(Event::Key {
            key: key.into(),
            pressed: true,
        })
        .unwrap();
        f.pump();
        f.enqueue(Event::Key {
            key: key.into(),
            pressed: false,
        })
        .unwrap();
        f.pump();
        assert!(
            rx.try_iter()
                .any(|r| matches!(r.operation, Operation::InputReleased)),
            "unshifted {key} must release input after explicit revalidation"
        );
        assert_eq!(f.exact_draft.as_ref().unwrap().text, text);
        f.action(Action::ExactApply).unwrap();
        assert!(
            rx.try_iter()
                .any(|r| matches!(r.operation, Operation::ReviewSet { .. }))
        );
    }
}
#[test]
fn exact_queued_operation_blocks_open_and_count_retires_on_drop() {
    let (mut f, rx) = exact_surface();
    f.action(Action::Adjust(1000)).unwrap();
    assert_eq!(f.provider.pending_edits.load(Ordering::Acquire), 1);
    assert!(f.key("D").is_err());
    let request = rx.try_recv().unwrap();
    assert!(f.key("D").is_err()); // in progress, before worker PENDING publication
    drop(request);
    assert_eq!(f.provider.pending_edits.load(Ordering::Acquire), 0);
    f.key("D").unwrap();
    let (mut f, _rx) = exact_surface();
    for _ in 0..8 {
        f.provider.send(None, Operation::Grant).unwrap();
    }
    assert!(f.provider.send(None, Operation::Grant).is_err());
    assert_eq!(f.provider.pending_edits.load(Ordering::Acquire), 8); // failed enqueue retires its ticket
    assert!(f.key("C").is_err());
}
#[test]
fn exact_context_pins_and_explicit_revalidation() {
    use crate::exact_value::Parameter;
    for change in 0..9 {
        let (mut f, rx) = exact_surface();
        f.action(Action::ExactEdit(Parameter::Fader)).unwrap();
        f.action(Action::ExactText("-6.1".into())).unwrap();
        match change {
            0 => f.selected = 1,
            1 => {
                f.state
                    .as_mut()
                    .unwrap()
                    .snapshot
                    .as_mut()
                    .unwrap()
                    .authority
                    .revision = "1".into()
            }
            2 => {
                f.state
                    .as_mut()
                    .unwrap()
                    .snapshot
                    .as_mut()
                    .unwrap()
                    .authority
                    .epoch = "10".into()
            }
            3 => {
                f.state
                    .as_mut()
                    .unwrap()
                    .snapshot
                    .as_mut()
                    .unwrap()
                    .authority
                    .show_id = "22222222-2222-4222-8222-222222222222".into()
            }
            4 => f.scope = "monitor1".into(),
            5 => {
                f.provider.generation.store(2, Ordering::Release);
            }
            6 => f.focused = false,
            7 => f.state.as_mut().unwrap().writer_lease_remaining_ms = None,
            8 => {
                f.role_required = true;
                f.provider.authorization.store(false, Ordering::Release);
            }
            _ => unreachable!(),
        }
        assert!(f.action(Action::ExactApply).is_err(), "fence {change}");
        assert!(rx.try_recv().is_err());
        assert_eq!(f.exact_draft.as_ref().unwrap().text, "-6.1");
    }
    let (mut f, rx) = exact_surface();
    f.key("D").unwrap();
    f.key("-").unwrap();
    f.key("6").unwrap();
    f.key(".").unwrap();
    let update = f.state.clone().unwrap();
    f.inject_controller(Action::ExactApply).unwrap();
    f.enqueue(Event::Focus(false)).unwrap();
    assert_eq!(f.exact_draft.as_ref().unwrap().text, "-6.");
    assert!(f.queue.is_empty());
    f.enqueue(Event::Focus(true)).unwrap();
    f.state = Some(Update {
        generation: f.provider.generation(),
        received: Instant::now(),
        ..update
    });
    f.key("1").unwrap();
    f.key("Enter").unwrap();
    assert!(f.key("F4").is_err());
    f.key("D").unwrap(); // explicit revalidation, retained text
    assert_eq!(f.exact_draft.as_ref().unwrap().text, "-6.1");
    f.key("F4").unwrap();
    assert!(
        rx.try_iter()
            .any(|r| matches!(r.operation, Operation::ReviewSet { .. }))
    );
}
#[test]
fn exact_dynamic_identity_capacity_cancel_and_review_selection_fence() {
    use crate::exact_value::Parameter;
    let (mut f, rx) = exact_surface();
    let raw = f.state.as_mut().unwrap().snapshot.as_mut().unwrap();
    raw.authority.inputs[0] = "source-17".into();
    for p in &mut raw.authority.parameters {
        if p.target.input == "input-01" {
            p.target.input = "source-17".into();
        }
    }
    for c in &mut raw.coefficients {
        if c.input == "input-01" {
            c.input = "source-17".into();
        }
    }
    f.action(Action::ExactEdit(Parameter::Pan)).unwrap();
    f.key("0").unwrap();
    f.key("Enter").unwrap();
    f.key("F4").unwrap();
    assert!(
        matches!(rx.try_recv().unwrap().operation,Operation::ReviewSet{ref target,..} if target["input"]=="source-17")
    );
    f.state.as_mut().unwrap().review =
        Some((7, "source-17 FOH pan proposed center / unit %".into()));
    f.synchronize_review();
    f.mark_presented();
    assert!(f.scene().in_bounds());
    f.selected = 1;
    assert!(f.action(Action::Confirm).is_err());
    assert!(rx.try_recv().is_err());
    f.action(Action::Cancel).unwrap();
    assert!(f.exact_draft.is_none());
    assert!(
        rx.try_iter()
            .all(|r| !matches!(r.operation, Operation::Confirm(_) | Operation::Set { .. }))
    );
    let (mut f, _) = exact_surface();
    f.key("D").unwrap();
    for _ in 0..16 {
        f.key("0").unwrap();
    }
    assert!(f.key("0").is_err());
    assert_eq!(f.exact_draft.as_ref().unwrap().text.len(), 16);
    f.key("Backspace").unwrap();
    assert_eq!(f.exact_draft.as_ref().unwrap().text.len(), 15);
    f.state.as_mut().unwrap().last_operation =
        Some("PENDING; awaiting provider confirmation".into());
    assert!(f.action(Action::ExactEdit(Parameter::Fader)).is_err());
}
#[test]
fn detached_content_survives_focus_resize_disconnect_and_revision_changes() {
    let (mut f, rx) = brain_surface();
    f.brain_page = false;
    f.action(Action::ProcessingEdit).unwrap();
    f.processing_entry = "6.".into();
    let config = f.processing_draft.as_ref().unwrap().config.clone();
    let snapshot = crate::structure::decode_snapshot(include_bytes!(
        "../../tests/fixtures/gp14/v1/structure-16.json"
    ))
    .unwrap();
    f.structural_draft = Some(crate::structure::Draft::new(&snapshot, "output_routes", 1).unwrap());
    let document = f.structural_draft.as_ref().unwrap().document.clone();
    let crate::brain_device::Message::Snapshot(device) = crate::brain_device::decode(
        include_bytes!("../../tests/fixtures/gp15/device-v1/snapshot-unarmed.json"),
    )
    .unwrap() else {
        panic!()
    };
    f.device_draft = Some(device.observation.unwrap().config);
    f.device_entry = Some("{unfinished".into());
    let device_config = f.device_draft.clone();
    for event in [
        Event::Resize(1000, 700),
        Event::Focus(false),
        Event::Focus(true),
        Event::Resize(0, 0),
        Event::DeviceLost,
    ] {
        f.enqueue(event).unwrap();
        assert_eq!(f.processing_draft.as_ref().unwrap().config, config);
        assert_eq!(f.structural_draft.as_ref().unwrap().document, document);
        assert_eq!(f.device_draft, device_config);
        assert_eq!(f.processing_entry, "6.");
        assert_eq!(f.device_entry.as_deref(), Some("{unfinished"));
    }
    assert!(rx.try_recv().is_err());
    assert!(f.action(Action::ProcessingApply).is_err());
    assert!(f.action(Action::StructureApply).is_err());
    assert!(f.action(Action::DeviceApply).is_err());
    f.key("F5").unwrap();
    assert!(matches!(
        rx.try_recv().unwrap().operation,
        Operation::Reconnect
    ));
    assert!(rx.try_recv().is_err());
    let mut update = surface().state.unwrap();
    update.generation = f.provider.generation();
    update.attachment_generation = f.provider.generation();
    update.snapshot.as_mut().unwrap().authority.revision = "5".into();
    update.processing.as_mut().unwrap().revision = "5".into();
    f.accept_update(update);
    f.pump();
    assert_eq!(f.processing_draft.as_ref().unwrap().config, config);
    assert_eq!(f.structural_draft.as_ref().unwrap().document, document);
    assert_eq!(f.device_draft, device_config);
    assert!(f.state.as_ref().unwrap().review.is_none());
    assert!(
        rx.try_recv().is_err(),
        "no automatic submission after recovery"
    );
    f.action(Action::Cancel).unwrap();
    assert!(f.processing_draft.is_none());
    assert!(f.structural_draft.is_none());
    assert!(f.device_draft.is_none());
    assert!(f.device_entry.is_none());
    assert!(f.processing_entry.is_empty());
    assert!(matches!(
        rx.try_recv().unwrap().operation,
        Operation::Cancel
    ));
}
#[test]
fn retained_processing_draft_recovery_requests_new_review_only() {
    let (mut f, rx) = brain_surface();
    f.brain_page = false;
    f.action(Action::ProcessingEdit).unwrap();
    f.action(Action::ProcessingField(2)).unwrap();
    f.action(Action::ProcessingText("6.1".into())).unwrap();
    let config = f.processing_draft.as_ref().unwrap().config.clone();
    let mut update = f.state.as_ref().unwrap().clone();
    f.enqueue(Event::Focus(false)).unwrap();
    assert!(f.action(Action::ProcessingApply).is_err());
    f.enqueue(Event::Focus(true)).unwrap();
    update.generation = f.provider.generation();
    update.received = Instant::now();
    update.snapshot.as_mut().unwrap().authority.revision = "5".into();
    update.processing.as_mut().unwrap().revision = "5".into();
    update.review = None;
    f.accept_update(update);
    assert!(rx.try_recv().is_err());
    f.action(Action::ProcessingApply).unwrap();
    let request = rx.try_recv().unwrap();
    assert_eq!(request.revision.as_deref(), Some("5"));
    assert!(
        matches!(request.operation, Operation::ReviewProcessing { config: sent, .. } if sent == config)
    );
    assert!(
        rx.try_recv().is_err(),
        "Apply requests review, never confirmation"
    );
}
#[test]
fn publication_preserves_raw_brain_device_and_structure_observation_lifetimes() {
    let (mut f, _) = brain_surface();
    let crate::brain_device::Message::Snapshot(device) = crate::brain_device::decode(
        include_bytes!("../../tests/fixtures/gp15/device-v1/snapshot-unarmed.json"),
    )
    .unwrap() else {
        panic!()
    };
    let u = f.state.as_mut().unwrap();
    u.device = Some(device);
    u.device_fresh = true;
    u.structural_fresh = true;
    u.snapshot_age_ms = Some(240);
    u.brain_age_ms = Some(240);
    u.device_age_ms = Some(240);
    u.structural_age_ms = Some(240);
    u.received = Instant::now();
    assert!(u.raw_fresh() && u.brain_is_fresh() && u.device_is_fresh() && u.structure_is_fresh());
    u.received = Instant::now() - Duration::from_millis(20);
    assert!(
        !u.raw_fresh() && !u.brain_is_fresh() && !u.device_is_fresh() && !u.structure_is_fresh()
    );
    // A younger raw observation cannot extend older independent observations.
    u.snapshot_age_ms = Some(0);
    assert!(u.raw_fresh());
    assert!(!u.brain_is_fresh() && !u.device_is_fresh() && !u.structure_is_fresh());
    assert!(f.fresh());
    let text = f
        .scene()
        .primitives
        .into_iter()
        .filter_map(|p| match p {
            Primitive::Text { value, .. } => Some(value),
            _ => None,
        })
        .collect::<Vec<_>>()
        .join("\n");
    assert!(text.contains("observation STALE"));
    assert!(
        text.lines()
            .any(|line| line.starts_with("DEVICE ") && line.contains(" / STALE / "))
    );
}
#[test]
fn confirmed_grant_survives_health_coalescing_but_expiry_and_disconnect_revoke_it() {
    struct NoIo;
    impl crate::local_audio::AuthorityConnection for NoIo {
        fn send_frame_until(&mut self, _: &[u8], _: Instant) -> Result<(), String> {
            panic!("no I/O")
        }
        fn receive_until(&mut self, _: Instant) -> Result<Option<Vec<u8>>, String> {
            panic!("no I/O")
        }
        fn receive_available(&mut self) -> Result<Option<Vec<u8>>, String> {
            panic!("no I/O")
        }
    }
    let corpus: Value = serde_json::from_str(include_str!(
        "../../tests/fixtures/gp03/v1/e03-rendered.json"
    ))
    .unwrap();
    let mut operator = Operator::from_connection(
        Box::new(NoIo),
        "11111111-1111-4111-8111-111111111111",
        9,
        "desk-corpus",
        "foh",
    )
    .unwrap();
    operator
        .session
        .ingest_snapshot(
            crate::audio::decode_snapshot(&serde_json::to_vec(&corpus["initial"]).unwrap())
                .unwrap(),
            0,
        )
        .unwrap();
    assert!(confirmed_lease_remaining(&operator).is_none());
    operator
        .session
        .begin("grant", json!({"scope":"foh"}), 0)
        .unwrap();
    assert!(confirmed_lease_remaining(&operator).is_none());
    operator
        .session
        .accept(
            crate::audio::decode_reply(&serde_json::to_vec(&corpus["grant_response"]).unwrap())
                .unwrap(),
            1,
        )
        .unwrap();
    let mut update = surface().state.take().unwrap();
    update.last_operation = Some(operator.session.last_result.clone());
    update.writer_lease_remaining_ms = confirmed_lease_remaining(&operator);
    update.status = "Structural state unavailable: bounded poll failed".into();
    update.fresh = false;
    let latest = Latest::default();
    publish_provider_update(&latest, update, None);
    let mut final_update = latest.update.lock().unwrap().take().unwrap();
    assert!(
        final_update
            .last_operation
            .as_ref()
            .unwrap()
            .starts_with("grant applied")
    );
    assert!(final_update.status.contains("poll failed"));
    assert!(!final_update.fresh);
    assert!(final_update.writer_granted());
    final_update.received = Instant::now() - Duration::from_secs(2);
    assert!(!final_update.writer_granted());
    operator.session.disconnect();
    assert!(confirmed_lease_remaining(&operator).is_none());
}
#[test]
fn final_worker_update_keeps_stage_error_after_failed_health_poll() {
    let f = surface();
    let latest = Latest::default();
    let mut update = f.state.as_ref().unwrap().clone();
    update.status = "PENDING structural review".into();
    publish_provider_update(&latest, update.clone(), None);
    let operation_error = "REFUSED/UNCERTAIN: staged context changed";
    update.status = "Structural state unavailable: structural snapshot deadline".into();
    update.fresh = false;
    update.structural_fresh = false;
    // Read only the final coalesced update, as a slow frontend would.
    publish_provider_update(&latest, update.clone(), Some(operation_error));
    let final_update = latest.update.lock().unwrap().take().unwrap();
    assert_eq!(final_update.status, operation_error);
    assert!(!final_update.fresh);
    assert!(!final_update.structural_fresh);
    publish_provider_update(&latest, update, None); // next explicit operation clears sticky error
    assert!(
        latest
            .update
            .lock()
            .unwrap()
            .as_ref()
            .unwrap()
            .status
            .starts_with("Structural state unavailable")
    );
}
#[test]
fn batched_raw_keys_apply_editor_mode_at_dispatch() {
    let snapshot = crate::structure::decode_snapshot(include_bytes!(
        "../../tests/fixtures/gp14/v1/structure-16.json"
    ))
    .unwrap();
    let mut f = surface();
    let (tx, _rx) = mpsc::sync_channel(8);
    f.provider = Provider {
        tx,
        pending_edits: Arc::new(AtomicUsize::new(0)),
        latest: Arc::new(Latest::default()),
        generation: Arc::new(AtomicU64::new(1)),
        stop: Arc::new(AtomicBool::new(false)),
        child: None,
        authorization: Arc::new(AtomicBool::new(true)),
        brain_signal: Arc::new(crate::brain::HoldSignal::default()),
    };
    let state = f.state.as_mut().unwrap();
    state.structural_fresh = true;
    state.snapshot.as_mut().unwrap().authority.revision = snapshot.revision.clone();
    state.received = Instant::now();
    f.structural_draft =
        Some(crate::structure::Draft::new(&snapshot, "pa_configuration", 1).unwrap());
    let draft = f.structural_draft.as_mut().unwrap();
    draft.selected = draft
        .fields
        .iter()
        .position(|p| p == "/configuration/outputs/0/source")
        .unwrap();
    let selected = draft.selected;
    let mut keys = vec!["F3".to_string()];
    keys.extend(r#"{"node":0}"#.chars().map(|c| c.to_string()));
    keys.extend(["Enter".into(), "i".into()]);
    for key in keys {
        for pressed in [true, false] {
            f.enqueue(Event::Key {
                key: key.clone(),
                pressed,
            })
            .unwrap();
        }
    }
    f.pump();
    let draft = f.structural_draft.as_ref().unwrap();
    assert_eq!(
        draft.document["configuration"]["outputs"][0]["source"],
        json!({"node":0})
    );
    assert_eq!(
        draft.selected,
        selected + 1,
        "lowercase shortcut after Enter normalizes at dispatch"
    );
    assert!(!f.structure_text_entry);
}
#[test]
fn structural_json_keyboard_and_semantic_import_preserve_complete_owner_intent() {
    let snapshot = crate::structure::decode_snapshot(include_bytes!(
        "../../tests/fixtures/gp14/v1/structure-16.json"
    ))
    .unwrap();
    let mut f = surface();
    f.structural_draft =
        Some(crate::structure::Draft::new(&snapshot, "pa_configuration", 1).unwrap());
    let path = "/configuration/outputs/0/source";
    let draft = f.structural_draft.as_mut().unwrap();
    draft.selected = draft.fields.iter().position(|p| p == path).unwrap();
    f.key("F3").unwrap();
    for c in r#"{"node":0}"#.chars() {
        f.key(&c.to_string()).unwrap();
    }
    f.key("Enter").unwrap();
    assert!(!f.structure_text_entry);
    assert_eq!(
        f.structural_draft
            .as_ref()
            .unwrap()
            .document
            .pointer(path)
            .unwrap(),
        &json!({"node":0})
    );
    let document = serde_json::to_string(&f.structural_draft.as_ref().unwrap().document).unwrap();
    f.action(Action::StructureImport(document)).unwrap();
    assert!(f.scene().in_bounds());
    f.key("F3").unwrap();
    f.key("Q").unwrap(); // text, never a writer release
    assert_eq!(f.processing_entry, "Q");
    f.key("Esc").unwrap();
    assert!(f.structural_draft.is_some());
    assert!(f.processing_entry.is_empty());
}
#[test]
fn processing_selection_uses_identity_after_independent_inventory_reorder() {
    let mut f = surface();
    let u = f.state.as_mut().unwrap();
    u.snapshot.as_mut().unwrap().authority.inputs.swap(0, 7);
    u.processing.as_mut().unwrap().channels.reverse();
    f.key("E").unwrap();
    assert_eq!(f.processing_draft.as_ref().unwrap().input, "input-08");
    assert_eq!(f.target("fader").unwrap()["input"], "input-08");
    // Removing the selected processing identity must not edit another channel.
    f.processing_draft = None;
    f.state
        .as_mut()
        .unwrap()
        .processing
        .as_mut()
        .unwrap()
        .channels
        .retain(|c| c.input != "input-08");
    assert!(f.key("E").is_err());
}
#[test]
fn changed_inventory_preserves_identity_and_revokes_queued_edits() {
    let mut f = surface();
    f.selected = 7;
    f.key("E").unwrap();
    let mut update = f.state.as_ref().unwrap().clone();
    update.snapshot.as_mut().unwrap().authority.inputs.reverse();
    f.inject_controller(Action::ProcessingAdjust(1)).unwrap();
    let generation = f.provider.generation();
    f.accept_update(update.clone());
    assert_eq!(f.selected, 0);
    assert_eq!(f.processing_draft.as_ref().unwrap().input, "input-08");
    assert!(f.queue.is_empty());
    assert!(f.state.is_none());
    assert!(f.provider.generation() > generation);
    update.generation = f.provider.generation();
    f.accept_update(update);
    assert_eq!(f.selected_input(), Some("input-08"));
}
#[test]
fn monitor_scope_resolution_never_aliases_an_unknown_scope() {
    let mut f = surface();
    f.scope = "monitor3".into();
    assert!(f.target("fader").is_err());
    let authority = &mut f
        .state
        .as_mut()
        .unwrap()
        .snapshot
        .as_mut()
        .unwrap()
        .authority;
    // Consumer model test only; legacy wire fixtures retain their exact shape.
    authority.monitors.push("monitor-3".into());
    authority.modes.push(("monitor3".into(), "manual".into()));
    assert_eq!(f.target("fader").unwrap()["monitor"], "monitor-3");
    assert!(f.target("pan").is_err());
    f.scope = "pa".into();
    assert!(f.target("fader").is_err());
    assert!(f.target("mute").is_err());
}
#[test]
fn partial_bank_navigation_wraps_banks_without_skipping_the_first_strip() {
    let mut f = surface();
    let mut update = f.state.as_ref().unwrap().clone();
    update.snapshot.as_mut().unwrap().authority.inputs =
        (1..=49).map(|n| format!("strip-{n}")).collect();
    for expected in [12, 24, 36, 48, 0] {
        f.state = Some(update.clone());
        f.action(Action::Bank(1)).unwrap();
        assert_eq!(f.selected, expected);
    }
    f.state = Some(update.clone());
    f.action(Action::Bank(-1)).unwrap();
    assert_eq!(f.selected, 48);
    f.state = Some(update);
    f.action(Action::Bank(i32::MAX)).unwrap();
    assert!(f.selected < 49);
}
#[test]
fn high_channel_selection_and_banking_are_presentation_dimensions() {
    for count in [16, 17, 32, 33, 48, 49] {
        let mut f = surface();
        f.state
            .as_mut()
            .unwrap()
            .snapshot
            .as_mut()
            .unwrap()
            .authority
            .inputs = (1..=count).map(|n| format!("strip-{n}")).collect();
        for number in [16, 17, 32, 33, 48].into_iter().filter(|n| *n <= count) {
            f.selected = number - 1;
            assert_eq!(
                f.target("fader").unwrap()["input"],
                format!("strip-{number}")
            );
            f.page = Page::Mix;
            assert!(f.scene().in_bounds());
            assert!(f.scene().primitives.iter().any(|p| matches!(p,
                Primitive::Text {value, ..} if value.starts_with(&format!("> strip-{number} ")))));
        }
        f.selected = count - 1;
        f.action(Action::Move(1)).unwrap();
        assert_eq!(f.selected, 0);
    }
}
#[test]
fn processing_keyboard_and_controller_semantics_edit_same_detached_draft() {
    let mut keyboard = surface();
    let mut controller = surface();
    keyboard.key("E").unwrap();
    controller.action(Action::ProcessingEdit).unwrap();
    keyboard.key("I").unwrap();
    keyboard.key("I").unwrap();
    controller.action(Action::ProcessingField(2)).unwrap();
    keyboard.key("6").unwrap();
    keyboard.key(".").unwrap();
    keyboard.key("1").unwrap();
    keyboard.key("Enter").unwrap();
    controller
        .action(Action::ProcessingText("6.1".into()))
        .unwrap();
    assert_eq!(
        keyboard.processing_draft.as_ref().unwrap().config,
        controller.processing_draft.as_ref().unwrap().config
    );
    assert_eq!(
        keyboard.state.as_ref().unwrap().processing,
        controller.state.as_ref().unwrap().processing
    );
    assert!(keyboard.scene().in_bounds());
    let lines: Vec<_> = keyboard
        .scene()
        .primitives
        .into_iter()
        .filter_map(|p| {
            if let Primitive::Text { value, .. } = p {
                Some(value)
            } else {
                None
            }
        })
        .collect();
    assert!(lines.iter().any(|s| s.contains("LOCAL DRAFT")));
    assert!(lines.iter().any(|s| s.contains("Band 1 gain +6.1 dB")));
    keyboard.key("Right").unwrap();
    assert!(keyboard.processing_draft.is_some());
    assert!(keyboard.state.is_none());
    assert!(keyboard.action(Action::ProcessingApply).is_err());
}
#[test]
fn navigation_release_is_not_lost_to_generation_synchronization() {
    let mut f = surface();
    f.pressed.insert("Right".into());
    f.key("Right").unwrap();
    assert!(f.blocked.contains("Right"));
    assert_eq!(f.observed_generation, f.provider.generation());
    f.enqueue(Event::Key {
        key: "Right".into(),
        pressed: false,
    })
    .unwrap();
    f.pump();
    assert!(!f.blocked.contains("Right"));
}
#[test]
fn processing_stale_role_loss_and_review_fences_disable_edits() {
    let mut f = surface();
    f.state.as_mut().unwrap().processing_age_ms = Some(251);
    assert!(f.key("E").is_err());
    f.state.as_mut().unwrap().processing_age_ms = Some(0);
    f.key("E").unwrap();
    f.key("K").unwrap();
    let retained = f.processing_draft.as_ref().unwrap().config.clone();
    f.processing_entry = "6.".into();
    f.enqueue(Event::Focus(false)).unwrap();
    assert_eq!(f.processing_draft.as_ref().unwrap().config, retained);
    assert_eq!(f.processing_entry, "6.");
    assert!(f.action(Action::ProcessingApply).is_err());
    let mut f = surface();
    f.require_role();
    assert!(f.key("E").is_err());
    let mut f = surface();
    f.state.as_mut().unwrap().review = Some((1, "Processing review".into()));
    f.synchronize_review();
    assert!(f.key("E").is_err());
    assert!(f.key("Enter").unwrap_err().contains("every displayed"));
    f.fence();
    assert!(f.key("Enter").is_err());
}
#[test]
fn four_band_extreme_values_all_endpoints_fit_without_truncation_or_overlap() {
    let mut f = surface();
    let p = f.state.as_mut().unwrap().processing.as_mut().unwrap();
    let mut value = serde_json::to_value(&p.channels[0].target).unwrap();
    for band in 1..=4 {
        value[format!("band{band}_hz")] = json!(20000);
        value[format!("band{band}_gain_mdb")] = json!(-12000);
        value[format!("band{band}_q_milli")] = json!(10000);
        value[format!("band{band}_bypass")] = json!(true);
    }
    p.channels[0].current = crate::processing::decode_config(&value).unwrap();
    p.channels[0].target = p.channels[0].current.clone();
    f.key("E").unwrap();
    for i in 0..24 {
        assert_eq!(f.processing_field, i);
        let scene = f.scene();
        let mut rows = Vec::new();
        for primitive in scene.primitives {
            if let Primitive::Text { x, y, value, .. } = primitive {
                assert!(x + value.chars().count() as u32 * 12 <= 1920, "{value}");
                if (312..600).contains(&y) {
                    rows.push((y, value));
                }
            }
        }
        assert_eq!(rows.len(), 12);
        for (band, (_, row)) in rows.iter().enumerate().take(5).skip(1) {
            let expected = format!("Band {band} bell 20000Hz -12.0dB Q10.0 BYPASS");
            assert_eq!(
                row.matches(&expected).count(),
                3,
                "all three endpoints must be fully visible: {row}"
            );
        }
        assert!(rows.windows(2).all(|r| r[1].0 >= r[0].0 + 24));
        f.key("I").unwrap();
    }
    assert_eq!(f.processing_field, 0);
}
#[test]
fn zero_size_review_never_counts_as_presented_and_all24_values_are_reviewable() {
    let mut f = surface();
    let config = f
        .state
        .as_ref()
        .unwrap()
        .processing
        .as_ref()
        .unwrap()
        .channels[0]
        .target
        .clone();
    let fields = crate::processing::FIELDS
        .iter()
        .map(|field| config.display(*field))
        .collect::<Vec<_>>();
    f.state.as_mut().unwrap().review = Some((10, fields.join("\n")));
    f.synchronize_review();
    f.width = 0;
    let _ = crate::raster::rgba(&f.scene());
    f.mark_presented();
    assert!(f.review_seen.is_empty());
    assert!(f.action(Action::Confirm).is_err());
    f.width = 540;
    f.height = 960;
    let scene = f.scene();
    for field in fields {
        assert!(
            scene
                .primitives
                .iter()
                .any(|p| matches!(p,Primitive::Text{value,..} if value==&field)),
            "{field}"
        );
    }
    let _ = crate::raster::rgba(&scene);
    f.state.as_mut().unwrap().received = Instant::now();
    f.mark_presented();
    assert_eq!(f.review_seen.len(), 1);
}
#[test]
fn injected_local_field_burst_does_not_fill_provider_release_queue() {
    let mut f = surface();
    let (tx, rx) = mpsc::sync_channel(8);
    f.provider = Provider {
        tx,
        pending_edits: Arc::new(AtomicUsize::new(0)),
        latest: Arc::new(Latest::default()),
        generation: Arc::new(AtomicU64::new(1)),
        stop: Arc::new(AtomicBool::new(false)),
        child: None,
        authorization: Arc::new(AtomicBool::new(true)),
        brain_signal: Arc::new(crate::brain::HoldSignal::default()),
    };
    f.state.as_mut().unwrap().received = Instant::now();
    f.inject_controller(Action::ProcessingEdit).unwrap();
    f.pump();
    assert!(matches!(
        rx.try_recv().unwrap().operation,
        Operation::InputReleased
    ));
    for _ in 0..24 {
        f.inject_controller(Action::ProcessingField(1)).unwrap();
        f.pump();
    }
    f.inject_controller(Action::ProcessingText("0".into()))
        .unwrap();
    f.pump();
    assert!(
        rx.try_recv().is_err(),
        "local fields must not send provider traffic"
    );
    assert!(!f.processing_draft.as_ref().unwrap().config.eq_bypass);
    f.inject_controller(Action::ProcessingApply).unwrap();
    f.pump();
    assert!(matches!(
        rx.try_recv().unwrap().operation,
        Operation::ReviewProcessing { .. }
    ));
    assert!(matches!(
        rx.try_recv().unwrap().operation,
        Operation::InputReleased
    ));
    assert!(rx.try_recv().is_err());
}

#[test]
fn meters_preserve_master_eq_shortcuts_and_local_drafts() {
    let (mut f, rx) = master_surface();
    f.brain_page = false;
    f.key("B").unwrap();
    assert!(f.meter_page);
    f.key("T").unwrap();
    assert!(f.meter_processed);
    f.key("B").unwrap();
    assert!(!f.meter_page);
    f.key("F11").unwrap();
    assert!(f.structural_draft.is_some());
    f.key("B").unwrap();
    assert!(!f.meter_page, "Master EQ B remains the section selector");
    assert!(f.structural_draft.is_some());
    assert!(
        rx.try_recv().is_err(),
        "browsing never sends a control operation"
    );
}
