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
/// Framed authority connection. Implementations authenticate their endpoint before
/// construction and preserve bounded whole-frame deadlines. This seam carries
/// bytes only: leases, freshness, correlation and no-replay stay in `Session`.
/// `None` means no frame before the deadline, never a partial decoded message.
pub trait AuthorityConnection: Send {
    fn send_frame_until(&mut self, bytes: &[u8], deadline: Instant) -> Result<(), String>;
    fn receive_until(&mut self, deadline: Instant) -> Result<Option<Vec<u8>>, String>;
    fn receive_available(&mut self) -> Result<Option<Vec<u8>>, String>;
    fn send(&mut self, request: &Request) -> Result<(), String> {
        self.send_frame_until(
            &request.encode()?,
            Instant::now() + Duration::from_millis(200),
        )
    }
    fn receive(&mut self) -> Result<Option<Vec<u8>>, String> {
        self.receive_until(Instant::now() + Duration::from_millis(200))
    }
}
impl AuthorityConnection for Transport {
    fn send_frame_until(&mut self, bytes: &[u8], deadline: Instant) -> Result<(), String> {
        Transport::send_frame_until(self, bytes, deadline)
    }
    fn receive_until(&mut self, deadline: Instant) -> Result<Option<Vec<u8>>, String> {
        Transport::receive_until(self, deadline)
    }
    fn receive_available(&mut self) -> Result<Option<Vec<u8>>, String> {
        Transport::receive_available(self)
    }
}
pub struct Operator {
    transport: Box<dyn AuthorityConnection>,
    pub(crate) session: Session,
    start: Instant,
    draft: Option<Draft>,
    scope: String,
    brain_signal: Option<std::sync::Arc<crate::brain::HoldSignal>>,
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
        Self::connect_version(endpoint, show, epoch, writer, scope, 1)
    }
    pub fn connect_remote(
        config: &crate::remote::Config,
        show: &str,
        epoch: u64,
        scope: &str,
    ) -> Result<Self, String> {
        let connection = crate::remote::Connection::connect(config, scope)?;
        if connection.source_epoch != epoch {
            return Err("authenticated source epoch differs from expected epoch".into());
        }
        let writer = connection.writer.clone();
        // QUIC has already reassembled the outer Response into a complete payload.
        // Applying the Unix frame assembler again would reject documents >64 KiB.
        Self::from_document_connection(Box::new(connection), show, epoch, &writer, scope, 2)
    }
    pub fn connect_version(
        endpoint: &Path,
        show: &str,
        epoch: u64,
        writer: &str,
        scope: &str,
        version: u8,
    ) -> Result<Self, String> {
        Self::from_connection_version(
            Box::new(Transport::connect(endpoint)?),
            show,
            epoch,
            writer,
            scope,
            version,
        )
    }
    /// Attach a previously authenticated connection read-only. Callers must use
    /// the authenticated writer identity; no grants or previous intent are copied.
    pub fn from_connection(
        transport: Box<dyn AuthorityConnection>,
        show: &str,
        epoch: u64,
        writer: &str,
        scope: &str,
    ) -> Result<Self, String> {
        Self::from_connection_version(transport, show, epoch, writer, scope, 1)
    }
    pub fn from_connection_version(
        transport: Box<dyn AuthorityConnection>,
        show: &str,
        epoch: u64,
        writer: &str,
        scope: &str,
        version: u8,
    ) -> Result<Self, String> {
        let transport = if version == 2 {
            Box::new(crate::pages::Connection::new(transport)) as Box<dyn AuthorityConnection>
        } else {
            transport
        };
        Self::from_document_connection(transport, show, epoch, writer, scope, version)
    }
    fn from_document_connection(
        transport: Box<dyn AuthorityConnection>,
        show: &str,
        epoch: u64,
        writer: &str,
        scope: &str,
        version: u8,
    ) -> Result<Self, String> {
        Ok(Self {
            transport,
            session: Session::new_version(show, epoch, writer, scope, version)?,
            start: Instant::now(),
            draft: None,
            scope: scope.into(),
            guard: None,
            brain_signal: None,
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
    pub(crate) fn brain_signal(&mut self, signal: std::sync::Arc<crate::brain::HoldSignal>) {
        self.brain_signal = Some(signal);
    }
    fn check_guard(&mut self) -> Result<(), String> {
        if self
            .brain_signal
            .as_ref()
            .is_some_and(|s| s.close.swap(false, std::sync::atomic::Ordering::AcqRel))
        {
            // Best effort bounded priority close, even when ordinary mutation is pending.
            // Heartbeats stop at the UI signal immediately; provider deadman is final bound.
            let _ = self.close_brain();
        }
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
                && (d.kind != "processing_set" || self.session.processing_fresh(self.now()))
                && (!crate::structure::is_kind(&d.kind)
                    || self.session.structural_fresh(self.now()))
                && (d.kind != "device_configure"
                    || self
                        .session
                        .validate_device_intent(&d.body, self.now())
                        .is_ok())
                && (!crate::brain::is_kind(&d.kind) || self.session.brain_fresh(self.now()))
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
            if d.kind=="device_configure" {return format!("DEVICE CONFIGURATION / revision {} / separate rearm required\n{}",d.revision,serde_json::to_string_pretty(&d.body["config"]).unwrap_or_default());}
            if d.kind == "processing_set" {
                let config = crate::processing::decode_config(&d.body["config"]).expect("validated draft");
                return format!("APPLY channel {} / FOH EQ then compressor / monitors raw-post-mute unchanged / revision {} / show {} / epoch {}\n{}\n240-frame output crossfade; Enter confirms complete replacement; Esc cancels",
                    d.body["input"], d.revision, self.session.snapshot_request().context.show_id, self.session.snapshot_request().context.epoch,
                    crate::processing::FIELDS.iter().map(|f| config.display(*f)).collect::<Vec<_>>().join("\n"));
            }
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
    /// Dispatch by contract before touching the shared correlation domain.
    fn processing_frame(&mut self, bytes: &[u8]) -> Result<bool, String> {
        // Discriminator only: skipped content is never trusted. The selected
        // strict decoder still checks every field, duplicate and depth bound.
        #[derive(serde::Deserialize)]
        struct Contract {
            contract: Option<String>,
        }
        let tag: Contract = serde_json::from_slice(bytes).map_err(|e| e.to_string())?;
        if tag.contract.as_deref() == Some("GP15-device") {
            self.session.dispatch_device(bytes, self.now())?;
            return Ok(true);
        }
        if tag.contract.as_deref() == Some("GP15-brain") {
            let reply = crate::brain::decode_reply(bytes)?;
            self.session.dispatch_brain(reply, self.now())?;
            return Ok(true);
        }
        if tag.contract.as_deref() == Some("GP14-structure") {
            let reply = crate::structure::decode_reply(bytes)?;
            if reply.context == self.session.structural_request().context {
                if reply.state != "snapshot" {
                    return Err(format!("structural query unavailable: {:?}", reply.reason));
                }
                self.session.ingest_structural(
                    reply
                        .snapshot
                        .ok_or_else(|| format!("structural unavailable: {:?}", reply.reason))?,
                    self.now(),
                )?;
            } else {
                self.session.dispatch_structural(reply, self.now())?;
            }
            return Ok(true);
        }
        if tag.contract.as_deref() != Some("GP07-processing") {
            return Ok(false);
        }
        let r = crate::processing::decode_reply(bytes)?;
        if r.version != self.session.snapshot_request().version + 1 {
            return Err("processing version differs from selected session".into());
        }
        if r.context == self.session.processing_request().context {
            if let Some(s) = r.snapshot {
                self.session.ingest_processing(s, self.now())?;
            } else {
                return Err(format!("processing unavailable: {:?}", r.reason));
            }
        } else if self
            .session
            .pending
            .as_ref()
            .is_some_and(|p| p.request.kind == "processing_set" && p.request.context == r.context)
        {
            self.session.accept_processing(r, self.now())?;
        } else if r.context.show_id != self.session.snapshot_request().context.show_id
            || r.context.epoch != self.session.snapshot_request().context.epoch
        {
            return Err("unrelated processing session".into());
        }
        // Cached old replies never ingest snapshots or renew freshness.
        Ok(true)
    }
    pub(crate) fn refresh_device(&mut self) -> Result<(), String> {
        self.check_guard()?;
        let deadline = Instant::now() + Duration::from_millis(250);
        self.transport
            .send_frame_until(&self.session.device_request().encode()?, deadline)?;
        for _ in 0..64 {
            self.check_guard()?;
            if Instant::now() >= deadline {
                break;
            }
            if let Some(bytes) = self.transport.receive_until(deadline)? {
                let v: serde_json::Value =
                    serde_json::from_slice(&bytes).map_err(|e| e.to_string())?;
                let observation = v["contract"] == "GP15-device" && v["state"] == "snapshot";
                if Instant::now() >= deadline {
                    break;
                }
                if !self.processing_frame(&bytes)? {
                    let r = audio::decode_reply(&bytes)?;
                    self.telemetry(&r)?;
                }
                if observation {
                    return Ok(());
                }
            }
        }
        self.session.invalidate_device();
        Err("device observation deadline".into())
    }
    pub(crate) fn refresh_brain(&mut self) -> Result<(), String> {
        let result = self.refresh_brain_inner();
        if result.is_err() {
            self.session.invalidate_brain_observation();
        }
        result
    }
    fn refresh_brain_inner(&mut self) -> Result<(), String> {
        self.check_guard()?;
        let deadline = Instant::now() + Duration::from_millis(250);
        self.transport
            .send_frame_until(&self.session.snapshot_request().encode()?, deadline)?;
        self.transport
            .send_frame_until(&self.session.brain_request().encode()?, deadline)?;
        let previous = self
            .session
            .brain
            .as_ref()
            .map(|s| (s.revision.clone(), s.frame.clone()));
        let mut requested_revision = None;
        for _ in 0..64 {
            self.check_guard()?;
            if Instant::now() >= deadline {
                break;
            }
            if let Some(bytes) = self.transport.receive_until(deadline)? {
                if Instant::now() >= deadline {
                    break;
                }
                if !self.processing_frame(&bytes)? {
                    let r = audio::decode_reply(&bytes)?;
                    self.telemetry(&r)?;
                }
                if let (Some(raw), Some(brain)) = (&self.session.snapshot, &self.session.brain)
                    && raw.authority.revision != brain.revision
                    && requested_revision.as_ref() != Some(&brain.revision)
                {
                    requested_revision = Some(brain.revision.clone());
                    self.transport
                        .send_frame_until(&self.session.snapshot_request().encode()?, deadline)?;
                }
                if self.session.brain_fresh(self.now())
                    && self
                        .session
                        .brain
                        .as_ref()
                        .map(|s| (s.revision.clone(), s.frame.clone()))
                        != previous
                {
                    return Ok(());
                }
            }
        }
        Err("Brain paired observation deadline".into())
    }
    pub(crate) fn close_brain(&mut self) -> Result<(), String> {
        if self
            .session
            .brain
            .as_ref()
            .is_none_or(|b| b.held_generation.is_none())
            && self.session.pending.as_ref().is_none_or(|p| {
                !matches!(p.request.kind.as_str(), "brain_hold" | "brain_heartbeat")
            })
        {
            return Ok(());
        }
        let r = self.session.brain_close_request(self.now())?;
        self.transport
            .send_frame_until(&r.encode()?, Instant::now() + Duration::from_millis(20))
    }
    /// Send one ephemeral edge; worker polls completion, never retries a hold/heartbeat.
    pub(crate) fn brain_edge(&mut self, kind: &str, generation: u64) -> Result<(), String> {
        let body = if kind == "brain_heartbeat" {
            if self
                .session
                .brain_age(self.now())
                .is_none_or(|age| age > 50)
            {
                return Err("heartbeat needs observation no older than 50 ms".into());
            }
            json!({"generation":generation.to_string(),"observed_frame":self.session.brain.as_ref().ok_or("Brain readback")?.frame})
        } else {
            json!({"generation":generation.to_string()})
        };
        let r = self.session.begin(kind, body, self.now())?;
        self.transport
            .send_frame_until(&r.encode()?, Instant::now() + Duration::from_millis(20))
    }
    pub(crate) fn refresh_structural(&mut self) -> Result<(), String> {
        let result = self.refresh_structural_inner();
        if result.is_err() {
            // A later raw-only poll cannot turn a failed pair into fresh structure.
            self.session.invalidate_structural_observation();
        }
        result
    }
    fn refresh_structural_inner(&mut self) -> Result<(), String> {
        self.check_guard()?;
        let started = Instant::now();
        let deadline = started + Duration::from_millis(250);
        let trace = std::env::var_os("GP14_TIMING").is_some();
        // Raw state can expire while the structural response is in flight. Request
        // both observations within one budget; never wait for an unrequested raw reply.
        self.transport
            .send_frame_until(&self.session.snapshot_request().encode()?, deadline)?;
        if trace {
            eprintln!("GP14 pair raw_sent_us={}", started.elapsed().as_micros());
        }
        self.transport
            .send_frame_until(&self.session.structural_request().encode()?, deadline)?;
        if trace {
            eprintln!(
                "GP14 pair sent elapsed_us={}",
                started.elapsed().as_micros()
            );
        }
        let (mut raw, mut structural) = (false, false);
        let mut count = 0;
        while Instant::now() < deadline && count < 64 {
            self.check_guard()?;
            if let Some(bytes) = self.transport.receive_until(deadline)? {
                count += 1;
                let received = started.elapsed();
                #[derive(serde::Deserialize)]
                struct Tag {
                    contract: Option<String>,
                    state: Option<String>,
                }
                let tag: Tag = serde_json::from_slice(&bytes).map_err(|e| e.to_string())?;
                if tag.contract.as_deref() == Some("GP14-structure")
                    && tag.state.as_deref() == Some("snapshot")
                {
                    let reply = crate::structure::decode_reply(&bytes)?;
                    if reply.context != self.session.structural_request().context {
                        return Err("structural query context".into());
                    }
                    if trace {
                        eprintln!(
                            "GP14 structural decoded_us={} incoming_revision={} incoming_frame={:?}",
                            started.elapsed().as_micros(),
                            reply.revision,
                            reply.snapshot.as_ref().map(|s| &s.frame)
                        );
                    }
                    if Instant::now() >= deadline {
                        break;
                    }
                    structural |= self.session.ingest_structural(
                        reply.snapshot.ok_or("structural readback missing")?,
                        self.now(),
                    )?;
                } else if !self.processing_frame(&bytes)? {
                    let reply = audio::decode_reply(&bytes)?;
                    if trace {
                        eprintln!(
                            "GP14 raw decoded_us={} incoming={:?}",
                            started.elapsed().as_micros(),
                            reply.snapshot.as_ref().map(|s| (
                                &s.authority.sequence,
                                &s.authority.revision,
                                &s.frame
                            ))
                        );
                    }
                    if Instant::now() >= deadline {
                        break;
                    }
                    raw |= self.telemetry(&reply)?;
                }
                if trace {
                    eprintln!(
                        "GP14 pair frame={count} bytes={} contract={:?} received_us={} decoded_us={} total_us={} raw={raw} structural={structural} coherent_fresh={}",
                        bytes.len(),
                        tag.contract,
                        received.as_micros(),
                        (started.elapsed() - received).as_micros(),
                        started.elapsed().as_micros(),
                        self.session.structural_fresh(self.now())
                    );
                    eprintln!(
                        "GP14 admitted raw={:?} structural={:?}",
                        self.session.snapshot.as_ref().map(|s| (
                            &s.authority.sequence,
                            &s.authority.revision,
                            &s.frame
                        )),
                        self.session
                            .structural
                            .as_ref()
                            .map(|s| (&s.revision, &s.frame))
                    );
                }
                if raw
                    && structural
                    && Instant::now() < deadline
                    && self.session.structural_fresh(self.now())
                {
                    return Ok(());
                }
            }
        }
        Err(format!(
            "structural snapshot deadline/queue bound (frames={count} elapsed_ms={} raw={raw} structural={structural} coherent_fresh={})",
            started.elapsed().as_millis(),
            self.session.structural_fresh(self.now())
        ))
    }
    pub(crate) fn refresh_processing(&mut self) -> Result<(), String> {
        self.check_guard()?;
        self.transport.send(&self.session.processing_request())?;
        let previous = self.session.processing.as_ref().map(|s| s.sequence.clone());
        let deadline = Instant::now() + Duration::from_millis(250);
        let mut count = 0;
        let mut latest_raw = None;
        while Instant::now() < deadline && count < 64 {
            self.check_guard()?;
            if let Some(bytes) = self.transport.receive_until(deadline)? {
                count += 1;
                if !self.processing_frame(&bytes)? {
                    latest_raw = Some(bytes);
                }
                if self.session.processing.as_ref().map(|s| &s.sequence) != previous.as_ref() {
                    if let Some(bytes) = latest_raw {
                        self.telemetry(&audio::decode_reply(&bytes)?)?;
                    }
                    return Ok(());
                }
            }
        }
        if let Some(bytes) = latest_raw {
            self.telemetry(&audio::decode_reply(&bytes)?)?;
        }
        Err(format!(
            "processing snapshot deadline/queue bound ({count} frames)"
        ))
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
        // Contract replies are state transitions, not coalescible telemetry:
        // pending must precede final, including the independent priority-close lane.
        // Dispatch them before raw ingestion so a final's needs_snapshot flag is
        // cleared only by a subsequently validated raw observation. In particular,
        // do not admit the newest raw revision first and suppress older contract
        // observations that still belong to this wire-ordered transaction batch.
        let mut raw_frames = Vec::new();
        for bytes in frames {
            if Instant::now() >= deadline {
                return Err("snapshot deadline/queue bound".into());
            }
            if !self.processing_frame(&bytes)? {
                raw_frames.push(bytes);
            }
        }
        // Preserve newest-first raw coalescing, including fallback past regressive
        // observations. Discarded raw frames never touch trusted/session state.
        for bytes in raw_frames.into_iter().rev() {
            if Instant::now() >= deadline {
                return Err("snapshot deadline/queue bound".into());
            }
            let r = audio::decode_reply(&bytes)?;
            if Instant::now() >= deadline {
                return Err("snapshot deadline/queue bound".into());
            }
            if r.context == self.session.snapshot_request().context {
                if self.telemetry(&r)? {
                    break;
                }
            } else if r.context.show_id != self.session.snapshot_request().context.show_id
                || r.context.epoch != self.session.snapshot_request().context.epoch
            {
                return Err("unrelated snapshot session".into());
            }
        }
        if Instant::now() < deadline && self.session.fresh(self.now()) {
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
                Ok(Some(b)) => {
                    if self.processing_frame(&b)? {
                        if self.session.pending.is_none() {
                            if kind == "device_configure"
                                && self
                                    .session
                                    .device_final
                                    .as_ref()
                                    .is_some_and(|r| r.state != "applied_device")
                            {
                                return Err(
                                    "device configuration failed; actual device not applied".into(),
                                );
                            }
                            if crate::brain::is_kind(kind)
                                && let Some(reason) = self
                                    .session
                                    .brain_final
                                    .as_ref()
                                    .and_then(|r| r.reason.as_ref())
                            {
                                return Err(format!("Brain refused: {reason}"));
                            }
                            // Structural map commits may deliberately close the old
                            // authenticated session after its final. Preserve that
                            // correlated completion independently of the next refresh.
                            if !crate::structure::is_kind(kind) {
                                self.refresh()?;
                            }
                            return Ok(());
                        }
                    } else {
                        match audio::decode_reply(&b) {
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
                                }
                            }
                            Err(e) => eprintln!("invalid reply refused; pending retained: {e}"),
                        }
                    }
                }
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
        if kind == "device_configure" {
            self.session.validate_device_intent(&body, self.now())?;
        }
        // Pin the queued review's context before any refresh can observe a newer
        // revision. The frontend already checked its original queued revision.
        let processing_context = if kind == "processing_set"
            || crate::structure::is_kind(kind)
            || crate::brain::is_kind(kind)
            || kind == "device_configure"
        {
            Some((
                self.session
                    .snapshot
                    .as_ref()
                    .ok_or("snapshot")?
                    .authority
                    .revision
                    .clone(),
                self.session.generation(),
            ))
        } else {
            None
        };
        if !self.session.fresh(self.now()) {
            self.refresh()?;
        }
        if let Some((revision, generation)) = processing_context {
            if kind == "processing_set" {
                crate::processing::validate_body_version(
                    &body,
                    self.session.snapshot_request().version,
                )?;
                self.refresh_processing()?;
            } else if kind == "device_configure" {
                self.refresh_device()?;
                self.session.validate_device_intent(&body, self.now())?;
            } else if crate::brain::is_kind(kind) {
                self.refresh_brain()?;
                let raw = self.session.snapshot.as_ref().ok_or("snapshot")?;
                crate::brain::validate_body(
                    kind,
                    &body,
                    self.session.brain.as_ref(),
                    raw.authority.inputs.len(),
                    raw.authority.monitors.len(),
                )?;
            } else {
                self.refresh_structural()?;
                crate::structure::validate_body(kind, &body, self.session.structural.as_ref())?;
            }
            self.refresh()?;
            if self.session.generation() != generation
                || self
                    .session
                    .snapshot
                    .as_ref()
                    .is_none_or(|s| s.authority.revision != revision)
            {
                return Err("processing review context changed during paired refresh".into());
            }
            if (kind == "processing_set" && !self.session.processing_fresh(self.now()))
                || (crate::structure::is_kind(kind) && !self.session.structural_fresh(self.now()))
            {
                return Err("fresh paired module state required".into());
            }
        }
        // The raw drain may include a newer device observation too.
        if kind == "device_configure" {
            self.session.validate_device_intent(&body, self.now())?;
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
        if d.kind == "processing_set"
            || crate::structure::is_kind(&d.kind)
            || crate::brain::is_kind(&d.kind)
            || d.kind == "device_configure"
        {
            if d.kind == "processing_set" {
                self.refresh_processing()?;
            } else if d.kind == "device_configure" {
                self.refresh_device()?;
                self.session.validate_device_intent(&d.body, self.now())?;
            } else if crate::brain::is_kind(&d.kind) {
                self.refresh_brain()?;
            } else {
                self.refresh_structural()?;
            }
            self.refresh()?;
            if self.session.generation() != d.generation
                || self
                    .session
                    .snapshot
                    .as_ref()
                    .is_none_or(|s| s.authority.revision != d.revision)
            {
                return Err("processing confirmation context changed during paired refresh".into());
            }
        }
        // No refresh between the final revision check and begin; the request pins it.
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
        transport: Box::new(Transport::connect(Path::new(&args[0]))?),
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
        brain_signal: None,
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

#[cfg(test)]
mod gp07_stage_tests {
    use super::*;
    use std::os::unix::net::UnixStream;
    fn paired(change_before_stage: bool) {
        let corpus: Value =
            serde_json::from_str(include_str!("../tests/fixtures/gp03/v1/e03-rendered.json"))
                .unwrap();
        let (socket, mut server) = UnixStream::pair().unwrap();
        server
            .set_read_timeout(Some(Duration::from_secs(2)))
            .unwrap();
        let original = corpus.clone();
        let child = std::thread::spawn(move || {
            let mut seen = Vec::new();
            let mut raws = 0;
            loop {
                let mut size = [0; 4];
                if server.read_exact(&mut size).is_err() {
                    break;
                }
                let size = u32::from_be_bytes(size) as usize;
                assert!(size <= 65536);
                let mut bytes = vec![0; size];
                server.read_exact(&mut bytes).unwrap();
                let request: Value = serde_json::from_slice(&bytes).unwrap();
                assert!(request["writer"].is_null(), "refresh must never mutate");
                let kind = request["kind"].as_str().unwrap().to_string();
                seen.push(kind.clone());
                let revision = if change_before_stage || raws >= 1 && kind == "snapshot" {
                    "13"
                } else {
                    "12"
                };
                let mut reply = if kind == "snapshot" {
                    raws += 1;
                    let mut r = original["grant_response"].clone();
                    for k in ["writer", "lease", "request_id", "expected_revision"] {
                        r["context"][k] = Value::Null;
                        r["outcome"][k] = Value::Null;
                    }
                    for k in ["granted_lease", "lease_remaining_ms", "scope"] {
                        r["outcome"]["body"][k] = Value::Null;
                    }
                    r["outcome"]["body"]["revision"] = revision.into();
                    r["snapshot"] = original["initial"].clone();
                    r["snapshot"]["authority"]["revision"] = revision.into();
                    r
                } else {
                    assert_eq!(kind, "processing_snapshot");
                    let mut r: Value = serde_json::from_str(include_str!(
                        "../tests/fixtures/gp07/v2/snapshot-reply.json"
                    ))
                    .unwrap();
                    r["revision"] = revision.into();
                    r["snapshot"]["revision"] = revision.into();
                    r["snapshot"]["sequence"] = "5000".into();
                    r
                };
                reply["snapshot"]["frame"] = "100000".into();
                let bytes = serde_json::to_vec(&reply).unwrap();
                if server
                    .write_all(&(bytes.len() as u32).to_be_bytes())
                    .is_err()
                    || server.write_all(&bytes).is_err()
                {
                    break;
                }
            }
            seen
        });
        let mut session = Session::new(
            "11111111-1111-4111-8111-111111111111",
            9,
            "stage-regression",
            "foh",
        )
        .unwrap();
        session
            .ingest_snapshot(
                audio::decode_snapshot(&serde_json::to_vec(&corpus["initial"]).unwrap()).unwrap(),
                0,
            )
            .unwrap();
        let mut processing = crate::processing::decode_reply(include_bytes!(
            "../tests/fixtures/gp07/v2/snapshot-reply.json"
        ))
        .unwrap()
        .snapshot
        .unwrap();
        processing.revision = "12".into();
        let body = json!({"input":"input-01", "config":processing.channels[0].target});
        session.ingest_processing(processing, 0).unwrap();
        let mut op = Operator {
            transport: Box::new(Transport { socket }),
            session,
            start: Instant::now() - Duration::from_millis(300),
            draft: None,
            scope: "foh".into(),
            guard: None,
            brain_signal: None,
        };
        assert!(!op.session.processing_fresh(op.now()));
        if change_before_stage {
            assert!(
                op.stage("processing_set", body)
                    .unwrap_err()
                    .contains("context changed")
            );
            assert!(op.draft.is_none());
        } else {
            op.stage("processing_set", body).unwrap();
            assert_eq!(op.draft.as_ref().unwrap().revision, "12");
            assert!(op.session.processing_fresh(op.now()));
            op.start -= Duration::from_millis(300);
            assert!(
                op.confirm()
                    .unwrap_err()
                    .contains("context/revision changed")
            );
            assert!(op.draft.is_none());
            assert!(op.session.pending.is_none());
        }
        drop(op);
        let seen = child.join().unwrap();
        assert!(seen.contains(&"processing_snapshot".into()));
        assert!(
            seen.iter()
                .all(|k| k == "snapshot" || k == "processing_snapshot")
        );
    }
    #[test]
    fn aged_processing_review_refreshes_without_rebasing_then_changed_confirmation_refuses() {
        paired(false);
    }
    #[test]
    fn expired_raw_snapshot_cannot_rebase_queued_processing_review() {
        paired(true);
    }
}

#[cfg(test)]
mod remote_document_tests {
    use super::*;
    struct CompleteDocument(Option<Vec<u8>>);
    impl AuthorityConnection for CompleteDocument {
        fn send_frame_until(&mut self, _: &[u8], _: Instant) -> Result<(), String> {
            Ok(())
        }
        fn receive_until(&mut self, _: Instant) -> Result<Option<Vec<u8>>, String> {
            Ok(self.0.take())
        }
        fn receive_available(&mut self) -> Result<Option<Vec<u8>>, String> {
            Ok(self.0.take())
        }
    }
    #[test]
    fn remote_complete_payload_does_not_pass_through_unix_frame_size_gate_again() {
        let corpus: Value =
            serde_json::from_slice(include_bytes!("../tests/fixtures/gp14/v1/profile-48.json"))
                .unwrap();
        let bytes = serde_json::to_vec(&corpus["snapshot"]).unwrap();
        assert!(bytes.len() > crate::provider::MAX_BYTES);
        let snapshot = audio::decode_reply(&bytes).unwrap().snapshot.unwrap();
        let mut operator = Operator::from_document_connection(
            Box::new(CompleteDocument(Some(bytes))),
            &snapshot.authority.show_id,
            snapshot.authority.epoch.parse().unwrap(),
            "remote-doc-test",
            "foh",
            2,
        )
        .unwrap();
        operator.refresh().unwrap();
        assert_eq!(
            operator.session.snapshot.unwrap().authority.inputs.len(),
            48
        );
    }
}

#[cfg(test)]
mod structural_pair_tests {
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
        let corpus: Value = serde_json::from_slice(include_bytes!(
            "../tests/fixtures/gp14/final/structure-replies-48.json"
        ))
        .unwrap();
        let structural = corpus["exchanges"][0]["reply"].clone();
        let profile: Value =
            serde_json::from_slice(include_bytes!("../tests/fixtures/gp14/v1/profile-48.json"))
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
}

#[cfg(test)]
mod device_fence_tests {
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
            "../tests/fixtures/gp15/v1/raw-snapshot-16-1.json"
        ))
        .unwrap();
        let device: Value = serde_json::from_slice(include_bytes!(
            "../tests/fixtures/gp15/device-v1/snapshot-unarmed.json"
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
}

#[cfg(test)]
mod brain_fifo_tests {
    use super::*;
    use std::{
        collections::VecDeque,
        sync::{Arc, Mutex},
    };

    #[derive(Clone, Default)]
    struct FakeAuthorityConnection(Arc<Mutex<VecDeque<Vec<u8>>>>);
    impl FakeAuthorityConnection {
        fn push(&self, value: Value) {
            self.0
                .lock()
                .unwrap()
                .push_back(serde_json::to_vec(&value).unwrap());
        }
    }
    impl AuthorityConnection for FakeAuthorityConnection {
        fn send_frame_until(&mut self, _: &[u8], _: Instant) -> Result<(), String> {
            panic!("buffered refresh must not send or replay a command")
        }
        fn receive_until(&mut self, _: Instant) -> Result<Option<Vec<u8>>, String> {
            self.receive_available()
        }
        fn receive_available(&mut self) -> Result<Option<Vec<u8>>, String> {
            Ok(self.0.lock().unwrap().pop_front())
        }
    }
    fn setup() -> (Operator, FakeAuthorityConnection, Value) {
        let raw = audio::decode_snapshot(include_bytes!(
            "../tests/fixtures/gp15/v1/raw-snapshot-16-1.json"
        ))
        .unwrap();
        let peer = FakeAuthorityConnection::default();
        let mut op = Operator::from_document_connection(
            Box::new(peer.clone()),
            &raw.authority.show_id,
            1,
            "talkback",
            "talkback_destinations",
            2,
        )
        .unwrap();
        op.session.ingest_snapshot(raw.clone(), 0).unwrap();
        op.session
            .begin("grant", json!({"scope":"talkback_destinations"}), 0)
            .unwrap();
        op.session
            .accept(
                audio::decode_reply(include_bytes!(
                    "../tests/fixtures/gp15/v1/grant-16-talkback-reply.json"
                ))
                .unwrap(),
                0,
            )
            .unwrap();
        op.session.ingest_snapshot(raw.clone(), 0).unwrap();
        op.session.input_released();
        let mut brain =
            crate::brain::decode_reply(include_bytes!("../tests/fixtures/gp15/v1/hold-final.json"))
                .unwrap()
                .snapshot
                .unwrap();
        brain.talkback_monitors = vec![0];
        brain.revision = raw.authority.revision.clone();
        brain.frame = raw.frame.clone();
        brain.held_generation = None;
        brain.hold_generation_counter = "0".into();
        brain.hold_deadline_ms = None;
        op.session.ingest_brain(brain, 0).unwrap();
        let mut telemetry: Value = serde_json::from_slice(include_bytes!(
            "../tests/fixtures/gp15/v1/grant-16-talkback-reply.json"
        ))
        .unwrap();
        telemetry["snapshot"] = serde_json::to_value(raw).unwrap();
        for k in ["writer", "lease", "request_id", "expected_revision"] {
            telemetry["context"][k] = Value::Null;
            telemetry["outcome"][k] = Value::Null;
        }
        for k in ["granted_lease", "lease_remaining_ms", "scope"] {
            telemetry["outcome"]["body"][k] = Value::Null;
        }
        (op, peer, telemetry)
    }
    fn replies(op: &Operator, request: &Request, held: bool) -> (Value, Value) {
        let revision = request
            .context
            .expected_revision
            .as_ref()
            .unwrap()
            .parse::<u64>()
            .unwrap();
        let frame = op
            .session
            .brain
            .as_ref()
            .unwrap()
            .frame
            .parse::<u64>()
            .unwrap()
            + 48;
        let pending = json!({"contract":"GP15-brain","version":1,"state":"pending","reason":null,"context":request.context,"revision":revision.to_string(),"applied_frame":frame.to_string(),"snapshot":null});
        let mut final_reply = pending.clone();
        final_reply["state"] = json!("final");
        final_reply["revision"] = json!((revision + 1).to_string());
        let mut brain = op.session.brain.clone().unwrap();
        brain.revision = (revision + 1).to_string();
        brain.frame = frame.to_string();
        brain.held_generation = held.then(|| "1".into());
        brain.hold_generation_counter = "1".into();
        brain.hold_deadline_ms = held.then(|| "150".into());
        final_reply["snapshot"] = serde_json::to_value(brain).unwrap();
        (pending, final_reply)
    }
    fn raw_at(raw: &Value, revision: &str, frame: &str) -> Value {
        let mut raw = raw.clone();
        raw["outcome"]["body"]["revision"] = json!(revision);
        raw["snapshot"]["authority"]["revision"] = json!(revision);
        raw["snapshot"]["authority"]["sequence"] = json!(frame);
        raw["snapshot"]["frame"] = json!(frame);
        raw["snapshot"]["clock"]["next_frame"] = json!(frame.parse::<u64>().unwrap());
        raw
    }
    #[test]
    fn pending_final_batch_and_repeated_heartbeats_keep_common_revision_fresh() {
        let (mut op, peer, raw) = setup();
        for index in 0..5 {
            let kind = if index == 0 {
                "brain_hold"
            } else {
                "brain_heartbeat"
            };
            let body = if index == 0 {
                json!({"generation":"1"})
            } else {
                json!({"generation":"1","observed_frame":op.session.brain.as_ref().unwrap().frame})
            };
            let request = op.session.begin(kind, body, op.now()).unwrap();
            let (pending, final_reply) = replies(&op, &request, true);
            let revision = final_reply["revision"].as_str().unwrap();
            let frame = final_reply["applied_frame"].as_str().unwrap();
            // Newest raw can arrive before final, with older raw after it. Only
            // final-then-newest-raw admission clears needs_snapshot coherently.
            peer.push(pending);
            peer.push(raw_at(&raw, revision, frame));
            peer.push(final_reply.clone());
            let mut regressive = raw.clone();
            regressive["snapshot"]["authority"]["sequence"] = json!("0");
            peer.push(regressive);
            op.refresh().unwrap();
            assert!(op.session.pending.is_none());
            assert!(op.session.brain_fresh(op.now()));
            assert_eq!(
                op.session.snapshot.as_ref().unwrap().authority.revision,
                revision
            );
            assert_eq!(
                op.session.brain_final.as_ref().unwrap().context,
                request.context
            );
        }
    }
    #[test]
    fn priority_close_interleaves_with_hold_without_losing_either_correlation() {
        let (mut op, peer, raw) = setup();
        let hold = op
            .session
            .begin("brain_hold", json!({"generation":"1"}), op.now())
            .unwrap();
        let close = op.session.brain_close_request(op.now()).unwrap();
        let (pending, final_reply) = replies(&op, &hold, true);
        let (close_pending, mut close_final) = replies(&op, &close, false);
        // Both requests pinned revision zero. Hold applies first; close is
        // explicitly refused at revision one, never silently treated as applied.
        close_final["reason"] = json!("conflict");
        close_final["snapshot"] = Value::Null;
        peer.push(pending);
        peer.push(close_pending);
        peer.push(final_reply.clone());
        peer.push(close_final);
        peer.push(raw_at(
            &raw,
            "1",
            final_reply["applied_frame"].as_str().unwrap(),
        ));
        op.refresh().unwrap();
        assert!(op.session.pending.is_none());
        assert_eq!(
            op.session.brain_final.as_ref().unwrap().context,
            close.context
        );
        assert!(op.session.brain_final.as_ref().unwrap().reason.is_some());
        assert!(!op.session.brain_fresh(op.now()));
        assert_eq!(
            op.session
                .brain
                .as_ref()
                .unwrap()
                .held_generation
                .as_deref(),
            Some("1")
        );
        assert_ne!(
            op.session
                .brain_close_request(op.now())
                .unwrap()
                .context
                .request_id,
            close.context.request_id
        );
    }
    #[test]
    fn differing_raw_and_brain_revisions_require_a_matching_raw_observation() {
        let (mut op, peer, raw) = setup();
        let request = op
            .session
            .begin("brain_hold", json!({"generation":"1"}), op.now())
            .unwrap();
        let (pending, final_reply) = replies(&op, &request, true);
        peer.push(pending);
        peer.push(final_reply.clone());
        peer.push(raw.clone());
        op.refresh().unwrap();
        assert!(op.session.fresh(op.now()));
        assert!(!op.session.brain_fresh(op.now()));
        assert!(op.session.pending.is_none());
        peer.push(raw_at(
            &raw,
            "1",
            final_reply["applied_frame"].as_str().unwrap(),
        ));
        op.refresh().unwrap();
        assert!(op.session.brain_fresh(op.now()));
    }
    #[test]
    fn unknown_reply_still_fails_and_saturated_batch_dispatches_nothing() {
        let (mut op, peer, raw) = setup();
        let request = op
            .session
            .begin("brain_hold", json!({"generation":"1"}), op.now())
            .unwrap();
        let (pending, final_reply) = replies(&op, &request, true);
        peer.push(pending.clone());
        peer.push(final_reply.clone());
        for _ in 0..62 {
            peer.push(raw.clone());
        }
        assert!(op.refresh().unwrap_err().contains("backlog saturated"));
        assert_eq!(
            op.session.pending.as_ref().unwrap().state,
            audio::PendingState::Sent
        );
        assert!(op.session.brain_final.is_none());
        let mut unknown = pending;
        unknown["context"]["request_id"] = json!("999");
        peer.push(unknown);
        assert!(
            op.refresh()
                .unwrap_err()
                .contains("unknown Brain reply correlation")
        );
        assert!(op.session.pending.is_some());
    }
}
