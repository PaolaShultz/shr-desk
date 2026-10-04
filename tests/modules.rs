//! Accepted GP05 data only; no owner source, DSP, devices or recording writes.
use serde_json::{Value, json};
use shr_desk::modules;
const SHOW: &str = "11111111-1111-4111-8111-111111111111";
fn fixture() -> Value {
    serde_json::from_slice(include_bytes!("fixtures/gp05/v1/status-finalized.json")).unwrap()
}
fn decode(v: &Value) -> Result<modules::Status, String> {
    modules::decode(&serde_json::to_vec(v).unwrap(), SHOW, 9)
}
#[test]
fn exact_accepted_available_unavailable_and_native_integer_precision() {
    let status = modules::decode(
        include_bytes!("fixtures/gp05/v1/status-finalized.json"),
        SHOW,
        9,
    )
    .unwrap();
    assert!(status.available);
    assert_eq!(status.recording.as_ref().unwrap().written_frames, "1440");
    assert!(status.recording.as_ref().unwrap().durable_frames.is_none());
    assert!(status.lines().iter().any(|l| l.contains("MAIN ONLY")));
    assert!(status.lines().iter().any(|l| l.contains("durable UNKNOWN")));
    assert!(
        modules::decode(
            include_bytes!("fixtures/gp05/v1/status-unavailable.json"),
            SHOW,
            9
        )
        .unwrap()
        .fx
        .is_none()
    );
    let expected: Value =
        serde_json::from_slice(include_bytes!("fixtures/gp05/v1/status-request.json")).unwrap();
    assert_eq!(
        serde_json::from_slice::<Value>(&modules::request(SHOW, 9).unwrap()).unwrap(),
        expected
    );
    let mut v = fixture();
    v["fx"]["status"]["reset_count"] = json!(9007199254740993u64);
    assert_eq!(
        decode(&v).unwrap().fx.unwrap().status.reset_count,
        9007199254740993
    );
}
#[test]
fn strict_identity_fields_counters_float_location_and_capability_domains() {
    let mut v = fixture();
    v["recording"]["written_frames"] = json!(1440);
    assert!(decode(&v).is_err());
    let mut v = fixture();
    v["recording"]["accepted_frames"] = json!("01440");
    assert!(decode(&v).is_err());
    let mut v = fixture();
    v["fx"]["status"]["reset_count"] = json!(1.0);
    assert!(decode(&v).is_err());
    let mut v = fixture();
    v["fx"]["capabilities"]["identity"][31] = json!(1);
    assert!(decode(&v).is_err());
    let mut v = fixture();
    v["pa"]["descriptor"]["physical_io_owned"] = json!(1);
    assert!(decode(&v).is_err());
    let mut v = fixture();
    v["recording"]
        .as_object_mut()
        .unwrap()
        .remove("durable_frames");
    assert!(decode(&v).is_err());
    let mut v = fixture();
    v["extra"] = json!(0);
    assert!(decode(&v).is_err());
    assert!(
        modules::decode(
            include_bytes!("fixtures/gp05/v1/status-finalized.json"),
            SHOW,
            10
        )
        .is_err()
    );
    let duplicate = String::from_utf8(serde_json::to_vec(&fixture()).unwrap())
        .unwrap()
        .replacen("\"version\":1", "\"version\":1,\"version\":1", 1);
    assert!(modules::decode(duplicate.as_bytes(), SHOW, 9).is_err());
    let nonfinite = String::from_utf8(serde_json::to_vec(&fixture()).unwrap())
        .unwrap()
        .replacen("20.0", "1e400", 1);
    assert!(modules::decode(nonfinite.as_bytes(), SHOW, 9).is_err());
}
#[test]
fn separate_private_readonly_query_cli_and_malformed_refusal() {
    use std::{
        fs,
        io::{Read, Write},
        os::unix::{fs::PermissionsExt, net::UnixListener},
        process::Command,
        time::Duration,
    };
    for malformed in [false, true] {
        let dir =
            std::env::temp_dir().join(format!("desk-modules-{}-{malformed}", std::process::id()));
        fs::create_dir(&dir).unwrap();
        fs::set_permissions(&dir, fs::Permissions::from_mode(0o700)).unwrap();
        let socket = dir.join("audio.sock");
        let listener = UnixListener::bind(&socket).unwrap();
        fs::set_permissions(&socket, fs::Permissions::from_mode(0o600)).unwrap();
        let child = std::thread::spawn(move || {
            let (mut s, _) = listener.accept().unwrap();
            s.set_read_timeout(Some(Duration::from_secs(2))).unwrap();
            let mut h = [0; 4];
            s.read_exact(&mut h).unwrap();
            let n = u32::from_be_bytes(h) as usize;
            assert!(n < 65536);
            let mut b = vec![0; n];
            s.read_exact(&mut b).unwrap();
            let q: Value = serde_json::from_slice(&b).unwrap();
            assert_eq!(q["kind"], "module_status");
            assert!(q["writer"].is_null());
            assert!(q["lease"].is_null());
            let mut v = fixture();
            if malformed {
                v["physical"] = json!("safe");
            }
            let b = serde_json::to_vec(&v).unwrap();
            s.write_all(&(b.len() as u32).to_be_bytes()).unwrap();
            s.write_all(&b).unwrap();
        });
        let output = Command::new(env!("CARGO_BIN_EXE_shr-desk"))
            .args(["--modules-status", socket.to_str().unwrap(), SHOW, "9"])
            .output()
            .unwrap();
        child.join().unwrap();
        fs::remove_dir_all(&dir).unwrap();
        assert_eq!(
            output.status.success(),
            !malformed,
            "{}",
            String::from_utf8_lossy(&output.stderr)
        );
        if !malformed {
            let v: Value = serde_json::from_slice(&output.stdout).unwrap();
            assert_eq!(v["recording"]["written_frames"], "1440");
            assert!(v["recording"]["durable_frames"].is_null());
        }
    }
}

