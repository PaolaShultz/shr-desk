//! Synthetic adversarial authority revisions; no engine algorithm implementation.
#[cfg(target_os = "linux")]
#[test]
fn confirmed_mode_keeps_reviewed_revision_when_authority_changes() {
    use serde_json::{Value, json};
    use std::{
        fs,
        io::{Read, Write},
        os::unix::{fs::PermissionsExt, net::UnixListener},
        process::Command,
        time::Duration,
    };
    let dir = std::env::temp_dir().join(format!("shr-desk-confirm-{}", std::process::id()));
    fs::create_dir(&dir).unwrap();
    fs::set_permissions(&dir, fs::Permissions::from_mode(0o700)).unwrap();
    let socket = dir.join("audio.sock");
    let script = dir.join("commands");
    fs::write(&script, "grant\ninput-release\nmode assist\nconfirm\n").unwrap();
    let listener = UnixListener::bind(&socket).unwrap();
    fs::set_permissions(&socket, fs::Permissions::from_mode(0o600)).unwrap();
    let child = std::thread::spawn(move || {
        let c: Value =
            serde_json::from_str(include_str!("fixtures/gp03/v1/e03-rendered.json")).unwrap();
        let (mut s, _) = listener.accept().unwrap();
        s.set_read_timeout(Some(Duration::from_secs(2))).unwrap();
        let mut mode_seen = false;
        let mut sequence = 0u64;
        loop {
            let mut h = [0; 4];
            if let Err(e) = s.read_exact(&mut h) {
                assert_eq!(e.kind(), std::io::ErrorKind::UnexpectedEof);
                break;
            }
            let mut b = vec![0; u32::from_be_bytes(h) as usize];
            s.read_exact(&mut b).unwrap();
            let q: Value = serde_json::from_slice(&b).unwrap();
            let mut r = c["grant_response"].clone();
            for k in [
                "show_id",
                "module",
                "epoch",
                "writer",
                "lease",
                "request_id",
                "expected_revision",
            ] {
                r["context"][k] = q[k].clone();
                r["outcome"][k] = q[k].clone();
            }
            match q["kind"].as_str().unwrap() {
                "snapshot" => {
                    for k in ["granted_lease", "scope", "lease_remaining_ms"] {
                        r["outcome"]["body"][k] = Value::Null;
                    }
                    r["snapshot"] = c["initial"].clone();
                    // Advancing telemetry must preserve the adversarial revision.
                    sequence = sequence.max(
                        r["snapshot"]["authority"]["sequence"]
                            .as_str()
                            .unwrap()
                            .parse()
                            .unwrap(),
                    ) + 1;
                    r["snapshot"]["authority"]["sequence"] = json!(sequence.to_string());
                    if mode_seen {
                        r["outcome"]["body"]["revision"] = json!("13");
                        r["snapshot"]["authority"]["revision"] = json!("13");
                    }
                }
                "grant" => {}
                "set_mode" => {
                    assert_eq!(q["expected_revision"], "12");
                    mode_seen = true;
                    r["outcome"]["kind"] = json!("conflict");
                    r["outcome"]["body"]["reason"] = json!("stale_revision");
                    r["outcome"]["body"]["revision"] = json!("13");
                    for k in ["granted_lease", "scope", "lease_remaining_ms"] {
                        r["outcome"]["body"][k] = Value::Null;
                    }
                }
                k => panic!("unexpected {k}"),
            }
            let b = serde_json::to_vec(&r).unwrap();
            s.write_all(&(b.len() as u32).to_be_bytes()).unwrap();
            s.write_all(&b).unwrap();
        }
        assert!(mode_seen);
    });
    let output = Command::new(env!("CARGO_BIN_EXE_shr-desk"))
        .args([
            "--audio-local",
            socket.to_str().unwrap(),
            "11111111-1111-4111-8111-111111111111",
            "9",
            "desk-corpus",
            "foh",
            "--script",
            script.to_str().unwrap(),
        ])
        .output()
        .unwrap();
    child.join().unwrap();
    fs::remove_dir_all(&dir).unwrap();
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let text = String::from_utf8(output.stdout).unwrap();
    let status: Value = serde_json::from_str(text.lines().last().unwrap()).unwrap();
    assert!(status["result"].as_str().unwrap().contains("conflict"));
}
