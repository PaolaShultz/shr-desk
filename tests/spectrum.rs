use shr_desk::{
    metering::Identity,
    spectrum::{Cache, Snapshot},
};
use std::time::Duration;
fn identity() -> Identity {
    Identity {
        show: "11111111-1111-4111-8111-111111111111".into(),
        epoch: 1,
        map: 1,
        topology: "legacy8-2".into(),
        inputs: 8,
        monitors: 2,
        rate: 48000,
    }
}
fn sample(end: u64, db: i32) -> Snapshot {
    Snapshot {
        contract: "GP-SPECTRUM".into(),
        version: 1,
        supported: true,
        show_id: identity().show,
        epoch: 1,
        map: 1,
        query: 1,
        input: "input-01".into(),
        sample_rate: 48000,
        tap: "raw-pre-processing".into(),
        units: "millidBFS-peak-Hann-max".into(),
        first_frame: end - 4096,
        end_frame: end,
        acquisition_age_ms: 0,
        valid: true,
        bins: vec![db; 64],
        fft_bins: None,
        sources: None,
    }
}
#[test]
fn peak_hold_decay_silence_and_stale_do_not_animate() {
    let mut c = Cache::default();
    let id = identity();
    c.accept(sample(4800, -6000), &id, "input-01", 1, Duration::ZERO)
        .unwrap();
    c.accept(sample(9600, -96000), &id, "input-01", 1, Duration::ZERO)
        .unwrap();
    assert_eq!(c.view().unwrap().peaks[0], -6000);
    c.accept(sample(33600, -96000), &id, "input-01", 1, Duration::ZERO)
        .unwrap();
    assert_eq!(c.view().unwrap().peaks[0], -8400);
    c.accept(sample(240000, -96000), &id, "input-01", 1, Duration::ZERO)
        .unwrap();
    assert_eq!(c.view().unwrap().peaks[0], -96000);
    c.accept(
        sample(244800, -6000),
        &id,
        "input-01",
        1,
        Duration::from_millis(351),
    )
    .unwrap();
    assert!(c.view().is_none());
    // A repeated source frame with a new query never extends freshness.
    c.accept(sample(244800, -6000), &id, "input-01", 1, Duration::ZERO)
        .unwrap();
    assert!(c.view().is_none());
}
#[test]
fn wrong_identity_units_bounds_and_missing_data_are_rejected() {
    for edit in 0..8 {
        let mut s = sample(4800, -6000);
        match edit {
            0 => s.input = "input-02".into(),
            1 => s.epoch = 2,
            2 => s.map = 2,
            3 => s.units = "dBSPL".into(),
            4 => s.bins.pop().map(|_| ()).unwrap(),
            5 => s.bins[0] = 24001,
            6 => s.end_frame = u64::MAX,
            _ => s.query = 2,
        }
        assert!(
            Cache::default()
                .accept(s, &identity(), "input-01", 1, Duration::ZERO)
                .is_err()
        );
    }
    let mut c = Cache::default();
    c.accept(
        sample(4800, -6000),
        &identity(),
        "input-01",
        1,
        Duration::ZERO,
    )
    .unwrap();
    let mut s = sample(9600, 0);
    s.valid = false;
    s.bins.clear();
    c.accept(s, &identity(), "input-01", 1, Duration::ZERO)
        .unwrap();
    assert!(c.view().is_none());
}

