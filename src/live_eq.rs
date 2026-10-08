//! GP18-master-eq:1 optional owner extension; full graph guards remain separate.
use crate::{audio::Context, provider};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
pub const CONTRACT: &str = "GP18-master-eq";
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Snapshot {
    pub show_id: String,
    pub epoch: String,
    pub revision: String,
    pub map_revision: String,
    pub frame: String,
    pub live_supported: bool,
    pub live_available: bool,
    pub fault_latched: bool,
    pub source_recovery_required: bool,
    pub settled: bool,
    pub unavailable_reason: Option<String>,
    pub owner_instance: String,
    pub graph_generation: String,
    pub eq_generation: String,
    pub program_buses: Vec<usize>,
    pub master_input_indices: Option<[usize; 2]>,
    pub transition_remaining_frames: String,
    pub retirement_occupied: bool,
    pub owner_json: Option<String>,
}
impl Snapshot {
    pub fn decode(v: Value) -> Result<Self, String> {
        provider::keys(
            &v,
            &[
                "show_id",
                "epoch",
                "revision",
                "map_revision",
                "frame",
                "live_supported",
                "live_available",
                "fault_latched",
                "source_recovery_required",
                "settled",
                "unavailable_reason",
                "owner_instance",
                "graph_generation",
                "eq_generation",
                "program_buses",
                "master_input_indices",
                "transition_remaining_frames",
                "retirement_occupied",
                "owner_json",
            ],
        )?;
        let s: Self = serde_json::from_value(v).map_err(|e| e.to_string())?;
        s.validate()?;
        Ok(s)
    }
    pub fn owner(&self) -> Result<Value, String> {
        crate::master_eq::parse_owner(self.owner_json.as_deref().ok_or("live owner unavailable")?)
    }
    pub fn validate(&self) -> Result<(), String> {
        if !provider::uuid(&self.show_id) || self.epoch == "0" || self.program_buses.len() > 4096 {
            return Err("live EQ identity/bounds".into());
        }
        for n in [
            &self.epoch,
            &self.revision,
            &self.map_revision,
            &self.frame,
            &self.owner_instance,
            &self.graph_generation,
            &self.eq_generation,
            &self.transition_remaining_frames,
        ] {
            provider::counter(n)?;
        }
        let remaining = provider::counter(&self.transition_remaining_frames)?;
        if remaining > 240
            || self.settled != (remaining == 0 && !self.fault_latched) && self.live_supported
            || self.source_recovery_required && !self.fault_latched
            || self.live_available
                && (!self.live_supported
                    || self.fault_latched
                    || self.source_recovery_required
                    || self.unavailable_reason.is_some())
        {
            return Err("live EQ availability/settlement".into());
        }
        if !self.live_supported {
            if self.live_available
                || self.owner_json.is_some()
                || self.master_input_indices.is_some()
                || self.settled
            {
                return Err("unavailable live EQ claimed owner".into());
            }
            return Ok(());
        }
        if self.owner_instance == "0" || self.graph_generation == "0" {
            return Err("live owner identity".into());
        }
        if !self.live_available
            && self.master_input_indices.is_none()
            && self.owner_json.is_none()
            && self.unavailable_reason.is_some()
        {
            return Ok(());
        }
        let indices = self
            .master_input_indices
            .ok_or("live master indices missing")?;
        if indices[0] == indices[1] {
            return Err("live stereo identity".into());
        }
        for (bus, index) in indices.iter().enumerate() {
            if self.program_buses.iter().filter(|b| **b == bus).count() != 1
                || self.program_buses.get(*index) != Some(&bus)
            {
                return Err("live unique main mapping".into());
            }
        }
        let owner = self.owner()?;
        provider::keys(
            &owner,
            &[
                "version",
                "sample_rate",
                "graph_generation",
                "eq_generation",
                "settled",
                "fault_latched",
                "current",
                "target",
                "current_coefficients",
                "target_coefficients",
                "current_fingerprint",
                "target_fingerprint",
                "coefficient_order",
                "denominator",
            ],
        )?;
        if owner["version"] != 1
            || owner["sample_rate"] != 48000
            || owner["graph_generation"].as_u64()
                != Some(provider::counter(&self.graph_generation)?)
            || owner["eq_generation"].as_u64() != Some(provider::counter(&self.eq_generation)?)
            || owner["settled"].as_bool() != Some(remaining == 0 && owner["fault_latched"] == false)
            || owner["fault_latched"].as_bool().is_none()
            || owner["coefficient_order"] != json!(["b0", "b1", "b2", "a1", "a2"])
            || owner["denominator"] != "1+a1*z^-1+a2*z^-2"
        {
            return Err("live outer/owner binding".into());
        }
        // Outer source faults and inner PA faults have distinct domains. An inner
        // fault must still close outer availability; the converse is not inferred.
        if owner["fault_latched"] == true && !self.fault_latched {
            return Err("unreported owner fault".into());
        }
        for state in ["current", "target"] {
            let settings = owner[state]
                .as_array()
                .filter(|s| s.len() == 2)
                .ok_or("live stereo settings")?;
            let mut found = Vec::new();
            let supplied = owner[format!("{state}_coefficients")]
                .as_array()
                .filter(|s| s.len() == 2)
                .ok_or("live stereo coefficients")?;
            for (side, input) in settings.iter().enumerate() {
                provider::keys(
                    input,
                    &["input_index", "eq_enabled", "eq", "geq_enabled", "geq_db"],
                )?;
                let index = input["input_index"].as_u64().ok_or("live input index")? as usize;
                if index != indices[side] || found.contains(&index) {
                    return Err("live settings mapping".into());
                }
                found.push(index);
                let calculated = crate::eq_response::coefficients(input, 48000)?;
                let bank = supplied[side]
                    .as_array()
                    .filter(|s| s.len() == 39)
                    .ok_or("live coefficient bank")?;
                for (expected, c) in calculated.iter().zip(bank) {
                    let c = c
                        .as_array()
                        .filter(|c| c.len() == 5)
                        .ok_or("live normalized coefficient")?;
                    for (a, b) in expected.iter().zip(c) {
                        if b.as_f64()
                            .is_none_or(|b| !b.is_finite() || (a - b).abs() > 1e-10)
                        {
                            return Err("live settings/coefficient mismatch".into());
                        }
                    }
                }
            }
        }
        if owner["settled"] == true
            && (owner["current"] != owner["target"]
                || owner["current_coefficients"] != owner["target_coefficients"])
        {
            return Err("live settled endpoints differ".into());
        }
        for key in ["current_fingerprint", "target_fingerprint"] {
            if owner[key].as_str().is_none_or(|s| {
                !s.starts_with("fnv1a64:")
                    || s.len() != 24
                    || !s[8..].bytes().all(|b| b.is_ascii_hexdigit())
            }) {
                return Err("live fingerprint".into());
            }
        }
        Ok(())
    }
    pub fn editable(&self) -> bool {
        self.live_supported
            && self.live_available
            && self.settled
            && !self.fault_latched
            && !self.source_recovery_required
    }
    pub fn same_context(&self, old: &Self) -> bool {
        self.editable()
            && old.editable()
            && self.show_id == old.show_id
            && self.epoch == old.epoch
            && self.revision == old.revision
            && self.map_revision == old.map_revision
            && self.owner_instance == old.owner_instance
            && self.graph_generation == old.graph_generation
            && self.eq_generation == old.eq_generation
            && self.program_buses == old.program_buses
            && self.master_input_indices == old.master_input_indices
            && self.owner_json == old.owner_json
    }
}
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Reply {
    pub contract: String,
    pub version: u8,
    pub context: Context,
    pub state: String,
    pub reason: Option<String>,
    pub effective_frame: Option<String>,
    pub revision: String,
    pub snapshot: Option<Value>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub master_eq: Option<Snapshot>,
}
pub fn decode_reply(bytes: &[u8]) -> Result<Reply, String> {
    let v = provider::parse_document(bytes)?;
    let mut expected = vec![
        "contract",
        "version",
        "context",
        "state",
        "reason",
        "effective_frame",
        "revision",
        "snapshot",
    ];
    if v.get("master_eq").is_some() {
        expected.push("master_eq");
        Snapshot::decode(v["master_eq"].clone())?;
    }
    provider::keys(&v, &expected)?;
    crate::audio::validate_context_value(&v["context"])?;
    let r: Reply = serde_json::from_value(v).map_err(|e| e.to_string())?;
    r.context.validate()?;
    let revision = provider::counter(&r.revision)?;
    if r.contract != CONTRACT || r.version != 1 || r.context.epoch == "0" || r.snapshot.is_some() {
        return Err("live EQ envelope".into());
    }
    if let Some(f) = &r.effective_frame {
        let frame = provider::counter(f)?;
        if frame == 0 || frame % 48 != 0 {
            return Err("live EQ boundary".into());
        }
    }
    if r.reason
        .as_ref()
        .is_some_and(|s| s.is_empty() || s.len() > 256)
    {
        return Err("live EQ reason".into());
    }
    let mutation = r.context.writer.is_some();
    match r.state.as_str() {
        "snapshot"
            if !mutation
                && r.reason.is_none()
                && r.effective_frame.is_none()
                && r.master_eq.is_some() => {}
        "pending"
            if mutation
                && r.reason.is_none()
                && r.effective_frame.is_some()
                && r.master_eq.is_none() => {}
        "final"
            if mutation
                && r.master_eq.is_none()
                && (r.reason.is_some() == r.effective_frame.is_none()) => {}
        _ => return Err("live EQ state/shape".into()),
    }
    if let Some(s) = &r.master_eq {
        s.validate()?;
        if s.show_id != r.context.show_id || s.epoch != r.context.epoch || s.revision != r.revision
        {
            return Err("live EQ snapshot binding".into());
        }
    }
    if mutation {
        if r.context.lease.is_none() {
            return Err("live EQ lease".into());
        }
        let expected = provider::counter(
            r.context
                .expected_revision
                .as_deref()
                .ok_or("live EQ expected revision")?,
        )?;
        if r.state == "pending" && revision != expected
            || r.state == "final" && r.reason.is_none() && Some(revision) != expected.checked_add(1)
        {
            return Err("live EQ commit revision".into());
        }
    }
    Ok(r)
}
pub fn validate_body(body: &Value, snapshot: Option<&Snapshot>) -> Result<(), String> {
    provider::keys(
        body,
        &[
            "patch_json",
            "program_buses",
            "owner_instance",
            "graph_generation",
            "eq_generation",
            "map_revision",
        ],
    )?;
    for key in [
        "owner_instance",
        "graph_generation",
        "eq_generation",
        "map_revision",
    ] {
        provider::counter(body[key].as_str().ok_or("live EQ pin")?)?;
    }
    if body["owner_instance"] == "0" || body["graph_generation"] == "0" {
        return Err("live EQ owner pin".into());
    }
    let text = body["patch_json"]
        .as_str()
        .filter(|s| !s.is_empty() && s.len() <= 32768)
        .ok_or("live EQ patch bounds")?;
    let patch = crate::master_eq::parse_owner(text)?;
    provider::keys(&patch, &["version", "inputs"])?;
    if patch["version"] != 1 {
        return Err("live EQ patch version".into());
    }
    let inputs = patch["inputs"]
        .as_array()
        .filter(|a| a.len() == 2)
        .ok_or("live stereo patch")?;
    let buses: Vec<usize> =
        serde_json::from_value(body["program_buses"].clone()).map_err(|e| e.to_string())?;
    if buses.is_empty() || buses.len() > 4096 {
        return Err("live EQ complete buses".into());
    }
    let mut indices = Vec::new();
    for input in inputs {
        provider::keys(
            input,
            &["input_index", "eq_enabled", "eq", "geq_enabled", "geq_db"],
        )?;
        let index = input["input_index"]
            .as_u64()
            .filter(|i| *i < buses.len() as u64)
            .ok_or("live EQ index")? as usize;
        if indices.contains(&index)
            || buses[index] > 1
            || buses.iter().filter(|b| **b == buses[index]).count() != 1
        {
            return Err("live EQ unique program inputs".into());
        }
        indices.push(index);
        crate::eq_response::coefficients(input, 48000)?;
    }
    if let Some(s) = snapshot {
        if !s.editable()
            || s.program_buses != buses
            || body["owner_instance"] != s.owner_instance
            || body["graph_generation"] != s.graph_generation
            || body["eq_generation"] != s.eq_generation
            || body["map_revision"] != s.map_revision
        {
            return Err("fresh compatible settled live EQ context required".into());
        }
        let mut indices = indices;
        indices.sort();
        let mut expected = s.master_input_indices.unwrap();
        expected.sort();
        if indices.as_slice() != expected {
            return Err("live EQ mapping changed".into());
        }
    }
    Ok(())
}
