//! Device-free frontend integration using accepted provider-encoded state.
use serde_json::{Value, json};
use shr_desk::{
    frontend::{Config, Event, Frontend},
    raster,
    render::{Primitive, Scene},
};
use std::{
    fs,
    io::{Read, Write},
    os::unix::{fs::PermissionsExt, net::UnixListener},
    sync::{Arc, Mutex},
    thread,
    time::{Duration, Instant},
};
struct Peer {
    dir: std::path::PathBuf,
    child: Option<thread::JoinHandle<()>>,
    seen: Arc<Mutex<Vec<String>>>,
}
impl Drop for Peer {
    fn drop(&mut self) {
        if let Some(c) = self.child.take() {
            c.join().unwrap();
        }
        fs::remove_dir_all(&self.dir).unwrap();
    }
}
fn peer() -> (Peer, Config) {
    peer_delayed(false)
}
fn peer_delayed(drop_grant: bool) -> (Peer, Config) {
    let dir = std::env::temp_dir().join(format!(
        "desk-front-{}-{}",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    ));
    fs::create_dir(&dir).unwrap();
    fs::set_permissions(&dir, fs::Permissions::from_mode(0o700)).unwrap();
    let endpoint = dir.join("audio.sock");
    let listener = UnixListener::bind(&endpoint).unwrap();
    fs::set_permissions(&endpoint, fs::Permissions::from_mode(0o600)).unwrap();
    let seen = Arc::new(Mutex::new(Vec::new()));
    let copy = seen.clone();
    let child = thread::spawn(move || {
        let (mut s, _) = listener.accept().unwrap();
        s.set_read_timeout(Some(Duration::from_secs(5))).unwrap();
        let corpus: Value =
            serde_json::from_str(include_str!("fixtures/gp03/v1/e03-rendered.json")).unwrap();
        loop {
            let mut n = [0; 4];
            if s.read_exact(&mut n).is_err() {
                break;
            }
            let n = u32::from_be_bytes(n) as usize;
            assert!(n <= 65536);
            let mut b = vec![0; n];
            s.read_exact(&mut b).unwrap();
            let request: Value = serde_json::from_slice(&b).unwrap();
            let kind = request["kind"].as_str().unwrap().to_string();
            copy.lock().unwrap().push(kind.clone());
            if drop_grant && kind == "grant" {
                continue;
            }
            assert_eq!(kind, "snapshot", "read-only tests must never mutate");
            assert!(request["writer"].is_null());
            let mut r = corpus["grant_response"].clone();
            for k in ["writer", "lease", "request_id", "expected_revision"] {
                r["context"][k] = Value::Null;
                r["outcome"][k] = Value::Null;
            }
            for k in ["granted_lease", "lease_remaining_ms", "scope"] {
                r["outcome"]["body"][k] = Value::Null;
            }
            r["snapshot"] = corpus["initial"].clone();
            let b = serde_json::to_vec(&r).unwrap();
            if s.write_all(&(b.len() as u32).to_be_bytes()).is_err() || s.write_all(&b).is_err() {
                break;
            }
        }
    });
    let config = Config {
        wire_version: 1,
        remote: None,
        endpoint,
        show: "11111111-1111-4111-8111-111111111111".into(),
        epoch: 9,
        writer: "desk-cli".into(),
        scope: "foh".into(),
    };
    (
        Peer {
            dir,
            child: Some(child),
            seen,
        },
        config,
    )
}
fn wait(f: &mut Frontend) {
    let until = Instant::now() + Duration::from_secs(3);
    while !f.fresh() {
        f.pump();
        assert!(Instant::now() < until, "{:?}", f.state);
        thread::sleep(Duration::from_millis(10));
    }
}
fn text(scene: &Scene) -> String {
    scene
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
        .join("\n")
}
#[test]
fn real_provider_scene_has_exact_authority_coefficients_and_no_simulator() {
    let (p, c) = peer();
    let mut f = Frontend::new(c);
    wait(&mut f);
    let s = f.scene();
    assert!(s.in_bounds());
    let t = text(&s);
    assert!(t.contains("REAL GP03"));
    assert_eq!(
        f.state
            .as_ref()
            .unwrap()
            .snapshot
            .as_ref()
            .unwrap()
            .authority
            .revision,
        "12"
    );
    assert!(t.contains("-6.0 dB"));
    assert!(t.contains("center"));
    assert!(!t.contains("Some("));
    assert!(!t.contains("None"));
    assert!(t.contains("input-01"));
    assert!(t.contains("RENDERED GAIN"));
    assert!(!t.contains("SIMULATION"));
    assert!(t.contains("METERS UNAVAILABLE"));
    assert_eq!(
        f.state
            .as_ref()
            .unwrap()
            .snapshot
            .as_ref()
            .unwrap()
            .authority
            .parameters[0]
            .target_value,
        json!(-6000)
    );
    drop(f);
    assert!(!p.seen.lock().unwrap().is_empty());
}
#[test]
fn focus_overflow_resize_and_device_recovery_fence_input_without_recalls() {
    let (_p, c) = peer();
    let mut f = Frontend::new(c);
    wait(&mut f);
    let g = f.provider.generation();
    f.enqueue(Event::Key {
        key: "G".into(),
        pressed: true,
    })
    .unwrap();
    f.enqueue(Event::Focus(false)).unwrap();
    assert!(f.provider.generation() > g);
    f.pump();
    f.enqueue(Event::Focus(true)).unwrap();
    f.enqueue(Event::Key {
        key: "G".into(),
        pressed: true,
    })
    .unwrap();
    f.pump(); // Held G blocked; no grant.
    f.enqueue(Event::Resize(0, 0)).unwrap();
    assert_eq!((f.width, f.height), (0, 0));
    f.enqueue(Event::Resize(640, 360)).unwrap();
    f.enqueue(Event::DeviceLost).unwrap();
    assert!(!f.device_ready);
    f.enqueue(Event::DeviceRestored).unwrap();
    assert!(f.device_ready);
    for _ in 0..64 {
        f.enqueue(Event::Key {
            key: "+".into(),
            pressed: true,
        })
        .unwrap();
    }
    assert!(
        f.enqueue(Event::Key {
            key: "+".into(),
            pressed: true
        })
        .is_err()
    );
    f.pump();
    assert!(f.message.contains("OVERFLOW"));
    drop(f);
}
#[test]
fn displayed_freshness_expires_and_confirmation_without_review_refused() {
    let (_p, c) = peer();
    let mut f = Frontend::new(c);
    wait(&mut f);
    f.enqueue(Event::Key {
        key: "Enter".into(),
        pressed: true,
    })
    .unwrap();
    f.pump();
    assert!(f.message.contains("reviewed"));
    f.state.as_mut().unwrap().received = Instant::now() - Duration::from_secs(1);
    assert!(!f.fresh());
    assert!(text(&f.scene()).contains("EDITS DISABLED"));
    drop(f);
}
#[test]
fn bitmap_raster_clips_and_uses_bundled_glyphs() {
    let s = Scene {
        primitives: vec![
            Primitive::Rect {
                x: 0,
                y: 0,
                w: 1920,
                h: 1080,
                fill: "#10151d",
            },
            Primitive::Text {
                x: 0,
                y: 0,
                value: "A".into(),
                color: "#ffffff",
            },
            Primitive::Rect {
                x: 1919,
                y: 1079,
                w: 200,
                h: 200,
                fill: "#ff0000",
            },
        ],
    };
    let pixels = raster::rgba(&s);
    assert_eq!(pixels.len(), 1920 * 1080 * 4);
    assert_eq!(&pixels[pixels.len() - 4..], &[255, 0, 0, 255]);
    let ink = pixels
        .chunks_exact(4)
        .filter(|p| p[..3] == [255, 255, 255])
        .count();
    assert!(ink > 20 && ink < 288);
}
#[cfg(feature = "native")]
#[test]
fn offscreen_never_discovers_adapters_without_explicit_cpu_icd() {
    if std::env::var("VK_DRIVER_FILES").as_deref() != Ok("/usr/share/vulkan/icd.d/lvp_icd.json") {
        assert!(
            shr_desk::native::offscreen(&Scene::default())
                .unwrap_err()
                .contains("offscreen")
        );
        return;
    }
    let s = Scene {
        primitives: vec![
            Primitive::Rect {
                x: 0,
                y: 0,
                w: 1920,
                h: 1080,
                fill: "#10151d",
            },
            Primitive::Text {
                x: 24,
                y: 24,
                value: "REAL PROVIDER / TERMINUS / GPU CHECK".into(),
                color: "#66dfd3",
            },
            Primitive::Line {
                x1: 0,
                y1: 0,
                x2: 1919,
                y2: 1079,
                color: "#ffffff",
            },
        ],
    };
    let result = shr_desk::native::offscreen(&s).unwrap();
    assert!(result.contains("exact RGBA match"));
    println!("{result}");
}