#[test]
#[ignore = "requires exact independently accepted GP05 executable/activation manifest in SHR_DESK_GP05 and SHR_DESK_GP05_MANIFEST; synthetic private process only"]
fn actual_gp05_readonly_metadata_and_independent_frontend_health() {
    use std::{
        fs,
        os::unix::fs::PermissionsExt,
        process::{Child, Command, Stdio},
        thread,
        time::{Duration, Instant},
    };
    struct Service {
        child: Child,
        dir: std::path::PathBuf,
    }
    impl Drop for Service {
        fn drop(&mut self) {
            let _ = self.child.kill();
            let _ = self.child.wait();
            fs::remove_dir_all(&self.dir).unwrap();
        }
    }
    let dir = std::env::temp_dir().join(format!("desk-gp05-real-{}", std::process::id()));
    fs::create_dir(&dir).unwrap();
    fs::set_permissions(&dir, fs::Permissions::from_mode(0o700)).unwrap();
    let child = Command::new(std::env::var_os("SHR_DESK_GP05").expect("accepted GP05 binary"))
        .args([
            "--directory",
            dir.to_str().unwrap(),
            "--show",
            SHOW,
            "--epoch",
            "9",
            "--synthetic-source",
            "fouraux",
            "--analysis",
            "--modules",
        ])
        .arg(
            std::env::var_os("SHR_DESK_GP05_MANIFEST")
                .expect("accepted module activation manifest"),
        )
        .args(["--ticks", "60000"])
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
        .unwrap();
    let mut service = Service {
        child,
        dir: dir.clone(),
    };
    let end = Instant::now() + Duration::from_secs(3);
    while !dir.join("audio.sock").exists() {
        assert!(service.child.try_wait().unwrap().is_none());
        assert!(Instant::now() < end);
        thread::sleep(Duration::from_millis(10));
    }
    let config = shr_desk::frontend::Config {
        endpoint: dir.join("audio.sock"),
        show: SHOW.into(),
        epoch: 9,
        writer: "desk-modules-real".into(),
        scope: "foh".into(),
    };
    let status = modules::query(&config).unwrap();
    assert!(status.available);
    assert_eq!(status.physical, "unverified");
    assert_eq!(
        status.fx.as_ref().unwrap().capabilities.writable_parameters,
        0
    );
    assert_eq!(status.pa.as_ref().unwrap().descriptor.active_output_mask, 3);
    let mut front = shr_desk::frontend::Frontend::new(config);
    let end = Instant::now() + Duration::from_secs(3);
    loop {
        front.pump();
        if front.fresh()
            && front
                .modules
                .as_ref()
                .is_some_and(|m| m.fresh() && m.status.as_ref().is_some_and(|s| s.available))
        {
            break;
        }
        assert!(Instant::now() < end, "{:?}", front.modules);
        thread::sleep(Duration::from_millis(10));
    }
    assert!(front.state.as_ref().unwrap().status.contains("read-only"));
    // The real module observer must coexist with the primary GP03 authority
    // workflow, including a presented protected review and confirmed mutation.
    fn wait(
        front: &mut shr_desk::frontend::Frontend,
        predicate: impl Fn(&shr_desk::frontend::Frontend) -> bool,
    ) {
        let end = Instant::now() + Duration::from_secs(5);
        loop {
            front.pump();
            if predicate(front) {
                return;
            }
            assert!(
                Instant::now() < end,
                "primary workflow timeout: {:?}",
                front.state
            );
            thread::sleep(Duration::from_millis(10));
        }
    }
    fn tap(front: &mut shr_desk::frontend::Frontend, key: &str) {
        wait(front, |f| f.fresh());
        if key == "Enter" {
            let _ = front.scene();
            front.mark_presented();
        }
        for pressed in [true, false] {
            front
                .enqueue(shr_desk::frontend::Event::Key {
                    key: key.into(),
                    pressed,
                })
                .unwrap();
            front.pump();
        }
    }
    tap(&mut front, "G");
    wait(&mut front, |f| {
        f.state
            .as_ref()
            .is_some_and(|s| s.status.starts_with("grant applied "))
    });
    let revision = front
        .state
        .as_ref()
        .unwrap()
        .snapshot
        .as_ref()
        .unwrap()
        .authority
        .revision
        .parse::<u64>()
        .unwrap();
    tap(&mut front, "M");
    wait(&mut front, |f| {
        f.state.as_ref().is_some_and(|s| s.review.is_some())
    });
    tap(&mut front, "Enter");
    wait(&mut front, |f| {
        f.fresh()
            && f.state
                .as_ref()
                .unwrap()
                .snapshot
                .as_ref()
                .unwrap()
                .authority
                .revision
                .parse::<u64>()
                .unwrap()
                == revision + 1
    });
    assert!(
        front
            .state
            .as_ref()
            .unwrap()
            .snapshot
            .as_ref()
            .unwrap()
            .authority
            .parameters
            .iter()
            .any(|p| p.target.input == "input-01"
                && p.target.parameter == "mute"
                && p.target_value == true)
    );
    wait(&mut front, |f| {
        f.modules.as_ref().is_some_and(|m| {
            m.fresh()
                && m.status.as_ref().is_some_and(|s| {
                    s.available && s.physical == "unverified" && s.source_fault == 0
                })
        })
    });
    front.page = shr_desk::model::Page::Analysis;
    let scene = front.scene();
    assert!(scene.in_bounds());
    let text = scene
        .primitives
        .iter()
        .filter_map(|p| {
            if let shr_desk::render::Primitive::Text { value, .. } = p {
                Some(value.as_str())
            } else {
                None
            }
        })
        .collect::<Vec<_>>()
        .join("\n");
    assert!(text.contains("MAIN ONLY"));
    assert!(text.contains("durable UNKNOWN"));
    assert!(text.contains("RAW MIXER"));
    assert!(text.contains("REC/PA/FX writable controls and analysis unavailable"));
    #[cfg(feature = "native")]
    if std::env::var("VK_DRIVER_FILES").as_deref() == Ok("/usr/share/vulkan/icd.d/lvp_icd.json") {
        println!(
            "actual module Health CPU frame: {}",
            shr_desk::native::offscreen(&scene).unwrap()
        );
    }
    if let Some(path) = std::env::var_os("SHR_DESK_EVIDENCE") {
        shr_desk::raster::ppm(
            &scene,
            &std::path::PathBuf::from(path).join("real-modules-health.ppm"),
        )
        .unwrap();
    }
    println!(
        "actual read-only GP05 health: {}",
        serde_json::to_string(&status).unwrap()
    );
    drop(front);
}

