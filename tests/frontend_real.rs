//! Opt-in actual accepted GP03 service interoperability; private synthetic endpoints only.
use shr_desk::frontend::{Config, Event, Frontend};
use std::{
    fs,
    os::unix::fs::PermissionsExt,
    path::PathBuf,
    process::{Child, Command, Stdio},
    thread,
    time::{Duration, Instant},
};
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
#[track_caller]
fn tick_until(f: &mut Frontend, mut predicate: impl FnMut(&Frontend) -> bool) {
    let end = Instant::now() + Duration::from_secs(5);
    loop {
        f.pump();
        if predicate(f) {
            return;
        }
        assert!(
            Instant::now() < end,
            "frontend timeout: {:?} / {}",
            f.state,
            f.message
        );
        thread::sleep(Duration::from_millis(10));
    }
}
fn tap(f: &mut Frontend, key: &str) {
    eprintln!(
        "actual action {key} at {:?}: generation/status {:?}",
        Instant::now(),
        f.state.as_ref().map(|s| (s.generation, &s.status))
    );
    if matches!(
        key,
        "G" | "Q" | "+" | "M" | "H" | "R" | "1" | "2" | "3" | "Enter"
    ) {
        tick_until(f, |f| f.fresh());
    }
    if key == "Enter" {
        let _ = f.scene();
        f.mark_presented();
    }
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
fn revision(f: &Frontend) -> u64 {
    f.state
        .as_ref()
        .unwrap()
        .snapshot
        .as_ref()
        .unwrap()
        .authority
        .revision
        .parse()
        .unwrap()
}
fn target(f: &Frontend, parameter: &str) -> serde_json::Value {
    f.state
        .as_ref()
        .unwrap()
        .snapshot
        .as_ref()
        .unwrap()
        .authority
        .parameters
        .iter()
        .find(|p| {
            p.target.input == "input-01"
                && p.target.parameter == parameter
                && p.target.monitor.is_none()
        })
        .unwrap()
        .target_value
        .clone()
}
#[test]
#[ignore = "requires exact independently accepted provider binary through SHR_DESK_GP03; no physical endpoints"]
fn real_grant_controls_engine_review_and_no_replay_reconnect() {
    let binary = std::env::var_os("SHR_DESK_GP03").expect("accepted provider path required");
    let dir = std::env::temp_dir().join(format!(
        "desk-real-front-{}-{}",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    ));
    fs::create_dir(&dir).unwrap();
    fs::set_permissions(&dir, fs::Permissions::from_mode(0o700)).unwrap();
    let show = "11111111-1111-4111-8111-111111111111";
    let child = Command::new(binary)
        .args([
            "--directory",
            dir.to_str().unwrap(),
            "--show",
            show,
            "--epoch",
            "10",
            "--ticks",
            "60000",
        ])
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
    let config = Config {
        endpoint: dir.join("audio.sock"),
        show: show.into(),
        epoch: 10,
        writer: "desk-native-real".into(),
        scope: "foh".into(),
    };
    let mut role_config = None;
    let mut f = Frontend::new(config.clone());
    if let Some(executable) = std::env::var_os("SHR_DESK_GP09") {
        let role_dir = dir.join("roles");
        fs::create_dir(&role_dir).unwrap();
        fs::set_permissions(&role_dir, fs::Permissions::from_mode(0o700)).unwrap();
        let rc = shr_desk::roles::Config {
            executable: executable.into(),
            directory: role_dir,
            acquire: shr_desk::roles::decode_acquire(include_bytes!(
                "fixtures/gp09/v1/acquire-a.json"
            ))
            .unwrap(),
        };
        f.attach_role(rc.clone());
        role_config = Some(rc);
        tick_until(&mut f, |f| f.role_status.as_ref().is_some_and(|r| r.live));
    }

    tick_until(&mut f, |f| f.fresh());
    if let Some(path) = std::env::var_os("SHR_DESK_EVIDENCE") {
        shr_desk::raster::ppm(&f.scene(), &PathBuf::from(path).join("real-provider.ppm")).unwrap();
    }
    assert!(f.state.as_ref().unwrap().status.contains("read-only"));
    tick_until(&mut f, |f| {
        f.modules.as_ref().is_some_and(|m| m.status.is_none())
    });
    assert!(f.modules.as_ref().unwrap().message.contains("UNAVAILABLE"));
    tick_until(&mut f, |f| f.fresh());
    let initial = target(&f, "fader").as_i64().unwrap();
    let r = revision(&f);
    tap(&mut f, "G");
    tick_until(&mut f, |f| {
        f.state
            .as_ref()
            .is_some_and(|s| s.status.starts_with("grant applied "))
    });
    thread::sleep(Duration::from_millis(80));
    f.pump();
    tap(&mut f, "+");
    tick_until(&mut f, |f| {
        f.fresh() && target(f, "fader").as_i64() == Some(initial + 1000)
    });
    assert_eq!(revision(&f), r + 1);
    let r = revision(&f);
    tap(&mut f, "M");
    tick_until(&mut f, |f| {
        f.state.as_ref().is_some_and(|s| s.review.is_some())
    });
    tap(&mut f, "Enter");
    tick_until(&mut f, |f| f.fresh() && target(f, "mute") == true);
    assert_eq!(revision(&f), r + 1);
    tap(&mut f, "M");
    tick_until(&mut f, |f| {
        f.state.as_ref().is_some_and(|s| s.review.is_some())
    });
    tap(&mut f, "Enter");
    tick_until(&mut f, |f| f.fresh() && target(f, "mute") == false);
    tap(&mut f, "A");
    tap(&mut f, "2");
    tick_until(&mut f, |f| {
        f.state.as_ref().is_some_and(|s| s.review.is_some())
    });
    assert!(
        f.state
            .as_ref()
            .unwrap()
            .review
            .as_ref()
            .unwrap()
            .1
            .contains("assist")
    );
    tap(&mut f, "Enter");
    tick_until(&mut f, |f| {
        f.fresh()
            && f.state
                .as_ref()
                .unwrap()
                .snapshot
                .as_ref()
                .unwrap()
                .authority
                .modes
                .iter()
                .any(|(scope, mode)| scope == "foh" && mode == "assist")
    });
    // Hand off authority explicitly to an actual synthetic proposal producer.
    tap(&mut f, "Q");
    tick_until(&mut f, |f| {
        f.state
            .as_ref()
            .is_some_and(|s| s.status.starts_with("release applied "))
    });
    let seed = dir.join("proposal.txt");
    fs::write(&seed,"grant\ninput-release\njson propose {\"targets\":[{\"target\":{\"input\":\"input-01\",\"parameter\":\"fader\"},\"value\":-6000}]}\nstatus\nrelease\n").unwrap();
    let seeded = Command::new(env!("CARGO_BIN_EXE_shr-desk"))
        .args([
            "--audio-local",
            dir.join("audio.sock").to_str().unwrap(),
            show,
            "10",
            "proposal-seed",
            "foh",
            "--script",
            seed.to_str().unwrap(),
        ])
        .output()
        .unwrap();
    assert!(
        seeded.status.success(),
        "{}",
        String::from_utf8_lossy(&seeded.stderr)
    );
    let seed_output = String::from_utf8_lossy(&seeded.stdout);
    let seeded_status = seed_output
        .lines()
        .filter_map(|line| serde_json::from_str::<serde_json::Value>(line).ok())
        .find(|status| {
            status["result"]
                .as_str()
                .is_some_and(|r| r.starts_with("proposal stored;"))
        })
        .unwrap_or_else(|| panic!("proposal was not stored: {seed_output}"));
    assert!(
        seeded_status["authority"]["parameters"]
            .as_array()
            .unwrap()
            .iter()
            .any(|p| {
                p["target"]["input"] == "input-01"
                    && p["target"]["parameter"] == "fader"
                    && p["proposal"] == -6000
            }),
        "stored provider proposal missing: {seed_output}"
    );
    tap(&mut f, "F5");
    tick_until(&mut f, |f| {
        f.fresh() && f.state.as_ref().unwrap().status.contains("read-only")
    });
    assert!(
        f.state
            .as_ref()
            .unwrap()
            .snapshot
            .as_ref()
            .unwrap()
            .authority
            .parameters
            .iter()
            .any(|p| p.target.input == "input-01"
                && p.target.parameter == "fader"
                && p.proposal == Some(serde_json::json!(-6000)))
    );
    tap(&mut f, "G");
    tick_until(&mut f, |f| {
        f.state
            .as_ref()
            .is_some_and(|s| s.status.starts_with("grant applied "))
    });
    tap(&mut f, "R");
    tick_until(&mut f, |f| {
        f.state.as_ref().is_some_and(|s| s.review.is_some())
    });
    assert!(
        f.state
            .as_ref()
            .unwrap()
            .review
            .as_ref()
            .unwrap()
            .1
            .contains("ENGINE ramp 240")
    );
    let reviewed_scene = f.scene();
    let r = revision(&f);
    tap(&mut f, "Enter");
    tick_until(&mut f, |f| f.fresh() && revision(f) > r);
    if let Some(path) = std::env::var_os("SHR_DESK_EVIDENCE") {
        shr_desk::raster::ppm(
            &reviewed_scene,
            &PathBuf::from(path).join("real-confirmation.ppm"),
        )
        .unwrap();
    }
    #[cfg(feature = "native")]
    if std::env::var("VK_DRIVER_FILES").as_deref() == Ok("/usr/share/vulkan/icd.d/lvp_icd.json") {
        println!(
            "actual reviewed provider scene: {}",
            shr_desk::native::offscreen(&reviewed_scene).unwrap()
        );
        println!(
            "actual confirmed provider scene: {}",
            shr_desk::native::offscreen(&f.scene()).unwrap()
        );
    }
    assert_eq!(target(&f, "fader"), serde_json::json!(-6000));
    let preserved = target(&f, "fader");
    let r = revision(&f);
    if let Some(mut role) = role_config {
        let pid = f.role_child_pid().unwrap();
        f.enqueue(Event::Key {
            key: "+".into(),
            pressed: true,
        })
        .unwrap();
        let loss_at = Instant::now();
        assert!(
            Command::new("kill")
                .args(["-KILL", &pid.to_string()])
                .status()
                .unwrap()
                .success()
        );
        // Leave the old input queued until asynchronous broker verification fences it.
        thread::sleep(Duration::from_millis(650));
        tick_until(&mut f, |f| f.role_status.as_ref().is_some_and(|r| !r.live));
        assert!(loss_at.elapsed() < Duration::from_millis(1700));
        assert!(f.leds.is_none());
        f.enqueue(Event::Key {
            key: "+".into(),
            pressed: false,
        })
        .unwrap();
        f.pump();
        assert!(f.state.as_ref().is_none_or(|s| {
            s.snapshot
                .as_ref()
                .is_none_or(|s| s.authority.revision == r.to_string())
        }));
        tap(&mut f, "+");
        assert!(f.message.contains("live GP09"));
        role.acquire["expected_generation"] = serde_json::json!("1");
        f.attach_role(role);
        tick_until(&mut f, |f| {
            f.role_status
                .as_ref()
                .is_some_and(|r| r.live && r.generation.as_deref() == Some("2"))
        });
    }
    tap(&mut f, "F5");
    tick_until(&mut f, |f| {
        f.fresh() && f.state.as_ref().unwrap().status.contains("read-only")
    });
    assert_eq!(target(&f, "fader"), preserved);
    assert_eq!(revision(&f), r);
    tap(&mut f, "+");
    tick_until(&mut f, |f| {
        f.state
            .as_ref()
            .is_some_and(|s| s.status.contains("lease expired"))
    });
    assert_eq!(revision(&f), r);
    drop(f);
    let mut fresh = Frontend::new(Config {
        writer: "desk-new-readonly".into(),
        ..config
    });
    tick_until(&mut fresh, |f| f.fresh());
    assert_eq!(target(&fresh, "fader"), preserved);
    assert_eq!(revision(&fresh), r);
    drop(fresh);
}
