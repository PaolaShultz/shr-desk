//! Accepted producer bytes plus adversarial consumer validation; no DSP claims.
use serde_json::{Value, json};
use shr_desk::{audio, processing};
fn fixture(name: &str) -> Vec<u8> {
    std::fs::read(format!("tests/fixtures/gp07/v1/{name}.json")).unwrap()
}
fn reply(name: &str) -> processing::Reply {
    processing::decode_reply(&fixture(name)).unwrap()
}
#[test]
fn exact_producer_replies_and_request_envelopes() {
    for name in [
        "snapshot-reply",
        "set-pending",
        "set-final",
        "ready-reply",
        "backpressure",
        "stale-revision",
    ] {
        reply(name);
    }
    let s =
        audio::Session::new("11111111-1111-4111-8111-111111111111", 9, "desk-1", "foh").unwrap();
    assert_eq!(
        serde_json::from_slice::<Value>(&s.processing_request().encode().unwrap()).unwrap(),
        serde_json::from_slice::<Value>(&fixture("snapshot-request")).unwrap()
    );
    let v: Value = serde_json::from_slice(&fixture("set-request")).unwrap();
    let r = audio::Request {
        context: reply("set-pending").context,
        kind: "processing_set".into(),
        body: v["body"].clone(),
    };
    assert_eq!(
        serde_json::from_slice::<Value>(&r.encode().unwrap()).unwrap(),
        v
    );
    let settling = reply("set-final").snapshot.unwrap();
    assert!(!settling.channels[0].ready);
    assert_ne!(settling.channels[0].current, settling.channels[0].target);
    assert_eq!(
        reply("ready-reply").snapshot.unwrap().channels[0].gain_reduction_mdb,
        Some(1800)
    );
}
#[test]
fn strict_domains_and_required_nullable_fields() {
    let original: Value = serde_json::from_slice(&fixture("ready-reply")).unwrap();
    let edits = [
        ("/version", json!(2)),
        ("/revision", json!("01")),
        ("/context/epoch", json!(9)),
        ("/context/extra", json!(null)),
        ("/snapshot/channels/0/current/low_hz", json!(20.0)),
        ("/snapshot/channels/0/current/mid_gain_mdb", json!(101)),
        ("/snapshot/channels/0/current/mid_q_milli", json!(0)),
        ("/snapshot/channels/0/current/eq_bypass", json!(1)),
        ("/snapshot/channels/0/gain_reduction_mdb", json!(240001)),
        ("/snapshot/channels/0/ready", json!(false)),
        (
            "/snapshot/channels/0/transition_remaining_frames",
            json!(241),
        ),
        ("/snapshot/channels/0/input", json!("input-02")),
        ("/snapshot/foh_tap", json!("raw")),
        ("/snapshot/faulted", json!(true)),
    ];
    for (path, value) in edits {
        let mut v = original.clone();
        if let Some(slot) = v.pointer_mut(path) {
            *slot = value;
        } else {
            v["context"]["extra"] = value;
        }
        assert!(
            processing::decode_reply(&serde_json::to_vec(&v).unwrap()).is_err(),
            "{path}"
        );
    }
    for key in ["reason", "ticket", "snapshot"] {
        let mut v = original.clone();
        v.as_object_mut().unwrap().remove(key);
        assert!(processing::decode_reply(&serde_json::to_vec(&v).unwrap()).is_err());
    }
    let mut v = original.clone();
    v["snapshot"]["channels"][0]
        .as_object_mut()
        .unwrap()
        .remove("gain_reduction_mdb");
    assert!(processing::decode_reply(&serde_json::to_vec(&v).unwrap()).is_err());
    let duplicate = String::from_utf8(fixture("snapshot-reply"))
        .unwrap()
        .replacen("\"version\":1", "\"version\":1,\"version\":1", 1);
    assert!(processing::decode_reply(duplicate.as_bytes()).is_err());
    let mut trailing = fixture("snapshot-reply");
    trailing.extend_from_slice(b" null");
    assert!(processing::decode_reply(&trailing).is_err());
    assert!(processing::decode_reply(&vec![b' '; 65537]).is_err());
}
#[test]
fn numeric_operator_units_validate_atomically() {
    let mut c = reply("snapshot-reply").snapshot.unwrap().channels[0]
        .current
        .clone();
    for (field, text) in [
        (processing::Field::LowGain, "6.1"),
        (processing::Field::MidQ, "2.4"),
        (processing::Field::Attack, "0.1"),
        (processing::Field::CompressorBypass, "0"),
    ] {
        c.set_text(field, text).unwrap();
    }
    assert_eq!(c.low_gain_mdb, 6100);
    assert_eq!(c.mid_q_milli, 2400);
    assert_eq!(c.attack_us, 100);
    assert!(!c.compressor_bypass);
    let before = c.clone();
    for (field, text) in [
        (processing::Field::LowHz, "100.1"),
        (processing::Field::LowGain, "12.1"),
        (processing::Field::Ratio, "0"),
        (processing::Field::EqBypass, "2"),
        (processing::Field::Attack, "0.01"),
    ] {
        assert!(c.set_text(field, text).is_err());
        assert_eq!(c, before);
    }
}
fn session() -> audio::Session {
    let corpus: Value =
        serde_json::from_str(include_str!("fixtures/gp03/v1/e03-rendered.json")).unwrap();
    let mut s = audio::Session::new(
        "11111111-1111-4111-8111-111111111111",
        9,
        "desk-corpus",
        "foh",
    )
    .unwrap();
    s.ingest_snapshot(
        audio::decode_snapshot(&serde_json::to_vec(&corpus["initial"]).unwrap()).unwrap(),
        0,
    )
    .unwrap();
    s.begin("grant", json!({"scope":"foh"}), 0).unwrap();
    s.accept(
        audio::decode_reply(&serde_json::to_vec(&corpus["grant_response"]).unwrap()).unwrap(),
        1,
    )
    .unwrap();
    s.snapshot.as_mut().unwrap().frame = "0".into();
    s.input_released();
    let mut p = reply("snapshot-reply").snapshot.unwrap();
    p.revision = "12".into();
    s.ingest_processing(p, 2).unwrap();
    s
}
#[test]
fn shared_ids_lease_pending_correlation_and_late_observation_fences() {
    let mut s = session();
    let body = serde_json::from_slice::<Value>(&fixture("set-request")).unwrap()["body"].clone();
    let request = s.begin("processing_set", body.clone(), 3).unwrap();
    assert_eq!(request.context.request_id.as_deref(), Some("2"));
    assert!(s.begin("renew", json!({}), 4).is_err());
    let mut pending = reply("set-pending");
    pending.context = request.context.clone();
    pending.revision = "12".into();
    s.accept_processing(pending, 5).unwrap();
    let mut wrong = reply("set-final");
    wrong.context = request.context.clone();
    wrong.ticket = Some("2".into());
    wrong.revision = "13".into();
    wrong.snapshot.as_mut().unwrap().revision = "13".into();
    assert!(s.accept_processing(wrong, 6).is_err());
    assert!(s.pending.is_some());
    let mut final_reply = reply("set-final");
    final_reply.context = request.context;
    final_reply.revision = "13".into();
    final_reply.snapshot.as_mut().unwrap().revision = "13".into();
    s.accept_processing(final_reply.clone(), 7).unwrap();
    assert!(s.pending.is_none());
    assert!(!s.processing_fresh(7));
    assert!(s.accept_processing(final_reply, 8).is_err());
    assert!(
        !s.ingest_processing(reply("snapshot-reply").snapshot.unwrap(), 100)
            .unwrap()
    );
    assert_eq!(s.processing_age(100), Some(93));
    s.disconnect();
    assert!(s.begin("processing_set", body, 101).is_err());
    assert!(s.retry(101).is_none());
}
#[test]
fn processing_pressure_has_no_admission_and_does_not_burn_request_id() {
    let mut s = session();
    let body = serde_json::from_slice::<Value>(&fixture("set-request")).unwrap()["body"].clone();
    let request = s.begin("processing_set", body.clone(), 3).unwrap();
    let mut pressure = reply("backpressure");
    pressure.context = request.context.clone();
    s.accept_processing(pressure, 4).unwrap();
    assert!(s.pending.is_none());
    assert!(s.retry(200).is_none());
    let next = s.begin("processing_set", body, 5).unwrap();
    assert_eq!(next.context.request_id, request.context.request_id);
}

