//! Opt-in actual frontend drivers. This test does not supply a mock provider or DSP.
use serde_json::json;
use shr_desk::{
    actions::Action,
    frontend::{Config, Event, Frontend},
    processing,
    render::Primitive,
};
use std::{
    fs,
    os::unix::fs::PermissionsExt,
    path::{Path, PathBuf},
    process::{Child, Command, Stdio},
    thread,
    time::{Duration, Instant},
};
const SHOW: &str = "11111111-1111-4111-8111-111111111111";
fn text(f: &Frontend) -> String {
    f.scene()
        .primitives
        .iter()
        .filter_map(|p| {
            if let Primitive::Text { value, .. } = p {
                Some(value.clone())
            } else {
                None
            }
        })
        .collect::<Vec<_>>()
        .join("\n")
}
fn snapshot(f: &Frontend) -> &processing::Snapshot {
    f.state.as_ref().unwrap().processing.as_ref().unwrap()
}
fn wait(f: &mut Frontend, deadline: Instant, label: &str, predicate: impl Fn(&Frontend) -> bool) {
    eprintln!("driver stage: {label}");
    let mut previous = String::new();
    loop {
        f.pump();
        let diagnostic = format!(
            "message={} status={} fresh={} structural={} processing={}",
            f.message,
            f.state.as_ref().map_or("no update", |u| u.status.as_str()),
            f.fresh(),
            f.state.as_ref().is_some_and(|u| u.structural_fresh),
            f.processing_ready()
        );
        if diagnostic != previous {
            eprintln!("driver {label}: {diagnostic}");
            previous = diagnostic;
        }
        if predicate(f) {
            eprintln!("driver stage complete: {label}");
            return;
        }
        assert!(
            Instant::now() < deadline,
            "GP07 deadline at {label}: message={} state={:?}",
            f.message,
            f.state
        );
        thread::sleep(Duration::from_millis(5));
    }
}
fn tap(f: &mut Frontend, key: &str) {
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
}
fn controller(f: &mut Frontend, action: Action) {
    let local = matches!(
        action,
        Action::ProcessingField(_) | Action::ProcessingText(_) | Action::ProcessingAdjust(_)
    );
    f.inject_controller(action).unwrap();
    f.pump();
    // Local fields share keyboard semantics and do not enqueue provider releases.
    // Allow actual context/authority gestures' release queue to drain.
    if !local {
        thread::sleep(Duration::from_millis(45));
        f.pump();
    }
}
fn field_keyboard(f: &mut Frontend, index: usize, value: &str) {
    assert!(index < processing::FIELDS.len());
    for _ in 0..processing::FIELDS.len() {
        assert!(
            f.processing_draft.is_some(),
            "draft cancelled during field navigation: {}",
            f.message
        );
        if f.processing_field == index {
            break;
        }
        tap(f, "I");
    }
    assert_eq!(
        f.processing_field, index,
        "field navigation did not progress: {}",
        f.message
    );
    for ch in value.chars() {
        tap(f, &ch.to_string());
    }
    tap(f, "Enter");
    assert!(
        f.processing_entry.is_empty(),
        "numeric entry refused: {}",
        f.message
    );
}
fn review_confirm(f: &mut Frontend, deadline: Instant, injected: bool, cpu: bool) {
    let reviewed_config = f.processing_draft.as_ref().unwrap().config.clone();
    if injected {
        controller(f, Action::ProcessingApply);
    } else {
        tap(f, "F4");
    }
    wait(f, deadline, "displayed review", |f| {
        f.state.as_ref().is_some_and(|u| u.review.is_some()) && f.processing_ready()
    });
    let mut visible = String::new();
    for page in 0..f.review_pages() {
        if page > 0 {
            tap(f, "PageDown");
        }
        let scene = f.scene();
        assert!(scene.in_bounds());
        let _pixels = shr_desk::raster::rgba(&scene);
        #[cfg(feature = "native")]
        if cpu {
            cpu_layout(&scene);
            if let Some(path) = std::env::var_os("GP07_REVIEW_SCENE_EVIDENCE") {
                shr_desk::raster::ppm(&scene, &PathBuf::from(path)).unwrap();
            }
        }
        #[cfg(not(feature = "native"))]
        let _ = cpu;
        visible.push_str(&text(f));
        let presented_id = f.state.as_ref().unwrap().review.as_ref().unwrap().0;
        wait(f, deadline, "fresh same rendered review", |f| {
            f.processing_ready()
                && f.state
                    .as_ref()
                    .and_then(|u| u.review.as_ref())
                    .is_some_and(|r| r.0 == presented_id)
        });
        f.mark_presented();
    }
    assert!(visible.contains("APPLY channel"));
    assert!(visible.contains("Compressor bypass"));
    for field in processing::FIELDS {
        let displayed = reviewed_config.display(field);
        assert!(
            visible.contains(&displayed),
            "unreviewable atomic field: {displayed}"
        );
    }
    assert!(visible.contains("240-frame"));
    if injected {
        controller(f, Action::Confirm);
    } else {
        tap(f, "Enter");
    }
}
#[cfg(feature = "native")]
fn cpu_layout(scene: &shr_desk::render::Scene) {
    for (width, height) in [(1920, 1080), (960, 540), (540, 960), (3840, 2160), (0, 0)] {
        let result = shr_desk::native::offscreen_at(scene, width, height).unwrap();
        eprintln!("{result}");
    }
}
fn entries(c: &processing::Config) -> Vec<String> {
    vec![
        u8::from(c.eq_bypass).to_string(),
        c.band1_hz.to_string(),
        format!("{:.3}", c.band1_gain_mdb as f64 / 1000.0),
        format!("{:.3}", c.band1_q_milli as f64 / 1000.0),
        u8::from(c.band1_bypass).to_string(),
        c.band2_hz.to_string(),
        format!("{:.3}", c.band2_gain_mdb as f64 / 1000.0),
        format!("{:.3}", c.band2_q_milli as f64 / 1000.0),
        u8::from(c.band2_bypass).to_string(),
        c.band3_hz.to_string(),
        format!("{:.3}", c.band3_gain_mdb as f64 / 1000.0),
        format!("{:.3}", c.band3_q_milli as f64 / 1000.0),
        u8::from(c.band3_bypass).to_string(),
        c.band4_hz.to_string(),
        format!("{:.3}", c.band4_gain_mdb as f64 / 1000.0),
        format!("{:.3}", c.band4_q_milli as f64 / 1000.0),
        u8::from(c.band4_bypass).to_string(),
        u8::from(c.compressor_bypass).to_string(),
        format!("{:.3}", c.threshold_mdb as f64 / 1000.0),
        format!("{:.3}", c.ratio_milli as f64 / 1000.0),
        format!("{:.3}", c.knee_mdb as f64 / 1000.0),
        format!("{:.3}", c.attack_us as f64 / 1000.0),
        c.release_ms.to_string(),
        format!("{:.3}", c.makeup_mdb as f64 / 1000.0),
    ]
}
fn run_driver(endpoint: &Path, epoch: u64, evidence: Option<&Path>, cpu: bool) {
    assert!(endpoint.is_absolute());
    let start = Instant::now();
    let deadline = start + Duration::from_secs(55);
    let mut f = Frontend::new(Config {
        wire_version: 1,
        remote: None,
        endpoint: endpoint.to_path_buf(),
        show: SHOW.into(),
        epoch,
        writer: format!("desk-gp07-{}", std::process::id()),
        scope: "foh".into(),
    });
    f.enable_processing().unwrap();
    wait(
        &mut f,
        deadline,
        "fresh capability",
        Frontend::processing_ready,
    );
    let initial = snapshot(&f).clone();
    tap(&mut f, "G");
    wait(&mut f, deadline, "explicit FOH grant", |f| {
        f.state
            .as_ref()
            .is_some_and(|u| u.status.starts_with("grant applied"))
    });
    tap(&mut f, "F2");
    wait(
        &mut f,
        deadline,
        "Channel context",
        Frontend::processing_ready,
    );
    let neutral = initial.channels[0].target.clone();
    let mut planned = Vec::new();
    for band in 1..=4 {
        for (variant, hz, gain, q) in [
            ("a", [200, 700, 2500, 6000][band - 1], 6000, 700),
            ("b", [350, 1100, 4000, 9000][band - 1], -5000, 2300),
        ] {
            let mut v = serde_json::to_value(&neutral).unwrap();
            v["eq_bypass"] = json!(false);
            v[format!("band{band}_hz")] = json!(hz);
            v[format!("band{band}_gain_mdb")] = json!(gain);
            v[format!("band{band}_q_milli")] = json!(q);
            planned.push((
                format!("band{band}-{variant}"),
                1usize,
                processing::decode_config(&v).unwrap(),
            ));
        }
    }
    let mut combined = neutral.clone();
    combined.eq_bypass = false;
    combined.band1_hz = 6000;
    combined.band1_gain_mdb = 4000;
    combined.band1_q_milli = 700;
    combined.band2_hz = 350;
    combined.band2_gain_mdb = -3000;
    combined.band2_q_milli = 2300;
    combined.band3_hz = 9000;
    combined.band3_gain_mdb = 5000;
    combined.band3_q_milli = 1200;
    combined.band4_hz = 1100;
    combined.band4_gain_mdb = -4000;
    combined.band4_q_milli = 1800;
    planned.push(("crossed-cascade".into(), 1, combined.clone()));
    for band in 1..=4 {
        let mut v = serde_json::to_value(&combined).unwrap();
        v[format!("band{band}_bypass")] = json!(true);
        planned.push((
            format!("band{band}-bypass"),
            1,
            processing::decode_config(&v).unwrap(),
        ));
    }
    let mut bypass = combined.clone();
    bypass.eq_bypass = true;
    planned.push(("global-bypass".into(), 1, bypass));
    let mut enabled_neutral = neutral.clone();
    enabled_neutral.eq_bypass = false;
    planned.push(("neutral".into(), 1, enabled_neutral));
    let mut channel1 = combined.clone();
    channel1.compressor_bypass = false;
    channel1.threshold_mdb = -48000;
    channel1.ratio_milli = 4000;
    channel1.makeup_mdb = 3000;
    planned.push(("compressor".into(), 1, channel1.clone()));
    let mut channel2 = neutral.clone();
    channel2.eq_bypass = false;
    channel2.band3_hz = 1700;
    channel2.band3_gain_mdb = -6000;
    channel2.band3_q_milli = 1900;
    planned.push(("channel2".into(), 2, channel2.clone()));
    let mut edits = Vec::new();
    let mut actions = Vec::new();
    let mut first_revision = String::new();
    let mut positive_gr = None;
    for (edit_index, (label, channel, config)) in planned.into_iter().enumerate() {
        if f.selected != channel - 1 {
            let delta = channel as i32 - 1 - f.selected as i32;
            controller(&mut f, Action::Move(delta));
            wait(
                &mut f,
                deadline,
                "channel context",
                Frontend::processing_ready,
            );
        }
        let injected = edit_index != 0;
        if injected {
            controller(&mut f, Action::ProcessingEdit);
        } else {
            tap(&mut f, "E");
        }
        assert!(f.processing_draft.is_some(), "{}", f.message);
        let before = snapshot(&f).channels[channel - 1].target.clone();
        let previous = entries(&before);
        for (index, value) in entries(&config).into_iter().enumerate() {
            if previous[index] == value {
                continue;
            }
            if injected {
                let delta = index as i32 - f.processing_field as i32;
                controller(&mut f, Action::ProcessingField(delta));
                controller(&mut f, Action::ProcessingText(value.clone()));
            } else {
                field_keyboard(&mut f, index, &value);
            }
            actions.push(json!({"edit":label,"field":format!("{:?}",processing::FIELDS[index]),"value":value,"path":if injected {"injected semantic action"} else {"keyboard semantic action"}}));
        }
        assert!(
            f.processing_draft.is_some(),
            "{label}: draft cancelled: {} / {:?}",
            f.message,
            f.state.as_ref().map(|s| (&s.status, s.processing_age_ms))
        );
        assert_eq!(
            f.processing_draft.as_ref().unwrap().config,
            config,
            "{label}: {}",
            f.message
        );
        assert_eq!(
            snapshot(&f).channels[channel - 1].target,
            before,
            "draft must not write"
        );
        assert!(text(&f).contains("LOCAL DRAFT"));
        review_confirm(&mut f, deadline, injected, cpu && edit_index == 0);
        wait(&mut f, deadline, &label, |f| {
            f.processing_ready()
                && snapshot(f).channels[channel - 1].current == config
                && f.state
                    .as_ref()
                    .and_then(|u| u.processing_final.as_ref())
                    .is_some_and(|r| r.revision == snapshot(f).revision)
        });
        let final_reply = f.state.as_ref().unwrap().processing_final.as_ref().unwrap();
        assert!(final_reply.reason.is_none() && final_reply.state == "final");
        let frame = final_reply
            .effective_frame
            .clone()
            .expect("actual final boundary required");
        edits
            .push(json!({"label":label,"channel":channel,"config":config,"effective_frame":frame}));
        if edit_index == 0 {
            first_revision = snapshot(&f).revision.clone();
        }
        let settled = snapshot(&f).frame.parse::<u64>().unwrap();
        wait(&mut f, deadline, "576 source frame settled dwell", |f| {
            f.processing_ready() && snapshot(f).frame.parse::<u64>().unwrap() >= settled + 576
        });
        if label == "compressor" {
            wait(&mut f, deadline, "positive compressor GR", |f| {
                snapshot(f).channels[0]
                    .gain_reduction_mdb
                    .is_some_and(|n| n > 0)
            });
            positive_gr = snapshot(&f).channels[0].gain_reduction_mdb;
        }
        assert_eq!(snapshot(&f).channels[2..], initial.channels[2..]);
        eprintln!(
            "GP07 edit {label} final boundary {frame} revision {}",
            snapshot(&f).revision
        );
        // Preserve ordered useful evidence even if a later acceptance assertion fails.
        if let Some(path) = evidence {
            fs::write(
                path,
                serde_json::to_vec_pretty(
                    &json!({"state":"in-progress","edits":edits,"actions":actions}),
                )
                .unwrap(),
            )
            .unwrap();
        }
    }
    let second = snapshot(&f).clone();
    assert_eq!(second.channels[0].current, channel1);
    assert_eq!(second.channels[1].current, channel2);
    controller(&mut f, Action::ProcessingEdit);
    controller(&mut f, Action::ProcessingField(2));
    controller(&mut f, Action::ProcessingText("12".into()));
    controller(&mut f, Action::Move(-1));
    wait(
        &mut f,
        deadline,
        "cancelled context",
        Frontend::processing_ready,
    );
    assert!(f.processing_draft.is_none());
    assert_eq!(snapshot(&f).revision, second.revision);
    tap(&mut f, "E");
    field_keyboard(&mut f, 2, "-6");
    tap(&mut f, "F4");
    wait(&mut f, deadline, "review before reconnect", |f| {
        f.state.as_ref().is_some_and(|u| u.review.is_some())
    });
    let _ = shr_desk::raster::rgba(&f.scene());
    f.mark_presented();
    tap(&mut f, "F5");
    wait(&mut f, deadline, "fresh reconnect", |f| {
        f.processing_ready()
            && f.state
                .as_ref()
                .is_some_and(|u| u.status.contains("read-only") && u.review.is_none())
    });
    assert_eq!(snapshot(&f).revision, second.revision);
    assert_eq!(snapshot(&f).channels[0].current, channel1);
    assert_eq!(snapshot(&f).channels[1].current, channel2);
    assert!(
        f.state.as_ref().unwrap().processing_final.is_none(),
        "reconnect must discard old completion"
    );
    tap(&mut f, "Enter");
    assert!(f.message.contains("no displayed"));
    let final_snapshot = snapshot(&f).clone();
    assert!(f.scene().in_bounds());
    if let Some(path) = std::env::var_os("GP07_SCENE_EVIDENCE") {
        shr_desk::raster::ppm(&f.scene(), &PathBuf::from(path)).unwrap();
    }
    #[cfg(feature = "native")]
    if cpu {
        cpu_layout(&f.scene());
    }
    #[cfg(not(feature = "native"))]
    let _ = cpu;
    if let Some(path) = evidence {
        fs::write(path, serde_json::to_vec_pretty(&json!({"kind":"gp07-real-frontend-driver","show":SHOW,"epoch":epoch,"initial_revision":initial.revision,"keyboard_revision":first_revision,"controller_revision":second.revision,"channel1_gain_reduction_mdb":positive_gr,"reconnect_revision":final_snapshot.revision,"channel1":channel1,"channel2":channel2,"context_cancel":true,"reconnect_no_replay":true,"visible_scene":true,"edits":edits,"actions":actions,"elapsed_ms":start.elapsed().as_millis(),"scope":"operator/provider readback; host owns sample/REC/analysis assertions"})).unwrap()).unwrap();
    }
    drop(f);
    assert!(
        start.elapsed() < Duration::from_secs(60),
        "driver exceeded 60s bound"
    );
    eprintln!(
        "GP07 success frontend dropped elapsed_ms={}",
        start.elapsed().as_millis()
    );
}
#[test]
#[ignore = "requires externally hosted real LocalAudio at GP07_EXTERNAL_ENDPOINT; host owns samples/modules"]
fn gp07_external_driver() {
    let endpoint = PathBuf::from(
        std::env::var_os("GP07_EXTERNAL_ENDPOINT").expect("absolute external endpoint required"),
    );
    let evidence = std::env::var_os("GP07_DRIVER_EVIDENCE").map(PathBuf::from);
    run_driver(&endpoint, 100, evidence.as_deref(), false);
}
struct Service {
    child: Child,
    dir: PathBuf,
}
impl Drop for Service {
    fn drop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
        fs::remove_dir_all(&self.dir).unwrap();
    }
}
#[test]
#[ignore = "requires independently accepted real release provider through SHR_DESK_GP07"]
fn gp07_actual_release_provider() {
    let binary =
        std::env::var_os("SHR_DESK_GP07").expect("accepted release provider path required");
    let service = launch_service(&binary);
    let dir = service.dir.clone();
    let evidence = std::env::var_os("GP07_DRIVER_EVIDENCE").map(PathBuf::from);
    run_driver(
        &dir.join("audio.sock"),
        100,
        evidence.as_deref(),
        std::env::var("VK_DRIVER_FILES").is_ok_and(|v| v == "/usr/share/vulkan/icd.d/lvp_icd.json"),
    );
}

