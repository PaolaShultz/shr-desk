//! Exact GP09 DTO consumption; synthetic pipe children never enumerate/open endpoints.
use serde_json::{Value, json};
use shr_desk::roles::{self, Client, Config};
use std::{
    fs,
    os::unix::fs::PermissionsExt,
    sync::{
        Arc,
        atomic::{AtomicBool, AtomicU64, Ordering},
    },
    thread,
    time::{Duration, Instant},
};
fn acquire() -> Value {
    roles::decode_acquire(include_bytes!("fixtures/gp09/v1/acquire-a.json")).unwrap()
}
#[test]
fn exact_grant_verified_registry_cas_is_distinct_and_strict() {
    let a = acquire();
    let lease = roles::decode_grant(
        include_bytes!("fixtures/gp09/v1/grant-a.json"),
        &a["binding"],
    )
    .unwrap();
    let mut verified = json!({"format":"gigpies-role-verified","version":1,"lease":serde_json::from_slice::<Value>(include_bytes!("fixtures/gp09/v1/grant-a.json")).unwrap(),"registry_generation":"2"});
    assert_eq!(
        roles::decode_verified(&serde_json::to_vec(&verified).unwrap(), &lease).unwrap(),
        "2"
    );
    verified["lease"]["generation"] = json!("2");
    assert!(roles::decode_verified(&serde_json::to_vec(&verified).unwrap(), &lease).is_err());
    let duplicate=br#"{"format":"gigpies-role-lease","format":"gigpies-role-lease","version":1,"generation":"1","binding":{}}"#;
    assert!(roles::decode_grant(duplicate, &a["binding"]).is_err());
    let mut wrong = a.clone();
    wrong["binding"]["role"] = json!("lighting-desk");
    assert!(roles::decode_acquire(&serde_json::to_vec(&wrong).unwrap()).is_err());
}
#[test]
fn process_loss_fences_authority_generation_and_no_automatic_reclaim() {
    let dir = std::env::temp_dir().join(format!(
        "desk-role-client-{}-{}",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    ));
    fs::create_dir(&dir).unwrap();
    fs::set_permissions(&dir, fs::Permissions::from_mode(0o700)).unwrap();
    let script = dir.join("fixture-peer");
    let grant = std::str::from_utf8(include_bytes!("fixtures/gp09/v1/grant-a.json"))
        .unwrap()
        .trim();
    // Data-only synthetic peer responses; no producer identity/ownership algorithm.
    fs::write(&script,format!("#!/usr/bin/python3\nimport sys,json,time\nsys.stdin.readline()\ngrant=json.loads({grant:?})\nprint(json.dumps(grant),flush=True)\nsys.stdin.readline()\nprint(json.dumps(dict(format='gigpies-role-verified',version=1,lease=grant,registry_generation='2')),flush=True)\ntime.sleep(.7)\n")).unwrap();
    fs::set_permissions(&script, fs::Permissions::from_mode(0o700)).unwrap();
    let fence = Arc::new(AtomicU64::new(7));
    let live = Arc::new(AtomicBool::new(false));
    let client = Client::start(
        Config {
            executable: script,
            directory: dir.clone(),
            acquire: acquire(),
        },
        fence.clone(),
        live.clone(),
    );
    let end = Instant::now() + Duration::from_secs(3);
    while !live.load(Ordering::Acquire) {
        assert!(Instant::now() < end);
        thread::sleep(Duration::from_millis(5));
    }
    let end = Instant::now() + Duration::from_secs(3);
    while live.load(Ordering::Acquire) {
        assert!(Instant::now() < end);
        thread::sleep(Duration::from_millis(5));
    }
    assert_eq!(fence.load(Ordering::Acquire), 8);
    let status = client.take().unwrap();
    assert!(!status.live);
    assert!(status.message.contains("explicit fresh"));
    drop(client);
    fs::remove_dir_all(dir).unwrap();
}
#[test]
#[ignore = "requires independently accepted actual GP09 binary through SHR_DESK_GP09"]
fn actual_role_provider_verifies_and_loss_is_not_a_replay() {
    let executable = std::env::var_os("SHR_DESK_GP09")
        .expect("accepted role executable")
        .into();
    let dir = std::env::temp_dir().join(format!(
        "desk-role-real-{}-{}",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    ));
    fs::create_dir(&dir).unwrap();
    fs::set_permissions(&dir, fs::Permissions::from_mode(0o700)).unwrap();
    let fence = Arc::new(AtomicU64::new(1));
    let live = Arc::new(AtomicBool::new(false));
    let client = Client::start(
        Config {
            executable,
            directory: dir.clone(),
            acquire: acquire(),
        },
        fence.clone(),
        live.clone(),
    );
    let end = Instant::now() + Duration::from_secs(3);
    while !live.load(Ordering::Acquire) {
        assert!(Instant::now() < end, "{:?}", client.take());
        thread::sleep(Duration::from_millis(10));
    }
    // The other accepted role changes registry CAS without invalidating audio's generation1 lease.
    use std::io::{BufRead, BufReader, Write};
    struct Owned(std::process::Child);
    impl Drop for Owned {
        fn drop(&mut self) {
            let _ = self.0.kill();
            let _ = self.0.wait();
        }
    }
    let mut lighting = Owned(
        std::process::Command::new(std::env::var_os("SHR_DESK_GP09").unwrap())
            .arg(&dir)
            .stdin(std::process::Stdio::piped())
            .stdout(std::process::Stdio::piped())
            .stderr(std::process::Stdio::null())
            .spawn()
            .unwrap(),
    );
    lighting
        .0
        .stdin
        .as_mut()
        .unwrap()
        .write_all(include_bytes!("fixtures/gp09/v1/acquire-b.json"))
        .unwrap();
    let mut grant = String::new();
    BufReader::new(lighting.0.stdout.take().unwrap())
        .read_line(&mut grant)
        .unwrap();
    assert!(grant.contains("lighting-desk"));
    thread::sleep(Duration::from_millis(650));
    let status = client.take().unwrap();
    assert!(status.live);
    assert_eq!(status.generation.as_deref(), Some("1"));
    assert_eq!(status.registry_generation.as_deref(), Some("2"));
    let _ = lighting.0.kill();
    lighting.0.wait().unwrap();
    // Freeze only our owned injected broker: verification must time out and revoke
    // authority within its 500ms polling interval plus 1000ms response deadline.
    let lost_at = Instant::now();
    assert!(
        std::process::Command::new("kill")
            .args(["-STOP", &client.child_pid().unwrap().to_string()])
            .status()
            .unwrap()
            .success()
    );
    while live.load(Ordering::Acquire) {
        assert!(
            lost_at.elapsed() < Duration::from_millis(1700),
            "role loss deadline fence"
        );
        thread::sleep(Duration::from_millis(5));
    }
    let lost = client.take().unwrap();
    assert!(!lost.live);
    assert!(lost.message.contains("deadline"), "{lost:?}");
    assert_eq!(fence.load(Ordering::Acquire), 2);
    drop(client); // joins and kills only the stopped owned child
    let mut fresh_acquire = acquire();
    fresh_acquire["expected_generation"] = json!("2");
    let reclaimed = Client::start(
        Config {
            executable: std::env::var_os("SHR_DESK_GP09").unwrap().into(),
            directory: dir.clone(),
            acquire: fresh_acquire,
        },
        fence.clone(),
        live.clone(),
    );
    let end = Instant::now() + Duration::from_secs(3);
    while !live.load(Ordering::Acquire) {
        assert!(Instant::now() < end, "{:?}", reclaimed.take());
        thread::sleep(Duration::from_millis(5));
    }
    assert_eq!(reclaimed.take().unwrap().generation.as_deref(), Some("3"));
    drop(reclaimed);
    assert!(!live.load(Ordering::Acquire));
    assert_eq!(fence.load(Ordering::Acquire), 3);
    let registry: Value =
        serde_json::from_slice(&fs::read(dir.join("registry.json")).unwrap()).unwrap();
    assert!(registry.to_string().contains("audio-desk"));
    fs::remove_dir_all(dir).unwrap();
}
