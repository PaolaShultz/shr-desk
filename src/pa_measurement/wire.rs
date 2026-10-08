//! Frozen GP20 transport DTOs. No analysis or candidate construction.
use crate::provider;
use serde::{Deserialize, Serialize};
use serde_json::Value;
pub const CONTRACT: &str = "GP20-measurement";
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Basis {
    pub source_epoch: String,
    pub map_revision: String,
    pub graph_generation: String,
    pub clock_domain: String,
    pub configuration_json: String,
    pub program_buses: Vec<usize>,
}
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Summary {
    pub id: String,
    pub kind: String,
    pub state: String,
    pub reason: Option<String>,
}
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Record {
    pub summary: Summary,
    pub basis: Basis,
    pub owner_result_json: Option<String>,
    pub candidate_configuration_json: Option<String>,
}
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Progress {
    pub id: String,
    pub received_samples: usize,
    pub requested_samples: usize,
    pub output_index: usize,
}
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Snapshot {
    pub available: bool,
    pub software_only: bool,
    pub reason: Option<String>,
    pub active_id: Option<String>,
    pub capture_progress: Option<Progress>,
    pub results: Vec<Summary>,
    pub current_basis: Option<Basis>,
    pub reserved_mic_slots: Vec<usize>,
    pub reference_inputs: Vec<String>,
}
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Reply {
    pub contract: String,
    pub version: u32,
    pub context: crate::audio::Context,
    pub state: String,
    pub reason: Option<String>,
    pub revision: String,
    pub effective_frame: Option<String>,
    pub snapshot: Option<Snapshot>,
    pub result: Option<Record>,
}
pub fn is_kind(k: &str) -> bool {
    matches!(
        k,
        "measurement_snapshot"
            | "measurement_result"
            | "capture_start"
            | "capture_cancel"
            | "measurement_propose"
    )
}
pub fn mutation(k: &str) -> bool {
    matches!(
        k,
        "capture_start" | "capture_cancel" | "measurement_propose"
    )
}
fn id(v: &Value) -> Result<(), String> {
    if v.as_str().is_some_and(super::identifier) {
        Ok(())
    } else {
        Err("measurement identifier".into())
    }
}
pub fn validate_body(k: &str, b: &Value, s: Option<&Snapshot>) -> Result<(), String> {
    match k {
        "measurement_snapshot" => provider::keys(b, &[])?,
        "measurement_result" | "capture_cancel" => {
            provider::keys(b, &["id"])?;
            id(&b["id"])?;
        }
        "capture_start" => {
            provider::keys(
                b,
                &[
                    "id",
                    "reference_input",
                    "mic_capture_slot",
                    "output_index",
                    "position_id",
                    "samples",
                    "options",
                ],
            )?;
            for k in ["id", "reference_input", "position_id"] {
                id(&b[k])?;
            }
            provider::keys(&b["options"], &["max_arrival_samples", "band_hz"])?;
            let n = |v: &Value| v.as_u64().ok_or_else(|| "measurement integer".to_string());
            let samples = n(&b["samples"])?;
            let lag = n(&b["options"]["max_arrival_samples"])?;
            let band = b["options"]["band_hz"].as_array().ok_or("frequency pair")?;
            if !(32768..=65536).contains(&samples)
                || !(1..=2048).contains(&lag)
                || band.len() != 2
                || n(&band[0])? < 20
                || n(&band[1])? > 20000
                || n(&band[0])? >= n(&band[1])?
            {
                return Err("capture bounds".into());
            }
            let slot = n(&b["mic_capture_slot"])? as usize;
            let out = n(&b["output_index"])? as usize;
            if slot >= 4096 || out >= 4096 {
                return Err("capture index bound".into());
            }
            if let Some(s) = s {
                if !s.reserved_mic_slots.contains(&slot)
                    || !s
                        .reference_inputs
                        .iter()
                        .any(|x| b["reference_input"] == *x)
                    || s.active_id.is_some()
                {
                    return Err("reserved microphone/reference/idle capture required".into());
                }
                let cfg = crate::master_eq::parse_owner(
                    &s.current_basis
                        .as_ref()
                        .ok_or("measurement basis")?
                        .configuration_json,
                )?;
                if cfg["outputs"].as_array().is_none_or(|a| out >= a.len()) {
                    return Err("PA output selection".into());
                }
            }
        }
        "measurement_propose" => {
            provider::keys(b, &["id", "pairs"])?;
            id(&b["id"])?;
            let ps = b["pairs"].as_array().ok_or("capture pairs")?;
            if ps.is_empty() || ps.len() > 8 {
                return Err("1..8 pairs required".into());
            }
            for p in ps {
                let p = p.as_array().ok_or("capture pair")?;
                if p.len() != 2 {
                    return Err("capture pair length".into());
                }
                id(&p[0])?;
                id(&p[1])?;
                if p[0] == p[1] {
                    return Err("distinct capture pair".into());
                }
            }
        }
        _ => return Err("measurement command".into()),
    };
    Ok(())
}
impl Basis {
    pub fn validate(&self) -> Result<(), String> {
        for c in [
            &self.source_epoch,
            &self.map_revision,
            &self.graph_generation,
        ] {
            if provider::counter(c)? == 0 {
                return Err("measurement basis origin".into());
            }
        }
        if self.clock_domain != "software-common-source"
            || self.program_buses.is_empty()
            || self.program_buses.len() > 128
            || self.program_buses.iter().any(|v| *v >= 4096)
        {
            return Err("measurement basis bound".into());
        }
        crate::master_eq::parse_owner(&self.configuration_json)?;
        Ok(())
    }
}
impl Summary {
    fn validate(&self) -> Result<(), String> {
        if !super::identifier(&self.id)
            || !matches!(self.kind.as_str(), "capture" | "proposal")
            || !matches!(
                self.state.as_str(),
                "capturing"
                    | "analyzing"
                    | "measured"
                    | "proposed"
                    | "no_change"
                    | "refused"
                    | "cancelled"
                    | "invalidated"
            )
            || self.reason.as_ref().is_some_and(|s| s.len() > 1024)
        {
            return Err("measurement summary".into());
        }
        Ok(())
    }
}
impl Reply {
    pub fn decode(bytes: &[u8]) -> Result<Self, String> {
        let v = provider::parse_document(bytes)?;
        provider::keys(
            &v,
            &[
                "contract",
                "version",
                "context",
                "state",
                "reason",
                "revision",
                "effective_frame",
                "snapshot",
                "result",
            ],
        )?;
        crate::audio::validate_context_value(&v["context"])?;
        if !v["snapshot"].is_null() {
            provider::keys(
                &v["snapshot"],
                &[
                    "available",
                    "software_only",
                    "reason",
                    "active_id",
                    "capture_progress",
                    "results",
                    "current_basis",
                    "reserved_mic_slots",
                    "reference_inputs",
                ],
            )?;
        }
        if !v["result"].is_null() {
            provider::keys(
                &v["result"],
                &[
                    "summary",
                    "basis",
                    "owner_result_json",
                    "candidate_configuration_json",
                ],
            )?;
        }

        let r: Self = serde_json::from_value(v).map_err(|e| e.to_string())?;
        r.context.validate()?;
        let rev = provider::counter(&r.revision)?;
        if r.contract != CONTRACT
            || r.version != 1
            || r.context.epoch == "0"
            || r.reason.as_ref().is_some_and(|s| s.len() > 1024)
        {
            return Err("measurement envelope".into());
        }
        if r.context.writer.is_none() {
            if !matches!(r.state.as_str(), "snapshot" | "result") || r.effective_frame.is_some() {
                return Err("measurement read envelope".into());
            }
        } else {
            let expected = provider::counter(
                r.context
                    .expected_revision
                    .as_deref()
                    .ok_or("measurement revision")?,
            )?;
            if !matches!(r.state.as_str(), "pending" | "final")
                || r.snapshot.is_some()
                || r.result.is_some()
                || r.context.lease.is_none()
            {
                return Err("measurement mutation envelope".into());
            }
            if r.reason.is_none() {
                if r.effective_frame
                    .as_deref()
                    .is_none_or(|f| provider::counter(f).is_err() || f == "0")
                    || rev
                        != if r.state == "pending" {
                            expected
                        } else {
                            expected.checked_add(1).ok_or("revision overflow")?
                        }
                {
                    return Err("measurement boundary/revision".into());
                }
            } else if r.state != "final" || r.effective_frame.is_some() {
                return Err("measurement refusal".into());
            }
        }
        if let Some(s) = &r.snapshot {
            if r.state != "snapshot"
                || !s.software_only
                || s.results.len() > 16
                || s.reserved_mic_slots.len() > 4096
                || s.reference_inputs.len() > 4096
                || s.reference_inputs.iter().any(|s| !super::identifier(s))
            {
                return Err("measurement snapshot bound".into());
            }
            for v in &s.results {
                v.validate()?;
            }
            if let Some(b) = &s.current_basis {
                b.validate()?;
            }
            if s.available && s.current_basis.is_none() {
                return Err("available measurement basis".into());
            }
            if let Some(p) = &s.capture_progress
                && (Some(&p.id) != s.active_id.as_ref()
                    || !(32768..=65536).contains(&p.requested_samples)
                    || p.received_samples > p.requested_samples
                    || p.output_index >= 4096)
            {
                return Err("capture progress".into());
            }
        }
        if let Some(s) = &r.result {
            if r.state != "result" {
                return Err("measurement result state".into());
            }
            s.summary.validate()?;
            s.basis.validate()?;
            if let Some(j) = &s.owner_result_json {
                let doc = super::owner_document(j)?;
                if let Some(c) = doc.get("capture").or_else(|| doc.get("basis_capture")) {
                    if c["source_epoch"] != s.basis.source_epoch
                        || c["map_revision"] != s.basis.map_revision
                        || c["configuration_revision"] != s.basis.graph_generation
                        || c["clock_domain"] != s.basis.clock_domain
                    {
                        return Err("owner result/basis provenance mismatch".into());
                    }
                    if s.summary.kind == "capture" && c["id"] != s.summary.id {
                        return Err("owner capture identity mismatch".into());
                    }
                }
            }
            if let Some(j) = &s.candidate_configuration_json {
                let candidate = crate::master_eq::parse_owner(j)?;
                let doc = super::owner_document(
                    s.owner_result_json
                        .as_deref()
                        .ok_or("candidate owner result")?,
                )?;
                let proposal = super::Proposal::decode(&doc)?;
                proposal.validate_candidate(
                    &crate::master_eq::parse_owner(&s.basis.configuration_json)?,
                    &candidate,
                )?;
            }
        }
        Ok(r)
    }
}
#[derive(Clone, Debug, Default)]
pub struct State {
    pub snapshot: Option<Snapshot>,
    pub result: Option<Record>,
    pub revision: Option<String>,
    pub receipt: Option<u64>,
    pub final_reply: Option<Reply>,
}
impl State {
    pub fn lines(&self) -> Vec<String> {
        let mut v = vec!["Explicit GP20 probe; PA owner performs all analysis.".into()];
        if let Some(s) = &self.snapshot {
            v.push(format!(
                "Available={} / active={:?} / reason={:?}",
                s.available, s.active_id, s.reason
            ));
            v.push(format!(
                "Reserved mic slots {:?}; references {:?}",
                s.reserved_mic_slots, s.reference_inputs
            ));
            if let Some(b) = &s.current_basis {
                v.push(format!(
                    "Source {} / map {} / graph {} / program buses {:?}",
                    b.source_epoch, b.map_revision, b.graph_generation, b.program_buses
                ));
            }
            if let Some(p) = &s.capture_progress {
                v.push(format!(
                    "{}: {}/{} samples; output {}",
                    p.id, p.received_samples, p.requested_samples, p.output_index
                ));
            }
            for x in &s.results {
                v.push(format!("{} {} {} {:?}", x.id, x.kind, x.state, x.reason));
            }
        }
        if let Some(r) = &self.result {
            v.push(format!(
                "RESULT {} {} {:?}",
                r.summary.id, r.summary.state, r.summary.reason
            ));
            if let Some(j) = &r.owner_result_json {
                match super::owner_lines(j) {
                    Ok(lines) => v.extend(lines),
                    Err(e) => v.push(e),
                }
            }
        }
        v
    }
}
