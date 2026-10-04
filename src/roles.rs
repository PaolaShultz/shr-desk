//! GP09 process client only. Inventory is explicit injected data, never device discovery.
use crate::provider;
use serde_json::{Value, json};
use std::{
    io::{Read, Write},
    os::fd::AsRawFd,
    path::PathBuf,
    process::{Child, ChildStdin, ChildStdout, Command, Stdio},
    sync::{
        Arc, Mutex,
        atomic::{AtomicBool, AtomicU32, AtomicU64, Ordering},
    },
    thread::{self, JoinHandle},
    time::{Duration, Instant},
};
#[derive(Clone)]
pub struct Config {
    pub executable: PathBuf,
    pub directory: PathBuf,
    pub acquire: Value,
}
#[derive(Clone, Debug)]
pub struct Lease {
    pub generation: String,
    pub binding: Value,
}
pub fn decode_grant(bytes: &[u8], binding: &Value) -> Result<Lease, String> {
    let v = provider::parse(bytes)?;
    provider::keys(&v, &["format", "version", "generation", "binding"])?;
    if v["format"] != "gigpies-role-lease" || v["version"] != 1 || &v["binding"] != binding {
        return Err("role grant format/version/exact binding mismatch".into());
    }
    let generation = v["generation"].as_str().ok_or("role generation")?;
    if provider::counter(generation)? == 0 {
        return Err("zero acquisition generation".into());
    }
    Ok(Lease {
        generation: generation.into(),
        binding: binding.clone(),
    })
}
pub fn decode_verified(bytes: &[u8], lease: &Lease) -> Result<String, String> {
    let v = provider::parse(bytes)?;
    provider::keys(&v, &["format", "version", "lease", "registry_generation"])?;
    if v["format"] != "gigpies-role-verified" || v["version"] != 1 {
        return Err("role verified format/version".into());
    }
    let check = decode_grant(
        &serde_json::to_vec(&v["lease"]).map_err(|e| e.to_string())?,
        &lease.binding,
    )?;
    if check.generation != lease.generation {
        return Err("live acquisition generation mismatch".into());
    }
    let global = v["registry_generation"]
        .as_str()
        .ok_or("registry generation")?;
    if provider::counter(global)? < provider::counter(&lease.generation)? {
        return Err("registry generation precedes acquisition".into());
    }
    Ok(global.into())
}
pub fn decode_acquire(bytes: &[u8]) -> Result<Value, String> {
    let v = provider::parse(bytes)?;
    validate_acquire(&v)?;
    Ok(v)
}
fn validate_acquire(v: &Value) -> Result<(), String> {
    // Exact producer data is passed through; identity/ownership/profile policy stays in the broker.
    provider::keys(
        v,
        &[
            "format",
            "version",
            "expected_generation",
            "binding",
            "inventory",
        ],
    )?;
    if v["format"] != "gigpies-role-acquire" || v["version"] != 1 {
        return Err("role acquire format/version".into());
    }
    provider::counter(
        v["expected_generation"]
            .as_str()
            .ok_or("expected generation")?,
    )?;
    provider::keys(
        &v["binding"],
        &[
            "role",
            "display_connector",
            "display_edid",
            "controller",
            "profile",
            "operator_label",
        ],
    )?;
    if v["binding"]["role"] != "audio-desk" {
        return Err("Desk requires exact audio-desk role".into());
    }
    provider::keys(&v["inventory"], &["displays", "controllers"])?;
    for (name, keys) in [
        ("displays", vec!["connector", "edid", "operator_label"]),
        (
            "controllers",
            vec!["identity", "serial", "profile", "operator_label"],
        ),
    ] {
        let list = v["inventory"][name]
            .as_array()
            .ok_or("role inventory list")?;
        if list.len() > 16 {
            return Err("role inventory capacity".into());
        }
        for d in list {
            provider::keys(d, &keys)?;
        }
    }
    Ok(())
}
#[repr(C)]
struct PollFd {
    fd: i32,
    events: i16,
    revents: i16,
}
unsafe extern "C" {
    fn poll(fds: *mut PollFd, count: usize, timeout: i32) -> i32;
    fn fcntl(fd: i32, cmd: i32, ...) -> i32;
}
fn nonblocking(fd: i32) -> Result<(), String> {
    let flags = unsafe { fcntl(fd, 3) };
    if flags < 0 || unsafe { fcntl(fd, 4, flags | 0x800) } < 0 {
        return Err(std::io::Error::last_os_error().to_string());
    }
    Ok(())
}
fn ready(fd: i32, events: i16, deadline: Instant) -> Result<(), String> {
    loop {
        let left = deadline
            .checked_duration_since(Instant::now())
            .ok_or("role pipe deadline")?;
        let mut p = PollFd {
            fd,
            events,
            revents: 0,
        };
        let n = unsafe { poll(&mut p, 1, left.as_millis().clamp(1, 1000) as i32) };
        if n > 0 {
            return Ok(());
        }
        if n < 0 && std::io::Error::last_os_error().kind() != std::io::ErrorKind::Interrupted {
            return Err("role pipe poll".into());
        }
    }
}
struct Process {
    child: Child,
    input: ChildStdin,
    output: ChildStdout,
    lease: Lease,
}
impl Process {
    fn acquire(config: &Config) -> Result<Self, String> {
        validate_acquire(&config.acquire)?;
        if !config.executable.is_absolute() || !config.directory.is_absolute() {
            return Err("role paths must be absolute".into());
        }
        let mut child = Command::new(&config.executable)
            .arg(&config.directory)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::null())
            .spawn()
            .map_err(|e| e.to_string())?;
        let input = child.stdin.take().ok_or("role stdin")?;
        let output = child.stdout.take().ok_or("role stdout")?;
        let placeholder = Lease {
            generation: "0".into(),
            binding: config.acquire["binding"].clone(),
        };
        let mut p = Self {
            child,
            input,
            output,
            lease: placeholder,
        };
        nonblocking(p.input.as_raw_fd())?;
        nonblocking(p.output.as_raw_fd())?;
        p.send(&config.acquire)?;
        p.lease = decode_grant(&p.receive()?, &config.acquire["binding"])?;
        Ok(p)
    }
    fn send(&mut self, v: &Value) -> Result<(), String> {
        let mut b = serde_json::to_vec(v).map_err(|e| e.to_string())?;
        b.push(b'\n');
        if b.len() > 65536 {
            return Err("role line capacity".into());
        }
        let deadline = Instant::now() + Duration::from_millis(1000);
        let mut remaining = b.as_slice();
        while !remaining.is_empty() {
            ready(self.input.as_raw_fd(), 4, deadline)?;
            match self.input.write(remaining) {
                Ok(0) => return Err("role write EOF".into()),
                Ok(n) => remaining = &remaining[n..],
                Err(e)
                    if matches!(
                        e.kind(),
                        std::io::ErrorKind::WouldBlock | std::io::ErrorKind::Interrupted
                    ) => {}
                Err(e) => return Err(e.to_string()),
            }
        }
        Ok(())
    }
    fn receive(&mut self) -> Result<Vec<u8>, String> {
        let deadline = Instant::now() + Duration::from_millis(1000);
        let mut b = Vec::new();
        loop {
            ready(self.output.as_raw_fd(), 1, deadline)?;
            let mut byte = [0];
            match self.output.read(&mut byte) {
                Ok(0) => return Err("role child EOF".into()),
                Ok(_) => {
                    b.push(byte[0]);
                    if b.len() > 65536 {
                        return Err("role reply capacity".into());
                    }
                    if byte[0] == b'\n' {
                        return Ok(b);
                    }
                }
                Err(e)
                    if matches!(
                        e.kind(),
                        std::io::ErrorKind::WouldBlock | std::io::ErrorKind::Interrupted
                    ) => {}
                Err(e) => return Err(e.to_string()),
            }
        }
    }
    fn verify(&mut self) -> Result<String, String> {
        if self.child.try_wait().map_err(|e| e.to_string())?.is_some() {
            return Err("role child exited".into());
        }
        self.send(&json!({"format":"gigpies-role-command","version":1,"operation":"verify","generation":self.lease.generation,"binding":self.lease.binding}))?;
        decode_verified(&self.receive()?, &self.lease)
    }
}
impl Drop for Process {
    fn drop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}
