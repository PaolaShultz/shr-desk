//! Explicit GP15 Brain consumer. No audio device, DSP, or inferred physical identity.
use crate::{audio::Context, provider};
use serde::{Deserialize, Serialize};
use serde_json::Value;

pub const HEARTBEAT_MS: u64 = 50;
pub const DEADMAN_MS: u64 = 150;
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
pub enum Source {
    #[default]
    None,
    Main,
    Monitor {
        index: usize,
    },
    Pfl {
        input: usize,
    },
    Afl {
        input: usize,
    },
}
impl Source {
    pub fn validate(self, inputs: usize, monitors: usize) -> Result<(), String> {
        if matches!(self, Self::Monitor {index} if index >= monitors)
            || matches!(self, Self::Pfl {input} | Self::Afl {input} if input >= inputs)
        {
            return Err("Brain source outside actual topology".into());
        }
        Ok(())
    }
}
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Snapshot {
    pub source: Source,
    pub selection_generation: String,
    pub monitor_gain_cdb: i32,
    pub monitor_mute: bool,
    pub monitor_dim: bool,
    pub monitor_armed: bool,
    pub talkback_monitors: Vec<usize>,
    pub talkback_foh: bool,
    pub talkback_gain_cdb: i32,
    pub talkback_mute: bool,
    pub hold_generation_counter: String,
    pub held_generation: Option<String>,
    pub hold_deadline_ms: Option<String>,
    pub audible_path_ready: bool,
    pub talkback_path_ready: bool,
    pub monitor_path_ready: bool,
    pub microphone_peak_nano: u64,
    pub outgoing_peak_nano: u64,
    pub monitor_peak_nano: u64,
    pub frame: String,
    pub revision: String,
    pub heartbeat_ms: u64,
    pub deadman_ms: u64,
    pub fade_frames: u64,
}
impl Snapshot {
    pub fn validate(&self, inputs: usize, monitors: usize) -> Result<(), String> {
        self.source.validate(inputs, monitors)?;
        gain(self.monitor_gain_cdb)?;
        gain(self.talkback_gain_cdb)?;
        destinations(&self.talkback_monitors, monitors)?;
        for n in [
            &self.selection_generation,
            &self.hold_generation_counter,
            &self.frame,
            &self.revision,
        ] {
            provider::counter(n)?;
        }
        for n in [&self.held_generation, &self.hold_deadline_ms]
            .into_iter()
            .flatten()
        {
            provider::counter(n)?;
        }
        if self.held_generation.is_some() != self.hold_deadline_ms.is_some()
            || self.held_generation.as_deref() == Some("0")
            || self.held_generation.as_ref().is_some_and(|g| {
                provider::counter(g).unwrap_or(u64::MAX)
                    > provider::counter(&self.hold_generation_counter).unwrap_or(0)
            })
            || self.heartbeat_ms != HEARTBEAT_MS
            || self.deadman_ms != DEADMAN_MS
            || self.fade_frames != 240
        {
            return Err("Brain timing/generation/sample meter domain".into());
        }
        Ok(())
    }
}
fn gain(n: i32) -> Result<(), String> {
    if (-9000..=0).contains(&n) {
        Ok(())
    } else {
        Err("Brain gain requires -9000..0 cdb".into())
    }
}
fn destinations(v: &[usize], count: usize) -> Result<(), String> {
    if v.len() > 4096
        || v.iter()
            .enumerate()
            .any(|(i, n)| *n >= count || v[..i].contains(n))
    {
        Err("Brain destinations outside topology or duplicated".into())
    } else {
        Ok(())
    }
}
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Reply {
    pub contract: String,
    pub version: u8,
    pub state: String,
    pub reason: Option<String>,
    pub context: Context,
    pub revision: String,
    pub applied_frame: Option<String>,
    pub snapshot: Option<Snapshot>,
}
pub fn decode_reply(bytes: &[u8]) -> Result<Reply, String> {
    let v = provider::parse_document(bytes)?;
    provider::keys(
        &v,
        &[
            "contract",
            "version",
            "state",
            "reason",
            "context",
            "revision",
            "applied_frame",
            "snapshot",
        ],
    )?;
    crate::audio::validate_context_value(&v["context"])?;
    // Require explicit nulls as well as all fields, rather than serde's optional defaults.
    if !v["snapshot"].is_null() {
        provider::keys(
            &v["snapshot"],
            &[
                "source",
                "selection_generation",
                "monitor_gain_cdb",
                "monitor_mute",
                "monitor_dim",
                "monitor_armed",
                "talkback_monitors",
                "talkback_foh",
                "talkback_gain_cdb",
                "talkback_mute",
                "hold_generation_counter",
                "held_generation",
                "hold_deadline_ms",
                "audible_path_ready",
                "talkback_path_ready",
                "monitor_path_ready",
                "microphone_peak_nano",
                "outgoing_peak_nano",
                "monitor_peak_nano",
                "frame",
                "revision",
                "heartbeat_ms",
                "deadman_ms",
                "fade_frames",
            ],
        )?;
    }
    let r: Reply = serde_json::from_value(v).map_err(|e| e.to_string())?;
    r.context.validate()?;
    provider::counter(&r.revision)?;
    if r.contract != "GP15-brain"
        || r.version != 1
        || r.context.epoch == "0"
        || !matches!(r.state.as_str(), "snapshot" | "pending" | "final")
    {
        return Err("Brain reply version/context/state".into());
    }
    if let Some(f) = &r.applied_frame {
        provider::counter(f)?;
    }
    if r.reason
        .as_ref()
        .is_some_and(|s| s.is_empty() || s.len() > 256)
    {
        return Err("Brain refusal".into());
    }
    if r.state == "snapshot"
        && (r.context.writer.is_some()
            || r.context.lease.is_some()
            || r.context.request_id.is_some()
            || r.context.expected_revision.is_some()
            || r.reason.is_some()
            || r.applied_frame.is_some()
            || r.snapshot.is_none())
    {
        return Err("Brain snapshot shape".into());
    }
    if r.state != "snapshot"
        && (r.context.writer.is_none()
            || r.context.lease.is_none()
            || r.context.request_id.is_none()
            || r.context.expected_revision.is_none())
    {
        return Err("Brain reply authority".into());
    }
    if r.state == "pending" && (r.reason.is_some() || r.snapshot.is_some()) {
        return Err("Brain pending shape".into());
    }
    if let Some(s) = &r.snapshot {
        s.validate(4096, 4096)?;
        if s.revision != r.revision {
            return Err("Brain snapshot revision binding".into());
        }
    }
    Ok(r)
}
// Internal prefix avoids collision with the C-AUDIO writer-release command.
pub fn is_kind(kind: &str) -> bool {
    matches!(
        kind,
        "brain_snapshot"
            | "brain_monitor_set"
            | "brain_talkback_set"
            | "brain_talkback_foh"
            | "brain_hold"
            | "brain_heartbeat"
            | "brain_release"
            | "brain_close"
    )
}
pub fn wire_kind(kind: &str) -> &str {
    if kind == "brain_snapshot" {
        kind
    } else {
        kind.strip_prefix("brain_").unwrap_or(kind)
    }
}
pub fn scope(kind: &str) -> Option<&'static str> {
    match kind {
        "brain_snapshot" => None,
        "brain_monitor_set" => Some("local_operator_monitor"),
        "brain_talkback_foh" => Some("talkback_foh"),
        _ if is_kind(kind) => Some("talkback_destinations"),
        _ => None,
    }
}
pub fn validate_body(
    kind: &str,
    body: &Value,
    snapshot: Option<&Snapshot>,
    inputs: usize,
    monitors: usize,
) -> Result<(), String> {
    match kind {
        "brain_snapshot" | "brain_close" => provider::keys(body, &[]),
        "brain_hold" | "brain_release" => {
            provider::keys(body, &["generation"])?;
            if provider::counter(
                body["generation"]
                    .as_str()
                    .ok_or("Brain generation string")?,
            )? == 0
            {
                return Err("Brain generation zero".into());
            }
            Ok(())
        }
        "brain_heartbeat" => {
            provider::keys(body, &["generation", "observed_frame"])?;
            if provider::counter(body["generation"].as_str().ok_or("generation string")?)? == 0 {
                return Err("generation zero".into());
            }
            provider::counter(
                body["observed_frame"]
                    .as_str()
                    .ok_or("observed frame string")?,
            )?;
            Ok(())
        }
        "brain_talkback_foh" => {
            provider::keys(body, &["enabled"])?;
            body["enabled"].as_bool().ok_or("Brain FOH boolean")?;
            Ok(())
        }
        "brain_talkback_set" => {
            provider::keys(body, &["monitors", "gain_cdb", "mute"])?;
            let selected: Vec<usize> =
                serde_json::from_value(body["monitors"].clone()).map_err(|e| e.to_string())?;
            destinations(&selected, monitors)?;
            validate_gain_body(body)?;
            Ok(())
        }
        "brain_monitor_set" => {
            provider::keys(body, &["source", "gain_cdb", "mute", "dim", "armed"])?;
            let source: Source =
                serde_json::from_value(body["source"].clone()).map_err(|e| e.to_string())?;
            source.validate(inputs, monitors)?;
            validate_gain_body(body)?;
            body["dim"].as_bool().ok_or("Brain dim boolean")?;
            let armed = body["armed"].as_bool().ok_or("Brain armed boolean")?;
            if armed && snapshot.is_some_and(|s| s.source != source) {
                return Err("source change disarms; separate confirmed-source arm required".into());
            }
            Ok(())
        }
        _ => Err("unknown Brain command".into()),
    }
}
fn validate_gain_body(body: &Value) -> Result<(), String> {
    gain(
        i32::try_from(body["gain_cdb"].as_i64().ok_or("Brain gain integer")?)
            .map_err(|_| "Brain gain range")?,
    )?;
    body["mute"].as_bool().ok_or("Brain mute boolean")?;
    Ok(())
}

