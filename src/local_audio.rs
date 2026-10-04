//! Explicit trusted local Unix endpoint only; no device or TCP transport.
use crate::audio::{self, Request, Session};
use serde_json::{Value, json};
use std::{
    io::{Read, Write},
    os::{
        fd::{AsRawFd, FromRawFd, OwnedFd},
        unix::{
            ffi::OsStrExt,
            fs::{FileTypeExt, MetadataExt},
            net::UnixStream,
        },
    },
    path::Path,
    time::{Duration, Instant},
};
#[repr(C)]
struct Credentials {
    pid: i32,
    uid: u32,
    gid: u32,
}
unsafe extern "C" {
    fn getsockopt(
        fd: i32,
        level: i32,
        opt: i32,
        value: *mut std::ffi::c_void,
        len: *mut u32,
    ) -> i32;
    fn getuid() -> u32;
    fn socket(domain: i32, kind: i32, protocol: i32) -> i32;
    fn connect(fd: i32, address: *const std::ffi::c_void, len: u32) -> i32;
    fn poll(fds: *mut PollFd, count: usize, timeout: i32) -> i32;
}
#[repr(C)]
struct PollFd {
    fd: i32,
    events: i16,
    revents: i16,
}
#[repr(C)]
struct UnixAddress {
    family: u16,
    path: [u8; 108],
}
/// Linux SO_PEERCRED; caller-supplied endpoint never comes from a provider response.
pub struct Transport {
    socket: UnixStream,
}
impl Transport {
    pub fn connect(path: &Path) -> Result<Self, String> {
        Self::connect_until(path, Instant::now() + Duration::from_millis(200))
    }
    pub(crate) fn connect_until(path: &Path, deadline: Instant) -> Result<Self, String> {
        if !path.is_absolute() {
            return Err("endpoint must be absolute".into());
        }
        let parent = path.parent().ok_or("endpoint parent")?;
        if std::fs::canonicalize(parent).map_err(|e| e.to_string())? != parent {
            return Err("endpoint parent must be canonical".into());
        }
        let uid = unsafe { getuid() };
        let p = std::fs::symlink_metadata(parent).map_err(|e| e.to_string())?;
        let s = std::fs::symlink_metadata(path).map_err(|e| e.to_string())?;
        if !p.is_dir()
            || p.uid() != uid
            || p.mode() & 0o777 != 0o700
            || !s.file_type().is_socket()
            || s.uid() != uid
            || s.mode() & 0o777 != 0o600
        {
            return Err("private owned 0700 parent/0600 socket required".into());
        }
        let name = path.as_os_str().as_bytes();
        if name.len() >= 108 || name.contains(&0) {
            return Err("Unix endpoint path length/NUL".into());
        }
        // SAFETY: Linux socket constants; every created descriptor is immediately owned.
        let raw = unsafe { socket(1, 1 | 0x800 | 0x80000, 0) };
        if raw < 0 {
            return Err(std::io::Error::last_os_error().to_string());
        }
        let fd = unsafe { OwnedFd::from_raw_fd(raw) };
        let mut address = UnixAddress {
            family: 1,
            path: [0; 108],
        };
        address.path[..name.len()].copy_from_slice(name);
        if Instant::now() >= deadline {
            return Err("bounded Unix connect deadline".into());
        }
        let connected = unsafe {
            connect(
                fd.as_raw_fd(),
                (&address as *const UnixAddress).cast(),
                (2 + name.len() + 1) as u32,
            )
        };
        if connected != 0 {
            let e = std::io::Error::last_os_error();
            if !matches!(e.raw_os_error(), Some(11 | 115)) {
                return Err(e.to_string());
            }
            let mut ready = PollFd {
                fd: fd.as_raw_fd(),
                events: 4,
                revents: 0,
            };
            let left = deadline
                .checked_duration_since(Instant::now())
                .ok_or("bounded Unix connect deadline")?;
            if unsafe { poll(&mut ready, 1, left.as_millis().clamp(1, 200) as i32) } <= 0 {
                return Err("bounded Unix connect deadline".into());
            }
        }
        let socket = UnixStream::from(fd);
        if let Some(e) = socket.take_error().map_err(|e| e.to_string())? {
            return Err(e.to_string());
        }
        socket.set_nonblocking(false).map_err(|e| e.to_string())?;
        let mut c = Credentials {
            pid: 0,
            uid: u32::MAX,
            gid: u32::MAX,
        };
        let mut n = std::mem::size_of::<Credentials>() as u32;
        // SAFETY: Linux ABI uses a live writable ucred object and socklen_t u32.
        let result = unsafe {
            getsockopt(
                socket.as_raw_fd(),
                1,
                17,
                (&mut c as *mut Credentials).cast(),
                &mut n,
            )
        };
        if result != 0 || n as usize != std::mem::size_of::<Credentials>() || c.uid != uid {
            return Err("same UID peer required".into());
        }
        socket
            .set_read_timeout(Some(Duration::from_millis(100)))
            .map_err(|e| e.to_string())?;
        socket
            .set_write_timeout(Some(Duration::from_millis(100)))
            .map_err(|e| e.to_string())?;
        Ok(Self { socket })
    }
    pub fn send(&mut self, r: &Request) -> Result<(), String> {
        self.send_frame(&r.encode()?)
    }
    pub(crate) fn send_frame(&mut self, b: &[u8]) -> Result<(), String> {
        self.send_frame_until(b, Instant::now() + Duration::from_millis(200))
    }
    pub(crate) fn send_frame_until(&mut self, b: &[u8], deadline: Instant) -> Result<(), String> {
        if b.is_empty() || b.len() > crate::provider::MAX_BYTES {
            return Err("frame capacity".into());
        }
        let mut frame = (b.len() as u32).to_be_bytes().to_vec();
        frame.extend_from_slice(b);
        let deadline = deadline.min(Instant::now() + Duration::from_millis(200));
        let mut bytes = frame.as_slice();
        while !bytes.is_empty() {
            let left = deadline
                .checked_duration_since(Instant::now())
                .ok_or("full-frame write deadline")?;
            self.socket
                .set_write_timeout(Some(left.min(Duration::from_millis(100))))
                .map_err(|e| e.to_string())?;
            match self.socket.write(bytes) {
                Ok(0) => return Err("closed write".into()),
                Ok(n) => bytes = &bytes[n..],
                Err(e) if e.kind() == std::io::ErrorKind::Interrupted => {}
                Err(e) => return Err(e.to_string()),
            }
        }
        Ok(())
    }
    fn receive_available(&mut self) -> Result<Option<Vec<u8>>, String> {
        let mut ready = PollFd {
            fd: self.socket.as_raw_fd(),
            events: 1,
            revents: 0,
        };
        let result = unsafe { poll(&mut ready, 1, 0) };
        if result < 0 {
            return Err(std::io::Error::last_os_error().to_string());
        }
        if result == 0 {
            return Ok(None);
        }
        self.receive()
    }
    pub fn receive(&mut self) -> Result<Option<Vec<u8>>, String> {
        // Frame deadline covers every byte, preventing drip-fed frames from extending idle time.
        self.receive_until(Instant::now() + Duration::from_millis(200))
    }
    pub(crate) fn receive_until(&mut self, deadline: Instant) -> Result<Option<Vec<u8>>, String> {
        let deadline = deadline.min(Instant::now() + Duration::from_millis(200));
        let left = deadline
            .checked_duration_since(Instant::now())
            .ok_or("frame deadline")?;
        self.socket
            .set_read_timeout(Some(left.min(Duration::from_millis(100))))
            .map_err(|e| e.to_string())?;
        let mut prefix = [0; 4];
        match self.socket.read(&mut prefix[..1]) {
            Ok(0) => return Err("provider disconnected".into()),
            Ok(_) => {}
            Err(e)
                if matches!(
                    e.kind(),
                    std::io::ErrorKind::WouldBlock | std::io::ErrorKind::TimedOut
                ) =>
            {
                return Ok(None);
            }
            Err(e) => return Err(e.to_string()),
        }
        self.read_deadline(&mut prefix[1..], deadline)?;
        let size = u32::from_be_bytes(prefix) as usize;
        if size == 0 || size > 65536 {
            return Err("frame length outside1..65536".into());
        }
        let mut body = vec![0; size];
        self.read_deadline(&mut body, deadline)?;
        Ok(Some(body))
    }
    fn read_deadline(&mut self, mut bytes: &mut [u8], deadline: Instant) -> Result<(), String> {
        while !bytes.is_empty() {
            let left = deadline
                .checked_duration_since(Instant::now())
                .ok_or("frame deadline")?;
            self.socket
                .set_read_timeout(Some(left.min(Duration::from_millis(100))))
                .map_err(|e| e.to_string())?;
            match self.socket.read(bytes) {
                Ok(0) => return Err("provider disconnected".into()),
                Ok(n) => {
                    bytes = &mut bytes[n..];
                }
                Err(e) => return Err(e.to_string()),
            }
        }
        Ok(())
    }
}
struct Draft {
    kind: String,
    body: Value,
    revision: String,
    generation: u64,
}
pub struct Operator {
    transport: Transport,
    pub(crate) session: Session,
    start: Instant,
    draft: Option<Draft>,
    scope: String,
    guard: Option<(std::sync::Arc<std::sync::atomic::AtomicU64>, u64)>,
}
impl Operator {
    pub fn connect(
        endpoint: &Path,
        show: &str,
        epoch: u64,
        writer: &str,
        scope: &str,
    ) -> Result<Self, String> {
        Ok(Self {
            transport: Transport::connect(endpoint)?,
            session: Session::new(show, epoch, writer, scope)?,
            start: Instant::now(),
            draft: None,
            scope: scope.into(),
            guard: None,
        })
    }
    pub(crate) fn cancel(&mut self) {
        self.draft = None;
        self.session.context_changed();
    }
    pub(crate) fn guard(
        &mut self,
        generation: std::sync::Arc<std::sync::atomic::AtomicU64>,
        expected: u64,
    ) {
        self.guard = Some((generation, expected));
    }
    fn check_guard(&self) -> Result<(), String> {
        if self
            .guard
            .as_ref()
            .is_some_and(|(g, n)| g.load(std::sync::atomic::Ordering::Acquire) != *n)
        {
            Err("UNCERTAIN: input context revoked; submitted operation may have applied; no further send/retry".into())
        } else {
            Ok(())
        }
    }
    pub(crate) fn review_valid(&self) -> bool {
        self.draft.as_ref().is_some_and(|d| {
            self.session.generation() == d.generation
                && (d.kind != "release_preview"
                    || self
                        .session
                        .preview(self.now())
                        .is_some_and(|p| d.body["token"] == p.token))
                && self
                    .session
                    .snapshot
                    .as_ref()
                    .is_some_and(|s| s.authority.revision == d.revision)
        })
    }
    pub(crate) fn reviewed(&self) -> Option<String> {
        self.draft.as_ref().map(|d| {
            format!(
                "{} {} / revision {} / scope {} / show {} / epoch {}",
                d.kind,
                d.body,
                d.revision,
                self.scope,
                self.session.snapshot_request().context.show_id,
                self.session.snapshot_request().context.epoch
            )
        })
    }
    pub(crate) fn now(&self) -> u64 {
        self.start
            .elapsed()
            .as_millis()
            .try_into()
            .unwrap_or(u64::MAX)
    }
    fn telemetry(&mut self, r: &audio::Reply) -> Result<bool, String> {
        if r.context != self.session.snapshot_request().context {
            return Ok(false);
        }
        if r.state != "final" || r.outcome.as_ref().is_none_or(|o| o.kind != "applied") {
            return Err("snapshot telemetry outcome".into());
        }
        let snapshot = r.snapshot.clone().ok_or("rendered telemetry missing")?;
        self.session.ingest_snapshot(snapshot, self.now())
    }
    pub(crate) fn refresh(&mut self) -> Result<(), String> {
        // Coalesce a bounded raw telemetry batch before validating its newest
        // observation. Discarded frames never update trusted/session state.
        let deadline = Instant::now() + Duration::from_millis(250);
        let mut frames = Vec::new();
        if let Some(bytes) = self.transport.receive_available()? {
            frames.push(bytes);
        } else if self.session.fresh(self.now()) {
            return Ok(());
        } else {
            // Solicited and unsolicited GP03 snapshots have the same null context.
            // Request only after quiet + stale; never request on every poll.
            self.transport.send(&self.session.snapshot_request())?;
            while frames.is_empty() && Instant::now() < deadline {
                if let Some(bytes) = self.transport.receive()? {
                    frames.push(bytes);
                }
            }
        }
        let mut drained = false;
        while frames.len() < 64 && Instant::now() < deadline {
            if let Some(bytes) = self.transport.receive_available()? {
                frames.push(bytes);
            } else {
                drained = true;
                break;
            }
        }
        if !drained {
            return Err("snapshot backlog saturated; freshness not admitted".into());
        }
        for bytes in frames.into_iter().rev() {
            if Instant::now() >= deadline {
                break;
            }
            let r = audio::decode_reply(&bytes)?;
            if r.context == self.session.snapshot_request().context {
                if self.telemetry(&r)? {
                    return Ok(());
                }
            } else if r.context.show_id != self.session.snapshot_request().context.show_id
                || r.context.epoch != self.session.snapshot_request().context.epoch
            {
                return Err("unrelated snapshot session".into());
            }
        }
        if self.session.fresh(self.now()) {
            Ok(())
        } else {
            Err("snapshot deadline/queue bound".into())
        }
    }
    pub(crate) fn mutate(&mut self, kind: &str, body: Value) -> Result<(), String> {
        self.refresh()?;
        if self.session.renewal_due(self.now()) && kind != "renew" {
            self.mutate_inner("renew", json!({}))?;
            self.refresh()?;
        }
        self.mutate_inner(kind, body)
    }
    pub(crate) fn mutate_inner(&mut self, kind: &str, body: Value) -> Result<(), String> {
        let r = self.session.begin(kind, body, self.now())?;
        self.check_guard()?;
        self.transport.send(&r)?;
        let deadline = Instant::now() + Duration::from_millis(1900);
        loop {
            self.check_guard()?;
            let mut idle = false;
            match self.transport.receive() {
                Ok(Some(b)) => match audio::decode_reply(&b) {
                    Ok(reply) => {
                        if self.telemetry(&reply)? {
                        } else if self
                            .session
                            .pending
                            .as_ref()
                            .is_some_and(|p| p.request.context == reply.context)
                        {
                            if let Err(e) = self.session.accept(reply, self.now()) {
                                eprintln!("refused reply; pending retained: {e}");
                            }
                            if self.session.pending.is_none() {
                                if !self.session.fresh(self.now()) {
                                    self.refresh()?;
                                }
                                return Ok(());
                            }
                        } else {
                            eprintln!(
                                "uncorrelated or cached prior reply ignored; pending retained"
                            );
                        }
                    }
                    Err(e) => eprintln!("invalid reply refused; pending retained: {e}"),
                },
                Ok(None) => idle = true,
                Err(e) => {
                    self.session.disconnect();
                    return Err(format!("uncertain operation; transport closed: {e}"));
                }
            }
            if let Some(retry) = self.session.retry(self.now()) {
                self.check_guard()?;
                self.transport.send(&retry)?;
            }
            if Instant::now() >= deadline {
                return Err("pending/uncertain deadline; no applied claim".into());
            }
            // Buffered telemetry/cached replies must not delay the correlated reply.
            // Deadlines and retry guards still apply to every iteration.
            if idle {
                std::thread::sleep(Duration::from_millis(10));
            }
        }
    }
    fn wait(&mut self, ms: u64) -> Result<(), String> {
        if ms > 2000 {
            return Err("wait maximum2000ms".into());
        }
        let deadline = Instant::now() + Duration::from_millis(ms);
        while Instant::now() < deadline {
            self.refresh()?;
            if self.session.renewal_due(self.now()) {
                self.mutate_inner("renew", json!({}))?;
            }
            std::thread::sleep(
                deadline
                    .saturating_duration_since(Instant::now())
                    .min(Duration::from_millis(50)),
            );
        }
        self.refresh()
    }
    pub(crate) fn stage(&mut self, kind: &str, body: Value) -> Result<(), String> {
        if self.draft.is_some() {
            return Err("confirm or cancel existing draft".into());
        }
        if !self.session.fresh(self.now()) {
            self.refresh()?;
        }
        let s = self.session.snapshot.as_ref().ok_or("snapshot")?;
        println!(
            "CONFIRM exact scope/context/targets: {kind} {body} scope={} show={} epoch={} revision={} frame={}",
            self.scope, s.authority.show_id, s.authority.epoch, s.authority.revision, s.frame
        );
        self.draft = Some(Draft {
            kind: kind.into(),
            body,
            revision: s.authority.revision.clone(),
            generation: self.session.generation(),
        });
        Ok(())
    }
    pub(crate) fn confirm(&mut self) -> Result<(), String> {
        let d = self
            .draft
            .take()
            .ok_or("no confirmation draft; held/repeated confirm refused")?;
        if !self.session.fresh(self.now()) {
            self.refresh()?;
        }
        let s = self.session.snapshot.as_ref().ok_or("snapshot")?;
        if s.authority.revision != d.revision || self.session.generation() != d.generation {
            return Err("context/revision changed; draft discarded".into());
        }
        if self.session.renewal_due(self.now()) {
            self.mutate_inner("renew", json!({}))?;
            self.refresh()?;
            if self
                .session
                .snapshot
                .as_ref()
                .is_none_or(|s| s.authority.revision != d.revision)
                || self.session.generation() != d.generation
            {
                return Err("reviewed confirmation context changed during renewal".into());
            }
        }
        // No refresh between reviewed revision and begin; the immutable request pins it.
        self.mutate_inner(&d.kind, d.body)
    }
    pub fn status(&self) -> Value {
        let s = self.session.snapshot.as_ref();
        json!({"mode":"real-local-offline-provider","scope":self.scope,"faulted":s.map(|s|s.faulted),"authority":s.map(|s|json!({"show_id":s.authority.show_id,"epoch":s.authority.epoch,"inputs":s.authority.inputs,"monitors":s.authority.monitors,"modes":s.authority.modes,"parameters":s.authority.parameters.iter().map(|p|json!({"target":audio::target_value(&p.target),"actual_integer_db":null,"target_value":p.target_value,"proposal":p.proposal,"hold":p.hold,"owner":p.owner})).collect::<Vec<_>>()})),"fresh":self.session.fresh(self.now()),"lease_deadline_local_ms":self.session.lease_deadline(),"result":self.session.last_result,"pending":self.session.pending.as_ref().map(|p|format!("{:?}",p.state)),"frame":s.map(|s|&s.frame),"revision":s.map(|s|&s.authority.revision),"protection":"offline-unprotected","meters":null,"coefficients":s.map(|s|s.coefficients.iter().map(|c|json!({"input":c.input,"current_nanogain":c.current_nanogain.iter().map(|n|n.0).collect::<Vec<_>>(),"ramp_target_nanogain":c.ramp_target_nanogain.iter().map(|n|n.0).collect::<Vec<_>>()})).collect::<Vec<_>>())})
    }
}
/// Bounded script input avoids a blocking stdin read starving a granted lease.
/// No writer is granted before the operator's explicit `grant` command.
pub fn run(args: &[String]) -> Result<(), String> {
    if args.len() != 5 && args.len() != 7 {
        return Err(
            "usage: --audio-local ENDPOINT SHOW EPOCH WRITER SCOPE [--script FILE|-]".into(),
        );
    }
    let mut commands = String::new();
    if args.len() == 7 {
        if args[5] != "--script" {
            return Err("expected --script".into());
        }
        if args[6] == "-" {
            let deadline = Instant::now() + Duration::from_secs(10);
            let mut bytes = Vec::new();
            let mut input = std::io::stdin().lock();
            loop {
                if Instant::now() >= deadline {
                    return Err("stdin batch EOF deadline10s; no connection/grant made".into());
                }
                let mut ready = PollFd {
                    fd: 0,
                    events: 1,
                    revents: 0,
                };
                let result = unsafe { poll(&mut ready, 1, 100) };
                if result < 0 {
                    return Err(std::io::Error::last_os_error().to_string());
                }
                if result == 0 {
                    continue;
                }
                let mut chunk = [0u8; 1024];
                let n = input.read(&mut chunk).map_err(|e| e.to_string())?;
                if n == 0 {
                    break;
                }
                bytes.extend_from_slice(&chunk[..n]);
                if bytes.len() > 65536 {
                    return Err("stdin batch exceeds64KiB".into());
                }
            }
            commands = String::from_utf8(bytes).map_err(|_| "stdin UTF8")?;
        } else {
            if !std::fs::metadata(&args[6])
                .map_err(|e| e.to_string())?
                .is_file()
            {
                return Err("script must be regular file".into());
            }
            std::fs::File::open(&args[6])
                .map_err(|e| e.to_string())?
                .take(65537)
                .read_to_string(&mut commands)
                .map_err(|e| e.to_string())?;
        }
    }
    if commands.len() > 65536 || commands.lines().count() > 256 {
        return Err("script exceeds64KiB/256commands".into());
    }
    let mut op = Operator {
        transport: Transport::connect(Path::new(&args[0]))?,
        session: Session::new(
            &args[1],
            args[2].parse().map_err(|_| "epoch")?,
            &args[3],
            &args[4],
        )?,
        start: Instant::now(),
        draft: None,
        scope: args[4].clone(),
        guard: None,
    };
    op.refresh()?;
    let session_deadline = Instant::now() + Duration::from_secs(30);
    for line in commands.lines() {
        if Instant::now() >= session_deadline {
            return Err("script30sdeadline".into());
        }
        let w: Vec<_> = line.split_whitespace().collect();
        match w.as_slice(){
            []=>{},["status"]=>{op.refresh()?;println!("{}",op.status());},
            ["snapshot"]=>op.refresh()?,
            ["wait",ms]=>op.wait(ms.parse().map_err(|_|"wait milliseconds")?)?,
            ["grant"]=>op.mutate("grant",json!({"scope":args[4]}))?,
            ["input-release"]=>op.session.input_released(),
            ["renew"]|["release"]=>op.mutate(w[0],json!({}))?,
            ["set",input,parameter,value]=>{
                let v=if *parameter=="mute" {match *value{"true"=>json!(true),"false"=>json!(false),_=>return Err("mute requires true/false".into())}}else{json!(value.parse::<i64>().map_err(|_|"integer value")?)};
                op.mutate("set",json!({"targets":[{"target":{"input":input,"parameter":parameter},"value":v}]}))?;
            },
            ["send",input,monitor,value]=>op.mutate("set",json!({"targets":[{"target":{"input":input,"parameter":"send","monitor":monitor},"value":value.parse::<i64>().map_err(|_|"integer send")?}]}))?,
            ["mode",mode]=>op.stage("set_mode",json!({"mode":mode,"bounds":[]}))?,
            ["confirm"]=>op.confirm()?,
            ["cancel"]=>{if op.draft.take().is_some_and(|d|d.kind=="release_preview") && let Some(preview)=op.session.preview(op.now()).cloned(){op.mutate("cancel_preview",json!({"token":preview.token}))?;}},
            ["preview",input,parameter]=>{op.mutate("preview_release",json!({"targets":[{"input":input,"parameter":parameter}]}))?;let preview=op.session.preview(op.now()).ok_or("preview expired/unavailable")?.clone();println!("ENGINE PREVIEW destinations={:?} ramp={} token={}",preview.destinations,preview.ramp_frames,preview.token);op.stage("release_preview",json!({"token":preview.token}))?;},
            ["preview-send",input,monitor]=>{op.mutate("preview_release",json!({"targets":[{"input":input,"parameter":"send","monitor":monitor}]}))?;let preview=op.session.preview(op.now()).ok_or("preview expired/unavailable")?.clone();println!("ENGINE PREVIEW destinations={:?} ramp={} token={}",preview.destinations,preview.ramp_frames,preview.token);op.stage("release_preview",json!({"token":preview.token}))?;},
            ["context-reset"]=>{op.draft=None;op.session.context_changed();},
            ["json",kind,rest @ ..]=>{let body=audio::decode_command_body(rest.join(" ").as_bytes())?;if matches!(*kind,"set_mode"|"release_preview"){op.stage(kind,body)?;}else{op.mutate(kind,body)?;}},
            ["export",path]=>std::fs::write(path,format!("{}\n",op.status())).map_err(|e|e.to_string())?,
            _=>return Err(format!("unknown operator command: {line}")),
        }
    }
    println!("{}", op.status());
    Ok(())
}

#[cfg(test)]
mod refresh_tests {
    use super::*;
    use std::{
        fs,
        io::{Read, Write},
        os::unix::{fs::PermissionsExt, net::UnixListener},
        sync::mpsc,
        thread,
    };

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
            let corpus: Value =
                serde_json::from_str(include_str!("../tests/fixtures/gp03/v1/e03-rendered.json"))
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
}
