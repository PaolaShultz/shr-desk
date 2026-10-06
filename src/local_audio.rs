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
/// Opt-in fixed-size cumulative transport counters. Stage microseconds are:
/// send, first byte await, remaining frame await, assembly, envelope decode,
/// payload serialization, outbound envelope serialization, write await,
/// outbound command parse/envelope construction.
/// Await includes runtime/QUIC scheduling, not just wire time.
#[derive(Clone, Copy, Debug, Default)]
pub struct TransportTiming {
    pub session: u64,
    pub send_attempts: u64,
    pub completed_sends: u64,
    pub completed_reply_documents: u64,
    pub micros: [u64; 9],
    pub frames: u64,
    pub bytes: u64,
    pub overflow: bool,
    /// RX then TX: MAX_DATA, MAX_STREAM_DATA, DATA_BLOCKED, STREAM_DATA_BLOCKED.
    pub flow: [u64; 8],
    pub rtt_us: u64,
    pub lost_packets: u64,
    /// Connection-wide RX then TX UDP datagram counters.
    pub udp_datagrams: [u64; 2],
}
pub(crate) fn trace_timing_enabled() -> bool {
    std::env::var_os("SHR_DESK_TRACE_TIMING").is_some_and(|v| v == "1")
}
pub(crate) fn trace_add(value: &mut u64, amount: u64, overflow: &mut bool) {
    match value.checked_add(amount) {
        Some(next) => *value = next,
        None => {
            *value = u64::MAX;
            *overflow = true;
        }
    }
}
pub(crate) fn trace_us(duration: Duration) -> u64 {
    duration.as_micros().min(u64::MAX as u128) as u64
}
#[derive(Clone, Copy, Debug, Default)]
struct ProbeFrameTiming {
    bytes: u64,
    // receive wall, contract discriminator/dispatch, raw decode, telemetry/ingest
    micros: [u64; 4],
    kind: u8,
    receive_start_us: u64,
    transport_before: Option<TransportTiming>,
    transport_after: Option<TransportTiming>,
}
#[derive(Debug, Default)]
struct ProbeTiming {
    totals: [u64; 4],
    sends_us: u64,
    decision_us: u64,
    empty_receive_us: u64,
    frames: u64,
    overwritten: u64,
    overflow: bool,
    tail: [ProbeFrameTiming; 4],
}
impl ProbeTiming {
    fn record(&mut self, frame: ProbeFrameTiming) {
        for (total, amount) in self.totals.iter_mut().zip(frame.micros) {
            trace_add(total, amount, &mut self.overflow);
        }
        if self.frames >= 4 {
            trace_add(&mut self.overwritten, 1, &mut self.overflow);
        }
        self.tail[(self.frames % 4) as usize] = frame;
        trace_add(&mut self.frames, 1, &mut self.overflow);
    }
}
pub trait AuthorityConnection: Send {
    fn held_identity(&self) -> Option<crate::held_proof::Identity> {
        None
    }
    /// Optional parsed receive. Default adapters use the same strict byte parser;
    /// implementations cannot construct a proof from an arbitrary Value.
    fn receive_document_until(
        &mut self,
        deadline: Instant,
    ) -> Result<Option<crate::provider::StrictDocument>, String> {
        self.receive_until(deadline)?
            .map(|bytes| crate::provider::StrictDocument::parse(&bytes))
            .transpose()
    }
    fn timing_snapshot(&self) -> Option<TransportTiming> {
        None
    }
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
struct HeldBaseline {
    identity: crate::held_proof::Identity,
    template: crate::held_proof::Request,
    dimensions: [u32; 6],
}
struct HeldQuery {
    request: crate::held_proof::Request,
    sent: Instant,
    generation: u64,
    dimensions: [u32; 6],
}
pub struct Operator {
    held_baseline: Option<HeldBaseline>,
    held_query: Option<HeldQuery>,
    held_matched: Option<crate::held_proof::Matched>,
    held_nonce: u64,
    held_highwater: u64,
    held_observation: Option<crate::held_proof::Status>,
    held_refusal: Option<String>,
    brain_probes: BrainProbes,
    last_brain_send: Option<Instant>,
    trace_timing: bool,
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
            held_baseline: None,
            held_query: None,
            held_matched: None,
            held_nonce: 0,
            held_highwater: 0,
            held_observation: None,
            held_refusal: None,
            last_brain_send: None,
            trace_timing: trace_timing_enabled(),
        })
    }
    pub(crate) fn cancel(&mut self) {
        // Keep FIFO ordinals for replies already in flight, but none of their
        // observations may authorize input in the next controller generation.
        self.brain_probes.matched = None;
        self.held_matched = None;
        self.held_observation = None;
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
    fn processing_document(
        &mut self,
        document: crate::provider::StrictDocument,
    ) -> Result<Option<crate::provider::StrictDocument>, String> {
        let contract = match document.value().get("contract") {
            None | Some(serde_json::Value::Null) => None,
            Some(serde_json::Value::String(s)) => Some(s.as_str()),
            _ => return Err("contract discriminator type".into()),
        };
        if contract == Some("GP07-processing")
            && document.value()["version"] != 3
            && document.admitted_bytes() > crate::provider::MAX_BYTES
        {
            return Err("legacy processing capacity".into());
        }
        if matches!(
            contract,
            Some(
                "GP15-device"
                    | "GP15-brain"
                    | "GP15-held-proof"
                    | "GP14-structure"
                    | "GP07-processing"
            )
        ) {
            self.processing_frame(&document.into_bytes()?)?;
            Ok(None)
        } else {
            Ok(Some(document))
        }
    }
    fn processing_frame(&mut self, bytes: &[u8]) -> Result<bool, String> {
        // Discriminator only: skipped content is never trusted. The selected
        // strict decoder still checks every field, duplicate and depth bound.
        #[derive(serde::Deserialize)]
        struct Contract {
            contract: Option<String>,
        }
        let tag: Contract = serde_json::from_slice(bytes).map_err(|e| e.to_string())?;
        if tag.contract.as_deref() == Some(crate::held_proof::CONTRACT) {
            let query = self.held_query.as_ref().ok_or("unmatched held proof")?;
            let reply = crate::held_proof::Reply::decode(bytes, query.dimensions)?;
            if reply.context != query.request {
                return Err("held proof query/context mismatch".into());
            }
            let query = self.held_query.take().unwrap();
            if query.generation != self.session.generation() {
                return Ok(true);
            }
            if reply.witness.is_none() {
                self.held_baseline = None;
                self.held_matched = None;
                self.held_observation = None;
                self.held_refusal = Some(format!("held proof refused: {:?}", reply.reason));
                return Ok(true);
            }
            let matched = crate::held_proof::Matched::admit(
                reply,
                &query.request,
                query.sent,
                query.generation,
            )?;
            let w = matched.witness();
            let highwater = crate::provider::counter(&w.brain.hold_generation_counter)?;
            let known = self
                .session
                .brain
                .as_ref()
                .map(|b| crate::provider::counter(&b.hold_generation_counter))
                .transpose()?
                .unwrap_or(0);
            if highwater < known.max(self.held_highwater) {
                return Err("held proof generation highwater regressed".into());
            }
            let revision = crate::provider::counter(&w.revision)?;
            let frame = crate::provider::counter(&w.source_frame)?;
            if self.session.snapshot.as_ref().is_some_and(|s| {
                revision < crate::provider::counter(&s.authority.revision).unwrap()
                    || frame < crate::provider::counter(&s.frame).unwrap()
            }) || self.session.brain.as_ref().is_some_and(|s| {
                revision < crate::provider::counter(&s.revision).unwrap()
                    || frame < crate::provider::counter(&s.frame).unwrap()
            }) {
                return Err("held proof regresses known authority".into());
            }
            if self.held_observation.as_ref().is_some_and(|old| {
                crate::provider::counter(&w.revision).unwrap()
                    < crate::provider::counter(&old.revision).unwrap()
                    || crate::provider::counter(&w.source_frame).unwrap()
                        < crate::provider::counter(&old.source_frame).unwrap()
            }) {
                return Err("regressive held proof".into());
            }
            self.held_highwater = highwater;
            self.held_observation = Some(crate::held_proof::Status {
                generation: w.brain.held_generation.clone(),
                source_frame: w.source_frame.clone(),
                revision: w.revision.clone(),
                talkback_path_ready: w.brain.talkback_path_ready,
                media_authorized: w.media_authorized,
                observed: query.sent,
                valid_until: query.sent
                    + Duration::from_millis(u64::from(w.lease_remaining_ms).min(50)),
            });
            self.held_matched = Some(matched);
            return Ok(true);
        }
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
        self.held_baseline = None;
        self.held_matched = None;
        self.held_observation = None;
        self.held_query = None;
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
        self.refresh_brain_budget(deadline, &mut 64, false)
    }
    fn probe_guard(&mut self, held_mode: bool) -> Result<(), BrainOperationError> {
        if held_mode {
            self.held_guard()
        } else {
            self.brain_guard()
        }
    }
    fn refresh_brain_budget(
        &mut self,
        deadline: Instant,
        remaining: &mut usize,
        held_mode: bool,
    ) -> Result<(), BrainOperationError> {
        let began = Instant::now();
        let mut timing = self.trace_timing.then(ProbeTiming::default);
        let transport_before = timing
            .as_ref()
            .and_then(|_| self.transport.timing_snapshot());
        let budget = deadline.saturating_duration_since(began);
        self.probe_guard(held_mode)?;
        let send_started = timing.as_ref().map(|_| Instant::now());
        let mut probe = self.send_brain_probe(deadline)?;
        let mut requested_revision = None;
        if held_mode && !self.session.fresh(self.now()) {
            // A pending final already made raw readback necessary. Pipeline it
            // behind our timestamped Brain query rather than waiting for that
            // reply first. Raw has no query identity: only exact revision pairing
            // with the independently matched Brain probe can authorize anything.
            self.probe_guard(held_mode)?;
            requested_revision = self.session.brain.as_ref().map(|b| b.revision.clone());
            self.transport
                .send_frame_until(&self.session.snapshot_request().encode()?, deadline)?;
        }
        if let (Some(t), Some(start)) = (&mut timing, send_started) {
            trace_add(&mut t.sends_us, trace_us(start.elapsed()), &mut t.overflow);
        }
        let mut loops = 0usize;
        let mut processed = 0usize;
        for _ in 0..64 {
            loops += 1;
            self.probe_guard(held_mode)?;
            if Instant::now() >= deadline {
                break;
            }
            if *remaining == 0 {
                break;
            }
            let receive_started = timing.as_ref().map(|_| Instant::now());
            let frame_transport_before = timing
                .as_ref()
                .and_then(|_| self.transport.timing_snapshot());
            if let Some(document) = self.transport.receive_document_until(deadline)? {
                let mut frame = ProbeFrameTiming {
                    bytes: document.admitted_bytes() as u64,
                    receive_start_us: receive_started
                        .map_or(0, |start| trace_us(start.saturating_duration_since(began))),
                    transport_before: frame_transport_before,
                    transport_after: timing
                        .as_ref()
                        .and_then(|_| self.transport.timing_snapshot()),
                    ..Default::default()
                };
                if let Some(start) = receive_started {
                    frame.micros[0] = trace_us(start.elapsed());
                }
                let dispatch_started = timing.as_ref().map(|_| Instant::now());
                *remaining -= 1;
                let raw_document = self.processing_document(document)?;
                let contract = raw_document.is_none();
                if let Some(start) = dispatch_started {
                    frame.micros[1] = trace_us(start.elapsed());
                }
                frame.kind = if contract { 1 } else { 2 };
                if let Some(document) = raw_document {
                    let decode_started = timing.as_ref().map(|_| Instant::now());
                    let r = audio::decode_reply_document(document)?;
                    if let Some(start) = decode_started {
                        frame.micros[2] = trace_us(start.elapsed());
                    }
                    let ingest_started = timing.as_ref().map(|_| Instant::now());
                    self.telemetry(&r)?;
                    if let Some(start) = ingest_started {
                        frame.micros[3] = trace_us(start.elapsed());
                    }
                }
                if let Some(t) = &mut timing {
                    t.record(frame);
                }
                let decision_started = timing.as_ref().map(|_| Instant::now());
                processed += 1;
                // Validate bytes/correlation before observing concurrent cancellation:
                // a focus change must never hide malformed or partial wire data.
                self.probe_guard(held_mode)?;
                if Instant::now() >= deadline {
                    break;
                }
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
                    let started = timing.as_ref().map(|_| Instant::now());
                    probe = self.send_brain_probe(deadline)?;
                    if let (Some(t), Some(start)) = (&mut timing, started) {
                        trace_add(&mut t.sends_us, trace_us(start.elapsed()), &mut t.overflow);
                    }
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
                    let started = timing.as_ref().map(|_| Instant::now());
                    self.transport
                        .send_frame_until(&self.session.snapshot_request().encode()?, deadline)?;
                    if let (Some(t), Some(start)) = (&mut timing, started) {
                        trace_add(&mut t.sends_us, trace_us(start.elapsed()), &mut t.overflow);
                    }
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
                    self.pin_held_baseline()?;
                    return Ok(());
                }
                if let (Some(t), Some(start)) = (&mut timing, decision_started) {
                    trace_add(
                        &mut t.decision_us,
                        trace_us(start.elapsed()),
                        &mut t.overflow,
                    );
                }
            } else if let (Some(t), Some(start)) = (&mut timing, receive_started) {
                trace_add(
                    &mut t.empty_receive_us,
                    trace_us(start.elapsed()),
                    &mut t.overflow,
                );
            }
        }
        self.probe_guard(held_mode)?;
        let at = Instant::now();
        let now = self.now();
        let pending = self.brain_probes.pending.iter().find(|p| p.0 == probe);
        let matched = self.brain_probes.matched.as_ref();
        // Only validated numeric identities/revisions and bounded scalar state:
        // never include a packet, credential, destination list or unbounded queue.
        let mut failure = format!(
            "Brain paired observation deadline: elapsed_ms={} budget_ms={} processed={} loops={} probe={} probe_age_ms={:?} probe_generation={:?} generation={} matched_id={:?} matched_age_ms={:?} matched_revision={:?} matched_generation={:?} raw_revision={:?} raw_age_ms={:?} raw_fresh={} brain_revision={:?} brain_age_ms={:?} brain_fresh={} requested_revision={:?} pending={}",
            at.saturating_duration_since(began).as_millis(),
            budget.as_millis(),
            processed,
            loops,
            probe,
            pending
                .map(|p| at.saturating_duration_since(p.1).as_millis())
                .or_else(|| matched
                    .filter(|m| m.0 == probe)
                    .map(|m| at.saturating_duration_since(m.1).as_millis())),
            pending.map(|p| p.2).or_else(|| matched
                .filter(|m| m.0 == probe)
                .map(|_| self.session.generation())),
            self.session.generation(),
            matched.map(|m| m.0),
            matched.map(|m| at.saturating_duration_since(m.1).as_millis()),
            matched.map(|m| m.2.revision.as_str()),
            matched.map(|_| self.session.generation()),
            self.session
                .snapshot
                .as_ref()
                .map(|r| r.authority.revision.as_str()),
            self.session.snapshot_age(now),
            self.session.fresh(now),
            self.session.brain.as_ref().map(|b| b.revision.as_str()),
            self.session.brain_age(now),
            self.session.brain_fresh(now),
            requested_revision,
            self.brain_probes.pending.len(),
        );
        if let Some(t) = timing {
            use std::fmt::Write;
            let _ = write!(
                failure,
                " timing_totals_us={:?} sends_us={} decision_us={} empty_receive_us={} frames={} overwritten={} overflow={} transport_before={transport_before:?} transport_after={:?}",
                t.totals,
                t.sends_us,
                t.decision_us,
                t.empty_receive_us,
                t.frames,
                t.overwritten,
                t.overflow,
                self.transport.timing_snapshot()
            );
            for (slot, frame) in t.tail.iter().enumerate() {
                let _ = write!(
                    failure,
                    " tail{slot}=(bytes={},kind={},start_us={},stages_us={:?},before={:?},after={:?})",
                    frame.bytes,
                    frame.kind,
                    frame.receive_start_us,
                    frame.micros,
                    frame.transport_before,
                    frame.transport_after
                );
            }
        }
        Err(failure.into())
    }
    fn pin_held_baseline(&mut self) -> Result<(), String> {
        if self.scope != "talkback_destinations"
            || !self.session.can_pin_held_baseline(self.now())
            || !self.session.brain_fresh(self.now())
        {
            return Ok(());
        }
        let Some(identity) = self.transport.held_identity() else {
            return Ok(());
        };
        let raw = self.session.snapshot.as_ref().ok_or("held baseline raw")?;
        let topology = raw.topology.as_ref().ok_or("held baseline topology")?;
        if identity.epoch != raw.authority.epoch
            || crate::provider::counter(&identity.map)? != topology.map_revision
        {
            return Err("held baseline attachment/map".into());
        }
        let dimensions = [
            raw.authority.inputs.len(),
            raw.authority.monitors.len(),
            topology.pa_outputs,
            topology.capture_channels,
            topology.playback_channels,
            topology.sample_rate as usize,
        ]
        .map(|n| u32::try_from(n).map_err(|_| "held baseline dimensions"));
        let dimensions = [
            dimensions[0]?,
            dimensions[1]?,
            dimensions[2]?,
            dimensions[3]?,
            dimensions[4]?,
            dimensions[5]?,
        ];
        let digest = crate::held_proof::configuration_digest(
            &raw.authority.show_id,
            &identity,
            dimensions,
            self.session.brain.as_ref().unwrap(),
        )?;
        let template = self.session.held_query(&identity, &digest, 1, self.now())?;
        self.held_baseline = Some(HeldBaseline {
            identity,
            template,
            dimensions,
        });
        Ok(())
    }
    pub(crate) fn held_transport_authenticated(&self) -> bool {
        self.transport.held_identity().is_some()
    }
    pub(crate) fn held_baseline_ready(&self) -> bool {
        self.held_baseline.as_ref().is_some_and(|b| {
            self.transport.held_identity().as_ref() == Some(&b.identity)
                && self
                    .session
                    .held_query(
                        &b.identity,
                        &b.template.expected_config_digest,
                        1,
                        self.now(),
                    )
                    .as_ref()
                    == Ok(&b.template)
        })
    }
    pub(crate) fn held_status(&self) -> Option<crate::held_proof::Status> {
        self.held_observation
            .as_ref()
            .filter(|s| s.fresh())
            .cloned()
    }
    fn compact_read(
        &mut self,
        deadline: Instant,
        remaining: &mut usize,
    ) -> Result<(), BrainOperationError> {
        self.held_guard()?;
        if self.brain_probes.invalid {
            return Err(self
                .brain_probes
                .first_fault
                .clone()
                .unwrap_or("held proof invalid".into())
                .into());
        }
        if !self.held_baseline_ready() {
            self.held_baseline = None;
            return Err(BrainOperationError::Admission("held proof unsupported or accepted baseline missing; full readback and new gesture required".into()));
        }
        self.held_matched = None;
        // Drain a canceled query before allocating a new nonce. Never resynchronize.
        while self.held_query.is_some() {
            self.compact_receive(deadline, remaining)?;
        }
        self.held_guard()?;
        if let Some(reason) = self.held_refusal.take() {
            return Err(BrainOperationError::Admission(reason));
        }
        if Instant::now() >= deadline || *remaining == 0 {
            return Err("held proof observation deadline/frame bound".into());
        }
        let baseline = self
            .held_baseline
            .as_ref()
            .ok_or("held baseline invalidated")?;
        self.held_nonce = self
            .held_nonce
            .checked_add(1)
            .ok_or("held query nonce exhausted")?;
        let request = self.session.held_query(
            &baseline.identity,
            &baseline.template.expected_config_digest,
            self.held_nonce,
            self.now(),
        )?;
        let sent = Instant::now();
        self.held_query = Some(HeldQuery {
            request: request.clone(),
            sent,
            generation: self.session.generation(),
            dimensions: baseline.dimensions,
        });
        self.transport
            .send_frame_until(&request.encode()?, deadline)?;
        while self.held_query.is_some() {
            self.compact_receive(deadline, remaining)?;
        }
        self.held_guard()?;
        if let Some(reason) = self.held_refusal.take() {
            return Err(BrainOperationError::Admission(reason));
        }
        if Instant::now() >= deadline
            || self
                .held_matched
                .as_ref()
                .is_none_or(|m| !m.fresh() || m.generation() != self.session.generation())
        {
            return Err("held proof observation deadline/context".into());
        }
        Ok(())
    }
    fn compact_receive(
        &mut self,
        deadline: Instant,
        remaining: &mut usize,
    ) -> Result<(), BrainOperationError> {
        self.held_guard()?;
        if Instant::now() >= deadline || *remaining == 0 {
            return Err("held proof observation deadline/frame bound".into());
        }
        if let Some(document) = self.transport.receive_document_until(deadline)? {
            *remaining -= 1;
            if let Some(raw) = self.processing_document(document)? {
                let reply = audio::decode_reply_document(raw)?;
                self.telemetry(&reply)?;
            }
            // Strict bytes/correlation take priority over simultaneous cancellation.
            self.held_guard()?;
        } else {
            self.held_guard()?;
            std::thread::sleep(
                deadline
                    .saturating_duration_since(Instant::now())
                    .min(Duration::from_millis(1)),
            );
        }
        self.held_guard()?;
        if Instant::now() >= deadline {
            return Err("held proof observation deadline".into());
        }
        Ok(())
    }
    fn proved_request(
        &mut self,
        kind: &str,
        generation: Option<u64>,
    ) -> Result<(audio::Request, u64, Instant), BrainOperationError> {
        self.held_guard()?;
        let proof = self.held_matched.take().ok_or("held own proof missing")?;
        if !self.held_baseline_ready()
            || self.held_baseline.as_ref().is_none_or(|b| {
                b.template.expected_config_digest != proof.request().expected_config_digest
            })
        {
            return Err(BrainOperationError::Admission(
                "held baseline changed".into(),
            ));
        }
        let deadline = (Instant::now() + Duration::from_millis(20))
            .min(proof.sent() + Duration::from_millis(50))
            .min(
                proof.sent() + Duration::from_millis(u64::from(proof.witness().lease_remaining_ms)),
            );
        let (request, generation) = self
            .session
            .begin_proved_held(kind, &proof, generation, self.now())
            .map_err(BrainOperationError::Admission)?;
        let deadline = deadline.min(
            self.start
                + Duration::from_millis(
                    self.session
                        .pending_authority_deadline()
                        .ok_or("held authority expired")?,
                ),
        );
        if Instant::now() >= deadline {
            return Err("held authority expired".into());
        }
        Ok((request, generation, deadline))
    }
    pub(crate) fn start_held(&mut self) -> Result<u64, String> {
        let result = (|| -> Result<u64, BrainOperationError> {
            self.compact_read(Instant::now() + Duration::from_millis(30), &mut 64)?;
            let (request, generation, deadline) = self.proved_request("brain_hold", None)?;
            let sent = Instant::now();
            self.transport
                .send_frame_until(&request.encode()?, deadline)?;
            self.last_brain_send = Some(sent);
            Ok(generation)
        })();
        match result {
            Ok(g) => Ok(g),
            Err(e) => Err(self.finish_brain_operation(Err(e)).unwrap_err()),
        }
    }
    pub(crate) fn held_send_anchor(&self) -> Option<Instant> {
        self.last_brain_send
    }
    fn held_guard(&mut self) -> Result<(), BrainOperationError> {
        self.brain_guard()?;
        if self
            .brain_signal
            .as_ref()
            .is_some_and(|signal| !signal.live())
        {
            return Err(BrainOperationError::Admission("held input released".into()));
        }
        Ok(())
    }
    // Only correlated completion is accepted. Empty first-byte polls are wakes;
    // partial frame errors remain fatal and are never retried here.
    fn settle_held_pending(
        &mut self,
        deadline: Instant,
        remaining: &mut usize,
    ) -> Result<(), BrainOperationError> {
        while self.session.pending.is_some() {
            self.held_guard()?;
            if Instant::now() >= deadline || *remaining == 0 {
                return Err("held completion budget exhausted".into());
            }
            let receive_deadline = deadline.min(
                self.start
                    + Duration::from_millis(
                        self.session
                            .pending_authority_deadline()
                            .ok_or("held authority expired")?,
                    ),
            );
            if Instant::now() >= receive_deadline {
                return Err("held authority expired".into());
            }
            if let Some(document) = self.transport.receive_document_until(receive_deadline)? {
                *remaining -= 1;
                let expected_brain = self
                    .session
                    .pending
                    .as_ref()
                    .filter(|p| matches!(p.request.kind.as_str(), "brain_hold" | "brain_heartbeat"))
                    .map(|p| p.request.context.clone());
                if let Some(document) = self.processing_document(document)? {
                    let reply = audio::decode_reply_document(document)?;
                    if reply.context == self.session.snapshot_request().context {
                        self.telemetry(&reply)?;
                    } else {
                        let refused = reply.state == "final"
                            && reply.outcome.as_ref().is_some_and(|o| o.kind != "applied");
                        self.session.accept(reply, self.now())?;
                        if refused {
                            return Err("held renewal refused".into());
                        }
                    }
                }
                if self.session.pending.is_none()
                    && let Some(expected) = expected_brain
                    && let Some(final_reply) = self.session.brain_final.as_ref()
                    && final_reply.context == expected
                    && let Some(reason) = &final_reply.reason
                {
                    return Err(BrainOperationError::Admission(format!(
                        "held Brain refused: {reason}"
                    )));
                }
                self.held_guard()?;
            } else {
                std::thread::sleep(
                    deadline
                        .saturating_duration_since(Instant::now())
                        .min(Duration::from_millis(1)),
                );
            }
        }
        if Instant::now() >= deadline {
            return Err("held completion deadline".into());
        }
        Ok(())
    }
    /// The real worker's entire held maintenance path. No passive250ms poll or
    /// ordinary1900ms mutation wait is reachable while this service owns the hold.
    pub(crate) fn service_held(&mut self, generation: u64) -> Result<Instant, String> {
        let began = Instant::now();
        let initial_anchor = self.last_brain_send;
        let result = (|| -> Result<Instant, BrainOperationError> {
            let deadline =
                self.last_brain_send.ok_or("held send anchor missing")? + Duration::from_millis(50);
            if Instant::now() >= deadline {
                return Err("held service arrived late".into());
            }
            self.held_guard()?;
            let mut remaining = 64;
            self.settle_held_pending(deadline, &mut remaining)?;
            self.compact_read(
                deadline.min(Instant::now() + Duration::from_millis(30)),
                &mut remaining,
            )?;
            self.held_guard()?;
            let (request, _, probe_deadline) =
                self.proved_request("brain_heartbeat", Some(generation))?;
            let sent = Instant::now();
            self.transport
                .send_frame_until(&request.encode()?, deadline.min(probe_deadline))?;
            self.last_brain_send = Some(sent);
            // Completion and due renewal use the NEXT period, anchored to actual
            // send start, not to work completion or a fresh per-read timeout.
            let next = sent + Duration::from_millis(50);
            self.settle_held_pending(next, &mut remaining)?;
            self.compact_read(
                next.min(Instant::now() + Duration::from_millis(30)),
                &mut remaining,
            )?;
            if self.session.renewal_due(self.now()) {
                let (request, _, proof_deadline) =
                    self.proved_request("renew", Some(generation))?;
                self.held_guard()?;
                let renewal_deadline = next.min(proof_deadline).min(
                    self.start
                        + Duration::from_millis(
                            self.session
                                .pending_authority_deadline()
                                .ok_or("held renewal authority missing")?,
                        ),
                );
                if Instant::now() >= renewal_deadline {
                    return Err("held renewal authority expired".into());
                }
                self.transport
                    .send_frame_until(&request.encode()?, renewal_deadline)?;
                self.settle_held_pending(next, &mut remaining)?;
            }
            Ok(sent)
        })();
        match result {
            Ok(sent) => Ok(sent),
            Err(error) => {
                let timing = format!(
                    "held_elapsed_ms={} initial_send_age_ms={:?} last_send_age_ms={:?}",
                    began.elapsed().as_millis(),
                    initial_anchor.map(|t| t.elapsed().as_millis()),
                    self.last_brain_send.map(|t| t.elapsed().as_millis())
                );
                let error = match error {
                    BrainOperationError::Admission(reason) => {
                        BrainOperationError::Admission(format!("{reason}; {timing}"))
                    }
                    BrainOperationError::Fault(reason) => {
                        BrainOperationError::Fault(format!("{reason}; {timing}"))
                    }
                };
                let result = self.finish_brain_operation(Err(error));
                if let Some(signal) = &self.brain_signal {
                    signal.release();
                }
                let _ = self.close_brain();
                result.map(|()| Instant::now())
            }
        }
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
        held_baseline: None,
        held_query: None,
        held_matched: None,
        held_nonce: 0,
        held_highwater: 0,
        held_observation: None,
        held_refusal: None,
        last_brain_send: None,
        trace_timing: trace_timing_enabled(),
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
            held_baseline: None,
            held_query: None,
            held_matched: None,
            held_nonce: 0,
            held_highwater: 0,
            held_observation: None,
            held_refusal: None,
            last_brain_send: None,
            trace_timing: trace_timing_enabled(),
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
        delayed: Option<Vec<u8>>,
        enqueue_after_wait: Vec<Vec<u8>>,
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
            // Wire preparation belongs to the fake peer, before the consumer
            // starts its deadline; arrival still occurs at this delayed read.
            self.queued
                .0
                .lock()
                .unwrap()
                .extend(self.enqueue_after_wait.drain(..));
            if let Some(bytes) = self.delayed.take() {
                Ok(Some(bytes))
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
            delayed: delayed.map(|v| serde_json::to_vec(&v).unwrap()),
            enqueue_after_wait: tail
                .into_iter()
                .map(|v| serde_json::to_vec(&v).unwrap())
                .collect(),
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
        let error = op.refresh().unwrap_err();
        assert!(error.contains("backlog saturated"), "{error}");
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
    // Scheduling tests isolate timing from full-capacity debug JSON decoding.
    // Dynamic16/32/48 contracts and integrated profiles remain separate gates.
    fn held_setup() -> (Operator, FakeAuthorityConnection, Value) {
        let (mut op, peer, mut raw) = setup();
        let snapshot = &mut raw["snapshot"];
        snapshot["authority"]["inputs"]
            .as_array_mut()
            .unwrap()
            .truncate(1);
        let input = snapshot["authority"]["inputs"][0].clone();
        snapshot["authority"]["parameters"]
            .as_array_mut()
            .unwrap()
            .retain(|p| p["target"]["input"] == input);
        snapshot["coefficients"].as_array_mut().unwrap().truncate(1);
        snapshot["topology"]["inputs"]
            .as_array_mut()
            .unwrap()
            .truncate(1);
        snapshot["topology"]["capture_channels"] = json!(1);
        let parsed =
            audio::decode_snapshot(&serde_json::to_vec(&raw["snapshot"]).unwrap()).unwrap();
        op.session.snapshot = Some(parsed);
        (op, peer, raw)
    }
    /// Run only on request: cargo test --release --locked -j1 held_decode_stage_benchmark -- --ignored --nocapture
    #[test]
    #[ignore = "finite optimized offline decode-stage benchmark; no network/PCM; descriptive timings only"]
    fn held_decode_stage_benchmark() {
        use sha2::{Digest, Sha256};
        use std::hint::black_box;
        for inputs in [16usize, 32, 48] {
            // Derive equal-five-bus shapes from the producer48x9 corpus. All
            // transformations and frame preparation precede measurement, and the
            // actual strict decoder validates the derived complete document.
            let mut snapshot: Value = serde_json::from_slice(include_bytes!(
                "../tests/fixtures/gp15/v1/raw-snapshot-48-9.json"
            ))
            .unwrap();
            snapshot["authority"]["inputs"]
                .as_array_mut()
                .unwrap()
                .truncate(inputs);
            snapshot["authority"]["monitors"]
                .as_array_mut()
                .unwrap()
                .truncate(5);
            snapshot["authority"]["modes"]
                .as_array_mut()
                .unwrap()
                .truncate(6);
            let selected_inputs = snapshot["authority"]["inputs"].as_array().unwrap().clone();
            let selected_monitors = snapshot["authority"]["monitors"]
                .as_array()
                .unwrap()
                .clone();
            snapshot["authority"]["parameters"]
                .as_array_mut()
                .unwrap()
                .retain(|p| {
                    selected_inputs.contains(&p["target"]["input"])
                        && (p["target"]["parameter"] != "send"
                            || selected_monitors.contains(&p["target"]["monitor"]))
                });
            snapshot["coefficients"]
                .as_array_mut()
                .unwrap()
                .truncate(inputs);
            for coefficient in snapshot["coefficients"].as_array_mut().unwrap() {
                for lane in ["current_nanogain", "ramp_target_nanogain", "held_nanogain"] {
                    coefficient[lane].as_array_mut().unwrap().truncate(9);
                }
            }
            snapshot["topology"]["inputs"]
                .as_array_mut()
                .unwrap()
                .truncate(inputs);
            snapshot["topology"]["capture_channels"] = json!(inputs);
            snapshot["topology"]["monitors"] = json!(5);
            snapshot["topology"]["outputs"]
                .as_array_mut()
                .unwrap()
                .truncate(7);
            snapshot["topology"]["playback_channels"] = json!(7);
            let parsed = audio::decode_snapshot(&serde_json::to_vec(&snapshot).unwrap()).unwrap();
            assert_eq!(parsed.authority.inputs.len(), inputs);
            assert_eq!(parsed.authority.monitors.len(), 5);
            let (mut op, _, mut raw) = setup();
            raw["snapshot"] = snapshot;
            raw["outcome"]["body"]["revision"] = json!(parsed.authority.revision);
            op.session.snapshot = Some(parsed);
            let payload = serde_json::to_vec(&raw).unwrap();
            audio::decode_reply(&payload).unwrap();
            let envelope =
                serde_json::to_vec(&json!({"kind":"reply", "session":"1", "payload":raw})).unwrap();
            let frames: Vec<Vec<u8>> = if envelope.len() <= crate::provider::MAX_BYTES {
                vec![envelope.clone()]
            } else {
                let identity = format!("{:x}", Sha256::digest(&envelope));
                let count = envelope.len().div_ceil(8192);
                envelope.chunks(8192).enumerate().map(|(index, chunk)| serde_json::to_vec(&json!({"contract":"GP14-snapshot-pages", "version":1,"identity":identity,"index":index,"count":count,"total_bytes":envelope.len(),"payload":std::str::from_utf8(chunk).unwrap()})).unwrap()).collect()
            };
            // Establish full semantic equivalence once, outside measured iterations.
            // Walking the parsed tree immediately before decode would warm its cache.
            {
                let (document, _) = crate::remote::benchmark_snapshot_decode_stages(frames.clone());
                assert_eq!(document.admitted_bytes(), payload.len());
                assert_eq!(document.value(), &raw);
            }
            let mut measurements: [Vec<u128>; 7] = std::array::from_fn(|_| Vec::with_capacity(20));
            for _ in 0..20 {
                let owned_frames = frames.clone(); // transport already owns its received buffers
                let (document, remote) =
                    crate::remote::benchmark_snapshot_decode_stages(black_box(owned_frames));
                assert_eq!(document.admitted_bytes(), payload.len());
                let started = Instant::now();
                let document = op
                    .processing_document(black_box(document))
                    .unwrap()
                    .expect("raw document");
                let discriminator = started.elapsed().as_nanos();
                let started = Instant::now();
                let reply = audio::decode_reply_document(black_box(document)).unwrap();
                let decode = started.elapsed().as_nanos();
                let started = Instant::now();
                black_box(op.telemetry(black_box(&reply)).unwrap());
                let ingest = started.elapsed().as_nanos();
                let values = [
                    remote[0],
                    remote[1],
                    remote[2],
                    discriminator,
                    decode,
                    ingest,
                    remote.iter().sum::<u128>() + discriminator + decode + ingest,
                ];
                for (samples, value) in measurements.iter_mut().zip(values) {
                    samples.push(value);
                }
            }
            let mut stages = serde_json::Map::new();
            for (name, mut values) in [
                "page_assembly",
                "envelope_decode",
                "payload_serialize",
                "contract_discriminator",
                "raw_reply_decode",
                "telemetry_clone_ingest",
                "sum_measured_stages",
            ]
            .into_iter()
            .zip(measurements)
            {
                values.sort_unstable();
                stages.insert(name.into(), json!({"min_us":values[0] as f64/1000.0,"median_us":values[10] as f64/1000.0,"p95_us":values[18] as f64/1000.0,"max_us":values[19] as f64/1000.0}));
            }
            eprintln!(
                "HELD_DECODE_BENCH {}",
                json!({"inputs":inputs,"monitors":5,"iterations":20,"debug_assertions":cfg!(debug_assertions),"payload_bytes":payload.len(),"envelope_bytes":envelope.len(),"frames":frames.len(),"stages":stages,"scope":"derived producer48x9; strict validated16/32/48x5; post-I/O elapsed Instant stages including scheduling; not thread CPU; strict-document production path; payload serialization eliminated; no network/PCM; no timing pass criterion"})
            );
        }
    }
    struct PairedReadGate {
        peer: FakeAuthorityConnection,
        sent: Arc<Mutex<Vec<Value>>>,
        expected_first_queries: usize,
        first: bool,
        deadline: Instant,
    }
    impl AuthorityConnection for PairedReadGate {
        fn send_frame_until(&mut self, bytes: &[u8], deadline: Instant) -> Result<(), String> {
            assert_eq!(deadline, self.deadline, "no nested budget reset");
            self.sent
                .lock()
                .unwrap()
                .push(serde_json::from_slice(bytes).unwrap());
            Ok(())
        }
        fn receive_until(&mut self, deadline: Instant) -> Result<Option<Vec<u8>>, String> {
            assert_eq!(deadline, self.deadline);
            if self.first {
                self.first = false;
                let sent = self.sent.lock().unwrap();
                assert_eq!(
                    sent.len(),
                    self.expected_first_queries,
                    "required queries must precede first response wait"
                );
                assert_eq!(sent[0]["kind"], "brain_snapshot");
                if self.expected_first_queries == 2 {
                    assert_eq!(sent[1]["kind"], "snapshot");
                }
            }
            self.peer.receive_available()
        }
        fn receive_available(&mut self) -> Result<Option<Vec<u8>>, String> {
            self.peer.receive_available()
        }
    }
    #[test]
    fn held_stale_pair_is_pipelined_but_passive_and_fresh_paths_are_not() {
        for (held, stale, expected_first) in [(true, true, 2), (false, true, 1), (true, false, 1)] {
            let (mut op, peer, raw) = held_setup();
            if stale {
                op.start = Instant::now() - Duration::from_millis(251);
            }
            peer.push(probe_reply(&op, 48));
            if stale {
                peer.push(raw_at(&raw, "0", "48"));
            }
            let sent = Arc::new(Mutex::new(Vec::new()));
            let deadline = Instant::now() + Duration::from_millis(30);
            op.transport = Box::new(PairedReadGate {
                peer,
                sent: sent.clone(),
                expected_first_queries: expected_first,
                first: true,
                deadline,
            });
            let mut remaining = 64;
            op.refresh_brain_budget(deadline, &mut remaining, held)
                .map_err(BrainOperationError::message)
                .unwrap();
            assert_eq!(remaining, if stale { 62 } else { 63 });
            assert!(op.session.brain_fresh(op.now()));
            let sent = sent.lock().unwrap();
            assert_eq!(
                sent.len(),
                if stale { 2 } else { 1 },
                "no duplicate raw request for the anticipated revision"
            );
        }
    }
    #[test]
    fn held_eager_pair_reprobes_superseded_revision_without_duplicate_raw() {
        let (mut op, peer, raw) = held_setup();
        op.start = Instant::now() - Duration::from_millis(251);
        peer.push(probe_reply(&op, 48));
        peer.push(raw_at(&raw, "1", "96"));
        let mut current = probe_reply(&op, 96);
        current["revision"] = json!("1");
        current["snapshot"]["revision"] = json!("1");
        peer.push(current);
        let sent = Arc::new(Mutex::new(Vec::new()));
        let deadline = Instant::now() + Duration::from_millis(30);
        op.transport = Box::new(PairedReadGate {
            peer,
            sent: sent.clone(),
            expected_first_queries: 2,
            first: true,
            deadline,
        });
        let mut remaining = 64;
        op.refresh_brain_budget(deadline, &mut remaining, true)
            .map_err(BrainOperationError::message)
            .unwrap();
        assert_eq!(remaining, 61);
        assert_eq!(op.brain_probes.matched.as_ref().unwrap().0, 2);
        assert_eq!(op.session.brain.as_ref().unwrap().revision, "1");
        assert_eq!(
            op.session.snapshot.as_ref().unwrap().authority.revision,
            "1"
        );
        let sent = sent.lock().unwrap();
        assert_eq!(
            sent.iter()
                .map(|r| r["kind"].as_str().unwrap())
                .collect::<Vec<_>>(),
            ["brain_snapshot", "snapshot", "brain_snapshot"]
        );
        assert!(sent.iter().all(|r| r["writer"].is_null()));
    }
    struct HeldPeer {
        identity: crate::held_proof::Identity,
        raw: Value,
        brain: Value,
        replies: VecDeque<Vec<u8>>,
        sent: Arc<Mutex<Vec<(Value, Instant, Instant)>>>,
        fail: Option<&'static str>,
        cancel: Option<(Arc<crate::brain::HoldSignal>, &'static str)>,
        probes: usize,
        cancel_stage: u8,
        cached: std::collections::BTreeMap<String, Vec<u8>>,
    }
    impl AuthorityConnection for HeldPeer {
        fn held_identity(&self) -> Option<crate::held_proof::Identity> {
            Some(self.identity.clone())
        }
        fn send_frame_until(&mut self, bytes: &[u8], deadline: Instant) -> Result<(), String> {
            let request: Value = serde_json::from_slice(bytes).unwrap();
            self.sent
                .lock()
                .unwrap()
                .push((request.clone(), Instant::now(), deadline));
            if let Some(id) = request["request_id"].as_str()
                && let Some(reply) = self.cached.get(id)
            {
                self.replies.push_back(reply.clone());
                return Ok(());
            }
            let mut context = request.clone();
            context.as_object_mut().unwrap().retain(|k, _| {
                [
                    "show_id",
                    "module",
                    "epoch",
                    "writer",
                    "lease",
                    "request_id",
                    "expected_revision",
                ]
                .contains(&k.as_str())
            });
            let revision = self.brain["snapshot"]["revision"]
                .as_str()
                .unwrap()
                .parse::<u64>()
                .unwrap();
            let frame = self.brain["snapshot"]["frame"]
                .as_str()
                .unwrap()
                .parse::<u64>()
                .unwrap()
                + 48;
            let revision_text = revision.to_string();
            let reply = match request["kind"].as_str().unwrap() {
                "held_proof" => {
                    self.probes += 1;
                    if self.probes == 2 && self.cancel.is_some() {
                        self.cancel_stage = 2;
                    }
                    self.brain["snapshot"]["frame"] = json!(frame.to_string());
                    let b = &self.brain["snapshot"];
                    let mut projection = serde_json::Map::new();
                    for field in [
                        "selection_generation",
                        "hold_generation_counter",
                        "held_generation",
                        "hold_deadline_ms",
                        "source",
                        "monitor_armed",
                        "monitor_mute",
                        "monitor_dim",
                        "talkback_mute",
                        "talkback_foh",
                        "monitor_path_ready",
                        "talkback_path_ready",
                    ] {
                        projection.insert(field.into(), b[field].clone());
                    }
                    json!({"contract":"GP15-held-proof","version":1,"state":"proof","reason":null,"context":request,"witness":{
                        "revision":revision.to_string(),"source_frame":frame.to_string(),"config_digest":request["expected_config_digest"],"lease_remaining_ms":2000,
                        "brain":projection,"foh_authorized":true,"media_authorized":!b["held_generation"].is_null() && b["talkback_mute"] == false,
                        "heartbeat_ms":50,"deadman_ms":150,"fade_frames":240
                    }})
                }
                "brain_snapshot" => {
                    self.probes += 1;
                    if self.probes == 2 && self.cancel.is_some() {
                        self.cancel_stage = 1;
                    }
                    self.brain["snapshot"]["frame"] = json!(frame.to_string());
                    self.brain.clone()
                }
                "snapshot" => raw_at(&self.raw, &revision.to_string(), &frame.to_string()),
                "hold" | "heartbeat" | "release"
                    if request["expected_revision"] != revision_text =>
                {
                    json!({"contract":"GP15-brain","version":1,"state":"final","reason":"stale_revision","context":context,"revision":revision.to_string(),"applied_frame":null,"snapshot":null})
                }
                "hold" | "heartbeat" | "release" => {
                    let held = request["kind"] != "release";
                    self.brain["revision"] = json!((revision + 1).to_string());
                    self.brain["snapshot"]["revision"] = json!((revision + 1).to_string());
                    self.brain["snapshot"]["frame"] = json!(frame.to_string());
                    self.brain["snapshot"]["held_generation"] =
                        if held { json!("1") } else { Value::Null };
                    if held {
                        self.brain["snapshot"]["hold_generation_counter"] = json!("1");
                    }
                    self.brain["snapshot"]["hold_deadline_ms"] =
                        if held { json!("99999") } else { Value::Null };
                    json!({"contract":"GP15-brain","version":1,"state":"final","reason":null,"context":context,"revision":(revision+1).to_string(),"applied_frame":frame.to_string(),"snapshot":self.brain["snapshot"]})
                }
                "renew" => {
                    let mut reply: Value = serde_json::from_slice(include_bytes!(
                        "../tests/fixtures/gp15/v1/grant-16-talkback-reply.json"
                    ))
                    .unwrap();
                    reply["context"] = context.clone();
                    for (k, v) in context.as_object().unwrap() {
                        reply["outcome"][k] = v.clone();
                    }
                    reply["snapshot"] = Value::Null;
                    reply["outcome"]["body"]["granted_lease"] = Value::Null;
                    reply["outcome"]["body"]["revision"] = json!(revision.to_string());
                    reply
                }
                other => panic!("passive/ordinary maintenance entered held service: {other}"),
            };
            let bytes = serde_json::to_vec(&reply).unwrap();
            if let Some(id) = request["request_id"].as_str() {
                self.cached.insert(id.into(), bytes.clone());
            }
            self.replies.push_back(bytes);
            Ok(())
        }
        fn receive_until(&mut self, deadline: Instant) -> Result<Option<Vec<u8>>, String> {
            if self.cancel_stage == 2 {
                self.cancel_stage = 3;
                let (signal, mode) = self.cancel.as_ref().unwrap();
                signal.release();
                match *mode {
                    "none" => {
                        std::thread::sleep(deadline.saturating_duration_since(Instant::now()));
                        return Ok(None);
                    }
                    "partial" => return Err("partial post-heartbeat probe frame".into()),
                    "malformed" => return Ok(Some(b"{malformed".to_vec())),
                    "late_malformed" => {
                        std::thread::sleep(deadline.saturating_duration_since(Instant::now()));
                        return Ok(Some(b"{malformed".to_vec()));
                    }
                    "valid" => {}
                    _ => unreachable!(),
                }
            } else if self.cancel_stage == 1 {
                self.cancel_stage = 2;
            }
            if self.fail == Some("slow_ack") {
                if self.replies.front().is_some_and(|b| {
                    let reply: Value = serde_json::from_slice(b).unwrap();
                    reply["contract"] == "GP15-brain" && reply["state"] == "final"
                }) {
                    std::thread::sleep(Duration::from_millis(10));
                }
                return Ok(self.replies.pop_front());
            }
            if let Some(fail) = self.fail {
                if fail == "partial" {
                    return Err("partial held frame".into());
                }
                std::thread::sleep(deadline.saturating_duration_since(Instant::now()));
                return Ok(None);
            }
            Ok(self.replies.pop_front())
        }
        fn receive_available(&mut self) -> Result<Option<Vec<u8>>, String> {
            panic!("legacy passive drain entered held service")
        }
    }
    fn held_peer(
        op: &mut Operator,
        raw: Value,
        fail: Option<&'static str>,
    ) -> Arc<Mutex<Vec<(Value, Instant, Instant)>>> {
        held_peer_cancellation(op, raw, fail, None)
    }
    fn held_peer_cancellation(
        op: &mut Operator,
        raw: Value,
        fail: Option<&'static str>,
        cancel_mode: Option<&'static str>,
    ) -> Arc<Mutex<Vec<(Value, Instant, Instant)>>> {
        let signal = Arc::new(crate::brain::HoldSignal::default());
        signal.press();
        op.brain_signal(signal.clone());
        let identity = crate::held_proof::Identity {
            session: "1".into(),
            epoch: op
                .session
                .snapshot
                .as_ref()
                .unwrap()
                .authority
                .epoch
                .clone(),
            capability: "1".into(),
            map: op
                .session
                .snapshot
                .as_ref()
                .unwrap()
                .topology
                .as_ref()
                .unwrap()
                .map_revision
                .to_string(),
        };
        let dimensions = {
            let r = op.session.snapshot.as_ref().unwrap();
            [
                r.authority.inputs.len() as u32,
                r.authority.monitors.len() as u32,
                r.topology.as_ref().unwrap().pa_outputs as u32,
                r.topology.as_ref().unwrap().capture_channels as u32,
                r.topology.as_ref().unwrap().playback_channels as u32,
                48000,
            ]
        };
        let digest = crate::held_proof::configuration_digest(
            &op.session.snapshot.as_ref().unwrap().authority.show_id,
            &identity,
            dimensions,
            op.session.brain.as_ref().unwrap(),
        )
        .unwrap();
        let template = op
            .session
            .held_query(&identity, &digest, 1, op.now())
            .unwrap();
        op.held_baseline = Some(HeldBaseline {
            identity: identity.clone(),
            template,
            dimensions,
        });
        let b = op.session.brain.as_mut().unwrap();
        b.held_generation = if fail == Some("initial") {
            None
        } else {
            Some("1".into())
        };
        b.hold_generation_counter = if fail == Some("initial") {
            "0".into()
        } else {
            "1".into()
        };
        b.hold_deadline_ms = b.held_generation.as_ref().map(|_| "99999".into());
        let brain = probe_reply(op, b_frame(op) + 48);
        let sent = Arc::new(Mutex::new(Vec::new()));
        op.transport = Box::new(HeldPeer {
            identity,
            raw,
            brain,
            replies: VecDeque::new(),
            sent: sent.clone(),
            fail: fail.filter(|v| *v != "initial"),
            cancel: cancel_mode.map(|mode| (signal, mode)),
            probes: 0,
            cancel_stage: 0,
            cached: std::collections::BTreeMap::new(),
        });
        op.last_brain_send = Some(Instant::now());
        sent
    }
    fn b_frame(op: &Operator) -> u64 {
        op.session.brain.as_ref().unwrap().frame.parse().unwrap()
    }
    #[test]
    fn compact_heartbeat_uses_own_frame_and_original_probe_send_age() {
        let (mut op, _, raw) = held_setup();
        let sent = held_peer(&mut op, raw, None);
        op.compact_read(Instant::now() + Duration::from_millis(30), &mut 64)
            .map_err(BrainOperationError::message)
            .unwrap();
        let proof = op.held_matched.take().unwrap();
        let frame = proof.witness().source_frame.clone();
        let reply = crate::held_proof::Reply {
            contract: crate::held_proof::CONTRACT.into(),
            version: 1,
            state: "proof".into(),
            reason: None,
            context: proof.request().clone(),
            witness: Some(proof.witness().clone()),
        };
        op.held_matched = Some(
            crate::held_proof::Matched::admit(
                reply.clone(),
                proof.request(),
                Instant::now() - Duration::from_millis(31),
                op.session.generation(),
            )
            .unwrap(),
        );
        assert!(op.proved_request("brain_heartbeat", Some(1)).is_err());
        assert!(op.session.pending.is_none());
        op.held_matched = Some(
            crate::held_proof::Matched::admit(
                reply,
                proof.request(),
                Instant::now(),
                op.session.generation(),
            )
            .unwrap(),
        );
        let (request, _, deadline) = op
            .proved_request("brain_heartbeat", Some(1))
            .map_err(BrainOperationError::message)
            .unwrap();
        assert_eq!(request.body["observed_frame"], frame);
        assert!(deadline <= Instant::now() + Duration::from_millis(20));
        assert_eq!(
            sent.lock().unwrap().len(),
            1,
            "admission alone does not retransmit"
        );
    }
    #[test]
    fn initial_press_uses_compact_authority_without_full_snapshot_io() {
        let (mut op, _, raw) = held_setup();
        let sent = held_peer(&mut op, raw, Some("initial"));
        assert_eq!(op.start_held().unwrap(), 1);
        let sent = sent.lock().unwrap();
        assert_eq!(
            sent.iter()
                .map(|(r, _, _)| r["kind"].as_str().unwrap())
                .collect::<Vec<_>>(),
            ["held_proof", "hold"]
        );
        assert_eq!(
            op.session
                .pending
                .as_ref()
                .unwrap()
                .request
                .context
                .expected_revision
                .as_deref(),
            Some("0")
        );
    }
    #[test]
    fn compact_held_service_has_no_full_snapshot_query_and_keeps_raw_stale() {
        let (mut op, _, raw) = held_setup();
        let sent = held_peer(&mut op, raw, None);
        let raw_frame = op.session.snapshot.as_ref().unwrap().frame.clone();
        op.service_held(1).unwrap();
        assert_eq!(op.session.snapshot.as_ref().unwrap().frame, raw_frame);
        assert!(
            op.held_status()
                .is_some_and(|s| s.generation.as_deref() == Some("1") && s.media_authorized)
        );
        assert!(
            !op.session.brain_fresh(op.now()),
            "compact witness does not refresh the full raw pair"
        );
        let sent = sent.lock().unwrap();
        assert_eq!(
            sent.iter()
                .filter(|(r, _, _)| r["kind"] == "held_proof")
                .count(),
            2
        );
        assert!(
            sent.iter()
                .all(|(r, _, _)| r["kind"] != "snapshot" && r["kind"] != "brain_snapshot")
        );
    }
    #[test]
    fn compact_generation_highwater_survives_cancel_and_rejects_regression() {
        let (mut op, _, raw) = held_setup();
        held_peer(&mut op, raw, None);
        op.compact_read(Instant::now() + Duration::from_millis(30), &mut 64)
            .map_err(BrainOperationError::message)
            .unwrap();
        assert_eq!(op.held_highwater, 1);
        let proof = op.held_matched.take().unwrap();
        let mut reply = crate::held_proof::Reply {
            contract: crate::held_proof::CONTRACT.into(),
            version: 1,
            state: "proof".into(),
            reason: None,
            context: proof.request().clone(),
            witness: Some(proof.witness().clone()),
        };
        op.cancel();
        assert_eq!(op.held_highwater, 1);
        // Even if no full Brain highwater is available, the previous own proof
        // remains a rejection floor throughout this healthy attachment.
        op.session.brain = None;
        reply.context.query_id = "2".into();
        let w = reply.witness.as_mut().unwrap();
        w.brain.hold_generation_counter = "0".into();
        w.brain.held_generation = None;
        w.brain.hold_deadline_ms = None;
        w.brain.talkback_path_ready = false;
        w.media_authorized = false;
        op.held_query = Some(HeldQuery {
            request: reply.context.clone(),
            sent: Instant::now(),
            generation: op.session.generation(),
            dimensions: op.held_baseline.as_ref().unwrap().dimensions,
        });
        assert!(
            op.processing_frame(&serde_json::to_vec(&reply).unwrap())
                .unwrap_err()
                .contains("highwater regressed")
        );
        assert_eq!(op.held_highwater, 1);
        assert!(op.held_matched.is_none());
    }
    #[test]
    fn compact_canceled_query_is_drained_before_new_nonce_without_new_authority() {
        let (mut op, _, raw) = held_setup();
        let sent = held_peer(&mut op, raw, None);
        let b = op.held_baseline.as_ref().unwrap();
        let request = op
            .session
            .held_query(&b.identity, &b.template.expected_config_digest, 1, op.now())
            .unwrap();
        op.held_nonce = 1;
        op.held_query = Some(HeldQuery {
            request: request.clone(),
            sent: Instant::now(),
            generation: op.session.generation(),
            dimensions: b.dimensions,
        });
        op.transport
            .send_frame_until(
                &request.encode().unwrap(),
                Instant::now() + Duration::from_millis(30),
            )
            .unwrap();
        op.cancel();
        op.session.input_released();
        op.brain_signal.as_ref().unwrap().press();
        op.compact_read(Instant::now() + Duration::from_millis(30), &mut 64)
            .map_err(BrainOperationError::message)
            .unwrap();
        assert_eq!(op.held_matched.as_ref().unwrap().request().query_id, "2");
        assert_eq!(
            op.held_matched.as_ref().unwrap().generation(),
            op.session.generation()
        );
        assert!(
            sent.lock()
                .unwrap()
                .iter()
                .all(|(r, _, _)| r["kind"] == "held_proof")
        );
    }
    #[test]
    fn compact_ptt_on_transport_without_authenticated_identity_is_closed() {
        let (mut op, _, _) = held_setup();
        assert!(!op.held_baseline_ready());
        assert!(op.start_held().unwrap_err().contains("unsupported"));
        assert!(op.session.pending.is_none());
    }
    #[test]
    fn held_worker_service_renews_beyond_two_seconds_and_key_up_closes() {
        let (mut op, _, raw) = held_setup();
        let old_expiry = op.session.lease_deadline().unwrap();
        let sent = held_peer(&mut op, raw, None);
        let began = Instant::now();
        let signal = op.brain_signal.as_ref().unwrap().clone();
        let mut held = Some((1, 1));
        let mut heartbeat = op.held_send_anchor().unwrap();
        let mut status = String::new();
        while began.elapsed() < Duration::from_millis(2100) {
            std::thread::sleep(
                (op.held_send_anchor().unwrap() + Duration::from_millis(20))
                    .saturating_duration_since(Instant::now()),
            );
            op.brain_signal.as_ref().unwrap().pulse();
            let prior = op.held_send_anchor().unwrap();
            crate::frontend::service_worker_hold(
                &mut op,
                &signal,
                &mut held,
                &mut heartbeat,
                &mut status,
            );
            assert_eq!(held, Some((1, 1)), "{status}");
            assert!(heartbeat.duration_since(prior) <= Duration::from_millis(50));
            assert!(op.session.pending.is_none());
        }
        assert!(op.session.lease_deadline().unwrap() > old_expiry);
        signal.release();
        crate::frontend::service_worker_hold(
            &mut op,
            &signal,
            &mut held,
            &mut heartbeat,
            &mut status,
        );
        assert!(held.is_none());
        let sent = sent.lock().unwrap();
        assert!(sent.iter().any(|(r, _, _)| r["kind"] == "renew"));
        assert!(sent.iter().any(|(r, _, _)| r["kind"] == "release"));
        assert!(sent.iter().all(|(_, at, deadline)| *deadline >= *at
            && deadline.duration_since(*at) <= Duration::from_millis(50)));
        let renews: Vec<_> = sent
            .iter()
            .filter(|(r, _, _)| r["kind"] == "renew")
            .collect();
        assert!(
            renews
                .windows(2)
                .all(|w| w[0].0["request_id"] != w[1].0["request_id"])
        );
    }
    #[test]
    fn held_service_anchor_precedes_delayed_completion_and_late_service_closes() {
        let (mut op, _, raw) = held_setup();
        let sent = held_peer(&mut op, raw, Some("slow_ack"));
        let anchor = op.service_held(1).unwrap();
        assert!(anchor.elapsed() >= Duration::from_millis(10));
        let writes = sent.lock().unwrap();
        let (_, write_at, _) = writes
            .iter()
            .find(|(r, _, _)| r["kind"] == "heartbeat")
            .unwrap();
        assert!(anchor <= *write_at);
        drop(writes);
        op.last_brain_send = Some(Instant::now() - Duration::from_millis(51));
        assert!(op.service_held(1).unwrap_err().contains("arrived late"));
        assert!(!op.brain_signal.as_ref().unwrap().live());
    }
    #[test]
    fn held_worker_service_silence_and_partial_frame_close_without_heartbeat_retry() {
        for failure in ["silent", "partial"] {
            let (mut op, _, raw) = held_setup();
            let sent = held_peer(&mut op, raw, Some(failure));
            let began = Instant::now();
            assert!(op.service_held(1).is_err());
            assert!(began.elapsed() < Duration::from_millis(150));
            assert!(!op.brain_signal.as_ref().unwrap().live());
            let sent = sent.lock().unwrap();
            assert!(sent.iter().any(|(r, _, _)| r["kind"] == "release"));
            assert!(!sent.iter().any(|(r, _, _)| r["kind"] == "heartbeat"));
        }
    }
    #[test]
    fn key_up_inside_post_heartbeat_pair_preserves_read_only_recovery() {
        for mode in ["valid", "none"] {
            let (mut op, _, raw) = held_setup();
            let sent = held_peer_cancellation(&mut op, raw, None, Some(mode));
            let error = op.service_held(1).unwrap_err();
            assert!(error.contains("held input released"), "{mode}: {error}");
            assert!(
                !op.brain_probes.invalid,
                "ordinary release is not a protocol fault"
            );
            assert!(op.brain_probes.first_fault.is_none());
            let before = sent.lock().unwrap().clone();
            assert_eq!(
                before
                    .iter()
                    .filter(|(r, _, _)| r["kind"] == "heartbeat")
                    .count(),
                1
            );
            assert!(!before.iter().any(|(r, _, _)| r["kind"] == "renew"));
            assert!(before.iter().any(|(r, _, _)| r["kind"] == "release"));
            // Drain the retained raw/close replies, then match a new passive probe.
            // No new gesture, automatic grant or heartbeat is issued.
            op.refresh_brain().unwrap();
            assert!(op.session.brain_fresh(op.now()));
            assert!(op.session.brain.as_ref().unwrap().held_generation.is_none());
            assert!(op.session.brain_final.as_ref().unwrap().reason.is_none());
            assert_eq!(
                sent.lock()
                    .unwrap()
                    .iter()
                    .filter(|(r, _, _)| r["kind"] == "heartbeat")
                    .count(),
                1
            );
        }
    }
    #[test]
    fn key_up_inside_post_heartbeat_pair_does_not_hide_wire_faults() {
        for mode in ["malformed", "late_malformed", "partial"] {
            let (mut op, _, raw) = held_setup();
            let sent = held_peer_cancellation(&mut op, raw, None, Some(mode));
            let error = op.service_held(1).unwrap_err();
            assert!(op.brain_probes.invalid, "{mode}: {error}");
            assert!(!error.starts_with("held input released"));
            assert!(op.brain_probes.first_fault.is_some());
            assert_eq!(
                sent.lock()
                    .unwrap()
                    .iter()
                    .filter(|(r, _, _)| r["kind"] == "heartbeat")
                    .count(),
                1
            );
        }
    }
    #[test]
    fn held_service_correlated_refusal_closes_even_if_snapshot_still_held() {
        for kind in ["brain_hold", "brain_heartbeat"] {
            let (mut op, peer, raw) = held_setup();
            let hold = (kind == "brain_hold").then(|| {
                op.session
                    .begin(kind, json!({"generation":"1"}), op.now())
                    .unwrap()
            });
            held_peer(&mut op, raw, None);
            let request = hold.unwrap_or_else(|| op.session.begin(kind, json!({"generation":"1", "observed_frame":op.session.brain.as_ref().unwrap().frame}), op.now()).unwrap());
            let (_, mut refusal) = replies(&op, &request, true);
            refusal["reason"] = json!("heartbeat observed frame stale/future");
            refusal["applied_frame"] = Value::Null;
            refusal["revision"] = json!(request.context.expected_revision.clone().unwrap());
            refusal["snapshot"]["revision"] = refusal["revision"].clone();
            peer.push(refusal);
            let sent = probe_connection(&mut op, &peer, false);
            let error = op.service_held(1).unwrap_err();
            assert!(error.contains("held Brain refused:"), "{error}");
            assert!(!op.brain_signal.as_ref().unwrap().live());
            assert!(op.session.pending.is_none());
            assert!(
                op.session.brain.as_ref().unwrap().held_generation.is_some(),
                "closure must not depend on heldNone readback"
            );
            let sent = sent.lock().unwrap();
            assert!(!sent.is_empty());
            assert!(
                sent.iter().all(|r| r["kind"] == "release"),
                "no new probe, heartbeat or renewal after refusal: {sent:?}"
            );
        }
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
        let fault = op.refresh_brain_until(original_deadline).unwrap_err();
        assert!(fault.starts_with("Brain paired observation deadline:"));
        assert!(fault.contains("processed=64 loops=64"));
        for field in [
            "elapsed_ms=",
            "budget_ms=",
            "probe=",
            "probe_age_ms=",
            "probe_generation=",
            "matched_id=",
            "matched_revision=",
            "matched_generation=",
            "raw_revision=",
            "raw_age_ms=",
            "raw_fresh=",
            "brain_revision=",
            "brain_age_ms=",
            "brain_fresh=",
            "requested_revision=",
            "pending=",
        ] {
            assert!(fault.contains(field), "missing {field}: {fault}");
        }
        assert!(fault.len() < 1024, "diagnostic remains scalar and bounded");
        assert_eq!(op.brain_probes.first_fault.as_deref(), Some(fault.as_str()));
        assert_eq!(
            op.refresh_brain().unwrap_err(),
            fault,
            "first failure retained"
        );
        assert!(op.brain_probes.invalid);
        assert_eq!(peer.0.lock().unwrap().len(), 4, "exactly64 frames consumed");
        let deadlines = deadlines.lock().unwrap();
        assert!(!deadlines.is_empty());
        assert!(deadlines.iter().all(|d| *d == original_deadline));
        assert!(sent.lock().unwrap().iter().all(|r| r["writer"].is_null()));
    }
    #[test]
    fn paired_trace_is_opt_in_bounded_and_reports_unavailable_transport() {
        for enabled in [false, true] {
            let (mut op, peer, _) = setup();
            probe_connection(&mut op, &peer, false);
            op.trace_timing = enabled;
            let error = op
                .refresh_brain_budget(Instant::now() + Duration::from_millis(100), &mut 0, false)
                .unwrap_err()
                .message();
            assert_eq!(error.contains("timing_totals_us="), enabled);
            if enabled {
                for field in [
                    "transport_before=None",
                    "transport_after=None",
                    "overwritten=0",
                    "overflow=false",
                    "tail3=",
                ] {
                    assert!(error.contains(field), "missing {field}");
                }
            }
            assert!(error.len() < 8000);
        }
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

#[cfg(test)]
mod timing_trace_tests {
    use super::*;
    #[test]
    fn trace_tail_and_cumulative_counters_are_bounded_and_overflow_is_distinct() {
        let mut trace = ProbeTiming::default();
        for n in 0..64 {
            trace.record(ProbeFrameTiming {
                bytes: n,
                micros: [1, 2, 3, 4],
                ..Default::default()
            });
        }
        assert_eq!(trace.frames, 64);
        assert_eq!(trace.overwritten, 60);
        assert_eq!(trace.totals, [64, 128, 192, 256]);
        assert_eq!(trace.tail.map(|f| f.bytes), [60, 61, 62, 63]);
        assert!(!trace.overflow);
        trace.totals[0] = u64::MAX;
        trace.record(ProbeFrameTiming {
            micros: [1, 0, 0, 0],
            ..Default::default()
        });
        assert_eq!(trace.overwritten, 61);
        assert!(trace.overflow);
        assert_eq!(trace.totals[0], u64::MAX);
    }
}
