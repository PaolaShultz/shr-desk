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
        self.receive_available_until(Instant::now() + Duration::from_millis(200))
    }
    fn receive_available_until(&mut self, deadline: Instant) -> Result<Option<Vec<u8>>, String> {
        if Instant::now() >= deadline {
            return Err("frame deadline".into());
        }
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
        self.receive_until(deadline)
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
    /// Complete-frame queue implementations may use this default. Stream-backed
    /// implementations must override it so partial reads inherit the caller budget.
    fn receive_available_until(&mut self, deadline: Instant) -> Result<Option<Vec<u8>>, String> {
        if Instant::now() >= deadline {
            return Err("frame deadline".into());
        }
        let result = self.receive_available()?;
        if Instant::now() >= deadline {
            return Err("frame deadline".into());
        }
        Ok(result)
    }
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
    fn receive_available_until(&mut self, deadline: Instant) -> Result<Option<Vec<u8>>, String> {
        Transport::receive_available_until(self, deadline)
    }
}
pub(crate) enum BrainOperationError {
    Admission(String),
    Fault(String),
}
impl BrainOperationError {
    pub(crate) fn message(self) -> String {
        match self {
            Self::Admission(s) | Self::Fault(s) => s,
        }
    }
}
impl From<String> for BrainOperationError {
    fn from(value: String) -> Self {
        Self::Fault(value)
    }
}
impl From<&str> for BrainOperationError {
    fn from(value: &str) -> Self {
        Self::Fault(value.into())
    }
}
#[derive(Default)]
struct BrainProbes {
    next: u64,
    pending: std::collections::VecDeque<(u64, Instant, u64)>,
    matched: Option<(u64, Instant, crate::brain::Snapshot)>,
    invalid: bool,
    first_fault: Option<String>,
}
pub struct Operator {
    brain_probes: BrainProbes,
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
            brain_probes: BrainProbes::default(),
        })
    }
    pub(crate) fn cancel(&mut self) {
        // Keep FIFO ordinals for replies already in flight, but none of their
        // observations may authorize input in the next controller generation.
        self.brain_probes.matched = None;
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
            if let Err(error) = self.close_brain() {
                self.invalidate_brain_probes(&error);
                return Err(error);
            }
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
            let result: Result<(), String> = (|| {
                let reply = crate::brain::decode_reply(bytes)?;
                let snapshot = (reply.state == "snapshot")
                    .then(|| reply.snapshot.clone())
                    .flatten();
                self.session.dispatch_brain(reply, self.now())?;
                if let Some(snapshot) = snapshot {
                    if self.brain_probes.invalid {
                        return Err("Brain probe provenance invalid".into());
                    }
                    let (id, sent, generation) = self
                        .brain_probes
                        .pending
                        .pop_front()
                        .ok_or("unmatched Brain snapshot probe")?;
                    if generation == self.session.generation() {
                        self.brain_probes.matched = Some((id, sent, snapshot));
                    }
                }
                Ok(())
            })();
            if let Err(e) = result {
                self.invalidate_brain_probes(&e);
                let _ = self.close_brain();
                return Err(e);
            }
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
    fn invalidate_brain_probes(&mut self, reason: &str) {
        self.brain_probes
            .first_fault
            .get_or_insert_with(|| reason.into());
        self.brain_probes.invalid = true;
        self.brain_probes.pending.clear();
        self.brain_probes.matched = None;
        self.session.invalidate_brain_observation();
        if let Some(signal) = &self.brain_signal {
            signal.release();
        }
    }
    fn send_brain_probe(&mut self, deadline: Instant) -> Result<u64, String> {
        if self.brain_probes.invalid || self.brain_probes.pending.len() >= 64 {
            self.invalidate_brain_probes("Brain probe provenance/queue bound");
            let _ = self.close_brain();
            return Err(self.brain_probes.first_fault.clone().unwrap());
        }
        let id = self
            .brain_probes
            .next
            .checked_add(1)
            .ok_or("Brain probe exhausted")?;
        let bytes = self.session.brain_request().encode()?;
        let sent = Instant::now();
        self.brain_probes.next = id;
        self.brain_probes
            .pending
            .push_back((id, sent, self.session.generation()));
        if let Err(e) = self.transport.send_frame_until(&bytes, deadline) {
            self.invalidate_brain_probes(&e);
            let _ = self.close_brain();
            return Err(e);
        }
        Ok(id)
    }
    pub(crate) fn refresh_brain(&mut self) -> Result<(), String> {
        self.refresh_brain_until(Instant::now() + Duration::from_millis(250))
    }
    fn finish_brain_operation(
        &mut self,
        result: Result<(), BrainOperationError>,
    ) -> Result<(), String> {
        match result {
            Ok(()) => Ok(()),
            Err(BrainOperationError::Admission(reason)) => {
                // Keep all in-flight FIFO identities. A new controller generation
                // cannot use their observations as its matched authorization.
                self.cancel();
                self.session.invalidate_brain_observation();
                if let Some(signal) = &self.brain_signal {
                    signal.release();
                }
                if let Err(error) = self.close_brain() {
                    self.invalidate_brain_probes(&error);
                    return Err(error);
                }
                Err(reason)
            }
            Err(BrainOperationError::Fault(reason)) => {
                self.invalidate_brain_probes(&reason);
                let _ = self.close_brain();
                Err(self.brain_probes.first_fault.clone().unwrap())
            }
        }
    }
    fn brain_guard(&mut self) -> Result<(), BrainOperationError> {
        if self
            .brain_signal
            .as_ref()
            .is_some_and(|s| s.close.swap(false, std::sync::atomic::Ordering::AcqRel))
        {
            self.close_brain().map_err(BrainOperationError::Fault)?;
        }
        if self
            .guard
            .as_ref()
            .is_some_and(|(g, n)| g.load(std::sync::atomic::Ordering::Acquire) != *n)
        {
            return Err(BrainOperationError::Admission(
                "Brain input context revoked".into(),
            ));
        }
        Ok(())
    }
    fn refresh_brain_until(&mut self, deadline: Instant) -> Result<(), String> {
        let result = self.refresh_brain_inner(deadline);
        self.finish_brain_operation(result)
    }
    fn refresh_brain_inner(&mut self, deadline: Instant) -> Result<(), BrainOperationError> {
        self.brain_guard()?;
        let mut probe = self.send_brain_probe(deadline)?;
        let mut requested_revision = None;
        for _ in 0..64 {
            self.brain_guard()?;
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
                // Validate bytes/correlation before observing concurrent cancellation:
                // a focus change must never hide malformed or partial wire data.
                self.brain_guard()?;
                // Another writer may advance the common revision between our
                // Brain reply and its raw pair. Raw cannot regress: obtain a new
                // read-only probe within this operation's original frame/time budget.
                if self
                    .brain_probes
                    .matched
                    .as_ref()
                    .is_some_and(|(id, _, matched)| {
                        *id == probe
                            && self.session.snapshot.as_ref().is_some_and(|raw| {
                                raw.authority.revision.parse::<u64>().unwrap()
                                    > matched.revision.parse::<u64>().unwrap()
                            })
                    })
                {
                    probe = self.send_brain_probe(deadline)?;
                }
                if let Some(brain) = &self.session.brain
                    && (!self.session.fresh(self.now())
                        || self.session.snapshot.as_ref().is_none_or(|raw| {
                            raw.authority.revision.parse::<u64>().unwrap()
                                < brain.revision.parse::<u64>().unwrap()
                        }))
                    && requested_revision.as_ref() != Some(&brain.revision)
                {
                    requested_revision = Some(brain.revision.clone());
                    self.transport
                        .send_frame_until(&self.session.snapshot_request().encode()?, deadline)?;
                }
                if self.session.brain_fresh(self.now())
                    && self
                        .brain_probes
                        .matched
                        .as_ref()
                        .is_some_and(|(id, _, snapshot)| {
                            *id == probe
                                && self
                                    .session
                                    .snapshot
                                    .as_ref()
                                    .is_some_and(|raw| raw.authority.revision == snapshot.revision)
                        })
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
    fn heartbeat_probe_body(
        &self,
        generation: u64,
        now: Instant,
    ) -> Result<(Value, Instant), String> {
        let (_, sent, snapshot) = self
            .brain_probes
            .matched
            .as_ref()
            .ok_or("heartbeat needs matched probe")?;
        if self.brain_probes.invalid
            || now.saturating_duration_since(*sent) > Duration::from_millis(30)
            || !self.session.brain_fresh(self.now())
            || self.session.pending.is_some()
            || snapshot.held_generation.as_deref() != Some(generation.to_string().as_str())
            || self
                .session
                .snapshot
                .as_ref()
                .is_none_or(|raw| raw.authority.revision != snapshot.revision)
            || self.session.brain.as_ref().is_none_or(|b| {
                b.held_generation != snapshot.held_generation || b.revision != snapshot.revision
            })
            || self.brain_signal.as_ref().is_some_and(|s| !s.live())
        {
            return Err("heartbeat fresh probe/held context unavailable".into());
        }
        Ok((
            json!({"generation":generation.to_string(),"observed_frame":snapshot.frame}),
            (now + Duration::from_millis(20)).min(*sent + Duration::from_millis(50)),
        ))
    }
    /// Send one ephemeral edge; worker polls completion, never retries a hold/heartbeat.
    pub(crate) fn brain_edge(&mut self, kind: &str, generation: u64) -> Result<(), String> {
        let result: Result<(), BrainOperationError> = (|| {
            let mut deadline = Instant::now() + Duration::from_millis(20);
            let body = if kind == "brain_heartbeat" {
                // Receipt time is not source freshness. Match our own FIFO query,
                // then bound the entire observation-to-send interval from BEFORE send.
                self.refresh_brain_inner(Instant::now() + Duration::from_millis(30))?;
                self.brain_guard()?;
                let (body, send_deadline) = self
                    .heartbeat_probe_body(generation, Instant::now())
                    .map_err(BrainOperationError::Admission)?;
                deadline = send_deadline;
                body
            } else {
                if kind == "brain_hold" {
                    self.refresh_brain_inner(Instant::now() + Duration::from_millis(250))?;
                    self.brain_guard()?;
                    let snapshot = &self
                        .brain_probes
                        .matched
                        .as_ref()
                        .ok_or_else(|| {
                            BrainOperationError::Admission("hold needs current probe".into())
                        })?
                        .2;
                    if self.brain_signal.as_ref().is_some_and(|s| !s.live())
                        || snapshot.held_generation.is_some()
                        || snapshot
                            .hold_generation_counter
                            .parse::<u64>()
                            .ok()
                            .and_then(|n| n.checked_add(1))
                            != Some(generation)
                    {
                        return Err(BrainOperationError::Admission(
                            "hold edge/context no longer current".into(),
                        ));
                    }
                    deadline = Instant::now() + Duration::from_millis(20);
                }
                json!({"generation":generation.to_string()})
            };
            let r = self
                .session
                .begin(kind, body, self.now())
                .map_err(BrainOperationError::Admission)?;
            self.transport
                .send_frame_until(&r.encode()?, deadline)
                .map_err(BrainOperationError::Fault)
        })();
        self.finish_brain_operation(result)
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
        self.refresh_classified()
            .map_err(BrainOperationError::message)
    }
    pub(crate) fn refresh_classified(&mut self) -> Result<(), BrainOperationError> {
        if let Some(reason) = &self.brain_probes.first_fault {
            return Err(BrainOperationError::Fault(reason.clone()));
        }
        // One total budget covers queued replies, a final's required readback,
        // solicited frames and decoding. Never replay a mutation here.
        let deadline = Instant::now() + Duration::from_millis(250);
        let mut remaining = 64usize;
        let mut requested = None;
        let mut next_frame = None;
        let mut continuation = false;
        let mut first_batch = true;
        loop {
            self.brain_guard()?;
            if Instant::now() >= deadline {
                return Err("snapshot deadline/queue bound".into());
            }
            let mut frames = Vec::new();
            if let Some(bytes) = next_frame.take() {
                frames.push(bytes);
            }
            let mut drained = false;
            while frames.len() < remaining && Instant::now() < deadline {
                if let Some(bytes) = self.transport.receive_available_until(deadline)? {
                    frames.push(bytes);
                } else {
                    drained = true;
                    break;
                }
            }
            // Prove the queue drained before admitting any state from this batch.
            if !drained {
                return Err("snapshot backlog saturated; freshness not admitted".into());
            }
            // Legacy unsolicited raw-only batches must not trigger a query when
            // all their observations are stale/regressive. Initial quiet stale
            // attachment and actual contract readback may wait for a new frame.
            if first_batch && frames.is_empty() {
                continuation = true;
            }
            first_batch = false;
            remaining -= frames.len();
            let mut raw_frames = Vec::new();
            for bytes in frames {
                if Instant::now() >= deadline {
                    return Err("snapshot deadline/queue bound".into());
                }
                // Stateful replies retain wire order, including priority close.
                if self.processing_frame(&bytes)? {
                    continuation = true;
                } else {
                    raw_frames.push(bytes);
                }
            }
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
            self.brain_guard()?;
            let brain_observed = self.session.brain_age(self.now()).is_some();
            if Instant::now() < deadline
                && self.session.fresh(self.now())
                && (!brain_observed || self.session.brain_fresh(self.now()))
            {
                return Ok(());
            }
            if (!continuation && !brain_observed) || remaining == 0 || Instant::now() >= deadline {
                return Err("snapshot deadline/queue bound".into());
            }
            // A valid final can arrive before its raw observation. Request the
            // missing pair once per observed revision, inside the original budget.
            let revisions = (
                self.session
                    .snapshot
                    .as_ref()
                    .map(|s| s.authority.revision.clone()),
                self.session.brain.as_ref().map(|s| s.revision.clone()),
            );
            if requested.as_ref() != Some(&revisions) {
                self.transport
                    .send_frame_until(&self.session.snapshot_request().encode()?, deadline)?;
                if brain_observed {
                    self.send_brain_probe(deadline)?;
                }
                requested = Some(revisions);
            }
            self.brain_guard()?;
            if Instant::now() >= deadline {
                return Err("snapshot deadline/queue bound".into());
            }
            // Include the delayed frame with the next drained FIFO batch, so
            // contract ordering and newest-first raw coalescing stay identical.
            next_frame = self.transport.receive_until(deadline)?;
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
        let deadline = Instant::now() + Duration::from_millis(1900);
        if kind == "renew" && self.session.brain.is_some() {
            // Null-context raw telemetry cannot identify a solicited query. The
            // matched GP15 probe anchors current authority revision; raw must pair
            // exactly before a NEW renewal request gets its immutable ID/revision.
            let lease_deadline = self.start
                + Duration::from_millis(self.session.lease_deadline().ok_or("renew no lease")?);
            self.refresh_brain_until(
                deadline
                    .min(lease_deadline)
                    .min(Instant::now() + Duration::from_millis(250)),
            )?;
        }
        let r = self.session.begin(kind, body, self.now())?;
        self.check_guard()?;
        self.transport.send_frame_until(
            &r.encode()?,
            deadline
                .min(Instant::now() + Duration::from_millis(200))
                .min(
                    self.start
                        + Duration::from_millis(
                            self.session
                                .pending_authority_deadline()
                                .ok_or("pending authority unavailable")?,
                        ),
                ),
        )?;
        loop {
            self.check_guard()?;
            if Instant::now() >= deadline {
                return Err("pending/uncertain deadline; no applied claim".into());
            }
            let authority_deadline = self.start
                + Duration::from_millis(
                    self.session
                        .pending_authority_deadline()
                        .ok_or("pending authority unavailable")?,
                );
            if Instant::now() >= authority_deadline {
                return Err("pending authority expired; no further retry".into());
            }
            // Service the existing byte-identical retry schedule before blocking.
            // A silent backpressure peer must not hide the100/250/500ms wakes.
            if let Some(retry) = self.session.retry(self.now()) {
                self.transport
                    .send_frame_until(&retry.encode()?, deadline.min(authority_deadline))?;
            }
            let now = self.now();
            let wake = self
                .session
                .retry_wake(now)
                .ok_or("pending authority expired/uncertain; no further retry")?;
            if wake <= now {
                // At most the three existing retries can already be due.
                continue;
            }
            let receive_deadline = deadline.min(self.start + Duration::from_millis(wake));
            match self.transport.receive_until(receive_deadline) {
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
                                    let renewal_refusal =
                                        if kind == "renew" && reply.state == "final" {
                                            reply
                                                .outcome
                                                .as_ref()
                                                .filter(|o| o.kind != "applied")
                                                .map(|o| format!("{}: {:?}", o.kind, o.body.reason))
                                        } else {
                                            None
                                        };
                                    if let Err(e) = self.session.accept(reply, self.now()) {
                                        eprintln!("refused reply; pending retained: {e}");
                                    }
                                    if self.session.pending.is_none() {
                                        if let Some(reason) = renewal_refusal {
                                            return Err(format!("renewal refused: {reason}"));
                                        }
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
                Ok(None) => {
                    // Abstract/fake connections may return before their deadline.
                    // Avoid spinning without delaying the next scheduled wake.
                    let pause = receive_deadline
                        .saturating_duration_since(Instant::now())
                        .min(Duration::from_millis(10));
                    if !pause.is_zero() {
                        std::thread::sleep(pause);
                    }
                }
                Err(e) => {
                    self.session.disconnect();
                    return Err(format!("uncertain operation; transport closed: {e}"));
                }
            }
            // No-first-byte timeout is a scheduler wake. Partial-frame timeout
            // remains an error above and retires the session; it is never retried.
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
        brain_probes: BrainProbes::default(),
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
    fn available_partial_unix_frame_inherits_the_remaining_refresh_deadline() {
        let (socket, mut peer) = UnixStream::pair().unwrap();
        let observer = socket.try_clone().unwrap();
        let transport = Transport { socket };
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
            brain_probes: BrainProbes::default(),
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
            Box::new(CompleteDocument(Some(bytes.clone()))),
            &snapshot.authority.show_id,
            snapshot.authority.epoch.parse().unwrap(),
            "remote-doc-test",
            "foh",
            2,
        )
        .unwrap();
        // This regression protects document-vs-Unix-frame transport selection,
        // not debug schema throughput. Exercise the actual constructor's transport
        // under a deadline; keep strict decoding outside refresh's separate250ms
        // production budget (covered by the focused deadline/readback tests).
        let received = operator
            .transport
            .receive_available_until(Instant::now() + Duration::from_millis(250))
            .unwrap()
            .unwrap();
        assert_eq!(received, bytes);
        let reply = audio::decode_reply(&received).unwrap();
        operator.telemetry(&reply).unwrap();
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
        // Protect pairing/expiry, not debug48-input schema throughput. Full-size
        // documents have separate transport/schema and release acceptance coverage.
        let corpus: Value = serde_json::from_slice(include_bytes!(
            "../tests/fixtures/gp14/final/structure-replies-16.json"
        ))
        .unwrap();
        let structural = corpus["exchanges"][0]["reply"].clone();
        let profile: Value =
            serde_json::from_slice(include_bytes!("../tests/fixtures/gp14/v1/profile-16.json"))
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
        fn send_frame_until(&mut self, bytes: &[u8], _: Instant) -> Result<(), String> {
            let request: Value = serde_json::from_slice(bytes).unwrap();
            assert_eq!(
                request["kind"], "close",
                "only failure closure may send during buffered refresh"
            );
            Ok(())
        }
        fn receive_until(&mut self, _: Instant) -> Result<Option<Vec<u8>>, String> {
            self.receive_available()
        }
        fn receive_available(&mut self) -> Result<Option<Vec<u8>>, String> {
            Ok(self.0.lock().unwrap().pop_front())
        }
    }
    struct DelayedReadback {
        queued: FakeAuthorityConnection,
        delayed: Option<Value>,
        enqueue_after_wait: Vec<Value>,
        deadlines: Arc<Mutex<Vec<Instant>>>,
    }
    impl AuthorityConnection for DelayedReadback {
        fn send_frame_until(&mut self, bytes: &[u8], deadline: Instant) -> Result<(), String> {
            let request: Value = serde_json::from_slice(bytes).unwrap();
            assert!(
                request["writer"].is_null(),
                "refresh must never replay a mutation"
            );
            assert!(request["kind"].as_str().unwrap().contains("snapshot"));
            self.deadlines.lock().unwrap().push(deadline);
            Ok(())
        }
        fn receive_available(&mut self) -> Result<Option<Vec<u8>>, String> {
            self.queued.receive_available()
        }
        fn receive_until(&mut self, deadline: Instant) -> Result<Option<Vec<u8>>, String> {
            self.deadlines.lock().unwrap().push(deadline);
            for value in self.enqueue_after_wait.drain(..) {
                self.queued.push(value);
            }
            if let Some(value) = self.delayed.take() {
                Ok(Some(serde_json::to_vec(&value).unwrap()))
            } else {
                // Honor the same real deadline without inventing a fresh timeout.
                std::thread::sleep(deadline.saturating_duration_since(Instant::now()));
                Ok(None)
            }
        }
    }
    fn delayed_transport(
        op: &mut Operator,
        peer: FakeAuthorityConnection,
        delayed: Option<Value>,
        tail: Vec<Value>,
    ) -> Arc<Mutex<Vec<Instant>>> {
        let deadlines = Arc::new(Mutex::new(Vec::new()));
        op.transport = Box::new(DelayedReadback {
            queued: peer,
            delayed,
            enqueue_after_wait: tail,
            deadlines: deadlines.clone(),
        });
        deadlines
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
    fn delayed_raw_after_hold_and_heartbeat_finals_uses_one_budget_and_keeps_brain_fresh() {
        let (mut op, peer, raw) = setup();
        for index in 0..3 {
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
            peer.push(pending);
            peer.push(final_reply.clone());
            // An equal old raw observation cannot make this new Brain state fresh.
            peer.push(raw.clone());
            let delayed = raw_at(
                &raw,
                final_reply["revision"].as_str().unwrap(),
                final_reply["applied_frame"].as_str().unwrap(),
            );
            let deadlines = delayed_transport(&mut op, peer.clone(), Some(delayed), vec![]);
            op.refresh().unwrap();
            assert!(op.session.pending.is_none());
            assert!(op.session.brain_fresh(op.now()));
            assert_eq!(
                op.session
                    .brain
                    .as_ref()
                    .unwrap()
                    .held_generation
                    .as_deref(),
                Some("1")
            );
            let deadlines = deadlines.lock().unwrap();
            assert!(deadlines.len() >= 2);
            assert!(deadlines.iter().all(|d| *d == deadlines[0]));
        }
    }
    #[test]
    fn final_without_raw_times_out_without_admitting_stale_brain_or_resetting_deadline() {
        let (mut op, peer, _) = setup();
        let request = op
            .session
            .begin("brain_hold", json!({"generation":"1"}), op.now())
            .unwrap();
        let (pending, final_reply) = replies(&op, &request, true);
        peer.push(pending);
        peer.push(final_reply);
        let deadlines = delayed_transport(&mut op, peer, None, vec![]);
        assert!(op.refresh().unwrap_err().contains("deadline/queue bound"));
        assert!(!op.session.brain_fresh(op.now()));
        assert!(
            op.session.pending.is_none(),
            "valid final remains applied, never replayed"
        );
        let deadlines = deadlines.lock().unwrap();
        assert!(deadlines.len() >= 2);
        assert!(deadlines.iter().all(|d| *d == deadlines[0]));
    }
    #[test]
    fn delayed_readback_cannot_reset_the_total_frame_budget() {
        let (mut op, peer, raw) = setup();
        let request = op
            .session
            .begin("brain_hold", json!({"generation":"1"}), op.now())
            .unwrap();
        let (pending, final_reply) = replies(&op, &request, true);
        peer.push(pending);
        peer.push(final_reply.clone());
        let delayed = raw_at(&raw, "1", final_reply["applied_frame"].as_str().unwrap());
        delayed_transport(&mut op, peer, Some(delayed.clone()), vec![delayed; 61]);
        assert!(op.refresh().unwrap_err().contains("backlog saturated"));
        assert!(!op.session.brain_fresh(op.now()));
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
    struct InterruptedRead {
        replies: VecDeque<Result<Vec<u8>, String>>,
        signal: Option<Arc<crate::brain::HoldSignal>>,
        guard: Option<Arc<std::sync::atomic::AtomicU64>>,
        sent: Arc<Mutex<Vec<Value>>>,
    }
    impl AuthorityConnection for InterruptedRead {
        fn send_frame_until(&mut self, bytes: &[u8], _: Instant) -> Result<(), String> {
            self.sent
                .lock()
                .unwrap()
                .push(serde_json::from_slice(bytes).unwrap());
            Ok(())
        }
        fn receive_until(&mut self, _: Instant) -> Result<Option<Vec<u8>>, String> {
            self.receive_available()
        }
        fn receive_available(&mut self) -> Result<Option<Vec<u8>>, String> {
            if let Some(signal) = self.signal.take() {
                signal.release();
            }
            if let Some(guard) = self.guard.take() {
                guard.fetch_add(1, std::sync::atomic::Ordering::AcqRel);
            }
            self.replies.pop_front().transpose()
        }
    }
    #[test]
    fn release_during_valid_probe_does_not_poison_later_readback_or_send_heartbeat() {
        let (mut op, _, raw) = setup();
        let signal = Arc::new(crate::brain::HoldSignal::default());
        signal.press();
        op.brain_signal(signal.clone());
        let brain = op.session.brain.as_mut().unwrap();
        brain.held_generation = Some("1".into());
        brain.hold_generation_counter = "1".into();
        brain.hold_deadline_ms = Some("150".into());
        let reply = probe_reply(&op, 48);
        let sent = Arc::new(Mutex::new(Vec::new()));
        op.transport = Box::new(InterruptedRead {
            replies: VecDeque::from([Ok(serde_json::to_vec(&reply).unwrap())]),
            signal: Some(signal.clone()),
            guard: None,
            sent: sent.clone(),
        });
        assert!(op.brain_edge("brain_heartbeat", 1).is_err());
        assert!(!op.brain_probes.invalid);
        assert!(op.brain_probes.matched.is_none());
        assert!(
            sent.lock()
                .unwrap()
                .iter()
                .all(|r| r["kind"] != "heartbeat")
        );
        // The real provider settles priority close before a later hold. Supply
        // its actual correlated final and matching raw revision, not just a
        // heldNone snapshot (which intentionally cannot settle the close lane).
        let close_request = op.session.brain_close_request(op.now()).unwrap();
        let (close_pending, close_final) = replies(&op, &close_request, false);
        let mut closed = probe_reply(&op, 96);
        closed["revision"] = json!("1");
        closed["snapshot"] = close_final["snapshot"].clone();
        let mut next_press = closed.clone();
        next_press["snapshot"]["frame"] = json!("144");
        op.transport = Box::new(InterruptedRead {
            replies: VecDeque::from([
                Ok(serde_json::to_vec(&close_pending).unwrap()),
                Ok(serde_json::to_vec(&close_final).unwrap()),
                Ok(serde_json::to_vec(&raw_at(&raw, "1", "96")).unwrap()),
                Ok(serde_json::to_vec(&closed).unwrap()),
                Ok(serde_json::to_vec(&next_press).unwrap()),
            ]),
            signal: None,
            guard: None,
            sent: sent.clone(),
        });
        op.refresh_brain().unwrap();
        assert!(op.session.brain_fresh(op.now()));
        assert!(op.session.brain.as_ref().unwrap().held_generation.is_none());
        op.session.input_released();
        signal.press(); // New explicit user gesture, never automatic rearm.
        op.brain_edge("brain_hold", 2).unwrap();
        let sent = sent.lock().unwrap();
        assert_eq!(sent.iter().filter(|r| r["kind"] == "hold").count(), 1);
        assert_eq!(
            sent.iter()
                .filter(|r| r["kind"] == "brain_snapshot")
                .count(),
            3
        );
        assert_eq!(sent.last().unwrap()["body"]["generation"], "2");
    }
    #[test]
    fn revoked_context_preserves_fifo_then_requires_new_generation_probe() {
        let (mut op, peer, _) = setup();
        probe_connection(&mut op, &peer, false);
        let guard = Arc::new(std::sync::atomic::AtomicU64::new(1));
        op.guard(guard.clone(), 1);
        op.send_brain_probe(Instant::now() + Duration::from_millis(50))
            .unwrap();
        let old = probe_reply(&op, 48);
        guard.store(2, std::sync::atomic::Ordering::Release);
        assert!(op.refresh_brain().is_err());
        assert!(!op.brain_probes.invalid);
        assert_eq!(op.brain_probes.pending.len(), 1);
        op.guard(guard, 2);
        peer.push(old);
        peer.push(probe_reply(&op, 96));
        op.refresh_brain().unwrap();
        assert_eq!(op.brain_probes.matched.as_ref().unwrap().0, 2);
        assert!(op.brain_probes.pending.is_empty());
    }
    #[test]
    fn generic_read_classifies_only_pure_guard_cancellation_as_recoverable() {
        for malformed in [false, true] {
            let (mut op, _, raw) = setup();
            let guard = Arc::new(std::sync::atomic::AtomicU64::new(1));
            op.guard(guard.clone(), 1);
            let bytes = if malformed {
                b"{broken".to_vec()
            } else {
                serde_json::to_vec(&raw_at(&raw, "0", "48")).unwrap()
            };
            op.transport = Box::new(InterruptedRead {
                replies: VecDeque::from([Ok(bytes)]),
                signal: None,
                guard: Some(guard.clone()),
                sent: Arc::new(Mutex::new(Vec::new())),
            });
            let result = crate::frontend::refresh_worker_context(&mut op, &guard, 1);
            assert_eq!(result.as_ref().is_ok_and(|admitted| !admitted), !malformed);
            if malformed {
                assert!(result.is_err());
            }
            assert!(
                op.session.lease_deadline().is_some(),
                "classification leaves owner retirement to worker"
            );
            if !malformed {
                op.cancel(); // actual worker's next-generation transition
                op.guard(guard, 2);
                let peer = FakeAuthorityConnection::default();
                probe_connection(&mut op, &peer, false);
                peer.push(probe_reply(&op, 96));
                op.refresh_brain().unwrap();
                assert!(op.session.brain_fresh(op.now()));
            }
        }
    }
    #[test]
    fn cancellation_cannot_mask_malformed_or_partial_probe_and_first_fault_persists() {
        for response in [
            Ok(b"{broken".to_vec()),
            Err("partial frame deadline".to_string()),
        ] {
            let (mut op, _, _) = setup();
            let guard = Arc::new(std::sync::atomic::AtomicU64::new(1));
            op.guard(guard.clone(), 1);
            op.transport = Box::new(InterruptedRead {
                replies: VecDeque::from([response]),
                signal: None,
                guard: Some(guard),
                sent: Arc::new(Mutex::new(Vec::new())),
            });
            let first = op.refresh_brain().unwrap_err();
            assert!(op.brain_probes.invalid);
            assert_eq!(op.brain_probes.first_fault.as_deref(), Some(first.as_str()));
            assert_eq!(
                op.send_brain_probe(Instant::now() + Duration::from_millis(50))
                    .unwrap_err(),
                first
            );
        }
    }
    #[test]
    fn priority_close_send_failure_remains_terminal_with_revoked_context() {
        let (mut op, peer, _) = setup();
        op.session.brain.as_mut().unwrap().held_generation = Some("1".into());
        let signal = Arc::new(crate::brain::HoldSignal::default());
        signal.press();
        signal.release();
        op.brain_signal(signal);
        let guard = Arc::new(std::sync::atomic::AtomicU64::new(2));
        op.guard(guard, 1);
        probe_connection(&mut op, &peer, true);
        let error = op.check_guard().unwrap_err();
        assert_eq!(error, "partial probe send");
        assert!(op.brain_probes.invalid);
        assert_eq!(op.brain_probes.first_fault.as_deref(), Some(error.as_str()));
    }
    struct CurrentRevisionRenew {
        raw: Value,
        brain: Value,
        replies: VecDeque<Vec<u8>>,
        requests: Arc<Mutex<Vec<Value>>>,
        race: bool,
    }
    impl AuthorityConnection for CurrentRevisionRenew {
        fn send_frame_until(&mut self, bytes: &[u8], _: Instant) -> Result<(), String> {
            let request: Value = serde_json::from_slice(bytes).unwrap();
            self.requests.lock().unwrap().push(request.clone());
            let reply = match request["kind"].as_str().unwrap() {
                "brain_snapshot" => self.brain.clone(),
                "snapshot" => self.raw.clone(),
                "renew" => {
                    let refused = self.race || request["expected_revision"] != "1";
                    let mut reply: Value = serde_json::from_slice(include_bytes!(
                        "../tests/fixtures/gp15/v1/grant-16-talkback-reply.json"
                    ))
                    .unwrap();
                    for key in [
                        "show_id",
                        "module",
                        "epoch",
                        "writer",
                        "lease",
                        "request_id",
                        "expected_revision",
                    ] {
                        reply["context"][key] = request[key].clone();
                        reply["outcome"][key] = request[key].clone();
                    }
                    reply["snapshot"] = Value::Null;
                    reply["outcome"]["body"]["granted_lease"] = Value::Null;
                    reply["outcome"]["body"]["revision"] = json!(if self.race { "2" } else { "1" });
                    if refused {
                        reply["outcome"]["kind"] = json!("conflict");
                        reply["outcome"]["body"]["reason"] = json!("stale_revision");
                        reply["outcome"]["body"]["lease_remaining_ms"] = Value::Null;
                        reply["outcome"]["body"]["scope"] = Value::Null;
                    }
                    reply
                }
                other => panic!("unexpected operation {other}"),
            };
            self.replies.push_back(serde_json::to_vec(&reply).unwrap());
            Ok(())
        }
        fn receive_until(&mut self, _: Instant) -> Result<Option<Vec<u8>>, String> {
            Ok(self.replies.pop_front())
        }
        fn receive_available(&mut self) -> Result<Option<Vec<u8>>, String> {
            Ok(self.replies.pop_front())
        }
    }
    #[test]
    fn gp15_renew_anchors_new_probe_and_raw_before_pinning_and_reports_later_conflict() {
        for race in [false, true] {
            let (mut op, peer, raw) = setup();
            // An old progressing query reply must not satisfy the new renewal probe.
            probe_connection(&mut op, &peer, false);
            op.send_brain_probe(Instant::now() + Duration::from_millis(50))
                .unwrap();
            let old = probe_reply(&op, 48);
            let mut current = probe_reply(&op, 96);
            current["revision"] = json!("1");
            current["snapshot"]["revision"] = json!("1");
            let requests = Arc::new(Mutex::new(Vec::new()));
            op.transport = Box::new(CurrentRevisionRenew {
                raw: raw_at(&raw, "1", "96"),
                brain: current,
                replies: VecDeque::from([serde_json::to_vec(&old).unwrap()]),
                requests: requests.clone(),
                race,
            });
            op.start = Instant::now() - Duration::from_millis(100);
            assert!(op.session.brain_fresh(op.now()));
            let expiry = op.session.lease_deadline();
            let result = op.mutate_inner("renew", json!({}));
            if race {
                assert!(result.unwrap_err().contains("stale_revision"));
                assert_eq!(op.session.lease_deadline(), expiry);
            } else {
                result.unwrap();
                assert!(op.session.lease_deadline().unwrap() >= expiry.unwrap() + 100);
            }
            assert!(op.session.pending.is_none());
            let requests = requests.lock().unwrap();
            let kinds: Vec<_> = requests
                .iter()
                .map(|r| r["kind"].as_str().unwrap())
                .collect();
            assert_eq!(kinds, ["brain_snapshot", "snapshot", "renew"]);
            assert_eq!(requests[2]["expected_revision"], "1");
            assert_eq!(op.brain_probes.matched.as_ref().unwrap().0, 2);
        }
    }
    struct RenewBackpressure {
        sent: Arc<Mutex<Vec<Vec<u8>>>>,
        replies: VecDeque<Vec<u8>>,
        started: Instant,
    }
    impl AuthorityConnection for RenewBackpressure {
        fn send_frame_until(&mut self, bytes: &[u8], _: Instant) -> Result<(), String> {
            let mut sent = self.sent.lock().unwrap();
            let request: Value = serde_json::from_slice(bytes).unwrap();
            assert_eq!(request["kind"], "renew");
            let context: serde_json::Map<String, Value> = [
                "show_id",
                "module",
                "epoch",
                "writer",
                "lease",
                "request_id",
                "expected_revision",
            ]
            .into_iter()
            .map(|k| (k.into(), request[k].clone()))
            .collect();
            let mut reply: Value = serde_json::from_slice(include_bytes!(
                "../tests/fixtures/gp15/v1/grant-16-talkback-reply.json"
            ))
            .unwrap();
            reply["context"] = Value::Object(context.clone());
            if sent.is_empty() {
                reply["state"] = json!("backpressure");
                reply["outcome"] = Value::Null;
            } else {
                assert_eq!(sent.len(), 1);
                assert_eq!(bytes, sent[0], "retry must preserve every request byte");
                assert!(
                    self.started.elapsed() < Duration::from_millis(500),
                    "retry missed its existing100ms wake"
                );
                for (k, v) in context {
                    reply["outcome"][k] = v;
                }
                reply["outcome"]["body"]["granted_lease"] = Value::Null;
            }
            reply["snapshot"] = Value::Null;
            sent.push(bytes.to_vec());
            self.replies.push_back(serde_json::to_vec(&reply).unwrap());
            Ok(())
        }
        fn receive_until(&mut self, deadline: Instant) -> Result<Option<Vec<u8>>, String> {
            if let Some(reply) = self.replies.pop_front() {
                return Ok(Some(reply));
            }
            std::thread::sleep(deadline.saturating_duration_since(Instant::now()));
            Ok(None)
        }
        fn receive_available(&mut self) -> Result<Option<Vec<u8>>, String> {
            Ok(self.replies.pop_front())
        }
    }
    #[test]
    fn renew_backpressure_silence_wakes_identical_retry_before_lease_expiry() {
        let (mut op, _, raw) = setup();
        op.session.brain = None; // This test isolates ordinary C-AUDIO retry/framing.
        op.start = Instant::now() - Duration::from_millis(500);
        let revision = op
            .session
            .snapshot
            .as_ref()
            .unwrap()
            .authority
            .revision
            .clone();
        let fresh =
            audio::decode_reply(&serde_json::to_vec(&raw_at(&raw, &revision, "48")).unwrap())
                .unwrap()
                .snapshot
                .unwrap();
        op.session.ingest_snapshot(fresh, op.now()).unwrap();
        let original_expiry = op.session.lease_deadline().unwrap();
        let sent = Arc::new(Mutex::new(Vec::new()));
        op.transport = Box::new(RenewBackpressure {
            sent: sent.clone(),
            replies: VecDeque::new(),
            started: Instant::now(),
        });
        op.mutate_inner("renew", json!({})).unwrap();
        assert_eq!(sent.lock().unwrap().len(), 2);
        assert!(op.session.pending.is_none());
        assert!(op.session.lease_deadline().unwrap() >= original_expiry + 500);
        assert!(op.now() < original_expiry);
    }
    #[test]
    fn renew_partial_prefix_timeout_is_fatal_without_request_retry() {
        let (mut op, _, _) = setup();
        op.session.brain = None; // This test isolates ordinary C-AUDIO retry/framing.
        let (socket, mut server) = UnixStream::pair().unwrap();
        let child = std::thread::spawn(move || {
            let mut prefix = [0; 4];
            server.read_exact(&mut prefix).unwrap();
            let mut body = vec![0; u32::from_be_bytes(prefix) as usize];
            server.read_exact(&mut body).unwrap();
            server.write_all(&[0]).unwrap();
            std::thread::sleep(Duration::from_millis(200));
            server
                .set_read_timeout(Some(Duration::from_millis(30)))
                .unwrap();
            let mut byte = [0];
            let result = server.read(&mut byte);
            assert!(
                matches!(result, Err(ref e) if matches!(e.kind(), std::io::ErrorKind::WouldBlock | std::io::ErrorKind::TimedOut)),
                "partial reply must not trigger retransmission: {result:?}"
            );
        });
        op.transport = Box::new(Transport { socket });
        op.start = Instant::now();
        assert!(op.mutate_inner("renew", json!({})).is_err());
        assert!(op.session.lease_deadline().is_none());
        assert!(op.session.pending.is_none());
        child.join().unwrap();
    }
    fn probe_reply(op: &Operator, frame: u64) -> Value {
        let mut snapshot = op.session.brain.clone().unwrap();
        snapshot.frame = frame.to_string();
        json!({"contract":"GP15-brain","version":1,"state":"snapshot","reason":null,
            "context":op.session.brain_request().context,"revision":snapshot.revision,
            "applied_frame":null,"snapshot":snapshot})
    }
    struct ProbeConnection {
        peer: FakeAuthorityConnection,
        sent: Arc<Mutex<Vec<Value>>>,
        fail: bool,
        first_delay: Option<Duration>,
        deadlines: Option<Arc<Mutex<Vec<Instant>>>>,
    }
    impl AuthorityConnection for ProbeConnection {
        fn send_frame_until(&mut self, bytes: &[u8], deadline: Instant) -> Result<(), String> {
            if let Some(deadlines) = &self.deadlines {
                deadlines.lock().unwrap().push(deadline);
            }
            self.sent
                .lock()
                .unwrap()
                .push(serde_json::from_slice(bytes).unwrap());
            if self.fail {
                Err("partial probe send".into())
            } else {
                Ok(())
            }
        }
        fn receive_until(&mut self, deadline: Instant) -> Result<Option<Vec<u8>>, String> {
            if let Some(deadlines) = &self.deadlines {
                deadlines.lock().unwrap().push(deadline);
            }
            if let Some(delay) = self.first_delay.take() {
                std::thread::sleep(delay);
            }
            self.peer.receive_available()
        }
        fn receive_available(&mut self) -> Result<Option<Vec<u8>>, String> {
            self.peer.receive_available()
        }
    }
    fn probe_connection(
        op: &mut Operator,
        peer: &FakeAuthorityConnection,
        fail: bool,
    ) -> Arc<Mutex<Vec<Value>>> {
        let sent = Arc::new(Mutex::new(Vec::new()));
        op.transport = Box::new(ProbeConnection {
            peer: peer.clone(),
            sent: sent.clone(),
            fail,
            first_delay: None,
            deadlines: None,
        });
        sent
    }
    #[test]
    fn brain_probe_waits_for_own_reply_not_buffered_progress() {
        let (mut op, peer, _) = setup();
        probe_connection(&mut op, &peer, false);
        let base = op
            .session
            .brain
            .as_ref()
            .unwrap()
            .frame
            .parse::<u64>()
            .unwrap();
        let old = op
            .send_brain_probe(Instant::now() + Duration::from_millis(50))
            .unwrap();
        peer.push(probe_reply(&op, base + 48));
        peer.push(probe_reply(&op, base + 96));
        op.refresh_brain().unwrap();
        assert_eq!(op.brain_probes.matched.as_ref().unwrap().0, old + 1);
        assert_eq!(
            op.brain_probes.matched.as_ref().unwrap().2.frame,
            (base + 96).to_string()
        );
        assert!(op.brain_probes.pending.is_empty());
        assert!(peer.0.lock().unwrap().is_empty());
    }
    #[test]
    fn equal_snapshot_consumes_probe_but_final_never_does() {
        let (mut op, peer, _) = setup();
        probe_connection(&mut op, &peer, false);
        let id = op
            .send_brain_probe(Instant::now() + Duration::from_millis(50))
            .unwrap();
        let frame = op
            .session
            .brain
            .as_ref()
            .unwrap()
            .frame
            .parse::<u64>()
            .unwrap();
        op.processing_frame(&serde_json::to_vec(&probe_reply(&op, frame)).unwrap())
            .unwrap();
        assert_eq!(op.brain_probes.matched.as_ref().unwrap().0, id);
        assert!(op.brain_probes.pending.is_empty());
        op.send_brain_probe(Instant::now() + Duration::from_millis(50))
            .unwrap();
        let r = op
            .session
            .begin("brain_hold", json!({"generation":"1"}), op.now())
            .unwrap();
        let (pending, final_reply) = replies(&op, &r, true);
        for reply in [pending, final_reply] {
            op.processing_frame(&serde_json::to_vec(&reply).unwrap())
                .unwrap();
        }
        assert_eq!(op.brain_probes.pending.len(), 1);
        assert_eq!(op.brain_probes.matched.as_ref().unwrap().0, id);
    }
    #[test]
    fn heartbeat_uses_probe_send_age_and_exact_frame_not_newer_cached_final() {
        let (mut op, _, _) = setup();
        let mut snapshot = op.session.brain.clone().unwrap();
        snapshot.held_generation = Some("1".into());
        op.session.brain = Some(snapshot.clone());
        let now = Instant::now();
        op.brain_probes.matched = Some((1, now - Duration::from_millis(31), snapshot.clone()));
        // Receipt is fresh, but a probe with insufficient send budget is refused.
        assert!(op.session.brain_age(op.now()).unwrap() < 50);
        assert!(op.heartbeat_probe_body(1, now).is_err());
        op.brain_probes.matched = Some((1, now - Duration::from_millis(29), snapshot.clone()));
        op.session.brain.as_mut().unwrap().frame = "999999".into();
        let (body, deadline) = op.heartbeat_probe_body(1, now).unwrap();
        assert_eq!(body["observed_frame"], snapshot.frame);
        assert_eq!(deadline, now + Duration::from_millis(20));
        op.session.brain.as_mut().unwrap().held_generation = None;
        assert!(op.heartbeat_probe_body(1, now).is_err());
    }
    #[test]
    fn heartbeat_edge_queries_own_fresh_frame_before_single_send() {
        let (mut op, peer, raw) = setup();
        let sent = probe_connection(&mut op, &peer, false);
        let brain = op.session.brain.as_mut().unwrap();
        brain.held_generation = Some("1".into());
        brain.hold_generation_counter = "1".into();
        brain.hold_deadline_ms = Some("150".into());
        let frame = brain.frame.parse::<u64>().unwrap() + 48;
        let _ = raw;
        peer.push(probe_reply(&op, frame));
        op.brain_edge("brain_heartbeat", 1).unwrap();
        let sent = sent.lock().unwrap();
        assert_eq!(sent.len(), 2);
        assert_eq!(sent[0]["kind"], "brain_snapshot");
        assert_eq!(sent[1]["kind"], "heartbeat");
        assert_eq!(sent[1]["body"]["observed_frame"], frame.to_string());
    }
    #[test]
    fn brain_probe_requests_expired_same_revision_raw_before_admitting_pair() {
        let (mut op, peer, raw) = setup();
        let sent = Arc::new(Mutex::new(Vec::new()));
        op.transport = Box::new(ProbeConnection {
            peer: peer.clone(),
            sent: sent.clone(),
            fail: false,
            first_delay: Some(Duration::from_millis(10)),
            deadlines: None,
        });
        // Raw starts fresh, then expires while the Brain reply is in flight.
        op.start = Instant::now() - Duration::from_millis(245);
        assert!(op.session.fresh(op.now()));
        let frame = op
            .session
            .brain
            .as_ref()
            .unwrap()
            .frame
            .parse::<u64>()
            .unwrap()
            + 48;
        peer.push(probe_reply(&op, frame));
        peer.push(raw_at(
            &raw,
            &op.session.brain.as_ref().unwrap().revision,
            &frame.to_string(),
        ));
        op.refresh_brain().unwrap();
        assert!(op.session.brain_fresh(op.now()));
        let sent = sent.lock().unwrap();
        assert_eq!(sent.len(), 2);
        assert_eq!(sent[0]["kind"], "brain_snapshot");
        assert_eq!(sent[1]["kind"], "snapshot");
    }
    #[test]
    fn superseded_brain_pair_reprobes_without_replaying_mutation() {
        let (mut op, peer, raw) = setup();
        let sent = probe_connection(&mut op, &peer, false);
        op.start = Instant::now() - Duration::from_millis(251);
        let frame = op
            .session
            .brain
            .as_ref()
            .unwrap()
            .frame
            .parse::<u64>()
            .unwrap()
            + 48;
        let old = probe_reply(&op, frame);
        let revision = op
            .session
            .brain
            .as_ref()
            .unwrap()
            .revision
            .parse::<u64>()
            .unwrap()
            + 1;
        let mut advanced = old.clone();
        advanced["revision"] = json!(revision.to_string());
        advanced["snapshot"]["revision"] = json!(revision.to_string());
        advanced["snapshot"]["frame"] = json!((frame + 48).to_string());
        peer.push(old);
        peer.push(raw_at(
            &raw,
            &revision.to_string(),
            &(frame + 48).to_string(),
        ));
        peer.push(advanced);
        op.refresh_brain().unwrap();
        assert!(op.session.brain_fresh(op.now()));
        assert_eq!(op.brain_probes.matched.as_ref().unwrap().0, 2);
        assert_eq!(
            op.brain_probes.matched.as_ref().unwrap().2.revision,
            revision.to_string()
        );
        let sent = sent.lock().unwrap();
        let kinds: Vec<_> = sent.iter().map(|r| r["kind"].as_str().unwrap()).collect();
        assert_eq!(kinds, ["brain_snapshot", "snapshot", "brain_snapshot"]);
        assert!(sent.iter().all(|r| r["writer"].is_null()));
    }
    #[test]
    fn continual_revision_churn_keeps_original_probe_deadline_and_frame_bound() {
        let (mut op, peer, raw) = setup();
        let sent = Arc::new(Mutex::new(Vec::new()));
        let deadlines = Arc::new(Mutex::new(Vec::new()));
        op.transport = Box::new(ProbeConnection {
            peer: peer.clone(),
            sent: sent.clone(),
            fail: false,
            first_delay: None,
            deadlines: Some(deadlines.clone()),
        });
        op.start = Instant::now() - Duration::from_millis(251);
        let frame = op
            .session
            .brain
            .as_ref()
            .unwrap()
            .frame
            .parse::<u64>()
            .unwrap()
            + 48;
        let base = op
            .session
            .brain
            .as_ref()
            .unwrap()
            .revision
            .parse::<u64>()
            .unwrap();
        let initial = probe_reply(&op, frame);
        peer.push(initial.clone());
        peer.push(raw_at(
            &raw,
            &(base + 1).to_string(),
            &(frame + 48).to_string(),
        ));
        for i in 1..34 {
            // The next raw revision overtakes each pending Brain response.
            peer.push(raw_at(
                &raw,
                &(base + i + 1).to_string(),
                &(frame + (i + 1) * 48).to_string(),
            ));
            let mut brain = initial.clone();
            brain["revision"] = json!((base + i).to_string());
            brain["snapshot"]["revision"] = json!((base + i).to_string());
            brain["snapshot"]["frame"] = json!((frame + i * 48).to_string());
            peer.push(brain);
        }
        // Isolate the frame bound from debug decode throughput. Production
        // entrypoints still supply250ms/30ms; their timeout tests remain separate.
        let original_deadline = Instant::now() + Duration::from_secs(10);
        assert!(
            op.refresh_brain_until(original_deadline)
                .unwrap_err()
                .contains("deadline")
        );
        assert!(op.brain_probes.invalid);
        assert_eq!(peer.0.lock().unwrap().len(), 4, "exactly64 frames consumed");
        let deadlines = deadlines.lock().unwrap();
        assert!(!deadlines.is_empty());
        assert!(deadlines.iter().all(|d| *d == original_deadline));
        assert!(sent.lock().unwrap().iter().all(|r| r["writer"].is_null()));
    }
    #[test]
    fn probe_partial_send_unmatched_reply_and_overflow_poison_provenance() {
        let (mut op, peer, _) = setup();
        probe_connection(&mut op, &peer, true);
        assert!(
            op.send_brain_probe(Instant::now() + Duration::from_millis(50))
                .is_err()
        );
        assert!(op.brain_probes.invalid);
        assert!(op.brain_probes.pending.is_empty());
        let (mut op, peer, _) = setup();
        probe_connection(&mut op, &peer, false);
        let frame = op
            .session
            .brain
            .as_ref()
            .unwrap()
            .frame
            .parse::<u64>()
            .unwrap();
        assert!(
            op.processing_frame(&serde_json::to_vec(&probe_reply(&op, frame)).unwrap())
                .is_err()
        );
        assert!(op.brain_probes.invalid);
        let (mut op, peer, _) = setup();
        probe_connection(&mut op, &peer, false);
        for _ in 0..64 {
            op.send_brain_probe(Instant::now() + Duration::from_millis(50))
                .unwrap();
        }
        assert!(
            op.send_brain_probe(Instant::now() + Duration::from_millis(50))
                .is_err()
        );
        assert!(op.brain_probes.invalid);
        let (mut op, peer, _) = setup();
        probe_connection(&mut op, &peer, false);
        op.send_brain_probe(Instant::now() + Duration::from_millis(50))
            .unwrap();
        let frame = op
            .session
            .brain
            .as_ref()
            .unwrap()
            .frame
            .parse::<u64>()
            .unwrap();
        op.cancel();
        assert!(!op.brain_probes.invalid);
        assert_eq!(op.brain_probes.pending.len(), 1);
        op.processing_frame(&serde_json::to_vec(&probe_reply(&op, frame)).unwrap())
            .unwrap();
        assert!(op.brain_probes.matched.is_none());
        assert!(op.brain_probes.pending.is_empty());
        op.send_brain_probe(Instant::now() + Duration::from_millis(50))
            .unwrap();
        op.processing_frame(&serde_json::to_vec(&probe_reply(&op, frame)).unwrap())
            .unwrap();
        assert!(op.brain_probes.matched.is_some());
    }
}
