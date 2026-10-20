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
        .args([dir.to_str().unwrap(), &id.show, "1", "sequence"])
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
