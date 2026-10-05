//! Brain consumer safety adversaries. Producer acceptance is separately hash-pinned.
use serde_json::json;
use shr_desk::{
    audio::{Context, Request},
    brain::{self, HoldMidi, Source},
    scopes,
};
#[test]
fn separate_scopes_and_explicit_version_preserve_authority() {
    for scope in [
        "local_operator_monitor",
        "talkback_destinations",
        "talkback_foh",
    ] {
        assert_eq!(scopes::value(scope).unwrap(), scope);
        assert!(
            shr_desk::audio::Session::new("11111111-1111-4111-8111-111111111111", 1, "desk", scope)
                .is_err()
        );
    }
    let mut r = Request {
        version: 2,
        context: Context {
            show_id: "11111111-1111-4111-8111-111111111111".into(),
            module: "audio".into(),
            epoch: "1".into(),
            writer: Some("desk".into()),
            lease: Some("1".into()),
            request_id: Some("3".into()),
            expected_revision: Some("2".into()),
        },
        kind: "brain_release".into(),
        body: json!({"generation":"9007199254740993"}),
    };
    let wire: serde_json::Value = serde_json::from_slice(&r.encode().unwrap()).unwrap();
    assert_eq!(wire["contract"], "GP15-brain");
    assert_eq!(wire["version"], 1);
    assert_eq!(wire["kind"], "release");
    assert_eq!(wire["body"]["generation"], "9007199254740993");
    r.version = 1;
    assert!(r.encode().is_err());
    r.version = 2;
    r.context.lease = None;
    assert!(r.encode().is_err());
}
#[test]
fn actual_topology_counts_are_independent_and_zero_based() {
    for inputs in [16, 17, 32, 33, 48] {
        for monitors in [0, 1, 3, 8, 17] {
            assert!(
                Source::Pfl { input: inputs - 1 }
                    .validate(inputs, monitors)
                    .is_ok()
            );
            assert!(
                Source::Afl { input: inputs - 1 }
                    .validate(inputs, monitors)
                    .is_ok()
            );
            assert!(
                Source::Pfl { input: inputs }
                    .validate(inputs, monitors)
                    .is_err()
            );
            assert!(
                Source::Monitor { index: monitors }
                    .validate(inputs, monitors)
                    .is_err()
            );
            if monitors > 0 {
                assert!(
                    Source::Monitor {
                        index: monitors - 1
                    }
                    .validate(inputs, monitors)
                    .is_ok()
                );
            }
            assert!(
                brain::validate_body(
                    "brain_talkback_set",
                    &json!({"monitors":[monitors],"gain_cdb":-1200,"mute":false}),
                    None,
                    inputs,
                    monitors
                )
                .is_err()
            );
        }
    }
}
#[test]
fn strict_commands_refuse_duplicate_destinations_extra_keys_and_noncanonical_counters() {
    for generation in [
        json!(1),
        json!("0"),
        json!("01"),
        json!("18446744073709551616"),
    ] {
        assert!(
            brain::validate_body("brain_hold", &json!({"generation":generation}), None, 48, 4)
                .is_err()
        );
    }
    for body in [
        json!({"monitors":[0,0],"gain_cdb":-1200,"mute":false}),
        json!({"monitors":[0],"gain_cdb":1,"mute":false}),
        json!({"monitors":[],"gain_cdb":-9001,"mute":false}),
        json!({"monitors":[],"gain_cdb":0,"mute":false,"foh":true}),
    ] {
        assert!(brain::validate_body("brain_talkback_set", &body, None, 48, 4).is_err());
    }
    assert!(
        brain::validate_body("brain_talkback_foh", &json!({"enabled":true}), None, 48, 4).is_ok()
    );
    assert_ne!(
        brain::scope("brain_talkback_set"),
        brain::scope("brain_talkback_foh")
    );
}
#[test]
fn injected_midi_real_release_edges_fence_repeated_and_reconnected_presses() {
    let mut d = HoldMidi::new(9, 36).unwrap();
    assert_eq!(d.decode(&[0x99, 36, 127]), None); // explicit release after bind
    assert_eq!(d.decode(&[0x89, 36, 127]), Some(false));
    assert_eq!(d.decode(&[0x99, 36, 127]), Some(true));
    assert_eq!(d.decode(&[0x99, 36, 127]), None);
    assert_eq!(d.decode(&[0xa9, 36, 127]), None);
    assert_eq!(d.decode(&[0x98, 36, 0]), None);
    assert_eq!(d.decode(&[0x99, 36, 0]), Some(false));
    assert_eq!(d.decode(&[0x99, 36, 127]), Some(true));
    d.fence();
    assert_eq!(d.decode(&[0x99, 36, 127]), None);
    assert_eq!(d.decode(&[0x89, 36, 0]), Some(false));
    assert_eq!(d.decode(&[0x99, 36, 127]), Some(true));
    assert_eq!(d.decode(&[0x89, 36, 128]), None);
}

