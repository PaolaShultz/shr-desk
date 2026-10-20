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

#[test]
fn channel_panel_has_all_processing_targets_and_preserves_modal_precedence() {
    use shr_instrument_panel::{
        Material,
        terminal::ratatui::{buffer::Buffer, layout::Rect},
    };
    let (peer, config) = peer();
    let mut f = Frontend::new(config);
    wait(&mut f);
    f.page = shr_desk::model::Page::Channel;
    let v: Value =
        serde_json::from_str(include_str!("fixtures/gp07/v2/snapshot-reply.json")).unwrap();
    let mut p: shr_desk::processing::Snapshot =
        serde_json::from_value(v["snapshot"].clone()).unwrap();
    let u = f.state.as_mut().unwrap();
    p.revision = u.snapshot.as_ref().unwrap().authority.revision.clone();
    u.processing = Some(p);
    u.processing_age_ms = Some(0);
    u.received = Instant::now();
    assert!(f.channel_panel_visible());
    for material in [Material::Text, Material::Glyph] {
        let mut b = Buffer::empty(Rect::new(0, 0, 160, 45));
        let hits = shr_desk::channel_panel::draw(&f, &mut b, material);
        let fields: std::collections::BTreeSet<_> = hits
            .iter()
            .filter_map(|h| {
                if let shr_desk::channel_panel::Target::Field(i) = h.target {
                    Some(i)
                } else {
                    None
                }
            })
            .collect();
        assert_eq!(fields, (0..24).collect());
        assert!(
            hits.iter()
                .all(|h| h.area.right() <= 160 && h.area.bottom() <= 45)
        );
        let text: String = b.content.iter().map(|c| c.symbol()).collect();
        assert!(!text.contains("CONFIRMED TARGET"));
        for removed in ["SETTLED", "TARGET", "EDITED", "ENTRY"] {
            assert!(!text.contains(removed));
        }
        for y in 4..20 {
            assert_eq!(b[(145, y)].symbol(), "▰");
            assert_eq!(
                b[(145, y)].fg,
                shr_instrument_panel::terminal::ratatui::style::Color::Rgb(29, 39, 45)
            );
        }
        // Header is framed: an empty border, labels/LEDs, then an empty row.
        for y in [0, 2] {
            for x in 2..158 {
                assert!(
                    matches!(b[(x, y)].symbol(), " " | "▔" | "▁" | "─" | "┌" | "┐" | "│"),
                    "unexpected header content at {x},{y}: {}",
                    b[(x, y)].symbol()
                );
            }
        }
        // Expanded plot has labelled +/-18 dB endpoints, not a taller empty well.
        assert_eq!(b[(4, 4)].symbol(), "+");
        assert_eq!(b[(4, 19)].symbol(), "-");
        assert!(!text.contains("GATE"));
        assert!(!text.contains("PHANTOM"));
        for hint in [
            "F1 ", "F2 ", "F4 ", "F12 ", "PgUp", "U / I", "C exact", "D exact",
        ] {
            assert!(!text.contains(hint), "keyboard hint leaked: {hint}");
        }
        if material == Material::Glyph {
            let on = !f
                .state
                .as_ref()
                .unwrap()
                .processing
                .as_ref()
                .unwrap()
                .channels[0]
                .current
                .eq_bypass;
            assert_eq!(
                b[(60, 1)].symbol(),
                shr_instrument_panel::tile(&format!(
                    "lens_green_{}_0_0",
                    if on { "on" } else { "off" }
                ))
                .to_string()
            );
        }
        for (w, h) in [(159, 45), (160, 44), (0, 0)] {
            let mut small = Buffer::empty(Rect::new(0, 0, w, h));
            assert!(shr_desk::channel_panel::draw(&f, &mut small, material).is_empty());
        }
    }
    // Physical grouping is a presentation contract: eight columns, exactly
    // eight contextual pads, and the same confirmed state for every menu/layer.
    for secondary in [false, true] {
        for menu in [
            shr_desk::channel_panel::Menu::Channel,
            shr_desk::channel_panel::Menu::Band(0),
            shr_desk::channel_panel::Menu::Band(3),
            shr_desk::channel_panel::Menu::Compressor,
            shr_desk::channel_panel::Menu::Output,
        ] {
            let mut b = Buffer::empty(Rect::new(0, 0, 160, 45));
            let hits = shr_desk::channel_panel::draw_with_ui(
                &f,
                &mut b,
                Material::Glyph,
                &shr_desk::channel_panel::PanelUi { menu, secondary },
            );
            let pads: Vec<_> = hits.iter().filter(|h| h.area.y == 44).collect();
            assert_eq!(pads.len(), 8);
            for (i, pad) in pads.iter().enumerate() {
                assert_eq!(pad.area, Rect::new(1 + i as u16 * 20, 44, 18, 1));
            }
            for (column, upper, lower) in [
                (0, 1, 2),
                (1, 5, 6),
                (2, 9, 10),
                (3, 13, 14),
                (4, 18, 21),
                (5, 19, 22),
                (6, 20, 23),
            ] {
                for (y, index) in [(24, upper), (33, lower)] {
                    assert!(hits.iter().any(|h|h.area.contains((column*20+10,y).into())
                        &&matches!(h.target,shr_desk::channel_panel::Target::Field(i) if i==index)));
                }
            }
        }
    }
    // The EQ graph cannot paint into the compressor plate. Reduction is actual
    // owner telemetry, fills downward, and clears independently when stale.
    {
        let u = f.state.as_mut().unwrap();
        let c = &mut u.processing.as_mut().unwrap().channels[0];
        c.ready = true;
        c.gain_reduction_mdb = Some(12000);
    }
    let mut observed = Buffer::empty(Rect::new(0, 0, 160, 45));
    shr_desk::channel_panel::draw(&f, &mut observed, Material::Text);
    for y in 2..22 {
        assert_eq!(observed[(80, y)].symbol(), " ");
        assert_eq!(
            observed[(80, y)].bg,
            shr_instrument_panel::terminal::ratatui::style::Color::Rgb(19, 23, 27)
        );
    }
    assert_eq!(observed[(125, 6)].fg, shr_instrument_panel::rack::AMBER);
    assert_eq!(
        observed[(125, 18)].fg,
        shr_instrument_panel::terminal::ratatui::style::Color::Rgb(29, 39, 45)
    );
    let labels: String = observed.content.iter().map(|c| c.symbol()).collect();
    assert!(labels.contains("RAW PEAK"));
    assert!(labels.contains("RAW RMS"));
    f.state.as_mut().unwrap().processing_age_ms = Some(1000);
    shr_desk::channel_panel::draw(&f, &mut observed, Material::Text);
    assert_eq!(
        observed[(125, 6)].fg,
        shr_instrument_panel::terminal::ratatui::style::Color::Rgb(29, 39, 45)
    );
    assert_eq!(observed[(125, 20)].symbol(), "—");
    f.state.as_mut().unwrap().processing_age_ms = Some(0);
    let before = f
        .state
        .as_ref()
        .unwrap()
        .processing
        .as_ref()
        .unwrap()
        .channels[0]
        .target
        .clone();
    f.channel_panel_field(2).unwrap();
    assert_eq!(f.processing_field, 2);
    assert_eq!(f.processing_draft.as_ref().unwrap().config, before);
    assert_eq!(
        f.state
            .as_ref()
            .unwrap()
            .processing
            .as_ref()
            .unwrap()
            .channels[0]
            .target,
        before
    );
    // An unapplied bypass change must not change the settled-owner lamp.
    let mut before_buffer = Buffer::empty(Rect::new(0, 0, 160, 45));
    shr_desk::channel_panel::draw(&f, &mut before_buffer, Material::Glyph);
    f.processing_draft.as_mut().unwrap().config.eq_bypass = !before.eq_bypass;
    let mut draft_buffer = Buffer::empty(Rect::new(0, 0, 160, 45));
    shr_desk::channel_panel::draw(&f, &mut draft_buffer, Material::Glyph);
    for x in 60..63 {
        assert_eq!(before_buffer[(x, 1)], draft_buffer[(x, 1)]);
    }
    f.state.as_mut().unwrap().review = Some((77, "EXACT REVIEW".into()));
    assert!(!f.channel_panel_visible());
    assert!(f.channel_panel_field(1).is_err());
    f.state.as_mut().unwrap().review = None;
    f.meter_page = true;
    assert!(!f.channel_panel_visible());
    f.meter_page = false;
    f.sends_page = true;
    assert!(!f.channel_panel_visible());
    f.sends_page = false;
    f.state.as_mut().unwrap().received = Instant::now() - Duration::from_secs(1);
    assert!(f.channel_panel_field(1).is_err());
    drop(f);
    drop(peer);
}

