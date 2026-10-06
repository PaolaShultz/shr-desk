//! Offline atomic consumer contracts. No device or network acceptance.
use serde_json::{Value, json};
use shr_desk::{
    audio, brain, held_proof::Identity, lease_maintenance as maintenance,
    paired_readback as paired, provider,
};

fn pair(inputs: usize) -> Value {
    let root =
        std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/held-proof-v1");
    let raw: Value = serde_json::from_slice(
        &std::fs::read(root.join(format!("baseline-raw-{inputs}.json"))).unwrap(),
    )
    .unwrap();
    let brain: Value = serde_json::from_slice(
        &std::fs::read(root.join(format!("baseline-brain-{inputs}.json"))).unwrap(),
    )
    .unwrap();
    json!({"contract":paired::CONTRACT,"version":1,"state":"snapshot","reason":null,"context":{
        "contract":paired::CONTRACT,"version":1,"kind":"readback","query_id":"1","show_id":raw["authority"]["show_id"],"module":"audio","epoch":raw["authority"]["epoch"],"authenticated_session":"1","writer":"talkback","capability_generation":"1","map_generation":raw["topology"]["map_revision"].as_u64().unwrap().to_string()
    },"raw":raw,"brain":brain})
}
fn bytes(value: &Value) -> Vec<u8> {
    serde_json::to_vec(value).unwrap()
}
fn maintenance_value() -> Value {
    let p = pair(16);
    let mut context = p["context"].clone();
    context.as_object_mut().unwrap().remove("query_id");
    context["contract"] = json!(maintenance::CONTRACT);
    context["kind"] = json!("maintain");
    context["maintenance_id"] = json!("1");
    context["scope"] = json!("talkback_destinations");
    context["lease"] = json!("1");
    json!({"contract":maintenance::CONTRACT,"version":1,"state":"maintained","reason":null,"context":context,"result":{"revision":"18446744073709551615","source_frame":"48","lease_remaining_ms":2000}})
}
#[test]
fn strict_atomic_shapes_counters_nulls_and_limits() {
    let good = maintenance_value();
    maintenance::Reply::decode(&bytes(&good)).unwrap();
    for field in ["reason", "result", "context", "state"] {
        let mut v = good.clone();
        v.as_object_mut().unwrap().remove(field);
        assert!(maintenance::Reply::decode(&bytes(&v)).is_err(), "{field}");
    }
    for (path, bad) in [
        ("/version", json!(1.0)),
        ("/context/version", json!(2)),
        ("/context/maintenance_id", json!("01")),
        ("/context/lease", json!("0")),
        ("/context/scope", json!({"monitor":2})),
        ("/result/lease_remaining_ms", json!(1999)),
        ("/result/source_frame", json!("18446744073709551616")),
    ] {
        let mut v = good.clone();
        *v.pointer_mut(path).unwrap() = bad;
        assert!(maintenance::Reply::decode(&bytes(&v)).is_err(), "{path}");
    }
    assert!(maintenance::Reply::decode(&vec![b' '; 8193]).is_err());
    let duplicate = String::from_utf8(bytes(&good))
        .unwrap()
        .replacen('{', "{\"version\":1,", 1);
    assert!(maintenance::Reply::decode(duplicate.as_bytes()).is_err());
    for reason in [
        "identity",
        "permission",
        "scope",
        "lease",
        "clock",
        "unavailable",
        "reused_id",
        "expired_id",
        "capacity",
    ] {
        let mut v = good.clone();
        v["state"] = json!("refused");
        v["result"] = Value::Null;
        v["reason"] = json!(reason);
        maintenance::Reply::decode(&bytes(&v)).unwrap();
    }
    for inputs in [16, 32, 48] {
        let p = pair(inputs);
        let decoded = paired::Reply::decode(&bytes(&p)).unwrap();
        assert_eq!(decoded.raw.unwrap().authority.inputs.len(), inputs);
        for path in [
            "/reason",
            "/brain/held_generation",
            "/brain/hold_deadline_ms",
            "/raw/meters",
        ] {
            let mut v = p.clone();
            let (parent, key) = path.rsplit_once('/').unwrap();
            v.pointer_mut(parent)
                .unwrap()
                .as_object_mut()
                .unwrap()
                .remove(key);
            assert!(
                paired::Reply::decode(&bytes(&v)).is_err(),
                "{inputs}:{path}"
            );
        }
        for (path, bad) in [
            ("/version", json!(2)),
            ("/brain/frame", json!("49")),
            ("/brain/revision", json!("9999")),
            ("/context/map_generation", json!("9999")),
            ("/raw/coefficients/0/current_nanogain", json!([])),
        ] {
            let mut v = p.clone();
            *v.pointer_mut(path).unwrap() = bad;
            assert!(paired::Reply::decode(&bytes(&v)).is_err(), "{path}");
        }
    }
    assert!(paired::Reply::decode(&vec![b' '; provider::MAX_DOCUMENT_BYTES + 1]).is_err());
}
#[test]
fn paired_install_cannot_refresh_old_context_or_partial_state() {
    let value = pair(16);
    let reply = paired::Reply::decode(&bytes(&value)).unwrap();
    let request = reply.context.clone();
    let mut session = audio::Session::new_version(
        &request.show_id,
        request.epoch.parse().unwrap(),
        &request.writer,
        "talkback_destinations",
        2,
    )
    .unwrap();
    session
        .accept_paired(reply.clone(), &request, 10, 0, 200)
        .unwrap();
    session
        .accept_paired(reply.clone(), &request, 210, 0, 211)
        .unwrap();
    assert!(session.brain_fresh(250));
    assert!(
        !session.brain_fresh(261),
        "receive time must not restart freshness"
    );
    let previous = session.snapshot.clone();
    let mut mismatch = reply.clone();
    mismatch.brain.as_mut().unwrap().frame = "999".into();
    assert!(
        session
            .accept_paired(mismatch, &request, 200, 0, 201)
            .is_err()
    );
    assert_eq!(session.snapshot, previous);
    session.context_changed();
    assert!(session.accept_paired(reply, &request, 200, 0, 201).is_err());
    assert!(!session.brain_fresh(261));
    let identity = Identity {
        session: "1".into(),
        epoch: request.epoch,
        capability: "1".into(),
        map: request.map_generation,
    };
    assert!(
        session.begin_maintenance(&identity, 201).is_err(),
        "readback never grants a lease"
    );
}
#[test]
fn paired_nested_brain_uses_existing_strict_schema() {
    let mut v = pair(16);
    v["brain"]["source"] = json!({"kind":"main","unexpected":true});
    assert!(paired::Reply::decode(&bytes(&v)).is_err());
    let mut v = pair(16);
    v["brain"]["talkback_monitors"] = json!([65535]);
    assert!(paired::Reply::decode(&bytes(&v)).is_err());
    let _: brain::Snapshot = serde_json::from_value(pair(16)["brain"].clone()).unwrap();
}

