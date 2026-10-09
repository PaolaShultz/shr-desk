//! GP-METER:1 independent read-only telemetry; no control admission or lease state.
type Result<T> = std::result::Result<T, String>;
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Counter(pub u64);
impl serde::Serialize for Counter {
    fn serialize<S: serde::Serializer>(&self, s: S) -> std::result::Result<S::Ok, S::Error> {
        s.serialize_str(&self.0.to_string())
    }
}
impl<'de> serde::Deserialize<'de> for Counter {
    fn deserialize<D: serde::Deserializer<'de>>(d: D) -> std::result::Result<Self, D::Error> {
        let s = String::deserialize(d)?;
        crate::provider::counter(&s)
            .map(Self)
            .map_err(serde::de::Error::custom)
    }
}
use serde::{Deserialize, Serialize};
fn uuid(s: &str) -> bool {
    s.len() == 36
        && s.bytes().enumerate().all(|(i, b)| {
            if [8, 13, 18, 23].contains(&i) {
                b == b'-'
            } else {
                b.is_ascii_digit() || (b'a'..=b'f').contains(&b)
            }
        })
}
pub const CONTRACT: &str = "GP-METER";
pub const MAX_TAPS: usize = 2048;
pub const MAX_BYTES: usize = 16 * 8192;
fn required<'de, D: serde::Deserializer<'de>, T: Deserialize<'de>>(
    d: D,
) -> std::result::Result<Option<T>, D::Error> {
    Option::deserialize(d)
}
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Request {
    pub contract: String,
    pub version: u32,
    pub kind: String,
    pub show_id: String,
    pub module: String,
    pub source_epoch: Counter,
    pub query_id: Counter,
    pub expected_map: Counter,
    #[serde(deserialize_with = "required")]
    pub writer: Option<String>,
    #[serde(deserialize_with = "required")]
    pub lease: Option<Counter>,
    #[serde(deserialize_with = "required")]
    pub request_id: Option<Counter>,
    #[serde(deserialize_with = "required")]
    pub expected_revision: Option<Counter>,
}
impl Request {
    pub fn new(show: &str, epoch: u64, map: u64, query: u64) -> Self {
        Self {
            contract: CONTRACT.into(),
            version: 1,
            kind: "meter_snapshot".into(),
            show_id: show.into(),
            module: "audio".into(),
            source_epoch: Counter(epoch),
            query_id: Counter(query),
            expected_map: Counter(map),
            writer: None,
            lease: None,
            request_id: None,
            expected_revision: None,
        }
    }
    pub fn validate(&self) -> Result<()> {
        if self.contract != CONTRACT
            || self.kind != "meter_snapshot"
            || self.module != "audio"
            || !uuid(&self.show_id)
            || self.source_epoch.0 == 0
            || self.query_id.0 == 0
            || self.expected_map.0 == 0
            || self.writer.is_some()
            || self.lease.is_some()
            || self.request_id.is_some()
            || self.expected_revision.is_some()
        {
            return Err("meter read envelope".into());
        }
        Ok(())
    }
    pub fn decode(bytes: &[u8]) -> Result<Self> {
        if bytes.len() > 65536 {
            return Err("meter request capacity".into());
        }
        let r: Self = serde_json::from_value(crate::provider::parse_document(bytes)?)
            .map_err(|e| e.to_string())?;
        r.validate()?;
        Ok(r)
    }
}
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct Tap {
    pub id: String,
    pub valid: bool,
    #[serde(deserialize_with = "required")]
    pub reason: Option<String>,
    #[serde(deserialize_with = "required")]
    pub peak_millidbfs: Option<i32>,
    #[serde(deserialize_with = "required")]
    pub rms_millidbfs: Option<i32>,
    pub silent: bool,
    pub below_floor: bool,
    pub over_range: bool,
    pub clip_count: Counter,
    pub invalid_count: Counter,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Snapshot {
    pub contract: String,
    pub version: u32,
    pub kind: String,
    pub show_id: String,
    pub module: String,
    pub source_epoch: Counter,
    pub query_id: Counter,
    pub capability_generation: Counter,
    pub map_generation: Counter,
    pub topology: String,
    pub sample_rate: u32,
    pub inputs: usize,
    pub monitors: usize,
    pub sequence: Counter,
    #[serde(deserialize_with = "required")]
    pub first_frame: Option<Counter>,
    #[serde(deserialize_with = "required")]
    pub end_frame: Option<Counter>,
    #[serde(deserialize_with = "required")]
    pub acquisition_age_ms: Option<Counter>,
    pub publication_loss: Counter,
    pub valid: bool,
    #[serde(deserialize_with = "required")]
    pub reason: Option<String>,
    pub taps: Vec<Tap>,
}
impl Snapshot {
    pub fn validate(&self) -> Result<()> {
        let count = self
            .inputs
            .checked_mul(2)
            .and_then(|n| n.checked_add(self.monitors))
            .and_then(|n| n.checked_add(2))
            .ok_or("meter inventory overflow")?;
        if self.contract != CONTRACT
            || self.version != 1
            || self.kind != "meter_snapshot"
            || self.module != "audio"
            || !uuid(&self.show_id)
            || self.source_epoch.0 == 0
            || self.query_id.0 == 0
            || self.map_generation.0 == 0
            || self.capability_generation.0 == 0
            || self.topology.is_empty()
            || self.topology.len() > 128
        {
            return Err("meter identity".into());
        }
        if !self.valid {
            if !matches!(
                self.reason.as_deref(),
                Some("missing_window" | "unsupported" | "identity" | "capacity" | "quiesced")
            ) || !self.taps.is_empty()
                || self.first_frame.is_some()
                || self.end_frame.is_some()
                || self.acquisition_age_ms.is_some()
            {
                return Err("meter unavailable shape".into());
            }
            return Ok(());
        }
        if count > MAX_TAPS
            || self.inputs == 0
            || self.sample_rate == 0
            || !self.sample_rate.is_multiple_of(50)
            || self.reason.is_some()
            || self.taps.len() != count
            || self.sequence.0 == 0
            || self.acquisition_age_ms.is_none()
        {
            return Err("meter window shape".into());
        }
        let first = self.first_frame.ok_or("first frame")?.0;
        let end = self.end_frame.ok_or("end frame")?.0;
        let frames = u64::from(self.sample_rate / 50);
        if first.checked_add(frames) != Some(end) {
            return Err("meter window frames".into());
        }
        for (i, t) in self.taps.iter().enumerate() {
            let id = if i < self.inputs * 2 {
                format!(
                    "input-{:02}:{}",
                    i / 2 + 1,
                    if i % 2 == 0 { "raw" } else { "processed" }
                )
            } else if i == self.inputs * 2 {
                "main-l".into()
            } else if i == self.inputs * 2 + 1 {
                "main-r".into()
            } else {
                format!("monitor-{}", i - self.inputs * 2 - 1)
            };
            if t.id != id
                || t.clip_count.0 > frames
                || t.invalid_count.0 > frames
                || t.clip_count
                    .0
                    .checked_add(t.invalid_count.0)
                    .is_none_or(|n| n > frames)
            {
                return Err("meter tap identity/count".into());
            }
            if t.valid {
                let p = t.peak_millidbfs.ok_or("peak")?;
                let r = t.rms_millidbfs.ok_or("rms")?;
                if t.reason.is_some()
                    || t.invalid_count.0 != 0
                    || !(-120000..=6165095).contains(&p)
                    || !(-120000..=6165095).contains(&r)
                    || r > p
                    || (t.silent
                        && (p != -120000
                            || r != -120000
                            || t.clip_count.0 != 0
                            || t.over_range
                            || !t.below_floor))
                    || (t.below_floor && p != -120000)
                    || (t.over_range && (p < 0 || t.clip_count.0 == 0))
                    || (t.clip_count.0 > 0 && p < 0)
                    || (p > 0 && !t.over_range)
                {
                    return Err("meter numeric relationships".into());
                }
            } else if (t.reason.as_deref() == Some("nonfinite")) != (t.invalid_count.0 > 0)
                || !matches!(
                    t.reason.as_deref(),
                    Some("nonfinite" | "graph_fault" | "quiesced")
                )
                || t.peak_millidbfs.is_some()
                || t.rms_millidbfs.is_some()
                || t.silent
                || t.below_floor
                || t.over_range
            {
                return Err("meter invalid shape".into());
            }
        }
        Ok(())
    }
    pub fn decode(bytes: &[u8]) -> Result<Self> {
        if bytes.len() > MAX_BYTES {
            return Err("meter document capacity".into());
        }
        let s: Self = serde_json::from_value(crate::provider::parse_document(bytes)?)
            .map_err(|e| e.to_string())?;
        s.validate()?;
        Ok(s)
    }
}

use crate::{
    frontend::Config,
    local_audio::{AuthorityConnection, Transport},
    topology::Topology,
};
use std::{
    collections::BTreeMap,
    sync::{
        Arc, Mutex,
        atomic::{AtomicBool, Ordering},
    },
    thread::{self, JoinHandle},
    time::{Duration, Instant},
};
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Identity {
    pub show: String,
    pub epoch: u64,
    pub map: u64,
    pub topology: String,
    pub inputs: usize,
    pub monitors: usize,
    pub rate: u32,
}
impl Identity {
    pub fn new(show: &str, epoch: u64, t: &Topology) -> Self {
        Self {
            show: show.into(),
            epoch,
            map: t.map_revision,
            topology: t.identity.clone(),
            inputs: t.inputs.len(),
            monitors: t.monitors,
            rate: t.sample_rate,
        }
    }
}
#[derive(Clone, Debug, Default)]
pub struct Cache {
    pub snapshot: Option<Snapshot>,
    pub message: String,
    received: Option<Instant>,
    initial_age: u64,
    expires: Option<Instant>,
    clips: BTreeMap<String, Instant>,
}
impl Cache {
    pub fn clear(&mut self, message: &str) {
        *self = Self {
            message: message.into(),
            ..Self::default()
        };
    }
    pub fn age(&self, now: Instant) -> Option<u64> {
        self.received.map(|t| {
            self.initial_age.saturating_add(
                now.saturating_duration_since(t)
                    .as_millis()
                    .min(u128::from(u64::MAX)) as u64,
            )
        })
    }
    pub fn fresh(&self, now: Instant) -> bool {
        self.snapshot.as_ref().is_some_and(|s| s.valid) && self.expires.is_some_and(|t| now <= t)
    }
    pub fn clipped(&self, id: &str, now: Instant) -> bool {
        self.fresh(now) && self.clips.get(id).is_some_and(|t| now < *t)
    }
    pub fn accept(
        &mut self,
        s: Snapshot,
        identity: &Identity,
        query: u64,
        elapsed: Duration,
        now: Instant,
    ) -> Result<()> {
        s.validate()?;
        if elapsed > Duration::from_millis(100)
            || s.show_id != identity.show
            || s.source_epoch.0 != identity.epoch
            || s.query_id.0 != query
            || s.map_generation.0 != identity.map
            || s.topology != identity.topology
            || s.inputs != identity.inputs
            || s.monitors != identity.monitors
            || s.sample_rate != identity.rate
        {
            self.clear("IDENTITY / UNAVAILABLE");
            return Err("meter query/identity/deadline".into());
        }
        if !s.valid {
            self.clear(s.reason.as_deref().unwrap_or("UNAVAILABLE"));
            return Ok(());
        }
        let mut age = s
            .acquisition_age_ms
            .unwrap()
            .0
            .checked_add(
                elapsed
                    .as_millis()
                    .try_into()
                    .map_err(|_| "meter age overflow")?,
            )
            .ok_or("meter age overflow")?;
        let mut expiry = now + Duration::from_millis(250u64.saturating_sub(age));
        let repeated = if let Some(old) = &self.snapshot {
            if s.capability_generation != old.capability_generation
                || s.sequence.0 < old.sequence.0
                || s.first_frame.unwrap().0 < old.first_frame.unwrap().0
            {
                self.clear("REGRESSION / UNAVAILABLE");
                return Err("meter regression/generation".into());
            }
            if s.sequence == old.sequence {
                if s.first_frame != old.first_frame
                    || s.end_frame != old.end_frame
                    || s.taps != old.taps
                    || s.acquisition_age_ms.unwrap().0 < old.acquisition_age_ms.unwrap().0
                {
                    self.clear("INVALID / UNAVAILABLE");
                    return Err("meter repeated window changed".into());
                }
                age = age.max(self.age(now).unwrap_or(0));
                if let Some(prior) = self.expires {
                    expiry = expiry.min(prior);
                }
                true
            } else {
                if s.first_frame.unwrap().0 < old.end_frame.unwrap().0 {
                    self.clear("INVALID / UNAVAILABLE");
                    return Err("meter overlapping window".into());
                }
                false
            }
        } else {
            false
        };
        if age > 250 {
            expiry = expiry.min(now.checked_sub(Duration::from_millis(1)).unwrap_or(now));
        }
        if !repeated && age <= 250 {
            for tap in &s.taps {
                if tap.valid && tap.clip_count.0 > 0 {
                    self.clips
                        .insert(tap.id.clone(), now + Duration::from_secs(1));
                }
            }
        }
        self.initial_age = age;
        self.received = Some(now);
        self.expires = Some(expiry);
        self.snapshot = Some(s);
        self.message = "GP-METER:1".into();
        Ok(())
    }
}
pub struct Worker {
    latest: Arc<Mutex<Option<Cache>>>,
    stop: Arc<AtomicBool>,
    child: Option<JoinHandle<()>>,
}
impl Worker {
    pub fn start(config: Config, identity: Identity) -> Self {
        let latest = Arc::new(Mutex::new(None));
        let stop = Arc::new(AtomicBool::new(false));
        let (out, halt) = (latest.clone(), stop.clone());
        let child = thread::spawn(move || {
            let mut connection: Option<Box<dyn AuthorityConnection>> = None;
            let mut query = 0u64;
            let mut cache = Cache::default();
            let mut unsupported = false;
            while !halt.load(Ordering::Acquire) {
                let began = Instant::now();
                let deadline = began + Duration::from_millis(100);
                let result = (|| -> Result<()> {
                    if unsupported {
                        return Ok(());
                    }
                    if connection.is_none() {
                        connection = Some(if let Some(remote) = &config.remote {
                            let c = crate::remote::Connection::connect_observer(remote, deadline)?;
                            if c.source_epoch != identity.epoch {
                                return Err("meter session epoch".into());
                            }
                            Box::new(c)
                        } else {
                            Box::new(Transport::connect_until(&config.endpoint, deadline)?)
                        });
                        if Instant::now() >= deadline {
                            return Err("meter attach deadline".into());
                        }
                    }
                    query = query.checked_add(1).ok_or("meter query exhausted")?;
                    let request = Request::new(&identity.show, identity.epoch, identity.map, query);
                    let c = connection.as_mut().ok_or("meter connection")?;
                    c.send_frame_until(
                        &serde_json::to_vec(&request).map_err(|e| e.to_string())?,
                        deadline,
                    )?;
                    let bytes = c
                        .receive_meter_until(deadline)?
                        .ok_or("meter response missing")?;
                    let value = crate::provider::parse_document(&bytes)?;
                    if value.get("contract").and_then(|v| v.as_str()) != Some(CONTRACT) {
                        unsupported = true;
                        cache.clear("UNSUPPORTED / UNAVAILABLE");
                        return Ok(());
                    }
                    let s = Snapshot::decode(&bytes)?;
                    if s.reason.as_deref() == Some("unsupported") {
                        unsupported = true;
                    }
                    // QUIC outer framing verifies session and capability; map is independently checked.
                    if let Some(pin) = c.held_identity()
                        && (crate::provider::counter(&pin.capability)? != s.capability_generation.0
                            || crate::provider::counter(&pin.map)? != s.map_generation.0)
                    {
                        return Err("meter session generation".into());
                    }
                    let now = Instant::now();
                    cache.accept(s, &identity, query, now.duration_since(began), now)
                })();
                if let Err(e) = result {
                    connection = None;
                    cache.clips.clear();
                    cache.expires = Instant::now().checked_sub(Duration::from_millis(1));
                    cache.message = format!("METER UNAVAILABLE: {e}"); /* retain values with original expiry, never refresh */
                }
                let update = cache.clone();
                *out.lock().unwrap() = Some(update);
                let rest = Duration::from_millis(40).saturating_sub(began.elapsed());
                thread::sleep(rest);
            }
        });
        Self {
            latest,
            stop,
            child: Some(child),
        }
    }
    pub fn stop(&self) {
        self.stop.store(true, Ordering::Release);
    }
    pub fn stopping(&self) -> bool {
        self.stop.load(Ordering::Acquire)
    }
    pub fn finished(&self) -> bool {
        self.child.as_ref().is_none_or(|c| c.is_finished())
    }
    pub fn take(&self) -> Option<Cache> {
        self.latest.lock().unwrap().take()
    }
}
impl Drop for Worker {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::Release);
        if let Some(c) = self.child.take()
            && c.is_finished()
        {
            let _ = c.join();
        }
    }
}

/// Meter transport is stricter than the legacy shared document parser. The
/// allowance includes the authenticated outer envelope, never more pages.
pub(crate) fn admit_page(frame: &[u8]) -> Result<()> {
    let v = crate::provider::parse(frame)?;
    if v["contract"] == "GP14-snapshot-pages"
        && (v["count"].as_u64().is_none_or(|n| n == 0 || n > 16)
            || v["total_bytes"]
                .as_u64()
                .is_none_or(|n| n > (MAX_BYTES + 4096) as u64))
    {
        return Err("meter page capacity".into());
    }
    Ok(())
}