#[test]
#[ignore = "actual deterministic software provider; set GP_SPECTRUM_SOURCE to canonical example executable"]
fn actual_provider_spectrum_recovery() {
    use std::{os::unix::fs::PermissionsExt, process::Command, time::Instant};
    let executable =
        std::env::var("GP_SPECTRUM_SOURCE").expect("explicit software source executable");
    let dir = std::env::temp_dir().join(format!("desk-spectrum-{}", std::process::id()));
    std::fs::create_dir(&dir).unwrap();
    std::fs::set_permissions(&dir, std::fs::Permissions::from_mode(0o700)).unwrap();
    let mut id = identity();
    let mut child = Command::new(&executable)
        .args([
            dir.to_str().unwrap(),
            &id.show,
            "1",
            if std::env::var_os("GP_SPECTRUM_RENDER").is_some() {
                "rta-sequence"
            } else {
                "sequence"
            },
        ])
        .spawn()
        .unwrap();
    let deadline = Instant::now() + Duration::from_secs(3);
    while !dir.join("audio.sock").exists() {
        assert!(Instant::now() < deadline);
        std::thread::sleep(Duration::from_millis(5));
    }
    let mut config = shr_desk::frontend::Config {
        wire_version: 1,
        remote: None,
        endpoint: dir.join("audio.sock"),
        show: id.show.clone(),
        epoch: 1,
        writer: "spectrum-proof".into(),
        scope: "foh".into(),
    };
    let worker = shr_desk::spectrum::Worker::start(
        config.clone(),
        id.clone(),
        "input-01".into(),
        "proof".into(),
    );
    let began = Instant::now();
    let mut high = false;
    let mut low = false;
    let mut silence = false;
    let mut held = false;
    let mut decayed = false;
    let mut frames = std::collections::BTreeSet::new();
    while began.elapsed() < Duration::from_secs(6) {
        if let Some(v) = worker.view() {
            frames.insert(v.end);
            let peak = *v.bins.iter().max().unwrap();
            let hold = *v.peaks.iter().max().unwrap();
            high |= (peak + 6021).abs() < 5;
            low |= (peak + 18062).abs() < 5;
            silence |= peak == -96000;
            held |= (peak + 18062).abs() < 5 && (hold + 6021).abs() < 5;
            decayed |= peak == -96000 && hold < -18062;
        }
        std::thread::sleep(Duration::from_millis(20));
    }
    assert!(
        high && low && silence && held && decayed,
        "high {high} low {low} silence {silence} held {held} decay {decayed}"
    );
    assert!(
        (45..=65).contains(&frames.len()),
        "updates {}",
        frames.len()
    );
    if let Ok(path) = std::env::var("GP_SPECTRUM_RENDER") {
        use shr_desk::{
            frontend::{Event, Frontend},
            model::Page,
        };
        use shr_instrument_panel::{
            Material,
            terminal::ratatui::{buffer::Buffer, layout::Rect, style::Color},
        };
        let mut front = Frontend::new(config.clone());
        front.enable_processing().unwrap();
        front.page = Page::Channel;
        let deadline = Instant::now() + Duration::from_secs(3);
        while !front.channel_panel_visible()
            || front
                .spectrum
                .as_ref()
                .and_then(|w| w.view())
                .is_none_or(|v| v.end < 310000)
        {
            front.pump();
            assert!(
                Instant::now() < deadline,
                "actual frontend/processing/spectrum not ready: {}",
                front.message
            );
            std::thread::sleep(Duration::from_millis(10));
        }
        front
            .enqueue(Event::Key {
                key: "F11".into(),
                pressed: true,
            })
            .unwrap();
        front.pump();
        assert!(front.eq_fullscreen);
        use shr_desk::actions::Action;
        for action in [
            Action::ProcessingEdit,
            Action::ProcessingText("0".into()),
            Action::ProcessingField(2),
            Action::ProcessingText("4".into()),
            Action::ProcessingField(4),
            Action::ProcessingText("-6".into()),
            Action::ProcessingField(4),
            Action::ProcessingText("3".into()),
            Action::ProcessingField(1),
            Action::ProcessingText("2.4".into()),
        ] {
            front.inject_controller(action).unwrap();
            front.pump();
        }
        let draft = front
            .processing_draft
            .as_ref()
            .expect("existing semantic editor opens in fullscreen")
            .config
            .clone();
        assert_eq!(draft.band2_gain_mdb, -6000);
        assert_eq!(draft.band3_q_milli, 2400);
        assert!(!draft.eq_bypass);
        assert!(
            front
                .state
                .as_ref()
                .unwrap()
                .processing
                .as_ref()
                .unwrap()
                .channels[0]
                .target
                .eq_bypass,
            "detached editing must not apply"
        );
        let mut buffer = Buffer::empty(Rect::new(0, 0, 160, 45));
        shr_desk::channel_panel::draw(&front, &mut buffer, Material::Text);
        let filled = buffer.content.iter().filter(|c| c.symbol() == "█").count();
        assert!(
            filled > 158 * 32 / 3,
            "actual broadband RTA must be clearly visible: {filled} cells"
        );
        let rgb = |c: Color| match c {
            Color::Rgb(r, g, b) => [r, g, b],
            _ => [180, 190, 200],
        };
        let cells: Vec<_> = buffer
            .content
            .iter()
            .map(|c| serde_json::json!({"text":c.symbol(),"fg":rgb(c.fg),"bg":rgb(c.bg)}))
            .collect();
        std::fs::write(
            path,
            serde_json::to_vec(&serde_json::json!({"width":160,"height":45,"cells":cells}))
                .unwrap(),
        )
        .unwrap();
        let eq_target = front.selected;
        front
            .enqueue(Event::Key {
                key: "]".into(),
                pressed: true,
            })
            .unwrap();
        front.pump();
        assert_eq!(front.selected, eq_target);
        assert_eq!(front.processing_draft.as_ref().unwrap().config, draft);
        assert!(
            front.spectrum.as_ref().and_then(|w| w.view()).is_none(),
            "source change clears previous observation immediately"
        );
        front.inject_controller(Action::Cancel).unwrap();
        front.pump();
        assert!(front.processing_draft.is_none());
    }
    child.kill().unwrap();
    child.wait().unwrap();
    std::thread::sleep(Duration::from_millis(450));
    assert!(worker.view().is_none());
    drop(worker);
    // New provider epoch requires a new observation identity, just as Frontend reconnect does.
    std::fs::remove_file(dir.join("audio.sock")).unwrap();
    child = Command::new(&executable)
        .args([dir.to_str().unwrap(), &id.show, "2", "tones"])
        .spawn()
        .unwrap();
    let deadline = Instant::now() + Duration::from_secs(3);
    while !dir.join("audio.sock").exists() {
        assert!(Instant::now() < deadline);
        std::thread::sleep(Duration::from_millis(5));
    }
    id.epoch = 2;
    config.epoch = 2;
    let worker =
        shr_desk::spectrum::Worker::start(config, id, "input-01".into(), "reconnect".into());
    let deadline = Instant::now() + Duration::from_secs(2);
    while worker.view().is_none() {
        assert!(Instant::now() < deadline);
        std::thread::sleep(Duration::from_millis(10));
    }
    assert!((worker.view().unwrap().bins.iter().max().unwrap() + 6021).abs() < 5);
    child.kill().unwrap();
    child.wait().unwrap();
    drop(worker);
    std::thread::sleep(Duration::from_millis(120));
    std::fs::remove_dir_all(dir).unwrap();
    eprintln!(
        "real analysis/provider/Desk: {} unique windows in 6 seconds; tones, level changes, silence, peak hold/decay, loss and new-epoch reconnect passed",
        frames.len()
    );
}

