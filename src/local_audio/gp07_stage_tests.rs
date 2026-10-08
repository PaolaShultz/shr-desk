use super::*;
use std::io::{Read, Write};
use std::os::unix::net::UnixStream;
fn paired(change_before_stage: bool) {
    let corpus: Value = serde_json::from_str(include_str!(
        "../../tests/fixtures/gp03/v1/e03-rendered.json"
    ))
    .unwrap();
    let (socket, mut server) = UnixStream::pair().unwrap();
    server
        .set_read_timeout(Some(Duration::from_secs(2)))
        .unwrap();
    let original = corpus.clone();
    let child = std::thread::spawn(move || {
        let mut seen = Vec::new();
        let mut raws = 0;
        loop {
            let mut size = [0; 4];
            if server.read_exact(&mut size).is_err() {
                break;
            }
            let size = u32::from_be_bytes(size) as usize;
            assert!(size <= 65536);
            let mut bytes = vec![0; size];
            server.read_exact(&mut bytes).unwrap();
            let request: Value = serde_json::from_slice(&bytes).unwrap();
            assert!(request["writer"].is_null(), "refresh must never mutate");
            let kind = request["kind"].as_str().unwrap().to_string();
            seen.push(kind.clone());
            let revision = if change_before_stage || raws >= 1 && kind == "snapshot" {
                "13"
            } else {
                "12"
            };
            let mut reply = if kind == "snapshot" {
                raws += 1;
                let mut r = original["grant_response"].clone();
                for k in ["writer", "lease", "request_id", "expected_revision"] {
                    r["context"][k] = Value::Null;
                    r["outcome"][k] = Value::Null;
                }
                for k in ["granted_lease", "lease_remaining_ms", "scope"] {
                    r["outcome"]["body"][k] = Value::Null;
                }
                r["outcome"]["body"]["revision"] = revision.into();
                r["snapshot"] = original["initial"].clone();
                r["snapshot"]["authority"]["revision"] = revision.into();
                r
            } else {
                assert_eq!(kind, "processing_snapshot");
                let mut r: Value = serde_json::from_str(include_str!(
                    "../../tests/fixtures/gp07/v2/snapshot-reply.json"
                ))
                .unwrap();
                r["revision"] = revision.into();
                r["snapshot"]["revision"] = revision.into();
                r["snapshot"]["sequence"] = "5000".into();
                r
            };
            reply["snapshot"]["frame"] = "100000".into();
            let bytes = serde_json::to_vec(&reply).unwrap();
            if server
                .write_all(&(bytes.len() as u32).to_be_bytes())
                .is_err()
                || server.write_all(&bytes).is_err()
            {
                break;
            }
        }
        seen
    });
    let mut session = Session::new(
        "11111111-1111-4111-8111-111111111111",
        9,
        "stage-regression",
        "foh",
    )
    .unwrap();
    session
        .ingest_snapshot(
            audio::decode_snapshot(&serde_json::to_vec(&corpus["initial"]).unwrap()).unwrap(),
            0,
        )
        .unwrap();
    let mut processing = crate::processing::decode_reply(include_bytes!(
        "../../tests/fixtures/gp07/v2/snapshot-reply.json"
    ))
    .unwrap()
    .snapshot
    .unwrap();
    processing.revision = "12".into();
    let body = json!({"input":"input-01", "config":processing.channels[0].target});
    session.ingest_processing(processing, 0).unwrap();
    let mut op = Operator {
        fx_notice: None,
        paired_nonce: 0,
        paired_enabled: false,
        maintenance_replies: std::collections::VecDeque::new(),
        transport: Box::new(Transport::from_stream(socket)),
        session,
        start: Instant::now() - Duration::from_millis(300),
        draft: None,
        scope: "foh".into(),
        guard: None,
        brain_signal: None,
        brain_probes: BrainProbes::default(),
        held_baseline: None,
        held_query: None,
        held_matched: None,
        held_reuse: None,
        held_nonce: 0,
        held_highwater: 0,
        held_observation: None,
        held_refusal: None,
        last_brain_send: None,
        trace_timing: trace_timing_enabled(),
    };
    assert!(!op.session.processing_fresh(op.now()));
    if change_before_stage {
        assert!(
            op.stage("processing_set", body)
                .unwrap_err()
                .contains("context changed")
        );
        assert!(op.draft.is_none());
    } else {
        op.stage("processing_set", body).unwrap();
        assert_eq!(op.draft.as_ref().unwrap().revision, "12");
        assert!(op.session.processing_fresh(op.now()));
        op.start -= Duration::from_millis(300);
        assert!(
            op.confirm()
                .unwrap_err()
                .contains("context/revision changed")
        );
        assert!(op.draft.is_none());
        assert!(op.session.pending.is_none());
    }
    drop(op);
    let seen = child.join().unwrap();
    assert!(seen.contains(&"processing_snapshot".into()));
    assert!(
        seen.iter()
            .all(|k| k == "snapshot" || k == "processing_snapshot")
    );
}
#[test]
fn aged_processing_review_refreshes_without_rebasing_then_changed_confirmation_refuses() {
    paired(false);
}
#[test]
fn expired_raw_snapshot_cannot_rebase_queued_processing_review() {
    paired(true);
}
