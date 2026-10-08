use super::*;
use std::collections::VecDeque;
struct Replies {
    raw: Vec<u8>,
    extension: Vec<u8>,
    queue: VecDeque<Vec<u8>>,
    deadlines: std::sync::Arc<std::sync::Mutex<Vec<Instant>>>,
}
impl AuthorityConnection for Replies {
    fn send_frame_until(&mut self, bytes: &[u8], deadline: Instant) -> Result<(), String> {
        self.deadlines.lock().unwrap().push(deadline);
        let v: Value = serde_json::from_slice(bytes).unwrap();
        self.queue.push_back(if v["kind"] == "snapshot" {
            self.raw.clone()
        } else {
            self.extension.clone()
        });
        Ok(())
    }
    fn receive_available(&mut self) -> Result<Option<Vec<u8>>, String> {
        Ok(None)
    }
    fn receive_until(&mut self, deadline: Instant) -> Result<Option<Vec<u8>>, String> {
        self.deadlines.lock().unwrap().push(deadline);
        if self.queue.is_empty() {
            std::thread::sleep(Duration::from_millis(2));
        }
        Ok(self.queue.pop_front())
    }
}
fn raw_reply(snapshot: &audio::RenderedSnapshot) -> Vec<u8> {
    let corpus: Value = serde_json::from_slice(include_bytes!(
        "../../tests/fixtures/gp14/v1/profile-17.json"
    ))
    .unwrap();
    let mut r = corpus["snapshot"].clone();
    let body = serde_json::to_value(snapshot).unwrap();
    r["snapshot"] = body.clone();
    r["context"]["epoch"] = json!(snapshot.authority.epoch);
    r["context"]["show_id"] = json!(snapshot.authority.show_id);
    for key in [
        "show_id",
        "epoch",
        "writer",
        "lease",
        "expected_revision",
        "request_id",
        "module",
    ] {
        r["outcome"][key] = r["context"][key].clone();
    }
    r["outcome"]["body"]["revision"] = json!(snapshot.authority.revision);
    r["outcome"]["body"]["snapshot"] = body["authority"].clone();
    serde_json::to_vec(&r).unwrap()
}
#[test]
fn sends_pair_requires_both_source_progress_and_one_original_deadline() {
    let baseline = audio::decode_snapshot(include_bytes!(
        "../../tests/fixtures/gp18/v1-corrected/raw-baseline.json"
    ))
    .unwrap();
    let old = crate::sends::decode_reply(include_bytes!(
        "../../tests/fixtures/gp18/v1-corrected/baseline.json"
    ))
    .unwrap();
    for (raw_new, ext_new) in [(false, true), (true, false), (true, true)] {
        let mut raw = baseline.clone();
        if raw_new {
            raw.frame = "48".into();
            raw.clock.as_mut().unwrap().next_frame = 48;
            raw.authority.sequence =
                (crate::provider::counter(&raw.authority.sequence).unwrap() + 1).to_string();
        }
        let mut ext = old.clone();
        if ext_new {
            ext.snapshot.as_mut().unwrap().sequence = "99".into();
        }
        let deadlines = std::sync::Arc::new(std::sync::Mutex::new(Vec::new()));
        let mut o = Operator::from_document_connection(
            Box::new(Replies {
                raw: raw_reply(&raw),
                extension: {
                    let mut v: Value = serde_json::from_slice(include_bytes!(
                        "../../tests/fixtures/gp18/v1-corrected/baseline.json"
                    ))
                    .unwrap();
                    v["snapshot"] = serde_json::to_value(ext.snapshot.as_ref().unwrap()).unwrap();
                    serde_json::to_vec(&v).unwrap()
                },
                queue: VecDeque::new(),
                deadlines: deadlines.clone(),
            }),
            &baseline.authority.show_id,
            9,
            "paired-sends",
            "monitor3",
            2,
        )
        .unwrap();
        o.session.ingest_snapshot(baseline.clone(), 0).unwrap();
        o.session
            .ingest_sends(old.snapshot.clone().unwrap(), 0)
            .unwrap();
        o.start = Instant::now() - Duration::from_millis(300);
        let result = o.refresh_sends();
        assert_eq!(
            result.is_ok(),
            raw_new && ext_new,
            "{raw_new}/{ext_new}: {result:?}"
        );
        if !result.is_ok() {
            assert!(o.session.sends_age(o.now()).unwrap() >= 300);
            assert!(o.session.snapshot_age(o.now()).unwrap() >= 300);
            assert!(!o.session.sends_fresh(o.now()));
        }
        let times = deadlines.lock().unwrap();
        assert!(!times.is_empty());
        assert!(times.iter().all(|d| *d == times[0]));
    }
}