#[test]
fn exact_producer_atomic_corpus_reconciles_hashes_and_contexts() {
    use sha2::{Digest, Sha256};
    let root =
        std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/atomic-control-v1");
    let manifest: Value =
        serde_json::from_slice(&std::fs::read(root.join("PROVENANCE.json")).unwrap()).unwrap();
    for (name, meta) in manifest["files"].as_object().unwrap() {
        let b = std::fs::read(root.join(name)).unwrap();
        assert_eq!(format!("{:x}", Sha256::digest(&b)), meta["sha256"]);
        assert_eq!(b.len() as u64, meta["bytes"].as_u64().unwrap());
        if name == "maintain-scope.json" {
            // Historical producer bytes: FOH is now admitted by the codec,
            // but this old provider's explicit refusal still grants no authority.
            let reply = maintenance::Reply::decode(&b).unwrap();
            assert_eq!(reply.state, "refused");
            assert_eq!(reply.reason.as_deref(), Some("scope"));
            assert!(reply.result.is_none());
        } else if name.starts_with("maintain-request") {
            let request: maintenance::Request = serde_json::from_slice(&b).unwrap();
            request.encode().unwrap();
        } else if name.starts_with("read-request") {
            let request: paired::Request = serde_json::from_slice(&b).unwrap();
            request.encode().unwrap();
        } else if name.starts_with("read-") {
            paired::Reply::decode(&b).unwrap_or_else(|e| panic!("{name}: {e}"));
        } else {
            maintenance::Reply::decode(&b).unwrap_or_else(|e| panic!("{name}: {e}"));
        }
    }
    for inputs in [16, 32, 48] {
        let read = paired::Reply::decode(
            &std::fs::read(root.join(format!("read-snapshot-{inputs}.json"))).unwrap(),
        )
        .unwrap();
        let expected: paired::Request = serde_json::from_slice(
            &std::fs::read(root.join(format!("read-request-{inputs}.json"))).unwrap(),
        )
        .unwrap();
        assert_eq!(read.context, expected);
        assert_eq!(read.raw.as_ref().unwrap().authority.inputs.len(), inputs);
        let maintained = maintenance::Reply::decode(
            &std::fs::read(root.join(format!("maintained-{inputs}.json"))).unwrap(),
        )
        .unwrap();
        let expected: maintenance::Request = serde_json::from_slice(
            &std::fs::read(root.join(format!("maintain-request-{inputs}.json"))).unwrap(),
        )
        .unwrap();
        assert_eq!(maintained.context, expected);
    }
}