#[test]
fn native_bin_boundaries_preserve_low_detail_and_combine_high_bins() {
    use shr_desk::spectrum::aggregate;
    let mut bins = vec![-96000; 2049];
    bins[3] = -6000;
    bins[1300] = -12000;
    let columns = aggregate(&bins, 48000, 158);
    let low: Vec<_> = columns
        .iter()
        .enumerate()
        .filter(|(_, v)| **v == -6000)
        .map(|(i, _)| i)
        .collect();
    assert!(low.len() > 3);
    for (x, value) in columns.iter().enumerate() {
        let lo = 20. * 1000f64.powf(x as f64 / 158.);
        let hi = 20. * 1000f64.powf((x + 1) as f64 / 158.);
        let expected = bins
            .iter()
            .enumerate()
            .skip(1)
            .filter(|(k, _)| {
                (*k as f64 + 0.5) * 48000. / 4096. > lo && (*k as f64 - 0.5) * 48000. / 4096. < hi
            })
            .map(|(_, v)| *v)
            .max()
            .unwrap_or(-96000);
        assert_eq!(*value, expected);
    }
    assert!(columns.contains(&-12000));
    assert_eq!(
        shr_desk::fullscreen_eq::graph_area(
            shr_instrument_panel::terminal::ratatui::layout::Rect::new(0, 0, 160, 45)
        )
        .width,
        158
    );
}