fn launch_service(binary: &std::ffi::OsStr) -> Service {
    let dir = std::env::temp_dir().join(format!(
        "desk-gp07-{}-{}",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    ));
    fs::create_dir(&dir).unwrap();
    fs::set_permissions(&dir, fs::Permissions::from_mode(0o700)).unwrap();
    let child = Command::new(binary)
        .args([
            "--directory",
            dir.to_str().unwrap(),
            "--show",
            SHOW,
            "--epoch",
            "100",
            "--ticks",
            "60000",
        ])
        .stdout(Stdio::null())
        .stderr(Stdio::inherit())
        .spawn()
        .unwrap();
    let mut service = Service {
        child,
        dir: dir.clone(),
    };
    let end = Instant::now() + Duration::from_secs(5);
    while !dir.join("audio.sock").exists() {
        assert!(service.child.try_wait().unwrap().is_none());
        assert!(Instant::now() < end);
        thread::sleep(Duration::from_millis(10));
    }
    service
}

#[test]
#[ignore = "requires hash-verified unchanged v1 release provider through SHR_DESK_GP07_LEGACY"]
fn gp07_legacy_disconnect_requires_explicit_gp03_reconnect() {
    let binary = std::env::var_os("SHR_DESK_GP07_LEGACY").expect("legacy provider required");
    let service = launch_service(&binary);
    let deadline = Instant::now() + Duration::from_secs(10);
    let mut f = Frontend::new(Config {
        wire_version: 1,
        remote: None,
        endpoint: service.dir.join("audio.sock"),
        show: SHOW.into(),
        epoch: 100,
        writer: "desk-v2-legacy-check".into(),
        scope: "foh".into(),
    });
    wait(
        &mut f,
        deadline,
        "ordinary GP03 attachment",
        Frontend::fresh,
    );
    let revision = f
        .state
        .as_ref()
        .unwrap()
        .snapshot
        .as_ref()
        .unwrap()
        .authority
        .revision
        .clone();
    f.enable_processing().unwrap();
    wait(&mut f, deadline, "old provider disconnect observed", |f| {
        f.state
            .as_ref()
            .is_some_and(|s| !s.fresh && s.processing_status.contains("UNAVAILABLE"))
    });
    let failure = f.state.as_ref().unwrap().processing_status.clone();
    assert!(
        !failure.contains("unsupported_version"),
        "disconnect alone is not version proof"
    );
    assert!(!f.processing_ready());
    eprintln!("Observed legacy probe failure: {failure}");
    tap(&mut f, "F8");
    wait(&mut f, deadline, "explicit GP03-only reconnect", |f| {
        f.fresh()
            && f.state.as_ref().is_some_and(|s| {
                s.status.contains("read-only") && s.processing_status.contains("explicit legacy")
            })
    });
    assert!(f.state.as_ref().unwrap().processing.is_none());
    assert_eq!(
        f.state
            .as_ref()
            .unwrap()
            .snapshot
            .as_ref()
            .unwrap()
            .authority
            .revision,
        revision
    );
    tap(&mut f, "G");
    wait(&mut f, deadline, "legacy GP03 grant", |f| {
        f.state
            .as_ref()
            .is_some_and(|s| s.status.starts_with("grant applied"))
    });
    tap(&mut f, "M");
    wait(&mut f, deadline, "legacy GP03 protected review", |f| {
        f.state.as_ref().is_some_and(|s| s.review.is_some())
    });
    let _ = shr_desk::raster::rgba(&f.scene());
    f.mark_presented();
    tap(&mut f, "Enter");
    wait(&mut f, deadline, "legacy GP03 confirmation", |f| {
        f.state
            .as_ref()
            .is_some_and(|s| s.status.starts_with("set applied"))
    });
    assert!(f.state.as_ref().unwrap().processing.is_none());
    drop(f);
    drop(service);
}