#[test]
fn all_existing_scopes_maintain_without_fresh_topology_and_cancel_retires_authority() {
    let raw =
        audio::decode_snapshot(include_bytes!("fixtures/gp15/v1/raw-snapshot-16-1.json")).unwrap();
    for scope in [
        "foh",
        "monitor1",
        "monitor2",
        "monitor3",
        "monitor65535",
        "pa_configuration",
        "output_routes",
        "local_operator_monitor",
        "talkback_destinations",
        "talkback_foh",
    ] {
        let mut session =
            audio::Session::new_version(&raw.authority.show_id, 1, "talkback", scope, 2).unwrap();
        session.ingest_snapshot(raw.clone(), 0).unwrap();
        session
            .begin(
                "grant",
                json!({"scope":shr_desk::scopes::value(scope).unwrap()}),
                0,
            )
            .unwrap();
        let mut grant: Value = serde_json::from_slice(include_bytes!(
            "fixtures/gp15/v1/grant-16-talkback-reply.json"
        ))
        .unwrap();
        grant["outcome"]["body"]["scope"] = shr_desk::scopes::value(scope).unwrap();
        session
            .accept(audio::decode_reply(&bytes(&grant)).unwrap(), 0)
            .unwrap();
        let identity = Identity {
            session: "1".into(),
            epoch: "1".into(),
            capability: "1".into(),
            map: raw.topology.as_ref().unwrap().map_revision.to_string(),
        };
        let request = session.begin_maintenance(&identity, 500).unwrap();
        assert_eq!(request.scope, scope);
        assert!(!session.fresh(500));
        assert!(
            session.begin_maintenance(&identity, 501).is_err(),
            "only one outstanding operation"
        );
        let reply: maintenance::Reply = serde_json::from_value(json!({"contract":maintenance::CONTRACT,"version":1,"state":"maintained","reason":null,"context":request,"result":{"revision":"999","source_frame":"4800","lease_remaining_ms":2000}})).unwrap();
        for field in [
            "show_id",
            "epoch",
            "authenticated_session",
            "writer",
            "capability_generation",
            "map_generation",
            "maintenance_id",
            "scope",
            "lease",
        ] {
            let mut altered = serde_json::to_value(&reply).unwrap();
            altered["context"][field] = match field {
                "show_id" => json!("22222222-2222-4222-8222-222222222222"),
                "writer" => json!("other-writer"),
                "scope" => json!(if scope == "foh" {
                    "pa_configuration"
                } else {
                    "foh"
                }),
                _ => json!("999"),
            };
            let mismatch = maintenance::Reply::decode(&bytes(&altered)).unwrap();
            let mut candidate = session.clone();
            assert!(
                candidate.accept_maintenance(mismatch, 750).is_err(),
                "{scope}:{field}"
            );
            assert_eq!(candidate.lease_deadline(), Some(2000));
        }
        for reason in ["permission", "identity", "scope", "lease"] {
            let mut refused = reply.clone();
            refused.state = "refused".into();
            refused.reason = Some(reason.into());
            refused.result = None;
            let mut candidate = session.clone();
            assert_eq!(
                candidate
                    .accept_maintenance(refused, 750)
                    .unwrap()
                    .as_deref(),
                Some(reason)
            );
            assert_eq!(candidate.lease_deadline(), None, "{scope}:{reason}");
        }
        let mut expired = session.clone();
        assert!(expired.accept_maintenance(reply.clone(), 2000).is_err());
        assert!(expired.begin_maintenance(&identity, 2000).is_err());
        session.accept_maintenance(reply.clone(), 750).unwrap();
        assert!(
            session.accept_maintenance(reply, 751).is_err(),
            "no double extension"
        );
        assert_eq!(session.lease_deadline(), Some(2500));
        assert!(!session.fresh(750), "maintenance never freshens topology");
        let request = session.begin_maintenance(&identity, 1000).unwrap();
        let late: maintenance::Reply = serde_json::from_value(json!({"contract":maintenance::CONTRACT,"version":1,"state":"maintained","reason":null,"context":request,"result":{"revision":"999","source_frame":"4800","lease_remaining_ms":2000}})).unwrap();
        session.context_changed();
        assert!(session.accept_maintenance(late, 1001).is_err());
        assert_eq!(session.lease_deadline(), None);
        assert!(session.begin_maintenance(&identity, 1001).is_err());
    }
}

#[test]
fn maintenance_scope_codec_is_canonical_without_topology_caps() {
    for (label, wire) in [
        ("foh", json!("foh")),
        ("monitor1", json!("monitor1")),
        ("monitor2", json!("monitor2")),
        ("monitor3", json!({"monitor":3})),
        ("monitor65535", json!({"monitor":65535})),
        ("pa_configuration", json!("pa_configuration")),
        ("output_routes", json!("output_routes")),
        ("local_operator_monitor", json!("local_operator_monitor")),
        ("talkback_destinations", json!("talkback_destinations")),
        ("talkback_foh", json!("talkback_foh")),
    ] {
        let mut v = maintenance_value();
        v["context"]["scope"] = wire.clone();
        let reply = maintenance::Reply::decode(&bytes(&v)).unwrap();
        assert_eq!(reply.context.scope, label);
        assert_eq!(serde_json::to_value(&reply.context).unwrap()["scope"], wire);
        reply.context.encode().unwrap();
    }
    for invalid in [
        json!({"monitor":0}),
        json!({"monitor":1}),
        json!({"monitor":2}),
        json!({"monitor":65536}),
        json!({"monitor":3.0}),
        json!({"monitor":"3"}),
        json!("monitor3"),
        json!("monitor03"),
        json!("unknown"),
        json!({"monitor":3,"extra":0}),
    ] {
        let mut v = maintenance_value();
        v["context"]["scope"] = invalid.clone();
        assert!(maintenance::Reply::decode(&bytes(&v)).is_err(), "{invalid}");
    }
}