/// UI liveness is separate from provider authority. A stalled UI cannot renew PTT.
#[derive(Default)]
pub(crate) struct HoldSignal {
    deadline: std::sync::Mutex<Option<std::time::Instant>>,
    pub close: std::sync::atomic::AtomicBool,
    serial: std::sync::atomic::AtomicU64,
}
impl HoldSignal {
    pub fn press(&self) -> u64 {
        let serial = self
            .serial
            .fetch_update(
                std::sync::atomic::Ordering::AcqRel,
                std::sync::atomic::Ordering::Acquire,
                |n| n.checked_add(1),
            )
            .expect("PTT serial exhausted")
            + 1;
        *self.deadline.lock().unwrap() =
            Some(std::time::Instant::now() + std::time::Duration::from_millis(100));
        serial
    }
    pub fn live_id(&self, id: u64) -> bool {
        self.serial.load(std::sync::atomic::Ordering::Acquire) == id && self.live()
    }
    pub fn pulse(&self) {
        let mut d = self.deadline.lock().unwrap();
        if d.is_some_and(|t| std::time::Instant::now() < t) {
            *d = Some(std::time::Instant::now() + std::time::Duration::from_millis(100));
        }
    }
    pub fn live(&self) -> bool {
        self.deadline
            .lock()
            .unwrap()
            .is_some_and(|t| std::time::Instant::now() < t)
    }
    pub fn release(&self) {
        let _ = self.serial.fetch_update(
            std::sync::atomic::Ordering::AcqRel,
            std::sync::atomic::Ordering::Acquire,
            |n| n.checked_add(1),
        );
        let was_held = self.deadline.lock().unwrap().take().is_some();
        if was_held {
            self.close.store(true, std::sync::atomic::Ordering::Release);
        }
    }
}

