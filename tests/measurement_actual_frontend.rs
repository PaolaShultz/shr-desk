//! Owned software PA/GP20 provider episode. Explicit opt-in; no devices/windows.
use serde_json::json;
use shr_desk::frontend::{Config, Event, Frontend};
use std::{
    fs,
    path::PathBuf,
    thread,
    time::{Duration, Instant},
};
fn wait(f: &mut Frontend, end: Instant, label: &str, ready: impl Fn(&Frontend) -> bool) {
    eprintln!("WAIT {label}");
    let end = end.min(Instant::now() + Duration::from_secs(8));
    loop {
        f.pump();
        if ready(f) {
            return;
        }
        assert!(
            Instant::now() < end,
            "{label}: {} / {} / {:?}",
            f.message,
            f.state
                .as_ref()
                .map(|u| u.status.as_str())
                .unwrap_or("no state"),
            f.state.as_ref().and_then(|u| u.last_operation.as_ref())
        );
        thread::sleep(Duration::from_millis(5));
    }
}
fn tap(f: &mut Frontend, k: &str) {
    for pressed in [true, false] {
        f.enqueue(Event::Key {
            key: k.into(),
            pressed,
        })
        .unwrap();
        f.pump();
    }
}
fn form(f: &mut Frontend, key: &str, text: &str) {
    tap(f, &key.to_lowercase());
    for c in text.chars() {
        tap(f, &c.to_string());
        thread::sleep(Duration::from_millis(20));
    }
    tap(f, "Enter");
}
fn confirm(f: &mut Frontend, end: Instant) {
    wait(f, end, "review", |f| {
        f.fresh() && f.state.as_ref().is_some_and(|s| s.review.is_some())
    });
    f.synchronize_review();
    for p in 0..f.review_pages() {
        if p > 0 {
            tap(f, "PageDown");
        }
        let scene = f.scene();
        assert!(scene.in_bounds());
        assert_eq!(shr_desk::raster::rgba(&scene).len(), 1920 * 1080 * 4);
        f.mark_presented();
    }
    tap(f, "Enter");
    wait(f, end, "confirmed completion", |f| {
        f.fresh()
            && f.state.as_ref().is_some_and(|s| {
                s.review.is_none()
                    && s.last_operation.as_ref().is_some_and(|s| {
                        s.contains("accepted at frame") || s.contains("applied revision")
                    })
            })
    });
}
fn probe(f: &mut Frontend, end: Instant) {
    let previous = f.state.as_ref().and_then(|s| s.measurement.receipt);
    tap(f, "Y");
    wait(f, end, "measurement probe", |f| {
        f.state
            .as_ref()
            .is_some_and(|s| s.measurement.receipt != previous && s.measurement.snapshot.is_some())
    });
}
fn result(f: &mut Frontend, end: Instant, id: &str, state: &str) {
    loop {
        form(f, "R", id);
        let wait_end = (Instant::now() + Duration::from_secs(2)).min(end);
        wait(f, wait_end, "result response", |f| {
            f.state.as_ref().is_some_and(|s| {
                s.measurement
                    .result
                    .as_ref()
                    .is_some_and(|r| r.summary.id == id)
            })
        });
        if f.state
            .as_ref()
            .unwrap()
            .measurement
            .result
            .as_ref()
            .unwrap()
            .summary
            .state
            == state
        {
            return;
        }
        assert!(
            Instant::now() < end,
            "result state {id}: {:?}",
            f.state
                .as_ref()
                .unwrap()
                .measurement
                .result
                .as_ref()
                .unwrap()
                .summary
        );
        thread::sleep(Duration::from_millis(100));
    }
}
#[test]
#[ignore = "requires explicitly launched hash-pinned measurement-provider; software-only"]
fn actual_keyboard_measurement_to_muted_owner_apply() {
    let root = PathBuf::from(std::env::var("GP20_EPISODE_DIR").expect("private witness directory"));
    let end = Instant::now() + Duration::from_secs(80);
    if let Ok(path) = std::env::var("GP20_RAW_DIAGNOSIS") {
        let bytes = fs::read(path).unwrap();
        let began = Instant::now();
        let raw = shr_desk::audio::decode_reply(&bytes).unwrap();
        eprintln!(
            "RAW DECODE {} us / faulted={}",
            began.elapsed().as_micros(),
            raw.snapshot.unwrap().faulted
        );
    }

    let mut f = Frontend::new(Config {
        wire_version: 2,
        remote: None,
        endpoint: root.join("audio.sock"),
        show: "11111111-1111-4111-8111-111111111111".into(),
        epoch: 1,
        writer: "desk-measurement-test".into(),
        scope: "pa_configuration".into(),
    });
    wait(&mut f, end, "attach", Frontend::fresh);
    assert!(f.state.as_ref().unwrap().measurement.snapshot.is_none());
    tap(&mut f, "G");
    wait(&mut f, end, "explicit PA grant", |f| {
        f.state.as_ref().is_some_and(|s| s.writer_granted())
    });
    probe(&mut f, end);
    let basis = f
        .state
        .as_ref()
        .unwrap()
        .measurement
        .snapshot
        .as_ref()
        .unwrap()
        .current_basis
        .clone()
        .unwrap();
    // A reviewed cancellation terminates its active owned capture; no replay.
    form(&mut f, "C", "aborted input-01 16 0 p1 65536 2048 100 10000");
    confirm(&mut f, end);
    form(&mut f, "X", "aborted");
    confirm(&mut f, end);
    result(&mut f, end, "aborted", "cancelled");
    let mut evidence = vec![];
    for (id, out, position) in [
        ("a1", 0, "p1"),
        ("b1", 1, "p1"),
        ("a2", 0, "p2"),
        ("b2", 1, "p2"),
    ] {
        form(
            &mut f,
            "C",
            &format!("{id} input-01 16 {out} {position} 32768 2048 100 10000"),
        );
        confirm(&mut f, end);
        result(&mut f, end, id, "measured");
        let record = f
            .state
            .as_ref()
            .unwrap()
            .measurement
            .result
            .clone()
            .unwrap();
        evidence.push(serde_json::to_value(record).unwrap());
    }
    form(&mut f, "P", "align a1 b1 a2 b2");
    confirm(&mut f, end);
    result(&mut f, end, "align", "proposed");
    let proposal = f
        .state
        .as_ref()
        .unwrap()
        .measurement
        .result
        .clone()
        .unwrap();
    assert_eq!(proposal.basis, basis);
    let candidate = proposal.candidate_configuration_json.clone().unwrap();
    // Escape consumes only the unsent whole-configuration review.
    form(&mut f, "A", "align");
    wait(&mut f, end, "muted review ready", |f| {
        f.state.as_ref().is_some_and(|s| s.review.is_some())
    });
    tap(&mut f, "Esc");
    wait(&mut f, end, "cancel review", |f| {
        f.state.as_ref().is_some_and(|s| s.review.is_none())
    });
    assert_eq!(
        f.state
            .as_ref()
            .unwrap()
            .structural
            .as_ref()
            .unwrap()
            .pa_configuration_json
            .as_ref(),
        Some(&basis.configuration_json)
    );
    form(&mut f, "A", "align");
    confirm(&mut f, end);
    wait(&mut f, end, "actual muted configuration readback", |f| {
        f.state
            .as_ref()
            .and_then(|s| s.structural.as_ref())
            .is_some_and(|s| {
                s.outputs_quiesced && s.pa_configuration_json.as_ref() == Some(&candidate)
            })
    });
    let current = f.state.as_ref().unwrap().structural.clone().unwrap();
    // The corrected graph produces a review-only no_change owner result.
    for (id, out) in [("n1", 0), ("n2", 1)] {
        form(
            &mut f,
            "C",
            &format!("{id} input-01 16 {out} verify 32768 2048 100 10000"),
        );
        confirm(&mut f, end);
        result(&mut f, end, id, "measured");
    }
    form(&mut f, "P", "unchanged n1 n2");
    confirm(&mut f, end);
    result(&mut f, end, "unchanged", "no_change");
    let no_change = f
        .state
        .as_ref()
        .unwrap()
        .measurement
        .result
        .clone()
        .unwrap();
    assert_eq!(
        serde_json::from_str::<serde_json::Value>(
            no_change.candidate_configuration_json.as_deref().unwrap()
        )
        .unwrap(),
        serde_json::from_str::<serde_json::Value>(&no_change.basis.configuration_json).unwrap()
    );
    assert_ne!(no_change.summary.state, "proposed");
    probe(&mut f, end);
    form(&mut f, "A", "align");
    wait(&mut f, end, "old proposal refused", |f| {
        f.state.as_ref().is_some_and(|s| {
            s.review.is_none()
                && s.last_operation
                    .as_ref()
                    .is_some_and(|x| x.contains("basis changed") || x.contains("did not propose"))
        })
    });
    fs::write(root.join("desk-acceptance.json"),serde_json::to_vec_pretty(&json!({"physical":false,"captures":evidence,"proposal":proposal,"readback":current,"rearm":false,"cancelled_review":true,"capture_cancelled":true,"no_change":no_change,"stale_proposal_refused":true})).unwrap()).unwrap();
    let scene = f.scene();
    assert!(scene.in_bounds());
    fs::write(
        root.join("desk-measurement.svg"),
        shr_desk::render::svg(&scene),
    )
    .unwrap();
}
