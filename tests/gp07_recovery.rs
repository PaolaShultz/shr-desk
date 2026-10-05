//! Fault-injected transport regression, not actual-provider or DSP acceptance.
use serde_json::Value;
use shr_desk::frontend::{Config, Frontend};
use std::{
    fs,
    io::{Read, Write},
    os::unix::{fs::PermissionsExt, net::UnixListener},
    sync::{
        Arc,
        atomic::{AtomicUsize, Ordering},
    },
    thread,
    time::{Duration, Instant},
};
#[test]
fn one_lost_readonly_processing_observation_does_not_disable_future_polls() {
    let dir = std::env::temp_dir().join(format!(
        "desk-gp07-recovery-{}-{}",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    ));
    fs::create_dir(&dir).unwrap();
    fs::set_permissions(&dir, fs::Permissions::from_mode(0o700)).unwrap();
    let endpoint = dir.join("audio.sock");
    let listener = UnixListener::bind(&endpoint).unwrap();
    fs::set_permissions(&endpoint, fs::Permissions::from_mode(0o600)).unwrap();
    let polls = Arc::new(AtomicUsize::new(0));
    let observed = polls.clone();
    let server = thread::spawn(move || {
        let (mut stream, _) = listener.accept().unwrap();
        stream
            .set_read_timeout(Some(Duration::from_secs(3)))
            .unwrap();
        let corpus: Value =
            serde_json::from_str(include_str!("fixtures/gp03/v1/e03-rendered.json")).unwrap();
        let mut sequence = 1000u64;
        loop {
            let mut prefix = [0; 4];
            if stream.read_exact(&mut prefix).is_err() {
                break;
            }
            let size = u32::from_be_bytes(prefix) as usize;
            assert!(size <= 65536);
            let mut bytes = vec![0; size];
            stream.read_exact(&mut bytes).unwrap();
            let request: Value = serde_json::from_slice(&bytes).unwrap();
            assert!(request["writer"].is_null(), "recovery may only query");
            sequence += 1;
            let mut reply = match request["kind"].as_str().unwrap() {
                "processing_snapshot" => {
                    if observed.fetch_add(1, Ordering::AcqRel) == 0 {
                        continue;
                    }
                    let mut v: Value =
                        serde_json::from_str(include_str!("fixtures/gp07/v2/snapshot-reply.json"))
                            .unwrap();
                    v["revision"] = "12".into();
                    v["snapshot"]["revision"] = "12".into();
                    v["snapshot"]["sequence"] = sequence.to_string().into();
                    v
                }
                "snapshot" => {
                    let mut v = corpus["grant_response"].clone();
                    for k in ["writer", "lease", "request_id", "expected_revision"] {
                        v["context"][k] = Value::Null;
                        v["outcome"][k] = Value::Null;
                    }
                    for k in ["granted_lease", "lease_remaining_ms", "scope"] {
                        v["outcome"]["body"][k] = Value::Null;
                    }
                    v["snapshot"] = corpus["initial"].clone();
                    v["snapshot"]["authority"]["sequence"] = sequence.to_string().into();
                    v
                }
                other => panic!("unexpected mutation {other}"),
            };
            reply["snapshot"]["frame"] = (sequence * 48).to_string().into();
            let bytes = serde_json::to_vec(&reply).unwrap();
            if stream
                .write_all(&(bytes.len() as u32).to_be_bytes())
                .is_err()
                || stream.write_all(&bytes).is_err()
            {
                break;
            }
        }
    });
    let mut front = Frontend::new(Config {
        endpoint,
        show: "11111111-1111-4111-8111-111111111111".into(),
        epoch: 9,
        writer: "gp07-readonly".into(),
        scope: "foh".into(),
    });
    front.enable_processing().unwrap();
    let end = Instant::now() + Duration::from_secs(2);
    while !front.processing_ready() && Instant::now() < end {
        front.pump();
        thread::sleep(Duration::from_millis(5));
    }
    let ready = front.processing_ready();
    let status = front.state.as_ref().map(|s| s.processing_status.clone());
    drop(front);
    server.join().unwrap();
    fs::remove_dir_all(dir).unwrap();
    assert!(ready, "read-only polling did not recover: {status:?}");
    assert!(polls.load(Ordering::Acquire) >= 2);
}
