//! Actual accepted producer bytes; adversarial variants never claim producer provenance.
use serde_json::{Value, json};
use sha2::{Digest, Sha256};
use shr_desk::{audio, processing, sends};
fn bytes(n: &str) -> Vec<u8> {
    std::fs::read(format!("tests/fixtures/gp18/v1-corrected/{n}.json")).unwrap()
}
#[test]
fn frozen_bytes_provenance_and_all_producer_replies() {
    let sums = std::fs::read("tests/fixtures/gp18/v1-corrected/SHA256SUMS").unwrap();
    assert_eq!(
        format!("{:x}", Sha256::digest(&sums)),
        "348e9d78c4b8ca5dff9b7a4240f3976eaf4e5dcbd64b2cff952966e923c3b6b4"
    );
    for l in std::str::from_utf8(&sums).unwrap().lines() {
        let (h, n) = l.split_once(' ').unwrap();
        let b = std::fs::read(format!(
            "tests/fixtures/gp18/v1-corrected/{}",
            n.trim_start_matches([' ', '*'])
        ))
        .unwrap();
        assert_eq!(format!("{:x}", Sha256::digest(b)), h);
    }
    for n in [
        "baseline",
        "baseline-16",
        "baseline-32",
        "baseline-48",
        "pending",
        "pending-read",
        "transition",
        "final",
        "ready",
        "backpressure",
        "stale-revision",
        "lease-expired",
        "reused-id",
        "recovery",
    ] {
        sends::decode_reply(&bytes(n)).unwrap();
    }
    for n in [
        "gp07v4-baseline",
        "gp07v4-pending",
        "gp07v4-final",
        "gp07v4-ready",
        "gp07v4-16",
        "gp07v4-32",
        "gp07v4-48",
    ] {
        assert_eq!(processing::decode_reply(&bytes(n)).unwrap().version, 4);
    }
    assert!(processing::decode_reply(&bytes("gp07v3-refusal")).is_err());
    let session = audio::Session::new_version(
        "11111111-1111-4111-8111-111111111111",
        9,
        "sends-writer",
        "monitor3",
        2,
    )
    .unwrap();
    assert_eq!(
        serde_json::from_slice::<Value>(&session.sends_request().encode().unwrap()).unwrap(),
        serde_json::from_slice::<Value>(&bytes("snapshot-request")).unwrap()
    );
    assert_eq!(
        serde_json::from_slice::<Value>(&session.processing_request().encode().unwrap()).unwrap(),
        serde_json::from_slice::<Value>(&bytes("gp07v4-snapshot-request")).unwrap()
    );
}
#[test]
fn strict_sends_domains_inventory_fade_and_complete_nullable_fields() {
    let v: Value = serde_json::from_slice(&bytes("transition")).unwrap();
    for (path, bad) in [
        ("/version", json!(2)),
        ("/snapshot/sequence", json!("0")),
        ("/snapshot/channels/0/input", json!("input-02")),
        ("/snapshot/monitors/0", json!("monitor-01")),
        ("/snapshot/supported_modes/0", json!("pre")),
        ("/snapshot/tap_positions/1", json!("pre_eq")),
        ("/snapshot/channels/16/sends/2/ready", json!(true)),
        (
            "/snapshot/channels/16/sends/2/transition_remaining_frames",
            json!(241),
        ),
        (
            "/snapshot/channels/16/sends/2/target",
            json!("raw_post_mute"),
        ),
    ] {
        let mut bad_reply = v.clone();
        *bad_reply.pointer_mut(path).unwrap() = bad;
        assert!(
            sends::decode_reply(&serde_json::to_vec(&bad_reply).unwrap()).is_err(),
            "{path}"
        );
    }
    for key in [
        "reason",
        "ticket",
        "effective_frame",
        "ramp_frames",
        "snapshot",
    ] {
        let mut b = v.clone();
        b.as_object_mut().unwrap().remove(key);
        assert!(
            sends::decode_reply(&serde_json::to_vec(&b).unwrap()).is_err(),
            "{key}"
        );
    }
    let duplicate = String::from_utf8(bytes("baseline")).unwrap().replacen(
        "\"version\": 1",
        "\"version\": 1, \"version\": 1",
        1,
    );
    assert!(sends::decode_reply(duplicate.as_bytes()).is_err());
    let mut pending: Value = serde_json::from_slice(&bytes("pending")).unwrap();
    pending["revision"] = json!("1");
    assert!(sends::decode_reply(&serde_json::to_vec(&pending).unwrap()).is_err());
    let mut final_reply: Value = serde_json::from_slice(&bytes("final")).unwrap();
    final_reply["snapshot"]["frame"] = final_reply["effective_frame"].clone();
    assert!(sends::decode_reply(&serde_json::to_vec(&final_reply).unwrap()).is_err());
}
#[test]
fn every_read_transition_endpoint_is_aligned_and_overflow_safe() {
    for (frame, remaining) in [("48", 191), ("18446744073709551615", 1)] {
        let mut v: Value = serde_json::from_slice(&bytes("transition")).unwrap();
        v["snapshot"]["frame"] = json!(frame);
        v["snapshot"]["channels"][16]["sends"][2]["transition_remaining_frames"] = json!(remaining);
        assert!(sends::decode_reply(&serde_json::to_vec(&v).unwrap()).is_err());
    }
}
