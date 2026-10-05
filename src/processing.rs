//! Strict GP07-processing:1 consumer data. No filters, detector or DSP algorithms.
use crate::{audio::Context, provider};
use serde::{Deserialize, Serialize};
use serde_json::Value;

#[derive(Clone, Debug, PartialEq, Eq, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct Config {
    pub eq_bypass: bool,
    pub low_hz: i32,
    pub low_gain_mdb: i32,
    pub mid_hz: i32,
    pub mid_gain_mdb: i32,
    pub mid_q_milli: i32,
    pub high_hz: i32,
    pub high_gain_mdb: i32,
    pub compressor_bypass: bool,
    pub threshold_mdb: i32,
    pub ratio_milli: i32,
    pub knee_mdb: i32,
    pub attack_us: i32,
    pub release_ms: i32,
    pub makeup_mdb: i32,
}
impl Config {
    pub fn validate(&self) -> Result<(), String> {
        for (value, min, max, step) in [
            (self.low_hz, 20, 20000, 1),
            (self.mid_hz, 20, 20000, 1),
            (self.high_hz, 20, 20000, 1),
            (self.low_gain_mdb, -12000, 12000, 100),
            (self.mid_gain_mdb, -12000, 12000, 100),
            (self.high_gain_mdb, -12000, 12000, 100),
            (self.mid_q_milli, 100, 10000, 100),
            (self.threshold_mdb, -60000, 0, 100),
            (self.ratio_milli, 1000, 20000, 100),
            (self.knee_mdb, 0, 18000, 100),
            (self.attack_us, 100, 200000, 100),
            (self.release_ms, 10, 2000, 1),
            (self.makeup_mdb, -12000, 12000, 100),
        ] {
            if !(min..=max).contains(&value) || value % step != 0 {
                return Err("processing parameter range/step".into());
            }
        }
        Ok(())
    }
    pub fn adjust(&mut self, field: Field, steps: i32) -> Result<(), String> {
        let mut candidate = self.clone();
        let (value, step) = match field {
            Field::EqBypass => {
                candidate.eq_bypass = !candidate.eq_bypass;
                *self = candidate;
                return Ok(());
            }
            Field::CompressorBypass => {
                candidate.compressor_bypass = !candidate.compressor_bypass;
                *self = candidate;
                return Ok(());
            }
            Field::LowHz => (&mut candidate.low_hz, 1),
            Field::LowGain => (&mut candidate.low_gain_mdb, 100),
            Field::MidHz => (&mut candidate.mid_hz, 1),
            Field::MidGain => (&mut candidate.mid_gain_mdb, 100),
            Field::MidQ => (&mut candidate.mid_q_milli, 100),
            Field::HighHz => (&mut candidate.high_hz, 1),
            Field::HighGain => (&mut candidate.high_gain_mdb, 100),
            Field::Threshold => (&mut candidate.threshold_mdb, 100),
            Field::Ratio => (&mut candidate.ratio_milli, 100),
            Field::Knee => (&mut candidate.knee_mdb, 100),
            Field::Attack => (&mut candidate.attack_us, 100),
            Field::Release => (&mut candidate.release_ms, 1),
            Field::Makeup => (&mut candidate.makeup_mdb, 100),
        };
        *value = steps
            .checked_mul(step)
            .and_then(|d| value.checked_add(d))
            .ok_or("processing edit overflow")?;
        candidate.validate()?;
        *self = candidate;
        Ok(())
    }
    /// Operator units: Hz, dB, Q, ratio and ms; bypass accepts 0/1.
    pub fn set_text(&mut self, field: Field, text: &str) -> Result<(), String> {
        if text.len() > 16 {
            return Err("numeric entry capacity".into());
        }
        let (negative, text) = text.strip_prefix('-').map_or((false, text), |v| (true, v));
        let (whole, fraction) = text.split_once('.').unwrap_or((text, ""));
        if whole.is_empty()
            || !whole.bytes().all(|b| b.is_ascii_digit())
            || fraction.len() > 3
            || !fraction.bytes().all(|b| b.is_ascii_digit())
        {
            return Err("enter a decimal number with at most three decimal places".into());
        }
        let scale = if matches!(
            field,
            Field::EqBypass
                | Field::CompressorBypass
                | Field::LowHz
                | Field::MidHz
                | Field::HighHz
                | Field::Release
        ) {
            1
        } else {
            1000
        };
        let whole: i64 = whole.parse().map_err(|_| "numeric entry overflow")?;
        let frac: i64 = if fraction.is_empty() {
            0
        } else {
            fraction.parse().map_err(|_| "numeric fraction")?
        };
        let denom = 10i64.pow(fraction.len() as u32);
        if frac * scale % denom != 0 {
            return Err("parameter requires integral units".into());
        }
        let value = whole
            .checked_mul(scale)
            .and_then(|n| n.checked_add(frac * scale / denom))
            .ok_or("numeric overflow")?;
        let value =
            i32::try_from(if negative { -value } else { value }).map_err(|_| "numeric overflow")?;
        let mut c = self.clone();
        match field {
            Field::EqBypass | Field::CompressorBypass if !(0..=1).contains(&value) => {
                return Err("bypass is 0 (enabled) or 1 (bypassed)".into());
            }
            Field::EqBypass => c.eq_bypass = value == 1,
            Field::CompressorBypass => c.compressor_bypass = value == 1,
            Field::LowHz => c.low_hz = value,
            Field::LowGain => c.low_gain_mdb = value,
            Field::MidHz => c.mid_hz = value,
            Field::MidGain => c.mid_gain_mdb = value,
            Field::MidQ => c.mid_q_milli = value,
            Field::HighHz => c.high_hz = value,
            Field::HighGain => c.high_gain_mdb = value,
            Field::Threshold => c.threshold_mdb = value,
            Field::Ratio => c.ratio_milli = value,
            Field::Knee => c.knee_mdb = value,
            Field::Attack => c.attack_us = value,
            Field::Release => c.release_ms = value,
            Field::Makeup => c.makeup_mdb = value,
        }
        c.validate()?;
        *self = c;
        Ok(())
    }
    pub fn display(&self, field: Field) -> String {
        match field {
            Field::EqBypass => format!("EQ bypass {}", self.eq_bypass),
            Field::LowHz => format!("Low shelf {} Hz", self.low_hz),
            Field::LowGain => format!("Low gain {:+.1} dB", self.low_gain_mdb as f64 / 1000.0),
            Field::MidHz => format!("Mid bell {} Hz", self.mid_hz),
            Field::MidGain => format!("Mid gain {:+.1} dB", self.mid_gain_mdb as f64 / 1000.0),
            Field::MidQ => format!("Mid Q {:.1}", self.mid_q_milli as f64 / 1000.0),
            Field::HighHz => format!("High shelf {} Hz", self.high_hz),
            Field::HighGain => format!("High gain {:+.1} dB", self.high_gain_mdb as f64 / 1000.0),
            Field::CompressorBypass => format!("Compressor bypass {}", self.compressor_bypass),
            Field::Threshold => format!("Threshold {:.1} dBFS", self.threshold_mdb as f64 / 1000.0),
            Field::Ratio => format!("Ratio {:.1}:1", self.ratio_milli as f64 / 1000.0),
            Field::Knee => format!("Knee {:.1} dB", self.knee_mdb as f64 / 1000.0),
            Field::Attack => format!("Attack {:.1} ms", self.attack_us as f64 / 1000.0),
            Field::Release => format!("Release {} ms", self.release_ms),
            Field::Makeup => format!("Makeup {:+.1} dB", self.makeup_mdb as f64 / 1000.0),
        }
    }
}
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Field {
    EqBypass,
    LowHz,
    LowGain,
    MidHz,
    MidGain,
    MidQ,
    HighHz,
    HighGain,
    CompressorBypass,
    Threshold,
    Ratio,
    Knee,
    Attack,
    Release,
    Makeup,
}
pub const FIELDS: [Field; 15] = [
    Field::EqBypass,
    Field::LowHz,
    Field::LowGain,
    Field::MidHz,
    Field::MidGain,
    Field::MidQ,
    Field::HighHz,
    Field::HighGain,
    Field::CompressorBypass,
    Field::Threshold,
    Field::Ratio,
    Field::Knee,
    Field::Attack,
    Field::Release,
    Field::Makeup,
];