#[test]
fn stale_duplicate_faulted_and_transition_observations_do_not_enable_edits() {
    let mut s = session();
    assert!(s.processing_fresh(3));
    let same = s.processing.clone().unwrap();
    assert!(!s.ingest_processing(same.clone(), 200).unwrap());
    assert!(!s.processing_fresh(253));
    let mut fault = same.clone();
    fault.sequence = "3".into();
    fault.faulted = true;
    s.ingest_processing(fault, 5).unwrap();
    assert!(!s.processing_fresh(5));
    let mut transition = reply("set-final").snapshot.unwrap();
    transition.revision = "12".into();
    transition.sequence = "4".into();
    s.ingest_processing(transition, 6).unwrap();
    assert!(!s.processing_fresh(6));
    let body = serde_json::from_slice::<Value>(&fixture("set-request")).unwrap()["body"].clone();
    assert!(s.begin("processing_set", body, 7).is_err());
    assert!(s.pending.is_none());
}
#[test]
fn wrong_applied_config_and_cross_contract_reply_preserve_pending() {
    let mut s = session();
    let body = serde_json::from_slice::<Value>(&fixture("set-request")).unwrap()["body"].clone();
    let request = s.begin("processing_set", body, 3).unwrap();
    let mut r = reply("set-final");
    r.context = request.context.clone();
    r.revision = "13".into();
    r.snapshot.as_mut().unwrap().revision = "13".into();
    r.snapshot.as_mut().unwrap().channels[0].target.mid_gain_mdb = 5000;
    assert!(s.accept_processing(r, 4).is_err());
    assert!(s.pending.is_some());
    let corpus: Value =
        serde_json::from_str(include_str!("fixtures/gp03/v1/e03-rendered.json")).unwrap();
    let mut raw =
        audio::decode_reply(&serde_json::to_vec(&corpus["grant_response"]).unwrap()).unwrap();
    raw.context = request.context.clone();
    raw.outcome.as_mut().unwrap().context = request.context;
    assert!(s.accept(raw, 4).is_err());
    assert!(s.pending.is_some());
}

