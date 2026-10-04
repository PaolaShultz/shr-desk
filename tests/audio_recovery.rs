use serde_json::{Value, json};
use shr_desk::audio::{self, PendingState, Session};
fn corpus() -> Value {
    serde_json::from_str(include_str!("fixtures/gp03/v1/e03-rendered.json")).unwrap()
}
fn b(v: &Value) -> Vec<u8> {
    serde_json::to_vec(v).unwrap()
}
fn granted() -> Session {
    let c = corpus();
    let mut s = Session::new(
        "11111111-1111-4111-8111-111111111111",
        9,
        "desk-corpus",
        "foh",
    )
    .unwrap();
    s.ingest_snapshot(audio::decode_snapshot(&b(&c["initial"])).unwrap(), 0)
        .unwrap();
    s.begin("grant", json!({"scope":"foh"}), 0).unwrap();
    s.accept(audio::decode_reply(&b(&c["grant_response"])).unwrap(), 1)
        .unwrap();
    s.input_released();
    s
}
fn edit() -> Value {
    json!({"targets":[{"target":{"input":"input-01","parameter":"fader"},"value":-3000}]})
}
fn id2(mut v: Value) -> Value {
    v["context"]["request_id"] = json!("2");
    if !v["outcome"].is_null() {
        v["outcome"]["request_id"] = json!("2");
    }
    v
}
#[test]
fn cached_renew_from_first_send_not_retry_receipt() {
    let c = corpus();
    let mut s = granted();
    s.ingest_snapshot(audio::decode_snapshot(&b(&c["initial"])).unwrap(), 500)
        .unwrap();
    let r = s.begin("renew", json!({}), 500).unwrap();
    assert_eq!(s.retry(600).unwrap(), r);
    let mut reply = c["grant_response"].clone();
    reply["context"] = serde_json::to_value(&r.context).unwrap();
    for k in ["lease", "request_id", "expected_revision"] {
        reply["outcome"][k] = reply["context"][k].clone();
    }
    reply["outcome"]["body"]["granted_lease"] = Value::Null;
    reply["outcome"]["body"]["scope"] = json!("foh");
    s.accept(audio::decode_reply(&b(&reply)).unwrap(), 1800)
        .unwrap();
    assert_eq!(s.lease_deadline(), Some(2500));
}
#[test]
fn pending_ticket_and_wrong_ticket_retain_request() {
    let c = corpus();
    let mut s = granted();
    let r = s.begin("set", edit(), 2).unwrap();
    s.accept(
        audio::decode_reply(&b(&id2(c["pending"].clone()))).unwrap(),
        3,
    )
    .unwrap();
    assert_eq!(s.pending.as_ref().unwrap().state, PendingState::Accepted);
    let mut wrong = id2(c["committed"][0].clone());
    wrong["ticket"] = json!("2");
    assert!(
        s.accept(audio::decode_reply(&b(&wrong)).unwrap(), 4)
            .is_err()
    );
    assert_eq!(s.pending.as_ref().unwrap().request, r);
    assert_eq!(s.retry(102).unwrap(), r);
}
#[test]
fn cached_commit_does_not_regress_newer_frame() {
    let c = corpus();
    let mut s = granted();
    s.begin("set", edit(), 2).unwrap();
    s.ingest_snapshot(audio::decode_snapshot(&b(&c["final"])).unwrap(), 3)
        .unwrap();
    let frame = s.snapshot.as_ref().unwrap().frame.clone();
    s.accept(
        audio::decode_reply(&b(&id2(c["committed"][0].clone()))).unwrap(),
        4,
    )
    .unwrap();
    assert_eq!(s.snapshot.as_ref().unwrap().frame, frame);
    assert!(s.pending.is_none());
    assert!(!s.fresh(4));
}
#[test]
fn nonadmitted_backpressure_keeps_same_id_envelope() {
    let c = corpus();
    let mut s = granted();
    let r = s.begin("set", edit(), 2).unwrap();
    let mut reply = id2(c["pending"].clone());
    reply["state"] = json!("backpressure");
    for k in ["ticket", "effective_frame", "ramp_frames"] {
        reply[k] = Value::Null;
    }
    s.accept(audio::decode_reply(&b(&reply)).unwrap(), 3)
        .unwrap();
    assert_eq!(
        s.pending.as_ref().unwrap().state,
        PendingState::Backpressure
    );
    assert_eq!(s.retry(102).unwrap(), r);
    assert!(s.begin("set", edit(), 3).is_err());
}
#[test]
fn expired_pending_never_replays() {
    let mut s = granted();
    s.begin("set", edit(), 2).unwrap();
    assert!(s.retry(2000).is_none());
    assert_eq!(s.pending.as_ref().unwrap().state, PendingState::Uncertain);
    s.disconnect();
    assert!(s.pending.is_none());
    assert!(s.retry(2001).is_none());
}
#[test]
fn new_epoch_zero_counters_accept_without_old_mix_replay() {
    let c = corpus();
    let mut s = granted();
    s.begin("set", edit(), 2).unwrap();
    s.reconnect(10, "desk-new-epoch").unwrap();
    let mut snapshot = c["initial"].clone();
    snapshot["authority"]["epoch"] = json!("10");
    snapshot["authority"]["revision"] = json!("0");
    snapshot["authority"]["sequence"] = json!("0");
    snapshot["frame"] = json!("0");
    assert!(
        s.ingest_snapshot(audio::decode_snapshot(&b(&snapshot)).unwrap(), 3)
            .unwrap()
    );
    assert!(s.pending.is_none());
    let r = s.begin("grant", json!({"scope":"foh"}), 3).unwrap();
    assert_eq!(r.context.request_id.as_deref(), Some("1"));
    assert_eq!(r.context.expected_revision.as_deref(), Some("0"));
}
#[test]
fn final_missing_admitted_ticket_retains_pending() {
    let c = corpus();
    let mut s = granted();
    s.begin("set", edit(), 2).unwrap();
    s.accept(
        audio::decode_reply(&b(&id2(c["pending"].clone()))).unwrap(),
        3,
    )
    .unwrap();
    let mut final_reply = id2(c["committed"][0].clone());
    for k in ["ticket", "effective_frame", "ramp_frames"] {
        final_reply[k] = Value::Null;
    }
    assert!(
        s.accept(audio::decode_reply(&b(&final_reply)).unwrap(), 4)
            .is_err()
    );
    assert!(s.pending.is_some());
}
#[test]
fn nested_body_snapshot_cross_session_refused() {
    let c = corpus();
    let mut r = c["grant_response"].clone();
    r["outcome"]["body"]["snapshot"] = c["initial"]["authority"].clone();
    r["outcome"]["body"]["snapshot"]["show_id"] = json!("22222222-2222-4222-8222-222222222222");
    assert!(audio::decode_reply(&b(&r)).is_err());
}
fn preview_reply(request: &audio::Request, remaining: u64) -> Value {
    let c = corpus();
    let mut r = c["grant_response"].clone();
    r["context"] = serde_json::to_value(&request.context).unwrap();
    for k in ["lease", "request_id", "expected_revision"] {
        r["outcome"][k] = r["context"][k].clone();
    }
    for k in ["granted_lease", "lease_remaining_ms", "scope"] {
        r["outcome"]["body"][k] = Value::Null;
    }
    r["outcome"]["body"]["preview"] = json!({"token":"preview-1","revision":"12","scope":"foh","remaining_ms":remaining,"ramp_frames":240,"destinations":[{"target":{"input":"input-01","parameter":"fader"},"value":-6000}]});
    r
}
fn preview_request(s: &mut Session) -> audio::Request {
    s.begin(
        "preview_release",
        json!({"targets":[{"input":"input-01","parameter":"fader"}]}),
        2,
    )
    .unwrap()
}
#[test]
fn preview_conservative_deadline_and_context_invalidation() {
    let mut s = granted();
    let r = preview_request(&mut s);
    s.accept(
        audio::decode_reply(&b(&preview_reply(&r, 2000))).unwrap(),
        1000,
    )
    .unwrap();
    assert!(s.preview(2001).is_some());
    assert!(s.preview(2002).is_none());
    s.context_changed();
    assert!(s.preview(1001).is_none());
}
#[test]
fn preview_new_revision_invalidates_but_sequence_alone_does_not() {
    let c = corpus();
    let mut s = granted();
    let r = preview_request(&mut s);
    s.accept(
        audio::decode_reply(&b(&preview_reply(&r, 2000))).unwrap(),
        3,
    )
    .unwrap();
    let mut current = c["initial"].clone();
    current["authority"]["sequence"] = json!("20");
    current["frame"] = json!("48020");
    s.ingest_snapshot(audio::decode_snapshot(&b(&current)).unwrap(), 4)
        .unwrap();
    assert!(s.preview(4).is_some());
    current["authority"]["revision"] = json!("13");
    s.ingest_snapshot(audio::decode_snapshot(&b(&current)).unwrap(), 5)
        .unwrap();
    assert!(s.preview(5).is_none());
}
#[test]
fn preview_adversarial_fields_and_wrong_target_refused() {
    let mut s = granted();
    let r = preview_request(&mut s);
    let valid = preview_reply(&r, 2000);
    for (path, v) in [
        ("/remaining_ms", json!(2001)),
        ("/ramp_frames", json!(0)),
        ("/revision", json!("13")),
        ("/destinations/0/value", json!(-3001)),
        ("/destinations/0/target/parameter", json!("mute")),
    ] {
        let mut mutated = valid.clone();
        *mutated["outcome"]["body"]["preview"]
            .pointer_mut(path)
            .unwrap() = v;
        assert!(audio::decode_reply(&b(&mutated)).is_err(), "{path}");
    }
    let mut unknown = valid.clone();
    unknown["outcome"]["body"]["preview"]["extra"] = json!(0);
    assert!(audio::decode_reply(&b(&unknown)).is_err());
    let mut missing = valid.clone();
    missing["outcome"]["body"]["preview"]
        .as_object_mut()
        .unwrap()
        .remove("scope");
    assert!(audio::decode_reply(&b(&missing)).is_err());
    let mut wrong = valid.clone();
    wrong["outcome"]["body"]["preview"]["destinations"][0]["target"]["input"] = json!("input-02");
    assert!(
        s.accept(audio::decode_reply(&b(&wrong)).unwrap(), 3)
            .is_err()
    );
    assert!(s.pending.is_some());
    assert!(s.preview(3).is_none());
}
#[test]
fn delayed_cached_expired_preview_cannot_commit() {
    let mut s = granted();
    let r = preview_request(&mut s);
    s.accept(
        audio::decode_reply(&b(&preview_reply(&r, 100))).unwrap(),
        150,
    )
    .unwrap();
    assert!(s.preview(150).is_none());
    assert!(
        s.begin("release_preview", json!({"token":"preview-1"}), 151)
            .is_err()
    );
}
#[test]
fn independent_monitor_scope_and_auto_bounds() {
    let c = corpus();
    let mut s = Session::new(
        "11111111-1111-4111-8111-111111111111",
        9,
        "desk-monitor",
        "monitor1",
    )
    .unwrap();
    s.ingest_snapshot(audio::decode_snapshot(&b(&c["initial"])).unwrap(), 0)
        .unwrap();
    let request = s.begin("grant", json!({"scope":"monitor1"}), 0).unwrap();
    let mut reply = c["grant_response"].clone();
    reply["context"] = serde_json::to_value(&request.context).unwrap();
    reply["outcome"]["writer"] = json!("desk-monitor");
    reply["outcome"]["body"]["scope"] = json!("monitor1");
    s.accept(audio::decode_reply(&b(&reply)).unwrap(), 1)
        .unwrap();
    s.input_released();
    for target in [
        json!({"input":"input-01","parameter":"fader"}),
        json!({"input":"input-01","parameter":"send","monitor":"monitor-2"}),
    ] {
        assert!(
            s.begin(
                "set",
                json!({"targets":[{"target":target,"value":-3000}]}),
                2
            )
            .is_err()
        );
    }
    assert!(
        s.begin("set_mode", json!({"mode":"auto","bounds":[]}), 2)
            .is_err()
    );
    assert!(s.begin("set_mode",json!({"mode":"auto","bounds":[{"target":{"input":"input-01","parameter":"send","monitor":"monitor-2"},"min":-60000,"max":0}]}),2).is_err());
    let request=s.begin("set_mode",json!({"mode":"auto","bounds":[{"target":{"input":"input-01","parameter":"send","monitor":"monitor-1"},"min":-60000,"max":0}]}),2).unwrap();
    assert_eq!(request.context.request_id.as_deref(), Some("2"));
}
#[test]
fn wrong_show_epoch_lease_revision_refused_without_pending_loss() {
    let c = corpus();
    for (field, value) in [
        ("show_id", json!("22222222-2222-4222-8222-222222222222")),
        ("epoch", json!("10")),
        ("lease", json!("2")),
        ("expected_revision", json!("11")),
    ] {
        let mut s = granted();
        s.begin("set", edit(), 2).unwrap();
        let mut reply = id2(c["pending"].clone());
        reply["context"][field] = value;
        let r = audio::decode_reply(&b(&reply)).unwrap();
        assert!(s.accept(r, 3).is_err());
        assert!(s.pending.is_some());
    }
}
#[test]
fn stale_and_faulted_state_disable_new_writes() {
    let c = corpus();
    let mut s = granted();
    assert!(s.begin("set", edit(), 251).is_err());
    let mut fault = c["initial"].clone();
    fault["faulted"] = json!(true);
    s.ingest_snapshot(audio::decode_snapshot(&b(&fault)).unwrap(), 252)
        .unwrap();
    assert!(s.begin("set", edit(), 252).is_err());
}
#[test]
fn json_body_duplicates_and_floats_refuse_before_codec() {
    assert!(audio::decode_command_body(br#"{"targets":[],"targets":[]}"#).is_err());
    assert!(audio::decode_command_body(br#"{"value":1.0}"#).is_err());
}

#[test]
fn auto_bounds_null_monitor_is_not_a_foh_target() {
    let mut s = granted();
    assert!(s.begin("set_mode",json!({"mode":"auto","bounds":[{"target":{"input":"input-01","parameter":"fader","monitor":null},"min":-60000,"max":0}]}),2).is_err());
}
#[test]
fn invalid_typed_reply_is_transactional_and_preserves_every_trusted_component() {
    let c = corpus();
    let mut s = Session::new(
        "11111111-1111-4111-8111-111111111111",
        9,
        "desk-corpus",
        "foh",
    )
    .unwrap();
    s.ingest_snapshot(audio::decode_snapshot(&b(&c["initial"])).unwrap(), 0)
        .unwrap();
    let request = s.begin("grant", json!({"scope":"foh"}), 0).unwrap();
    let before = s.snapshot.clone();
    let mut reply = audio::decode_reply(&b(&c["grant_response"])).unwrap();
    let mut bad = audio::decode_snapshot(&b(&c["initial"])).unwrap();
    bad.frame = "bad-frame".into();
    reply.snapshot = Some(bad);
    assert!(s.accept(reply, 1).is_err());
    assert_eq!(s.snapshot, before);
    assert_eq!(s.lease_deadline(), None);
    assert_eq!(s.pending.as_ref().unwrap().request, request);
    assert_eq!(s.pending.as_ref().unwrap().state, PendingState::Sent);
    assert!(s.preview(1).is_none());
}