#[derive(Clone, Debug, PartialEq, Eq, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct Channel {
    pub input: String,
    pub current: Config,
    pub target: Config,
    pub transition_remaining_frames: u16,
    pub ready: bool,
    pub gain_reduction_mdb: Option<u32>,
}
#[derive(Clone, Debug, PartialEq, Eq, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct Snapshot {
    pub show_id: String,
    pub epoch: String,
    pub revision: String,
    pub sequence: String,
    pub frame: String,
    pub sample_rate: u32,
    pub foh_tap: String,
    pub monitor_tap: String,
    pub faulted: bool,
    pub channels: Vec<Channel>,
}
impl Snapshot {
    pub fn validate(&self) -> Result<(), String> {
        if !provider::uuid(&self.show_id)
            || self.sample_rate != 48000
            || self.foh_tap != "foh-post-eq-dynamics-v1"
            || self.monitor_tap != "raw-post-mute-v1"
            || self.channels.len() != 8
        {
            return Err("processing identity/rate/taps/channels".into());
        }
        for c in [&self.epoch, &self.revision, &self.sequence, &self.frame] {
            provider::counter(c)?;
        }
        if self.epoch == "0" || self.sequence == "0" {
            return Err("processing epoch/sequence must be nonzero".into());
        }
        for (i, c) in self.channels.iter().enumerate() {
            c.current.validate()?;
            c.target.validate()?;
            if c.input != format!("input-{:02}", i + 1)
                || c.transition_remaining_frames > 240
                || c.ready != (c.transition_remaining_frames == 0)
                || c.ready && c.current != c.target
                || c.gain_reduction_mdb.is_some_and(|n| n > 240000)
                || c.gain_reduction_mdb.is_some()
                    != (c.ready && !c.target.compressor_bypass && !self.faulted)
            {
                return Err("processing channel readiness/GR coherence".into());
            }
        }
        Ok(())
    }
}
#[derive(Clone, Debug, Deserialize, Serialize)]
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
pub fn decode_config(value: &Value) -> Result<Config, String> {
    // Nonnullable fields plus deny_unknown_fields enforce the exact complete config.
    let c: Config = serde_json::from_value(value.clone()).map_err(|e| e.to_string())?;
    c.validate()?;
    Ok(c)
}
pub fn validate_body(body: &Value) -> Result<(), String> {
    provider::keys(body, &["input", "config"])?;
    if !(1..=8).any(|i| body["input"] == format!("input-{i:02}")) {
        return Err("processing input".into());
    }
    decode_config(&body["config"])?;
    Ok(())
}
pub fn decode_reply(bytes: &[u8]) -> Result<Reply, String> {
    let v = provider::parse(bytes)?;
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
    if !v["snapshot"].is_null() {
        for c in v["snapshot"]["channels"]
            .as_array()
            .ok_or("processing channels")?
        {
            provider::keys(
                c,
                &[
                    "input",
                    "current",
                    "target",
                    "transition_remaining_frames",
                    "ready",
                    "gain_reduction_mdb",
                ],
            )?;
        }
    }
    let r: Reply = serde_json::from_value(v).map_err(|e| e.to_string())?;
    r.context.validate()?;
    if r.context.epoch == "0" {
        return Err("processing epoch must be nonzero".into());
    }
    if r.contract != "GP07-processing" || r.version != 1 {
        return Err("processing contract/version".into());
    }
    provider::counter(&r.revision)?;
    for n in [&r.ticket, &r.effective_frame].into_iter().flatten() {
        provider::counter(n)?;
    }
    let timing = r.ticket.as_deref().is_some_and(|t| t != "0")
        && r.effective_frame
            .as_deref()
            .is_some_and(|f| provider::counter(f).is_ok_and(|n| n > 0 && n % 48 == 0))
        && r.ramp_frames == Some(240);
    let no_timing = r.ticket.is_none() && r.effective_frame.is_none() && r.ramp_frames.is_none();
    let mutation = r.context.writer.is_some();
    if mutation && r.context.lease.is_none() {
        return Err("processing lease missing".into());
    }
    let valid = match r.state.as_str() {
        "pending" => mutation && timing && r.reason.is_none() && r.snapshot.is_none(),
        "backpressure" => mutation && no_timing && r.reason.is_none() && r.snapshot.is_none(),
        "final" if r.reason.is_some() => {
            no_timing
                && r.snapshot.is_none()
                && r.reason.as_deref().is_some_and(|s| {
                    !s.is_empty()
                        && s.len() <= 64
                        && s.bytes().all(|b| b.is_ascii_lowercase() || b == b'_')
                })
        }
        "final" => r.snapshot.is_some() && if mutation { timing } else { no_timing },
        _ => false,
    };
    if !valid {
        return Err("processing reply state/timing".into());
    }
    if let Some(s) = &r.snapshot {
        s.validate()?;
        if s.show_id != r.context.show_id
            || s.epoch != r.context.epoch
            || s.revision != r.revision
            || r.effective_frame.as_deref().is_some_and(|f| {
                provider::counter(&s.frame).unwrap() <= provider::counter(f).unwrap()
            })
        {
            return Err("processing snapshot binding".into());
        }
    }
    if mutation && r.reason.is_none() {
        let expected = provider::counter(
            r.context
                .expected_revision
                .as_deref()
                .ok_or("processing revision")?,
        )?;
        let revision = provider::counter(&r.revision)?;
        if (r.state == "pending" && revision != expected)
            || (r.state == "final" && Some(revision) != expected.checked_add(1))
        {
            return Err("processing commit revision".into());
        }
    }
    Ok(r)
}
