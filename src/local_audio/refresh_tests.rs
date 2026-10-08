use super::*;
use std::{
    fs,
    io::{Read, Write},
    os::unix::{fs::PermissionsExt, net::UnixListener},
    sync::mpsc,
    thread,
};

#[test]
fn partial_body_kernel_wait_slices_preserve_original_full_frame_deadline() {
    for delay in [130, 240] {
        let (socket, mut peer) = UnixStream::pair().unwrap();
        peer.write_all(&[0, 0, 0, 2, b'{']).unwrap();
        let writer = thread::spawn(move || {
            thread::sleep(Duration::from_millis(delay));
            let _ = peer.write_all(b"}");
        });
        let mut transport = Transport::from_stream(socket);
        let deadline = Instant::now() + Duration::from_millis(200);
        let result = transport.receive_until(deadline);
        if delay == 130 {
            assert_eq!(result.unwrap(), Some(b"{}".to_vec()));
        } else {
            assert!(
                result.is_err(),
                "partial body must not extend original deadline"
            );
            assert!(Instant::now() >= deadline);
        }
        writer.join().unwrap();
    }
}

#[test]
fn available_partial_unix_frame_inherits_the_remaining_refresh_deadline() {
    let (socket, mut peer) = UnixStream::pair().unwrap();
    let observer = socket.try_clone().unwrap();
    let transport = Transport::from_stream(socket);
    // Exercise the actual Operator wrapper stack, not only low-level framing.
    let mut connection = crate::pages::Connection::new(Box::new(transport));
    // Model a frame becoming readable late in the original 250ms operation.
    // Only one prefix byte is available; completion must not get a new200ms.
    let started = Instant::now() - Duration::from_millis(220);
    let deadline = started + Duration::from_millis(250);
    peer.write_all(&[0]).unwrap();
    let result = AuthorityConnection::receive_available_until(&mut connection, deadline);
    assert!(result.is_err(), "partial frame cannot be admitted");
    // Verify the real kernel read budget, without imposing a scheduler-speed
    // assertion on CI: allow kernel timeout rounding up to60ms; the old
    // available path used a fresh100ms read timeout.
    assert!(observer.read_timeout().unwrap().unwrap() <= Duration::from_millis(60));
    assert!(Instant::now() >= deadline);
    assert!(AuthorityConnection::receive_available_until(&mut connection, deadline).is_err());
}

#[test]
fn unsolicited_burst_is_coalesced_without_queries_and_old_frames_do_not_refresh() {
    let dir = std::env::temp_dir().join(format!(
        "desk-refresh-{}-{}",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    ));
    fs::create_dir(&dir).unwrap();
    fs::set_permissions(&dir, fs::Permissions::from_mode(0o700)).unwrap();
    let socket = dir.join("audio.sock");
    let listener = UnixListener::bind(&socket).unwrap();
    fs::set_permissions(&socket, fs::Permissions::from_mode(0o600)).unwrap();
    let (trigger, receive) = mpsc::channel();
    let (sent, ready) = mpsc::channel();
    let server = thread::spawn(move || {
        let corpus: Value = serde_json::from_str(include_str!(
            "../../tests/fixtures/gp03/v1/e03-rendered.json"
        ))
        .unwrap();
        let (mut stream, _) = listener.accept().unwrap();
        stream
            .set_read_timeout(Some(Duration::from_secs(2)))
            .unwrap();
        let mut prefix = [0; 4];
        stream.read_exact(&mut prefix).unwrap();
        let mut request = vec![0; u32::from_be_bytes(prefix) as usize];
        stream.read_exact(&mut request).unwrap();
        assert_eq!(
            serde_json::from_slice::<Value>(&request).unwrap()["kind"],
            "snapshot"
        );
        let mut reply = corpus["grant_response"].clone();
        for k in ["writer", "lease", "request_id", "expected_revision"] {
            reply["context"][k] = Value::Null;
            reply["outcome"][k] = Value::Null;
        }
        for k in ["granted_lease", "lease_remaining_ms", "scope"] {
            reply["outcome"]["body"][k] = Value::Null;
        }
        reply["snapshot"] = corpus["initial"].clone();
        let send = |stream: &mut UnixStream, reply: &Value| {
            let bytes = serde_json::to_vec(reply).unwrap();
            stream
                .write_all(&(bytes.len() as u32).to_be_bytes())
                .unwrap();
            stream.write_all(&bytes).unwrap();
        };
        send(&mut stream, &reply);
        receive.recv().unwrap();
        let mut newer = reply.clone();
        newer["snapshot"]["authority"]["sequence"] = json!("2");
        newer["snapshot"]["frame"] = json!("48048");
        let mut older = reply;
        older["snapshot"]["authority"]["sequence"] = json!("0");
        older["snapshot"]["frame"] = json!("47952");
        send(&mut stream, &newer);
        send(&mut stream, &older);
        sent.send(()).unwrap();
        receive.recv().unwrap();
        send(&mut stream, &older);
        sent.send(()).unwrap();
        receive.recv().unwrap();
        // 65 complete accepted-format frames fit in the private socket buffer:
        // an old observation, 63 cached replies, then the current observation.
        send(&mut stream, &newer);
        for _ in 0..63 {
            send(&mut stream, &corpus["pending"]);
        }
        newer["snapshot"]["authority"]["sequence"] = json!("3");
        newer["snapshot"]["frame"] = json!("48096");
        send(&mut stream, &newer);
        sent.send(()).unwrap();
        // No refresh may send another query while buffered telemetry exists.
        assert_eq!(stream.read(&mut prefix).unwrap(), 0);
    });
    let mut op = Operator::connect(
        &socket,
        "11111111-1111-4111-8111-111111111111",
        9,
        "refresh-test",
        "foh",
    )
    .unwrap();
    op.refresh().unwrap();
    trigger.send(()).unwrap();
    ready.recv().unwrap();
    op.refresh().unwrap();
    assert_eq!(
        op.session.snapshot.as_ref().unwrap().authority.sequence,
        "2"
    );
    op.refresh().unwrap(); // quiet but still fresh: no new query
    thread::sleep(Duration::from_millis(260));
    trigger.send(()).unwrap();
    ready.recv().unwrap();
    assert!(op.refresh().is_err());
    assert!(!op.session.fresh(op.now()));
    assert_eq!(
        op.session.snapshot.as_ref().unwrap().authority.sequence,
        "2"
    );
    let generation = op.session.generation();
    trigger.send(()).unwrap();
    ready.recv().unwrap();
    assert!(op.refresh().unwrap_err().contains("backlog saturated"));
    assert!(!op.session.fresh(op.now()));
    assert_eq!(op.session.generation(), generation);
    assert_eq!(
        op.session.snapshot.as_ref().unwrap().authority.revision,
        "12"
    );
    assert_eq!(
        op.session.snapshot.as_ref().unwrap().authority.sequence,
        "2"
    );
    assert!(op.session.preview(op.now()).is_none());
    op.refresh().unwrap();
    assert_eq!(
        op.session.snapshot.as_ref().unwrap().authority.sequence,
        "3"
    );
    drop(op);
    server.join().unwrap();
    fs::remove_dir_all(dir).unwrap();
}
