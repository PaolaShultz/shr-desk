//! Read-only PA measurement documents. Analysis and candidate construction remain
//! in SHR PA; these bounded descriptions never grant configuration authority.
pub mod editor;
pub mod wire;
use crate::provider;
use serde::{Deserialize, Serialize};
use serde_json::Value;

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Capture {
    pub id: String,
    pub source_epoch: String,
    pub map_revision: String,
    pub clock_domain: String,
    pub reference_id: String,
    pub reference_tap: String,
    pub reference_offset_frames: String,
    pub first_frame: String,
    pub sample_rate: u32,
    pub output_index: usize,
    pub position_id: String,
    pub configuration_revision: String,
    pub dropped_frames: String,
    pub clipped_reference: bool,
    pub clipped_mic: bool,
    pub timing_verified: bool,
}
pub fn identifier(value: &str) -> bool {
    !value.is_empty() && value.len() <= 128 && value.bytes().all(|b| b.is_ascii_graphic())
}
impl Capture {
    pub fn validate(&self) -> Result<(), String> {
        if [
            &self.id,
            &self.clock_domain,
            &self.reference_id,
            &self.reference_tap,
            &self.position_id,
        ]
        .iter()
        .any(|v| !identifier(v))
            || self.sample_rate != 48000
            || self.output_index >= 4096
        {
            return Err("PA capture identity".into());
        }
        for value in [
            &self.source_epoch,
            &self.map_revision,
            &self.configuration_revision,
        ] {
            if provider::counter(value)? == 0 {
                return Err("PA capture origin".into());
            }
        }
        for value in [
            &self.reference_offset_frames,
            &self.first_frame,
            &self.dropped_frames,
        ] {
            provider::counter(value)?;
        }
        if self.dropped_frames != "0"
            || self.clipped_reference
            || self.clipped_mic
            || !self.timing_verified
        {
            return Err("PA admitted capture quality".into());
        }
        Ok(())
    }
}
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Options {
    pub max_arrival_samples: usize,
    pub band_hz: [f64; 2],
}
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Bin {
    pub frequency_hz: f64,
    pub magnitude: f64,
    pub phase_radians: f64,
    pub coherence: f64,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Measurement {
    pub contract: String,
    pub version: u32,
    pub algorithm: String,
    pub capture: Capture,
    pub options: Options,
    pub samples: usize,
    pub arrival_samples: i32,
    pub signed_correlation: f64,
    pub competing_peak_ratio: f64,
    pub segments: usize,
    pub spectrum: Vec<Bin>,
    pub proposal_eligible: bool,
    pub reasons: Vec<String>,
}
impl Measurement {
    pub fn decode(value: &Value) -> Result<Self, String> {
        if serde_json::to_vec(value).map_err(|e| e.to_string())?.len() > 64 * 1024 {
            return Err("PA result document bound".into());
        }
        let result: Self = serde_json::from_value(value.clone()).map_err(|e| e.to_string())?;
        result.validate()?;
        Ok(result)
    }
    pub fn validate(&self) -> Result<(), String> {
        self.capture.validate()?;
        let [low, high] = self.options.band_hz;
        if self.contract != "C-PA-MEASUREMENT-RESULT"
            || self.version != 1
            || self.algorithm != "offline-h1-v1"
            || !(32768..=65536).contains(&self.samples)
            || !(8..=32).contains(&self.segments)
            || !(1..=2048).contains(&self.options.max_arrival_samples)
            || self.arrival_samples.unsigned_abs() as usize > self.options.max_arrival_samples
            || !low.is_finite()
            || !high.is_finite()
            || low < 20.
            || high > 20000.
            || low >= high
            || !self.signed_correlation.is_finite()
            || self.signed_correlation.abs() > 1.000001
            || !self.competing_peak_ratio.is_finite()
            || self.competing_peak_ratio < 0.
            || self.spectrum.len() > 64
            || self.reasons.len() > 16
            || self.reasons.iter().any(|v| !identifier(v))
            || self.proposal_eligible != self.reasons.is_empty()
        {
            return Err("PA measurement limits".into());
        }
        provider::counter(&self.capture.first_frame)?
            .checked_add(self.samples as u64)
            .ok_or("PA frame overflow")?;
        let mut previous = 0.;
        for bin in &self.spectrum {
            if ![
                bin.frequency_hz,
                bin.magnitude,
                bin.phase_radians,
                bin.coherence,
            ]
            .iter()
            .all(|v| v.is_finite())
                || bin.frequency_hz <= previous
                || bin.frequency_hz < low
                || bin.frequency_hz > high
                || bin.magnitude < 0.
                || !(-std::f64::consts::PI..=std::f64::consts::PI).contains(&bin.phase_radians)
                || !(0.0..=1.0).contains(&bin.coherence)
            {
                return Err("PA spectral result".into());
            }
            previous = bin.frequency_hz;
        }
        Ok(())
    }
    /// Complete scrollable report, with uncertainty kept beside the measured values.
    pub fn lines(&self) -> Vec<String> {
        let mut lines = vec![
            format!(
                "Capture {} / position {} / PA output index {}",
                self.capture.id, self.capture.position_id, self.capture.output_index
            ),
            format!(
                "Arrival {:+} samples ({:+.3} ms) / correlation {:+.4}",
                self.arrival_samples,
                f64::from(self.arrival_samples) / 48.,
                self.signed_correlation
            ),
            format!(
                "{} samples / {} spectral windows / competitor {:.4}",
                self.samples, self.segments, self.competing_peak_ratio
            ),
            format!(
                "Reference {} at {} / common timeline {}",
                self.capture.reference_id, self.capture.reference_tap, self.capture.clock_domain
            ),
            format!(
                "Source epoch {} / map {} / PA generation {} / first frame {}",
                self.capture.source_epoch,
                self.capture.map_revision,
                self.capture.configuration_revision,
                self.capture.first_frame
            ),
            if self.proposal_eligible {
                "Eligible for owner proposal review; acoustic verification still required".into()
            } else {
                format!("Not eligible for correction: {}", self.reasons.join(", "))
            },
            "Frequency Hz | transfer dB | phase degrees | coherence".into(),
        ];
        for bin in &self.spectrum {
            let magnitude = if bin.magnitude > 0. {
                format!("{:+.2}", 20. * bin.magnitude.log10())
            } else {
                "no coherent transfer".into()
            };
            lines.push(format!(
                "{:.1} | {} | {:+.1} | {:.4}",
                bin.frequency_hz,
                magnitude,
                bin.phase_radians.to_degrees(),
                bin.coherence
            ));
        }
        lines
    }
}
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Change {
    pub output_index: usize,
    pub before_delay_samples: u32,
    pub after_delay_samples: u32,
    pub before_inverted: bool,
    pub after_inverted: bool,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Proposal {
    pub contract: String,
    pub version: u32,
    pub status: String,
    pub basis_configuration_revision: String,
    pub basis_capture: Capture,
    pub basis_configuration: Value,
    pub measurement_ids: Vec<String>,
    pub changes: Vec<Change>,
    pub added_latency_samples: u32,
    pub verification_required: bool,
    pub reason: String,
}
impl Proposal {
    pub fn decode(value: &Value) -> Result<Self, String> {
        if serde_json::to_vec(value).map_err(|e| e.to_string())?.len() > 64 * 1024 {
            return Err("PA result document bound".into());
        }
        let result: Self = serde_json::from_value(value.clone()).map_err(|e| e.to_string())?;
        result.basis_capture.validate()?;
        if result.contract != "C-PA-ALIGNMENT-PROPOSAL"
            || result.version != 1
            || !matches!(result.status.as_str(), "proposed" | "no_change" | "refused")
            || result.basis_configuration_revision != result.basis_capture.configuration_revision
            || !result.basis_configuration.is_object()
            || !result.verification_required
            || result.measurement_ids.len() > 16
            || result.measurement_ids.iter().any(|v| !identifier(v))
            || result.changes.len() > 2
            || result.added_latency_samples > 480
            || !identifier(&result.reason)
            || (result.status != "proposed" && !result.changes.is_empty())
            || (result.status == "proposed" && result.changes.is_empty())
            || (result.status == "no_change" && result.added_latency_samples != 0)
        {
            return Err("PA proposal identity/limits".into());
        }
        let mut outputs = std::collections::BTreeSet::new();
        for change in &result.changes {
            if change.output_index >= 4096
                || !outputs.insert(change.output_index)
                || change.before_delay_samples > change.after_delay_samples
                || change.after_delay_samples > 480
            {
                return Err("PA proposal change".into());
            }
        }
        Ok(result)
    }
    /// Validate the owner's supplied candidate, without constructing a correction or running DSP.
    pub fn validate_candidate(&self, current: &Value, candidate: &Value) -> Result<(), String> {
        if &self.basis_configuration != current {
            return Err("fresh proposed PA basis required".into());
        }
        if self.status == "no_change" {
            return if candidate == current
                && self.changes.is_empty()
                && self.added_latency_samples == 0
            {
                Ok(())
            } else {
                Err("no_change candidate must exactly preserve reviewed basis".into())
            };
        }
        if self.status != "proposed" {
            return Err("fresh proposed PA basis required".into());
        }
        let mut remaining = candidate.clone();
        let mut added = 0;
        for c in &self.changes {
            let before = current["outputs"]
                .get(c.output_index)
                .ok_or("proposal output absent")?;
            let after = candidate["outputs"]
                .get(c.output_index)
                .ok_or("candidate output absent")?;
            let delay = before["processing"]["delay_ms"]
                .as_f64()
                .ok_or("basis delay")?;
            let actual = after["processing"]["delay_ms"]
                .as_f64()
                .ok_or("candidate delay")?;
            if !delay.is_finite()
                || !actual.is_finite()
                || delay < 0.
                || actual < 0.
                || (delay * 48.).round() != f64::from(c.before_delay_samples)
                || before["processing"]["inverted"] != c.before_inverted
                || after["processing"]["inverted"] != c.after_inverted
                || (c.before_delay_samples == c.after_delay_samples && actual != delay)
                || (c.before_delay_samples != c.after_delay_samples
                    && actual != f64::from(c.after_delay_samples) / 48.)
                || (c.before_delay_samples == c.after_delay_samples
                    && c.before_inverted == c.after_inverted)
            {
                return Err("candidate differs from reviewed owner change".into());
            }
            added = added.max(c.after_delay_samples - c.before_delay_samples);
            remaining["outputs"][c.output_index]["processing"]["delay_ms"] =
                before["processing"]["delay_ms"].clone();
            remaining["outputs"][c.output_index]["processing"]["inverted"] =
                before["processing"]["inverted"].clone();
        }
        if &remaining != current || added != self.added_latency_samples {
            return Err("candidate changes unrelated owner fields or latency".into());
        }
        Ok(())
    }
    pub fn lines(&self) -> Vec<String> {
        let mut lines = vec![
            format!("PA alignment {}: {}", self.status, self.reason),
            format!("Measurements: {}", self.measurement_ids.join(", ")),
            format!(
                "Added latency {} samples ({:.3} ms)",
                self.added_latency_samples,
                f64::from(self.added_latency_samples) / 48.
            ),
        ];
        for c in &self.changes {
            lines.push(format!(
                "Output index {}: delay {:.3} -> {:.3} ms; polarity {} -> {}",
                c.output_index,
                f64::from(c.before_delay_samples) / 48.,
                f64::from(c.after_delay_samples) / 48.,
                if c.before_inverted {
                    "inverted"
                } else {
                    "normal"
                },
                if c.after_inverted {
                    "inverted"
                } else {
                    "normal"
                }
            ));
        }
        lines.push(
            "Apply only through fresh muted whole-configuration review. Rearm is separate.".into(),
        );
        lines.push(
            "Offline proposal; combined-response and acoustic verification remain required.".into(),
        );
        lines
    }
}

/// Decode the opaque PA document using a separate finite-number parser. The
/// shared control envelope remains integer-only and rejects duplicate members.
pub fn owner_document(text: &str) -> Result<Value, String> {
    let value = crate::master_eq::parse_owner_bounded(text, 64 * 1024)?;
    match value["contract"].as_str() {
        Some("C-PA-MEASUREMENT-RESULT") => {
            Measurement::decode(&value)?;
        }
        Some("C-PA-ALIGNMENT-PROPOSAL") => {
            Proposal::decode(&value)?;
        }
        Some("C-PA-MEASUREMENT-REFUSAL") => {
            provider::keys(&value, &["contract", "version", "status", "reason"])?;
            if value["version"] != 1
                || value["status"] != "refused"
                || !value["reason"].as_str().is_some_and(|s| {
                    !s.is_empty()
                        && s.len() <= 256
                        && s.bytes().all(|b| b.is_ascii_graphic() || b == b' ')
                })
            {
                return Err("PA refusal document".into());
            }
        }
        _ => return Err("unknown PA owner document".into()),
    }
    Ok(value)
}
pub fn owner_lines(text: &str) -> Result<Vec<String>, String> {
    let value = owner_document(text)?;
    match value["contract"].as_str() {
        Some("C-PA-MEASUREMENT-RESULT") => Ok(Measurement::decode(&value)?.lines()),
        Some("C-PA-ALIGNMENT-PROPOSAL") => Ok(Proposal::decode(&value)?.lines()),
        _ => Ok(vec![
            format!("PA owner refused: {}", value["reason"]),
            "No correction or acoustic success is implied.".into(),
        ]),
    }
}