#[test]
#[ignore = "requires actual dynamic LocalAudio at GP14_EXTERNAL_ENDPOINT; coordinator owns sample/module acceptance"]
fn gp14_high_channel_external_driver() {
    eprintln!("GP14 high-channel driver start (45s internal deadline)");
    let endpoint = PathBuf::from(std::env::var_os("GP14_EXTERNAL_ENDPOINT").unwrap());
    let epoch: u64 = std::env::var("GP14_EPOCH").unwrap().parse().unwrap();
    let deadline = Instant::now() + Duration::from_secs(45);
    let mut f = Frontend::new(Config {
        wire_version: 2,
        remote: std::env::var_os("GP14_REMOTE_CONFIG")
            .map(|p| serde_json::from_slice(&fs::read(p).unwrap()).unwrap()),
        endpoint,
        show: SHOW.into(),
        epoch,
        writer: format!("desk-gp14-{}", std::process::id()),
        scope: "foh".into(),
    });
    f.enable_processing().unwrap();
    wait(
        &mut f,
        deadline,
        "dynamic ready",
        Frontend::processing_ready,
    );
    assert!(snapshot(&f).channels.len() >= 48);
    tap(&mut f, "G");
    wait(&mut f, deadline, "dynamic FOH grant", |f| {
        f.state
            .as_ref()
            .is_some_and(|u| u.status.starts_with("grant applied"))
    });
    tap(&mut f, "F2");
    wait(
        &mut f,
        deadline,
        "dynamic Channel",
        Frontend::processing_ready,
    );
    let mut evidence = Vec::new();
    for (index, number) in [16usize, 17, 32, 33, 48].into_iter().enumerate() {
        let delta = number as i32 - 1 - f.selected as i32;
        if index % 2 == 0 {
            for _ in 0..delta.unsigned_abs() {
                tap(&mut f, if delta > 0 { "Right" } else { "Left" });
                wait(
                    &mut f,
                    deadline,
                    "keyboard selection",
                    Frontend::processing_ready,
                );
            }
        } else {
            controller(&mut f, Action::Move(delta));
        }
        wait(
            &mut f,
            deadline,
            "selected high channel",
            Frontend::processing_ready,
        );
        assert_eq!(f.selected, number - 1);
        let id = format!("input-{number:02}");
        let before = snapshot(&f).clone();
        if index % 2 == 0 {
            tap(&mut f, "E");
        } else {
            controller(&mut f, Action::ProcessingEdit);
        }
        assert_eq!(f.processing_draft.as_ref().unwrap().input, id);
        let mut desired = f.processing_draft.as_ref().unwrap().config.clone();
        desired.eq_bypass = false;
        desired.band1_bypass = false;
        desired.band2_bypass = false;
        desired.band3_bypass = false;
        desired.band4_bypass = false;
        desired.band1_hz = 150 + number as i32;
        desired.band1_gain_mdb = 1000;
        desired.band1_q_milli = 800;
        desired.band2_hz = 600 + number as i32;
        desired.band2_gain_mdb = -2000;
        desired.band2_q_milli = 1100;
        desired.band3_hz = 2200 + number as i32;
        desired.band3_gain_mdb = 3000;
        desired.band3_q_milli = 1400;
        desired.band4_hz = 8000 + number as i32;
        desired.band4_gain_mdb = -1000;
        desired.band4_q_milli = 1800;
        desired.compressor_bypass = false;
        desired.threshold_mdb = -24000;
        desired.ratio_milli = 3000;
        desired.knee_mdb = 3000;
        desired.attack_us = 5000;
        desired.release_ms = 120;
        desired.makeup_mdb = 1000;
        for (field, value) in entries(&desired).into_iter().enumerate() {
            if index % 2 == 0 {
                field_keyboard(&mut f, field, &value);
            } else {
                let delta = field as i32 - f.processing_field as i32;
                controller(&mut f, Action::ProcessingField(delta));
                controller(&mut f, Action::ProcessingText(value));
            }
        }
        assert_eq!(f.processing_draft.as_ref().unwrap().config, desired);
        review_confirm(&mut f, deadline, index % 2 != 0, false);
        wait(&mut f, deadline, "high-channel applied and settled", |f| {
            f.processing_ready()
                && f.state
                    .as_ref()
                    .and_then(|u| u.processing.as_ref())
                    .is_some_and(|s| {
                        s.revision != before.revision
                            && s.channels
                                .iter()
                                .any(|c| c.input == id && c.target == desired && c.ready)
                    })
        });
        let after = snapshot(&f).clone();
        for old in &before.channels {
            if old.input != id {
                assert_eq!(
                    after
                        .channels
                        .iter()
                        .find(|c| c.input == old.input)
                        .unwrap()
                        .target,
                    old.target
                );
            }
        }
        evidence.push(json!({"input":id,"actions":24,"path":if index%2==0 {"keyboard"} else {"injected"},
            "config":desired,"revision":after.revision,"final":f.state.as_ref().unwrap().processing_final}));
    }
    let final_state = snapshot(&f).clone();
    tap(&mut f, "F5");
    wait(
        &mut f,
        deadline,
        "reconnect read-only",
        Frontend::processing_ready,
    );
    assert_eq!(snapshot(&f).revision, final_state.revision);
    assert_eq!(
        snapshot(&f)
            .channels
            .iter()
            .map(|c| &c.target)
            .collect::<Vec<_>>(),
        final_state
            .channels
            .iter()
            .map(|c| &c.target)
            .collect::<Vec<_>>()
    );
    if let Some(path) = std::env::var_os("GP14_DRIVER_EVIDENCE") {
        fs::write(
            path,
            serde_json::to_vec_pretty(
                &json!({"edits":evidence,"reconnect_revision":final_state.revision}),
            )
            .unwrap(),
        )
        .unwrap();
    }
}

