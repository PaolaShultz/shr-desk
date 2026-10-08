use super::*;
use std::io::Read;
use std::{
    collections::VecDeque,
    sync::{Arc, Mutex},
};

#[derive(Clone, Default)]
struct FakeAuthorityConnection(Arc<Mutex<VecDeque<Vec<u8>>>>);
impl FakeAuthorityConnection {
    fn push(&self, value: Value) {
        self.0
            .lock()
            .unwrap()
            .push_back(serde_json::to_vec(&value).unwrap());
    }
}
impl AuthorityConnection for FakeAuthorityConnection {
    fn send_frame_until(&mut self, bytes: &[u8], _: Instant) -> Result<(), String> {
        let request: Value = serde_json::from_slice(bytes).unwrap();
        assert_eq!(
            request["kind"], "close",
            "only failure closure may send during buffered refresh"
        );
        Ok(())
    }
    fn receive_until(&mut self, _: Instant) -> Result<Option<Vec<u8>>, String> {
        self.receive_available()
    }
    fn receive_available(&mut self) -> Result<Option<Vec<u8>>, String> {
        Ok(self.0.lock().unwrap().pop_front())
    }
}
struct DelayedReadback {
    queued: FakeAuthorityConnection,
    delayed: Option<Vec<u8>>,
    enqueue_after_wait: Vec<Vec<u8>>,
    deadlines: Arc<Mutex<Vec<Instant>>>,
}
impl AuthorityConnection for DelayedReadback {
    fn send_frame_until(&mut self, bytes: &[u8], deadline: Instant) -> Result<(), String> {
        let request: Value = serde_json::from_slice(bytes).unwrap();
        assert!(
            request["writer"].is_null(),
            "refresh must never replay a mutation"
        );
        assert!(request["kind"].as_str().unwrap().contains("snapshot"));
        self.deadlines.lock().unwrap().push(deadline);
        Ok(())
    }
    fn receive_available(&mut self) -> Result<Option<Vec<u8>>, String> {
        self.queued.receive_available()
    }
    fn receive_until(&mut self, deadline: Instant) -> Result<Option<Vec<u8>>, String> {
        self.deadlines.lock().unwrap().push(deadline);
        // Wire preparation belongs to the fake peer, before the consumer
        // starts its deadline; arrival still occurs at this delayed read.
        self.queued
            .0
            .lock()
            .unwrap()
            .extend(self.enqueue_after_wait.drain(..));
        if let Some(bytes) = self.delayed.take() {
            Ok(Some(bytes))
        } else {
            // Honor the same real deadline without inventing a fresh timeout.
            std::thread::sleep(deadline.saturating_duration_since(Instant::now()));
            Ok(None)
        }
    }
}
fn delayed_transport(
    op: &mut Operator,
    peer: FakeAuthorityConnection,
    delayed: Option<Value>,
    tail: Vec<Value>,
) -> Arc<Mutex<Vec<Instant>>> {
    let deadlines = Arc::new(Mutex::new(Vec::new()));
    op.transport = Box::new(DelayedReadback {
        queued: peer,
        delayed: delayed.map(|v| serde_json::to_vec(&v).unwrap()),
        enqueue_after_wait: tail
            .into_iter()
            .map(|v| serde_json::to_vec(&v).unwrap())
            .collect(),
        deadlines: deadlines.clone(),
    });
    deadlines
}
fn setup() -> (Operator, FakeAuthorityConnection, Value) {
    let raw = audio::decode_snapshot(include_bytes!(
        "../../tests/fixtures/gp15/v1/raw-snapshot-16-1.json"
    ))
    .unwrap();
    let peer = FakeAuthorityConnection::default();
    let mut op = Operator::from_document_connection(
        Box::new(peer.clone()),
        &raw.authority.show_id,
        1,
        "talkback",
        "talkback_destinations",
        2,
    )
    .unwrap();
    op.session.ingest_snapshot(raw.clone(), 0).unwrap();
    op.session
        .begin("grant", json!({"scope":"talkback_destinations"}), 0)
        .unwrap();
    op.session
        .accept(
            audio::decode_reply(include_bytes!(
                "../../tests/fixtures/gp15/v1/grant-16-talkback-reply.json"
            ))
            .unwrap(),
            0,
        )
        .unwrap();
    op.session.ingest_snapshot(raw.clone(), 0).unwrap();
    op.session.input_released();
    let mut brain = crate::brain::decode_reply(include_bytes!(
        "../../tests/fixtures/gp15/v1/hold-final.json"
    ))
    .unwrap()
    .snapshot
    .unwrap();
    brain.talkback_monitors = vec![0];
    brain.revision = raw.authority.revision.clone();
    brain.frame = raw.frame.clone();
    brain.held_generation = None;
    brain.hold_generation_counter = "0".into();
    brain.hold_deadline_ms = None;
    op.session.ingest_brain(brain, 0).unwrap();
    let mut telemetry: Value = serde_json::from_slice(include_bytes!(
        "../../tests/fixtures/gp15/v1/grant-16-talkback-reply.json"
    ))
    .unwrap();
    telemetry["snapshot"] = serde_json::to_value(raw).unwrap();
    for k in ["writer", "lease", "request_id", "expected_revision"] {
        telemetry["context"][k] = Value::Null;
        telemetry["outcome"][k] = Value::Null;
    }
    for k in ["granted_lease", "lease_remaining_ms", "scope"] {
        telemetry["outcome"]["body"][k] = Value::Null;
    }
    (op, peer, telemetry)
}
fn replies(op: &Operator, request: &Request, held: bool) -> (Value, Value) {
    let revision = request
        .context
        .expected_revision
        .as_ref()
        .unwrap()
        .parse::<u64>()
        .unwrap();
    let frame = op
        .session
        .brain
        .as_ref()
        .unwrap()
        .frame
        .parse::<u64>()
        .unwrap()
        + 48;
    let pending = json!({"contract":"GP15-brain","version":1,"state":"pending","reason":null,"context":request.context,"revision":revision.to_string(),"applied_frame":frame.to_string(),"snapshot":null});
    let mut final_reply = pending.clone();
    final_reply["state"] = json!("final");
    final_reply["revision"] = json!((revision + 1).to_string());
    let mut brain = op.session.brain.clone().unwrap();
    brain.revision = (revision + 1).to_string();
    brain.frame = frame.to_string();
    brain.held_generation = held.then(|| "1".into());
    brain.hold_generation_counter = "1".into();
    brain.hold_deadline_ms = held.then(|| "150".into());
    final_reply["snapshot"] = serde_json::to_value(brain).unwrap();
    (pending, final_reply)
}
fn raw_at(raw: &Value, revision: &str, frame: &str) -> Value {
    let mut raw = raw.clone();
    raw["outcome"]["body"]["revision"] = json!(revision);
    raw["snapshot"]["authority"]["revision"] = json!(revision);
    raw["snapshot"]["authority"]["sequence"] = json!(frame);
    raw["snapshot"]["frame"] = json!(frame);
    raw["snapshot"]["clock"]["next_frame"] = json!(frame.parse::<u64>().unwrap());
    raw
}
struct PostFinalPair {
    stale: Option<Vec<u8>>,
    raw: Option<Vec<u8>>,
    phase: &'static str,
    guard: Arc<std::sync::atomic::AtomicU64>,
    deadlines: Arc<Mutex<Vec<Instant>>>,
}
impl AuthorityConnection for PostFinalPair {
    fn send_frame_until(&mut self, bytes: &[u8], deadline: Instant) -> Result<(), String> {
        let r: Value = serde_json::from_slice(bytes).unwrap();
        assert!(matches!(
            r["kind"].as_str().unwrap(),
            "snapshot" | "brain_snapshot" | "release" | "close"
        ));
        if matches!(r["kind"].as_str().unwrap(), "snapshot" | "brain_snapshot") {
            self.deadlines.lock().unwrap().push(deadline);
        }
        Ok(())
    }
    fn receive_available(&mut self) -> Result<Option<Vec<u8>>, String> {
        Ok(self.stale.take())
    }
    fn receive_until(&mut self, deadline: Instant) -> Result<Option<Vec<u8>>, String> {
        self.deadlines.lock().unwrap().push(deadline);
        if let Some(raw) = self.raw.take() {
            return Ok(Some(raw));
        }
        match self.phase {
            "transport" => Err("post-partial transport failure".into()),
            "protocol" => Ok(Some(b"malformed post-partial document".to_vec())),
            "generation" => {
                self.guard.store(1, std::sync::atomic::Ordering::Release);
                Ok(None)
            }
            _ => {
                std::thread::sleep(deadline.saturating_duration_since(Instant::now()));
                Ok(None)
            }
        }
    }
}
#[test]
fn extension_final_partial_readback_errors_close_all_observation_admission() {
    for phase in ["success", "deadline", "transport", "protocol", "generation"] {
        let (mut op, _, raw) = setup();
        let request = op
            .session
            .begin("brain_hold", json!({"generation":"1"}), op.now())
            .unwrap();
        let (_, final_reply) = replies(&op, &request, true);
        assert!(
            op.processing_frame(&serde_json::to_vec(&final_reply).unwrap())
                .unwrap()
        );
        assert!(op.session.pending.is_none());
        assert!(op.session.last_result.contains("applied"));
        let frame = final_reply["applied_frame"]
            .as_str()
            .unwrap()
            .parse::<u64>()
            .unwrap()
            + 48;
        let advanced = raw_at(
            &raw,
            final_reply["revision"].as_str().unwrap(),
            &frame.to_string(),
        );
        if phase != "success" {
            op.session.invalidate_brain_observation();
        }
        let guard = Arc::new(std::sync::atomic::AtomicU64::new(0));
        op.guard(guard.clone(), 0);
        let deadlines = Arc::new(Mutex::new(Vec::new()));
        op.transport = Box::new(PostFinalPair {
            stale: Some(serde_json::to_vec(&raw).unwrap()),
            raw: Some(serde_json::to_vec(&advanced).unwrap()),
            phase,
            guard,
            deadlines: deadlines.clone(),
        });
        let result = op.refresh_after_final();
        assert_eq!(
            op.session.snapshot.as_ref().unwrap().frame,
            frame.to_string(),
            "raw partially admitted: {phase}"
        );
        assert!(op.session.pending.is_none());
        if phase == "success" {
            result.unwrap();
            assert!(op.session.brain_fresh(op.now()));
        } else {
            let error = result.unwrap_err();
            assert!(!op.session.fresh(op.now()), "{phase}");
            assert!(!op.session.brain_fresh(op.now()), "{phase}");
            assert!(op.session.snapshot_age(op.now()).is_none());
            assert!(op.session.brain_age(op.now()).is_none());
            if phase == "generation" {
                assert!(error.contains("input context revoked"));
                assert!(!error.contains("correlated completion:"));
            } else {
                assert!(error.starts_with("correlated completion:"), "{error}");
            }
            assert_eq!(
                op.brain_probes.first_fault.is_some(),
                matches!(phase, "transport" | "protocol")
            );
        }
        let deadlines = deadlines.lock().unwrap();
        assert!(!deadlines.is_empty());
        assert!(
            deadlines.iter().all(|d| *d == deadlines[0]),
            "original budget renewed: {phase}"
        );
        if phase == "deadline" {
            assert!(Instant::now() >= deadlines[0]);
        }
    }
}
#[test]
fn pending_final_batch_and_repeated_heartbeats_keep_common_revision_fresh() {
    let (mut op, peer, raw) = setup();
    for index in 0..5 {
        let kind = if index == 0 {
            "brain_hold"
        } else {
            "brain_heartbeat"
        };
        let body = if index == 0 {
            json!({"generation":"1"})
        } else {
            json!({"generation":"1","observed_frame":op.session.brain.as_ref().unwrap().frame})
        };
        let request = op.session.begin(kind, body, op.now()).unwrap();
        let (pending, final_reply) = replies(&op, &request, true);
        let revision = final_reply["revision"].as_str().unwrap();
        let frame = final_reply["applied_frame"].as_str().unwrap();
        // Newest raw can arrive before final, with older raw after it. Only
        // final-then-newest-raw admission clears needs_snapshot coherently.
        peer.push(pending);
        peer.push(raw_at(&raw, revision, frame));
        peer.push(final_reply.clone());
        let mut regressive = raw.clone();
        regressive["snapshot"]["authority"]["sequence"] = json!("0");
        peer.push(regressive);
        op.refresh().unwrap();
        assert!(op.session.pending.is_none());
        assert!(op.session.brain_fresh(op.now()));
        assert_eq!(
            op.session.snapshot.as_ref().unwrap().authority.revision,
            revision
        );
        assert_eq!(
            op.session.brain_final.as_ref().unwrap().context,
            request.context
        );
    }
}
#[test]
fn priority_close_interleaves_with_hold_without_losing_either_correlation() {
    let (mut op, peer, raw) = setup();
    let hold = op
        .session
        .begin("brain_hold", json!({"generation":"1"}), op.now())
        .unwrap();
    let close = op.session.brain_close_request(op.now()).unwrap();
    let (pending, final_reply) = replies(&op, &hold, true);
    let (close_pending, mut close_final) = replies(&op, &close, false);
    // Both requests pinned revision zero. Hold applies first; close is
    // explicitly refused at revision one, never silently treated as applied.
    close_final["reason"] = json!("conflict");
    close_final["snapshot"] = Value::Null;
    peer.push(pending);
    peer.push(close_pending);
    peer.push(final_reply.clone());
    peer.push(close_final);
    peer.push(raw_at(
        &raw,
        "1",
        final_reply["applied_frame"].as_str().unwrap(),
    ));
    op.refresh().unwrap();
    assert!(op.session.pending.is_none());
    assert_eq!(
        op.session.brain_final.as_ref().unwrap().context,
        close.context
    );
    assert!(op.session.brain_final.as_ref().unwrap().reason.is_some());
    assert!(!op.session.brain_fresh(op.now()));
    assert_eq!(
        op.session
            .brain
            .as_ref()
            .unwrap()
            .held_generation
            .as_deref(),
        Some("1")
    );
    assert_ne!(
        op.session
            .brain_close_request(op.now())
            .unwrap()
            .context
            .request_id,
        close.context.request_id
    );
}
#[test]
fn delayed_raw_after_hold_and_heartbeat_finals_uses_one_budget_and_keeps_brain_fresh() {
    let (mut op, peer, raw) = setup();
    for index in 0..3 {
        let kind = if index == 0 {
            "brain_hold"
        } else {
            "brain_heartbeat"
        };
        let body = if index == 0 {
            json!({"generation":"1"})
        } else {
            json!({"generation":"1","observed_frame":op.session.brain.as_ref().unwrap().frame})
        };
        let request = op.session.begin(kind, body, op.now()).unwrap();
        let (pending, final_reply) = replies(&op, &request, true);
        peer.push(pending);
        peer.push(final_reply.clone());
        // An equal old raw observation cannot make this new Brain state fresh.
        peer.push(raw.clone());
        let delayed = raw_at(
            &raw,
            final_reply["revision"].as_str().unwrap(),
            final_reply["applied_frame"].as_str().unwrap(),
        );
        let deadlines = delayed_transport(&mut op, peer.clone(), Some(delayed), vec![]);
        op.refresh().unwrap();
        assert!(op.session.pending.is_none());
        assert!(op.session.brain_fresh(op.now()));
        assert_eq!(
            op.session
                .brain
                .as_ref()
                .unwrap()
                .held_generation
                .as_deref(),
            Some("1")
        );
        let deadlines = deadlines.lock().unwrap();
        assert!(deadlines.len() >= 2);
        assert!(deadlines.iter().all(|d| *d == deadlines[0]));
    }
}
#[test]
fn final_without_raw_times_out_without_admitting_stale_brain_or_resetting_deadline() {
    let (mut op, peer, _) = setup();
    let request = op
        .session
        .begin("brain_hold", json!({"generation":"1"}), op.now())
        .unwrap();
    let (pending, final_reply) = replies(&op, &request, true);
    peer.push(pending);
    peer.push(final_reply);
    let deadlines = delayed_transport(&mut op, peer, None, vec![]);
    assert!(op.refresh().unwrap_err().contains("deadline/queue bound"));
    assert!(!op.session.brain_fresh(op.now()));
    assert!(
        op.session.pending.is_none(),
        "valid final remains applied, never replayed"
    );
    let deadlines = deadlines.lock().unwrap();
    assert!(deadlines.len() >= 2);
    assert!(deadlines.iter().all(|d| *d == deadlines[0]));
}
#[test]
fn delayed_readback_cannot_reset_the_total_frame_budget() {
    let (mut op, peer, raw) = setup();
    let request = op
        .session
        .begin("brain_hold", json!({"generation":"1"}), op.now())
        .unwrap();
    let (pending, final_reply) = replies(&op, &request, true);
    peer.push(pending);
    peer.push(final_reply.clone());
    let delayed = raw_at(&raw, "1", final_reply["applied_frame"].as_str().unwrap());
    delayed_transport(&mut op, peer, Some(delayed.clone()), vec![delayed; 61]);
    let error = op.refresh().unwrap_err();
    assert!(error.contains("backlog saturated"), "{error}");
    assert!(!op.session.brain_fresh(op.now()));
}
#[test]
fn unknown_reply_still_fails_and_saturated_batch_dispatches_nothing() {
    let (mut op, peer, raw) = setup();
    let request = op
        .session
        .begin("brain_hold", json!({"generation":"1"}), op.now())
        .unwrap();
    let (pending, final_reply) = replies(&op, &request, true);
    peer.push(pending.clone());
    peer.push(final_reply.clone());
    for _ in 0..62 {
        peer.push(raw.clone());
    }
    assert!(op.refresh().unwrap_err().contains("backlog saturated"));
    assert_eq!(
        op.session.pending.as_ref().unwrap().state,
        audio::PendingState::Sent
    );
    assert!(op.session.brain_final.is_none());
    let mut unknown = pending;
    unknown["context"]["request_id"] = json!("999");
    peer.push(unknown);
    assert!(
        op.refresh()
            .unwrap_err()
            .contains("unknown Brain reply correlation")
    );
    assert!(op.session.pending.is_some());
}
struct InterruptedRead {
    replies: VecDeque<Result<Vec<u8>, String>>,
    signal: Option<Arc<crate::brain::HoldSignal>>,
    guard: Option<Arc<std::sync::atomic::AtomicU64>>,
    sent: Arc<Mutex<Vec<Value>>>,
}
impl AuthorityConnection for InterruptedRead {
    fn send_frame_until(&mut self, bytes: &[u8], _: Instant) -> Result<(), String> {
        self.sent
            .lock()
            .unwrap()
            .push(serde_json::from_slice(bytes).unwrap());
        Ok(())
    }
    fn receive_until(&mut self, _: Instant) -> Result<Option<Vec<u8>>, String> {
        self.receive_available()
    }
    fn receive_available(&mut self) -> Result<Option<Vec<u8>>, String> {
        if let Some(signal) = self.signal.take() {
            signal.release();
        }
        if let Some(guard) = self.guard.take() {
            guard.fetch_add(1, std::sync::atomic::Ordering::AcqRel);
        }
        self.replies.pop_front().transpose()
    }
}
#[test]
fn revoked_context_preserves_fifo_then_requires_new_generation_probe() {
    let (mut op, peer, _) = setup();
    probe_connection(&mut op, &peer, false);
    let guard = Arc::new(std::sync::atomic::AtomicU64::new(1));
    op.guard(guard.clone(), 1);
    op.send_brain_probe(Instant::now() + Duration::from_millis(50))
        .unwrap();
    let old = probe_reply(&op, 48);
    guard.store(2, std::sync::atomic::Ordering::Release);
    assert!(op.refresh_brain().is_err());
    assert!(!op.brain_probes.invalid);
    assert_eq!(op.brain_probes.pending.len(), 1);
    op.guard(guard, 2);
    peer.push(old);
    peer.push(probe_reply(&op, 96));
    op.refresh_brain().unwrap();
    assert_eq!(op.brain_probes.matched.as_ref().unwrap().0, 2);
    assert!(op.brain_probes.pending.is_empty());
}
#[test]
fn generic_read_classifies_only_pure_guard_cancellation_as_recoverable() {
    for malformed in [false, true] {
        let (mut op, _, raw) = setup();
        let guard = Arc::new(std::sync::atomic::AtomicU64::new(1));
        op.guard(guard.clone(), 1);
        let bytes = if malformed {
            b"{broken".to_vec()
        } else {
            serde_json::to_vec(&raw_at(&raw, "0", "48")).unwrap()
        };
        op.transport = Box::new(InterruptedRead {
            replies: VecDeque::from([Ok(bytes)]),
            signal: None,
            guard: Some(guard.clone()),
            sent: Arc::new(Mutex::new(Vec::new())),
        });
        let result = crate::frontend::refresh_worker_context(&mut op, &guard, 1);
        assert_eq!(result.as_ref().is_ok_and(|admitted| !admitted), !malformed);
        if malformed {
            assert!(result.is_err());
        }
        assert!(
            op.session.lease_deadline().is_some(),
            "classification leaves owner retirement to worker"
        );
        if !malformed {
            op.cancel(); // actual worker's next-generation transition
            op.guard(guard, 2);
            let peer = FakeAuthorityConnection::default();
            probe_connection(&mut op, &peer, false);
            peer.push(probe_reply(&op, 96));
            op.refresh_brain().unwrap();
            assert!(op.session.brain_fresh(op.now()));
        }
    }
}
#[test]
fn cancellation_cannot_mask_malformed_or_partial_probe_and_first_fault_persists() {
    for response in [
        Ok(b"{broken".to_vec()),
        Err("partial frame deadline".to_string()),
    ] {
        let (mut op, _, _) = setup();
        let guard = Arc::new(std::sync::atomic::AtomicU64::new(1));
        op.guard(guard.clone(), 1);
        op.transport = Box::new(InterruptedRead {
            replies: VecDeque::from([response]),
            signal: None,
            guard: Some(guard),
            sent: Arc::new(Mutex::new(Vec::new())),
        });
        let first = op.refresh_brain().unwrap_err();
        assert!(op.brain_probes.invalid);
        assert_eq!(op.brain_probes.first_fault.as_deref(), Some(first.as_str()));
        assert_eq!(
            op.send_brain_probe(Instant::now() + Duration::from_millis(50))
                .unwrap_err(),
            first
        );
    }
}
#[test]
fn priority_close_send_failure_remains_terminal_with_revoked_context() {
    let (mut op, peer, _) = setup();
    op.session.brain.as_mut().unwrap().held_generation = Some("1".into());
    let signal = Arc::new(crate::brain::HoldSignal::default());
    signal.press();
    signal.release();
    op.brain_signal(signal);
    let guard = Arc::new(std::sync::atomic::AtomicU64::new(2));
    op.guard(guard, 1);
    probe_connection(&mut op, &peer, true);
    let error = op.check_guard().unwrap_err();
    assert_eq!(error, "partial probe send");
    assert!(op.brain_probes.invalid);
    assert_eq!(op.brain_probes.first_fault.as_deref(), Some(error.as_str()));
}
// Scheduling tests isolate timing from full-capacity debug JSON decoding.
// Dynamic16/32/48 contracts and integrated profiles remain separate gates.
fn held_setup() -> (Operator, FakeAuthorityConnection, Value) {
    let (mut op, peer, mut raw) = setup();
    let snapshot = &mut raw["snapshot"];
    snapshot["authority"]["inputs"]
        .as_array_mut()
        .unwrap()
        .truncate(1);
    let input = snapshot["authority"]["inputs"][0].clone();
    snapshot["authority"]["parameters"]
        .as_array_mut()
        .unwrap()
        .retain(|p| p["target"]["input"] == input);
    snapshot["coefficients"].as_array_mut().unwrap().truncate(1);
    snapshot["topology"]["inputs"]
        .as_array_mut()
        .unwrap()
        .truncate(1);
    snapshot["topology"]["capture_channels"] = json!(1);
    let parsed = audio::decode_snapshot(&serde_json::to_vec(&raw["snapshot"]).unwrap()).unwrap();
    op.session.snapshot = Some(parsed);
    (op, peer, raw)
}
/// Run only on request: cargo test --release --locked -j1 held_decode_stage_benchmark -- --ignored --nocapture
#[test]
#[ignore = "finite optimized offline decode-stage benchmark; no network/PCM; descriptive timings only"]
fn held_decode_stage_benchmark() {
    use sha2::{Digest, Sha256};
    use std::hint::black_box;
    for inputs in [16usize, 32, 48] {
        // Derive equal-five-bus shapes from the producer48x9 corpus. All
        // transformations and frame preparation precede measurement, and the
        // actual strict decoder validates the derived complete document.
        let mut snapshot: Value = serde_json::from_slice(include_bytes!(
            "../../tests/fixtures/gp15/v1/raw-snapshot-48-9.json"
        ))
        .unwrap();
        snapshot["authority"]["inputs"]
            .as_array_mut()
            .unwrap()
            .truncate(inputs);
        snapshot["authority"]["monitors"]
            .as_array_mut()
            .unwrap()
            .truncate(5);
        snapshot["authority"]["modes"]
            .as_array_mut()
            .unwrap()
            .truncate(6);
        let selected_inputs = snapshot["authority"]["inputs"].as_array().unwrap().clone();
        let selected_monitors = snapshot["authority"]["monitors"]
            .as_array()
            .unwrap()
            .clone();
        snapshot["authority"]["parameters"]
            .as_array_mut()
            .unwrap()
            .retain(|p| {
                selected_inputs.contains(&p["target"]["input"])
                    && (p["target"]["parameter"] != "send"
                        || selected_monitors.contains(&p["target"]["monitor"]))
            });
        snapshot["coefficients"]
            .as_array_mut()
            .unwrap()
            .truncate(inputs);
        for coefficient in snapshot["coefficients"].as_array_mut().unwrap() {
            for lane in ["current_nanogain", "ramp_target_nanogain", "held_nanogain"] {
                coefficient[lane].as_array_mut().unwrap().truncate(9);
            }
        }
        snapshot["topology"]["inputs"]
            .as_array_mut()
            .unwrap()
            .truncate(inputs);
        snapshot["topology"]["capture_channels"] = json!(inputs);
        snapshot["topology"]["monitors"] = json!(5);
        snapshot["topology"]["outputs"]
            .as_array_mut()
            .unwrap()
            .truncate(7);
        snapshot["topology"]["playback_channels"] = json!(7);
        let parsed = audio::decode_snapshot(&serde_json::to_vec(&snapshot).unwrap()).unwrap();
        assert_eq!(parsed.authority.inputs.len(), inputs);
        assert_eq!(parsed.authority.monitors.len(), 5);
        let (mut op, _, mut raw) = setup();
        raw["snapshot"] = snapshot;
        raw["outcome"]["body"]["revision"] = json!(parsed.authority.revision);
        op.session.snapshot = Some(parsed);
        let payload = serde_json::to_vec(&raw).unwrap();
        audio::decode_reply(&payload).unwrap();
        let envelope =
            serde_json::to_vec(&json!({"kind":"reply", "session":"1", "payload":raw})).unwrap();
        let frames: Vec<Vec<u8>> = if envelope.len() <= crate::provider::MAX_BYTES {
            vec![envelope.clone()]
        } else {
            let identity = format!("{:x}", Sha256::digest(&envelope));
            let count = envelope.len().div_ceil(8192);
            envelope.chunks(8192).enumerate().map(|(index, chunk)| serde_json::to_vec(&json!({"contract":"GP14-snapshot-pages", "version":1,"identity":identity,"index":index,"count":count,"total_bytes":envelope.len(),"payload":std::str::from_utf8(chunk).unwrap()})).unwrap()).collect()
        };
        // Establish full semantic equivalence once, outside measured iterations.
        // Walking the parsed tree immediately before decode would warm its cache.
        {
            let (document, _) = crate::remote::benchmark_snapshot_decode_stages(frames.clone());
            assert_eq!(document.admitted_bytes(), payload.len());
            assert_eq!(document.value(), &raw);
        }
        let mut measurements: [Vec<u128>; 7] = std::array::from_fn(|_| Vec::with_capacity(20));
        for _ in 0..20 {
            let owned_frames = frames.clone(); // transport already owns its received buffers
            let (document, remote) =
                crate::remote::benchmark_snapshot_decode_stages(black_box(owned_frames));
            assert_eq!(document.admitted_bytes(), payload.len());
            let started = Instant::now();
            let document = op
                .processing_document(black_box(document))
                .unwrap()
                .expect("raw document");
            let discriminator = started.elapsed().as_nanos();
            let started = Instant::now();
            let reply = audio::decode_reply_document(black_box(document)).unwrap();
            let decode = started.elapsed().as_nanos();
            let started = Instant::now();
            black_box(op.telemetry(black_box(&reply)).unwrap());
            let ingest = started.elapsed().as_nanos();
            let values = [
                remote[0],
                remote[1],
                remote[2],
                discriminator,
                decode,
                ingest,
                remote.iter().sum::<u128>() + discriminator + decode + ingest,
            ];
            for (samples, value) in measurements.iter_mut().zip(values) {
                samples.push(value);
            }
        }
        let mut stages = serde_json::Map::new();
        for (name, mut values) in [
            "page_assembly",
            "envelope_decode",
            "payload_serialize",
            "contract_discriminator",
            "raw_reply_decode",
            "telemetry_clone_ingest",
            "sum_measured_stages",
        ]
        .into_iter()
        .zip(measurements)
        {
            values.sort_unstable();
            stages.insert(name.into(), json!({"min_us":values[0] as f64/1000.0,"median_us":values[10] as f64/1000.0,"p95_us":values[18] as f64/1000.0,"max_us":values[19] as f64/1000.0}));
        }
        eprintln!(
            "HELD_DECODE_BENCH {}",
            json!({"inputs":inputs,"monitors":5,"iterations":20,"debug_assertions":cfg!(debug_assertions),"payload_bytes":payload.len(),"envelope_bytes":envelope.len(),"frames":frames.len(),"stages":stages,"scope":"derived producer48x9; strict validated16/32/48x5; post-I/O elapsed Instant stages including scheduling; not thread CPU; strict-document production path; payload serialization eliminated; no network/PCM; no timing pass criterion"})
        );
    }
}
struct PairedReadGate {
    peer: FakeAuthorityConnection,
    sent: Arc<Mutex<Vec<Value>>>,
    expected_first_queries: usize,
    first: bool,
    deadline: Instant,
}
impl AuthorityConnection for PairedReadGate {
    fn send_frame_until(&mut self, bytes: &[u8], deadline: Instant) -> Result<(), String> {
        assert_eq!(deadline, self.deadline, "no nested budget reset");
        self.sent
            .lock()
            .unwrap()
            .push(serde_json::from_slice(bytes).unwrap());
        Ok(())
    }
    fn receive_until(&mut self, deadline: Instant) -> Result<Option<Vec<u8>>, String> {
        assert_eq!(deadline, self.deadline);
        if self.first {
            self.first = false;
            let sent = self.sent.lock().unwrap();
            assert_eq!(
                sent.len(),
                self.expected_first_queries,
                "required queries must precede first response wait"
            );
            assert_eq!(sent[0]["kind"], "brain_snapshot");
            if self.expected_first_queries == 2 {
                assert_eq!(sent[1]["kind"], "snapshot");
            }
        }
        self.peer.receive_available()
    }
    fn receive_available(&mut self) -> Result<Option<Vec<u8>>, String> {
        self.peer.receive_available()
    }
}
#[test]
fn held_stale_pair_is_pipelined_but_passive_and_fresh_paths_are_not() {
    for (held, stale, expected_first) in [(true, true, 2), (false, true, 1), (true, false, 1)] {
        let (mut op, peer, raw) = held_setup();
        if stale {
            op.start = Instant::now() - Duration::from_millis(251);
        }
        peer.push(probe_reply(&op, 48));
        if stale {
            peer.push(raw_at(&raw, "0", "48"));
        }
        let sent = Arc::new(Mutex::new(Vec::new()));
        let deadline = Instant::now() + Duration::from_millis(30);
        op.transport = Box::new(PairedReadGate {
            peer,
            sent: sent.clone(),
            expected_first_queries: expected_first,
            first: true,
            deadline,
        });
        let mut remaining = 64;
        op.refresh_brain_budget(deadline, &mut remaining, held)
            .map_err(BrainOperationError::message)
            .unwrap();
        assert_eq!(remaining, if stale { 62 } else { 63 });
        assert!(op.session.brain_fresh(op.now()));
        let sent = sent.lock().unwrap();
        assert_eq!(
            sent.len(),
            if stale { 2 } else { 1 },
            "no duplicate raw request for the anticipated revision"
        );
    }
}
#[test]
fn held_eager_pair_reprobes_superseded_revision_without_duplicate_raw() {
    let (mut op, peer, raw) = held_setup();
    op.start = Instant::now() - Duration::from_millis(251);
    peer.push(probe_reply(&op, 48));
    peer.push(raw_at(&raw, "1", "96"));
    let mut current = probe_reply(&op, 96);
    current["revision"] = json!("1");
    current["snapshot"]["revision"] = json!("1");
    peer.push(current);
    let sent = Arc::new(Mutex::new(Vec::new()));
    let deadline = Instant::now() + Duration::from_millis(30);
    op.transport = Box::new(PairedReadGate {
        peer,
        sent: sent.clone(),
        expected_first_queries: 2,
        first: true,
        deadline,
    });
    let mut remaining = 64;
    op.refresh_brain_budget(deadline, &mut remaining, true)
        .map_err(BrainOperationError::message)
        .unwrap();
    assert_eq!(remaining, 61);
    assert_eq!(op.brain_probes.matched.as_ref().unwrap().0, 2);
    assert_eq!(op.session.brain.as_ref().unwrap().revision, "1");
    assert_eq!(
        op.session.snapshot.as_ref().unwrap().authority.revision,
        "1"
    );
    let sent = sent.lock().unwrap();
    assert_eq!(
        sent.iter()
            .map(|r| r["kind"].as_str().unwrap())
            .collect::<Vec<_>>(),
        ["brain_snapshot", "snapshot", "brain_snapshot"]
    );
    assert!(sent.iter().all(|r| r["writer"].is_null()));
}
struct HeldPeer {
    identity: crate::held_proof::Identity,
    raw: Value,
    brain: Value,
    replies: VecDeque<Vec<u8>>,
    sent: Arc<Mutex<Vec<(Value, Instant, Instant)>>>,
    fail: Option<&'static str>,
    cancel: Option<(Arc<crate::brain::HoldSignal>, &'static str)>,
    probes: usize,
    cancel_stage: u8,
    cached: std::collections::BTreeMap<String, Vec<u8>>,
}
impl AuthorityConnection for HeldPeer {
    fn held_identity(&self) -> Option<crate::held_proof::Identity> {
        Some(self.identity.clone())
    }
    fn send_frame_until(&mut self, bytes: &[u8], deadline: Instant) -> Result<(), String> {
        let request: Value = serde_json::from_slice(bytes).unwrap();
        self.sent
            .lock()
            .unwrap()
            .push((request.clone(), Instant::now(), deadline));
        if let Some(id) = request["request_id"].as_str()
            && let Some(reply) = self.cached.get(id)
        {
            self.replies.push_back(reply.clone());
            return Ok(());
        }
        let mut context = request.clone();
        context.as_object_mut().unwrap().retain(|k, _| {
            [
                "show_id",
                "module",
                "epoch",
                "writer",
                "lease",
                "request_id",
                "expected_revision",
            ]
            .contains(&k.as_str())
        });
        let revision = self.brain["snapshot"]["revision"]
            .as_str()
            .unwrap()
            .parse::<u64>()
            .unwrap();
        let frame = self.brain["snapshot"]["frame"]
            .as_str()
            .unwrap()
            .parse::<u64>()
            .unwrap()
            + 48;
        let revision_text = revision.to_string();
        let reply = match request["kind"].as_str().unwrap() {
            "held_proof" => {
                self.probes += 1;
                if self.probes == 2
                    && self
                        .cancel
                        .as_ref()
                        .is_some_and(|(_, mode)| *mode != "maintenance")
                {
                    self.cancel_stage = 2;
                }
                self.brain["snapshot"]["frame"] = json!(frame.to_string());
                let b = &self.brain["snapshot"];
                let mut projection = serde_json::Map::new();
                for field in [
                    "selection_generation",
                    "hold_generation_counter",
                    "held_generation",
                    "hold_deadline_ms",
                    "source",
                    "monitor_armed",
                    "monitor_mute",
                    "monitor_dim",
                    "talkback_mute",
                    "talkback_foh",
                    "monitor_path_ready",
                    "talkback_path_ready",
                ] {
                    projection.insert(field.into(), b[field].clone());
                }
                json!({"contract":"GP15-held-proof","version":1,"state":"proof","reason":null,"context":request,"witness":{
                    "revision":revision.to_string(),"source_frame":frame.to_string(),"config_digest":request["expected_config_digest"],"lease_remaining_ms":2000,
                    "brain":projection,"foh_authorized":true,"media_authorized":!b["held_generation"].is_null() && b["talkback_mute"] == false,
                    "heartbeat_ms":50,"deadman_ms":150,"fade_frames":240
                }})
            }
            "brain_snapshot" => {
                self.probes += 1;
                if self.probes == 2
                    && self
                        .cancel
                        .as_ref()
                        .is_some_and(|(_, mode)| *mode != "maintenance")
                {
                    self.cancel_stage = 1;
                }
                self.brain["snapshot"]["frame"] = json!(frame.to_string());
                self.brain.clone()
            }
            "snapshot" => raw_at(&self.raw, &revision.to_string(), &frame.to_string()),
            "hold" | "heartbeat" | "release" if request["expected_revision"] != revision_text => {
                json!({"contract":"GP15-brain","version":1,"state":"final","reason":"stale_revision","context":context,"revision":revision.to_string(),"applied_frame":null,"snapshot":null})
            }
            "hold" | "heartbeat" | "release" => {
                let held = request["kind"] != "release";
                self.brain["revision"] = json!((revision + 1).to_string());
                self.brain["snapshot"]["revision"] = json!((revision + 1).to_string());
                self.brain["snapshot"]["frame"] = json!(frame.to_string());
                self.brain["snapshot"]["held_generation"] =
                    if held { json!("1") } else { Value::Null };
                if held {
                    self.brain["snapshot"]["hold_generation_counter"] = json!("1");
                }
                self.brain["snapshot"]["hold_deadline_ms"] =
                    if held { json!("99999") } else { Value::Null };
                json!({"contract":"GP15-brain","version":1,"state":"final","reason":null,"context":context,"revision":(revision+1).to_string(),"applied_frame":frame.to_string(),"snapshot":self.brain["snapshot"]})
            }
            "maintain" if self.fail == Some("maintenance_refusal") => {
                json!({"contract":"GP15-lease-maintenance","version":1,"state":"refused","reason":"lease","context":request,"result":null})
            }
            "maintain" => {
                json!({"contract":"GP15-lease-maintenance","version":1,"state":"maintained","reason":null,"context":request,"result":{"revision":revision.to_string(),"source_frame":frame.to_string(),"lease_remaining_ms":2000}})
            }
            "readback" => {
                self.brain["snapshot"]["frame"] = json!(frame.to_string());
                json!({"contract":"GP15-paired-readback","version":1,"state":"snapshot","reason":null,"context":request,"raw":raw_at(&self.raw, &revision.to_string(), &frame.to_string())["snapshot"],"brain":self.brain["snapshot"]})
            }
            other => panic!("passive/ordinary maintenance entered held service: {other}"),
        };
        let mut reply = reply;
        if request["kind"] == "maintain" && self.fail == Some("maintenance_unknown") {
            reply["context"]["maintenance_id"] = json!("999");
        }
        let bytes = serde_json::to_vec(&reply).unwrap();
        if let Some(id) = request["request_id"].as_str() {
            self.cached.insert(id.into(), bytes.clone());
        }
        self.replies.push_back(bytes);
        Ok(())
    }
    fn receive_until(&mut self, deadline: Instant) -> Result<Option<Vec<u8>>, String> {
        if self.cancel_stage == 2 {
            self.cancel_stage = 3;
            let (signal, mode) = self.cancel.as_ref().unwrap();
            signal.release();
            match *mode {
                "none" => {
                    std::thread::sleep(deadline.saturating_duration_since(Instant::now()));
                    return Ok(None);
                }
                "partial" => return Err("partial post-heartbeat probe frame".into()),
                "malformed" => return Ok(Some(b"{malformed".to_vec())),
                "late_malformed" => {
                    std::thread::sleep(deadline.saturating_duration_since(Instant::now()));
                    return Ok(Some(b"{malformed".to_vec()));
                }
                "valid" => {}
                _ => unreachable!(),
            }
        } else if self.cancel_stage == 1 {
            self.cancel_stage = 2;
        }
        if self.fail.is_some_and(|f| {
            matches!(
                f,
                "maintenance_cancel"
                    | "maintenance_unknown"
                    | "maintenance_partial"
                    | "maintenance_malformed"
            )
        }) && self.replies.front().is_some_and(|r| {
            serde_json::from_slice::<Value>(r).unwrap()["contract"] == "GP15-lease-maintenance"
        }) {
            self.cancel.as_ref().unwrap().0.release();
            if self.fail == Some("maintenance_partial") {
                return Err("partial maintenance frame".into());
            }
            if self.fail == Some("maintenance_malformed") {
                return Ok(Some(b"{malformed".to_vec()));
            }
        }
        if self.fail == Some("slow_ack") {
            if self.replies.front().is_some_and(|b| {
                let reply: Value = serde_json::from_slice(b).unwrap();
                reply["contract"] == "GP15-brain" && reply["state"] == "final"
            }) {
                std::thread::sleep(Duration::from_millis(10));
            }
            return Ok(self.replies.pop_front());
        }
        if let Some(fail) = self.fail.filter(|f| !f.starts_with("maintenance_")) {
            if fail == "partial" {
                return Err("partial held frame".into());
            }
            std::thread::sleep(deadline.saturating_duration_since(Instant::now()));
            return Ok(None);
        }
        Ok(self.replies.pop_front())
    }
    fn receive_available(&mut self) -> Result<Option<Vec<u8>>, String> {
        panic!("legacy passive drain entered held service")
    }
}
fn held_peer(
    op: &mut Operator,
    raw: Value,
    fail: Option<&'static str>,
) -> Arc<Mutex<Vec<(Value, Instant, Instant)>>> {
    held_peer_cancellation(op, raw, fail, None)
}
fn held_peer_cancellation(
    op: &mut Operator,
    raw: Value,
    fail: Option<&'static str>,
    cancel_mode: Option<&'static str>,
) -> Arc<Mutex<Vec<(Value, Instant, Instant)>>> {
    let peer = held_peer_instance(op, raw, fail, cancel_mode);
    let sent = peer.sent.clone();
    op.transport = Box::new(peer);
    op.last_brain_send = Some(Instant::now());
    sent
}
fn held_peer_instance(
    op: &mut Operator,
    raw: Value,
    fail: Option<&'static str>,
    cancel_mode: Option<&'static str>,
) -> HeldPeer {
    let signal = Arc::new(crate::brain::HoldSignal::default());
    signal.press();
    op.brain_signal(signal.clone());
    let identity = crate::held_proof::Identity {
        session: "1".into(),
        epoch: op
            .session
            .snapshot
            .as_ref()
            .unwrap()
            .authority
            .epoch
            .clone(),
        capability: "1".into(),
        map: op
            .session
            .snapshot
            .as_ref()
            .unwrap()
            .topology
            .as_ref()
            .unwrap()
            .map_revision
            .to_string(),
    };
    let dimensions = {
        let r = op.session.snapshot.as_ref().unwrap();
        [
            r.authority.inputs.len() as u32,
            r.authority.monitors.len() as u32,
            r.topology.as_ref().unwrap().pa_outputs as u32,
            r.topology.as_ref().unwrap().capture_channels as u32,
            r.topology.as_ref().unwrap().playback_channels as u32,
            48000,
        ]
    };
    let digest = crate::held_proof::configuration_digest(
        &op.session.snapshot.as_ref().unwrap().authority.show_id,
        &identity,
        dimensions,
        op.session.brain.as_ref().unwrap(),
    )
    .unwrap();
    let template = op
        .session
        .held_query(&identity, &digest, 1, op.now())
        .unwrap();
    op.held_baseline = Some(HeldBaseline {
        identity: identity.clone(),
        template,
        dimensions,
    });
    let b = op.session.brain.as_mut().unwrap();
    b.held_generation = if fail == Some("initial") {
        None
    } else {
        Some("1".into())
    };
    b.hold_generation_counter = if fail == Some("initial") {
        "0".into()
    } else {
        "1".into()
    };
    b.hold_deadline_ms = b.held_generation.as_ref().map(|_| "99999".into());
    let brain = probe_reply(op, b_frame(op) + 48);
    let sent = Arc::new(Mutex::new(Vec::new()));
    HeldPeer {
        identity,
        raw,
        brain,
        replies: VecDeque::new(),
        sent: sent.clone(),
        fail: fail.filter(|v| *v != "initial"),
        cancel: cancel_mode.map(|mode| (signal, mode)),
        probes: 0,
        cancel_stage: 0,
        cached: std::collections::BTreeMap::new(),
    }
}
// Prepare every reply before establishing any production service deadline.
struct PreparedHeldPeer {
    last_close: Option<Value>,
    identity: crate::held_proof::Identity,
    exchanges: VecDeque<(Value, Vec<u8>)>,
    ready: VecDeque<Vec<u8>>,
    sent: Arc<Mutex<Vec<(Value, Instant, Instant)>>>,
    events: Arc<Mutex<Vec<String>>>,
}
impl AuthorityConnection for PreparedHeldPeer {
    fn held_identity(&self) -> Option<crate::held_proof::Identity> {
        Some(self.identity.clone())
    }
    fn send_frame_until(&mut self, bytes: &[u8], deadline: Instant) -> Result<(), String> {
        let request: Value = serde_json::from_slice(bytes).unwrap();
        if self.last_close.as_ref() == Some(&request) {
            self.sent
                .lock()
                .unwrap()
                .push((request, Instant::now(), deadline));
            return Ok(());
        }
        let (expected, reply) = self.exchanges.pop_front().expect("unexpected service send");
        if request["kind"] == "release" {
            self.last_close = Some(request.clone());
        }
        assert_eq!(request, expected);
        self.events
            .lock()
            .unwrap()
            .push(format!("send:{}", request["kind"].as_str().unwrap()));
        self.sent
            .lock()
            .unwrap()
            .push((request, Instant::now(), deadline));
        self.ready.push_back(reply);
        Ok(())
    }
    fn receive_until(&mut self, _: Instant) -> Result<Option<Vec<u8>>, String> {
        let reply = self.ready.pop_front();
        if let Some(bytes) = &reply {
            let value: Value = serde_json::from_slice(bytes).unwrap();
            self.events
                .lock()
                .unwrap()
                .push(format!("receive:{}", value["contract"].as_str().unwrap()));
        }
        Ok(reply)
    }
    fn receive_available(&mut self) -> Result<Option<Vec<u8>>, String> {
        panic!("unexpected passive receive")
    }
}
type HeldSends = Arc<Mutex<Vec<(Value, Instant, Instant)>>>;
fn prepared_held_peer(
    op: &mut Operator,
    raw: Value,
    kinds: &[&str],
) -> (HeldSends, Arc<Mutex<Vec<String>>>) {
    let mut peer = held_peer_instance(op, raw, None, None);
    let template = op.held_baseline.as_ref().unwrap().template.clone();
    let mut nonce = 0u64;
    let mut request_id = 2u64;
    let mut frame = String::new();
    let mut exchanges = VecDeque::new();
    for kind in kinds {
        let request = match *kind {
            "held_proof" => {
                nonce += 1;
                let mut query = template.clone();
                query.query_id = nonce.to_string();
                serde_json::to_value(query).unwrap()
            }
            "heartbeat" | "release" => {
                let request = audio::Request {
                    version: 2,
                    kind: if *kind == "heartbeat" {
                        "brain_heartbeat"
                    } else {
                        "brain_release"
                    }
                    .into(),
                    context: audio::Context {
                        show_id: template.show_id.clone(),
                        module: "audio".into(),
                        epoch: template.epoch.clone(),
                        writer: Some(template.writer.clone()),
                        lease: Some(template.lease.clone()),
                        request_id: Some(request_id.to_string()),
                        expected_revision: Some(
                            peer.brain["snapshot"]["revision"].as_str().unwrap().into(),
                        ),
                    },
                    body: if *kind == "heartbeat" {
                        json!({"generation":"1", "observed_frame":frame})
                    } else {
                        json!({"generation":"1"})
                    },
                };
                request_id += 1;
                serde_json::from_slice(&request.encode().unwrap()).unwrap()
            }
            "maintain" => {
                let mut r = serde_json::to_value(&template).unwrap();
                let object = r.as_object_mut().unwrap();
                object.remove("query_id");
                object.remove("expected_config_digest");
                object.insert("contract".into(), json!("GP15-lease-maintenance"));
                object.insert("kind".into(), json!("maintain"));
                object.insert("maintenance_id".into(), json!("1"));
                r
            }
            _ => unreachable!(),
        };
        peer.send_frame_until(&serde_json::to_vec(&request).unwrap(), Instant::now())
            .unwrap();
        let reply = peer.replies.pop_front().unwrap();
        if *kind == "held_proof" {
            frame = serde_json::from_slice::<Value>(&reply).unwrap()["witness"]["source_frame"]
                .as_str()
                .unwrap()
                .into();
        }
        exchanges.push_back((request, reply));
    }
    let sent = Arc::new(Mutex::new(Vec::new()));
    let events = Arc::new(Mutex::new(Vec::new()));
    op.transport = Box::new(PreparedHeldPeer {
        last_close: None,
        identity: peer.identity,
        exchanges,
        ready: VecDeque::new(),
        sent: sent.clone(),
        events: events.clone(),
    });
    op.last_brain_send = Some(Instant::now());
    (sent, events)
}
fn b_frame(op: &Operator) -> u64 {
    op.session.brain.as_ref().unwrap().frame.parse().unwrap()
}
#[test]
fn compact_heartbeat_uses_own_frame_and_original_probe_send_age() {
    let (mut op, _, raw) = held_setup();
    let sent = held_peer(&mut op, raw, None);
    op.compact_read(Instant::now() + Duration::from_millis(30), &mut 64)
        .map_err(BrainOperationError::message)
        .unwrap();
    let proof = op.held_matched.take().unwrap();
    let frame = proof.witness().source_frame.clone();
    let reply = crate::held_proof::Reply {
        contract: crate::held_proof::CONTRACT.into(),
        version: 1,
        state: "proof".into(),
        reason: None,
        context: proof.request().clone(),
        witness: Some(proof.witness().clone()),
    };
    op.held_matched = Some(
        crate::held_proof::Matched::admit(
            reply.clone(),
            proof.request(),
            Instant::now() - Duration::from_millis(31),
            op.session.generation(),
        )
        .unwrap(),
    );
    assert!(op.proved_request("brain_heartbeat", Some(1)).is_err());
    assert!(op.session.pending.is_none());
    op.held_matched = Some(
        crate::held_proof::Matched::admit(
            reply,
            proof.request(),
            Instant::now(),
            op.session.generation(),
        )
        .unwrap(),
    );
    let (request, _, deadline) = op
        .proved_request("brain_heartbeat", Some(1))
        .map_err(BrainOperationError::message)
        .unwrap();
    assert_eq!(request.body["observed_frame"], frame);
    assert!(deadline <= Instant::now() + Duration::from_millis(20));
    assert_eq!(
        sent.lock().unwrap().len(),
        1,
        "admission alone does not retransmit"
    );
}
#[test]
fn initial_press_uses_compact_authority_without_full_snapshot_io() {
    let (mut op, _, raw) = held_setup();
    let sent = held_peer(&mut op, raw, Some("initial"));
    assert_eq!(op.start_held().unwrap(), 1);
    let sent = sent.lock().unwrap();
    assert_eq!(
        sent.iter()
            .map(|(r, _, _)| r["kind"].as_str().unwrap())
            .collect::<Vec<_>>(),
        ["held_proof", "hold"]
    );
    assert_eq!(
        op.session
            .pending
            .as_ref()
            .unwrap()
            .request
            .context
            .expected_revision
            .as_deref(),
        Some("0")
    );
}
fn alter_post_proof(
    op: &mut Operator,
    age: Duration,
    change: impl FnOnce(&mut crate::held_proof::Reply),
) {
    let old = op.held_matched.take().unwrap();
    let mut reply = crate::held_proof::Reply {
        contract: crate::held_proof::CONTRACT.into(),
        version: 1,
        state: "proof".into(),
        reason: None,
        context: old.request().clone(),
        witness: Some(old.witness().clone()),
    };
    change(&mut reply);
    let request = reply.context.clone();
    op.held_matched = Some(
        crate::held_proof::Matched::admit(reply, &request, Instant::now() - age, old.generation())
            .unwrap(),
    );
}
#[test]
fn run36_aged_or_superseded_proof_queries_again_with_original_service_budget() {
    for case in [
        "age",
        "short_lease",
        "raw_revision",
        "raw_frame",
        "brain_revision",
        "brain_frame",
        "held_revision",
        "held_frame",
        "held_highwater",
        "brain_highwater",
        "absent",
    ] {
        let (mut op, _, raw) = held_setup();
        let sent = held_peer(&mut op, raw, None);
        op.service_held(1, 1).unwrap();
        let proof = op.held_matched.as_ref().unwrap();
        let revision = proof.witness().revision.parse::<u64>().unwrap() + 10;
        let frame = proof.witness().source_frame.parse::<u64>().unwrap() + 4800;
        match case {
            "age" => alter_post_proof(&mut op, Duration::from_millis(31), |_| {}),
            "short_lease" => alter_post_proof(&mut op, Duration::from_millis(10), |r| {
                r.witness.as_mut().unwrap().lease_remaining_ms = 1
            }),
            "raw_revision" => {
                op.session.snapshot.as_mut().unwrap().authority.revision = revision.to_string()
            }
            "raw_frame" => op.session.snapshot.as_mut().unwrap().frame = frame.to_string(),
            "brain_revision" => op.session.brain.as_mut().unwrap().revision = revision.to_string(),
            "brain_frame" => op.session.brain.as_mut().unwrap().frame = frame.to_string(),
            "held_revision" => {
                op.held_observation.as_mut().unwrap().revision = revision.to_string()
            }
            "held_frame" => op.held_observation.as_mut().unwrap().source_frame = frame.to_string(),
            "held_highwater" => op.held_highwater = 2,
            "brain_highwater" => {
                op.session.brain.as_mut().unwrap().hold_generation_counter = "2".into()
            }
            "absent" => op.held_matched = None,
            _ => unreachable!(),
        }
        let anchor = op.last_brain_send.unwrap();
        let before = sent.lock().unwrap().len();
        let result = op.service_held(1, 1);
        let writes = sent.lock().unwrap();
        assert_eq!(writes[before].0["kind"], "held_proof", "{case}: {result:?}");
        assert!(writes[before].2 <= anchor + Duration::from_millis(50));
        assert!(writes[before].2 <= writes[before].1 + Duration::from_millis(30));
        if matches!(case, "age" | "short_lease" | "absent") {
            assert!(result.is_ok(), "{case}: {result:?}");
            assert_ne!(op.held_matched.as_ref().unwrap().request().query_id, "2");
        } else {
            assert!(result.is_err(), "{case}");
            assert!(
                !writes[before..]
                    .iter()
                    .any(|(r, _, _)| r["kind"] == "heartbeat"),
                "{case}"
            );
        }
    }
}
#[test]
fn run36_invalid_reuse_context_closes_without_fresh_query_or_heartbeat() {
    for case in [
        "lease",
        "attachment",
        "dimensions",
        "configuration",
        "request_writer",
        "request_scope",
        "request_lease",
        "request_session",
        "request_capability",
        "generation",
        "gesture",
        "new_gesture",
        "input_guard",
        "refusal",
        "fault",
        "fault_canceled",
        "outstanding",
        "maintenance",
        "closing",
        "provider_generation",
        "media",
        "mute",
        "foh",
    ] {
        let (mut op, _, raw) = held_setup();
        let sent = held_peer(&mut op, raw, None);
        op.service_held(1, 1).unwrap();
        match case {
            "lease" => op.start = Instant::now() - Duration::from_millis(2100),
            "attachment" => op.held_baseline.as_mut().unwrap().identity.session = "2".into(),
            "dimensions" => {
                op.session
                    .snapshot
                    .as_mut()
                    .unwrap()
                    .topology
                    .as_mut()
                    .unwrap()
                    .capture_channels += 1
            }
            "configuration" => op.session.brain.as_mut().unwrap().talkback_gain_cdb -= 1,
            "generation" => op.session.context_changed(),
            "gesture" | "new_gesture" => {
                op.brain_signal.as_ref().unwrap().press();
            }
            "input_guard" => op.guard(Arc::new(std::sync::atomic::AtomicU64::new(2)), 1),
            "refusal" => op.held_refusal = Some("retained refusal".into()),
            "fault" | "fault_canceled" => {
                if case == "fault_canceled" {
                    op.brain_signal.as_ref().unwrap().release();
                }
                op.brain_probes.invalid = true;
                op.brain_probes.first_fault = Some("original fault".into());
            }
            "outstanding" => {
                let p = op.held_matched.as_ref().unwrap();
                op.held_query = Some(HeldQuery {
                    request: p.request().clone(),
                    sent: p.sent(),
                    generation: p.generation(),
                    dimensions: op.held_baseline.as_ref().unwrap().dimensions,
                });
            }
            "maintenance" => {
                let identity = op.transport.held_identity().unwrap();
                op.session.begin_maintenance(&identity, op.now()).unwrap();
            }
            "closing" => {
                op.close_brain().unwrap();
            }
            _ => alter_post_proof(&mut op, Duration::ZERO, |r| match case {
                "request_writer" => r.context.writer = "different".into(),
                "request_scope" => r.context.scope = "foh".into(),
                "request_lease" => r.context.lease = "different".into(),
                "request_session" => r.context.authenticated_session = "2".into(),
                "request_capability" => r.context.capability_generation = "2".into(),
                "provider_generation" => {
                    r.witness.as_mut().unwrap().brain.held_generation = Some("2".into())
                }
                "media" => r.witness.as_mut().unwrap().media_authorized = false,
                "mute" => r.witness.as_mut().unwrap().brain.talkback_mute = true,
                "foh" => {
                    let w = r.witness.as_mut().unwrap();
                    w.brain.talkback_foh = true;
                    w.foh_authorized = false;
                }
                _ => unreachable!(),
            }),
        }
        let before = sent.lock().unwrap().len();
        let error = op
            .service_held(1, if case == "new_gesture" { 2 } else { 1 })
            .unwrap_err();
        assert!(!op.brain_signal.as_ref().unwrap().live(), "{case}: {error}");
        assert!(op.held_matched.is_none(), "{case}: {error}");
        assert!(
            sent.lock().unwrap()[before..]
                .iter()
                .all(|(r, _, _)| r["kind"] == "release"),
            "{case}: {error}"
        );
        if matches!(case, "fault" | "fault_canceled") {
            assert!(error.starts_with("original fault"));
            assert_eq!(
                op.brain_probes.first_fault.as_deref(),
                Some("original fault")
            );
        }
    }
}
#[test]
fn run36_consumed_post_proof_is_not_restored_after_send_failure() {
    struct FailedHeartbeat {
        identity: crate::held_proof::Identity,
        sends: Arc<Mutex<Vec<Value>>>,
    }
    impl AuthorityConnection for FailedHeartbeat {
        fn held_identity(&self) -> Option<crate::held_proof::Identity> {
            Some(self.identity.clone())
        }
        fn send_frame_until(&mut self, bytes: &[u8], _: Instant) -> Result<(), String> {
            let request: Value = serde_json::from_slice(bytes).unwrap();
            self.sends.lock().unwrap().push(request.clone());
            if request["kind"] == "heartbeat" {
                Err("partial heartbeat send".into())
            } else {
                Ok(())
            }
        }
        fn receive_until(&mut self, _: Instant) -> Result<Option<Vec<u8>>, String> {
            panic!("no receive after failed send")
        }
        fn receive_available(&mut self) -> Result<Option<Vec<u8>>, String> {
            panic!("no passive receive")
        }
    }
    let (mut op, _, raw) = held_setup();
    held_peer(&mut op, raw, None);
    op.service_held(1, 1).unwrap();
    let proof = op.held_matched.as_ref().unwrap();
    let frame = proof.witness().source_frame.clone();
    let revision = proof.witness().revision.clone();
    let sends = Arc::new(Mutex::new(Vec::new()));
    op.transport = Box::new(FailedHeartbeat {
        identity: op.transport.held_identity().unwrap(),
        sends: sends.clone(),
    });
    assert!(
        op.service_held(1, 1)
            .unwrap_err()
            .starts_with("partial heartbeat send")
    );
    assert!(op.held_matched.is_none());
    assert!(op.held_reuse.is_none());
    let sends = sends.lock().unwrap();
    assert_eq!(sends[0]["kind"], "heartbeat");
    assert_eq!(sends[0]["body"]["observed_frame"], frame);
    assert_eq!(sends[0]["expected_revision"], revision);
    assert!(sends[1..].iter().all(|r| r["kind"] == "release"));
}
#[test]
fn run36_proof_take_is_exactly_once_even_when_admission_fails() {
    for fail in [false, true] {
        let (mut op, _, raw) = held_setup();
        held_peer(&mut op, raw, None);
        op.service_held(1, 1).unwrap();
        assert!(
            op.reuse_post_proof(1, 1)
                .map_err(BrainOperationError::message)
                .unwrap()
        );
        let proof = op.held_matched.as_ref().unwrap();
        let frame = proof.witness().source_frame.clone();
        let revision = proof.witness().revision.clone();
        let sent = proof.sent();
        let observed = op.held_observation.as_ref().unwrap().observed;
        if fail {
            op.session.context_changed();
        }
        let result = op.proved_request("brain_heartbeat", Some(1));
        if fail {
            assert!(result.is_err());
        } else {
            let (request, _, deadline) = result.map_err(BrainOperationError::message).unwrap();
            assert_eq!(request.body["observed_frame"], frame);
            assert_eq!(
                request.context.expected_revision.as_deref(),
                Some(revision.as_str())
            );
            assert!(deadline <= sent + Duration::from_millis(50));
        }
        assert_eq!(op.held_observation.as_ref().unwrap().observed, observed);
        assert!(op.held_matched.is_none());
        assert!(op.held_reuse.is_none());
        assert!(op.proved_request("brain_heartbeat", Some(1)).is_err());
    }
}
#[test]
fn run36_maintenance_wire_fault_precedes_simultaneous_keyup() {
    for mode in [
        "maintenance_unknown",
        "maintenance_partial",
        "maintenance_malformed",
    ] {
        let (mut op, _, raw) = held_setup();
        let sent = held_peer_cancellation(&mut op, raw, Some(mode), Some("maintenance"));
        op.start = Instant::now() - Duration::from_millis(1500);
        let error = op.service_held(1, 1).unwrap_err();
        assert!(!error.starts_with("held input released"), "{mode}: {error}");
        assert!(op.brain_probes.invalid);
        assert!(
            op.brain_probes
                .first_fault
                .as_ref()
                .unwrap()
                .contains(&error)
        );
        assert!(op.held_matched.is_none());
        assert_eq!(
            sent.lock()
                .unwrap()
                .iter()
                .filter(|(r, _, _)| r["kind"] == "held_proof")
                .count(),
            1
        );
    }
}
#[test]
fn run36_maintenance_refusal_and_cancellation_prevent_final_proof() {
    for mode in ["maintenance_refusal", "maintenance_cancel"] {
        let (mut op, _, raw) = held_setup();
        let sent = held_peer_cancellation(&mut op, raw, Some(mode), Some("maintenance"));
        op.start = Instant::now() - Duration::from_millis(1500);
        let error = op.service_held(1, 1).unwrap_err();
        let writes = sent.lock().unwrap();
        assert_eq!(
            writes
                .iter()
                .filter(|(r, _, _)| r["kind"] == "held_proof")
                .count(),
            1,
            "{mode}: {error}"
        );
        assert_eq!(
            writes
                .iter()
                .filter(|(r, _, _)| r["kind"] == "heartbeat")
                .count(),
            1
        );
        assert!(writes.iter().any(|(r, _, _)| r["kind"] == "maintain"));
        assert!(op.held_matched.is_none());
        assert!(!op.brain_signal.as_ref().unwrap().live());
    }
}
#[test]
fn run36_due_maintenance_precedes_final_proof_with_unchanged_deadlines() {
    let (mut op, _, raw) = held_setup();
    let (sent, events) = prepared_held_peer(
        &mut op,
        raw,
        &["held_proof", "heartbeat", "maintain", "held_proof"],
    );
    op.start = Instant::now() - Duration::from_millis(1500);
    assert!(op.session.renewal_due(op.now()));
    let anchor = op.service_held(1, 1).unwrap();
    let writes = sent.lock().unwrap();
    assert_eq!(
        writes
            .iter()
            .map(|(r, _, _)| r["kind"].as_str().unwrap())
            .collect::<Vec<_>>(),
        ["held_proof", "heartbeat", "maintain", "held_proof"]
    );
    assert!(writes[2].2 <= anchor + Duration::from_millis(50));
    assert!(writes[2].2 <= writes[2].1 + Duration::from_millis(20));
    assert!(writes[3].2 <= anchor + Duration::from_millis(50));
    assert!(writes[3].2 <= writes[3].1 + Duration::from_millis(30));
    let proof = op.held_matched.as_ref().unwrap();
    assert!(proof.sent() <= writes[3].1);
    assert!(proof.sent() >= writes[2].1);
    assert_eq!(op.held_status().unwrap().observed, proof.sent());
    assert_eq!(
        *events.lock().unwrap(),
        [
            "send:held_proof",
            "receive:GP15-held-proof",
            "send:heartbeat",
            "receive:GP15-brain",
            "send:maintain",
            "receive:GP15-lease-maintenance",
            "send:held_proof",
            "receive:GP15-held-proof"
        ]
    );
}
#[test]
fn run36_actual_worker_publishes_before_idle_wait_and_fences_input() {
    for action in [
        "success",
        "release",
        "generation",
        "authorization",
        "queued",
        "fault",
    ] {
        let (mut op, _, raw) = held_setup();
        if action == "fault" {
            held_peer(&mut op, raw, Some("partial"));
        } else {
            prepared_held_peer(
                &mut op,
                raw,
                &["held_proof", "heartbeat", "held_proof", "release"],
            );
        }
        let signal = op.brain_signal.as_ref().unwrap().clone();
        let (events, update) = crate::frontend::exercise_held_worker(op, signal, action);
        let published = events.iter().position(|e| e == "published").unwrap();
        let wait = events.iter().position(|e| e == "wait").unwrap();
        assert!(published < wait, "{action}: {events:?}");
        if matches!(action, "success" | "queued") {
            assert!(update.held_status.is_some(), "{action}: {update:?}");
        } else {
            assert!(update.held_status.is_none(), "{action}: {update:?}");
        }
        if action == "generation" {
            assert_eq!(update.generation, 2);
        }
        if action == "fault" {
            assert!(update.brain_status.contains("stopped"));
        }
        if action == "queued" {
            assert!(events.iter().any(|e| e == "input"));
        }
    }
}
#[test]
fn run36_two_services_reuse_unused_post_proof_once() {
    let (mut op, _, raw) = held_setup();
    let (sent, events) = prepared_held_peer(
        &mut op,
        raw,
        &[
            "held_proof",
            "heartbeat",
            "held_proof",
            "heartbeat",
            "held_proof",
        ],
    );
    op.service_held(1, 1).unwrap();
    let unused = op.held_matched.as_ref().unwrap();
    assert!(unused.fresh());
    let retained_frame = unused.witness().source_frame.clone();
    op.service_held(1, 1).unwrap();
    let sends = sent.lock().unwrap();
    let kinds: Vec<_> = sends
        .iter()
        .map(|(r, _, _)| r["kind"].as_str().unwrap())
        .collect();
    assert_eq!(
        kinds,
        [
            "held_proof",
            "heartbeat",
            "held_proof",
            "heartbeat",
            "held_proof"
        ]
    );
    assert_eq!(sends[3].0["body"]["observed_frame"], retained_frame);
    assert_eq!(sends[3].0["expected_revision"], "1");
    assert_eq!(events.lock().unwrap().len(), 10);
    assert_eq!(sends[0].0["query_id"], "1");
    assert_eq!(sends[2].0["query_id"], "2");
    assert_eq!(sends[4].0["query_id"], "3");
    eprintln!(
        "run36 corrected: two services, three proof queries, two heartbeats; unused post-proof consumed once"
    );
}
#[test]
fn compact_held_service_has_no_full_snapshot_query_and_keeps_raw_stale() {
    let (mut op, _, raw) = held_setup();
    let sent = held_peer(&mut op, raw, None);
    let raw_frame = op.session.snapshot.as_ref().unwrap().frame.clone();
    op.service_held(1, 1).unwrap();
    assert_eq!(op.session.snapshot.as_ref().unwrap().frame, raw_frame);
    assert!(
        op.held_status()
            .is_some_and(|s| s.generation.as_deref() == Some("1") && s.media_authorized)
    );
    assert!(
        !op.session.brain_fresh(op.now()),
        "compact witness does not refresh the full raw pair"
    );
    let sent = sent.lock().unwrap();
    assert_eq!(
        sent.iter()
            .filter(|(r, _, _)| r["kind"] == "held_proof")
            .count(),
        2
    );
    assert!(
        sent.iter()
            .all(|(r, _, _)| r["kind"] != "snapshot" && r["kind"] != "brain_snapshot")
    );
}
#[test]
fn compact_generation_highwater_survives_cancel_and_rejects_regression() {
    let (mut op, _, raw) = held_setup();
    held_peer(&mut op, raw, None);
    op.compact_read(Instant::now() + Duration::from_millis(30), &mut 64)
        .map_err(BrainOperationError::message)
        .unwrap();
    assert_eq!(op.held_highwater, 1);
    let proof = op.held_matched.take().unwrap();
    let mut reply = crate::held_proof::Reply {
        contract: crate::held_proof::CONTRACT.into(),
        version: 1,
        state: "proof".into(),
        reason: None,
        context: proof.request().clone(),
        witness: Some(proof.witness().clone()),
    };
    op.cancel();
    assert_eq!(op.held_highwater, 1);
    // Even if no full Brain highwater is available, the previous own proof
    // remains a rejection floor throughout this healthy attachment.
    op.session.brain = None;
    reply.context.query_id = "2".into();
    let w = reply.witness.as_mut().unwrap();
    w.brain.hold_generation_counter = "0".into();
    w.brain.held_generation = None;
    w.brain.hold_deadline_ms = None;
    w.brain.talkback_path_ready = false;
    w.media_authorized = false;
    op.held_query = Some(HeldQuery {
        request: reply.context.clone(),
        sent: Instant::now(),
        generation: op.session.generation(),
        dimensions: op.held_baseline.as_ref().unwrap().dimensions,
    });
    assert!(
        op.processing_frame(&serde_json::to_vec(&reply).unwrap())
            .unwrap_err()
            .contains("highwater regressed")
    );
    assert_eq!(op.held_highwater, 1);
    assert!(op.held_matched.is_none());
}
#[test]
fn compact_canceled_query_is_drained_before_new_nonce_without_new_authority() {
    let (mut op, _, raw) = held_setup();
    let sent = held_peer(&mut op, raw, None);
    let b = op.held_baseline.as_ref().unwrap();
    let request = op
        .session
        .held_query(&b.identity, &b.template.expected_config_digest, 1, op.now())
        .unwrap();
    op.held_nonce = 1;
    op.held_query = Some(HeldQuery {
        request: request.clone(),
        sent: Instant::now(),
        generation: op.session.generation(),
        dimensions: b.dimensions,
    });
    op.transport
        .send_frame_until(
            &request.encode().unwrap(),
            Instant::now() + Duration::from_millis(30),
        )
        .unwrap();
    op.cancel();
    op.session.input_released();
    op.brain_signal.as_ref().unwrap().press();
    op.compact_read(Instant::now() + Duration::from_millis(30), &mut 64)
        .map_err(BrainOperationError::message)
        .unwrap();
    assert_eq!(op.held_matched.as_ref().unwrap().request().query_id, "2");
    assert_eq!(
        op.held_matched.as_ref().unwrap().generation(),
        op.session.generation()
    );
    assert!(
        sent.lock()
            .unwrap()
            .iter()
            .all(|(r, _, _)| r["kind"] == "held_proof")
    );
}
#[test]
fn compact_ptt_on_transport_without_authenticated_identity_is_closed() {
    let (mut op, _, _) = held_setup();
    assert!(!op.held_baseline_ready());
    assert!(op.start_held().unwrap_err().contains("unsupported"));
    assert!(op.session.pending.is_none());
}
#[test]
fn held_worker_service_renews_beyond_two_seconds_and_key_up_closes() {
    let (mut op, _, raw) = held_setup();
    let old_expiry = op.session.lease_deadline().unwrap();
    let sent = held_peer(&mut op, raw, None);
    let began = Instant::now();
    let signal = op.brain_signal.as_ref().unwrap().clone();
    let mut held = Some((1, 1));
    let mut heartbeat = op.held_send_anchor().unwrap();
    let mut status = String::new();
    while began.elapsed() < Duration::from_millis(2100) {
        std::thread::sleep(
            (op.held_send_anchor().unwrap() + Duration::from_millis(20))
                .saturating_duration_since(Instant::now()),
        );
        op.brain_signal.as_ref().unwrap().pulse();
        let prior = op.held_send_anchor().unwrap();
        crate::frontend::service_worker_hold(
            &mut op,
            &signal,
            &mut held,
            &mut heartbeat,
            &mut status,
        );
        assert_eq!(held, Some((1, 1)), "{status}");
        assert!(heartbeat.duration_since(prior) <= Duration::from_millis(50));
        assert!(op.session.pending.is_none());
    }
    assert!(op.session.lease_deadline().unwrap() > old_expiry);
    signal.release();
    crate::frontend::service_worker_hold(&mut op, &signal, &mut held, &mut heartbeat, &mut status);
    assert!(held.is_none());
    let sent = sent.lock().unwrap();
    assert!(sent.iter().any(|(r, _, _)| r["kind"] == "maintain"));
    assert!(sent.iter().any(|(r, _, _)| r["kind"] == "release"));
    assert!(sent.iter().all(|(_, at, deadline)| *deadline >= *at
        && deadline.duration_since(*at) <= Duration::from_millis(50)));
    let renews: Vec<_> = sent
        .iter()
        .filter(|(r, _, _)| r["kind"] == "maintain")
        .collect();
    assert!(
        renews
            .windows(2)
            .all(|w| w[0].0["maintenance_id"] != w[1].0["maintenance_id"])
    );
}
#[test]
fn held_service_anchor_precedes_delayed_completion_and_late_service_closes() {
    let (mut op, _, raw) = held_setup();
    let sent = held_peer(&mut op, raw, Some("slow_ack"));
    let anchor = op.service_held(1, 1).unwrap();
    assert!(anchor.elapsed() >= Duration::from_millis(10));
    let writes = sent.lock().unwrap();
    let (_, write_at, _) = writes
        .iter()
        .find(|(r, _, _)| r["kind"] == "heartbeat")
        .unwrap();
    assert!(anchor <= *write_at);
    drop(writes);
    op.last_brain_send = Some(Instant::now() - Duration::from_millis(51));
    assert!(op.service_held(1, 1).unwrap_err().contains("arrived late"));
    assert!(!op.brain_signal.as_ref().unwrap().live());
}
#[test]
fn held_worker_service_silence_and_partial_frame_close_without_heartbeat_retry() {
    for failure in ["silent", "partial"] {
        let (mut op, _, raw) = held_setup();
        let sent = held_peer(&mut op, raw, Some(failure));
        let began = Instant::now();
        assert!(op.service_held(1, 1).is_err());
        assert!(began.elapsed() < Duration::from_millis(150));
        assert!(!op.brain_signal.as_ref().unwrap().live());
        let sent = sent.lock().unwrap();
        assert!(sent.iter().any(|(r, _, _)| r["kind"] == "release"));
        assert!(!sent.iter().any(|(r, _, _)| r["kind"] == "heartbeat"));
    }
}
#[test]
fn key_up_inside_post_heartbeat_pair_preserves_read_only_recovery() {
    for mode in ["valid", "none"] {
        let (mut op, _, raw) = held_setup();
        let sent = held_peer_cancellation(&mut op, raw, None, Some(mode));
        let error = op.service_held(1, 1).unwrap_err();
        assert!(error.contains("held input released"), "{mode}: {error}");
        assert!(
            !op.brain_probes.invalid,
            "ordinary release is not a protocol fault"
        );
        assert!(op.brain_probes.first_fault.is_none());
        let before = sent.lock().unwrap().clone();
        assert_eq!(
            before
                .iter()
                .filter(|(r, _, _)| r["kind"] == "heartbeat")
                .count(),
            1
        );
        assert!(!before.iter().any(|(r, _, _)| r["kind"] == "renew"));
        assert!(before.iter().any(|(r, _, _)| r["kind"] == "release"));
        // Drain the retained raw/close replies, then match a new passive probe.
        // No new gesture, automatic grant or heartbeat is issued.
        op.refresh_brain().unwrap();
        assert!(op.session.brain_fresh(op.now()));
        assert!(op.session.brain.as_ref().unwrap().held_generation.is_none());
        assert!(op.session.brain_final.as_ref().unwrap().reason.is_none());
        assert_eq!(
            sent.lock()
                .unwrap()
                .iter()
                .filter(|(r, _, _)| r["kind"] == "heartbeat")
                .count(),
            1
        );
    }
}
#[test]
fn key_up_inside_post_heartbeat_pair_does_not_hide_wire_faults() {
    for mode in ["malformed", "late_malformed", "partial"] {
        let (mut op, _, raw) = held_setup();
        let sent = held_peer_cancellation(&mut op, raw, None, Some(mode));
        let error = op.service_held(1, 1).unwrap_err();
        assert!(op.brain_probes.invalid, "{mode}: {error}");
        assert!(!error.starts_with("held input released"));
        assert!(op.brain_probes.first_fault.is_some());
        assert_eq!(
            sent.lock()
                .unwrap()
                .iter()
                .filter(|(r, _, _)| r["kind"] == "heartbeat")
                .count(),
            1
        );
    }
}
#[test]
fn held_service_correlated_refusal_closes_even_if_snapshot_still_held() {
    for kind in ["brain_hold", "brain_heartbeat"] {
        let (mut op, peer, raw) = held_setup();
        let hold = (kind == "brain_hold").then(|| {
            op.session
                .begin(kind, json!({"generation":"1"}), op.now())
                .unwrap()
        });
        held_peer(&mut op, raw, None);
        let request = hold.unwrap_or_else(|| op.session.begin(kind, json!({"generation":"1", "observed_frame":op.session.brain.as_ref().unwrap().frame}), op.now()).unwrap());
        let (_, mut refusal) = replies(&op, &request, true);
        refusal["reason"] = json!("heartbeat observed frame stale/future");
        refusal["applied_frame"] = Value::Null;
        refusal["revision"] = json!(request.context.expected_revision.clone().unwrap());
        refusal["snapshot"]["revision"] = refusal["revision"].clone();
        peer.push(refusal);
        let sent = probe_connection(&mut op, &peer, false);
        let error = op.service_held(1, 1).unwrap_err();
        assert!(error.contains("held Brain refused:"), "{error}");
        assert!(!op.brain_signal.as_ref().unwrap().live());
        assert!(op.session.pending.is_none());
        assert!(
            op.session.brain.as_ref().unwrap().held_generation.is_some(),
            "closure must not depend on heldNone readback"
        );
        let sent = sent.lock().unwrap();
        assert!(!sent.is_empty());
        assert!(
            sent.iter().all(|r| r["kind"] == "release"),
            "no new probe, heartbeat or renewal after refusal: {sent:?}"
        );
    }
}
// All fake response bytes are prepared before starting consumer deadlines.
struct AtomicPeer {
    identity: crate::held_proof::Identity,
    expected: Vec<Vec<u8>>,
    responses: VecDeque<Option<Vec<u8>>>,
    ready: Option<Vec<u8>>,
    sent: Arc<Mutex<Vec<Vec<u8>>>>,
    fault: bool,
    cancel: Option<Arc<std::sync::atomic::AtomicU64>>,
}
impl AuthorityConnection for AtomicPeer {
    fn held_identity(&self) -> Option<crate::held_proof::Identity> {
        Some(self.identity.clone())
    }
    fn send_frame_until(&mut self, bytes: &[u8], _: Instant) -> Result<(), String> {
        let mut sent = self.sent.lock().unwrap();
        assert_eq!(
            bytes,
            self.expected.get(sent.len()).expect("no extra operation")
        );
        sent.push(bytes.to_vec());
        self.ready = self.responses.pop_front().flatten();
        Ok(())
    }
    fn receive_until(&mut self, deadline: Instant) -> Result<Option<Vec<u8>>, String> {
        if let Some(g) = &self.cancel {
            g.fetch_add(1, std::sync::atomic::Ordering::AcqRel);
        }
        if self.fault {
            return Err("partial atomic frame".into());
        }
        if let Some(bytes) = self.ready.take() {
            return Ok(Some(bytes));
        }
        std::thread::sleep(deadline.saturating_duration_since(Instant::now()));
        Ok(None)
    }
    fn receive_available(&mut self) -> Result<Option<Vec<u8>>, String> {
        Ok(None)
    }
}
fn atomic_identity(op: &Operator) -> crate::held_proof::Identity {
    crate::held_proof::Identity {
        session: "1".into(),
        epoch: "1".into(),
        capability: "1".into(),
        map: op
            .session
            .snapshot
            .as_ref()
            .unwrap()
            .topology
            .as_ref()
            .unwrap()
            .map_revision
            .to_string(),
    }
}
fn maintenance_reply(request: &crate::lease_maintenance::Request, reason: Option<&str>) -> Vec<u8> {
    serde_json::to_vec(&json!({"contract":"GP15-lease-maintenance","version":1,"state":if reason.is_some() {"refused"} else {"maintained"},"reason":reason,"context":request,"result":if reason.is_some() {Value::Null} else {json!({"revision":"999","source_frame":"4800","lease_remaining_ms":2000})}})).unwrap()
}
fn atomic_peer(
    op: &mut Operator,
    expected: Vec<Vec<u8>>,
    responses: Vec<Option<Vec<u8>>>,
    fault: bool,
    cancel: Option<Arc<std::sync::atomic::AtomicU64>>,
) -> Arc<Mutex<Vec<Vec<u8>>>> {
    let sent = Arc::new(Mutex::new(Vec::new()));
    op.transport = Box::new(AtomicPeer {
        identity: atomic_identity(op),
        expected,
        responses: responses.into(),
        ready: None,
        sent: sent.clone(),
        fault,
        cancel,
    });
    sent
}
fn scope_setup(scope: &str) -> Operator {
    let (mut op, _, _) = setup();
    let raw = op.session.snapshot.clone().unwrap();
    let mut session =
        Session::new_version(&raw.authority.show_id, 1, "talkback", scope, 2).unwrap();
    session.ingest_snapshot(raw, 0).unwrap();
    session
        .begin(
            "grant",
            json!({"scope":crate::scopes::value(scope).unwrap()}),
            0,
        )
        .unwrap();
    let mut grant: Value = serde_json::from_slice(include_bytes!(
        "../../tests/fixtures/gp15/v1/grant-16-talkback-reply.json"
    ))
    .unwrap();
    grant["outcome"]["body"]["scope"] = crate::scopes::value(scope).unwrap();
    session
        .accept(
            audio::decode_reply(&serde_json::to_vec(&grant).unwrap()).unwrap(),
            0,
        )
        .unwrap();
    op.session = session;
    op.scope = scope.into();
    op
}
#[test]
fn atomic_pa_passive_renewal_ignores_unrelated_heartbeat_revision_churn() {
    let mut op = scope_setup("pa_configuration");
    op.paired_enabled = true;
    let (reference, _, _) = setup();
    let mut request = reference
        .session
        .clone()
        .begin_maintenance(&atomic_identity(&op), 500)
        .unwrap();
    request.scope = "pa_configuration".into();
    let sent = atomic_peer(
        &mut op,
        vec![serde_json::to_vec(&request).unwrap()],
        vec![Some(maintenance_reply(&request, None))],
        false,
        None,
    );
    let before = op.session.snapshot.clone();
    op.start = Instant::now() - Duration::from_millis(500);
    op.mutate("renew", json!({})).unwrap();
    assert_eq!(sent.lock().unwrap().len(), 1);
    assert_eq!(
        op.session.snapshot, before,
        "revision999 maintenance is not topology"
    );
}
const MAINTENANCE_SCOPES: &[&str] = &[
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
];
#[test]
fn atomic_all_scopes_route_both_entrypoints_without_topology() {
    for scope in MAINTENANCE_SCOPES {
        for inner in [false, true] {
            let mut op = scope_setup(scope);
            op.paired_enabled = true;
            let request = op
                .session
                .clone()
                .begin_maintenance(&atomic_identity(&op), 500)
                .unwrap();
            let sent = atomic_peer(
                &mut op,
                vec![request.encode().unwrap()],
                vec![Some(maintenance_reply(&request, None))],
                false,
                None,
            );
            let before = op.session.snapshot.clone();
            op.start = Instant::now() - Duration::from_millis(500);
            if inner {
                op.mutate_inner("renew", json!({}))
            } else {
                op.mutate("renew", json!({}))
            }
            .unwrap();
            assert_eq!(sent.lock().unwrap().len(), 1, "{scope}:{inner}");
            assert_eq!(op.session.snapshot, before);
            assert!(!op.session.fresh(op.now()));
        }
    }
}
#[test]
fn atomic_all_scopes_precommand_renewal_uses_maintenance_between_command_readbacks() {
    for scope in MAINTENANCE_SCOPES {
        let mut op = scope_setup(scope);
        op.paired_enabled = true;
        let identity = atomic_identity(&op);
        let mut model = op.session.clone();
        let mut expected = Vec::new();
        let mut responses = Vec::new();
        for nonce in 1..=2 {
            let request = model.paired_request(&identity, nonce).unwrap();
            let mut raw = model.snapshot.clone().unwrap();
            raw.frame = (nonce * 48).to_string();
            raw.authority.revision = "999".into();
            let (reference, _, _) = setup();
            let mut brain = reference.session.brain.unwrap();
            brain.frame = raw.frame.clone();
            brain.revision = raw.authority.revision.clone();
            let reply = serde_json::to_vec(&json!({"contract":"GP15-paired-readback", "version":1,
                "state":"snapshot", "reason":null, "context":request, "raw":raw, "brain":brain}))
            .unwrap();
            model
                .accept_paired(
                    crate::paired_readback::Reply::decode(&reply).unwrap(),
                    &request,
                    500,
                    model.generation(),
                    500,
                )
                .unwrap();
            expected.push(request.encode().unwrap());
            responses.push(Some(reply));
            if nonce == 1 {
                let request = model.begin_maintenance(&identity, 500).unwrap();
                let reply = maintenance_reply(&request, None);
                model
                    .accept_maintenance(
                        crate::lease_maintenance::Reply::decode(&reply).unwrap(),
                        500,
                    )
                    .unwrap();
                expected.push(request.encode().unwrap());
                responses.push(Some(reply));
            }
        }
        let request = model.begin("release", json!({}), 500).unwrap();
        let mut reply: Value = serde_json::from_slice(include_bytes!(
            "../../tests/fixtures/gp15/v1/grant-16-talkback-reply.json"
        ))
        .unwrap();
        let context = serde_json::to_value(&request.context).unwrap();
        reply["context"] = context.clone();
        for (k, v) in context.as_object().unwrap() {
            reply["outcome"][k] = v.clone();
        }
        reply["outcome"]["body"]["revision"] = json!("999");
        for k in ["scope", "granted_lease", "lease_remaining_ms"] {
            reply["outcome"]["body"][k] = Value::Null;
        }
        expected.push(request.encode().unwrap());
        responses.push(Some(serde_json::to_vec(&reply).unwrap()));
        // Release retires authority and requires its ordinary final readback.
        let request = model.paired_request(&identity, 3).unwrap();
        let mut final_pair: Value = serde_json::from_slice(responses[2].as_ref().unwrap()).unwrap();
        final_pair["context"] = serde_json::to_value(&request).unwrap();
        final_pair["raw"]["frame"] = json!("144");
        final_pair["brain"]["frame"] = json!("144");
        expected.push(request.encode().unwrap());
        responses.push(Some(serde_json::to_vec(&final_pair).unwrap()));
        let sent = atomic_peer(&mut op, expected, responses, false, None);
        op.start = Instant::now() - Duration::from_millis(500);
        op.mutate("release", json!({})).unwrap();
        assert_eq!(
            sent.lock().unwrap().len(),
            5,
            "three command readbacks, one maintenance, one release: {scope}"
        );
    }
}
#[test]
fn atomic_all_scopes_refuse_cancel_and_unsupported_without_fallback() {
    for scope in MAINTENANCE_SCOPES {
        for mode in [
            "permission",
            "scope",
            "lease",
            "identity",
            "version",
            "cancel",
        ] {
            for inner in [false, true] {
                let mut op = scope_setup(scope);
                op.paired_enabled = true;
                let identity = atomic_identity(&op);
                let request = op.session.clone().begin_maintenance(&identity, 0).unwrap();
                let guard = Arc::new(std::sync::atomic::AtomicU64::new(1));
                op.guard(guard.clone(), 1);
                let response = if mode == "version" {
                    let mut value: Value =
                        serde_json::from_slice(&maintenance_reply(&request, None)).unwrap();
                    value["version"] = json!(2);
                    serde_json::to_vec(&value).unwrap()
                } else {
                    maintenance_reply(&request, (mode != "cancel").then_some(mode))
                };
                let sent = atomic_peer(
                    &mut op,
                    vec![request.encode().unwrap()],
                    vec![Some(response)],
                    false,
                    (mode == "cancel").then_some(guard),
                );
                assert!(
                    if inner {
                        op.mutate_inner("renew", json!({}))
                    } else {
                        op.mutate("renew", json!({}))
                    }
                    .is_err(),
                    "{scope}:{mode}:{inner}"
                );
                assert_eq!(sent.lock().unwrap().len(), 1);
                assert_eq!(op.session.lease_deadline(), None);
                assert!(op.session.begin_maintenance(&identity, op.now()).is_err());
            }
        }
    }
}
#[test]
fn atomic_generic_identity_preserves_legacy_revision_renewal() {
    for scope in &MAINTENANCE_SCOPES[..7] {
        for inner in [false, true] {
            let mut op = scope_setup(scope);
            let request = op.session.clone().begin("renew", json!({}), 0).unwrap();
            let mut reply: Value = serde_json::from_slice(include_bytes!(
                "../../tests/fixtures/gp15/v1/grant-16-talkback-reply.json"
            ))
            .unwrap();
            let context = serde_json::to_value(&request.context).unwrap();
            reply["context"] = context.clone();
            for (k, v) in context.as_object().unwrap() {
                reply["outcome"][k] = v.clone();
            }
            reply["outcome"]["kind"] = json!("conflict");
            reply["outcome"]["body"]["reason"] = json!("stale_revision");
            reply["outcome"]["body"]["revision"] = json!("999");
            for k in ["scope", "granted_lease", "lease_remaining_ms"] {
                reply["outcome"]["body"][k] = Value::Null;
            }
            let sent = atomic_peer(
                &mut op,
                vec![request.encode().unwrap()],
                vec![Some(serde_json::to_vec(&reply).unwrap())],
                false,
                None,
            );
            assert!(op.transport.held_identity().is_some());
            assert!(!op.uses_atomic_maintenance());
            let error = if inner {
                op.mutate_inner("renew", json!({}))
            } else {
                op.mutate("renew", json!({}))
            }
            .unwrap_err();
            assert!(error.contains("stale_revision"), "{scope}:{inner}:{error}");
            assert_eq!(sent.lock().unwrap().len(), 1);
        }
    }
}
#[test]
fn atomic_failed_explicit_opt_in_never_falls_back_and_new_attachment_resets_mode() {
    let mut op = scope_setup("pa_configuration");
    let request = op.session.paired_request(&atomic_identity(&op), 1).unwrap();
    let sent = atomic_peer(
        &mut op,
        vec![request.encode().unwrap()],
        vec![Some(
            br#"{"contract":"GP15-paired-readback","version":2}"#.to_vec(),
        )],
        false,
        None,
    );
    assert!(
        op.refresh_brain_until(Instant::now() + Duration::from_millis(250))
            .is_err()
    );
    assert!(op.paired_enabled);
    assert!(op.mutate("renew", json!({})).is_err());
    assert!(op.mutate_inner("renew", json!({})).is_err());
    assert_eq!(sent.lock().unwrap().len(), 1);
    op.session.disconnect();
    assert!(op.mutate_inner("renew", json!({})).is_err());
    let replacement = scope_setup("pa_configuration");
    assert!(
        !replacement.paired_enabled,
        "a new attachment must opt in explicitly"
    );
    assert!(!replacement.uses_atomic_maintenance());
}
#[test]
fn atomic_maintenance_retries_exact_bytes_and_anchors_original_send_without_topology() {
    let (mut op, _, _) = setup();
    let identity = atomic_identity(&op);
    let request = op
        .session
        .clone()
        .begin_maintenance(&identity, 500)
        .unwrap();
    let bytes = request.encode().unwrap();
    let sent = atomic_peer(
        &mut op,
        vec![bytes.clone(), bytes],
        vec![None, Some(maintenance_reply(&request, None))],
        false,
        None,
    );
    let raw = op.session.snapshot.clone();
    op.start = Instant::now() - Duration::from_millis(500);
    let first = op.now();
    op.mutate("renew", json!({})).unwrap();
    assert_eq!(sent.lock().unwrap().len(), 2);
    assert!(op.now() >= first + 100);
    assert!(op.session.lease_deadline().unwrap() <= first + 2002);
    let confirmed = op.session.lease_deadline();
    assert!(
        op.processing_frame(&maintenance_reply(&request, None))
            .unwrap()
    );
    assert_eq!(
        op.session.lease_deadline(),
        confirmed,
        "duplicate reply cannot renew twice"
    );
    let mut changed: Value = serde_json::from_slice(&maintenance_reply(&request, None)).unwrap();
    changed["result"]["revision"] = json!("1000");
    assert!(
        op.processing_frame(&serde_json::to_vec(&changed).unwrap())
            .is_err()
    );
    assert_eq!(op.session.snapshot, raw);
    assert!(!op.session.brain_fresh(op.now()));
}
#[test]
fn atomic_busy_allows_new_id_but_unknown_partial_and_canceled_never_reissue() {
    let (mut op, _, _) = setup();
    let identity = atomic_identity(&op);
    let mut model = op.session.clone();
    let first = model.begin_maintenance(&identity, 0).unwrap();
    let busy = maintenance_reply(&first, Some("unavailable"));
    model
        .accept_maintenance(crate::lease_maintenance::Reply::decode(&busy).unwrap(), 1)
        .unwrap();
    let second = model.begin_maintenance(&identity, 1).unwrap();
    let sent = atomic_peer(
        &mut op,
        vec![first.encode().unwrap(), second.encode().unwrap()],
        vec![Some(busy), Some(maintenance_reply(&second, None))],
        false,
        None,
    );
    op.mutate_inner("renew", json!({})).unwrap();
    assert_eq!(sent.lock().unwrap().len(), 2);
    for mode in ["unknown", "partial", "cancel", "unsupported"] {
        let (mut op, _, _) = setup();
        let request = op.session.clone().begin_maintenance(&identity, 0).unwrap();
        let guard = Arc::new(std::sync::atomic::AtomicU64::new(1));
        op.guard(guard.clone(), 1);
        let response = if mode == "unknown" {
            None
        } else if mode == "unsupported" {
            let mut value: Value =
                serde_json::from_slice(&maintenance_reply(&request, None)).unwrap();
            value["version"] = json!(2);
            Some(serde_json::to_vec(&value).unwrap())
        } else {
            Some(maintenance_reply(&request, None))
        };
        let sent = atomic_peer(
            &mut op,
            vec![request.encode().unwrap()],
            vec![response],
            mode == "partial",
            (mode == "cancel").then_some(guard),
        );
        assert!(
            op.maintain(Instant::now() + Duration::from_millis(20), &mut 64, false)
                .is_err(),
            "{mode}"
        );
        assert!(op.session.lease_deadline().is_none(), "{mode}");
        assert!(op.session.begin_maintenance(&identity, op.now()).is_err());
        assert_eq!(sent.lock().unwrap().len(), 1);
        assert!(
            op.session
                .accept_maintenance(
                    crate::lease_maintenance::Reply::decode(&maintenance_reply(&request, None))
                        .unwrap(),
                    op.now()
                )
                .is_err()
        );
    }
}
#[test]
fn atomic_paired_install_is_correlated_transactional_and_uses_query_age() {
    for mode in ["valid", "half", "nonce", "version", "cancel", "budget"] {
        let (mut op, _, _) = setup();
        let identity = atomic_identity(&op);
        let request = op.session.paired_request(&identity, 1).unwrap();
        let mut raw = op.session.snapshot.clone().unwrap();
        raw.frame = "48".into();
        let mut brain = op.session.brain.clone().unwrap();
        brain.frame = "48".into();
        let mut value = json!({"contract":"GP15-paired-readback","version":1,"state":"snapshot","reason":null,"context":request,"raw":raw,"brain":brain});
        match mode {
            "half" => {
                value["brain"]["held_generation"] = json!("0");
            }
            "nonce" => {
                value["context"]["query_id"] = json!("2");
            }
            "version" => {
                value["version"] = json!(2);
            }
            _ => (),
        }
        let guard = Arc::new(std::sync::atomic::AtomicU64::new(1));
        op.guard(guard.clone(), 1);
        atomic_peer(
            &mut op,
            vec![request.encode().unwrap()],
            vec![Some(serde_json::to_vec(&value).unwrap())],
            false,
            (mode == "cancel").then_some(guard),
        );
        let old = op.session.snapshot.clone();
        let age = op.session.brain_age(op.now());
        let result = op.read_paired(
            Instant::now() + Duration::from_millis(250),
            &mut if mode == "budget" { 0 } else { 64 },
        );
        if mode == "valid" {
            result.map_err(BrainOperationError::message).unwrap();
            assert_eq!(op.session.snapshot.as_ref().unwrap().frame, "48");
            assert_eq!(op.session.brain.as_ref().unwrap().frame, "48");
        } else {
            assert!(result.is_err(), "{mode}");
            assert_eq!(op.session.snapshot, old);
            assert!(op.session.brain_age(op.now()).unwrap() >= age.unwrap());
        }
    }
}
struct RenewBackpressure {
    sent: Arc<Mutex<Vec<Vec<u8>>>>,
    replies: VecDeque<Vec<u8>>,
    started: Instant,
}
impl AuthorityConnection for RenewBackpressure {
    fn send_frame_until(&mut self, bytes: &[u8], _: Instant) -> Result<(), String> {
        let mut sent = self.sent.lock().unwrap();
        let request: Value = serde_json::from_slice(bytes).unwrap();
        assert_eq!(request["kind"], "renew");
        let context: serde_json::Map<String, Value> = [
            "show_id",
            "module",
            "epoch",
            "writer",
            "lease",
            "request_id",
            "expected_revision",
        ]
        .into_iter()
        .map(|k| (k.into(), request[k].clone()))
        .collect();
        let mut reply: Value = serde_json::from_slice(include_bytes!(
            "../../tests/fixtures/gp15/v1/grant-16-talkback-reply.json"
        ))
        .unwrap();
        reply["context"] = Value::Object(context.clone());
        if sent.is_empty() {
            reply["state"] = json!("backpressure");
            reply["outcome"] = Value::Null;
        } else {
            assert_eq!(sent.len(), 1);
            assert_eq!(bytes, sent[0], "retry must preserve every request byte");
            assert!(
                self.started.elapsed() < Duration::from_millis(500),
                "retry missed its existing100ms wake"
            );
            for (k, v) in context {
                reply["outcome"][k] = v;
            }
            reply["outcome"]["body"]["granted_lease"] = Value::Null;
        }
        reply["snapshot"] = Value::Null;
        sent.push(bytes.to_vec());
        self.replies.push_back(serde_json::to_vec(&reply).unwrap());
        Ok(())
    }
    fn receive_until(&mut self, deadline: Instant) -> Result<Option<Vec<u8>>, String> {
        if let Some(reply) = self.replies.pop_front() {
            return Ok(Some(reply));
        }
        std::thread::sleep(deadline.saturating_duration_since(Instant::now()));
        Ok(None)
    }
    fn receive_available(&mut self) -> Result<Option<Vec<u8>>, String> {
        Ok(self.replies.pop_front())
    }
}
#[test]
fn renew_backpressure_silence_wakes_identical_retry_before_lease_expiry() {
    let (mut op, _, raw) = setup();
    op.session.brain = None; // This test isolates ordinary C-AUDIO retry/framing.
    op.start = Instant::now() - Duration::from_millis(500);
    let revision = op
        .session
        .snapshot
        .as_ref()
        .unwrap()
        .authority
        .revision
        .clone();
    let fresh = audio::decode_reply(&serde_json::to_vec(&raw_at(&raw, &revision, "48")).unwrap())
        .unwrap()
        .snapshot
        .unwrap();
    op.session.ingest_snapshot(fresh, op.now()).unwrap();
    let original_expiry = op.session.lease_deadline().unwrap();
    let sent = Arc::new(Mutex::new(Vec::new()));
    op.transport = Box::new(RenewBackpressure {
        sent: sent.clone(),
        replies: VecDeque::new(),
        started: Instant::now(),
    });
    op.mutate_ordinary_inner(
        "renew",
        json!({}),
        Instant::now() + Duration::from_millis(1900),
    )
    .unwrap();
    assert_eq!(sent.lock().unwrap().len(), 2);
    assert!(op.session.pending.is_none());
    assert!(op.session.lease_deadline().unwrap() >= original_expiry + 500);
    assert!(op.now() < original_expiry);
}
#[test]
fn renew_partial_prefix_timeout_is_fatal_without_request_retry() {
    let (mut op, _, _) = setup();
    op.session.brain = None; // This test isolates ordinary C-AUDIO retry/framing.
    let (socket, mut server) = UnixStream::pair().unwrap();
    let child = std::thread::spawn(move || {
        let mut prefix = [0; 4];
        server.read_exact(&mut prefix).unwrap();
        let mut body = vec![0; u32::from_be_bytes(prefix) as usize];
        server.read_exact(&mut body).unwrap();
        server.write_all(&[0]).unwrap();
        std::thread::sleep(Duration::from_millis(200));
        server
            .set_read_timeout(Some(Duration::from_millis(30)))
            .unwrap();
        let mut byte = [0];
        let result = server.read(&mut byte);
        assert!(
            matches!(result, Err(ref e) if matches!(e.kind(), std::io::ErrorKind::WouldBlock | std::io::ErrorKind::TimedOut)),
            "partial reply must not trigger retransmission: {result:?}"
        );
    });
    op.transport = Box::new(Transport::from_stream(socket));
    op.start = Instant::now();
    assert!(
        op.mutate_ordinary_inner(
            "renew",
            json!({}),
            Instant::now() + Duration::from_millis(1900)
        )
        .is_err()
    );
    assert!(op.session.lease_deadline().is_none());
    assert!(op.session.pending.is_none());
    child.join().unwrap();
}
fn probe_reply(op: &Operator, frame: u64) -> Value {
    let mut snapshot = op.session.brain.clone().unwrap();
    snapshot.frame = frame.to_string();
    json!({"contract":"GP15-brain","version":1,"state":"snapshot","reason":null,
        "context":op.session.brain_request().context,"revision":snapshot.revision,
        "applied_frame":null,"snapshot":snapshot})
}
struct ProbeConnection {
    peer: FakeAuthorityConnection,
    sent: Arc<Mutex<Vec<Value>>>,
    fail: bool,
    first_delay: Option<Duration>,
    deadlines: Option<Arc<Mutex<Vec<Instant>>>>,
}
impl AuthorityConnection for ProbeConnection {
    fn send_frame_until(&mut self, bytes: &[u8], deadline: Instant) -> Result<(), String> {
        if let Some(deadlines) = &self.deadlines {
            deadlines.lock().unwrap().push(deadline);
        }
        self.sent
            .lock()
            .unwrap()
            .push(serde_json::from_slice(bytes).unwrap());
        if self.fail {
            Err("partial probe send".into())
        } else {
            Ok(())
        }
    }
    fn receive_until(&mut self, deadline: Instant) -> Result<Option<Vec<u8>>, String> {
        if let Some(deadlines) = &self.deadlines {
            deadlines.lock().unwrap().push(deadline);
        }
        if let Some(delay) = self.first_delay.take() {
            std::thread::sleep(delay);
        }
        self.peer.receive_available()
    }
    fn receive_available(&mut self) -> Result<Option<Vec<u8>>, String> {
        self.peer.receive_available()
    }
}
fn probe_connection(
    op: &mut Operator,
    peer: &FakeAuthorityConnection,
    fail: bool,
) -> Arc<Mutex<Vec<Value>>> {
    let sent = Arc::new(Mutex::new(Vec::new()));
    op.transport = Box::new(ProbeConnection {
        peer: peer.clone(),
        sent: sent.clone(),
        fail,
        first_delay: None,
        deadlines: None,
    });
    sent
}
#[test]
fn brain_probe_waits_for_own_reply_not_buffered_progress() {
    let (mut op, peer, _) = setup();
    probe_connection(&mut op, &peer, false);
    let base = op
        .session
        .brain
        .as_ref()
        .unwrap()
        .frame
        .parse::<u64>()
        .unwrap();
    let old = op
        .send_brain_probe(Instant::now() + Duration::from_millis(50))
        .unwrap();
    peer.push(probe_reply(&op, base + 48));
    peer.push(probe_reply(&op, base + 96));
    op.refresh_brain().unwrap();
    assert_eq!(op.brain_probes.matched.as_ref().unwrap().0, old + 1);
    assert_eq!(
        op.brain_probes.matched.as_ref().unwrap().2.frame,
        (base + 96).to_string()
    );
    assert!(op.brain_probes.pending.is_empty());
    assert!(peer.0.lock().unwrap().is_empty());
}
#[test]
fn equal_snapshot_consumes_probe_but_final_never_does() {
    let (mut op, peer, _) = setup();
    probe_connection(&mut op, &peer, false);
    let id = op
        .send_brain_probe(Instant::now() + Duration::from_millis(50))
        .unwrap();
    let frame = op
        .session
        .brain
        .as_ref()
        .unwrap()
        .frame
        .parse::<u64>()
        .unwrap();
    op.processing_frame(&serde_json::to_vec(&probe_reply(&op, frame)).unwrap())
        .unwrap();
    assert_eq!(op.brain_probes.matched.as_ref().unwrap().0, id);
    assert!(op.brain_probes.pending.is_empty());
    op.send_brain_probe(Instant::now() + Duration::from_millis(50))
        .unwrap();
    let r = op
        .session
        .begin("brain_hold", json!({"generation":"1"}), op.now())
        .unwrap();
    let (pending, final_reply) = replies(&op, &r, true);
    for reply in [pending, final_reply] {
        op.processing_frame(&serde_json::to_vec(&reply).unwrap())
            .unwrap();
    }
    assert_eq!(op.brain_probes.pending.len(), 1);
    assert_eq!(op.brain_probes.matched.as_ref().unwrap().0, id);
}
#[test]
fn brain_probe_requests_expired_same_revision_raw_before_admitting_pair() {
    let (mut op, peer, raw) = setup();
    let sent = Arc::new(Mutex::new(Vec::new()));
    op.transport = Box::new(ProbeConnection {
        peer: peer.clone(),
        sent: sent.clone(),
        fail: false,
        first_delay: Some(Duration::from_millis(10)),
        deadlines: None,
    });
    // Raw starts fresh, then expires while the Brain reply is in flight.
    op.start = Instant::now() - Duration::from_millis(245);
    assert!(op.session.fresh(op.now()));
    let frame = op
        .session
        .brain
        .as_ref()
        .unwrap()
        .frame
        .parse::<u64>()
        .unwrap()
        + 48;
    peer.push(probe_reply(&op, frame));
    peer.push(raw_at(
        &raw,
        &op.session.brain.as_ref().unwrap().revision,
        &frame.to_string(),
    ));
    op.refresh_brain().unwrap();
    assert!(op.session.brain_fresh(op.now()));
    let sent = sent.lock().unwrap();
    assert_eq!(sent.len(), 2);
    assert_eq!(sent[0]["kind"], "brain_snapshot");
    assert_eq!(sent[1]["kind"], "snapshot");
}
#[test]
fn superseded_brain_pair_reprobes_without_replaying_mutation() {
    let (mut op, peer, raw) = setup();
    let sent = probe_connection(&mut op, &peer, false);
    op.start = Instant::now() - Duration::from_millis(251);
    let frame = op
        .session
        .brain
        .as_ref()
        .unwrap()
        .frame
        .parse::<u64>()
        .unwrap()
        + 48;
    let old = probe_reply(&op, frame);
    let revision = op
        .session
        .brain
        .as_ref()
        .unwrap()
        .revision
        .parse::<u64>()
        .unwrap()
        + 1;
    let mut advanced = old.clone();
    advanced["revision"] = json!(revision.to_string());
    advanced["snapshot"]["revision"] = json!(revision.to_string());
    advanced["snapshot"]["frame"] = json!((frame + 48).to_string());
    peer.push(old);
    peer.push(raw_at(
        &raw,
        &revision.to_string(),
        &(frame + 48).to_string(),
    ));
    peer.push(advanced);
    op.refresh_brain().unwrap();
    assert!(op.session.brain_fresh(op.now()));
    assert_eq!(op.brain_probes.matched.as_ref().unwrap().0, 2);
    assert_eq!(
        op.brain_probes.matched.as_ref().unwrap().2.revision,
        revision.to_string()
    );
    let sent = sent.lock().unwrap();
    let kinds: Vec<_> = sent.iter().map(|r| r["kind"].as_str().unwrap()).collect();
    assert_eq!(kinds, ["brain_snapshot", "snapshot", "brain_snapshot"]);
    assert!(sent.iter().all(|r| r["writer"].is_null()));
}
#[test]
fn continual_revision_churn_keeps_original_probe_deadline_and_frame_bound() {
    let (mut op, peer, raw) = setup();
    let sent = Arc::new(Mutex::new(Vec::new()));
    let deadlines = Arc::new(Mutex::new(Vec::new()));
    op.transport = Box::new(ProbeConnection {
        peer: peer.clone(),
        sent: sent.clone(),
        fail: false,
        first_delay: None,
        deadlines: Some(deadlines.clone()),
    });
    op.start = Instant::now() - Duration::from_millis(251);
    let frame = op
        .session
        .brain
        .as_ref()
        .unwrap()
        .frame
        .parse::<u64>()
        .unwrap()
        + 48;
    let base = op
        .session
        .brain
        .as_ref()
        .unwrap()
        .revision
        .parse::<u64>()
        .unwrap();
    let initial = probe_reply(&op, frame);
    peer.push(initial.clone());
    peer.push(raw_at(
        &raw,
        &(base + 1).to_string(),
        &(frame + 48).to_string(),
    ));
    for i in 1..34 {
        // The next raw revision overtakes each pending Brain response.
        peer.push(raw_at(
            &raw,
            &(base + i + 1).to_string(),
            &(frame + (i + 1) * 48).to_string(),
        ));
        let mut brain = initial.clone();
        brain["revision"] = json!((base + i).to_string());
        brain["snapshot"]["revision"] = json!((base + i).to_string());
        brain["snapshot"]["frame"] = json!((frame + i * 48).to_string());
        peer.push(brain);
    }
    // Isolate the frame bound from debug decode throughput. Production
    // entrypoints still supply250ms/30ms; their timeout tests remain separate.
    let original_deadline = Instant::now() + Duration::from_secs(10);
    let fault = op.refresh_brain_until(original_deadline).unwrap_err();
    assert!(fault.starts_with("Brain paired observation deadline:"));
    assert!(fault.contains("processed=64 loops=64"));
    for field in [
        "elapsed_ms=",
        "budget_ms=",
        "probe=",
        "probe_age_ms=",
        "probe_generation=",
        "matched_id=",
        "matched_revision=",
        "matched_generation=",
        "raw_revision=",
        "raw_age_ms=",
        "raw_fresh=",
        "brain_revision=",
        "brain_age_ms=",
        "brain_fresh=",
        "requested_revision=",
        "pending=",
    ] {
        assert!(fault.contains(field), "missing {field}: {fault}");
    }
    assert!(fault.len() < 1024, "diagnostic remains scalar and bounded");
    assert_eq!(op.brain_probes.first_fault.as_deref(), Some(fault.as_str()));
    assert_eq!(
        op.refresh_brain().unwrap_err(),
        fault,
        "first failure retained"
    );
    assert!(op.brain_probes.invalid);
    assert_eq!(peer.0.lock().unwrap().len(), 4, "exactly64 frames consumed");
    let deadlines = deadlines.lock().unwrap();
    assert!(!deadlines.is_empty());
    assert!(deadlines.iter().all(|d| *d == original_deadline));
    assert!(sent.lock().unwrap().iter().all(|r| r["writer"].is_null()));
}
#[test]
fn paired_trace_is_opt_in_bounded_and_reports_unavailable_transport() {
    for enabled in [false, true] {
        let (mut op, peer, _) = setup();
        probe_connection(&mut op, &peer, false);
        op.trace_timing = enabled;
        let error = op
            .refresh_brain_budget(Instant::now() + Duration::from_millis(100), &mut 0, false)
            .unwrap_err()
            .message();
        assert_eq!(error.contains("timing_totals_us="), enabled);
        if enabled {
            for field in [
                "transport_before=None",
                "transport_after=None",
                "overwritten=0",
                "overflow=false",
                "tail3=",
            ] {
                assert!(error.contains(field), "missing {field}");
            }
        }
        assert!(error.len() < 8000);
    }
}
#[test]
fn probe_partial_send_unmatched_reply_and_overflow_poison_provenance() {
    let (mut op, peer, _) = setup();
    probe_connection(&mut op, &peer, true);
    assert!(
        op.send_brain_probe(Instant::now() + Duration::from_millis(50))
            .is_err()
    );
    assert!(op.brain_probes.invalid);
    assert!(op.brain_probes.pending.is_empty());
    let (mut op, peer, _) = setup();
    probe_connection(&mut op, &peer, false);
    let frame = op
        .session
        .brain
        .as_ref()
        .unwrap()
        .frame
        .parse::<u64>()
        .unwrap();
    assert!(
        op.processing_frame(&serde_json::to_vec(&probe_reply(&op, frame)).unwrap())
            .is_err()
    );
    assert!(op.brain_probes.invalid);
    let (mut op, peer, _) = setup();
    probe_connection(&mut op, &peer, false);
    for _ in 0..64 {
        op.send_brain_probe(Instant::now() + Duration::from_millis(50))
            .unwrap();
    }
    assert!(
        op.send_brain_probe(Instant::now() + Duration::from_millis(50))
            .is_err()
    );
    assert!(op.brain_probes.invalid);
    let (mut op, peer, _) = setup();
    probe_connection(&mut op, &peer, false);
    op.send_brain_probe(Instant::now() + Duration::from_millis(50))
        .unwrap();
    let frame = op
        .session
        .brain
        .as_ref()
        .unwrap()
        .frame
        .parse::<u64>()
        .unwrap();
    op.cancel();
    assert!(!op.brain_probes.invalid);
    assert_eq!(op.brain_probes.pending.len(), 1);
    op.processing_frame(&serde_json::to_vec(&probe_reply(&op, frame)).unwrap())
        .unwrap();
    assert!(op.brain_probes.matched.is_none());
    assert!(op.brain_probes.pending.is_empty());
    op.send_brain_probe(Instant::now() + Duration::from_millis(50))
        .unwrap();
    op.processing_frame(&serde_json::to_vec(&probe_reply(&op, frame)).unwrap())
        .unwrap();
    assert!(op.brain_probes.matched.is_some());
}