#[test]
fn actual_producer_corpus_hashes_decoders_and_separate_grants() {
    use sha2::{Digest, Sha256};
    let dir = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/gp15/v1");
    let manifest: serde_json::Value =
        serde_json::from_slice(&std::fs::read(dir.join("PROVENANCE.json")).unwrap()).unwrap();
    for (name, hash) in manifest["files"].as_object().unwrap() {
        let b = std::fs::read(dir.join(name)).unwrap();
        assert_eq!(
            format!("{:x}", Sha256::digest(&b)),
            hash.as_str().unwrap(),
            "{name}"
        );
        let v: serde_json::Value = serde_json::from_slice(&b).unwrap();
        if v["contract"] == "GP15-brain" && v.get("state").is_some() {
            brain::decode_reply(&b).unwrap_or_else(|e| panic!("{name}: {e}"));
        }
    }
    for (inputs, buses) in [(16, 1), (17, 3), (32, 5), (33, 7), (48, 9)] {
        let b = std::fs::read(dir.join(format!("raw-snapshot-{inputs}-{buses}.json"))).unwrap();
        let raw = shr_desk::audio::decode_snapshot(&b).unwrap();
        assert_eq!(raw.authority.inputs.len(), inputs);
        assert_eq!(raw.authority.monitors.len(), buses);
        for (name, scope) in [
            ("operator", "local_operator_monitor"),
            ("talkback", "talkback_destinations"),
            ("protected-foh", "talkback_foh"),
        ] {
            let request: serde_json::Value = serde_json::from_slice(
                &std::fs::read(dir.join(format!("grant-{inputs}-{name}-request.json"))).unwrap(),
            )
            .unwrap();
            let mut s = shr_desk::audio::Session::new_version(
                &raw.authority.show_id,
                raw.authority.epoch.parse().unwrap(),
                request["writer"].as_str().unwrap(),
                scope,
                2,
            )
            .unwrap();
            s.ingest_snapshot(raw.clone(), 0).unwrap();
            let r = s.begin("grant", json!({"scope":scope}), 0).unwrap();
            assert_eq!(
                serde_json::from_slice::<serde_json::Value>(&r.encode().unwrap()).unwrap(),
                request
            );
            let reply = shr_desk::audio::decode_reply(
                &std::fs::read(dir.join(format!("grant-{inputs}-{name}-reply.json"))).unwrap(),
            )
            .unwrap();
            s.accept(reply, 1).unwrap();
            assert!(s.lease_deadline().is_some());
        }
    }
}
#[test]
fn producer_brain_request_codec_matches_exact_fields_and_highwater() {
    let dir = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/gp15/v1");
    for name in [
        "monitor-request.json",
        "monitor-failure-request.json",
        "talkback-configure-request.json",
        "hold-request.json",
        "heartbeat-request.json",
        "release-request.json",
        "heartbeat-after-release-request.json",
    ] {
        let v: serde_json::Value =
            serde_json::from_slice(&std::fs::read(dir.join(name)).unwrap()).unwrap();
        let c = Context {
            show_id: v["show_id"].as_str().unwrap().into(),
            module: v["module"].as_str().unwrap().into(),
            epoch: v["epoch"].as_str().unwrap().into(),
            writer: Some(v["writer"].as_str().unwrap().into()),
            lease: Some(v["lease"].as_str().unwrap().into()),
            request_id: Some(v["request_id"].as_str().unwrap().into()),
            expected_revision: Some(v["expected_revision"].as_str().unwrap().into()),
        };
        let r = Request {
            version: 2,
            context: c,
            kind: format!("brain_{}", v["kind"].as_str().unwrap()),
            body: v["body"].clone(),
        };
        assert_eq!(
            serde_json::from_slice::<serde_json::Value>(&r.encode().unwrap()).unwrap(),
            v,
            "{name}"
        );
    }
    let bytes = std::fs::read(dir.join("hold-final.json")).unwrap();
    let mut v: serde_json::Value = serde_json::from_slice(&bytes).unwrap();
    v["snapshot"]["microphone_peak_nano"] = json!(0.5);
    assert!(brain::decode_reply(&serde_json::to_vec(&v).unwrap()).is_err());
    let mut v: serde_json::Value = serde_json::from_slice(&bytes).unwrap();
    v["snapshot"]
        .as_object_mut()
        .unwrap()
        .remove("held_generation");
    assert!(brain::decode_reply(&serde_json::to_vec(&v).unwrap()).is_err());
}