#[test]
fn v2_source_change_clears_peaks_even_when_frames_go_backwards() {
    let mut c = Cache::default();
    let make = |source: &str, end, level| {
        let mut s = sample(end, level);
        s.version = 2;
        s.input = source.into();
        s.fft_bins = Some(vec![level; 2049]);
        s.sources = Some(vec![shr_desk::spectrum::Source {
            id: source.into(),
            label: source.into(),
            tap: s.tap.clone(),
        }]);
        s
    };
    c.accept(
        make("input-01", 9600, -6000),
        &identity(),
        "input-01",
        1,
        Duration::ZERO,
    )
    .unwrap();
    c.accept(
        make("input-02", 4800, -90000),
        &identity(),
        "input-02",
        1,
        Duration::ZERO,
    )
    .unwrap();
    assert!(
        c.view()
            .unwrap()
            .fft_peaks
            .unwrap()
            .iter()
            .all(|v| *v == -90000)
    );
    // The output namespace must leave room for the topology's full 128-byte ID.
    let output = format!("output:{}", "z".repeat(128));
    let mut s = make(&output, 9600, -12000);
    s.tap = "physical-output-post-patch".into();
    s.sources.as_mut().unwrap()[0].tap = s.tap.clone();
    c.accept(s, &identity(), &output, 1, Duration::ZERO)
        .unwrap();
    assert_eq!(c.view().unwrap().source, output);
}

#[test]
#[ignore = "actual mutually authenticated software provider; invoked by GigPies actual_remote_spectrum_desk"]
fn actual_remote_provider_spectrum() {
    use shr_desk::spectrum::Worker;
    use std::time::Instant;
    let remote: shr_desk::remote::Config = serde_json::from_slice(
        &std::fs::read(std::env::var("GP_SPECTRUM_REMOTE_CONFIG").unwrap()).unwrap(),
    )
    .unwrap();
    let mut remote_id = identity();
    remote_id.inputs = 16;
    remote_id.topology = "umc1820-ada8200-analog-48k-reference".into();
    let config = shr_desk::frontend::Config {
        wire_version: 2,
        remote: Some(remote),
        endpoint: "/unused".into(),
        show: identity().show,
        epoch: 1,
        writer: "spectrum-proof".into(),
        scope: "foh".into(),
    };
    for (source, tap) in [
        ("input-01", "raw-pre-processing"),
        ("main:left", "main-pre-pa"),
        ("output:analog-out-01", "physical-output-post-patch"),
    ] {
        let worker = Worker::start(
            config.clone(),
            remote_id.clone(),
            source.into(),
            source.into(),
        );
        let deadline = Instant::now() + Duration::from_secs(3);
        let view = loop {
            if let Some(v) = worker.view() {
                break v;
            }
            assert!(
                Instant::now() < deadline,
                "no real remote spectrum for {source}; choices {:?}",
                worker.sources()
            );
            std::thread::sleep(Duration::from_millis(10));
        };
        assert_eq!(view.tap, tap);
        assert_eq!(view.source, source);
        assert_eq!(view.fft_bins.as_ref().unwrap().len(), 2049);
        assert_eq!(worker.sources().len(), remote_id.inputs + 2 + 18);
        let peak = *view.fft_bins.as_ref().unwrap().iter().max().unwrap();
        let expected = if source.starts_with("input") {
            -6021
        } else {
            -15031
        };
        assert!((peak - expected).abs() < 10, "{source}: {peak}");
        let began = Instant::now();
        let mut frames = std::collections::BTreeSet::new();
        while began.elapsed() < Duration::from_secs(1) {
            if let Some(v) = worker.view() {
                frames.insert(v.end);
            }
            std::thread::sleep(Duration::from_millis(10));
        }
        assert!(
            (8..=12).contains(&frames.len()),
            "{source}: {} windows",
            frames.len()
        );
        eprintln!(
            "actual TLS provider -> Desk: {source} / {tap}, peak {peak} millidBFS, {} windows/s",
            frames.len()
        );
        drop(worker);
        std::thread::sleep(Duration::from_millis(120));
    }
    // Exercise the normal connected frontend too, rather than just its observer.
    use shr_desk::frontend::{Event, Frontend};
    use shr_instrument_panel::{
        Material,
        terminal::ratatui::{buffer::Buffer, layout::Rect},
    };
    let mut front = Frontend::new(config);
    front.enable_processing().unwrap();
    front.page = shr_desk::model::Page::Channel;
    let deadline = Instant::now() + Duration::from_secs(3);
    while !front.channel_panel_visible()
        || front.analysis_sources.len() != 36
        || front.spectrum.as_ref().and_then(|w| w.view()).is_none()
    {
        front.pump();
        assert!(
            Instant::now() < deadline,
            "remote frontend not ready: {}",
            front.message
        );
        std::thread::sleep(Duration::from_millis(10));
    }
    assert!(!front.state.as_ref().unwrap().writer_granted());
    front
        .enqueue(Event::Key {
            key: "F11".into(),
            pressed: true,
        })
        .unwrap();
    front.pump();
    assert!(front.eq_fullscreen);
    let mut buffer = Buffer::empty(Rect::new(0, 0, 160, 45));
    shr_desk::channel_panel::draw(&front, &mut buffer, Material::Text);
    let text = buffer
        .content
        .iter()
        .map(|c| c.symbol())
        .collect::<String>();
    assert!(text.contains("EQ input-01"));
    assert!(text.contains("RTA input-01/raw"));
    front
        .enqueue(Event::Key {
            key: "]".into(),
            pressed: true,
        })
        .unwrap();
    front.pump();
    assert!(front.spectrum.as_ref().and_then(|w| w.view()).is_none());
    let deadline = Instant::now() + Duration::from_secs(3);
    let view = loop {
        front.pump();
        if let Some(view) = front.spectrum.as_ref().and_then(|w| w.view()) {
            break view;
        }
        assert!(
            Instant::now() < deadline,
            "remote source change unavailable"
        );
        std::thread::sleep(Duration::from_millis(10));
    };
    assert_eq!(front.selected, 0);
    assert_eq!(view.source, "input-02");
    assert!(view.fft_peaks.unwrap().iter().all(|v| *v == -96000));
    eprintln!(
        "actual remote Frontend: read-only attach, GP07, fullscreen rendering and independent source change passed"
    );
}

