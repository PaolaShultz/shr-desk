use shr_desk::metering::{Cache, Identity, Request, Snapshot};
use std::time::{Duration, Instant};
fn fixture(name: &str) -> Vec<u8> {
    std::fs::read(format!("tests/fixtures/gp-meter/v1/{name}.json")).unwrap()
}
fn snapshot(name: &str) -> Snapshot {
    Snapshot::decode(&fixture(name)).unwrap()
}
fn identity(s: &Snapshot) -> Identity {
    Identity {
        show: s.show_id.clone(),
        epoch: s.source_epoch.0,
        map: s.map_generation.0,
        topology: s.topology.clone(),
        inputs: s.inputs,
        monitors: s.monitors,
        rate: s.sample_rate,
    }
}
#[test]
fn exact_producer_corpus_hashes_and_independent_strict_codec() {
    use sha2::{Digest, Sha256};
    let manifest: serde_json::Value = serde_json::from_slice(&fixture("manifest")).unwrap();
    for (name, hash) in manifest["files_sha256"].as_object().unwrap() {
        let b = std::fs::read(format!("tests/fixtures/gp-meter/v1/{name}")).unwrap();
        assert_eq!(format!("{:x}", Sha256::digest(&b)), hash.as_str().unwrap());
        if name.starts_with("valid-") || name.starts_with("unavailable-") {
            Snapshot::decode(&b).unwrap();
        } else if name.starts_with("reject-") {
            assert!(Snapshot::decode(&b).is_err(), "{name}");
        }
    }
    let r = Request::new("00000000-0000-0000-0000-000000000001", 1, 1, 1);
    let bytes = serde_json::to_vec(&r).unwrap();
    assert_eq!(
        serde_json::from_slice::<serde_json::Value>(&bytes).unwrap()["query_id"],
        "1"
    );
    Request::decode(&bytes).unwrap();
    let b = fixture("valid-dc");
    let mut text = String::from_utf8(b).unwrap();
    text = text.replacen("\"version\": 1", "\"version\": 1, \"version\": 1", 1);
    assert!(Snapshot::decode(text.as_bytes()).is_err());
}
#[test]
fn acquisition_age_query_budget_repeated_sequence_never_refresh_stopped_source() {
    let s = snapshot("valid-dc");
    let id = identity(&s);
    let now = Instant::now();
    let mut cache = Cache::default();
    cache
        .accept(s.clone(), &id, 1, Duration::from_millis(10), now)
        .unwrap();
    assert!(cache.fresh(now + Duration::from_millis(220)));
    assert!(!cache.fresh(now + Duration::from_millis(221)));
    let mut repeat = s.clone();
    repeat.query_id.0 = 2;
    repeat.acquisition_age_ms.as_mut().unwrap().0 = 20; // dishonest repeated age cannot extend existing expiry
    cache
        .accept(
            repeat,
            &id,
            2,
            Duration::ZERO,
            now + Duration::from_millis(200),
        )
        .unwrap();
    assert!(!cache.fresh(now + Duration::from_millis(221)));
    assert!(
        cache
            .accept(s.clone(), &id, 1, Duration::from_millis(101), now)
            .is_err()
    );
    assert!(cache.snapshot.is_none());
    let mut stale = s;
    stale.acquisition_age_ms.as_mut().unwrap().0 = 251;
    cache.accept(stale, &id, 1, Duration::ZERO, now).unwrap();
    assert!(!cache.fresh(now));
}
#[test]
fn context_regression_overlap_invalid_silence_and_clip_hold() {
    let mut s = snapshot("valid-over-range");
    let id = identity(&s);
    let now = Instant::now();
    let mut c = Cache::default();
    c.accept(s.clone(), &id, 1, Duration::ZERO, now).unwrap();
    assert!(c.clipped("input-01:raw", now));
    assert!(!c.clipped("input-01:raw", now + Duration::from_millis(251)));
    s.sequence.0 = 2;
    s.first_frame.as_mut().unwrap().0 = 960;
    s.end_frame.as_mut().unwrap().0 = 1920;
    s.query_id.0 = 2;
    s.taps.iter_mut().for_each(|t| {
        if t.clip_count.0 > 0 {
            t.clip_count.0 = 0;
            t.peak_millidbfs = Some(-6021);
            t.rms_millidbfs = Some(-6021);
            t.over_range = false;
        }
    });
    c.accept(
        s.clone(),
        &id,
        2,
        Duration::ZERO,
        now + Duration::from_millis(50),
    )
    .unwrap();
    assert!(c.clipped("input-01:raw", now + Duration::from_millis(100)));
    let mut bad = s.clone();
    bad.first_frame.as_mut().unwrap().0 = 959;
    bad.end_frame.as_mut().unwrap().0 = 1919;
    assert!(c.accept(bad, &id, 2, Duration::ZERO, now).is_err());
    assert!(c.snapshot.is_none());
    let mut wrong = id.clone();
    wrong.map = 2;
    assert!(c.accept(s, &wrong, 2, Duration::ZERO, now).is_err());
    let s = snapshot("valid-invalid");
    let id = identity(&s);
    c.accept(s, &id, 1, Duration::ZERO, now).unwrap();
    assert!(!c.snapshot.as_ref().unwrap().taps[94].valid);
    let s = snapshot("valid-silence");
    let id = identity(&s);
    c.clear("reattach");
    c.accept(s, &id, 1, Duration::ZERO, now).unwrap();
    assert!(c.snapshot.as_ref().unwrap().taps[32].silent);
    c.clear("context lost");
    assert!(c.snapshot.is_none());
    assert!(!c.clipped("input-01:raw", now));
}
#[test]
fn clip_hold_expires_after_one_second_without_repeated_window_relatching() {
    let mut s = snapshot("valid-over-range");
    let id = identity(&s);
    let now = Instant::now();
    let mut c = Cache::default();
    c.accept(s.clone(), &id, 1, Duration::ZERO, now).unwrap();
    for n in 1..=10 {
        s.sequence.0 += 1;
        s.query_id.0 += 1;
        s.first_frame.as_mut().unwrap().0 += 960;
        s.end_frame.as_mut().unwrap().0 += 960;
        for t in &mut s.taps {
            if t.clip_count.0 > 0 {
                t.clip_count.0 = 0;
                t.peak_millidbfs = Some(-6021);
                t.rms_millidbfs = Some(-6021);
                t.over_range = false;
            }
        }
        c.accept(
            s.clone(),
            &id,
            s.query_id.0,
            Duration::ZERO,
            now + Duration::from_millis(n * 100),
        )
        .unwrap();
    }
    assert!(c.fresh(now + Duration::from_millis(1000)));
    assert!(c.clipped("input-01:raw", now + Duration::from_millis(999)));
    assert!(!c.clipped("input-01:raw", now + Duration::from_millis(1000)));
}
#[test]
fn measured_scene_all_banks_buses_states_fit_and_share_native_primitives() {
    use shr_desk::{
        frontend::{Config, Frontend},
        render::Primitive,
    };
    let s = snapshot("valid-invalid");
    let id = identity(&s);
    let mut f = Frontend::new(Config {
        wire_version: 2,
        remote: None,
        endpoint: "/nonexistent/gp-meter-test.sock".into(),
        show: s.show_id.clone(),
        epoch: 1,
        writer: "meter-test".into(),
        scope: "foh".into(),
    });
    f.meter_page = true;
    f.meters
        .accept(s, &id, 1, Duration::ZERO, Instant::now())
        .unwrap();
    for bank in 0..4 {
        f.selected = bank * 12;
        for processed in [false, true] {
            f.meter_processed = processed;
            let scene = f.scene();
            assert!(scene.in_bounds());
            let text = scene
                .primitives
                .iter()
                .filter_map(|p| {
                    if let Primitive::Text { value, .. } = p {
                        Some(value.as_str())
                    } else {
                        None
                    }
                })
                .collect::<Vec<_>>();
            assert!(
                text.iter()
                    .any(|s| s.contains(&format!("input-{:02}", bank * 12 + 1)))
            );
            assert!(text.iter().any(|s| s.contains("monitor-7")));
            assert!(text.iter().any(|s| s.contains("Main pre-PA")));
            assert!(text.iter().any(|s| s.contains("GR")));
        }
    }
    f.selected = 47;
    let scene = f.scene();
    assert!(
        scene
            .primitives
            .iter()
            .any(|p| matches!(p,Primitive::Text{value,..}if value.contains("INVALID")))
    );
    f.meters.clear("unsupported");
    let scene = f.scene();
    assert!(
        scene
            .primitives
            .iter()
            .any(|p| matches!(p,Primitive::Text{value,..}if value.contains("UNAVAILABLE")))
    );
    // Monitor banks are independent of the four input banks.
    let mut s = snapshot("valid-silence");
    let template = s.taps.last().unwrap().clone();
    for n in s.monitors + 1..=13 {
        let mut tap = template.clone();
        tap.id = format!("monitor-{n}");
        s.taps.push(tap);
    }
    s.monitors = 13;
    let id = identity(&s);
    f.meters
        .accept(s, &id, 1, Duration::ZERO, Instant::now())
        .unwrap();
    f.meter_bus_bank = 1;
    let scene = f.scene();
    assert!(scene.in_bounds());
    assert!(
        scene
            .primitives
            .iter()
            .any(|p| matches!(p,Primitive::Text{value,..}if value.contains("monitor-13")))
    );
    assert!(
        !scene
            .primitives
            .iter()
            .any(|p| matches!(p,Primitive::Text{value,..}if value.starts_with("monitor-12 ")))
    );
}
#[test]
#[ignore = "explicit independently built GP_METER_PROVIDER; private finite synthetic source, optional CPU offscreen"]
fn actual_local_provider_native_scene() {
    use shr_desk::frontend::{Config, Frontend};
    use std::{os::unix::fs::PermissionsExt, process::Command};
    let provider =
        std::env::var("GP_METER_PROVIDER").expect("explicit independently built provider binary");
    for (inputs, monitors) in [(16, 3), (17, 1), (32, 5), (48, 7), (17, 13)] {
        let dir =
            std::env::temp_dir().join(format!("desk-gp-meter-{}-{inputs}", std::process::id()));
        std::fs::create_dir(&dir).unwrap();
        std::fs::set_permissions(&dir, std::fs::Permissions::from_mode(0o700)).unwrap();
        let show = "00000000-0000-0000-0000-000000000001";
        let template =
            std::env::var("GP_METER_TOPOLOGY_DIR").expect("explicit producer-created topologies");
        let topo = std::path::Path::new(&template).join(format!("{inputs}-{monitors}.json"));
        let mut child = Command::new(&provider)
            .args([
                "--directory",
                dir.to_str().unwrap(),
                "--show",
                show,
                "--epoch",
                "1",
                "--topology",
                topo.to_str().unwrap(),
                "--arm-synthetic",
                "--ticks",
                "10000",
            ])
            .spawn()
            .unwrap();
        let deadline = Instant::now() + Duration::from_secs(3);
        while !dir.join("audio.sock").exists() {
            assert!(Instant::now() < deadline);
            std::thread::sleep(Duration::from_millis(2));
        }
        let mut f = Frontend::new(Config {
            wire_version: 2,
            remote: None,
            endpoint: dir.join("audio.sock"),
            show: show.into(),
            epoch: 1,
            writer: "meter-driver".into(),
            scope: "foh".into(),
        });
        while !(f.meters.fresh(Instant::now())
            && f.fresh()
            && f.meters
                .snapshot
                .as_ref()
                .is_some_and(|s| s.first_frame.is_some_and(|n| n.0 >= 960)))
        {
            f.pump();
            assert!(Instant::now() < deadline, "{}", f.meters.message);
            std::thread::sleep(Duration::from_millis(2));
        }
        let s = f.meters.snapshot.as_ref().unwrap();
        assert_eq!(s.taps.len(), inputs * 2 + 2 + monitors);
        assert_eq!(s.taps[0].peak_millidbfs, Some(-18062));
        assert_eq!(s.taps[0].rms_millidbfs, Some(-18062));
        assert_eq!(s.taps[1].peak_millidbfs, Some(-18062));
        assert_eq!(s.taps[1].rms_millidbfs, Some(-18062));
        assert!(
            s.taps
                .iter()
                .all(|t| t.valid && t.clip_count.0 == 0 && t.invalid_count.0 == 0)
        );
        assert!(s.taps[(inputs - 1) * 2].silent);
        let main = 0.125 * 10f64.powf(-6. / 20.) * std::f64::consts::FRAC_1_SQRT_2;
        let expected = (20000. * main.log10()).round() as i32;
        assert_eq!(s.taps[inputs * 2].peak_millidbfs, Some(expected));
        assert_eq!(s.taps[inputs * 2].rms_millidbfs, Some(expected));
        for m in 0..monitors {
            let expected =
                (20000. * (0.125f64 * if m == 0 { 1. } else { 0.001 }).log10()).round() as i32;
            let tap = &s.taps[inputs * 2 + 2 + m];
            assert_eq!(tap.peak_millidbfs, Some(expected));
            assert_eq!(tap.rms_millidbfs, Some(expected));
        }
        assert_eq!(s.end_frame.unwrap().0 - s.first_frame.unwrap().0, 960);
        f.meter_page = true;
        f.meter_bus_bank = (monitors - 1) / 12;
        for selected in [0, inputs - 1] {
            f.selected = selected;
            for processed in [false, true] {
                f.meter_processed = processed;
                // CPU scene inspection pauses this UI pump. Await a new live
                // observation before freezing each scene; keep production ages/deadlines.
                let scene_deadline = Instant::now() + Duration::from_secs(3);
                loop {
                    f.pump();
                    if f.meters.fresh(Instant::now()) && f.fresh() {
                        break;
                    }
                    assert!(
                        Instant::now() < scene_deadline,
                        "fresh scene: meter={} control={:?}",
                        f.meters.message,
                        f.state.as_ref().map(|u| &u.status)
                    );
                    std::thread::sleep(Duration::from_millis(2));
                }
                let scene = f.scene();
                assert!(scene.in_bounds());
                assert!(scene.primitives.iter().any(|p| matches!(p,shr_desk::render::Primitive::Text{value,..}if value.starts_with(&format!("monitor-{monitors} ")))));
                if let Some(root) = std::env::var_os("GP_METER_EVIDENCE_DIR") {
                    let root = std::path::PathBuf::from(root);
                    std::fs::create_dir_all(&root).unwrap();
                    shr_desk::raster::ppm(
                        &scene,
                        &root.join(format!("{inputs}-{monitors}-{selected}-{processed}.ppm")),
                    )
                    .unwrap();
                }
                #[cfg(feature = "native")]
                if std::env::var_os("GP_METER_CPU").is_some() {
                    for (w, h) in [(1920, 1080), (1280, 720)] {
                        assert!(
                            shr_desk::native::offscreen_at(&scene, w, h)
                                .unwrap()
                                .contains("exact RGBA match")
                        );
                    }
                }
            }
        }
        // Stop only this child. A stopped source cannot remain live in the observer.
        if child.try_wait().unwrap().is_none() {
            child.kill().unwrap();
        }
        child.wait().unwrap();
        let stale_deadline = Instant::now() + Duration::from_millis(300);
        while Instant::now() < stale_deadline {
            f.pump();
            std::thread::sleep(Duration::from_millis(10));
        }
        assert!(!f.meters.fresh(Instant::now()));
        drop(f);
        std::fs::remove_dir_all(dir).unwrap();
    }
}
#[test]
fn meter_pages_keep_total_budget_identity_and_sixteen_page_capacity() {
    use serde_json::json;
    use sha2::{Digest, Sha256};
    use shr_desk::local_audio::AuthorityConnection;
    struct Frames(std::collections::VecDeque<Vec<u8>>);
    impl AuthorityConnection for Frames {
        fn send_frame_until(&mut self, _: &[u8], _: Instant) -> Result<(), String> {
            panic!("read-only assembly")
        }
        fn receive_until(&mut self, _: Instant) -> Result<Option<Vec<u8>>, String> {
            Ok(self.0.pop_front())
        }
        fn receive_available(&mut self) -> Result<Option<Vec<u8>>, String> {
            Ok(self.0.pop_front())
        }
    }
    // ASCII JSON slightly above one frame, split by the actual producer page size.
    let bytes = serde_json::to_vec(&json!({"padding":"x".repeat(70000)})).unwrap();
    let hash = format!("{:x}", Sha256::digest(&bytes));
    let chunks = bytes.chunks(8192).collect::<Vec<_>>();
    let pages=chunks.iter().enumerate().map(|(i,p)|serde_json::to_vec(&json!({"contract":"GP14-snapshot-pages","version":1,"identity":hash,"index":i,"count":chunks.len(),"total_bytes":bytes.len(),"payload":std::str::from_utf8(p).unwrap()})).unwrap()).collect::<std::collections::VecDeque<_>>();
    let result = Frames(pages.clone())
        .receive_meter_until(Instant::now() + Duration::from_millis(100))
        .unwrap()
        .unwrap();
    assert_eq!(result, bytes);
    let mut changed = pages.clone();
    let mut v: serde_json::Value = serde_json::from_slice(&changed[1]).unwrap();
    v["identity"] = "0".repeat(64).into();
    changed[1] = serde_json::to_vec(&v).unwrap();
    assert!(
        Frames(changed)
            .receive_meter_until(Instant::now() + Duration::from_millis(100))
            .is_err()
    );
    let mut many = pages.clone();
    let mut v: serde_json::Value = serde_json::from_slice(&many[0]).unwrap();
    v["count"] = 17.into();
    many[0] = serde_json::to_vec(&v).unwrap();
    assert!(
        Frames(many)
            .receive_meter_until(Instant::now() + Duration::from_millis(100))
            .is_err()
    );
    assert!(Frames(pages).receive_meter_until(Instant::now()).is_err());
}