#[test]
fn exact_nonzero_identity_reason_grammar_and_consumed_boundary() {
    let original: Value = serde_json::from_slice(&fixture("set-final")).unwrap();
    for path in ["/context/epoch", "/snapshot/epoch", "/snapshot/sequence"] {
        let mut v = original.clone();
        *v.pointer_mut(path).unwrap() = json!("0");
        assert!(
            processing::decode_reply(&serde_json::to_vec(&v).unwrap()).is_err(),
            "{path}"
        );
    }
    let mut zero = original.clone();
    zero["context"]["epoch"] = json!("0");
    zero["snapshot"]["epoch"] = json!("0");
    assert!(processing::decode_reply(&serde_json::to_vec(&zero).unwrap()).is_err());
    let mut v = original;
    v["snapshot"]["frame"] = v["effective_frame"].clone();
    assert!(processing::decode_reply(&serde_json::to_vec(&v).unwrap()).is_err());
    for reason in ["bad-thing", "bad1", "UPPER", "", "bad.thing"] {
        let mut v: Value = serde_json::from_slice(&fixture("stale-revision")).unwrap();
        v["reason"] = json!(reason);
        assert!(
            processing::decode_reply(&serde_json::to_vec(&v).unwrap()).is_err(),
            "{reason}"
        );
    }
}
