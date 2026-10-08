//! Strict accepted GP18-sends:1 consumer. Levels remain separate C-AUDIO commands.
use crate::{audio::Context, provider};
use serde::{Deserialize, Serialize};
use serde_json::Value;
pub const CONTRACT: &str = "GP18-sends";
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Tap {
    RawPostMute,
    ProcessedPreFader,
    ProcessedPostFader,
}
pub const MODES: [Tap; 3] = [
    Tap::RawPostMute,
    Tap::ProcessedPreFader,
    Tap::ProcessedPostFader,
];
pub const POSITIONS: [&str; 3] = [
    "raw-shared-mute-mono",
    "eq-compressor-shared-mute-mono",
    "eq-compressor-shared-mute-fader-mono",
];
impl Tap {
    pub fn name(self) -> &'static str {
        match self {
            Self::RawPostMute => "raw_post_mute",
            Self::ProcessedPreFader => "processed_pre_fader",
            Self::ProcessedPostFader => "processed_post_fader",
        }
    }
}
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Send {
    pub monitor: String,
    pub current: Tap,
    pub target: Tap,
    pub transition_remaining_frames: u16,
    pub ready: bool,
}
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Channel {
    pub input: String,
    pub sends: Vec<Send>,
}
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Snapshot {
    pub show_id: String,
    pub epoch: String,
    pub revision: String,
    pub sequence: String,
    pub frame: String,
    pub sample_rate: u32,
    pub supported_modes: Vec<Tap>,
    pub tap_positions: Vec<String>,
    pub monitors: Vec<String>,
    pub faulted: bool,
    pub channels: Vec<Channel>,
}
impl Snapshot {
    pub fn validate(&self) -> Result<(), String> {
        if !provider::uuid(&self.show_id)
            || self.epoch == "0"
            || self.sequence == "0"
            || self.sample_rate != 48000
            || self.supported_modes != MODES
            || self.tap_positions != POSITIONS
            || self.channels.is_empty()
            || self.channels.len() > u16::MAX as usize
            || self.monitors.len() > u16::MAX as usize
        {
            return Err("sends identity/inventory/modes".into());
        }
        for n in [&self.epoch, &self.revision, &self.sequence, &self.frame] {
            provider::counter(n)?;
        }
        for (i, m) in self.monitors.iter().enumerate() {
            if *m != format!("monitor-{}", i + 1) {
                return Err("sends monitor inventory".into());
            }
        }
        let mut active = 0;
        for (i, c) in self.channels.iter().enumerate() {
            if c.input != format!("input-{:02}", i + 1) || c.sends.len() != self.monitors.len() {
                return Err("sends channel inventory".into());
            }
            for (s, m) in c.sends.iter().zip(&self.monitors) {
                if &s.monitor != m
                    || s.transition_remaining_frames > 240
                    || s.ready != (s.transition_remaining_frames == 0)
                    || s.ready && s.current != s.target
                    || !s.ready && s.current == s.target
                {
                    return Err("sends transition coherence".into());
                }
                if !s.ready
                    && provider::counter(&self.frame)?
                        .checked_add(u64::from(s.transition_remaining_frames))
                        .is_none_or(|end| end % 48 != 0)
                {
                    return Err("sends fade endpoint alignment/overflow".into());
                }
                active += usize::from(!s.ready);
            }
        }
        if active > 1 {
            return Err("multiple send fades".into());
        }
        Ok(())
    }
    pub fn send(&self, input: &str, monitor: &str) -> Option<&Send> {
        self.channels
            .iter()
            .find(|c| c.input == input)?
            .sends
            .iter()
            .find(|s| s.monitor == monitor)
    }
}
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Reply {
    pub contract: String,
    pub version: u8,
    pub context: Context,
    pub state: String,
    pub reason: Option<String>,
    pub ticket: Option<String>,
    pub effective_frame: Option<String>,
    pub ramp_frames: Option<u16>,
    pub revision: String,
    pub snapshot: Option<Snapshot>,
}
pub fn validate_body(body: &Value) -> Result<(), String> {
    provider::keys(body, &["input", "monitor", "tap"])?;
    provider::target_version(
        &provider::Target {
            input: body["input"].as_str().ok_or("send input")?.into(),
            parameter: "send".into(),
            monitor: Some(body["monitor"].as_str().ok_or("send monitor")?.into()),
        },
        2,
    )?;
    serde_json::from_value::<Tap>(body["tap"].clone()).map_err(|e| e.to_string())?;
    Ok(())
}
pub fn decode_reply(bytes: &[u8]) -> Result<Reply, String> {
    let v = provider::parse_document(bytes)?;
    provider::keys(
        &v,
        &[
            "contract",
            "version",
            "context",
            "state",
            "reason",
            "ticket",
            "effective_frame",
            "ramp_frames",
            "revision",
            "snapshot",
        ],
    )?;
    crate::audio::validate_context_value(&v["context"])?;
    let r: Reply = serde_json::from_value(v).map_err(|e| e.to_string())?;
    r.context.validate()?;
    if r.contract != CONTRACT || r.version != 1 || r.context.epoch == "0" {
        return Err("sends contract/version".into());
    }
    let revision = provider::counter(&r.revision)?;
    for n in [&r.ticket, &r.effective_frame].into_iter().flatten() {
        provider::counter(n)?;
    }
    let mutation = r.context.writer.is_some();
    let timing = r.ticket.as_deref().is_some_and(|t| t != "0")
        && r.effective_frame
            .as_deref()
            .is_some_and(|f| provider::counter(f).is_ok_and(|n| n > 0 && n % 48 == 0))
        && r.ramp_frames == Some(240);
    let absent = r.ticket.is_none() && r.effective_frame.is_none() && r.ramp_frames.is_none();
    let valid = match r.state.as_str() {
        "pending" => mutation && timing && r.reason.is_none() && r.snapshot.is_none(),
        "backpressure" => mutation && absent && r.reason.is_none() && r.snapshot.is_none(),
        "final" if r.reason.is_some() => {
            absent
                && r.snapshot.is_none()
                && r.reason.as_deref().is_some_and(|s| {
                    !s.is_empty()
                        && s.len() <= 64
                        && s.bytes().all(|b| b.is_ascii_lowercase() || b == b'_')
                })
        }
        "final" => r.snapshot.is_some() && if mutation { timing } else { absent },
        _ => false,
    };
    if !valid || mutation && r.context.lease.is_none() {
        return Err("sends state/timing/context".into());
    }
    if let Some(s) = &r.snapshot {
        s.validate()?;
        if s.show_id != r.context.show_id || s.epoch != r.context.epoch || s.revision != r.revision
        {
            return Err("sends snapshot binding".into());
        }
        if let Some(frame) = &r.effective_frame {
            let frame = provider::counter(frame)?;
            let observed = provider::counter(&s.frame)?;
            if observed <= frame {
                return Err("sends unconsumed boundary".into());
            }
            for channel in &s.channels {
                for send in &channel.sends {
                    if !send.ready
                        && u64::from(send.transition_remaining_frames)
                            != 240u64.saturating_sub(observed - frame)
                    {
                        return Err("sends fade endpoint".into());
                    }
                }
            }
        }
    }
    if mutation && r.reason.is_none() {
        let expected = provider::counter(
            r.context
                .expected_revision
                .as_deref()
                .ok_or("sends expected revision")?,
        )?;
        if r.state == "pending" && revision != expected
            || r.state == "final" && Some(revision) != expected.checked_add(1)
        {
            return Err("sends commit revision".into());
        }
    }
    Ok(r)
}
