//! Provider-owned logical/physical patch and observational common-clock metadata.
use serde::{Deserialize, Serialize};
use std::collections::BTreeSet;
#[derive(Clone, Debug, PartialEq, Eq, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct InputPort {
    pub id: String,
    pub capture_slot: usize,
    pub physical_port: String,
}
#[derive(Clone, Debug, PartialEq, Eq, Deserialize, Serialize)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
pub enum OutputSource {
    Main { channel: usize },
    Monitor { index: usize },
    Pa { index: usize },
}
#[derive(Clone, Debug, PartialEq, Eq, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct OutputPort {
    pub id: String,
    pub playback_slot: usize,
    pub physical_port: String,
    pub source: Option<OutputSource>,
}
#[derive(Clone, Debug, PartialEq, Eq, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct Topology {
    pub identity: String,
    pub mapping_evidence: String,
    pub map_revision: u64,
    pub sample_rate: u32,
    pub max_block_frames: usize,
    pub capture_channels: usize,
    pub playback_channels: usize,
    pub inputs: Vec<InputPort>,
    pub measurement_slots: Vec<usize>,
    pub monitors: usize,
    pub pa_outputs: usize,
    pub outputs: Vec<OutputPort>,
}
fn label(s: &str) -> bool {
    !s.is_empty() && s.len() <= 128 && !s.chars().any(char::is_control)
}
impl Topology {
    pub fn validate(&self) -> Result<(), String> {
        if !label(&self.identity)
            || !label(&self.mapping_evidence)
            || self.map_revision == 0
            || self.sample_rate != 48000
            || self.max_block_frames == 0
            || self.inputs.is_empty()
            || self.inputs.len() > u16::MAX as usize
            || self.monitors > u16::MAX as usize
        {
            return Err("topology identity/rate/capacity".into());
        }
        let mut ids = BTreeSet::new();
        let mut slots = BTreeSet::new();
        for p in &self.inputs {
            if !crate::provider::id(&p.id)
                || !label(&p.physical_port)
                || !ids.insert(&p.id)
                || p.capture_slot >= self.capture_channels
                || !slots.insert(p.capture_slot)
            {
                return Err("input patch identity/slot".into());
            }
        }
        for &slot in &self.measurement_slots {
            if slot >= self.capture_channels || !slots.insert(slot) {
                return Err("measurement/program overlap".into());
            }
        }
        self.validate_outputs(&self.outputs)
    }
    pub fn validate_outputs(&self, outputs: &[OutputPort]) -> Result<(), String> {
        let mut ids = BTreeSet::new();
        let mut slots = BTreeSet::new();
        for p in outputs {
            if !label(&p.id)
                || !label(&p.physical_port)
                || !ids.insert(&p.id)
                || p.playback_slot >= self.playback_channels
                || !slots.insert(p.playback_slot)
            {
                return Err("output patch identity/duplicate writer/slot".into());
            }
            let valid = match p.source {
                None => true,
                Some(OutputSource::Main { channel }) => channel < 2,
                Some(OutputSource::Monitor { index }) => index < self.monitors,
                Some(OutputSource::Pa { index }) => index < self.pa_outputs,
            };
            if !valid {
                return Err("output source absent from topology".into());
            }
        }
        Ok(())
    }
}
#[derive(Clone, Debug, PartialEq, Eq, Deserialize, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum LockEvidence {
    Unknown,
    Locked,
    Lost,
}
#[derive(Clone, Debug, PartialEq, Eq, Deserialize, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum ClockState {
    Disarmed,
    Running,
    Quiesced,
}
#[derive(Clone, Debug, PartialEq, Eq, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct Clock {
    pub domain: String,
    pub sample_rate: u32,
    pub epoch: u64,
    pub next_frame: u64,
    pub state: ClockState,
    pub adat_lock: LockEvidence,
    pub mapping_verified: bool,
    pub fault: Option<String>,
}
impl Clock {
    pub fn validate(&self) -> Result<(), String> {
        if self.domain != "single-interface"
            || self.sample_rate != 48000
            || self.epoch == 0
            || self.fault.as_deref().is_some_and(|s| !label(s))
        {
            return Err("clock metadata".into());
        }
        Ok(())
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct Admission {
    pub estimated_bytes: usize,
    pub sample_operations: usize,
    pub estimated_snapshot_bytes: usize,
}
#[derive(Clone, Debug, PartialEq, Eq, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct Budget {
    pub bytes: usize,
    pub sample_operations: usize,
}
#[derive(Clone, Debug, PartialEq, Eq, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct Resources {
    pub admission: Admission,
    pub render_budget: Budget,
    pub snapshot_assembly_bytes: usize,
    pub live_writer_capacity: usize,
    pub reply_history_per_writer: usize,
}
impl Resources {
    pub fn validate(&self) -> Result<(), String> {
        if self.snapshot_assembly_bytes != crate::provider::MAX_DOCUMENT_BYTES
            || self.live_writer_capacity != 4
            || self.reply_history_per_writer != 64
            || self.admission.estimated_bytes > self.render_budget.bytes
            || self.admission.sample_operations > self.render_budget.sample_operations
            || self.admission.estimated_snapshot_bytes > self.snapshot_assembly_bytes
        {
            return Err("resource admission metadata".into());
        }
        Ok(())
    }
}
