use super::*;
use std::collections::VecDeque;
struct Staggered {
    raw: Vec<u8>,
    structural: Vec<u8>,
    replies: VecDeque<Vec<u8>>,
    delayed: bool,
    delay_ms: u64,
}
impl AuthorityConnection for Staggered {
    fn send_frame_until(&mut self, bytes: &[u8], _: Instant) -> Result<(), String> {
        let request: Value = serde_json::from_slice(bytes).unwrap();
        match request["kind"].as_str().unwrap() {
            "snapshot" => self.replies.push_back(self.raw.clone()),
            "structural_snapshot" => self.replies.push_front(self.structural.clone()),
            _ => panic!("read-only pair expected"),
        }
        Ok(())
    }
    fn receive_until(&mut self, _: Instant) -> Result<Option<Vec<u8>>, String> {
        if !self.delayed && self.replies.len() == 1 {
            std::thread::sleep(Duration::from_millis(self.delay_ms));
            self.delayed = true;
        }
        Ok(self.replies.pop_front())
    }
    fn receive_available(&mut self) -> Result<Option<Vec<u8>>, String> {
        Ok(None)
    }
}
#[test]
fn raw_expiring_in_flight_does_not_deadlock_structural_readback() {
    // Protect pairing/expiry, not debug48-input schema throughput. Full-size
    // documents have separate transport/schema and release acceptance coverage.
    let corpus: Value = serde_json::from_slice(include_bytes!(
        "../../tests/fixtures/gp14/final/structure-replies-16.json"
    ))
    .unwrap();
    let structural = corpus["exchanges"][0]["reply"].clone();
    let profile: Value = serde_json::from_slice(include_bytes!(
        "../../tests/fixtures/gp14/v1/profile-16.json"
    ))
    .unwrap();
    let mut raw = profile["snapshot"].clone();
    raw["context"]["epoch"] = "1".into();
    raw["outcome"]["epoch"] = "1".into();
    raw["outcome"]["body"]["snapshot"]["epoch"] = "1".into();
    raw["snapshot"]["authority"]["epoch"] = "1".into();
    raw["snapshot"]["clock"]["epoch"] = 1.into();
    raw["snapshot"]["topology"] = structural["snapshot"]["topology"].clone();
    let initial = audio::decode_reply(&serde_json::to_vec(&raw).unwrap())
        .unwrap()
        .snapshot
        .unwrap();
    raw["snapshot"]["frame"] = "48".into();
    raw["snapshot"]["clock"]["next_frame"] = 48.into();
    for delay_ms in [5, 260] {
        let mut operator = Operator::from_document_connection(
            Box::new(Staggered {
                raw: serde_json::to_vec(&raw).unwrap(),
                structural: serde_json::to_vec(&structural).unwrap(),
                replies: VecDeque::new(),
                delayed: false,
                delay_ms,
            }),
            &initial.authority.show_id,
            1,
            "paired-structure",
            "pa_configuration",
            2,
        )
        .unwrap();
        operator
            .session
            .ingest_snapshot(initial.clone(), 0)
            .unwrap();
        operator.start = Instant::now() - Duration::from_millis(249);
        let result = operator.refresh_structural();
        if delay_ms == 5 {
            result.unwrap();
            assert!(operator.session.structural_fresh(operator.now()));
            assert_eq!(operator.session.snapshot.as_ref().unwrap().frame, "48");
        } else {
            assert!(result.unwrap_err().contains("deadline/queue bound"));
            assert!(!operator.session.structural_fresh(operator.now()));
            operator
                .session
                .ingest_snapshot(initial.clone(), operator.now())
                .unwrap();
            assert!(!operator.session.structural_fresh(operator.now()));
        }
    }
}