/// Complete-message injected controller edge decoder; never opens a MIDI endpoint.
#[derive(Clone, Debug)]
pub struct HoldMidi {
    channel: u8,
    note: u8,
    down: bool,
    blocked: bool,
}
impl HoldMidi {
    pub fn new(channel: u8, note: u8) -> Result<Self, String> {
        if channel >= 16 || note >= 128 {
            return Err("PTT MIDI channel/note".into());
        }
        Ok(Self {
            channel,
            note,
            down: false,
            blocked: true,
        })
    }
    pub fn fence(&mut self) {
        self.down = false;
        self.blocked = true;
    }
    pub fn decode(&mut self, bytes: &[u8]) -> Option<bool> {
        if bytes.len() != 3
            || bytes[0] & 15 != self.channel
            || bytes[1] != self.note
            || bytes[1] >= 128
            || bytes[2] >= 128
        {
            return None;
        }
        let kind = bytes[0] & 0xf0;
        if kind == 0x80 || kind == 0x90 && bytes[2] == 0 {
            self.down = false;
            self.blocked = false;
            return Some(false);
        }
        if kind == 0x90 && !self.down && !self.blocked {
            self.down = true;
            return Some(true);
        }
        None
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::{
        sync::atomic::Ordering,
        time::{Duration, Instant},
    };
    #[test]
    fn expired_ui_liveness_cannot_be_resurrected_by_pulse_or_pending_work() {
        let signal = HoldSignal::default();
        signal.press();
        assert!(signal.live());
        *signal.deadline.lock().unwrap() = Some(Instant::now() - Duration::from_millis(1));
        signal.pulse();
        assert!(!signal.live());
        signal.release();
        assert!(signal.close.load(Ordering::Acquire));
        signal.pulse();
        assert!(!signal.live());
        signal.press();
        assert!(signal.live());
    }
}
