//! Exact GP05 read-only metadata consumer. No owner DSP or recorder commands.
use crate::{frontend::Config, local_audio::Transport, provider};
use serde::{
    Deserialize, Serialize,
    de::{self, MapAccess, SeqAccess, Visitor},
};
use serde_json::{Value, json};
use std::{
    collections::BTreeMap,
    fmt,
    sync::{
        Arc, Mutex,
        atomic::{AtomicBool, Ordering},
    },
    thread::{self, JoinHandle},
    time::{Duration, Instant},
};

struct Unique(Value);
impl<'de> Deserialize<'de> for Unique {
    fn deserialize<D: serde::Deserializer<'de>>(d: D) -> Result<Self, D::Error> {
        struct V;
        impl<'de> Visitor<'de> for V {
            type Value = Unique;
            fn expecting(&self, f: &mut fmt::Formatter) -> fmt::Result {
                f.write_str("bounded unique JSON")
            }
            fn visit_bool<E: de::Error>(self, v: bool) -> Result<Unique, E> {
                Ok(Unique(v.into()))
            }
            fn visit_i64<E: de::Error>(self, v: i64) -> Result<Unique, E> {
                Ok(Unique(v.into()))
            }
            fn visit_u64<E: de::Error>(self, v: u64) -> Result<Unique, E> {
                Ok(Unique(v.into()))
            }
            fn visit_f64<E: de::Error>(self, v: f64) -> Result<Unique, E> {
                serde_json::Number::from_f64(v)
                    .map(|n| Unique(Value::Number(n)))
                    .ok_or_else(|| E::custom("nonfinite quantity"))
            }
            fn visit_str<E: de::Error>(self, v: &str) -> Result<Unique, E> {
                Ok(Unique(v.into()))
            }
            fn visit_string<E: de::Error>(self, v: String) -> Result<Unique, E> {
                Ok(Unique(v.into()))
            }
            fn visit_unit<E: de::Error>(self) -> Result<Unique, E> {
                Ok(Unique(Value::Null))
            }
            fn visit_seq<A: SeqAccess<'de>>(self, mut a: A) -> Result<Unique, A::Error> {
                let mut list = Vec::new();
                while let Some(v) = a.next_element::<Unique>()? {
                    list.push(v.0);
                }
                Ok(Unique(list.into()))
            }
            fn visit_map<A: MapAccess<'de>>(self, mut a: A) -> Result<Unique, A::Error> {
                let mut map = serde_json::Map::new();
                while let Some(k) = a.next_key::<String>()? {
                    if map.contains_key(&k) {
                        return Err(de::Error::custom("duplicate field"));
                    }
                    map.insert(k, a.next_value::<Unique>()?.0);
                }
                Ok(Unique(Value::Object(map)))
            }
        }
        d.deserialize_any(V)
    }
}
fn parse(bytes: &[u8]) -> Result<Value, String> {
    if bytes.len() > provider::MAX_BYTES {
        return Err("module frame capacity".into());
    }
    fn depth(v: &Value, n: usize) -> bool {
        n <= 12
            && match v {
                Value::Array(a) => a.iter().all(|v| depth(v, n + 1)),
                Value::Object(m) => m.values().all(|v| depth(v, n + 1)),
                _ => true,
            }
    }
    let v = serde_json::from_slice::<Unique>(bytes)
        .map_err(|e| e.to_string())?
        .0;
    if !depth(&v, 0) {
        return Err("module JSON depth".into());
    }
    Ok(v)
}
fn required_option<'de, D: serde::Deserializer<'de>, T: Deserialize<'de>>(
    d: D,
) -> Result<Option<T>, D::Error> {
    Option::deserialize(d)
}
macro_rules! dto {
    ($name:ident { $($field:ident : $type:ty),* $(,)? }) => {
        #[derive(Clone,Debug,Deserialize,Serialize)] #[serde(deny_unknown_fields)]
        pub struct $name { $(pub $field: $type),* }
    };
}
dto!(Configuration { sample_rate:u32, block_frames:u32, raw_inputs:Vec<String>, fx_mix:String, pa_inputs:u32, pa_outputs:u32 });
dto!(Library {
    sha256: String,
    state: String
});
dto!(FxCapabilities {
    version: u32,
    size: u32,
    identity: [i8; 32],
    min_sample_rate: u32,
    max_sample_rate: u32,
    min_block_frames: u32,
    max_block_frames: u32,
    channels: u32,
    sample_bits: u32,
    reset_supported: u32,
    writable_parameters: u32,
    rack_available: u32,
    adapter_buffer_frames: u32,
    delay_ms: f64,
    feedback: f64,
    damping: f64,
    wet_gain: f64
});
dto!(FxStatus {
    version: u32,
    size: u32,
    sample_rate: u32,
    max_block_frames: u32,
    intentional_delay_frames: u32,
    adapter_buffer_frames: u32,
    last_process_result: i32,
    reset_reason: u32,
    reset_count: u64
});
dto!(Fx {
    capabilities: FxCapabilities,
    status: FxStatus
});
dto!(PaDescriptor {
    version: u32,
    size: u32,
    input_channels: u32,
    logical_outputs: u32,
    active_output_mask: u32,
    silent_output_mask: u32,
    physical_io_owned: u32,
    sample_format: u32,
    fixed_preset: u32,
    limiter_kind: u32,
    limiter_threshold_millidbfs: i32,
    limiter_linked: u32,
    limiter_release_ms: u32,
    startup_ramp_ms: u32,
    fixed_delay_frames: u32,
    min_rate: u32,
    max_rate: u32,
    min_block: u32,
    max_block: u32,
    unavailable_capabilities: u32
});
dto!(PaStatus {
    version: u32,
    size: u32,
    sample_rate: u32,
    max_block: u32,
    fault_latched: u32,
    recreate_required: u32
});
dto!(Pa {
    descriptor: PaDescriptor,
    status: PaStatus
});
#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct Recording {
    pub take_id: String,
    pub operation_id: String,
    pub source_epoch: String,
    #[serde(deserialize_with = "required_option")]
    pub first_source_frame: Option<String>,
    pub mapping: Vec<String>,
    pub state: String,
    pub outcome: String,
    pub accepted_frames: String,
    pub written_frames: String,
    #[serde(deserialize_with = "required_option")]
    pub durable_frames: Option<String>,
    pub dropped_frames: String,
    pub overflow_blocks: String,
    pub gap_frames: String,
    pub invalid_blocks: String,
    pub clipped_samples: String,
    pub writer_fault: bool,
    pub host_fault: u32,
}
#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct Status {
    pub contract: String,
    pub version: u32,
    pub show_id: String,
    pub module: String,
    pub epoch: String,
    pub frame: String,
    pub available: bool,
    pub physical: String,
    pub protection: String,
    pub readiness: String,
    pub activity: String,
    pub source_fault: u32,
    #[serde(deserialize_with = "required_option")]
    pub configuration: Option<Configuration>,
    pub libraries: BTreeMap<String, Library>,
    #[serde(deserialize_with = "required_option")]
    pub fx: Option<Fx>,
    #[serde(deserialize_with = "required_option")]
    pub pa: Option<Pa>,
    #[serde(deserialize_with = "required_option")]
    pub recording: Option<Recording>,
}
fn mapping(v: &[String]) -> bool {
    v.len() == 8
        && v.iter()
            .enumerate()
            .all(|(i, s)| s == &format!("input-{:02}/raw", i + 1))
}
impl Status {
    fn validate(&self, show: &str, epoch: u64) -> Result<(), String> {
        if self.contract != "GP05-modules"
            || self.version != 1
            || !provider::uuid(&self.show_id)
            || self.show_id != show
            || self.module != "audio"
            || epoch == 0
            || provider::counter(&self.epoch)? != epoch
            || self.physical != "unverified"
            || ![0, 6, 7].contains(&self.source_fault)
            || !["unavailable", "ready", "degraded", "faulted"].contains(&self.readiness.as_str())
            || !["idle", "processed"].contains(&self.activity.as_str())
        {
            return Err("module identity/domain".into());
        }
        provider::counter(&self.frame)?;
        if !self.available {
            if self.configuration.is_some()
                || self.fx.is_some()
                || self.pa.is_some()
                || self.recording.is_some()
                || !self.libraries.is_empty()
                || self.protection != "offline-unprotected"
                || self.readiness != "unavailable"
            {
                return Err("unavailable module carries capabilities".into());
            }
            return Ok(());
        }
        let c = self
            .configuration
            .as_ref()
            .ok_or("missing module configuration")?;
        if c.sample_rate != 48000
            || c.block_frames != 48
            || !mapping(&c.raw_inputs)
            || c.fx_mix != "dry-plus-fixed-wet"
            || c.pa_inputs != 2
            || c.pa_outputs != 6
            || self.protection != "owner-sample-limiter-only"
            || self.readiness == "unavailable"
        {
            return Err("fixed module configuration".into());
        }
        if self.libraries.len() != 3
            || !["rec", "fx", "pa"]
                .iter()
                .all(|k| self.libraries.contains_key(*k))
        {
            return Err("module library set".into());
        }
        for l in self.libraries.values() {
            if l.sha256.len() != 64
                || !l
                    .sha256
                    .bytes()
                    .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
                || !["verified", "loaded"].contains(&l.state.as_str())
            {
                return Err("module library metadata".into());
            }
        }
        let fx = self.fx.as_ref().ok_or("missing FX descriptor")?;
        let p = &fx.capabilities;
        let end = p
            .identity
            .iter()
            .position(|b| *b == 0)
            .ok_or("FX identity NUL")?;
        if end == 0
            || p.identity[end..].iter().any(|b| *b != 0)
            || p.identity[..end].iter().any(|b| *b < 32 || *b > 126)
            || p.version != 1
            || p.size != 112
            || p.channels != 2
            || p.sample_bits != 64
            || p.reset_supported != 1
            || p.writable_parameters != 0
            || p.rack_available != 0
            || p.adapter_buffer_frames != 0
            || p.min_sample_rate > c.sample_rate
            || p.max_sample_rate < c.sample_rate
            || p.min_block_frames > c.block_frames
            || p.max_block_frames < c.block_frames
            || ![p.delay_ms, p.feedback, p.damping, p.wet_gain]
                .iter()
                .all(|v| v.is_finite())
        {
            return Err("fixed FX capabilities/identity".into());
        }
        let s = &fx.status;
        if s.version != 1
            || s.size != 40
            || s.sample_rate != c.sample_rate
            || s.max_block_frames != c.block_frames
            || s.adapter_buffer_frames != 0
        {
            return Err("FX status domain".into());
        }
        let pa = self.pa.as_ref().ok_or("missing PA descriptor")?;
        let p = &pa.descriptor;
        if p.version != 1
            || p.size != 80
            || p.input_channels != 2
            || p.logical_outputs != 6
            || p.active_output_mask != 3
            || p.silent_output_mask != 60
            || p.physical_io_owned != 0
            || p.sample_format != 1
            || p.fixed_preset != 1
            || p.limiter_kind != 1
            || p.limiter_threshold_millidbfs != -1000
            || p.limiter_linked != 1
            || p.unavailable_capabilities != 15
            || p.min_rate > c.sample_rate
            || p.max_rate < c.sample_rate
            || p.min_block > c.block_frames
            || p.max_block < c.block_frames
        {
            return Err("fixed PA descriptor".into());
        }
        let s = &pa.status;
        if s.version != 1
            || s.size != 24
            || s.sample_rate != c.sample_rate
            || s.max_block != c.block_frames
            || s.fault_latched > 1
            || s.recreate_required > 1
        {
            return Err("PA status domain".into());
        }
        if let Some(r) = &self.recording {
            if !provider::id(&r.take_id)
                || provider::counter(&r.operation_id)? == 0
                || provider::counter(&r.source_epoch)? == 0
                || !mapping(&r.mapping)
                || !["preparing", "recording", "finalizing", "finalized"]
                    .contains(&r.state.as_str())
                || !["pending", "complete", "incomplete", "error"].contains(&r.outcome.as_str())
                || r.durable_frames.is_some()
                || r.state == "preparing" && r.first_source_frame.is_some()
            {
                return Err("recording identity/state/durability".into());
            }
            for n in [
                &r.accepted_frames,
                &r.written_frames,
                &r.dropped_frames,
                &r.overflow_blocks,
                &r.gap_frames,
                &r.invalid_blocks,
                &r.clipped_samples,
            ] {
                provider::counter(n)?;
            }
            if let Some(n) = &r.first_source_frame {
                provider::counter(n)?;
            }
        }
        Ok(())
    }
    pub fn lines(&self) -> Vec<String> {
        if !self.available {
            return vec![
                "GP05 optional modules unavailable / raw GP03 mixer remains separate".into(),
            ];
        }
        let mut lines = vec![format!(
            "GP05 OPTIONAL POSTMIXER: {} / {} / source fault {} / physical UNVERIFIED",
            self.readiness, self.activity, self.source_fault
        )];
        if let Some(r) = &self.recording {
            lines.push(format!(
                "REC {} / {} / take {} / operation {}",
                r.state, r.outcome, r.take_id, r.operation_id
            ));
            lines.push(format!(
                "REC accepted {} / written {} frames / durable UNKNOWN",
                r.accepted_frames, r.written_frames
            ));
            lines.push(format!(
                "REC dropped {} / gaps {} / invalid {} / writer fault {} / host fault {}",
                r.dropped_frames, r.gap_frames, r.invalid_blocks, r.writer_fault, r.host_fault
            ));
        } else {
            lines.push("REC no active/retained take / durable UNKNOWN".into());
        }
        if let Some(f) = &self.fx {
            lines.push(format!("FX fixed wet-only delay {} ms / {} frames / writable parameters and rack unavailable",f.capabilities.delay_ms,f.status.intentional_delay_frames));
            lines.push(format!(
                "FX last result {} / reset reason {} / reset count {}",
                f.status.last_process_result, f.status.reset_reason, f.status.reset_count
            ));
        }
        if let Some(p) = &self.pa {
            lines.push("PA logical 2->6 / active outputs 0,1 / silent 2..5 / sample limiter MAIN ONLY -1.0 dBFS".into());
            lines.push(format!(
                "PA fault latch {} / recreate {} / physical and acoustic protection UNVERIFIED",
                p.status.fault_latched, p.status.recreate_required
            ));
        }
        for (name, l) in &self.libraries {
            lines.push(format!("{} library {} / {}", name, l.state, l.sha256));
        }
        lines
    }
    pub fn compact(&self) -> String {
        if !self.available {
            return "MODULES unavailable / raw GP03 mixer only".into();
        }
        format!(
            "MODULES {} / REC {} / FX fixed wet / PA main-only sample limiter / physical UNVERIFIED",
            self.readiness,
            self.recording
                .as_ref()
                .map_or("no take", |r| r.state.as_str())
        )
    }
}
pub fn decode(bytes: &[u8], show: &str, epoch: u64) -> Result<Status, String> {
    let s: Status = serde_json::from_value(parse(bytes)?).map_err(|e| e.to_string())?;
    s.validate(show, epoch)?;
    Ok(s)
}
pub fn request(show: &str, epoch: u64) -> Result<Vec<u8>, String> {
    if !provider::uuid(show) || epoch == 0 {
        return Err("module query identity".into());
    }
    serde_json::to_vec(&json!({"contract":"GP05-modules","version":1,"show_id":show,"module":"audio","epoch":epoch.to_string(),"writer":null,"lease":null,"request_id":null,"expected_revision":null,"kind":"module_status","body":{}})).map_err(|e|e.to_string())
}
pub fn query(config: &Config) -> Result<Status, String> {
    let deadline = Instant::now() + Duration::from_millis(250);
    let mut transport = Transport::connect_until(&config.endpoint, deadline)?;
    transport.send_frame_until(&request(&config.show, config.epoch)?, deadline)?;
    for _ in 0..64 {
        if Instant::now() >= deadline {
            break;
        }
        if let Some(bytes) = transport.receive_until(deadline)? {
            let v = parse(&bytes)?;
            if v["contract"] == "GP05-modules" {
                let s: Status = serde_json::from_value(v).map_err(|e| e.to_string())?;
                s.validate(&config.show, config.epoch)?;
                if Instant::now() >= deadline {
                    return Err("module health total deadline".into());
                }
                return Ok(s);
            }
            if v["capability"] != "GP03-rendered" {
                return Err("module health unsupported reply".into());
            }
        }
    }
    Err("module health deadline/queue bound".into())
}
#[derive(Clone, Debug)]
pub struct Update {
    pub status: Option<Status>,
    pub message: String,
    pub received: Instant,
}
impl Update {
    pub fn fresh(&self) -> bool {
        self.status.is_some() && self.received.elapsed() <= Duration::from_millis(750)
    }
}
pub struct Worker {
    latest: Arc<Mutex<Option<Update>>>,
    stop: Arc<AtomicBool>,
    child: Option<JoinHandle<()>>,
}
impl Worker {
    pub fn start(config: Config) -> Self {
        let latest = Arc::new(Mutex::new(None));
        let stop = Arc::new(AtomicBool::new(false));
        let (l, s) = (latest.clone(), stop.clone());
        let child = thread::spawn(move || {
            while !s.load(Ordering::Acquire) {
                let began = Instant::now();
                let result = query(&config);
                let (status, message) = match result {
                    Ok(status) => {
                        let text = status.compact();
                        (Some(status), text)
                    }
                    Err(e) => (None, format!("MODULES UNAVAILABLE: {e}")),
                };
                *l.lock().unwrap() = Some(Update {
                    status,
                    message,
                    received: Instant::now(),
                });
                while !s.load(Ordering::Acquire) && began.elapsed() < Duration::from_millis(250) {
                    thread::sleep(Duration::from_millis(10));
                }
            }
        });
        Self {
            latest,
            stop,
            child: Some(child),
        }
    }
    pub fn take(&self) -> Option<Update> {
        self.latest.lock().unwrap().take()
    }
}
impl Drop for Worker {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::Release);
        if let Some(child) = self.child.take() {
            let _ = child.join();
        }
    }
}