#[test]
#[ignore = "requires actual integrated dynamic provider/PA; GP14_EXTERNAL_ENDPOINT,GP14_EPOCH and optional GP14_REMOTE_CONFIG"]
fn gp14_structural_external_driver() {
    eprintln!("GP14 structural driver start (45s internal deadline)");
    let endpoint = PathBuf::from(std::env::var_os("GP14_EXTERNAL_ENDPOINT").unwrap());
    let epoch: u64 = std::env::var("GP14_EPOCH").unwrap().parse().unwrap();
    let remote = std::env::var_os("GP14_REMOTE_CONFIG").map(|p| {
        serde_json::from_slice::<shr_desk::remote::Config>(&fs::read(p).unwrap()).unwrap()
    });
    let deadline = Instant::now() + Duration::from_secs(45);
    let make = |scope: &str| {
        Frontend::new(Config {
            wire_version: 2,
            remote: remote.clone(),
            endpoint: endpoint.clone(),
            show: SHOW.into(),
            epoch,
            writer: format!("desk-structure-{}-{scope}", std::process::id()),
            scope: scope.into(),
        })
    };
    let ready = |f: &Frontend| f.fresh() && f.state.as_ref().is_some_and(|u| u.structural_fresh);
    let confirm = |f: &mut Frontend| {
        wait(f, deadline, "structural review", |f| {
            ready(f) && f.state.as_ref().is_some_and(|u| u.review.is_some())
        });
        for page in 0..f.review_pages() {
            if page > 0 {
                tap(f, "PageDown");
            }
            assert!(f.scene().in_bounds());
            let _ = shr_desk::raster::rgba(&f.scene());
            f.mark_presented();
        }
        let revision = f
            .state
            .as_ref()
            .unwrap()
            .snapshot
            .as_ref()
            .unwrap()
            .authority
            .revision
            .clone();
        tap(f, "Enter");
        wait(f, deadline, "structural final", |f| {
            f.state
                .as_ref()
                .and_then(|u| u.structural_final.as_ref())
                .is_some_and(|r| r.reason.is_none() && r.revision != revision)
        });
    };
    let grant = |f: &mut Frontend| {
        wait(f, deadline, "structural attach", ready);
        tap(f, "G");
        wait(f, deadline, "separate structural grant", |f| {
            ready(f)
                && f.state
                    .as_ref()
                    .is_some_and(|u| u.status.starts_with("grant applied"))
        });
    };
    let mut actions = Vec::new();
    let mut pa = make("pa_configuration");
    grant(&mut pa);
    tap(&mut pa, "X");
    confirm(&mut pa);
    actions
        .push(json!({"kind":"baseline_rearm","final":pa.state.as_ref().unwrap().structural_final}));
    thread::sleep(Duration::from_millis(150));
    wait(&mut pa, deadline, "baseline rearm readback", ready);
    tap(&mut pa, "Z");
    confirm(&mut pa);
    actions.push(json!({"kind":"quiesce","final":pa.state.as_ref().unwrap().structural_final}));
    wait(&mut pa, deadline, "quiesced readback", ready);
    assert!(
        pa.state
            .as_ref()
            .unwrap()
            .structural
            .as_ref()
            .unwrap()
            .outputs_quiesced
    );
    tap(&mut pa, "F9");
    let field = pa
        .structural_draft
        .as_ref()
        .unwrap()
        .fields
        .iter()
        .position(|p| p.ends_with("/gain_db"))
        .expect("actual PA gain_db field required");
    controller(&mut pa, Action::StructureField(field as i32));
    controller(&mut pa, Action::StructureAdjust(-1));
    // Exercise full owner-document import through the same reviewed action path.
    let mut imported = pa.structural_draft.as_ref().unwrap().document.clone();
    let weighted_node = json!({"routes":[{"source":{"input":0},"weight":0.5},{"source":{"input":1},"weight":0.25}]});
    imported["configuration"]["nodes"] = json!([weighted_node]);
    imported["configuration"]["outputs"][0]["source"] = json!({"node":0});
    let desired_owner = imported["configuration"].clone();
    let imported = serde_json::to_string(&imported).unwrap();
    controller(&mut pa, Action::StructureImport(imported));
    controller(&mut pa, Action::StructureApply);
    confirm(&mut pa);
    actions.push(json!({"kind":"pa_set","final":pa.state.as_ref().unwrap().structural_final}));
    wait(&mut pa, deadline, "PA replacement readback", ready);
    let pa_readback = pa.state.as_ref().unwrap().structural.clone().unwrap();
    let confirmed_owner: serde_json::Value =
        serde_json::from_str(pa_readback.pa_configuration_json.as_ref().unwrap()).unwrap();
    assert_eq!(
        confirmed_owner, desired_owner,
        "complete imported gain and weighted route readback"
    );
    assert_eq!(pa_readback.pa_program_buses, vec![0, 1]);
    tap(&mut pa, "Q");
    wait(&mut pa, deadline, "PA lease released", |f| {
        f.state
            .as_ref()
            .is_some_and(|u| u.status.starts_with("release applied"))
    });
    drop(pa);
    let mut patch = make("output_routes");
    grant(&mut patch);
    assert!(
        patch
            .state
            .as_ref()
            .unwrap()
            .structural
            .as_ref()
            .unwrap()
            .topology
            .pa_outputs
            > 0
    );
    let outputs = &patch
        .state
        .as_ref()
        .unwrap()
        .structural
        .as_ref()
        .unwrap()
        .topology
        .outputs;
    let (patch_index, chosen) = outputs
        .iter()
        .enumerate()
        .skip(1)
        .find(|(_, output)| {
            output.source != Some(shr_desk::topology::OutputSource::Pa { index: 0 })
        })
        .expect("an additional physical port with a different source is required");
    let before_source = chosen.source.clone();
    let patch_port = chosen.id.clone();
    let target_index = 0;
    tap(&mut patch, "F9");
    controller(&mut patch, Action::StructureField(patch_index as i32));
    controller(
        &mut patch,
        Action::StructureText(json!({"kind":"pa","index":target_index}).to_string()),
    );
    tap(&mut patch, "F4");
    confirm(&mut patch);
    actions.push(
        json!({"kind":"output_patch","port_index":patch_index,"port_id":patch_port,"before_source":before_source,"after_source":{"kind":"pa","index":target_index},"final":patch.state.as_ref().unwrap().structural_final}),
    );
    let patch_revision = patch
        .state
        .as_ref()
        .unwrap()
        .structural_final
        .as_ref()
        .unwrap()
        .revision
        .clone();
    tap(&mut patch, "F5");
    wait(
        &mut patch,
        deadline,
        "fresh map readback without replay",
        ready,
    );
    let patch_readback = patch.state.as_ref().unwrap().structural.clone().unwrap();
    assert_eq!(patch_readback.revision, patch_revision);
    assert_eq!(
        patch_readback.topology.outputs[patch_index].source,
        Some(shr_desk::topology::OutputSource::Pa {
            index: target_index
        })
    );
    assert_ne!(
        patch_readback.topology.outputs[patch_index].source,
        before_source
    );
    assert!(patch_readback.outputs_quiesced);
    drop(patch);
    let mut rearm = make("pa_configuration");
    grant(&mut rearm);
    tap(&mut rearm, "X");
    confirm(&mut rearm);
    actions
        .push(json!({"kind":"final_rearm","final":rearm.state.as_ref().unwrap().structural_final}));
    thread::sleep(Duration::from_millis(150));
    wait(&mut rearm, deadline, "final rearm source advance", ready);
    let final_readback = rearm.state.as_ref().unwrap().structural.clone().unwrap();
    assert!(!final_readback.outputs_quiesced);
    if let Some(path) = std::env::var_os("GP14_STRUCTURE_EVIDENCE") {
        fs::write(
            path,
            serde_json::to_vec_pretty(
                &json!({"actions":actions,"pa":pa_readback,"patch":patch_readback,"rearmed":final_readback}),
            )
            .unwrap(),
        )
        .unwrap();
    }
}