#[derive(Clone, Debug)]
pub struct Status {
    pub live: bool,
    pub generation: Option<String>,
    pub registry_generation: Option<String>,
    pub message: String,
}
pub struct Client {
    pub live: Arc<AtomicBool>,
    latest: Arc<Mutex<Option<Status>>>,
    stop: Arc<AtomicBool>,
    child: Option<JoinHandle<()>>,
    pid: Arc<AtomicU32>,
}
impl Client {
    pub fn start(config: Config, fence: Arc<AtomicU64>, live: Arc<AtomicBool>) -> Self {
        live.store(false, Ordering::Release);
        let latest = Arc::new(Mutex::new(None));
        let stop = Arc::new(AtomicBool::new(false));
        let (l, u, s) = (live.clone(), latest.clone(), stop.clone());
        let pid = Arc::new(AtomicU32::new(0));
        let child_pid = pid.clone();
        let child = thread::spawn(move || {
            let mut p = match Process::acquire(&config) {
                Ok(p) => p,
                Err(e) => {
                    *u.lock().unwrap() = Some(Status {
                        live: false,
                        generation: None,
                        registry_generation: None,
                        message: format!(
                            "ROLE UNAVAILABLE: {e}; explicit fresh role request required"
                        ),
                    });
                    return;
                }
            };
            child_pid.store(p.child.id(), Ordering::Release);
            let mut next = Instant::now();
            while !s.load(Ordering::Acquire) {
                if Instant::now() >= next {
                    match p.verify() {
                        Ok(global) => {
                            l.store(true, Ordering::Release);
                            *u.lock().unwrap()=Some(Status{live:true,generation:Some(p.lease.generation.clone()),registry_generation:Some(global),message:"LIVE INJECTED GP09 LEASE / descriptors are not physical verification".into()});
                        }
                        Err(e) => {
                            l.store(false, Ordering::Release);
                            let _ = fence.fetch_update(Ordering::AcqRel, Ordering::Acquire, |n| {
                                n.checked_add(1)
                            });
                            *u.lock().unwrap() = Some(Status {
                                live: false,
                                generation: Some(p.lease.generation.clone()),
                                registry_generation: None,
                                message: format!(
                                    "ROLE LOST: {e}; queued intent discarded, explicit fresh role request required"
                                ),
                            });
                            return;
                        }
                    }
                    next = Instant::now() + Duration::from_millis(500);
                }
                thread::sleep(Duration::from_millis(10));
            }
            l.store(false, Ordering::Release);
            let _ = fence.fetch_update(Ordering::AcqRel, Ordering::Acquire, |n| n.checked_add(1));
        });
        Self {
            live,
            latest,
            stop,
            child: Some(child),
            pid,
        }
    }
    pub fn child_pid(&self) -> Option<u32> {
        let pid = self.pid.load(Ordering::Acquire);
        (pid != 0).then_some(pid)
    }
    pub fn take(&self) -> Option<Status> {
        self.latest.lock().unwrap().take()
    }
}
impl Drop for Client {
    fn drop(&mut self) {
        self.live.store(false, Ordering::Release);
        self.stop.store(true, Ordering::Release);
        if let Some(c) = self.child.take() {
            let _ = c.join();
        }
    }
}
