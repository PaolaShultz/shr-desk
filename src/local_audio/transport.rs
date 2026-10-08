//! Authenticated bounded Unix framing and the byte-only authority connection seam.
use crate::audio::Request;
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
    pub(super) fn poll(fds: *mut PollFd, count: usize, timeout: i32) -> i32;
}
#[repr(C)]
pub(super) struct PollFd {
    pub(super) fd: i32,
    pub(super) events: i16,
    pub(super) revents: i16,
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
    #[cfg(test)]
    pub(super) fn from_stream(socket: UnixStream) -> Self {
        Self { socket }
    }
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
                Err(e) if e.kind() == std::io::ErrorKind::Interrupted => {}
                Err(e)
                    if matches!(
                        e.kind(),
                        std::io::ErrorKind::WouldBlock | std::io::ErrorKind::TimedOut
                    ) =>
                {
                    // The kernel's 100ms wait slice is not a new frame budget.
                    // Preserve consumed-byte position and the original deadline.
                    if Instant::now() >= deadline {
                        return Err("remaining-frame read deadline".into());
                    }
                }
                Err(e) => return Err(e.to_string()),
            }
        }
        Ok(())
    }
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
