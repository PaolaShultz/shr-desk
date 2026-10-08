use super::*;
use std::collections::VecDeque;
struct DevicePeer {
    device: Value,
    replies: VecDeque<Vec<u8>>,
    replace: u8,
}
impl AuthorityConnection for DevicePeer {
    fn send_frame_until(&mut self, bytes: &[u8], _: Instant) -> Result<(), String> {
        let r: Value = serde_json::from_slice(bytes).unwrap();
        assert_eq!(
            r["kind"], "device_snapshot",
            "stale intent must never be sent"
        );
        if self.replace == 1 {
            self.device["observation"]["brain_epoch"] = json!(999);
        }
        self.device["observation"]["frame"] = json!(10000);
        self.replies
            .push_back(serde_json::to_vec(&self.device).unwrap());
        Ok(())
    }
    fn receive_until(&mut self, _: Instant) -> Result<Option<Vec<u8>>, String> {
        Ok(self.replies.pop_front())
    }
    fn receive_available(&mut self) -> Result<Option<Vec<u8>>, String> {
        if self.replace == 2 {
            self.replace = 0;
            self.device["observation"]["brain_epoch"] = json!(999);
            return Ok(Some(serde_json::to_vec(&self.device).unwrap()));
        }
        Ok(None)
    }
}
fn operator(replace: u8) -> (Operator, Value) {
    let raw = audio::decode_snapshot(include_bytes!(
        "../../tests/fixtures/gp15/v1/raw-snapshot-16-1.json"
    ))
    .unwrap();
    let device: Value = serde_json::from_slice(include_bytes!(
        "../../tests/fixtures/gp15/device-v1/snapshot-unarmed.json"
    ))
    .unwrap();
    let body = json!({"config":device["observation"]["config"],
        "device_identity":[device["observation"]["brain_epoch"],device["observation"]["brain_map"]]});
    let mut op = Operator::from_document_connection(
        Box::new(DevicePeer {
            device: device.clone(),
            replies: VecDeque::new(),
            replace,
        }),
        &raw.authority.show_id,
        raw.authority.epoch.parse().unwrap(),
        "device-fence",
        "local_operator_monitor",
        2,
    )
    .unwrap();
    op.session.ingest_snapshot(raw, 0).unwrap();
    op.session
        .dispatch_device(&serde_json::to_vec(&device).unwrap(), 0)
        .unwrap();
    (op, body)
}
#[test]
fn device_replacement_during_stage_cannot_rebase_old_intent() {
    let (mut op, body) = operator(1);
    assert!(
        op.stage("device_configure", body)
            .unwrap_err()
            .contains("epoch/map")
    );
    assert!(op.draft.is_none());
    let d = op.session.device.as_ref().unwrap();
    let fresh = json!({"config":d.observation.as_ref().unwrap().config,"device_identity":d.identity().unwrap()});
    op.stage("device_configure", fresh).unwrap();
    assert!(op.review_valid());
}
#[test]
fn device_replacement_in_interleaved_drain_cannot_publish_old_review() {
    let (mut op, body) = operator(2);
    assert!(
        op.stage("device_configure", body)
            .unwrap_err()
            .contains("epoch/map")
    );
    assert!(op.draft.is_none());
}
#[test]
fn device_replacement_during_confirm_discards_prepared_application() {
    let (mut op, body) = operator(1);
    op.draft = Some(Draft {
        measurement_basis: None,
        fx_basis: None,
        monitor_device: None,
        kind: "device_configure".into(),
        body,
        revision: op
            .session
            .snapshot
            .as_ref()
            .unwrap()
            .authority
            .revision
            .clone(),
        generation: op.session.generation(),
    });
    assert!(op.review_valid());
    assert!(op.confirm().unwrap_err().contains("epoch/map"));
    assert!(op.draft.is_none());
    assert!(op.session.pending.is_none());
}
#[test]
fn device_review_and_session_validation_require_original_map_and_freshness() {
    let (mut op, body) = operator(0);
    op.stage("device_configure", body.clone()).unwrap();
    assert!(op.review_valid());
    op.session
        .device
        .as_mut()
        .unwrap()
        .observation
        .as_mut()
        .unwrap()
        .brain_map += 1;
    assert!(!op.review_valid());
    assert!(op.session.validate_device_intent(&body, op.now()).is_err());
    op.session.invalidate_device();
    assert!(!op.review_valid());
}
