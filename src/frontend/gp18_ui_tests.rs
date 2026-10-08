use super::*;
fn sends_surface() -> (Frontend, Receiver<Request>) {
    let (mut f, rx) = super::processing_tests::brain_surface();
    f.brain_page = false;
    f.brain_enabled = false;
    f.sends_page = true;
    f.selected_monitor = 2;
    f.scope = "monitor3".into();
    let u = f.state.as_mut().unwrap();
    u.snapshot = Some(
        crate::audio::decode_snapshot(include_bytes!(
            "../../tests/fixtures/gp18/v1-corrected/raw-baseline.json"
        ))
        .unwrap(),
    );
    u.sends = crate::sends::decode_reply(include_bytes!(
        "../../tests/fixtures/gp18/v1-corrected/baseline.json"
    ))
    .unwrap()
    .snapshot;
    u.sends_age_ms = Some(0);
    u.processing = None;
    u.received = Instant::now();
    (f, rx)
}
#[test]
fn independent_inventories_separate_level_tap_reviews_and_scope_switch_do_not_mutate_holds() {
    let (mut f, rx) = sends_surface();
    let original = f.state.as_ref().unwrap().snapshot.clone();
    assert!(f.scene().in_bounds());
    f.key("E").unwrap();
    f.key("2").unwrap();
    assert!(rx.try_recv().is_err());
    f.key("F4").unwrap();
    assert!(
        matches!(rx.try_recv().unwrap().operation,Operation::ReviewTap(ref b) if b["input"]=="input-01"&&b["monitor"]=="monitor-3"&&b["tap"]=="processed_pre_fader")
    );
    assert_eq!(f.state.as_ref().unwrap().snapshot, original);
    f.key("S").unwrap();
    for key in ["-", "6", ".", "1", "Enter"] {
        f.key(key).unwrap();
    }
    assert!(matches!(
        f.send_draft.as_ref().unwrap().value,
        SendDraftValue::Level(-6100)
    ));
    f.key("F4").unwrap();
    assert!(
        matches!(rx.try_recv().unwrap().operation,Operation::ReviewSet{ref target,ref value} if target["monitor"]=="monitor-3"&&*value==json!(-6100))
    );
    f.state.as_mut().unwrap().review = Some((99, "queued tap / monitor3".into()));
    f.synchronize_review();
    f.action(Action::SwitchScope("foh".into())).unwrap();
    assert!(f.state.is_none());
    assert!(f.review_seen.is_empty());
    assert!(matches!(rx.try_recv().unwrap().operation,Operation::SwitchScope(ref s) if s=="foh"));
    assert!(rx.try_iter().all(|r| !matches!(
        r.operation,
        Operation::Grant | Operation::Set { .. } | Operation::Mode { .. } | Operation::Preview(_)
    )));
}
#[test]
fn sends_display_separates_raw_tap_age_and_elapsed_last_confirmed_values() {
    let (mut f, _rx) = sends_surface();
    let u = f.state.as_mut().unwrap();
    u.snapshot_age_ms = Some(0);
    u.sends_age_ms = Some(240);
    u.received = Instant::now() - Duration::from_millis(20);
    assert!(f.fresh());
    assert!(!f.sends_ready());
    let texts: Vec<_> = f
        .scene()
        .primitives
        .into_iter()
        .filter_map(|p| match p {
            Primitive::Text { value, .. } => Some(value),
            _ => None,
        })
        .collect();
    assert!(texts.iter().any(|s| s.contains("RAW fresh / TAPS stale")));
    assert!(
        texts
            .iter()
            .any(|s| s.contains("Raw / Post-mute") && s.contains("LAST CONFIRMED"))
    );
    assert!(texts.iter().any(|s| s.contains("-60.0 dB / -60.0 dB")));
    assert!(
        texts
            .iter()
            .any(|s| s.contains("raw age ") && s.contains("tap age "))
    );
    assert!(!texts.iter().any(|s| s.contains("Some(")));
    assert!(f.action(Action::SendTapEdit).is_err());
}
#[test]
fn response_frequency_labels_have_disjoint_text_bounds_in_both_sections() {
    let (mut f, _rx) = super::processing_tests::master_surface();
    f.key("F11").unwrap();
    for (graphic, y) in [(false, 684), (true, 852)] {
        if graphic {
            f.key("B").unwrap();
        }
        let mut labels: Vec<_> = f
            .scene()
            .primitives
            .into_iter()
            .filter_map(|p| match p {
                Primitive::Text {
                    x, y: yy, value, ..
                } if yy == y => Some((x, value.len() as u32 * 12)),
                _ => None,
            })
            .collect();
        labels.sort_unstable();
        assert_eq!(labels.len(), 5);
        assert!(
            labels
                .windows(2)
                .all(|pair| pair[0].0 + pair[0].1 + 12 <= pair[1].0)
        );
        assert!(labels.iter().all(|(x, width)| x + width <= 1920));
    }
}
#[test]
fn reattachment_ignores_old_attachment_even_after_input_generation_advances() {
    let (mut f, _rx) = sends_surface();
    let mut old = f.state.clone().unwrap();
    f.action(Action::SwitchScope("foh".into())).unwrap();
    old.generation = f.provider.generation();
    f.accept_update(old.clone());
    assert!(f.state.is_none());
    assert!(!f.fresh());
    assert!(f.key("G").is_err());
    old.attachment_generation = f.provider.generation();
    old.writer_lease_remaining_ms = None;
    f.accept_update(old);
    assert!(f.fresh());
    assert!(!f.state.as_ref().unwrap().writer_granted());
}
#[test]
fn monitor_browse_is_read_only_text_precedes_shortcuts_and_focus_revokes_apply() {
    let (mut f, _rx) = sends_surface();
    f.selected_monitor = 1;
    assert!(f.action(Action::SendTapEdit).is_err());
    assert!(f.action(Action::Adjust(1000)).is_err());
    f.selected_monitor = 2;
    f.key("S").unwrap();
    assert!(f.key("O").is_err());
    assert_eq!(f.scope, "monitor3");
    f.key("Esc").unwrap();
    f.action(Action::SendLevelText("-6.125".into()))
        .unwrap_err();
    f.action(Action::SendLevelText("12.0".into())).unwrap();
    f.enqueue(Event::Focus(false)).unwrap();
    assert!(f.send_draft.is_some());
    assert!(f.action(Action::SendApply).is_err());
}
#[test]
fn all_master_fields_and_all_graphic_positions_fit_native_scene() {
    let (mut f, _rx) = super::processing_tests::master_surface();
    f.key("F11").unwrap();
    for _ in 0..41 {
        assert!(f.scene().in_bounds());
        f.key("I").unwrap();
    }
    f.key("B").unwrap();
    for _ in 0..32 {
        assert!(f.scene().in_bounds());
        f.key("I").unwrap();
    }
}
#[test]
fn live_scene_independently_expired_raw_or_extension_is_last_confirmed() {
    for expire_raw in [false, true] {
        let (mut f, _) = super::processing_tests::master_surface();
        f.brain_page = false;
        f.live_page = true;
        let corpus: Vec<Value> = serde_json::from_slice(include_bytes!(
            "../../tests/fixtures/master-eq/v1/producer.json"
        ))
        .unwrap();
        let live = crate::live_eq::Snapshot::decode(
            corpus
                .into_iter()
                .find(|v| v["label"] == "baseline")
                .unwrap()["snapshot"]
                .clone(),
        )
        .unwrap();
        let u = f.state.as_mut().unwrap();
        u.snapshot = Some(
            crate::audio::decode_snapshot(include_bytes!(
                "../../tests/fixtures/gp18/v1-corrected/raw-baseline.json"
            ))
            .unwrap(),
        );
        u.snapshot.as_mut().unwrap().authority.revision = live.revision.clone();
        u.snapshot
            .as_mut()
            .unwrap()
            .topology
            .as_mut()
            .unwrap()
            .map_revision = live.map_revision.parse().unwrap();
        u.live_eq = Some(live);
        u.live_eq_age_ms = Some(0);
        u.snapshot_age_ms = Some(0);
        u.received = Instant::now();
        let labels: Vec<_> = f
            .scene()
            .primitives
            .into_iter()
            .filter_map(|p| match p {
                Primitive::Text {
                    x, y: 804, value, ..
                } => Some((x, value)),
                _ => None,
            })
            .collect();
        assert_eq!(labels.len(), 10);
        assert!(labels.contains(&(487, "1k".into())));
        assert!(labels.contains(&(1447, "1k".into())));
        for pane in labels.chunks(5) {
            assert!(
                pane.windows(2)
                    .all(|p| p[0].0 + p[0].1.len() as u32 * 12 + 12 <= p[1].0)
            );
        }
        assert!(f.live_eq_ready());
        if expire_raw {
            f.state.as_mut().unwrap().snapshot_age_ms = Some(251);
        } else {
            f.state.as_mut().unwrap().live_eq_age_ms = Some(251);
        }
        assert!(!f.live_eq_ready());
        assert!(f.action(Action::LiveEqEdit).is_err());
        assert!(f.scene().primitives.iter().any(
            |p| matches!(p,Primitive::Text{value,..} if value.contains("STALE / LAST CONFIRMED"))
        ));
    }
}
#[test]
fn processing_controller_cannot_open_hidden_under_sends_but_visible_keys_work() {
    let (mut f, rx) = sends_surface();
    f.scope = "foh".into();
    f.page = Page::Channel;
    let u = f.state.as_mut().unwrap();
    u.processing = crate::processing::decode_reply(include_bytes!(
        "../../tests/fixtures/gp18/v1-corrected/gp07v4-ready.json"
    ))
    .unwrap()
    .snapshot;
    u.processing_age_ms = Some(0);
    u.snapshot.as_mut().unwrap().authority.revision =
        u.processing.as_ref().unwrap().revision.clone();
    assert!(f.action(Action::ProcessingEdit).is_err());
    assert!(f.processing_draft.is_none());
    assert!(
        f.scene()
            .primitives
            .iter()
            .any(|p| matches!(p,Primitive::Text{value,..} if value.contains("SENDS")))
    );
    let mut fresh = f.state.clone().unwrap();
    f.action(Action::Page(Page::Channel)).unwrap();
    fresh.generation = f.provider.generation();
    fresh.received = Instant::now();
    f.accept_update(fresh);
    f.key("E").unwrap();
    f.key("I").unwrap();
    assert!(f.processing_draft.is_some());
    assert!(
        f.scene()
            .primitives
            .iter()
            .any(|p| matches!(p,Primitive::Text{value,..} if value.contains("EDIT FIELD")))
    );
    f.key("F4").unwrap();
    assert!(
        rx.try_iter()
            .any(|r| matches!(r.operation, Operation::ReviewProcessing { .. }))
    );
}
#[test]
#[ignore = "explicit current frontend offline preview gallery; no connected engine or display"]
fn current_frontend_offline_gallery() {
    let output = std::path::PathBuf::from(
        std::env::var_os("SHR_DESK_OFFLINE_GALLERY").expect("explicit output directory"),
    );
    std::fs::create_dir_all(&output).unwrap();
    let save = |name: &str, f: &mut Frontend| {
        f.state.as_mut().unwrap().received = Instant::now();
        let mut scene = f.scene();
        scene.primitives.push(Primitive::Rect {
            x: 0,
            y: 1032,
            w: 1920,
            h: 48,
            fill: "#10151d",
        });
        scene.primitives.push(Primitive::Text {
            x: 24,
            y: 1044,
            value:
                "OFFLINE / SIMULATED fixture-driven current Frontend scene / NO CONNECTED ENGINE"
                    .into(),
            color: "#f47c85",
        });
        assert!(scene.in_bounds(), "{name}");
        #[cfg(feature = "native")]
        if std::env::var_os("VK_DRIVER_FILES").is_some() {
            for (w, h) in [(1920, 1080), (960, 540), (728, 1024)] {
                println!(
                    "{name}: {}",
                    crate::native::offscreen_at(&scene, w, h).unwrap()
                );
            }
        }
        std::fs::write(
            output.join(format!("{name}.svg")),
            crate::render::svg(&scene),
        )
        .unwrap();
        crate::raster::ppm(&scene, &output.join(format!("{name}.ppm"))).unwrap();
        // Each next offline gesture starts from the same explicit simulated fixture.
        f.state.as_mut().unwrap().received = Instant::now();
    };
    let (mut f, _rx) = sends_surface();
    f.selected = 16;
    save("sends-overview-monitor3", &mut f);
    f.sends_channel = true;
    f.action(Action::SendTapEdit).unwrap();
    f.action(Action::SendTap(crate::sends::Tap::ProcessedPreFader))
        .unwrap();
    save("channel-sends", &mut f);
    f.sends_page = false;
    f.send_draft = None;
    f.scope = "foh".into();
    f.page = Page::Channel;
    let u = f.state.as_mut().unwrap();
    u.processing = crate::processing::decode_reply(include_bytes!(
        "../../tests/fixtures/gp18/v1-corrected/gp07v4-ready.json"
    ))
    .unwrap()
    .snapshot;
    u.snapshot.as_mut().unwrap().authority.revision =
        u.processing.as_ref().unwrap().revision.clone();
    u.processing_age_ms = Some(0);
    u.received = Instant::now();
    let channel = u
        .processing
        .as_mut()
        .unwrap()
        .channels
        .iter_mut()
        .find(|c| c.input == "input-17")
        .unwrap();
    channel.current.eq_bypass = false;
    channel.current.band2_hz = 1250;
    channel.current.band2_gain_mdb = 4500;
    channel.current.band2_bypass = false;
    channel.current.compressor_bypass = false;
    channel.current.threshold_mdb = -24000;
    channel.current.ratio_milli = 3000;
    channel.current.validate().unwrap();
    channel.target = channel.current.clone();
    channel.gain_reduction_mdb = None;
    save("channel-eq-compressor", &mut f);
    let (mut f, _rx) = super::processing_tests::master_surface();
    f.key("F11").unwrap();
    f.action(Action::StructureField(3)).unwrap();
    f.action(Action::StructureText("6.125".into())).unwrap();
    f.action(Action::StructureField(3)).unwrap();
    f.action(Action::StructureText("high_shelf".into()))
        .unwrap();
    f.action(Action::StructureField(1)).unwrap();
    f.action(Action::StructureText("4000".into())).unwrap();
    f.action(Action::StructureField(1)).unwrap();
    f.action(Action::StructureText("-3.5".into())).unwrap();
    f.action(Action::StructureField(-5)).unwrap();
    save("master-parametric", &mut f);
    f.key("B").unwrap();
    f.action(Action::StructureField(-3)).unwrap();
    f.action(Action::StructureText("true".into())).unwrap();
    f.action(Action::StructureField(18)).unwrap();
    f.action(Action::StructureText("-4.5".into())).unwrap();
    save("master-graphic", &mut f);
    let corpus: Vec<Value> = serde_json::from_slice(include_bytes!(
        "../../tests/fixtures/master-eq/v1/producer.json"
    ))
    .unwrap();
    f.structural_draft = None;
    f.live_page = true;
    f.state.as_mut().unwrap().snapshot = Some(
        crate::audio::decode_snapshot(include_bytes!(
            "../../tests/fixtures/gp18/v1-corrected/raw-baseline.json"
        ))
        .unwrap(),
    );
    for (label, name) in [
        ("transition", "live-current-target"),
        ("settled", "live-settled"),
        ("settled", "live-stale"),
        ("unavailable", "live-unavailable"),
    ] {
        let live = crate::live_eq::Snapshot::decode(
            corpus.iter().find(|r| r["label"] == label).unwrap()["snapshot"].clone(),
        )
        .unwrap();
        let u = f.state.as_mut().unwrap();
        let raw = u.snapshot.as_mut().unwrap();
        raw.authority.show_id = live.show_id.clone();
        raw.authority.epoch = live.epoch.clone();
        raw.clock.as_mut().unwrap().epoch = live.epoch.parse().unwrap();
        raw.authority.revision = live.revision.clone();
        raw.topology.as_mut().unwrap().map_revision = live.map_revision.parse().unwrap();
        u.snapshot_age_ms = Some(0);
        u.live_eq_age_ms = Some(if name == "live-stale" { 251 } else { 0 });
        u.live_eq = Some(live);
        save(name, &mut f);
    }
}