#[test]
fn end_to_end_query_deadline_stall_drip_backlog_and_worker_join() {
    use std::{
        fs,
        io::{Read, Write},
        os::unix::{fs::PermissionsExt, net::UnixListener},
        thread,
        time::{Duration, Instant},
    };
    for mode in ["stall", "drip", "backlog", "join"] {
        let dir = std::env::temp_dir().join(format!(
            "desk-health-deadline-{}-{mode}",
            std::process::id()
        ));
        fs::create_dir(&dir).unwrap();
        fs::set_permissions(&dir, fs::Permissions::from_mode(0o700)).unwrap();
        let socket = dir.join("audio.sock");
        let listener = UnixListener::bind(&socket).unwrap();
        fs::set_permissions(&socket, fs::Permissions::from_mode(0o600)).unwrap();
        let (accepted, ready) = std::sync::mpsc::channel();
        let child = thread::spawn(move || {
            let (mut stream, _) = listener.accept().unwrap();
            stream
                .set_read_timeout(Some(Duration::from_secs(2)))
                .unwrap();
            stream
                .set_write_timeout(Some(Duration::from_millis(50)))
                .unwrap();
            let mut h = [0; 4];
            stream.read_exact(&mut h).unwrap();
            let mut request = vec![0; u32::from_be_bytes(h) as usize];
            stream.read_exact(&mut request).unwrap();
            accepted.send(()).unwrap();
            if mode == "stall" || mode == "join" {
                thread::sleep(Duration::from_millis(350));
            } else if mode == "drip" {
                let b = serde_json::to_vec(&fixture()).unwrap();
                stream.write_all(&(b.len() as u32).to_be_bytes()).unwrap();
                for byte in b.iter().take(8) {
                    if stream.write_all(&[*byte]).is_err() {
                        break;
                    }
                    thread::sleep(Duration::from_millis(40));
                }
            } else {
                let v: Value =
                    serde_json::from_slice(include_bytes!("fixtures/gp03/v1/e03-rendered.json"))
                        .unwrap();
                let b = serde_json::to_vec(&v["grant_response"]).unwrap();
                for _ in 0..80 {
                    if stream
                        .write_all(&(b.len() as u32).to_be_bytes())
                        .and_then(|_| stream.write_all(&b))
                        .is_err()
                    {
                        break;
                    }
                }
            }
        });
        let config = shr_desk::frontend::Config {
            endpoint: socket,
            show: SHOW.into(),
            epoch: 9,
            writer: "health-deadline".into(),
            scope: "foh".into(),
        };
        let began = Instant::now();
        if mode == "join" {
            let worker = modules::Worker::start(config);
            ready.recv_timeout(Duration::from_secs(2)).unwrap();
            drop(worker);
        } else {
            assert!(modules::query(&config).is_err());
        }
        assert!(
            began.elapsed() < Duration::from_millis(350),
            "{mode} exceeded bounded end-to-end deadline: {:?}",
            began.elapsed()
        );
        child.join().unwrap();
        fs::remove_dir_all(dir).unwrap();
    }
    let mut v = fixture();
    v["source_fault"] = json!(5);
    assert!(decode(&v).is_err());
    v["source_fault"] = json!(7);
    assert!(decode(&v).is_ok());
    v["recording"]["host_fault"] = json!(u32::MAX);
    assert!(decode(&v).is_ok());
}
