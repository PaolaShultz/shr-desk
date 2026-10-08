//! Explicit actual GP03 synthetic-service acceptance. Never opens physical devices.
use serde_json::{Value, json};
use shr_desk::{
    actions::Action,
    exact_value::Parameter,
    frontend::{Config, Event, Frontend},
    render::Primitive,
};
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
fn until(f: &mut Frontend, mut p: impl FnMut(&Frontend) -> bool) {
    let end = Instant::now() + Duration::from_secs(5);
    loop {
        f.pump();
        if p(f) {
            return;
        }
        assert!(
            Instant::now() < end,
            "actual timeout {:?} / {}",
            f.state,
            f.message
        );
        thread::sleep(Duration::from_millis(10));
    }
}
fn tap(f: &mut Frontend, key: &str) {
    if matches!(key, "D" | "C" | "d" | "c" | "G" | "F4" | "Enter") {
        until(f, |f| f.fresh());
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
fn raw(f: &Frontend) -> &shr_desk::audio::RenderedSnapshot {
    f.state.as_ref().unwrap().snapshot.as_ref().unwrap()
}
fn revision(f: &Frontend) -> String {
    raw(f).authority.revision.clone()
}
fn params(f: &Frontend, input: &str) -> Value {
    json!(
        raw(f)
            .authority
            .parameters
            .iter()
            .filter(|p| p.target.input == input)
            .collect::<Vec<_>>()
    )
}
fn coeff(f: &Frontend, input: &str) -> Value {
    json!(
        raw(f)
            .coefficients
            .iter()
            .find(|c| c.input == input)
            .unwrap()
    )
}
fn save(f: &Frontend, label: &str, dir: &std::path::Path) {
    let scene = f.scene();
    assert!(scene.in_bounds());
    fs::write(
        dir.join(format!("{label}.svg")),
        shr_desk::render::svg(&scene),
    )
    .unwrap();
    shr_desk::raster::ppm(&scene, &dir.join(format!("{label}.ppm"))).unwrap();
    fs::write(dir.join(format!("{label}.json")),serde_json::to_vec_pretty(&json!({"operation":f.state.as_ref().unwrap().last_operation,"raw":raw(f),"message":f.message})).unwrap()).unwrap();
    #[cfg(feature = "native")]
    if std::env::var("VK_DRIVER_FILES").as_deref() == Ok("/usr/share/vulkan/icd.d/lvp_icd.json") {
        println!("{label}: {}", shr_desk::native::offscreen(&scene).unwrap());
    }
}
#[test]
#[ignore = "requires explicit SHA256-pinned actual GP03 executable and new private evidence directory"]
fn actual_exact_foh_keyboard_semantics_review_coefficients_cancel_reconnect() {
    let binary = PathBuf::from(std::env::var_os("SHR_DESK_GP03").expect("actual provider path"));
    let expected =
        std::env::var("SHR_DESK_GP03_SHA256").expect("accepted provider SHA256 required");
    let hash = Command::new("sha256sum").arg(&binary).output().unwrap();
    assert!(hash.status.success());
    assert_eq!(
        String::from_utf8(hash.stdout)
            .unwrap()
            .split_whitespace()
            .next()
            .unwrap(),
        expected
    );
    let evidence = PathBuf::from(
        std::env::var_os("SHR_DESK_EXACT_EVIDENCE").expect("new private evidence directory"),
    );
    fs::create_dir(&evidence).unwrap();
    let dir = std::env::temp_dir().join(format!(
        "desk-exact-real-{}-{}",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    ));
    fs::create_dir(&dir).unwrap();
    fs::set_permissions(&dir, fs::Permissions::from_mode(0o700)).unwrap();
    let show = "11111111-1111-4111-8111-111111111111";
    let child = Command::new(&binary)
        .args([
            "--directory",
            dir.to_str().unwrap(),
            "--show",
            show,
            "--epoch",
            "19",
            "--ticks",
            "60000",
        ])
        .stdout(Stdio::null())
        .stderr(Stdio::from(
            fs::File::create(evidence.join("provider.stderr")).unwrap(),
        ))
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
    let mut f = Frontend::new(Config {
        wire_version: 1,
        remote: None,
        endpoint: dir.join("audio.sock"),
        show: show.into(),
        epoch: 19,
        writer: "exact-real".into(),
        scope: "foh".into(),
    });
    until(&mut f, |f| f.fresh());
    assert!(!f.state.as_ref().unwrap().writer_granted());
    let modes = raw(&f).authority.modes.clone();
    let unrelated_params = params(&f, "input-02");
    let unrelated_coeff = coeff(&f, "input-02");
    tap(&mut f, "G");
    until(&mut f, |f| {
        f.fresh()
            && f.state
                .as_ref()
                .unwrap()
                .last_operation
                .as_deref()
                .is_some_and(|r| r.starts_with("grant applied "))
    });
    for (parameter, text, value, key) in [
        (Parameter::Fader, "-6.1", -6100, "d"),
        (Parameter::Pan, "-31", -31, "C"),
    ] {
        until(&mut f, |f| f.fresh());
        let before = revision(&f);
        let before_params = params(&f, "input-01");
        let before_coeff = coeff(&f, "input-01");
        if parameter == Parameter::Fader {
            tap(&mut f, key);
            for c in text.chars() {
                tap(&mut f, &c.to_string());
            }
            tap(&mut f, "Enter");
        } else {
            tap(&mut f, "F2");
            until(&mut f, |f| {
                f.fresh()
                    && f.state
                        .as_ref()
                        .unwrap()
                        .last_operation
                        .as_deref()
                        .is_some_and(|r| r.starts_with("CANCELLED;"))
            });
            f.inject_controller(Action::ExactEdit(parameter)).unwrap();
            f.pump();
            f.inject_controller(Action::ExactText(text.into())).unwrap();
            f.pump();
        }
        assert!(f.exact_draft.is_some(), "editor admission: {}", f.message);
        assert_eq!(f.exact_draft.as_ref().unwrap().value, Some(value));
        save(&f, &format!("{}-draft", parameter.name()), &evidence);
        let frame = raw(&f).frame.parse::<u64>().unwrap();
        until(&mut f, |f| {
            f.fresh() && raw(f).frame.parse::<u64>().unwrap() > frame + 240
        });
        assert_eq!(revision(&f), before);
        assert_eq!(params(&f, "input-01"), before_params);
        assert_eq!(coeff(&f, "input-01"), before_coeff);
        tap(&mut f, "F4");
        until(&mut f, |f| {
            f.state.as_ref().is_some_and(|u| u.review.is_some())
        });
        let review = &f.state.as_ref().unwrap().review.as_ref().unwrap().1;
        assert!(
            review.contains("input-01")
                && review.contains(parameter.name())
                && review.contains(if parameter == Parameter::Fader {
                    "-6.1 dB"
                } else {
                    "left 31%"
                })
                && review.contains(show)
                && review.contains("epoch 19")
                && review.contains("scope foh")
        );
        let scene = f.scene();
        let rendered = scene
            .primitives
            .iter()
            .filter_map(|p| {
                if let Primitive::Text { value, .. } = p {
                    Some(value.as_str())
                } else {
                    None
                }
            })
            .collect::<Vec<_>>()
            .join("\n");
        assert!(rendered.contains("proposed"));
        let displayed = rendered.replace('\n', "");
        for field in ["input-01", parameter.name(), show, "epoch 19", "scope foh"] {
            assert!(
                displayed.contains(field),
                "full displayed review lacks {field}"
            );
        }
        save(&f, &format!("{}-review", parameter.name()), &evidence);
        // Confirm before presented refuses; matching target cannot stand in for completion.
        tap(&mut f, "Enter");
        assert!(f.message.contains("review every displayed page"));
        assert_eq!(revision(&f), before);
        f.mark_presented();
        let expected_revision = (before.parse::<u64>().unwrap() + 1).to_string();
        let expected_result = format!("set applied revision {expected_revision}");
        tap(&mut f, "Enter");
        until(&mut f, |f| {
            f.fresh()
                && f.state.as_ref().unwrap().last_operation.as_deref()
                    == Some(expected_result.as_str())
                && revision(f) == expected_revision
        });
        assert!(
            raw(&f)
                .authority
                .parameters
                .iter()
                .any(|p| p.target.input == "input-01"
                    && p.target.parameter == parameter.name()
                    && p.target_value == json!(value)
                    && p.hold == Some(json!(value)))
        );
        until(&mut f, |f| {
            let c = raw(f)
                .coefficients
                .iter()
                .find(|c| c.input == "input-01")
                .unwrap();
            f.fresh() && c.current_nanogain == c.ramp_target_nanogain
        });
        let c = raw(&f)
            .coefficients
            .iter()
            .find(|c| c.input == "input-01")
            .unwrap();
        let gain = (10f64.powf(-6100. / 20000.) * 1e9).round() as u64;
        assert!(c.current_nanogain[0].0.abs_diff(gain) <= 1);
        let pan = if parameter == Parameter::Fader {
            0
        } else {
            -31
        };
        let angle = f64::from(pan + 100) * std::f64::consts::PI / 400.;
        assert!(
            c.current_nanogain[1]
                .0
                .abs_diff((angle.cos() * 1e9).round() as u64)
                <= 1
        );
        assert!(
            c.current_nanogain[2]
                .0
                .abs_diff((angle.sin() * 1e9).round() as u64)
                <= 1
        );
        assert_eq!(raw(&f).authority.modes, modes);
        assert_eq!(params(&f, "input-02"), unrelated_params);
        assert_eq!(coeff(&f, "input-02"), unrelated_coeff);
        save(&f, &format!("{}-applied", parameter.name()), &evidence);
    }
    let before = revision(&f);
    let preserved = params(&f, "input-01");
    tap(&mut f, "D");
    assert!(
        f.exact_draft.is_some(),
        "cancel draft admission: {}",
        f.message
    );
    f.inject_controller(Action::ExactText("-7.2".into()))
        .unwrap();
    f.pump();
    tap(&mut f, "Esc");
    until(&mut f, |f| {
        f.fresh()
            && f.state
                .as_ref()
                .unwrap()
                .last_operation
                .as_deref()
                .is_some_and(|r| r.starts_with("CANCELLED;"))
    });
    assert!(f.exact_draft.is_none());
    assert_eq!(revision(&f), before);
    assert_eq!(params(&f, "input-01"), preserved);
    tap(&mut f, "C");
    assert!(
        f.exact_draft.is_some(),
        "reconnect draft admission: {}",
        f.message
    );
    f.inject_controller(Action::ExactText("42".into())).unwrap();
    f.pump();
    tap(&mut f, "F5");
    until(&mut f, |f| {
        f.fresh() && f.state.as_ref().unwrap().status.contains("read-only")
    });
    assert!(!f.state.as_ref().unwrap().writer_granted());
    assert_eq!(f.exact_draft.as_ref().unwrap().text, "42");
    tap(&mut f, "F4");
    assert!(f.message.contains("context changed"));
    assert_eq!(revision(&f), before);
    assert_eq!(params(&f, "input-01"), preserved);
    let frame = raw(&f).frame.parse::<u64>().unwrap();
    until(&mut f, |f| {
        f.fresh() && raw(f).frame.parse::<u64>().unwrap() > frame + 240
    });
    assert_eq!(revision(&f), before);
    tap(&mut f, "c");
    assert_eq!(f.exact_draft.as_ref().unwrap().text, "42");
    tap(&mut f, "F4");
    assert!(!f.state.as_ref().unwrap().writer_granted());
    assert_eq!(revision(&f), before);
    assert_eq!(params(&f, "input-01"), preserved);
    save(&f, "reconnect-readonly-no-replay", &evidence);
    println!(
        "ACTUAL_EXACT accepted provider {} SHA256 {} / final revision {} / independent correlated completion, advancing readback and settled coefficients",
        binary.display(),
        expected,
        before
    );
    drop(f);
    drop(service);
}
