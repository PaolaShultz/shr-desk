use serde_json::{Value, json};
use shr_desk::{
    provider::{Client, Freshness, decode},
    render,
};
const SHOW: &str = "11111111-1111-4111-8111-111111111111";
const INITIAL: &[u8] = include_bytes!("fixtures/gp02/v1/e03-initial-snapshot.json");
const FINAL: &[u8] = include_bytes!("fixtures/gp02/v1/e03-final-snapshot.json");
fn client() -> Client {
    Client::new(SHOW, 9).unwrap()
}
fn parsed() -> Value {
    serde_json::from_slice(INITIAL).unwrap()
}
fn bytes(v: &Value) -> Vec<u8> {
    serde_json::to_vec(v).unwrap()
}
fn split() -> (Value, Value) {
    let mut a = parsed();
    let mut b = a.clone();
    a["page_count"] = json!(2);
    b["page_count"] = json!(2);
    b["page"] = json!(1);
    let p = a["parameters"].as_array().unwrap().clone();
    a["parameters"] = json!(&p[..20]);
    b["parameters"] = json!(&p[20..]);
    (a, b)
}
#[test]
fn exact_corpus_snapshots_preserve_provider_metadata_and_nullable_actual() {
    for raw in [
        INITIAL,
        FINAL,
        include_bytes!("fixtures/gp02/v1/e03m-initial-snapshot.json"),
        include_bytes!("fixtures/gp02/v1/e03m-final-snapshot.json"),
        include_bytes!("fixtures/gp02/v1/e03r-initial-snapshot.json"),
        include_bytes!("fixtures/gp02/v1/e03r-final-snapshot.json"),
    ] {
        let mut c = client();
        assert!(c.ingest(raw, 0).unwrap());
        let s = c.snapshot().unwrap();
        assert_eq!(s.parameters.len(), 40);
        assert!(s.parameters.iter().all(|p| p.actual.is_none()));
        assert_eq!(c.freshness(250), Freshness::Fresh);
        assert_eq!(c.freshness(251), Freshness::Stale);
        assert!(render::provider_scene(&c, 0).in_bounds());
    }
    let mut c = client();
    c.ingest(FINAL, 0).unwrap();
    let p = &c.snapshot().unwrap().parameters[0];
    assert_eq!(p.target_value, json!(-3000));
    assert_eq!(p.hold, Some(json!(-3000)));
    assert_eq!(p.owner.as_deref(), Some("desk-a"));
    assert!(
        c.lines(0)
            .join("\n")
            .contains("actual=unavailable target=-3000")
    );
}
#[test]
fn strict_json_guards_precede_trusted_state() {
    let raw = String::from_utf8(INITIAL.to_vec()).unwrap();
    let duplicate = raw.replacen("\"page\": 0", "\"page\": 0, \"page\": 0", 1);
    let nested_duplicate = raw.replacen(
        "\"parameter\": \"fader\"",
        "\"parameter\": \"fader\", \"parameter\": \"fader\"",
        1,
    );
    for bad in [
        vec![0xff],
        vec![b' '; 65537],
        format!("{raw} {{}}").into_bytes(),
        duplicate.into_bytes(),
        nested_duplicate.into_bytes(),
        format!("{}0{}", "[".repeat(13), "]".repeat(13)).into_bytes(),
    ] {
        let mut c = client();
        assert!(c.ingest(&bad, 0).is_err());
        assert!(c.snapshot().is_none());
    }
    let mut v = parsed();
    v["unknown"] = json!(1);
    assert!(decode(&bytes(&v)).is_err());
    for key in ["actual", "proposal", "owner", "hold"] {
        let mut v = parsed();
        v["parameters"][0].as_object_mut().unwrap().remove(key);
        assert!(decode(&bytes(&v)).is_err(), "missing{key}");
    }
}
#[test]
fn identity_version_counter_and_capacity_refusals() {
    for (k, v) in [
        ("show_id", json!("22222222-2222-4222-8222-222222222222")),
        ("epoch", json!("10")),
        ("version", json!(2)),
        ("sequence", json!("01")),
        ("revision", json!(18446744073709551615u64)),
        ("session_history_capacity", json!(1025)),
        ("page_count", json!(17)),
        ("rendered_application", json!(true)),
        ("inputs", json!(["input-01"])),
        ("monitors", json!(["monitor-1", "monitor-1"])),
    ] {
        let mut c = client();
        let mut d = parsed();
        d[k] = v;
        assert!(c.ingest(&bytes(&d), 0).is_err(), "{k}");
        assert!(c.snapshot().is_none());
    }
}
#[test]
fn ranges_duplicate_targets_and_measured_zero_cannot_impersonate_gp02() {
    for (k, v) in [
        ("actual", json!(0)),
        ("target_value", json!(-3001)),
        ("target_value", json!(1.5)),
        ("target_value", json!(-60001)),
        ("owner", json!("Bad ID")),
        ("hold", json!(0)),
    ] {
        let mut d = parsed();
        d["parameters"][0][k] = v;
        assert!(decode(&bytes(&d)).is_err());
    }
    let mut d = parsed();
    d["parameters"][0]["target"]["monitor"] = Value::Null;
    assert!(decode(&bytes(&d)).is_err());
    let mut d = parsed();
    d["parameters"][1] = d["parameters"][0].clone();
    assert!(decode(&bytes(&d)).is_err());
    let mut d = parsed();
    d["parameters"].as_array_mut().unwrap().pop();
    assert!(client().ingest(&bytes(&d), 0).is_err());
    let mut d = parsed();
    d["modes"][0][1] = json!("auto");
    assert!(decode(&bytes(&d)).is_err());
}
#[test]
fn bounds_reject_null_monitor_and_accept_valid_proposal_separately() {
    let mut d = parsed();
    d["modes"][0][1] = json!("auto");
    d["automation_bounds"] =
        json!([{"target":{"parameter":"fader","input":"input-01"},"min":-6000,"max":0}]);
    d["parameters"][0]["proposal"] = json!(-3000);
    let s = decode(&bytes(&d)).unwrap();
    assert_eq!(s.parameters[0].proposal, Some(json!(-3000)));
    assert_eq!(s.parameters[0].target_value, json!(-6000));
    assert_eq!(s.parameters[0].actual, None);
    d["automation_bounds"][0]["target"]["monitor"] = Value::Null;
    assert!(decode(&bytes(&d)).is_err());
}
#[test]
fn pages_complete_out_of_order_atomically_with_deadline() {
    let (a, b) = split();
    let mut c = client();
    assert!(!c.ingest(&bytes(&b), 0).unwrap());
    assert!(c.snapshot().is_none());
    assert!(c.ingest(&bytes(&a), 2000).unwrap());
    assert_eq!(c.snapshot().unwrap().parameters.len(), 40);
    let mut c = client();
    c.ingest(&bytes(&a), 0).unwrap();
    assert!(c.ingest(&bytes(&b), 2001).is_err());
    assert!(c.snapshot().is_none());
    assert!(!c.is_collecting());
    let mut c = client();
    c.ingest(&bytes(&a), 100).unwrap();
    assert!(c.ingest(&bytes(&b), 99).is_err());
}
#[test]
fn mixed_duplicate_and_partial_pages_never_publish() {
    let (a, b) = split();
    for k in ["revision", "sequence", "epoch", "show_id"] {
        let mut c = client();
        c.ingest(&bytes(&a), 0).unwrap();
        let mut bad = b.clone();
        bad[k] = json!("10");
        assert!(c.ingest(&bytes(&bad), 1).is_err());
        assert!(c.snapshot().is_none());
        assert!(!c.is_collecting());
    }
    let mut c = client();
    c.ingest(&bytes(&a), 0).unwrap();
    assert!(c.ingest(&bytes(&a), 1).is_err());
    assert!(c.snapshot().is_none());
    let mut c = client();
    let mut b = b;
    b["parameters"][0] = a["parameters"][0].clone();
    c.ingest(&bytes(&a), 0).unwrap();
    assert!(c.ingest(&bytes(&b), 1).is_err());
}
#[test]
fn stable_selection_survives_reorder_old_sequence_cannot_rollback() {
    let mut c = client();
    c.ingest(INITIAL, 0).unwrap();
    c.select("input-06").unwrap();
    let mut d: Value = serde_json::from_slice(FINAL).unwrap();
    d["inputs"].as_array_mut().unwrap().reverse();
    d["parameters"].as_array_mut().unwrap().reverse();
    c.ingest(&bytes(&d), 1).unwrap();
    assert_eq!(c.selected(), Some("input-06"));
    assert!(c.ingest(INITIAL, 2).is_err());
    assert_eq!(c.snapshot().unwrap().revision, "13");
    assert_eq!(c.freshness(2), Freshness::Stale);
    c.disconnect();
    assert!(c.ingest(FINAL, 3).is_err());
    c.reconnect(10);
    assert!(c.ingest(FINAL, 4).is_err());
    d["epoch"] = json!("10");
    d["sequence"] = json!("1");
    c.ingest(&bytes(&d), 5).unwrap();
    assert_eq!(c.freshness(5), Freshness::Fresh);
}
#[test]
fn provider_reconnect_drops_local_pending_draft_and_pickup() {
    use shr_desk::{
        console::{Console, Envelope, Event, HeadlessRenderer},
        model::{Desk, Snapshot},
    };
    let mut c = Console::new(
        Desk::new(Snapshot::fixture()),
        HeadlessRenderer::default(),
        8,
    );
    c.assign(true);
    let send = |c: &mut Console, e| {
        c.enqueue(Envelope {
            generation: c.generation(),
            event: e,
        })
        .unwrap()
    };
    send(&mut c, Event::Text("hold".into()));
    send(&mut c, Event::Text("confirm".into()));
    assert_eq!(c.pump().commands.len(), 1);
    assert!(c.desk.pending().is_some());
    let mut p = client();
    p.ingest(INITIAL, 0).unwrap();
    c.reconnect_provider(&mut p, 9);
    assert!(c.desk.pending().is_none());
    assert!(c.desk.draft.is_none());
    assert!(!c.desk.connected);
    assert_eq!(p.freshness(1), Freshness::Stale);
    send(&mut c, Event::Midi(vec![0xb0, 16, 0]));
    assert!(c.pump().commands.is_empty());
    assert!(p.ingest(INITIAL, 2).is_err());
    p.ingest(FINAL, 2).unwrap();
    assert_eq!(p.freshness(2), Freshness::Fresh);
}
#[test]
fn cli_uses_provider_decoder_and_rejects_missing_partial_and_wrong_show() {
    use std::process::Command;
    let file = concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/tests/fixtures/gp02/v1/e03-final-snapshot.json"
    );
    let output = Command::new(env!("CARGO_BIN_EXE_shr-desk"))
        .args([
            "--provider",
            SHOW,
            "9",
            "--snapshot-file",
            file,
            "--select",
            "input-01",
        ])
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let text = String::from_utf8(output.stdout).unwrap();
    assert!(text.contains("READ ONLY"));
    assert!(text.contains("actual=unavailable target=-3000"));
    assert!(!text.contains("SIMULATION"));
    assert!(
        !Command::new(env!("CARGO_BIN_EXE_shr-desk"))
            .args(["--provider", SHOW, "9"])
            .status()
            .unwrap()
            .success()
    );
    assert!(
        !Command::new(env!("CARGO_BIN_EXE_shr-desk"))
            .args(["--provider", SHOW, "10", "--snapshot-file", file])
            .status()
            .unwrap()
            .success()
    );
}
