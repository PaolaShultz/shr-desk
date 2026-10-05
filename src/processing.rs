//! Strict GP07-processing:2 consumer data. No filters, detector or DSP algorithms.
use crate::{audio::Context, provider};
use serde::{Deserialize, Serialize};
use serde_json::Value;
#[derive(Clone, Debug, PartialEq, Eq, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct Config {
    pub eq_bypass: bool,
    pub band1_hz: i32,
    pub band1_gain_mdb: i32,
    pub band1_q_milli: i32,
    pub band1_bypass: bool,
    pub band2_hz: i32,
    pub band2_gain_mdb: i32,
    pub band2_q_milli: i32,
    pub band2_bypass: bool,
    pub band3_hz: i32,
    pub band3_gain_mdb: i32,
    pub band3_q_milli: i32,
    pub band3_bypass: bool,
    pub band4_hz: i32,
    pub band4_gain_mdb: i32,
    pub band4_q_milli: i32,
    pub band4_bypass: bool,
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
            (self.band1_hz, 20, 20000, 1),
            (self.band1_gain_mdb, -12000, 12000, 100),
            (self.band1_q_milli, 100, 10000, 100),
            (self.band2_hz, 20, 20000, 1),
            (self.band2_gain_mdb, -12000, 12000, 100),
            (self.band2_q_milli, 100, 10000, 100),
            (self.band3_hz, 20, 20000, 1),
            (self.band3_gain_mdb, -12000, 12000, 100),
            (self.band3_q_milli, 100, 10000, 100),
            (self.band4_hz, 20, 20000, 1),
            (self.band4_gain_mdb, -12000, 12000, 100),
            (self.band4_q_milli, 100, 10000, 100),
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
            Field::Band1Hz => (&mut candidate.band1_hz, 1),
            Field::Band1Gain => (&mut candidate.band1_gain_mdb, 100),
            Field::Band1Q => (&mut candidate.band1_q_milli, 100),
            Field::Band1Bypass => {
                candidate.band1_bypass = !candidate.band1_bypass;
                *self = candidate;
                return Ok(());
            }
            Field::Band2Hz => (&mut candidate.band2_hz, 1),
            Field::Band2Gain => (&mut candidate.band2_gain_mdb, 100),
            Field::Band2Q => (&mut candidate.band2_q_milli, 100),
            Field::Band2Bypass => {
                candidate.band2_bypass = !candidate.band2_bypass;
                *self = candidate;
                return Ok(());
            }
            Field::Band3Hz => (&mut candidate.band3_hz, 1),
            Field::Band3Gain => (&mut candidate.band3_gain_mdb, 100),
            Field::Band3Q => (&mut candidate.band3_q_milli, 100),
            Field::Band3Bypass => {
                candidate.band3_bypass = !candidate.band3_bypass;
                *self = candidate;
                return Ok(());
            }
            Field::Band4Hz => (&mut candidate.band4_hz, 1),
            Field::Band4Gain => (&mut candidate.band4_gain_mdb, 100),
            Field::Band4Q => (&mut candidate.band4_q_milli, 100),
            Field::Band4Bypass => {
                candidate.band4_bypass = !candidate.band4_bypass;
                *self = candidate;
                return Ok(());
            }
            Field::CompressorBypass => {
                candidate.compressor_bypass = !candidate.compressor_bypass;
                *self = candidate;
                return Ok(());
            }
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
                | Field::Band1Hz
                | Field::Band1Bypass
                | Field::Band2Hz
                | Field::Band2Bypass
                | Field::Band3Hz
                | Field::Band3Bypass
                | Field::Band4Hz
                | Field::Band4Bypass
                | Field::CompressorBypass
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
            Field::EqBypass
            | Field::Band1Bypass
            | Field::Band2Bypass
            | Field::Band3Bypass
            | Field::Band4Bypass
            | Field::CompressorBypass
                if !(0..=1).contains(&value) =>
            {
                return Err("bypass is 0 (enabled) or 1 (bypassed)".into());
            }
            Field::EqBypass => c.eq_bypass = value == 1,
            Field::Band1Hz => c.band1_hz = value,
            Field::Band1Gain => c.band1_gain_mdb = value,
            Field::Band1Q => c.band1_q_milli = value,
            Field::Band1Bypass => c.band1_bypass = value == 1,
            Field::Band2Hz => c.band2_hz = value,
            Field::Band2Gain => c.band2_gain_mdb = value,
            Field::Band2Q => c.band2_q_milli = value,
            Field::Band2Bypass => c.band2_bypass = value == 1,
            Field::Band3Hz => c.band3_hz = value,
            Field::Band3Gain => c.band3_gain_mdb = value,
            Field::Band3Q => c.band3_q_milli = value,
            Field::Band3Bypass => c.band3_bypass = value == 1,
            Field::Band4Hz => c.band4_hz = value,
            Field::Band4Gain => c.band4_gain_mdb = value,
            Field::Band4Q => c.band4_q_milli = value,
            Field::Band4Bypass => c.band4_bypass = value == 1,
            Field::CompressorBypass => c.compressor_bypass = value == 1,
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
    /// Stable band identity order, never frequency-sorted.
    pub fn bands(&self) -> [(i32, i32, i32, bool); 4] {
        [
            (
                self.band1_hz,
                self.band1_gain_mdb,
                self.band1_q_milli,
                self.band1_bypass,
            ),
            (
                self.band2_hz,
                self.band2_gain_mdb,
                self.band2_q_milli,
                self.band2_bypass,
            ),
            (
                self.band3_hz,
                self.band3_gain_mdb,
                self.band3_q_milli,
                self.band3_bypass,
            ),
            (
                self.band4_hz,
                self.band4_gain_mdb,
                self.band4_q_milli,
                self.band4_bypass,
            ),
        ]
    }
    pub fn display(&self, field: Field) -> String {
        match field {
            Field::EqBypass => format!("EQ bypass {}", self.eq_bypass),
            Field::Band1Hz => format!("Band 1 bell {} Hz", self.band1_hz),
            Field::Band1Gain => {
                format!("Band 1 gain {:+.1} dB", self.band1_gain_mdb as f64 / 1000.0)
            }
            Field::Band1Q => format!("Band 1 Q {:.1}", self.band1_q_milli as f64 / 1000.0),
            Field::Band1Bypass => format!("Band 1 bypass {}", self.band1_bypass),
            Field::Band2Hz => format!("Band 2 bell {} Hz", self.band2_hz),
            Field::Band2Gain => {
                format!("Band 2 gain {:+.1} dB", self.band2_gain_mdb as f64 / 1000.0)
            }
            Field::Band2Q => format!("Band 2 Q {:.1}", self.band2_q_milli as f64 / 1000.0),
            Field::Band2Bypass => format!("Band 2 bypass {}", self.band2_bypass),
            Field::Band3Hz => format!("Band 3 bell {} Hz", self.band3_hz),
            Field::Band3Gain => {
                format!("Band 3 gain {:+.1} dB", self.band3_gain_mdb as f64 / 1000.0)
            }
            Field::Band3Q => format!("Band 3 Q {:.1}", self.band3_q_milli as f64 / 1000.0),
            Field::Band3Bypass => format!("Band 3 bypass {}", self.band3_bypass),
            Field::Band4Hz => format!("Band 4 bell {} Hz", self.band4_hz),
            Field::Band4Gain => {
                format!("Band 4 gain {:+.1} dB", self.band4_gain_mdb as f64 / 1000.0)
            }
            Field::Band4Q => format!("Band 4 Q {:.1}", self.band4_q_milli as f64 / 1000.0),
            Field::Band4Bypass => format!("Band 4 bypass {}", self.band4_bypass),
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
    Band1Hz,
    Band1Gain,
    Band1Q,
    Band1Bypass,
    Band2Hz,
    Band2Gain,
    Band2Q,
    Band2Bypass,
    Band3Hz,
    Band3Gain,
    Band3Q,
    Band3Bypass,
    Band4Hz,
    Band4Gain,
    Band4Q,
    Band4Bypass,
    CompressorBypass,
    Threshold,
    Ratio,
    Knee,
    Attack,
    Release,
    Makeup,
}
pub const FIELDS: [Field; 24] = [
    Field::EqBypass,
    Field::Band1Hz,
    Field::Band1Gain,
    Field::Band1Q,
    Field::Band1Bypass,
    Field::Band2Hz,
    Field::Band2Gain,
    Field::Band2Q,
    Field::Band2Bypass,
    Field::Band3Hz,
    Field::Band3Gain,
    Field::Band3Q,
    Field::Band3Bypass,
    Field::Band4Hz,
    Field::Band4Gain,
    Field::Band4Q,
    Field::Band4Bypass,
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
            || self.foh_tap != "foh-post-eq-dynamics-v2"
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
    if r.contract != "GP07-processing" || r.version != 2 {
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
