//! Synthetic IPC verifies console transport only; real provider integration is separate.
#[cfg(target_os = "linux")]
mod linux {
    use serde_json::{Value, json};
    use std::{
        fs,
        io::{Read, Write},
        os::unix::{fs::PermissionsExt, net::UnixListener},
        process::Command,
        time::Duration,
    };
    fn private() -> std::path::PathBuf {
        let p = std::env::temp_dir().join(format!(
            "shr-desk-audio-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        fs::create_dir(&p).unwrap();
        fs::set_permissions(&p, fs::Permissions::from_mode(0o700)).unwrap();
        p
    }
    fn reply() -> Value {
        let c: Value =
            serde_json::from_str(include_str!("fixtures/gp03/v1/e03-rendered.json")).unwrap();
        let mut r = c["grant_response"].clone();
        for k in ["writer", "lease", "request_id", "expected_revision"] {
            r["context"][k] = Value::Null;
            r["outcome"][k] = Value::Null;
        }
        r["outcome"]["body"]["granted_lease"] = Value::Null;
        r["outcome"]["body"]["lease_remaining_ms"] = Value::Null;
        r["outcome"]["body"]["scope"] = Value::Null;
        r["snapshot"] = c["initial"].clone();
        r
    }
    #[test]
    fn readonly_private_uds_status_without_grant() {
        let p = private();
        let socket = p.join("audio.sock");
        let listener = UnixListener::bind(&socket).unwrap();
        fs::set_permissions(&socket, fs::Permissions::from_mode(0o600)).unwrap();
        let child = std::thread::spawn(move || {
            let (mut s, _) = listener.accept().unwrap();
            s.set_read_timeout(Some(Duration::from_secs(2))).unwrap();
            let mut n = [0; 4];
            s.read_exact(&mut n).unwrap();
            let mut b = vec![0; u32::from_be_bytes(n) as usize];
            s.read_exact(&mut b).unwrap();
            let request: Value = serde_json::from_slice(&b).unwrap();
            assert_eq!(request["kind"], json!("snapshot"));
            assert!(request["writer"].is_null());
            let b = serde_json::to_vec(&reply()).unwrap();
            s.write_all(&(b.len() as u32).to_be_bytes()).unwrap();
            s.write_all(&b).unwrap();
            // Keep the provider connection live while its complete snapshot is drained.
            assert_eq!(s.read(&mut [0; 1]).unwrap(), 0);
        });
        let o = Command::new(env!("CARGO_BIN_EXE_shr-desk"))
            .args([
                "--audio-local",
                socket.to_str().unwrap(),
                "11111111-1111-4111-8111-111111111111",
                "9",
                "desk-cli",
                "foh",
            ])
            .output()
            .unwrap();
        child.join().unwrap();
        fs::remove_dir_all(&p).unwrap();
        assert!(o.status.success(), "{}", String::from_utf8_lossy(&o.stderr));
        let status: Value = serde_json::from_slice(&o.stdout).unwrap();
        assert_eq!(status["mode"], "real-local-offline-provider");
        assert!(status["lease_deadline_local_ms"].is_null());
        assert!(status["meters"].is_null());
        assert_eq!(status["coefficients"].as_array().unwrap().len(), 8);
    }
    #[test]
    fn refuses_public_parent_before_connection() {
        let p = private();
        let socket = p.join("audio.sock");
        let _listener = UnixListener::bind(&socket).unwrap();
        fs::set_permissions(&socket, fs::Permissions::from_mode(0o600)).unwrap();
        fs::set_permissions(&p, fs::Permissions::from_mode(0o755)).unwrap();
        let o = Command::new(env!("CARGO_BIN_EXE_shr-desk"))
            .args([
                "--audio-local",
                socket.to_str().unwrap(),
                "11111111-1111-4111-8111-111111111111",
                "9",
                "desk-cli",
                "foh",
            ])
            .output()
            .unwrap();
        fs::remove_dir_all(&p).unwrap();
        assert!(!o.status.success());
        assert!(String::from_utf8_lossy(&o.stderr).contains("0700"));
    }
    #[test]
    fn telemetry_and_cached_prior_reply_do_not_clear_pending() {
        let p = private();
        let socket = p.join("audio.sock");
        let script = p.join("operator.txt");
        fs::write(
            &script,
            "grant\ninput-release\nset input-01 fader -3000\nstatus\nrelease\n",
        )
        .unwrap();
        let listener = UnixListener::bind(&socket).unwrap();
        fs::set_permissions(&socket, fs::Permissions::from_mode(0o600)).unwrap();
        let child = std::thread::spawn(move || {
            let (mut stream, _) = listener.accept().unwrap();
            stream
                .set_read_timeout(Some(Duration::from_secs(2)))
                .unwrap();
            let c: Value =
                serde_json::from_str(include_str!("fixtures/gp03/v1/e03-rendered.json")).unwrap();
            stream
                .set_write_timeout(Some(Duration::from_secs(2)))
                .unwrap();
            let mut changed = false;
            let mut ids = Vec::new();
            fn send(s: &mut std::os::unix::net::UnixStream, v: &Value) {
                let b = serde_json::to_vec(v).unwrap();
                s.write_all(&(b.len() as u32).to_be_bytes()).unwrap();
                s.write_all(&b).unwrap();
            }
            loop {
                let mut n = [0; 4];
                match stream.read_exact(&mut n) {
                    Ok(()) => {}
                    Err(e) if e.kind() == std::io::ErrorKind::UnexpectedEof => break,
                    Err(e) => panic!("{e}"),
                };
                let mut b = vec![0; u32::from_be_bytes(n) as usize];
                stream.read_exact(&mut b).unwrap();
                let request: Value = serde_json::from_slice(&b).unwrap();
                match request["kind"].as_str().unwrap() {
                    "snapshot" => {
                        let mut r = reply();
                        if changed {
                            r["snapshot"] = c["final"].clone();
                            r["outcome"]["body"]["revision"] = json!("13");
                        }
                        send(&mut stream, &r);
                    }
                    "grant" => {
                        ids.push(request["request_id"].clone());
                        send(&mut stream, &c["grant_response"]);
                    }
                    "set" => {
                        ids.push(request["request_id"].clone());
                        send(&mut stream, &reply());
                        send(&mut stream, &c["grant_response"]);
                        for mut r in [c["pending"].clone(), c["committed"][0].clone()] {
                            r["context"]["request_id"] = request["request_id"].clone();
                            if !r["outcome"].is_null() {
                                r["outcome"]["request_id"] = request["request_id"].clone();
                            }
                            send(&mut stream, &r);
                        }
                        changed = true;
                    }
                    "release" => {
                        ids.push(request["request_id"].clone());
                        let mut r = c["grant_response"].clone();
                        for k in ["lease", "request_id", "expected_revision"] {
                            r["context"][k] = request[k].clone();
                            r["outcome"][k] = request[k].clone();
                        }
                        for k in ["granted_lease", "lease_remaining_ms", "scope"] {
                            r["outcome"]["body"][k] = Value::Null;
                        }
                        r["outcome"]["body"]["revision"] = json!("13");
                        send(&mut stream, &r);
                    }
                    other => panic!("unexpected {other}"),
                }
            }
            assert_eq!(ids, vec![json!("1"), json!("2"), json!("3")]);
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
        fs::remove_dir_all(&p).unwrap();
        assert!(
            output.status.success(),
            "{}",
            String::from_utf8_lossy(&output.stderr)
        );
        let last = String::from_utf8(output.stdout).unwrap();
        let status: Value = serde_json::from_str(last.lines().last().unwrap()).unwrap();
        assert_eq!(status["revision"], "13");
        assert!(status["pending"].is_null());
        assert!(status["lease_deadline_local_ms"].is_null());
    }
}