#[test]
fn monitor_pan_refuses_without_retargeting_send() {
    let (_p, mut c) = peer();
    c.scope = "monitor1".into();
    let mut f = Frontend::new(c);
    wait(&mut f);
    f.enqueue(Event::Key {
        key: "]".into(),
        pressed: true,
    })
    .unwrap();
    f.pump();
    assert!(f.message.contains("pan unavailable"));
    drop(f);
}
#[cfg(feature = "native")]
#[test]
fn viewport_preserves_aspect_portrait_narrow_minimized_and_integer_upscale() {
    for (w, h) in [(400, 1200), (100, 60), (1920, 1080), (5000, 3000)] {
        let (x, y, vw, vh) = shr_desk::native::viewport(w, h);
        assert!(x >= 0.0 && y >= 0.0);
        assert!(x + vw <= w as f32 + 0.1 && y + vh <= h as f32 + 0.1);
        assert!((vw / vh - 1920.0 / 1080.0).abs() < 0.001);
    }
    assert_eq!(shr_desk::native::viewport(0, 100), (0.0, 0.0, 0.0, 0.0));
    assert_eq!(shr_desk::native::viewport(5000, 3000).2, 3840.0);
}

#[test]
fn focus_and_overflow_during_inflight_grant_stop_every_retry() {
    for overflow in [false, true] {
        let (p, c) = peer_delayed(true);
        let mut f = Frontend::new(c);
        wait(&mut f);
        f.enqueue(Event::Key {
            key: "G".into(),
            pressed: true,
        })
        .unwrap();
        f.pump();
        let end = Instant::now() + Duration::from_secs(2);
        while !p.seen.lock().unwrap().iter().any(|k| k == "grant") {
            assert!(Instant::now() < end);
            thread::sleep(Duration::from_millis(2));
        }
        if overflow {
            for _ in 0..64 {
                f.enqueue(Event::Key {
                    key: "+".into(),
                    pressed: true,
                })
                .unwrap();
            }
            assert!(
                f.enqueue(Event::Key {
                    key: "+".into(),
                    pressed: true
                })
                .is_err()
            );
        } else {
            f.enqueue(Event::Focus(false)).unwrap();
        }
        thread::sleep(Duration::from_millis(400));
        f.pump();
        assert_eq!(
            p.seen
                .lock()
                .unwrap()
                .iter()
                .filter(|k| k.as_str() == "grant")
                .count(),
            1,
            "cancelled in-flight request must never retry"
        );
        assert!(
            f.state
                .as_ref()
                .is_none_or(|s| !s.status.contains("grant applied"))
        );
        drop(f);
    }
}