#[test]
fn actual_device_corpus_and_exact_configuration_envelope() {
    use sha2::{Digest, Sha256};
    use shr_desk::brain_device::{self, Message};
    let dir =
        std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/gp15/device-v1");
    let m: serde_json::Value =
        serde_json::from_slice(&std::fs::read(dir.join("PROVENANCE.json")).unwrap()).unwrap();
    for (name, hash) in m["files"].as_object().unwrap() {
        let name = std::path::Path::new(name).file_name().unwrap();
        let b = std::fs::read(dir.join(name)).unwrap();
        assert_eq!(format!("{:x}", Sha256::digest(&b)), hash.as_str().unwrap());
        if name.to_str().unwrap().starts_with("snapshot-")
            || name.to_str().unwrap().starts_with("configure-") && name != "configure-request.json"
        {
            brain_device::decode(&b).unwrap_or_else(|e| panic!("{name:?}: {e}"));
        }
    }
    let bytes = std::fs::read(dir.join("configure-request.json")).unwrap();
    let v: serde_json::Value = serde_json::from_slice(&bytes).unwrap();
    let c = Context {
        show_id: v["show_id"].as_str().unwrap().into(),
        module: v["module"].as_str().unwrap().into(),
        epoch: v["epoch"].as_str().unwrap().into(),
        writer: Some(v["writer"].as_str().unwrap().into()),
        lease: Some(v["lease"].as_str().unwrap().into()),
        request_id: Some(v["request_id"].as_str().unwrap().into()),
        expected_revision: Some(v["expected_revision"].as_str().unwrap().into()),
    };
    let r = Request {
        version: 2,
        context: c,
        kind: "device_configure".into(),
        body: json!({"config":v["config"],"device_identity":[1,1]}),
    };
    assert_eq!(
        serde_json::from_slice::<serde_json::Value>(&r.encode().unwrap()).unwrap(),
        v
    );
    let Message::Reply(r) =
        brain_device::decode(&std::fs::read(dir.join("configure-applied-device.json")).unwrap())
            .unwrap()
    else {
        panic!()
    };
    assert_eq!(r.observation.unwrap().config.monitor[0].slot, 1);
    let mut bad: serde_json::Value =
        serde_json::from_slice(&std::fs::read(dir.join("configure-applied-device.json")).unwrap())
            .unwrap();
    bad["observation"]["ticket"] = json!(999);
    assert!(brain_device::decode(&serde_json::to_vec(&bad).unwrap()).is_err());
    let mut bad = v["config"].clone();
    bad["monitor"][1]["slot"] = bad["monitor"][0]["slot"].clone();
    assert!(brain_device::Config::decode(bad).is_err());
}

#[test]
fn device_session_refuses_old_pin_before_admission_and_retry_after_restart() {
    use shr_desk::audio::{PendingState, Session, decode_reply, decode_snapshot};
    let raw = decode_snapshot(include_bytes!("fixtures/gp15/v1/raw-snapshot-16-1.json")).unwrap();
    let grant: serde_json::Value = serde_json::from_slice(include_bytes!(
        "fixtures/gp15/v1/grant-16-operator-request.json"
    ))
    .unwrap();
    let mut s = Session::new_version(
        &raw.authority.show_id,
        raw.authority.epoch.parse().unwrap(),
        grant["writer"].as_str().unwrap(),
        "local_operator_monitor",
        2,
    )
    .unwrap();
    s.ingest_snapshot(raw, 0).unwrap();
    s.begin("grant", json!({"scope":"local_operator_monitor"}), 0)
        .unwrap();
    s.accept(
        decode_reply(include_bytes!(
            "fixtures/gp15/v1/grant-16-operator-reply.json"
        ))
        .unwrap(),
        1,
    )
    .unwrap();
    s.input_released();
    let mut device: serde_json::Value = serde_json::from_slice(include_bytes!(
        "fixtures/gp15/device-v1/snapshot-unarmed.json"
    ))
    .unwrap();
    s.dispatch_device(&serde_json::to_vec(&device).unwrap(), 2)
        .unwrap();
    let old = json!({"config":device["observation"]["config"],"device_identity":[device["observation"]["brain_epoch"],device["observation"]["brain_map"]]});
    device["observation"]["brain_epoch"] = json!(999);
    s.dispatch_device(&serde_json::to_vec(&device).unwrap(), 3)
        .unwrap();
    assert!(
        s.begin("device_configure", old, 4)
            .unwrap_err()
            .contains("epoch/map")
    );
    assert!(s.pending.is_none());
    let fresh = json!({"config":device["observation"]["config"],"device_identity":[device["observation"]["brain_epoch"],device["observation"]["brain_map"]]});
    let request = s.begin("device_configure", fresh, 4).unwrap();
    assert_eq!(
        request.context.request_id.as_deref(),
        Some("2"),
        "refusal consumes no ID"
    );
    device["observation"]["brain_map"] = json!(999);
    s.dispatch_device(&serde_json::to_vec(&device).unwrap(), 5)
        .unwrap();
    assert!(s.retry(104).is_none());
    assert_eq!(s.pending.as_ref().unwrap().state, PendingState::Uncertain);
}
