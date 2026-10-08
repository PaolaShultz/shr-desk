//! Frozen actual GP/PA owner corpus plus explicitly adversarial consumer variants.
use serde_json::{Value, json};
use shr_desk::live_eq;
fn corpus() -> Vec<Value> {
    serde_json::from_slice(include_bytes!("fixtures/master-eq/v1/producer.json")).unwrap()
}
fn snapshot(label: &str) -> live_eq::Snapshot {
    live_eq::Snapshot::decode(
        corpus().into_iter().find(|v| v["label"] == label).unwrap()["snapshot"].clone(),
    )
    .unwrap()
}
#[test]
fn actual_owner_snapshots_mutation_envelopes_and_fault_domains() {
    for row in corpus() {
        if let Some(s) = row.get("snapshot") {
            let live = live_eq::Snapshot::decode(s.clone()).unwrap();
            assert_eq!(
                live.editable(),
                matches!(
                    row["label"].as_str().unwrap(),
                    "baseline" | "settled" | "recovery"
                )
            );
            if row["label"] == "source-failure" {
                assert!(live.fault_latched);
                assert!(!live.editable());
            }
        }
        if let Some(r) = row.get("reply") {
            live_eq::decode_reply(&serde_json::to_vec(r).unwrap()).unwrap();
        }
        if let Some(r) = row.get("request") {
            if row["label"] == "refusal" {
                assert!(live_eq::validate_body(&r["body"], None).is_err());
            } else {
                live_eq::validate_body(&r["body"], None).unwrap();
            }
        }
    }
    let s = snapshot("settled");
    assert!(s.retirement_occupied);
    assert!(
        s.editable(),
        "completed storage is retired at next prepare; never permanently disables editing"
    );
}
#[test]
fn live_identity_coefficients_unknown_keys_and_complete_stereo_reject_atomically() {
    let original = serde_json::to_value(snapshot("baseline")).unwrap();
    for (path, bad) in [
        ("/map_revision", json!("01")),
        ("/master_input_indices/0", json!(0)),
        ("/live_supported", json!(false)),
        ("/live_available", json!(true)),
        ("/transition_remaining_frames", json!("241")),
    ] {
        let mut v = original.clone();
        *v.pointer_mut(path).unwrap() = bad;
        if path == "/live_available" {
            v["master_input_indices"] = Value::Null;
            v["owner_json"] = Value::Null;
        }
        assert!(live_eq::Snapshot::decode(v).is_err(), "{path}");
    }
    for key in ["owner_json", "master_input_indices", "unavailable_reason"] {
        let mut v = original.clone();
        v.as_object_mut().unwrap().remove(key);
        assert!(live_eq::Snapshot::decode(v).is_err());
    }
    let mut owner = snapshot("baseline").owner().unwrap();
    owner["current"].as_array_mut().unwrap().swap(0, 1);
    owner["current_coefficients"]
        .as_array_mut()
        .unwrap()
        .swap(0, 1);
    let mut swapped = original.clone();
    swapped["owner_json"] = json!(owner.to_string());
    assert!(live_eq::Snapshot::decode(swapped).is_err());
    let mut owner = snapshot("baseline").owner().unwrap();
    owner["target_coefficients"][0][0][0] = json!(2.0);
    let mut forged = original.clone();
    forged["owner_json"] = json!(owner.to_string());
    assert!(live_eq::Snapshot::decode(forged).is_err());
}
#[test]
fn precise_supported_unavailable_mapping_and_retired_fault_state_are_honest() {
    for buses in [json!([2, 1]), json!([0, 0, 1])] {
        let mut v = serde_json::to_value(snapshot("baseline")).unwrap();
        v["program_buses"] = buses;
        v["master_input_indices"] = Value::Null;
        v["owner_json"] = Value::Null;
        v["live_available"] = json!(false);
        v["unavailable_reason"] = json!("main mapping unavailable");
        let s = live_eq::Snapshot::decode(v.clone()).unwrap();
        assert!(!s.editable());
        v["live_available"] = json!(true);
        assert!(live_eq::Snapshot::decode(v).is_err());
    }
    let mut s = snapshot("transition");
    let mut owner = s.owner().unwrap();
    owner["fault_latched"] = json!(true);
    owner["settled"] = json!(false);
    s.owner_json = Some(owner.to_string());
    s.transition_remaining_frames = "0".into();
    s.fault_latched = true;
    s.live_available = false;
    s.settled = false;
    s.unavailable_reason = Some("PA fault latched".into());
    s.validate().unwrap();
    assert!(!s.editable());
    let mut owner = s.owner().unwrap();
    owner["settled"] = json!(true);
    s.owner_json = Some(owner.to_string());
    assert!(s.validate().is_err());
}
#[test]
fn frozen_master_corpus_and_raw_map_revision_binding_preserve_confirmed_age() {
    use sha2::{Digest, Sha256};
    let sums = std::fs::read("tests/fixtures/master-eq/v1/SHA256SUMS").unwrap();
    assert_eq!(
        format!("{:x}", Sha256::digest(&sums)),
        "1ade8c839136a8b1c8351c18db66e850832b2060c1d6478c423aac13d38b5e19"
    );
    for line in std::str::from_utf8(&sums).unwrap().lines() {
        let (h, n) = line.split_once(' ').unwrap();
        let b = std::fs::read(format!(
            "tests/fixtures/master-eq/v1/{}",
            n.trim_start_matches([' ', '*'])
        ))
        .unwrap();
        assert_eq!(format!("{:x}", Sha256::digest(b)), h);
    }
    let live = snapshot("baseline");
    let mut raw = shr_desk::audio::decode_snapshot(include_bytes!(
        "fixtures/gp18/v1-corrected/raw-baseline.json"
    ))
    .unwrap();
    // Explicit adversarial transport-binding rig, not a newly produced corpus.
    raw.authority.epoch = live.epoch.clone();
    raw.authority.revision = live.revision.clone();
    raw.topology.as_mut().unwrap().map_revision = live.map_revision.parse().unwrap();
    let mut session = shr_desk::audio::Session::new_version(
        &live.show_id,
        live.epoch.parse().unwrap(),
        "map-fence",
        "pa_configuration",
        2,
    )
    .unwrap();
    session.ingest_snapshot(raw, 0).unwrap();
    session.ingest_live_eq(live.clone(), 0).unwrap();
    let mut forged = live.clone();
    forged.frame = (live.frame.parse::<u64>().unwrap() + 48).to_string();
    forged.map_revision = (live.map_revision.parse::<u64>().unwrap() + 1).to_string();
    assert!(session.ingest_live_eq(forged, 200).is_err());
    assert_eq!(session.live_eq, Some(live));
    assert_eq!(session.live_eq_age(200), Some(200));
}
#[test]
#[ignore = "explicit actual accepted PA C ABI retired fault readback; external evidence"]
fn actual_owner_fault_retired_readback_admission() {
    let path = std::env::var("GP18_OWNER_FAULT_READBACK").expect("actual PA ABI evidence path");
    let raw = std::fs::read_to_string(path).unwrap();
    let owner: Value = serde_json::from_str(&raw).unwrap();
    assert_eq!(owner["fault_latched"], true);
    assert_eq!(owner["settled"], false);
    assert_ne!(owner["current"], owner["target"]);
    let mut s = snapshot("transition");
    s.graph_generation = owner["graph_generation"].as_u64().unwrap().to_string();
    s.eq_generation = owner["eq_generation"].as_u64().unwrap().to_string();
    s.owner_json = Some(raw);
    s.master_input_indices = Some([0, 1]);
    s.program_buses = vec![0, 1];
    s.transition_remaining_frames = "0".into();
    s.fault_latched = true;
    s.live_available = false;
    s.settled = false;
    s.unavailable_reason = Some("PA fault latched".into());
    s.validate().unwrap();
    assert!(!s.editable());
}
#[test]
fn new_epoch_discards_old_live_counters_and_authorization_without_replay() {
    let old = snapshot("baseline");
    let mut raw = shr_desk::audio::decode_snapshot(include_bytes!(
        "fixtures/gp18/v1-corrected/raw-baseline.json"
    ))
    .unwrap();
    raw.authority.epoch = old.epoch.clone();
    raw.clock.as_mut().unwrap().epoch = old.epoch.parse().unwrap();
    raw.authority.revision = old.revision.clone();
    raw.topology.as_mut().unwrap().map_revision = old.map_revision.parse().unwrap();
    let mut session = shr_desk::audio::Session::new_version(
        &old.show_id,
        1,
        "old-epoch-live",
        "pa_configuration",
        2,
    )
    .unwrap();
    session.ingest_snapshot(raw.clone(), 0).unwrap();
    session.ingest_live_eq(old.clone(), 0).unwrap();
    session
        .begin("grant", json!({"scope":"pa_configuration"}), 0)
        .unwrap();
    assert!(session.pending.is_some());
    session.reconnect(2, "new-epoch-live").unwrap();
    assert!(session.pending.is_none());
    assert!(session.lease_deadline().is_none());
    assert!(session.live_eq.is_none());
    assert!(session.live_eq_age(1).is_none());
    assert!(!session.fresh(1));
    let mut fresh = old;
    fresh.epoch = "2".into();
    fresh.frame = "0".into();
    fresh.revision = "0".into();
    raw.authority.epoch = "2".into();
    raw.clock.as_mut().unwrap().epoch = 2;
    raw.authority.revision = "0".into();
    session.ingest_snapshot(raw, 1).unwrap();
    assert!(session.ingest_live_eq(fresh.clone(), 1).unwrap());
    assert!(session.live_eq_fresh(1));
    assert!(!session.ingest_live_eq(fresh, 2).unwrap());
    assert_eq!(session.live_eq_age(2), Some(1));
}
