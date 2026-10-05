//! Reassembles one immutable GP14 document; never merges separate observations.
use crate::{local_audio::AuthorityConnection, provider};
use serde::Deserialize;
use sha2::{Digest, Sha256};
use std::time::{Duration, Instant};
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Page {
    contract: String,
    version: u8,
    identity: String,
    index: usize,
    count: usize,
    total_bytes: usize,
    payload: String,
}
#[derive(Default)]
pub struct Assembly {
    identity: String,
    count: usize,
    total: usize,
    next: usize,
    bytes: Vec<u8>,
    started: Option<Instant>,
}
impl Assembly {
    pub fn offer(&mut self, frame: Vec<u8>, now: Instant) -> Result<Option<Vec<u8>>, String> {
        let result = self.offer_inner(frame, now);
        if result.is_err() {
            *self = Self::default();
        }
        result
    }
    fn offer_inner(&mut self, frame: Vec<u8>, now: Instant) -> Result<Option<Vec<u8>>, String> {
        self.check_deadline(now)?;
        let value = provider::parse(&frame)?;
        if value["contract"] != "GP14-snapshot-pages" {
            if self.started.is_some() {
                return Err("interleaved unsegmented document".into());
            }
            return Ok(Some(frame));
        }
        let p: Page = serde_json::from_value(value).map_err(|e| e.to_string())?;
        if p.contract != "GP14-snapshot-pages"
            || p.version != 1
            || p.total_bytes <= provider::MAX_BYTES
            || p.total_bytes > provider::MAX_DOCUMENT_BYTES
            || p.count == 0
            || p.count > 129
            || p.index >= p.count
            || p.payload.is_empty()
            || p.payload.len() > 8192
            || p.identity.len() != 64
            || !p
                .identity
                .bytes()
                .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
        {
            return Err("snapshot page admission".into());
        }
        if self.started.is_none() {
            if p.index != 0 {
                return Err("snapshot first page".into());
            }
            self.started = Some(now);
            self.identity = p.identity.clone();
            self.count = p.count;
            self.total = p.total_bytes;
            self.bytes = Vec::with_capacity(self.total);
        }
        if p.identity != self.identity
            || p.count != self.count
            || p.total_bytes != self.total
            || p.index != self.next
        {
            return Err("mixed/duplicate/reordered snapshot pages".into());
        }
        if self
            .bytes
            .len()
            .checked_add(p.payload.len())
            .is_none_or(|n| n > self.total)
        {
            return Err("snapshot size overflow".into());
        }
        self.bytes.extend_from_slice(p.payload.as_bytes());
        self.next += 1;
        if self.next != self.count {
            return Ok(None);
        }
        if self.bytes.len() != self.total
            || format!("{:x}", Sha256::digest(&self.bytes)) != self.identity
        {
            return Err("snapshot document checksum".into());
        }
        let bytes = std::mem::take(&mut self.bytes);
        *self = Self::default();
        Ok(Some(bytes))
    }
    fn check_deadline(&self, now: Instant) -> Result<(), String> {
        if self
            .started
            .is_some_and(|start| now < start || now.duration_since(start) > Duration::from_secs(2))
        {
            return Err("snapshot assembly expired".into());
        }
        Ok(())
    }
}
pub struct Connection {
    inner: Box<dyn AuthorityConnection>,
    assembly: Assembly,
}
impl Connection {
    pub fn new(inner: Box<dyn AuthorityConnection>) -> Self {
        Self {
            inner,
            assembly: Assembly::default(),
        }
    }
}
impl AuthorityConnection for Connection {
    fn send_frame_until(&mut self, bytes: &[u8], deadline: Instant) -> Result<(), String> {
        self.inner.send_frame_until(bytes, deadline)
    }
    fn receive_until(&mut self, deadline: Instant) -> Result<Option<Vec<u8>>, String> {
        for _ in 0..129 {
            if Instant::now() >= deadline {
                return Err("snapshot read deadline".into());
            }
            self.assembly.check_deadline(Instant::now())?;
            let Some(frame) = self.inner.receive_until(deadline)? else {
                return Ok(None);
            };
            if Instant::now() >= deadline {
                return Err("snapshot read deadline".into());
            }
            let document = self.assembly.offer(frame, Instant::now())?;
            if Instant::now() >= deadline {
                return Err("snapshot read deadline".into());
            }
            if document.is_some() {
                return Ok(document);
            }
        }
        Err("snapshot page drain capacity".into())
    }
    fn receive_available(&mut self) -> Result<Option<Vec<u8>>, String> {
        self.receive_available_until(Instant::now() + Duration::from_millis(200))
    }
    fn receive_available_until(&mut self, deadline: Instant) -> Result<Option<Vec<u8>>, String> {
        // Keep the caller's absolute budget through every layer and segment.
        for _ in 0..129 {
            if Instant::now() >= deadline {
                return Err("snapshot read deadline".into());
            }
            self.assembly.check_deadline(Instant::now())?;
            let Some(frame) = self.inner.receive_available_until(deadline)? else {
                return Ok(None);
            };
            if Instant::now() >= deadline {
                return Err("snapshot read deadline".into());
            }
            let document = self.assembly.offer(frame, Instant::now())?;
            if Instant::now() >= deadline {
                return Err("snapshot read deadline".into());
            }
            if document.is_some() {
                return Ok(document);
            }
        }
        Err("snapshot page drain capacity".into())
    }
}