/// Repeatable, opt-in renderer workload. Synthetic fixture changes are benchmark
/// inputs only; this does not qualify an owner, meter or controller. Final visual
/// evidence uses the separately connected production executable.
#[test]
#[ignore = "opt-in 100-frame Channel renderer/terminal benchmark; see docs/TERMINAL.md"]
fn channel_material_performance() {
    use shr_desk::channel_material::{Materials, prepare};
    use shr_instrument_panel::{
        Material,
        terminal::{
            Console,
            ratatui::{
                Terminal, TerminalOptions, Viewport,
                backend::CrosstermBackend,
                crossterm::event::{self, Event as Input, KeyCode, KeyEventKind},
                layout::Rect,
                style::Color,
            },
        },
    };
    use std::{cell::Cell, rc::Rc};
    #[derive(Clone)]
    struct Count {
        bytes: Rc<Cell<usize>>,
        tty: bool,
    }
    impl Write for Count {
        fn write(&mut self, b: &[u8]) -> std::io::Result<usize> {
            let n = if self.tty {
                std::io::stdout().write(b)?
            } else {
                b.len()
            };
            self.bytes.set(self.bytes.get() + n);
            Ok(n)
        }
        fn flush(&mut self) -> std::io::Result<()> {
            if self.tty {
                std::io::stdout().flush()
            } else {
                Ok(())
            }
        }
    }
    let (peer, config) = peer();
    let mut front = Frontend::new(config);
    wait(&mut front);
    front.page = shr_desk::model::Page::Channel;
    let fixture: Value =
        serde_json::from_str(include_str!("fixtures/gp07/v2/snapshot-reply.json")).unwrap();
    let mut processing: shr_desk::processing::Snapshot =
        serde_json::from_value(fixture["snapshot"].clone()).unwrap();
    processing.revision = front
        .state
        .as_ref()
        .unwrap()
        .snapshot
        .as_ref()
        .unwrap()
        .authority
        .revision
        .clone();
    processing.channels[0].target.compressor_bypass = false;
    processing.channels[0].current.compressor_bypass = false;
    processing.channels[0].gain_reduction_mdb = Some(0);
    front.state.as_mut().unwrap().processing = Some(processing);
    front.state.as_mut().unwrap().processing_age_ms = Some(0);
    front.channel_panel_field(19).unwrap();
    let timing_path = std::env::var("SHR_CHANNEL_TIMING").ok();
    let actual = timing_path.is_some();
    let plain = std::env::var_os("SHR_CHANNEL_PLAIN").is_some();
    let modes = if actual {
        vec![std::env::var("SHR_CHANNEL_WORKLOAD").unwrap()]
    } else {
        vec![
            "unchanged".into(),
            "controls".into(),
            "meters".into(),
            "spectrum".into(),
        ]
    };
    let lifecycle = if actual {
        Some(Console::open().unwrap())
    } else {
        None
    };
    if let (Some(path), Some(c)) = (&timing_path, &lifecycle) {
        fs::write(
            format!("{path}.size"),
            format!("{:?}", c.terminal.size().unwrap()),
        )
        .unwrap();
    }
    let mut timing = timing_path.as_ref().map(|p| fs::File::create(p).unwrap());
    for mode in modes {
        let count = Rc::new(Cell::new(0));
        let mut writer = Count {
            bytes: count.clone(),
            tty: actual,
        };
        let mut terminal = Terminal::with_options(
            CrosstermBackend::new(writer.clone()),
            TerminalOptions {
                viewport: Viewport::Fixed(Rect::new(0, 0, 160, 45)),
            },
        )
        .unwrap();
        let mut materials = Materials::default();
        let mut durations = vec![];
        let mut measured_bytes = 0;
        let mut startup_bytes = 0;
        let warmup = if actual { 0 } else { 34 };
        for frame in 0..warmup + 100 {
            let u = front.state.as_mut().unwrap();
            u.received = Instant::now();
            u.processing_age_ms = Some(0);
            if mode == "controls" {
                front.processing_draft.as_mut().unwrap().config.ratio_milli =
                    1000 + (frame % 17) * 1000;
            }
            if mode == "meters" {
                u.processing.as_mut().unwrap().channels[0].gain_reduction_mdb =
                    Some((frame as u32 % 25) * 1000);
            }
            let mut spectrum_cache = shr_desk::spectrum::Cache::default();
            if mode == "spectrum" {
                let id = shr_desk::metering::Identity {
                    show: "11111111-1111-4111-8111-111111111111".into(),
                    epoch: 1,
                    map: 1,
                    topology: "legacy8-2".into(),
                    inputs: 8,
                    monitors: 2,
                    rate: 48000,
                };
                let observation = serde_json::json!({"contract":"GP-SPECTRUM","version":1,"supported":true,"show_id":id.show,"epoch":1,"map":1,"query":1,"input":"input-01","sample_rate":48000,"tap":"raw-pre-processing","units":"millidBFS-peak-Hann-max","first_frame":0,"end_frame":4096,"acquisition_age_ms":0,"valid":true,"bins":(0..64).map(|i| -90000+((i*7919+frame*2711)%80000)).collect::<Vec<_>>()});
                spectrum_cache
                    .accept(
                        serde_json::from_value(observation).unwrap(),
                        &id,
                        "input-01",
                        1,
                        Duration::ZERO,
                    )
                    .unwrap();
            }
            let spectrum_view = spectrum_cache.view();
            if mode == "meters" {
                let mut meter: shr_desk::metering::Snapshot =
                    serde_json::from_str(include_str!("fixtures/gp-meter/v1/valid-sine.json"))
                        .unwrap();
                meter.taps[0].peak_millidbfs = Some(-60000 + (frame % 61) * 1000);
                meter.taps[0].rms_millidbfs = Some(-63000 + (frame % 61) * 1000);
                let id = shr_desk::metering::Identity {
                    show: meter.show_id.clone(),
                    epoch: meter.source_epoch.0,
                    map: meter.map_generation.0,
                    topology: meter.topology.clone(),
                    inputs: meter.inputs,
                    monitors: meter.monitors,
                    rate: meter.sample_rate,
                };
                front.meters = Default::default();
                front
                    .meters
                    .accept(meter, &id, 1, Duration::ZERO, Instant::now())
                    .unwrap();
            }
            let before = count.get();
            let mut stamp = libc::timespec {
                tv_sec: 0,
                tv_nsec: 0,
            };
            // SAFETY: writable local timespec, synchronous monotonic clock query.
            assert_eq!(
                unsafe { libc::clock_gettime(libc::CLOCK_MONOTONIC, &mut stamp) },
                0
            );
            let ns = stamp.tv_sec as u128 * 1_000_000_000 + stamp.tv_nsec as u128;
            let start = Instant::now();
            let mut placements = vec![];
            terminal
                .draw(|f| {
                    shr_desk::channel_panel::draw_observation(
                        &front,
                        f.buffer_mut(),
                        Material::Glyph,
                        &Default::default(),
                        spectrum_view.as_ref(),
                    );
                    if !plain {
                        placements = prepare(f.buffer_mut());
                    }
                    shr_instrument_panel::rack::text(
                        f.buffer_mut(),
                        2,
                        0,
                        100,
                        "SYNTHETIC RENDER BENCHMARK",
                        Color::White,
                    );
                    if actual {
                        for x in 0..160 {
                            f.buffer_mut()[(x, 44)].set_char(' ').set_bg(
                                match usize::from(x) * 8 / 160 {
                                    0 => Color::Rgb(255, 0, 255),
                                    bit if frame & (1 << (bit - 1)) != 0 => Color::White,
                                    _ => Color::Black,
                                },
                            );
                        }
                    }
                })
                .unwrap();
            if !plain {
                materials.present(placements, &mut writer).unwrap();
            }
            let elapsed = start.elapsed().as_nanos();
            if frame == 0 {
                startup_bytes = count.get() - before;
            }
            if frame >= warmup {
                durations.push(elapsed);
                measured_bytes += count.get() - before;
            }
            if let Some(timing) = &mut timing {
                writeln!(timing, "{frame} {ns}").unwrap();
                timing.flush().unwrap();
                let deadline = Instant::now() + Duration::from_secs(5);
                loop {
                    assert!(Instant::now() < deadline, "frame observer timeout");
                    if event::poll(Duration::from_millis(50)).unwrap()
                        && let Input::Key(k) = event::read().unwrap()
                        && k.kind == KeyEventKind::Press
                        && k.code == KeyCode::Char(' ')
                    {
                        break;
                    }
                }
            }
        }
        durations.sort();
        let report = format!(
            "CHANNEL {mode} {} median_us={} p95_us={} bytes_100={measured_bytes} first_frame_bytes={startup_bytes} material_bytes_total={}",
            if actual { "tty" } else { "sink" },
            durations[50] / 1000,
            durations[94] / 1000,
            materials.bytes
        );
        if let Some(path) = &timing_path {
            fs::write(format!("{path}.app"), &report).unwrap();
        } else {
            eprintln!("{report}");
        }
        materials.clear(&mut writer).unwrap();
    }
    drop(lifecycle);
    drop(front);
    drop(peer);
}