#[test]
fn v2_probe_falls_back_on_separate_connection_without_inventing_detail() {
    use std::{
        io::{Read, Write},
        os::unix::{fs::PermissionsExt, net::UnixListener},
        time::Instant,
    };
    let dir = std::env::temp_dir().join(format!("desk-spectrum-v1-{}", std::process::id()));
    std::fs::create_dir(&dir).unwrap();
    std::fs::set_permissions(&dir, std::fs::Permissions::from_mode(0o700)).unwrap();
    let endpoint = dir.join("audio.sock");
    let listener = UnixListener::bind(&endpoint).unwrap();
    std::fs::set_permissions(&endpoint, std::fs::Permissions::from_mode(0o600)).unwrap();
    let server = std::thread::spawn(move || {
        for version in [2, 1] {
            let (mut socket, _) = listener.accept().unwrap();
            socket
                .set_read_timeout(Some(Duration::from_secs(2)))
                .unwrap();
            for frame in 1..=3 {
                let mut head = [0; 4];
                if socket.read_exact(&mut head).is_err() {
                    break;
                }
                let mut bytes = vec![0; u32::from_be_bytes(head) as usize];
                socket.read_exact(&mut bytes).unwrap();
                let request: serde_json::Value = serde_json::from_slice(&bytes).unwrap();
                assert_eq!(request["version"], version);
                if version == 2 {
                    break;
                } // Historical provider closes an unknown extension version.
                let reply = serde_json::json!({"contract":"GP-SPECTRUM","version":1,"supported":true,"show_id":identity().show,"epoch":1,"map":1,"query":request["query"],"input":"input-01","sample_rate":48000,"tap":"raw-pre-processing","units":"millidBFS-peak-Hann-max","first_frame":frame*4800-4096,"end_frame":frame*4800,"acquisition_age_ms":0,"valid":true,"bins":vec![-18000;64]});
                let bytes = serde_json::to_vec(&reply).unwrap();
                socket
                    .write_all(&(bytes.len() as u32).to_be_bytes())
                    .unwrap();
                socket.write_all(&bytes).unwrap();
            }
        }
    });
    let config = shr_desk::frontend::Config {
        wire_version: 1,
        remote: None,
        endpoint,
        show: identity().show,
        epoch: 1,
        writer: "test".into(),
        scope: "foh".into(),
    };
    let worker =
        shr_desk::spectrum::Worker::start(config, identity(), "input-01".into(), "legacy".into());
    let deadline = Instant::now() + Duration::from_secs(2);
    let view = loop {
        if let Some(view) = worker.view() {
            break view;
        }
        assert!(Instant::now() < deadline);
        std::thread::sleep(Duration::from_millis(5));
    };
    assert_eq!(view.bins.len(), 64);
    assert!(view.fft_bins.is_none());
    drop(worker);
    server.join().unwrap();
    std::fs::remove_dir_all(dir).unwrap();
}
