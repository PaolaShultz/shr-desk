use serde_json::{Value, json};
use shr_desk::audio::{self, PendingState, Session};
fn corpus() -> Value {
    serde_json::from_str(include_str!("fixtures/gp03/v1/e03-rendered.json")).unwrap()
}
fn bytes(v: &Value) -> Vec<u8> {
    serde_json::to_vec(v).unwrap()
}
fn session() -> Session {
    let c = corpus();
    let mut s = Session::new(
        "11111111-1111-4111-8111-111111111111",
        9,
        "desk-corpus",
        "foh",
    )
    .unwrap();
    s.ingest_snapshot(audio::decode_snapshot(&bytes(&c["initial"])).unwrap(), 0)
        .unwrap();
    s
}
#[test]
fn accepted_wire_and_snapshot_decode() {
    let c = corpus();
    for k in ["grant_response", "pending"] {
        audio::decode_reply(&bytes(&c[k])).unwrap();
    }
    for r in c["committed"].as_array().unwrap() {
        audio::decode_reply(&bytes(r)).unwrap();
    }
    audio::decode_snapshot(&bytes(&c["final"])).unwrap();
}
#[test]
fn grant_codec_is_provider_fixture() {
    let c = corpus();
    let mut s = session();
    let r = s.begin("grant", json!({"scope":"foh"}), 0).unwrap();
    assert_eq!(
        serde_json::from_slice::<Value>(&r.encode().unwrap()).unwrap(),
        c["grant_request"]
    );
}
#[test]
fn delayed_cached_grant_never_extends_lease() {
    let c = corpus();
    let mut s = session();
    let r = s.begin("grant", json!({"scope":"foh"}), 0).unwrap();
    assert!(s.retry(99).is_none());
    assert_eq!(s.retry(100).unwrap(), r);
    assert_eq!(s.retry(250).unwrap(), r);
    assert_eq!(s.retry(500).unwrap(), r);
    s.accept(
        audio::decode_reply(&bytes(&c["grant_response"])).unwrap(),
        1500,
    )
    .unwrap();
    assert_eq!(s.lease_deadline(), Some(2000));
    assert!(s.begin("renew", json!({}), 2000).is_err());
}
#[test]
fn adversarial_schema_and_gain_are_refused() {
    let c = corpus();
    for (path, value) in [
        ("/capability_version", json!(3)),
        ("/meters", json!([0])),
        ("/protection", json!("protected")),
        ("/coefficients/0/current_nanogain/0", json!(3981071707u64)),
        ("/coefficients/0/held_nanogain/1", json!(-1)),
        ("/authority/revision", json!("01")),
        ("/authority/parameters/0/actual", json!(0)),
        ("/coefficients/0/input", json!("input-99")),
    ] {
        let mut v = c["initial"].clone();
        *v.pointer_mut(path).unwrap() = value;
        assert!(audio::decode_snapshot(&bytes(&v)).is_err(), "{path}");
    }
    let mut v = c["initial"].clone();
    v["coefficients"][0]["extra"] = json!(0);
    assert!(audio::decode_snapshot(&bytes(&v)).is_err());
    let b = bytes(&c["pending"]);
    let t = String::from_utf8(b).unwrap().replacen(
        "\"state\":\"pending\"",
        "\"state\":\"pending\",\"state\":\"pending\"",
        1,
    );
    assert!(audio::decode_reply(t.as_bytes()).is_err());
}
#[test]
fn wrong_identity_retains_pending() {
    let c = corpus();
    let mut s = session();
    s.begin("grant", json!({"scope":"foh"}), 0).unwrap();
    let mut r = c["grant_response"].clone();
    r["context"]["writer"] = json!("other");
    r["outcome"]["writer"] = json!("other");
    assert!(
        s.accept(audio::decode_reply(&bytes(&r)).unwrap(), 1)
            .is_err()
    );
    assert!(s.pending.is_some());
    assert_eq!(s.pending.as_ref().unwrap().state, PendingState::Sent);
}
#[test]
fn scope_and_reconnect_require_fresh_identity() {
    let c = corpus();
    let mut s = session();
    s.begin("grant", json!({"scope":"foh"}), 0).unwrap();
    s.accept(
        audio::decode_reply(&bytes(&c["grant_response"])).unwrap(),
        1,
    )
    .unwrap();
    assert!(
        s.begin(
            "set",
            json!({"targets":[{"target":{"parameter":"fader","input":"input-01"},"value":-3000}]}),
            2
        )
        .is_err()
    );
    s.input_released();
    assert!(s.begin("set",json!({"targets":[{"target":{"parameter":"send","input":"input-01","monitor":"monitor-1"},"value":-3000}]}),2).is_err());
    s.begin(
        "set",
        json!({"targets":[{"target":{"parameter":"fader","input":"input-01"},"value":-3000}]}),
        2,
    )
    .unwrap();
    s.disconnect();
    assert!(s.pending.is_none());
    assert!(s.reconnect(9, "desk-corpus").is_err());
    s.reconnect(9, "desk-new").unwrap();
    assert!(s.begin("grant", json!({"scope":"foh"}), 3).is_err());
}
