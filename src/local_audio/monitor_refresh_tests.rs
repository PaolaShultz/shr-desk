use super::*;
use std::{
    collections::VecDeque,
    sync::{Arc, Mutex},
};
#[derive(Default)]
struct Control {
    fault: &'static str,
    guard: Option<Arc<std::sync::atomic::AtomicU64>>,
}
struct Peer {
    control: Arc<Mutex<Control>>,
    raw: audio::RenderedSnapshot,
    brain: crate::brain::Snapshot,
    device: Value,
    replies: VecDeque<Vec<u8>>,
    sent: Arc<Mutex<Vec<String>>>,
}
impl AuthorityConnection for Peer {
    fn held_identity(&self) -> Option<crate::held_proof::Identity> {
        Some(crate::held_proof::Identity {
            session: "1".into(),
            epoch: "1".into(),
            capability: "1".into(),
            map: self.raw.topology.as_ref().unwrap().map_revision.to_string(),
        })
    }
    fn send_frame_until(&mut self, bytes: &[u8], _: Instant) -> Result<(), String> {
        let r: Value = serde_json::from_slice(bytes).unwrap();
        let kind = r["kind"].as_str().unwrap();
        self.sent.lock().unwrap().push(kind.into());
        let reply = match kind {
            "readback" => {
                self.raw.frame = (self.raw.frame.parse::<u64>().unwrap() + 48).to_string();
                self.brain.frame = self.raw.frame.clone();
                json!({"contract":"GP15-paired-readback","version":1,"state":"snapshot","reason":null,"context":r,"raw":self.raw,"brain":self.brain})
            }
            "device_snapshot" => {
                let control = self.control.lock().unwrap();
                if control.fault == "missing" {
                    return Ok(());
                }
                if control.fault != "duplicate" {
                    self.device["observation"]["frame"] =
                        json!(self.device["observation"]["frame"].as_u64().unwrap() + 48);
                }
                match control.fault {
                    "disconnected" => self.device["connected"] = json!(false),
                    "epoch" => self.device["observation"]["brain_epoch"] = json!(999),
                    "map" => self.device["observation"]["brain_map"] = json!(999),
                    "config" => self.device["observation"]["config"]["period_frames"] = json!(96),
                    "identity" => {
                        self.device["observation"]["config"]["device_id"] = json!("replacement")
                    }
                    "cancel" => {
                        control
                            .guard
                            .as_ref()
                            .unwrap()
                            .fetch_add(1, std::sync::atomic::Ordering::AcqRel);
                    }
                    _ => (),
                }
                self.device.clone()
            }
            "monitor_set" => return Err("test reached monitor mutation".into()),
            other => panic!("unexpected {other}"),
        };
        self.replies.push_back(serde_json::to_vec(&reply).unwrap());
        Ok(())
    }
    fn receive_until(&mut self, _: Instant) -> Result<Option<Vec<u8>>, String> {
        Ok(self.replies.pop_front())
    }
    fn receive_available(&mut self) -> Result<Option<Vec<u8>>, String> {
        Ok(None)
    }
}
type Rig = (Operator, Arc<Mutex<Vec<String>>>, Arc<Mutex<Control>>);
fn setup() -> Rig {
    let raw = audio::decode_snapshot(include_bytes!(
        "../../tests/fixtures/gp15/v1/raw-snapshot-16-1.json"
    ))
    .unwrap();
    let mut brain = crate::brain::decode_reply(include_bytes!(
        "../../tests/fixtures/gp15/v1/hold-final.json"
    ))
    .unwrap()
    .snapshot
    .unwrap();
    brain.revision = raw.authority.revision.clone();
    brain.frame = raw.frame.clone();
    brain.source = crate::brain::Source::Main;
    brain.talkback_monitors = vec![0];
    brain.held_generation = None;
    brain.hold_generation_counter = "0".into();
    brain.hold_deadline_ms = None;
    let device: Value = serde_json::from_slice(include_bytes!(
        "../../tests/fixtures/gp15/device-v1/snapshot-unarmed.json"
    ))
    .unwrap();
    let sent = Arc::new(Mutex::new(Vec::new()));
    let control = Arc::new(Mutex::new(Control::default()));
    let mut op = Operator::from_document_connection(
        Box::new(Peer {
            control: control.clone(),
            raw: raw.clone(),
            brain: brain.clone(),
            device: device.clone(),
            replies: VecDeque::new(),
            sent: sent.clone(),
        }),
        &raw.authority.show_id,
        1,
        "operator",
        "local_operator_monitor",
        2,
    )
    .unwrap();
    op.session.ingest_snapshot(raw.clone(), 300).unwrap();
    op.session
        .begin("grant", json!({"scope":"local_operator_monitor"}), 300)
        .unwrap();
    op.session
        .accept(
            audio::decode_reply(include_bytes!(
                "../../tests/fixtures/gp15/v1/grant-16-operator-reply.json"
            ))
            .unwrap(),
            300,
        )
        .unwrap();
    // Shift only the injected local origin: no wall-clock sleep or race.
    op.start = Instant::now() - Duration::from_millis(300);
    op.session.ingest_snapshot(raw, op.now()).unwrap();
    op.session.ingest_brain(brain, op.now()).unwrap();
    op.session
        .dispatch_device(&serde_json::to_vec(&device).unwrap(), 0)
        .unwrap();
    op.session.input_released();
    (op, sent, control)
}
#[test]
fn stale_background_device_is_refreshed_for_reviewed_monitor_admission() {
    let (mut op, sent, _) = setup();
    assert!(!op.session.device_fresh(op.now()));
    op.stage(
        "brain_monitor_set",
        json!({"source":{"kind":"main"},"gain_cdb":-1800,"mute":false,"dim":false,"armed":true}),
    )
    .unwrap();
    // The review's timely device report can itself age before confirmation.
    op.start -= Duration::from_millis(300);
    let error = op.confirm().unwrap_err();
    assert_eq!(error, "test reached monitor mutation");
    assert_eq!(
        sent.lock().unwrap().as_slice(),
        [
            "readback",
            "readback",
            "device_snapshot",
            "readback",
            "readback",
            "readback",
            "device_snapshot",
            "monitor_set"
        ]
    );
}
fn body() -> Value {
    json!({"source":{"kind":"main"},"gain_cdb":-1800,"mute":false,"dim":false,"armed":true})
}
#[test]
fn monitor_refresh_refuses_missing_stale_disconnected_and_changed_devices() {
    for fault in [
        "missing",
        "duplicate",
        "disconnected",
        "epoch",
        "map",
        "config",
        "identity",
    ] {
        for at_confirm in [false, true] {
            let (mut op, sent, control) = setup();
            if at_confirm {
                op.stage("brain_monitor_set", body()).unwrap();
                op.start -= Duration::from_millis(300);
            }
            control.lock().unwrap().fault = fault;
            let result = if at_confirm {
                op.confirm()
            } else {
                op.stage("brain_monitor_set", body())
            };
            let error = result.unwrap_err();
            let expected = match fault {
                "missing" => "device observation deadline",
                "duplicate" => "fresh actual monitor device",
                _ => "epoch/map/config",
            };
            assert!(
                error.contains(expected),
                "{fault} confirm={at_confirm}: {error}"
            );
            assert!(op.draft.is_none());
            assert!(op.session.pending.is_none());
            assert!(!sent.lock().unwrap().iter().any(|k| k == "monitor_set"));
        }
    }
}
#[test]
fn monitor_confirmation_cancellation_and_revision_change_never_send() {
    for fault in ["cancel", "revision", "generation", "explicit_cancel"] {
        let (mut op, sent, control) = setup();
        op.stage("brain_monitor_set", body()).unwrap();
        match fault {
            "cancel" => {
                let guard = Arc::new(std::sync::atomic::AtomicU64::new(1));
                op.guard(guard.clone(), 1);
                let mut c = control.lock().unwrap();
                c.fault = "cancel";
                c.guard = Some(guard);
            }
            "revision" => op.session.snapshot.as_mut().unwrap().authority.revision = "1".into(),
            "generation" => op.session.context_changed(),
            _ => op.cancel(),
        }
        assert!(op.confirm().is_err(), "{fault}");
        assert!(op.draft.is_none());
        assert!(op.session.pending.is_none());
        assert!(!sent.lock().unwrap().iter().any(|k| k == "monitor_set"));
    }
}
#[test]
fn changed_monitor_review_requires_new_explicit_review() {
    let (mut op, _, _) = setup();
    op.stage("brain_monitor_set", body()).unwrap();
    op.session
        .device
        .as_mut()
        .unwrap()
        .observation
        .as_mut()
        .unwrap()
        .brain_map += 1;
    assert!(!op.review_valid());
    assert!(op.confirm().unwrap_err().contains("epoch/map/config"));
    assert!(op.draft.is_none());
}