#[test]
fn live_pair_requires_independent_progress_under_original_deadline() {
    let corpus: Vec<Value> = serde_json::from_slice(include_bytes!(
        "../../tests/fixtures/master-eq/v1/producer.json"
    ))
    .unwrap();
    let old = crate::live_eq::Snapshot::decode(
        corpus.iter().find(|r| r["label"] == "baseline").unwrap()["snapshot"].clone(),
    )
    .unwrap();
    let mut baseline = audio::decode_snapshot(include_bytes!(
        "../../tests/fixtures/gp18/v1-corrected/raw-baseline.json"
    ))
    .unwrap();
    baseline.authority.epoch = old.epoch.clone();
    baseline.clock.as_mut().unwrap().epoch = old.epoch.parse().unwrap();
    baseline.authority.revision = old.revision.clone();
    baseline.topology.as_mut().unwrap().map_revision = old.map_revision.parse().unwrap();
    for (raw_new, ext_new) in [(false, true), (true, false), (true, true)] {
        let mut raw = baseline.clone();
        if raw_new {
            raw.frame = "48".into();
            raw.clock.as_mut().unwrap().next_frame = 48;
            raw.authority.sequence =
                (crate::provider::counter(&raw.authority.sequence).unwrap() + 1).to_string();
        }
        let mut live = old.clone();
        if ext_new {
            live.frame = (crate::provider::counter(&old.frame).unwrap() + 48).to_string();
        }
        let deadlines = std::sync::Arc::new(std::sync::Mutex::new(Vec::new()));
        let context = audio::Context {
            show_id: old.show_id.clone(),
            module: "audio".into(),
            epoch: old.epoch.clone(),
            writer: None,
            lease: None,
            request_id: None,
            expected_revision: None,
        };
        let extension = serde_json::to_vec(&json!({"contract":"GP18-master-eq","version":1,"context":context,"state":"snapshot","revision":live.revision,"snapshot":null,"effective_frame":null,"reason":null,"master_eq":live})).unwrap();
        let mut o = Operator::from_document_connection(
            Box::new(Replies {
                raw: raw_reply(&raw),
                extension,
                queue: VecDeque::new(),
                deadlines: deadlines.clone(),
            }),
            &old.show_id,
            old.epoch.parse().unwrap(),
            "paired-live",
            "pa_configuration",
            2,
        )
        .unwrap();
        o.session.ingest_snapshot(baseline.clone(), 0).unwrap();
        o.session.ingest_live_eq(old.clone(), 0).unwrap();
        o.start = Instant::now() - Duration::from_millis(300);
        let result = o.refresh_live_eq();
        assert_eq!(
            result.is_ok(),
            raw_new && ext_new,
            "{raw_new}/{ext_new}: {result:?}"
        );
        if result.is_err() {
            assert!(o.session.live_eq_age(o.now()).unwrap() >= 300);
            assert!(o.session.snapshot_age(o.now()).unwrap() >= 300);
            assert!(!o.session.live_eq_fresh(o.now()));
            assert_eq!(o.session.live_eq, Some(old.clone()));
        }
        let times = deadlines.lock().unwrap();
        assert!(!times.is_empty());
        assert!(times.iter().all(|d| *d == times[0]));
    }
}
