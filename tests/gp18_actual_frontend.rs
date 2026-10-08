//! Explicit actual Frontend/worker + hash-pinned provider/owner PCM acceptance.
//! External executable/library dependency; never a normal test or hardware claim.
use serde_json::{Value, json};
use sha2::{Digest, Sha256};
use shr_desk::{
    actions::Action,
    frontend::{Config, Event, Frontend},
    model::{Mode, Page},
    sends::Tap,
};
use std::{
    fs,
    io::Read,
    path::{Path, PathBuf},
    process::{Child, Command, Stdio},
    thread,
    time::{Duration, Instant},
};
const BINARY: &str = "5695633c64bffa5e96da62703fc9eb1610129332691ff673a2f7f76798965b4f";
const PA_FIXTURE: &str = "43d0de88d05261feb8587ddc3d5d3b6e3e8378ed9be42746219866daffb7d3cc";
const MANIFEST: &str = "521f23ea2354fd60ca8598d95fb2306a92e968f182dc72bb8f46b85c21313fb9";
fn hash(p: &Path) -> String {
    let mut f = fs::File::open(p).unwrap();
    let mut h = Sha256::new();
    let mut b = [0; 16384];
    loop {
        let n = f.read(&mut b).unwrap();
        if n == 0 {
            break;
        }
        h.update(&b[..n]);
    }
    format!("{:x}", h.finalize())
}
fn read(p: &Path) -> Value {
    serde_json::from_slice(&fs::read(p).unwrap()).unwrap()
}
struct Host {
    child: Child,
    dir: PathBuf,
}
impl Drop for Host {
    fn drop(&mut self) {
        use std::os::unix::process::ExitStatusExt;
        let mut killed_by_driver = false;
        let status = match self.child.try_wait() {
            Ok(Some(status)) => Some(status),
            _ => {
                killed_by_driver = true;
                let _ = self.child.kill();
                self.child.wait().ok()
            }
        };
        let result = json!({"killed_by_driver":killed_by_driver,"code":status.as_ref().and_then(|s| s.code()),"signal":status.as_ref().and_then(|s| s.signal()),"success":status.as_ref().is_some_and(|s| s.success())});
        let _ = fs::write(
            self.dir.join("host-exit.json"),
            serde_json::to_vec_pretty(&result).unwrap(),
        );
    }
}
fn wait(f: &mut Frontend, end: Instant, label: &str, ready: impl Fn(&Frontend) -> bool) {
    loop {
        f.pump();
        if ready(f) {
            return;
        }
        assert!(
            !f.state
                .as_ref()
                .is_some_and(|u| u.snapshot.is_none() && u.status.starts_with("STALE/UNCERTAIN:")),
            "attachment lost during {label}: {:?}",
            f.state
        );
        assert!(
            Instant::now() < end,
            "{label}: {} / {:?}",
            f.message,
            f.state
        );
        thread::sleep(Duration::from_millis(5));
    }
}
fn tap(f: &mut Frontend, key: &str) {
    for pressed in [true, false] {
        f.enqueue(Event::Key {
            key: key.into(),
            pressed,
        })
        .unwrap();
        f.pump();
    }
}
fn action(f: &mut Frontend, a: Action) {
    f.inject_controller(a).unwrap();
    f.pump();
}
fn present(f: &mut Frontend) {
    f.synchronize_review();
    let scene = f.scene();
    assert!(scene.in_bounds());
    let pixels = shr_desk::raster::rgba(&scene);
    assert_eq!(pixels.len(), 1920 * 1080 * 4);
    if let Some(root) = std::env::var_os("GP18_DRIVER_EVIDENCE") {
        static SCENE: std::sync::atomic::AtomicUsize = std::sync::atomic::AtomicUsize::new(0);
        let index = SCENE.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        let dir = PathBuf::from(root).join("presented-reviews");
        fs::create_dir_all(&dir).unwrap();
        let name = format!(
            "review-{index:03}{}",
            if f.live_page { "-live" } else { "" }
        );
        fs::write(
            dir.join(format!("{name}.svg")),
            shr_desk::render::svg(&scene),
        )
        .unwrap();
        if f.live_page {
            shr_desk::raster::ppm(&scene, &dir.join(format!("{name}.ppm"))).unwrap();
        }
    }
    f.mark_presented();
}
fn pa_lanes(
    configuration: &Value,
    indices: [usize; 2],
    slots: &[usize],
) -> (Vec<usize>, Vec<usize>) {
    assert_eq!(
        configuration["outputs"].as_array().unwrap().len(),
        slots.len()
    );
    let mut left = Vec::new();
    let mut right = Vec::new();
    for (output, slot) in configuration["outputs"]
        .as_array()
        .unwrap()
        .iter()
        .zip(slots)
    {
        let input = output["source"]["input"]
            .as_u64()
            .expect("fixture direct owner input") as usize;
        if input == indices[0] {
            left.push(*slot);
        } else {
            assert_eq!(input, indices[1], "unexpected owner route");
            right.push(*slot);
        }
    }
    (left, right)
}
#[test]
fn pa_pcm_lanes_follow_interleaved_and_nonadjacent_owner_routes() {
    for indices in [[0, 1], [2, 0]] {
        let configuration = json!({"outputs": (0..6).map(|i| json!({"source":{"input":indices[i % 2]}})).collect::<Vec<_>>()});
        assert_eq!(
            pa_lanes(&configuration, indices, &[5, 6, 7, 8, 9, 10]),
            (vec![5, 7, 9], vec![6, 8, 10])
        );
    }
}
fn final_operation_ready(
    fresh: bool,
    review_open: bool,
    status: &str,
    result: Option<&str>,
) -> bool {
    fresh
        && !review_open
        && !status.starts_with("COMPLETED/READBACK UNAVAILABLE:")
        && !status.starts_with("REFUSED/UNCERTAIN:")
        && result.is_some_and(|result| {
            let mut words = result.split_whitespace();
            words.next().is_some()
                && words.next() == Some("applied")
                && words.next() == Some("revision")
                && words
                    .next()
                    .is_some_and(|revision| revision.trim_end_matches(';').parse::<u64>().is_ok())
        })
}
#[test]
fn failed_completion_readback_cannot_be_accepted_after_status_overwrite() {
    let result = Some(
        "COMPLETED/READBACK UNAVAILABLE: correlated completion: set applied revision 13; fresh post-final readback unavailable: deadline",
    );
    for status in [
        "COMPLETED/READBACK UNAVAILABLE",
        "passive sends poll failed",
        "fresh readback",
    ] {
        assert!(!final_operation_ready(true, false, status, result));
    }
    assert!(final_operation_ready(
        true,
        false,
        "fresh readback",
        Some("set applied revision 13")
    ));
    assert!(final_operation_ready(
        true,
        false,
        "fresh readback",
        Some("sends_set applied revision 13; crossfade may still be active")
    ));
    assert!(!final_operation_ready(
        false,
        false,
        "stale",
        Some("set applied revision 13")
    ));
    assert!(!final_operation_ready(
        true,
        true,
        "ready",
        Some("set applied revision 13")
    ));
    assert!(!final_operation_ready(
        true,
        false,
        "ready",
        Some("set conflict revision 13")
    ));
}
fn structure_ready(f: &Frontend) -> bool {
    f.fresh()
        && f.state.as_ref().is_some_and(|u| {
            u.structural_fresh
                && u.structural_age_ms.is_some_and(|age| {
                    Duration::from_millis(age).saturating_add(u.received.elapsed())
                        <= Duration::from_millis(250)
                })
        })
}
fn confirm(f: &mut Frontend, end: Instant) {
    wait(f, end, "review", |f| {
        f.fresh() && f.state.as_ref().is_some_and(|u| u.review.is_some())
    });
    f.synchronize_review();
    for p in 0..f.review_pages() {
        if p > 0 {
            tap(f, "PageDown");
        }
        present(f);
    }
    tap(f, "Enter");
    wait(f, end, "correlated final", |f| {
        f.state.as_ref().is_some_and(|u| {
            final_operation_ready(
                f.fresh(),
                u.review.is_some(),
                &u.status,
                u.last_operation.as_deref(),
            )
        })
    });
}
fn grant(f: &mut Frontend, end: Instant) {
    wait(f, end, "read-only attach", Frontend::fresh);
    assert!(!f.state.as_ref().unwrap().writer_granted());
    tap(f, "G");
    wait(f, end, "explicit grant", |f| {
        f.fresh() && f.state.as_ref().is_some_and(|u| u.writer_granted())
    });
}
fn scope(f: &mut Frontend, end: Instant, scope: &str) {
    let before = f
        .state
        .as_ref()
        .unwrap()
        .snapshot
        .as_ref()
        .unwrap()
        .authority
        .clone();
    action(f, Action::SwitchScope(scope.into()));
    wait(f, end, "scope fresh read", Frontend::fresh);
    let after = &f
        .state
        .as_ref()
        .unwrap()
        .snapshot
        .as_ref()
        .unwrap()
        .authority;
    assert_eq!(
        before.parameters, after.parameters,
        "reattachment preserves parameter holds/levels"
    );
    assert_eq!(before.modes, after.modes, "reattachment preserves modes");
    grant(f, end);
    if scope == "foh" || scope == "pa_configuration" {
        action(f, Action::Page(Page::Mix));
        wait(f, end, "scope navigation fresh", Frontend::fresh);
    }
}
struct Trace {
    events: Vec<Value>,
    path: PathBuf,
}
impl Drop for Trace {
    fn drop(&mut self) {
        let _ = fs::write(&self.path, serde_json::to_vec_pretty(&self.events).unwrap());
    }
}
fn trace(f: &Frontend, label: &str, events: &mut Trace) {
    let u = f.state.as_ref().unwrap();
    events.events.push(json!({"label":label,"message":f.message,"generation":u.generation,"operation":u.last_operation,"raw":u.snapshot,"processing":u.processing,"processing_final":u.processing_final,"sends":u.sends,"sends_final":u.sends_final,"structure":u.structural,"structural_final":u.structural_final,"master_eq":u.live_eq,"master_eq_final":u.live_eq_final}));
}
fn capture(host: &Host, f: &mut Frontend, end: Instant, id: &str) -> Value {
    wait(f, end, "capture fresh state", Frontend::fresh);
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
    let tmp = host.dir.join(".capture-request.tmp");
    fs::write(
        &tmp,
        serde_json::to_vec(&json!({"id":id,"blocks":4})).unwrap(),
    )
    .unwrap();
    fs::rename(tmp, host.dir.join("capture-request.json")).unwrap();
    let path = host.dir.join(format!("capture-{id}.json"));
    while !path.exists() {
        f.pump();
        assert!(Instant::now() < end, "capture {id} deadline");
        thread::sleep(Duration::from_millis(2));
    }
    let v = read(&path);
    assert_eq!(v["before"]["raw"]["authority"]["revision"], revision);
    assert_eq!(v["after"]["raw"]["authority"]["revision"], revision);
    assert_eq!(v["before"]["raw"]["authority"]["epoch"], "1");
    for b in v["blocks"].as_array().unwrap() {
        assert_eq!(b["epoch"], "1");
        assert_eq!(b["revision_before"], revision);
        assert_eq!(b["revision_after"], revision);
    }
    for state in ["before", "after"] {
        assert!(
            v[state]["processing"]["channels"]
                .as_array()
                .unwrap()
                .iter()
                .all(|c| c["ready"] == true)
        );
        assert!(
            v[state]["sends"]["channels"]
                .as_array()
                .unwrap()
                .iter()
                .all(|c| c["sends"]
                    .as_array()
                    .unwrap()
                    .iter()
                    .all(|s| s["ready"] == true))
        );
        assert_eq!(v[state]["master_eq"]["transition_remaining_frames"], "0");
    }
    assert_eq!(
        v["before"]["raw"]["topology"],
        v["after"]["raw"]["topology"]
    );
    assert_eq!(
        v["before"]["structure"]["pa_program_buses"],
        v["after"]["structure"]["pa_program_buses"]
    );
    assert_eq!(
        v["before"]["master_eq"]["map_revision"],
        v["after"]["master_eq"]["map_revision"]
    );
    for state in ["before", "after"] {
        for c in v[state]["raw"]["coefficients"].as_array().unwrap() {
            assert_eq!(
                c["current_nanogain"], c["ramp_target_nanogain"],
                "raw ramps settled"
            );
        }
        for (i, c) in v[state]["sends"]["channels"]
            .as_array()
            .unwrap()
            .iter()
            .enumerate()
        {
            for (m, t) in c["sends"].as_array().unwrap().iter().enumerate() {
                assert_eq!(
                    v[state]["exported_intent"]["engine"]["send_taps"][i][m],
                    t["target"]
                );
            }
        }
        assert_eq!(
            v[state]["exported_intent"]["pa_program_buses"],
            v[state]["structure"]["pa_program_buses"]
        );
        assert_eq!(
            v[state]["exported_intent"]["pa_configuration_json"],
            v[state]["structure"]["pa_configuration_json"]
        );
    }
    let blocks = v["blocks"].as_array().unwrap();
    let a = blocks[blocks.len() - 2]["playback_interleaved"]
        .as_array()
        .unwrap();
    let b = blocks.last().unwrap()["playback_interleaved"]
        .as_array()
        .unwrap();
    assert_eq!(a.len(), b.len());
    assert!(
        a.iter()
            .zip(b)
            .all(|(a, b)| (a.as_f64().unwrap() - b.as_f64().unwrap()).abs() < 1e-10),
        "successive periodic capture blocks must be stationary"
    );
    v
}
fn lane(v: &Value, index: usize) -> Vec<f64> {
    let channels = v["channels"].as_u64().unwrap() as usize;
    v["blocks"]
        .as_array()
        .unwrap()
        .iter()
        .flat_map(|b| {
            b["playback_interleaved"]
                .as_array()
                .unwrap()
                .chunks(channels)
                .map(move |f| f[index].as_f64().unwrap())
        })
        .collect()
}
fn rms(a: &[f64]) -> f64 {
    (a.iter().map(|n| n * n).sum::<f64>() / a.len() as f64).sqrt()
}
fn equal(a: &Value, b: &Value, index: usize) {
    let (a, b) = (lane(a, index), lane(b, index));
    assert!(
        a.iter().zip(&b).all(|(a, b)| (a - b).abs() < 1e-11),
        "unaffected lane {index}"
    );
}
fn settled(f: &mut Frontend, end: Instant) {
    thread::sleep(Duration::from_millis(400));
    wait(f, end, "settled provider", |f| {
        f.fresh()
            && f.state.as_ref().is_some_and(|u| {
                u.processing
                    .as_ref()
                    .is_none_or(|p| p.channels.iter().all(|c| c.ready))
                    && u.sends
                        .as_ref()
                        .is_none_or(|s| s.channels.iter().flat_map(|c| &c.sends).all(|s| s.ready))
            })
    });
}
fn signal_history_converged(f: &mut Frontend, end: Instant) {
    // Configuration-ready does not imply stationary compressor or downstream
    // PA filter histories. Keep the ten configured 100ms release constants
    // (48000 actual frames), original run deadline and strict PCM assertions.
    let convergence_frame = f
        .state
        .as_ref()
        .unwrap()
        .snapshot
        .as_ref()
        .unwrap()
        .frame
        .parse::<u64>()
        .unwrap()
        + 48_000;
    wait(f, end, "signal history periodic convergence", |f| {
        f.fresh()
            && f.state.as_ref().is_some_and(|u| {
                u.snapshot.as_ref().is_some_and(|s| {
                    s.frame
                        .parse::<u64>()
                        .is_ok_and(|frame| frame >= convergence_frame)
                })
            })
    });
}
fn fader(f: &mut Frontend, end: Instant, mdb: i32) {
    wait(f, end, "fader fresh", Frontend::fresh);
    let current = f
        .state
        .as_ref()
        .unwrap()
        .snapshot
        .as_ref()
        .unwrap()
        .authority
        .parameters
        .iter()
        .find(|p| p.target.input == "input-01" && p.target.parameter == "fader")
        .unwrap()
        .target_value
        .as_i64()
        .unwrap() as i32;
    assert_ne!(
        current, mdb,
        "driver immediate knob requires nonzero movement"
    );
    let expected_revision = f
        .state
        .as_ref()
        .unwrap()
        .snapshot
        .as_ref()
        .unwrap()
        .authority
        .revision
        .parse::<u64>()
        .unwrap()
        .checked_add(1)
        .unwrap()
        .to_string();
    let expected_result = format!("set applied revision {expected_revision}");
    action(f, Action::Adjust(mdb - current));
    wait(f, end, "fader readback", |f| {
        f.fresh()
            && f.state.as_ref().is_some_and(|u| {
                final_operation_ready(
                    f.fresh(),
                    u.review.is_some(),
                    &u.status,
                    u.last_operation.as_deref(),
                ) && u.last_operation.as_deref() == Some(expected_result.as_str())
                    && u.snapshot
                        .as_ref()
                        .is_some_and(|s| s.authority.revision == expected_revision)
            })
            && f.state
                .as_ref()
                .unwrap()
                .snapshot
                .as_ref()
                .unwrap()
                .authority
                .parameters
                .iter()
                .any(|p| {
                    p.target.input == "input-01"
                        && p.target.parameter == "fader"
                        && p.target_value == mdb
                })
    });
    settled(f, end);
}
fn monitor(f: &mut Frontend, end: Instant, n: usize) {
    scope(f, end, &format!("monitor{n}"));
    tap(f, "F12");
    wait(f, end, "GP18 sends ready", Frontend::sends_ready);
    if f.selected_monitor != n - 1 {
        action(
            f,
            Action::BrowseMonitor(n as i32 - 1 - f.selected_monitor as i32),
        );
        wait(f, end, "selected sends ready", Frontend::sends_ready);
    }
}
fn level(f: &mut Frontend, end: Instant, text: &str) {
    wait(f, end, "fresh paired level editor", Frontend::sends_ready);
    action(f, Action::SendLevelEdit);
    assert!(f.send_draft.is_some(), "{}", f.message);
    action(f, Action::SendLevelText(text.into()));
    action(f, Action::SendApply);
    confirm(f, end);
    wait(f, end, "paired send level", Frontend::sends_ready);
}
fn send_tap(f: &mut Frontend, end: Instant, tap_value: Tap) {
    wait(f, end, "fresh paired tap editor", Frontend::sends_ready);
    action(f, Action::SendTapEdit);
    assert!(f.send_draft.is_some(), "{}", f.message);
    action(f, Action::SendTap(tap_value));
    action(f, Action::SendApply);
    confirm(f, end);
    wait(f, end, "settled tap", Frontend::sends_ready);
}
fn processing_edit(f: &mut Frontend, end: Instant, edits: &[(usize, &str)]) {
    action(f, Action::Page(Page::Channel));
    wait(f, end, "GP07v4 ready", Frontend::processing_ready);
    action(f, Action::ProcessingEdit);
    assert!(f.processing_draft.is_some(), "{}", f.message);
    for (field, text) in edits {
        let delta = *field as i32 - f.processing_field as i32;
        action(f, Action::ProcessingField(delta));
        action(f, Action::ProcessingText((*text).into()));
        assert!(f.processing_entry.is_empty(), "{}", f.message);
    }
    action(f, Action::ProcessingApply);
    confirm(f, end);
    wait(f, end, "processing settled", Frontend::processing_ready);
    settled(f, end);
}
#[test]
#[ignore = "explicit pinned witness executable and stable owner manifest; synthetic PCM only; two bounded runs"]
fn actual_frontend_sends_processing_and_master_eq_with_pcm_witness() {
    let executable = PathBuf::from(
        std::env::var_os("GP18_WITNESS_EXECUTABLE").expect("explicit witness executable"),
    );
    // A coordinator-reviewed witness-only correction supplies exact immutable
    // pins in the recorded invocation; absent overrides retain the original pin.
    let expected_binary =
        std::env::var("GP18_WITNESS_EXPECTED_SHA256").unwrap_or_else(|_| BINARY.into());
    let expected_source = std::env::var("GP18_WITNESS_EXPECTED_SOURCE_REVISION")
        .unwrap_or_else(|_| "7eaa820600531bc316aa86018a0176ce76879f76".into());
    assert_eq!(expected_binary.len(), 64);
    assert_eq!(expected_source.len(), 40);
    assert!(
        expected_binary
            .bytes()
            .chain(expected_source.bytes())
            .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
    );
    assert_eq!(hash(&executable), expected_binary);
    let manifest =
        PathBuf::from(std::env::var_os("GP_EQ_MANIFEST").expect("explicit stable manifest"));
    assert_eq!(hash(&manifest), MANIFEST);
    let root =
        PathBuf::from(std::env::var_os("GP18_DRIVER_EVIDENCE").expect("new private evidence root"));
    fs::create_dir(&root).unwrap();
    use std::os::unix::fs::PermissionsExt;
    fs::set_permissions(&root, fs::Permissions::from_mode(0o700)).unwrap();
    for run in ["sends", "master"] {
        let dir = root.join(run);
        fs::create_dir(&dir).unwrap();
        fs::set_permissions(&dir, fs::Permissions::from_mode(0o700)).unwrap();
        let log = fs::File::create(root.join(format!("{run}-host.log"))).unwrap();
        let mut command = if std::env::var_os("GP18_DRIVER_TRACE_FSYNC").is_some() {
            let mut trace = Command::new("/usr/bin/strace");
            trace
                .args([
                    "--kill-on-exit",
                    "-f",
                    "-ttt",
                    "-T",
                    "-e",
                    "trace=fsync",
                    "-o",
                ])
                .arg(root.join(format!("{run}-fsync-trace.txt")))
                .arg(&executable);
            trace
        } else {
            Command::new(&executable)
        };
        let child = command
            .env("GP_DESK_WITNESS_DIR", &dir)
            .env("GP_EQ_MANIFEST", &manifest)
            .args([
                "--ignored",
                "--exact",
                "serve_desk_operator_witness",
                "--nocapture",
                "--test-threads=1",
            ])
            .stdout(Stdio::from(log.try_clone().unwrap()))
            .stderr(Stdio::from(log))
            .spawn()
            .unwrap();
        let mut host = Host { child, dir };
        let startup = Instant::now() + Duration::from_secs(30);
        while !host.dir.join("ready.json").exists() {
            assert!(Instant::now() < startup, "startup deadline");
            assert!(
                host.child.try_wait().unwrap().is_none(),
                "host startup failed"
            );
            thread::sleep(Duration::from_millis(10));
        }
        let ready = read(&host.dir.join("ready.json"));
        assert_eq!(ready["provenance"]["executable_sha256"], expected_binary);
        assert_eq!(ready["provenance"]["owner_manifest_sha256"], MANIFEST);
        assert_eq!(ready["provenance"]["gigpies_revision"], expected_source);
        assert_eq!(ready["hardware_opened"], false);
        let modules = read(&manifest);
        for name in ["rec", "fx", "pa"] {
            assert_eq!(
                ready["provenance"]["libraries"][name]["sha256"],
                modules[name]["library_sha256"]
            );
        }
        assert_eq!(ready["topology"]["inputs"].as_array().unwrap().len(), 17);
        assert_eq!(ready["topology"]["monitors"], 3);
        assert_eq!(ready["topology"]["pa_outputs"], 6);
        let outputs = ready["topology"]["outputs"].as_array().unwrap();
        for (i, p) in outputs.iter().enumerate() {
            assert_eq!(p["playback_slot"], i, "position is proven playback slot");
        }
        let fixtures = PathBuf::from(std::env::var_os("GP_PA_V2_FIXTURES").unwrap());
        assert_eq!(hash(&fixtures.join("stereo3way.json")), PA_FIXTURE);
        assert_eq!(ready["provenance"]["initial_pa_fixture_sha256"], PA_FIXTURE);
        let output = |kind: &str, index: usize| {
            outputs
                .iter()
                .position(|p| {
                    p["source"]["kind"] == kind
                        && (p["source"]["index"] == index || p["source"]["channel"] == index)
                })
                .unwrap()
        };
        let mon1 = output("monitor", 0);
        let mon3 = output("monitor", 2);
        let main = output("main", 0);
        let pa: Vec<_> = (0..6).map(|n| output("pa", n)).collect();
        let end = Instant::now() + Duration::from_secs(150);
        let mut f = Frontend::new(Config {
            wire_version: 2,
            remote: None,
            endpoint: host.dir.join(ready["socket"].as_str().unwrap()),
            show: ready["show_id"].as_str().unwrap().into(),
            epoch: 1,
            writer: format!("desk-d-{}-{run}", std::process::id()),
            scope: "foh".into(),
        });
        f.enable_processing().unwrap();
        wait(&mut f, end, "GP07v4 initial", Frontend::processing_ready);
        grant(&mut f, end);
        let mut events = Trace {
            events: Vec::new(),
            path: root.join(format!("{run}-frontend-trace.json")),
        };
        trace(&f, "explicit initial FOH grant", &mut events);
        action(&mut f, Action::ModePicker);
        action(&mut f, Action::ChooseMode(Mode::Manual));
        confirm(&mut f, end);
        fader(&mut f, end, -12000);
        // Make channel input explicitly open if needed, using actual reviewed shared mute.
        if f.state
            .as_ref()
            .unwrap()
            .snapshot
            .as_ref()
            .unwrap()
            .authority
            .parameters
            .iter()
            .find(|p| p.target.parameter == "mute" && p.target.input == "input-01")
            .unwrap()
            .target_value
            == true
        {
            tap(&mut f, "M");
            confirm(&mut f, end);
        }
        if run == "sends" {
            monitor(&mut f, end, 1);
            level(&mut f, end, "0");
            monitor(&mut f, end, 3);
            level(&mut f, end, "-3");
            scope(&mut f, end, "pa_configuration");
            wait(&mut f, end, "global output rearm review readiness", |f| {
                structure_ready(f)
            });
            action(&mut f, Action::OutputRearm);
            confirm(&mut f, end);
            signal_history_converged(&mut f, end);
            monitor(&mut f, end, 3);
            settled(&mut f, end);
            let baseline = capture(&host, &mut f, end, "raw-baseline");
            let pattern = ready["input_zero_pattern"].as_array().unwrap();
            for (n, sample) in lane(&baseline, mon1).iter().enumerate() {
                assert!(
                    (sample - pattern[n % 48].as_f64().unwrap()).abs() < 1e-10,
                    "actual raw reference"
                );
            }
            level(&mut f, end, "-9");
            settled(&mut f, end);
            let lower = capture(&host, &mut f, end, "level-only");
            equal(&baseline, &lower, mon1);
            assert!(
                (rms(&lane(&lower, mon3)) / rms(&lane(&baseline, mon3)) - 10_f64.powf(-6. / 20.))
                    .abs()
                    < 1e-5
            );
            trace(&f, "separate exact level review", &mut events);
            send_tap(&mut f, end, Tap::ProcessedPreFader);
            settled(&mut f, end);
            let pre = capture(&host, &mut f, end, "processed-pre");
            equal(&lower, &pre, mon1);
            equal(&lower, &pre, mon3);
            trace(&f, "separate tap review", &mut events);
            scope(&mut f, end, "foh");
            processing_edit(&mut f, end, &[(0, "0"), (1, "1000"), (2, "6"), (17, "1")]);
            signal_history_converged(&mut f, end);
            let eq = capture(&host, &mut f, end, "channel-eq");
            equal(&pre, &eq, mon1);
            assert!(rms(&lane(&eq, mon3)) > rms(&lane(&pre, mon3)) * 1.5);
            trace(&f, "channel EQ", &mut events);
            processing_edit(&mut f, end, &[(17, "0"), (18, "-50"), (19, "4"), (23, "0")]);
            signal_history_converged(&mut f, end);
            let comp = capture(&host, &mut f, end, "compression");
            equal(&eq, &comp, mon1);
            assert!(rms(&lane(&comp, mon3)) < rms(&lane(&eq, mon3)) * 0.8);
            trace(&f, "channel compressor", &mut events);
            fader(&mut f, end, -6000);
            // The fader step also excites continuing PA crossover histories.
            // Raw and pre-fader monitors are unchanged, but every PA lane must
            // reach periodic stationarity before this all-output capture.
            signal_history_converged(&mut f, end);
            let prefader = capture(&host, &mut f, end, "pre-fader-change");
            equal(&comp, &prefader, mon1);
            equal(&comp, &prefader, mon3);
            monitor(&mut f, end, 3);
            send_tap(&mut f, end, Tap::ProcessedPostFader);
            settled(&mut f, end);
            let post = capture(&host, &mut f, end, "processed-post");
            equal(&prefader, &post, mon1);
            assert!(
                (rms(&lane(&post, mon3)) / rms(&lane(&prefader, mon3)) - 10_f64.powf(-6. / 20.))
                    .abs()
                    < 1e-5
            );
            trace(&f, "post-fader tap", &mut events);
            scope(&mut f, end, "foh");
            tap(&mut f, "F1");
            wait(&mut f, end, "visible FOH mute fresh", Frontend::fresh);
            tap(&mut f, "M");
            confirm(&mut f, end);
            settled(&mut f, end);
            signal_history_converged(&mut f, end);
            let muted = capture(&host, &mut f, end, "shared-mute");
            assert_eq!(rms(&lane(&muted, mon1)), 0.);
            assert_eq!(rms(&lane(&muted, mon3)), 0.);
            tap(&mut f, "M");
            confirm(&mut f, end);
            settled(&mut f, end);
            signal_history_converged(&mut f, end);
            let recovered = capture(&host, &mut f, end, "shared-unmute");
            equal(&post, &recovered, mon1);
            equal(&post, &recovered, mon3);
            monitor(&mut f, end, 3);
            action(&mut f, Action::SendTapEdit);
            action(&mut f, Action::SendTap(Tap::RawPostMute));
            action(&mut f, Action::SendApply);
            wait(&mut f, end, "unsent tap review", |f| {
                f.state.as_ref().is_some_and(|u| u.review.is_some())
            });
            present(&mut f);
            scope(&mut f, end, "foh");
            settled(&mut f, end);
            let no_replay = capture(&host, &mut f, end, "scope-no-replay");
            equal(&recovered, &no_replay, mon1);
            equal(&recovered, &no_replay, mon3);
            assert_eq!(
                no_replay["after"]["sends"]["channels"][0]["sends"][2]["target"],
                "processed_post_fader"
            );
        } else {
            processing_edit(&mut f, end, &[(0, "1"), (17, "1")]);
            monitor(&mut f, end, 1);
            level(&mut f, end, "0");
            scope(&mut f, end, "pa_configuration");
            tap(&mut f, "F7");
            wait(&mut f, end, "PA structural ready", |f| structure_ready(f));
            action(&mut f, Action::OutputRearm);
            confirm(&mut f, end);
            signal_history_converged(&mut f, end);
            settled(&mut f, end);
            let baseline = capture(&host, &mut f, end, "pa-rearmed-baseline");
            let power = |v: &Value| pa.iter().map(|i| rms(&lane(v, *i)).powi(2)).sum::<f64>();
            assert!(power(&baseline) > 1e-12);
            tap(&mut f, "L");
            wait(&mut f, end, "live owner", Frontend::live_eq_ready);
            tap(&mut f, "E");
            assert!(f.structural_draft.is_some(), "{}", f.message);
            tap(&mut f, "C");
            action(&mut f, Action::StructureField(3));
            action(&mut f, Action::StructureText("6.125".into()));
            let intended_body = f.structural_draft.as_ref().unwrap().body().unwrap();
            let intended_patch: Value =
                serde_json::from_str(intended_body["patch_json"].as_str().unwrap()).unwrap();
            fs::write(
                root.join("live-intended.json"),
                serde_json::to_vec_pretty(&intended_body).unwrap(),
            )
            .unwrap();
            let indices = f
                .state
                .as_ref()
                .unwrap()
                .live_eq
                .as_ref()
                .unwrap()
                .master_input_indices
                .unwrap();
            assert_eq!(intended_patch["inputs"][0]["input_index"], indices[0]);
            assert_eq!(intended_patch["inputs"][1]["input_index"], indices[1]);
            assert_eq!(intended_patch["inputs"][0]["eq"][0]["db"], 6.125);
            assert_eq!(intended_patch["inputs"][1]["eq"][0]["db"], 0.0);
            action(&mut f, Action::StructureApply);
            confirm(&mut f, end);
            wait(&mut f, end, "settled live EQ", Frontend::live_eq_ready);
            settled(&mut f, end);
            signal_history_converged(&mut f, end);
            let live = capture(&host, &mut f, end, "live-eq-settled");
            let owner: Value =
                serde_json::from_str(live["after"]["master_eq"]["owner_json"].as_str().unwrap())
                    .unwrap();
            assert_eq!(owner["target"], intended_patch["inputs"]);
            assert_eq!(owner["current"], intended_patch["inputs"]);
            let conf: Value = serde_json::from_str(
                live["after"]["structure"]["pa_configuration_json"]
                    .as_str()
                    .unwrap(),
            )
            .unwrap();
            for i in 0..2 {
                assert_eq!(
                    conf["inputs"][indices[i]]["eq"],
                    intended_patch["inputs"][i]["eq"]
                );
            }
            // Bind each actual playback slot to the owner's real output source.
            // The fixed stereo3way fixture interleaves L/R; lane positions cannot
            // substitute for routing identity.
            assert_eq!(
                live["after"]["structure"]["pa_program_buses"],
                json!([0, 1])
            );
            let fixture = read(&fixtures.join("stereo3way.json"));
            for i in 0..pa.len() {
                assert_eq!(
                    conf["outputs"][i]["source"],
                    fixture["outputs"][i]["source"]
                );
            }
            let (affected, unaffected) = pa_lanes(&conf, indices, &pa);
            assert_eq!(affected.len(), 3);
            assert_eq!(unaffected.len(), 3);
            for slot in &unaffected {
                equal(&baseline, &live, *slot);
            }

            assert_eq!(intended_patch["inputs"][0]["eq_enabled"], true);
            assert_eq!(intended_patch["inputs"][0]["eq"][0]["kind"], "bell");
            assert_eq!(intended_patch["inputs"][0]["eq"][0]["hz"], 1000.0);
            assert_eq!(fixture["inputs"][0]["compressor"]["enabled"], false);
            let fundamental = |v: &Value, slot: usize| {
                let samples = lane(v, slot);
                let (mut real, mut imag) = (0.0, 0.0);
                for (n, sample) in samples.iter().enumerate() {
                    let angle = 2.0 * std::f64::consts::PI * (n % 48) as f64 / 48.0;
                    real += sample * angle.cos();
                    imag -= sample * angle.sin();
                }
                (real / samples.len() as f64, imag / samples.len() as f64)
            };
            let expected = 10_f64.powf(6.125 / 20.0);
            for slot in &affected {
                let (ar, ai) = fundamental(&baseline, *slot);
                let (br, bi) = fundamental(&live, *slot);
                let energy = ar * ar + ai * ai;
                assert!(energy > 1e-18, "affected lane is independently excited");
                let ratio_real = (br * ar + bi * ai) / energy;
                let ratio_imag = (bi * ar - br * ai) / energy;
                // f32 provider PCM and owner numerical error; unchanged-lane checks
                // above retain their stricter sample-by-sample 1e-11 bound.
                assert!(
                    (ratio_real - expected).abs() < 1e-5,
                    "PA slot {slot} exact 6.125 dB gain: {ratio_real}"
                );
                assert!(
                    ratio_imag.abs() < 1e-5,
                    "PA slot {slot} centre-bell phase: {ratio_imag}"
                );
            }
            equal(&baseline, &live, main);
            equal(&baseline, &live, mon1);
            assert!((power(&live) / power(&baseline) - 1.).abs() > 0.1);
            assert_eq!(live["before"]["structure"]["outputs_quiesced"], false);
            trace(&f, "live narrow EQ complete review; no rearm", &mut events);
            action(&mut f, Action::OutputMute);
            confirm(&mut f, end);
            settled(&mut f, end);
            let mute = capture(&host, &mut f, end, "pa-muted");
            assert_eq!(power(&mute), 0.);
            // A correlated mute final and PCM silence do not grant a fresh GP14
            // editing observation. Await the actual matched structural read.
            wait(&mut f, end, "fresh muted setup observation", |f| {
                structure_ready(f)
                    && f.state
                        .as_ref()
                        .is_some_and(|u| u.structural.as_ref().is_some_and(|s| s.outputs_quiesced))
            });
            tap(&mut f, "F11");
            assert!(f.structural_draft.is_some(), "{}", f.message);
            let original = f.structural_draft.as_ref().unwrap().document.clone();
            tap(&mut f, "C");
            tap(&mut f, "C");
            action(&mut f, Action::StructureField(3));
            action(&mut f, Action::StructureText("-4.5".into()));
            let intended = f.structural_draft.as_ref().unwrap().document.clone();
            action(&mut f, Action::StructureApply);
            confirm(&mut f, end);
            settled(&mut f, end);
            let full = capture(&host, &mut f, end, "muted-full-eq");
            assert_eq!(power(&full), 0.);
            assert_eq!(full["after"]["structure"]["outputs_quiesced"], true);
            let applied: Value = serde_json::from_str(
                full["after"]["structure"]["pa_configuration_json"]
                    .as_str()
                    .unwrap(),
            )
            .unwrap();
            assert_eq!(applied, intended["configuration"]);
            let mut preserved = applied;
            for (i, input) in preserved["inputs"]
                .as_array_mut()
                .unwrap()
                .iter_mut()
                .enumerate()
            {
                for key in ["eq_enabled", "eq", "geq_enabled", "geq_db"] {
                    input[key] = original["configuration"]["inputs"][i][key].clone();
                }
            }
            assert_eq!(preserved, original["configuration"]);
            trace(
                &f,
                "muted full configuration review; no implicit rearm",
                &mut events,
            );
            action(&mut f, Action::OutputRearm);
            confirm(&mut f, end);
            signal_history_converged(&mut f, end);
            settled(&mut f, end);
            let rearmed = capture(&host, &mut f, end, "separate-rearm");
            assert!(power(&rearmed) > 1e-12);
            assert!((power(&rearmed) / power(&live) - 1.).abs() > 0.05);
            tap(&mut f, "L");
            wait(
                &mut f,
                end,
                "live and structural read after full setup",
                |f| Frontend::live_eq_ready(f) && structure_ready(f),
            );
            tap(&mut f, "E");
            assert!(
                f.structural_draft
                    .as_ref()
                    .is_some_and(|d| d.live_context.is_some()),
                "{}",
                f.message
            );
            action(&mut f, Action::StructureField(3));
            action(&mut f, Action::StructureText("2".into()));
            wait(&mut f, end, "fresh unsent review readback", |f| {
                Frontend::live_eq_ready(f) && structure_ready(f)
            });
            action(&mut f, Action::StructureApply);
            wait(&mut f, end, "unsent live review", |f| {
                f.state.as_ref().is_some_and(|u| u.review.is_some())
            });
            present(&mut f);
            tap(&mut f, "F5");
            wait(&mut f, end, "reconnect read-only", Frontend::fresh);
            assert!(!f.state.as_ref().unwrap().writer_granted());
            settled(&mut f, end);
            let no_replay = capture(&host, &mut f, end, "reconnect-no-replay");
            assert_eq!(
                no_replay["after"]["structure"]["pa_configuration_json"],
                rearmed["after"]["structure"]["pa_configuration_json"]
            );
            equal(&rearmed, &no_replay, main);
            equal(&rearmed, &no_replay, mon1);
            for slot in &pa {
                equal(&rearmed, &no_replay, *slot);
            }
        }
        fs::write(
            root.join(format!("{run}-frontend-trace.json")),
            serde_json::to_vec_pretty(&events.events).unwrap(),
        )
        .unwrap();
        drop(f);
        fs::write(host.dir.join("stop"), b"").unwrap();
        let shutdown = Instant::now() + Duration::from_secs(10);
        loop {
            if let Some(status) = host.child.try_wait().unwrap() {
                assert!(status.success());
                break;
            }
            assert!(Instant::now() < shutdown);
            thread::sleep(Duration::from_millis(10));
        }
        let summary = read(&host.dir.join("summary.json"));
        assert_eq!(summary["stopped_by_driver"], true);
        assert!(!host.dir.join("failure.json").exists());
    }
}
