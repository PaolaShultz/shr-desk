use super::*;
use std::{
    collections::VecDeque,
    sync::{
        Arc, Mutex,
        atomic::{AtomicU64, Ordering},
    },
};
#[derive(Clone, Copy)]
enum Readback {
    Fresh,
    Duplicate,
    Absent,
    Equal,
    Identity,
    Saturated,
    Guard,
    Transport,
    Protocol,
}
#[derive(Default)]
struct Calls {
    sends: Vec<(String, Instant)>,
    reads: Vec<Instant>,
    drained: usize,
    readback_started: Option<Instant>,
}
struct Transcript {
    corpus: Value,
    queue: VecDeque<Vec<u8>>,
    calls: Arc<Mutex<Calls>>,
    mode: Readback,
    guard: Arc<AtomicU64>,
    queried: bool,
}
impl Transcript {
    fn push(&mut self, value: Value) {
        self.queue.push_back(serde_json::to_vec(&value).unwrap());
    }
    fn raw(&self, fresh: bool) -> Value {
        let mut r = self.corpus["grant_response"].clone();
        for k in ["writer", "lease", "request_id", "expected_revision"] {
            r["context"][k] = Value::Null;
            r["outcome"][k] = Value::Null;
        }
        for k in ["granted_lease", "lease_remaining_ms", "scope"] {
            r["outcome"]["body"][k] = Value::Null;
        }
        r["snapshot"] = self.corpus[if fresh { "final" } else { "initial" }].clone();
        r["outcome"]["body"]["revision"] = r["snapshot"]["authority"]["revision"].clone();
        if !fresh {
            r["snapshot"]["authority"]["sequence"] = json!("0");
            r["snapshot"]["frame"] = json!("47952");
        }
        r
    }
}
impl AuthorityConnection for Transcript {
    fn send_frame_until(&mut self, bytes: &[u8], deadline: Instant) -> Result<(), String> {
        let request: Value = serde_json::from_slice(bytes).unwrap();
        let kind = request["kind"].as_str().unwrap();
        self.calls
            .lock()
            .unwrap()
            .sends
            .push((kind.into(), deadline));
        if kind == "set" {
            let mut final_reply = Value::Null;
            for mut r in [
                self.corpus["pending"].clone(),
                self.corpus["committed"][0].clone(),
            ] {
                for k in ["writer", "lease", "request_id", "expected_revision"] {
                    r["context"][k] = request[k].clone();
                    if !r["outcome"].is_null() {
                        r["outcome"][k] = request[k].clone();
                    }
                }
                final_reply = r.clone();
                self.push(r);
            }
            if matches!(self.mode, Readback::Duplicate) {
                self.push(final_reply);
            }
            if matches!(self.mode, Readback::Saturated) {
                let stale = serde_json::to_vec(&self.raw(false)).unwrap();
                self.queue.extend((0..64).map(|_| stale.clone()));
            } else {
                self.push(self.raw(false));
            }
        } else {
            assert_eq!(kind, "snapshot", "required readback never replays mutation");
            self.queried = true;
            match self.mode {
                Readback::Absent | Readback::Transport | Readback::Protocol => {}
                Readback::Equal => {
                    let mut raw = self.raw(true);
                    raw["snapshot"] = self.corpus["committed"][0]["snapshot"].clone();
                    self.push(raw);
                }
                Readback::Identity => {
                    let mut raw = self.raw(true);
                    raw["context"]["epoch"] = json!("10");
                    raw["outcome"]["epoch"] = json!("10");
                    raw["snapshot"]["authority"]["epoch"] = json!("10");
                    self.push(raw);
                }
                _ => self.push(self.raw(true)),
            }
        }
        Ok(())
    }
    fn receive_available(&mut self) -> Result<Option<Vec<u8>>, String> {
        Ok(self.queue.pop_front())
    }
    fn receive_available_until(&mut self, deadline: Instant) -> Result<Option<Vec<u8>>, String> {
        let mut calls = self.calls.lock().unwrap();
        calls.reads.push(deadline);
        calls.readback_started.get_or_insert_with(Instant::now);
        drop(calls);
        let result = self.queue.pop_front();
        if result.is_some() {
            self.calls.lock().unwrap().drained += 1;
        }
        // Revocation after the final guard but before fresh raw admission.
        if self.queried && result.is_none() && matches!(self.mode, Readback::Guard) {
            self.guard.store(1, Ordering::Release);
        }
        Ok(result)
    }
    fn receive_until(&mut self, deadline: Instant) -> Result<Option<Vec<u8>>, String> {
        self.calls.lock().unwrap().reads.push(deadline);
        if self.queried && matches!(self.mode, Readback::Transport) {
            return Err("test transport closed".into());
        }
        if self.queried && matches!(self.mode, Readback::Protocol) {
            return Ok(Some(b"invalid protocol".to_vec()));
        }
        if let Some(bytes) = self.queue.pop_front() {
            return Ok(Some(bytes));
        }
        std::thread::sleep(deadline.saturating_duration_since(Instant::now()));
        Ok(None)
    }
}
fn setup(mode: Readback) -> (Operator, Arc<Mutex<Calls>>) {
    let corpus: Value = serde_json::from_slice(include_bytes!(
        "../../tests/fixtures/gp03/v1/e03-rendered.json"
    ))
    .unwrap();
    let calls = Arc::new(Mutex::new(Calls::default()));
    let guard = Arc::new(AtomicU64::new(0));
    let transport = Transcript {
        corpus: corpus.clone(),
        queue: VecDeque::new(),
        calls: calls.clone(),
        mode,
        guard: guard.clone(),
        queried: false,
    };
    let mut op = Operator::from_document_connection(
        Box::new(transport),
        "11111111-1111-4111-8111-111111111111",
        9,
        "desk-corpus",
        "foh",
        1,
    )
    .unwrap();
    let raw = audio::decode_snapshot(&serde_json::to_vec(&corpus["initial"]).unwrap()).unwrap();
    op.session.ingest_snapshot(raw.clone(), 0).unwrap();
    op.session
        .begin("grant", json!({"scope":"foh"}), 0)
        .unwrap();
    op.session
        .accept(
            audio::decode_reply(&serde_json::to_vec(&corpus["grant_response"]).unwrap()).unwrap(),
            0,
        )
        .unwrap();
    op.session.ingest_snapshot(raw, 0).unwrap();
    op.session.input_released();
    op.guard(guard, 0);
    (op, calls)
}
fn mutate(op: &mut Operator) -> Result<(), String> {
    op.mutate_inner(
        "set",
        json!({"targets":[{"target":{"parameter":"fader","input":"input-01"},"value":-3000}]}),
    )
}
#[test]
fn applied_final_with_stale_fifo_requests_fresh_readback_without_replay() {
    for mode in [Readback::Fresh, Readback::Duplicate] {
        let (mut op, calls) = setup(mode);
        mutate(&mut op).unwrap();
        let calls = calls.lock().unwrap();
        assert_eq!(
            calls.sends.iter().map(|s| s.0.as_str()).collect::<Vec<_>>(),
            ["set", "snapshot"]
        );
        let deadline = calls.sends[1].1;
        assert!(calls.reads.iter().rev().take(3).all(|d| *d == deadline));
        assert!(op.session.pending.is_none());
        assert_eq!(op.session.last_result, "set applied revision 13");
        assert!(op.session.fresh(op.now()));
        assert_eq!(
            op.session.snapshot.as_ref().unwrap().authority.sequence,
            "3"
        );
    }
}
#[test]
fn absent_or_nonadvancing_readback_preserves_completion_but_fails_closed() {
    for mode in [Readback::Absent, Readback::Equal] {
        let (mut op, calls) = setup(mode);
        let error = mutate(&mut op).unwrap_err();
        assert!(error.starts_with("correlated completion: set applied revision 13;"));
        assert!(!op.session.fresh(op.now()));
        assert!(op.session.snapshot_age(op.now()).is_none());
        assert!(op.session.pending.is_none());
        assert!(
            mutate(&mut op).is_err(),
            "failed observation cannot authorize another edit"
        );
        let calls = calls.lock().unwrap();
        assert_eq!(calls.sends.len(), 2);
        let deadline = calls.sends[1].1;
        assert!(deadline <= calls.readback_started.unwrap() + Duration::from_millis(250));
        assert!(Instant::now() >= deadline);
        assert!(calls.reads.iter().skip(2).all(|d| *d == deadline));
    }
}
#[test]
fn identity_protocol_transport_and_saturation_fail_closed_without_replay() {
    for mode in [
        Readback::Identity,
        Readback::Transport,
        Readback::Protocol,
        Readback::Saturated,
    ] {
        let (mut op, calls) = setup(mode);
        let error = mutate(&mut op).unwrap_err();
        assert!(
            error.contains("correlated completion: set applied revision 13;"),
            "{error}"
        );
        assert!(!op.session.fresh(op.now()));
        assert!(op.session.pending.is_none());
        let calls = calls.lock().unwrap();
        assert_eq!(calls.sends.iter().filter(|s| s.0 == "set").count(), 1);
        assert!(calls.drained <= 64);
        if matches!(mode, Readback::Saturated) {
            assert!(error.contains("backlog saturated"));
            assert_eq!(calls.sends.len(), 1);
            assert!(
                op.brain_probes.first_fault.is_none(),
                "queue admission is recoverable"
            );
        } else {
            assert!(
                op.brain_probes.first_fault.is_some(),
                "protocol/transport faults remain classified"
            );
        }
    }
}
#[test]
fn partial_raw_then_guard_revocation_cannot_publish_fresh_or_old_completion() {
    let (mut op, _) = setup(Readback::Guard);
    let error = mutate(&mut op).unwrap_err();
    assert!(error.contains("input context revoked"));
    assert!(!error.contains("correlated completion:"));
    assert_eq!(
        op.session.snapshot.as_ref().unwrap().authority.sequence,
        "3",
        "raw was partially admitted before guard"
    );
    assert!(!op.session.fresh(op.now()));
    assert!(op.session.snapshot_age(op.now()).is_none());
    assert!(
        op.brain_probes.first_fault.is_none(),
        "guard is admission, not transport fault"
    );
    assert!(op.session.pending.is_none());
}
#[test]
fn unsolicited_stale_legacy_batch_never_solicits_or_renews_freshness() {
    let (mut op, calls) = setup(Readback::Fresh);
    // First drive completion then inject a regressive batch through a new transport.
    mutate(&mut op).unwrap();
    let corpus: Value = serde_json::from_slice(include_bytes!(
        "../../tests/fixtures/gp03/v1/e03-rendered.json"
    ))
    .unwrap();
    let mut t = Transcript {
        corpus,
        queue: VecDeque::new(),
        calls: calls.clone(),
        mode: Readback::Fresh,
        guard: Arc::new(AtomicU64::new(0)),
        queried: false,
    };
    t.push(t.raw(false));
    op.transport = Box::new(t);
    op.session.invalidate_raw_observation();
    assert!(op.refresh().is_err());
    assert_eq!(calls.lock().unwrap().sends.len(), 2);
    assert!(!op.session.fresh(op.now()));
}
