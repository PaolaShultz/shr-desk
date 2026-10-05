//! Brain-owned duplex device configuration DTOs. Desk never opens PCM endpoints.
use serde::{Deserialize, Serialize};
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SampleFormat {
    S16Le,
    S32Le,
    Float32Le,
}
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Port {
    pub id: String,
    pub socket: String,
    pub slot: usize,
}
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Config {
    pub device_id: String,
    pub endpoint: String,
    pub sample_rate: u32,
    pub capture_channels: usize,
    pub playback_channels: usize,
    pub format: SampleFormat,
    pub period_frames: usize,
    pub buffer_frames: usize,
    pub microphone: Port,
    pub monitor: [Port; 2],
    pub socket_mapping_record: Option<String>,
    pub shared_clock_record: Option<String>,
    pub hardware_monitoring_record: Option<String>,
}
impl Config {
    pub fn validate(&self) -> Result<(), String> {
        let label =
            |s: &str| !s.trim().is_empty() && s.len() <= 256 && !s.chars().any(char::is_control);
        if !label(&self.device_id)
            || !label(&self.endpoint)
            || self.sample_rate != 48000
            || !(1..=256).contains(&self.capture_channels)
            || !(2..=256).contains(&self.playback_channels)
            || !(48..=1024).contains(&self.period_frames)
            || !self.period_frames.is_multiple_of(48)
            || self.buffer_frames < 2 * self.period_frames
            || self.buffer_frames > 48000
        {
            return Err("device configuration domain".into());
        }
        if [&self.microphone, &self.monitor[0], &self.monitor[1]]
            .iter()
            .any(|p| !label(&p.id) || !label(&p.socket))
            || self.microphone.slot >= self.capture_channels
            || self
                .monitor
                .iter()
                .any(|p| p.slot >= self.playback_channels || p.id == self.microphone.id)
            || self.monitor[0].slot == self.monitor[1].slot
            || self.monitor[0].id == self.monitor[1].id
            || self.monitor[0].socket == self.monitor[1].socket
        {
            return Err("device mapping domain".into());
        }
        for s in [
            &self.socket_mapping_record,
            &self.shared_clock_record,
            &self.hardware_monitoring_record,
        ]
        .into_iter()
        .flatten()
        {
            if s.trim().is_empty() || s.len() > 4096 {
                return Err("device physical observation record".into());
            }
        }
        Ok(())
    }
    pub fn decode(value: serde_json::Value) -> Result<Self, String> {
        crate::provider::keys(
            &value,
            &[
                "device_id",
                "endpoint",
                "sample_rate",
                "capture_channels",
                "playback_channels",
                "format",
                "period_frames",
                "buffer_frames",
                "microphone",
                "monitor",
                "socket_mapping_record",
                "shared_clock_record",
                "hardware_monitoring_record",
            ],
        )?;
        let c: Self = serde_json::from_value(value).map_err(|e| e.to_string())?;
        c.validate()?;
        Ok(c)
    }
    pub fn review(&self) -> Result<String, String> {
        self.validate()?;
        serde_json::to_string_pretty(self).map_err(|e| e.to_string())
    }
}
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Observation {
    pub brain_epoch: u64,
    pub brain_map: u64,
    pub frame: u64,
    pub config: Config,
    pub status: serde_json::Value,
    pub ticket: Option<u64>,
    pub success: Option<bool>,
    pub error: Option<String>,
}
impl Observation {
    pub fn validate(&self) -> Result<(), String> {
        self.config.validate()?;
        validate_status(&self.status)?;
        if self.brain_epoch == 0
            || self.brain_map == 0
            || !self.status.is_object()
            || self.ticket.is_some() != self.success.is_some()
            || self.success == Some(true) && self.error.is_some()
            || self.error.as_ref().is_some_and(|e| e.len() > 1024)
        {
            return Err("device observation domain".into());
        }
        Ok(())
    }
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Snapshot {
    pub contract: String,
    pub version: u8,
    pub state: String,
    pub observation: Option<Observation>,
    pub observed_ms: Option<u64>,
    pub connected: bool,
    pub pending: Option<u64>,
    pub bridge: Option<serde_json::Value>,
}
impl Snapshot {
    pub fn identity(&self) -> Option<(u64, u64)> {
        self.observation
            .as_ref()
            .filter(|_| self.connected)
            .map(|o| (o.brain_epoch, o.brain_map))
    }
}
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Reply {
    pub contract: String,
    pub version: u8,
    pub state: String,
    pub reason: Option<String>,
    pub ticket: u64,
    pub context: crate::audio::Context,
    pub revision: String,
    pub stagebox_frame: u64,
    pub observation: Option<Observation>,
}
pub enum Message {
    Snapshot(Snapshot),
    Reply(Reply),
}
pub fn decode(bytes: &[u8]) -> Result<Message, String> {
    let v = crate::provider::parse_document(bytes)?;
    if v["contract"] != "GP15-device" || v["version"] != 1 {
        return Err("device relay version".into());
    }
    if let Some(o) = v.get("observation").filter(|o| !o.is_null()) {
        crate::provider::keys(
            o,
            &[
                "brain_epoch",
                "brain_map",
                "frame",
                "config",
                "status",
                "ticket",
                "success",
                "error",
            ],
        )?;
        Config::decode(o["config"].clone())?;
    }
    if v["state"] == "snapshot" {
        crate::provider::keys(
            &v,
            &[
                "contract",
                "version",
                "state",
                "observation",
                "observed_ms",
                "connected",
                "pending",
                "bridge",
            ],
        )?;
        let s: Snapshot = serde_json::from_value(v).map_err(|e| e.to_string())?;
        if let Some(o) = &s.observation {
            o.validate()?;
        }
        if let Some(b) = &s.bridge {
            validate_bridge(b)?;
        }
        if s.observation.is_some() != s.observed_ms.is_some()
            || s.bridge.as_ref().is_some_and(|v| !v.is_object())
        {
            return Err("device observation binding".into());
        }
        Ok(Message::Snapshot(s))
    } else {
        crate::provider::keys(
            &v,
            &[
                "contract",
                "version",
                "state",
                "reason",
                "ticket",
                "context",
                "revision",
                "stagebox_frame",
                "observation",
            ],
        )?;
        crate::audio::validate_context_value(&v["context"])?;
        let r: Reply = serde_json::from_value(v).map_err(|e| e.to_string())?;
        r.context.validate()?;
        crate::provider::counter(&r.revision)?;
        if r.ticket == 0
            || !matches!(
                r.state.as_str(),
                "pending" | "accepted_intent" | "applied_device" | "failed_device"
            )
            || r.context.writer.is_none()
            || r.context.lease.is_none()
            || r.context.request_id.is_none()
            || r.context.expected_revision.is_none()
        {
            return Err("device reply authority/state".into());
        }
        if let Some(o) = &r.observation {
            o.validate()?;
        }
        if r.state == "applied_device"
            && r.observation
                .as_ref()
                .is_none_or(|o| o.ticket != Some(r.ticket) || o.success != Some(true))
        {
            return Err("device applied observation ticket".into());
        }
        if r.state != "failed_device" && r.reason.is_some() {
            return Err("device unexpected refusal".into());
        }
        Ok(Message::Reply(r))
    }
}

fn unsigned(v: &serde_json::Value, fields: &[&str]) -> Result<(), String> {
    for f in fields {
        v[*f]
            .as_u64()
            .ok_or_else(|| format!("device integer {f}"))?;
    }
    Ok(())
}
fn boolean(v: &serde_json::Value, fields: &[&str]) -> Result<(), String> {
    for f in fields {
        v[*f]
            .as_bool()
            .ok_or_else(|| format!("device boolean {f}"))?;
    }
    Ok(())
}
fn optional_integer(v: &serde_json::Value, fields: &[&str]) -> Result<(), String> {
    for f in fields {
        if !v[*f].is_null() {
            v[*f]
                .as_u64()
                .ok_or_else(|| format!("device optional integer {f}"))?;
        }
    }
    Ok(())
}
fn fault(v: &serde_json::Value) -> Result<(), String> {
    if !v.is_null() && v.as_str().is_none_or(|s| s.is_empty() || s.len() > 128) {
        return Err("device fault enum".into());
    }
    Ok(())
}
fn validate_bridge(v: &serde_json::Value) -> Result<(), String> {
    crate::provider::keys(
        v,
        &[
            "armed",
            "destination_frame",
            "epochs",
            "fault",
            "filter_delay_output_frames",
            "filter_latency_nominal_us",
            "occupancy_frames",
            "overflows",
            "physical_clock_lock_verified",
            "physical_mapping_uncertainty_milliframes",
            "queue_latency_nominal_us",
            "ratio_ppb",
            "ready",
            "rejected",
            "skew_ppb",
            "source_frame",
            "target_frames",
            "underruns",
        ],
    )?;
    boolean(v, &["armed", "ready", "physical_clock_lock_verified"])?;
    unsigned(
        v,
        &[
            "filter_delay_output_frames",
            "filter_latency_nominal_us",
            "occupancy_frames",
            "overflows",
            "queue_latency_nominal_us",
            "rejected",
            "target_frames",
            "underruns",
        ],
    )?;
    optional_integer(
        v,
        &[
            "destination_frame",
            "source_frame",
            "physical_mapping_uncertainty_milliframes",
        ],
    )?;
    v["ratio_ppb"]
        .as_i64()
        .filter(|n| *n > 0)
        .ok_or("bridge ratio ppb")?;
    v["skew_ppb"].as_i64().ok_or("bridge skew ppb")?;
    fault(&v["fault"])?;
    if !v["epochs"].is_null() {
        crate::provider::keys(&v["epochs"], &["source", "destination", "route"])?;
        unsigned(&v["epochs"], &["source", "destination", "route"])?;
    }
    Ok(())
}
fn validate_status(v: &serde_json::Value) -> Result<(), String> {
    let mut fields = vec![
        "armed",
        "capabilities",
        "capture_frame",
        "capture_stalls",
        "captured_peak_nano",
        "fault",
        "faults",
        "monitor_bridge",
        "outgoing_peak_nano",
        "physical_mapping_verified",
        "playback_frame",
        "playback_peak_nano",
        "playback_stalls",
        "readback",
    ];
    if v.get("capture_queue_dropped").is_some() {
        fields.push("capture_queue_dropped");
    }
    crate::provider::keys(v, &fields)?;
    boolean(v, &["armed", "physical_mapping_verified"])?;
    unsigned(
        v,
        &[
            "capture_frame",
            "capture_stalls",
            "captured_peak_nano",
            "faults",
            "outgoing_peak_nano",
            "playback_frame",
            "playback_peak_nano",
            "playback_stalls",
        ],
    )?;
    // Additive v1 telemetry: older producers omit this counter, but present values
    // must be unsigned integers (including zero), never null or coerced numbers.
    if v.get("capture_queue_dropped").is_some() {
        unsigned(v, &["capture_queue_dropped"])?;
    }
    fault(&v["fault"])?;
    validate_bridge(&v["monitor_bridge"])?;
    let c = &v["capabilities"];
    crate::provider::keys(
        c,
        &[
            "buffer_frames",
            "capture_channels",
            "device_id",
            "endpoint",
            "format",
            "period_frames",
            "physical",
            "playback_channels",
            "sample_rate",
            "shared_duplex_clock",
        ],
    )?;
    unsigned(
        c,
        &[
            "buffer_frames",
            "capture_channels",
            "period_frames",
            "playback_channels",
            "sample_rate",
        ],
    )?;
    boolean(c, &["physical", "shared_duplex_clock"])?;
    for f in ["device_id", "endpoint"] {
        if c[f].as_str().is_none_or(|s| s.is_empty() || s.len() > 256) {
            return Err("device capability identity".into());
        }
    }
    serde_json::from_value::<SampleFormat>(c["format"].clone()).map_err(|e| e.to_string())?;
    if !v["readback"].is_null() {
        crate::provider::keys(&v["readback"], &["configuration_generation", "epoch"])?;
        unsigned(&v["readback"], &["configuration_generation", "epoch"])?;
    }
    Ok(())
}
