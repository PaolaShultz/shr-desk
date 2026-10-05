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
    loop {
        f.pump();
        if predicate(f) {
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
    f.inject_controller(action).unwrap();
    f.pump();
    // Explicit injected gestures are bounded; allow the existing release queue to drain.
    thread::sleep(Duration::from_millis(45));
    f.pump();
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
fn review_confirm(f: &mut Frontend, deadline: Instant, injected: bool) {
    if injected {
        controller(f, Action::ProcessingApply);
    } else {
        tap(f, "F4");
    }
    wait(f, deadline, "displayed review", |f| {
        f.state.as_ref().is_some_and(|u| u.review.is_some()) && f.processing_ready()
    });
    let scene = f.scene();
    assert!(scene.in_bounds());
    let visible = text(f);
    assert!(visible.contains("APPLY channel"));
    assert!(visible.contains("Compressor bypass"));
    assert!(visible.contains("240-frame"));
    f.mark_presented();
    if injected {
        controller(f, Action::Confirm);
    } else {
        tap(f, "Enter");
    }
}
fn run_driver(endpoint: &Path, epoch: u64, evidence: Option<&Path>, cpu: bool) {
    assert!(endpoint.is_absolute());
    let start = Instant::now();
    let deadline = start + Duration::from_secs(17);
    let mut f = Frontend::new(Config {
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
    eprintln!(
        "GP07 milestone fresh capability revision={}",
        initial.revision
    );
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
    tap(&mut f, "E");
    assert!(f.processing_draft.is_some(), "{}", f.message);
    // Full keyboard path: bypass, bell EQ, compressor law and explicit makeup.
    field_keyboard(&mut f, 0, "0");
    field_keyboard(&mut f, 4, "6");
    field_keyboard(&mut f, 8, "0");
    field_keyboard(&mut f, 9, "-48");
    field_keyboard(&mut f, 10, "4");
    field_keyboard(&mut f, 14, "3");
    let channel1 = f.processing_draft.as_ref().unwrap().config.clone();
    assert_eq!(
        snapshot(&f).channels,
        initial.channels,
        "draft must not write"
    );
    assert!(text(&f).contains("LOCAL DRAFT"));
    review_confirm(&mut f, deadline, false);
    wait(&mut f, deadline, "channel1 confirmed", |f| {
        f.processing_ready()
            && snapshot(f).channels[0].current == channel1
            && snapshot(f).channels[0]
                .gain_reduction_mdb
                .is_some_and(|n| n > 0)
    });
    assert_eq!(
        snapshot(&f).channels[1].current,
        initial.channels[1].current
    );
    assert!(text(&f).contains("Mid gain +6.0 dB"));
    assert!(text(&f).contains("GR"));
    let first = snapshot(&f).clone();
    assert!(first.channels[0].gain_reduction_mdb.is_some_and(|n| n > 0));
    eprintln!(
        "GP07 milestone keyboard channel1 confirmed revision={}",
        first.revision
    );
    // Second channel uses the same Action dispatcher, queue, review and authority.
    controller(&mut f, Action::Move(1));
    wait(
        &mut f,
        deadline,
        "channel2 context",
        Frontend::processing_ready,
    );
    controller(&mut f, Action::ProcessingEdit);
    controller(&mut f, Action::ProcessingText("0".into()));
    controller(&mut f, Action::ProcessingField(2));
    controller(&mut f, Action::ProcessingText("-3".into()));
    controller(&mut f, Action::ProcessingField(6));
    controller(&mut f, Action::ProcessingText("0".into()));
    controller(&mut f, Action::ProcessingField(1));
    controller(&mut f, Action::ProcessingText("-24".into()));
    controller(&mut f, Action::ProcessingField(1));
    controller(&mut f, Action::ProcessingText("2".into()));
    let channel2 = f.processing_draft.as_ref().unwrap().config.clone();
    review_confirm(&mut f, deadline, true);
    wait(&mut f, deadline, "channel2 confirmed", |f| {
        f.processing_ready() && snapshot(f).channels[1].current == channel2
    });
    assert_eq!(snapshot(&f).channels[0].current, channel1);
    assert_eq!(snapshot(&f).channels[2..], initial.channels[2..]);
    assert!(text(&f).contains("Low gain -3.0 dB"));
    let second = snapshot(&f).clone();
    eprintln!(
        "GP07 milestone injected controller channel2 confirmed revision={}",
        second.revision
    );
    // A channel change cancels a modified local draft without any command.
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
    // A presented review is also revoked on reconnect; no replay, no implicit grant.
    tap(&mut f, "E");
    field_keyboard(&mut f, 4, "-6");
    tap(&mut f, "F4");
    wait(&mut f, deadline, "review before reconnect", |f| {
        f.state.as_ref().is_some_and(|u| u.review.is_some())
    });
    let _ = f.scene();
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
    tap(&mut f, "Enter");
    assert!(f.message.contains("no displayed"));
    let final_snapshot = snapshot(&f).clone();
    assert!(f.scene().in_bounds());
    if let Some(path) = std::env::var_os("GP07_SCENE_EVIDENCE") {
        shr_desk::raster::ppm(&f.scene(), &PathBuf::from(path)).unwrap();
    }
    #[cfg(feature = "native")]
    if cpu {
        assert!(
            shr_desk::native::offscreen(&f.scene())
                .unwrap()
                .contains("1920")
        );
    }
    #[cfg(not(feature = "native"))]
    let _ = cpu;
    eprintln!(
        "GP07 milestone reconnect retained settings without replay revision={}",
        final_snapshot.revision
    );
    if let Some(path) = evidence {
        fs::write(path, serde_json::to_vec_pretty(&json!({"kind":"gp07-real-frontend-driver","show":SHOW,"epoch":epoch,"initial_revision":initial.revision,"keyboard_revision":first.revision,"controller_revision":second.revision,"channel1_gain_reduction_mdb":first.channels[0].gain_reduction_mdb,"reconnect_revision":final_snapshot.revision,"channel1":channel1,"channel2":channel2,"context_cancel":true,"reconnect_no_replay":true,"visible_scene":true,"elapsed_ms":start.elapsed().as_millis(),"scope":"operator/provider readback; host owns sample/REC/analysis assertions"})).unwrap()).unwrap();
    }
    drop(f);
    assert!(
        start.elapsed() < Duration::from_secs(20),
        "driver exceeded 20s bound"
    );
    eprintln!(
        "GP07 milestone success frontend dropped elapsed_ms={}",
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
    let evidence = std::env::var_os("GP07_DRIVER_EVIDENCE").map(PathBuf::from);
    run_driver(
        &dir.join("audio.sock"),
        100,
        evidence.as_deref(),
        std::env::var("VK_DRIVER_FILES").is_ok_and(|v| v == "/usr/share/vulkan/icd.d/lvp_icd.json"),
    );
}
