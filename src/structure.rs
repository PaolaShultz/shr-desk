//! GP14 structural controls. PA configuration remains the owner's opaque JSON;
//! Desk validates transport/authority and presents every proposed byte for review.
use crate::{
    audio::Context,
    provider,
    topology::{Clock, OutputPort, Topology},
};
use serde::{Deserialize, Serialize};
use serde_json::Value;
#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct Snapshot {
    pub show_id: String,
    pub epoch: String,
    pub revision: String,
    pub frame: String,
    pub topology: Topology,
    pub clock: Clock,
    pub outputs_quiesced: bool,
    pub pa_configuration_json: Option<String>,
    pub pa_program_buses: Vec<usize>,
    pub pa_status: Option<Value>,
    pub pa_capabilities: Option<Value>,
}
impl Snapshot {
    pub fn validate(&self) -> Result<(), String> {
        if !provider::uuid(&self.show_id) {
            return Err("structural show".into());
        }
        for n in [&self.epoch, &self.revision, &self.frame] {
            provider::counter(n)?;
        }
        self.topology.validate()?;
        self.clock.validate()?;
        if self.clock.epoch != provider::counter(&self.epoch)? {
            return Err("structural clock epoch".into());
        }
        if let Some(config) = &self.pa_configuration_json
            && (config.len() > 48 * 1024
                || !serde_json::from_str::<Value>(config).is_ok_and(|v| v.is_object()))
        {
            return Err("PA owner document".into());
        }
        if self.pa_program_buses.len() > 4096
            || self
                .pa_program_buses
                .iter()
                .any(|n| *n >= self.topology.monitors + 2)
        {
            return Err("PA program buses".into());
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
    pub effective_frame: Option<String>,
    pub revision: String,
    pub snapshot: Option<Snapshot>,
}
pub fn decode_reply(bytes: &[u8]) -> Result<Reply, String> {
    let value = provider::parse_document(bytes)?;
    provider::keys(
        &value,
        &[
            "contract",
            "version",
            "context",
            "state",
            "reason",
            "effective_frame",
            "revision",
            "snapshot",
        ],
    )?;
    crate::audio::validate_context_value(&value["context"])?;
    if !value["snapshot"].is_null() {
        provider::keys(
            &value["snapshot"],
            &[
                "show_id",
                "epoch",
                "revision",
                "frame",
                "topology",
                "clock",
                "outputs_quiesced",
                "pa_configuration_json",
                "pa_program_buses",
                "pa_status",
                "pa_capabilities",
            ],
        )?;
    }
    let reply: Reply = serde_json::from_value(value).map_err(|e| e.to_string())?;
    if reply.contract != "GP14-structure" || reply.version != 1 {
        return Err("structural version".into());
    }
    reply.context.validate()?;
    provider::counter(&reply.revision)?;
    if reply.context.epoch == "0" || !matches!(reply.state.as_str(), "pending" | "final") {
        return Err("structural context/state".into());
    }
    if let Some(frame) = &reply.effective_frame
        && !provider::counter(frame)?.is_multiple_of(48)
    {
        return Err("structural boundary".into());
    }
    if reply
        .reason
        .as_ref()
        .is_some_and(|s| s.is_empty() || s.len() > 256)
    {
        return Err("structural refusal".into());
    }
    if reply.state == "pending"
        && (reply.reason.is_some() || reply.snapshot.is_some() || reply.context.writer.is_none())
    {
        return Err("structural pending shape".into());
    }
    if let Some(snapshot) = &reply.snapshot {
        snapshot.validate()?;
        if snapshot.show_id != reply.context.show_id
            || snapshot.epoch != reply.context.epoch
            || snapshot.revision != reply.revision
        {
            return Err("structural snapshot binding".into());
        }
    }
    if reply.state == "final" && reply.reason.is_none() && reply.snapshot.is_none() {
        return Err("structural final readback required".into());
    }
    Ok(reply)
}
pub fn is_kind(kind: &str) -> bool {
    matches!(
        kind,
        "structural_snapshot" | "pa_set" | "output_patch" | "output_mute" | "output_rearm"
    )
}
pub fn scope(kind: &str) -> Option<&'static str> {
    match kind {
        "structural_snapshot" => None,
        "output_patch" => Some("output_routes"),
        "pa_set" | "output_mute" | "output_rearm" => Some("pa_configuration"),
        _ => None,
    }
}
pub fn validate_body(kind: &str, body: &Value, snapshot: Option<&Snapshot>) -> Result<(), String> {
    match kind {
        "structural_snapshot" | "output_mute" | "output_rearm" => provider::keys(body, &[]),
        "output_patch" => {
            provider::keys(body, &["outputs"])?;
            let outputs: Vec<OutputPort> =
                serde_json::from_value(body["outputs"].clone()).map_err(|e| e.to_string())?;
            if outputs.is_empty() || outputs.len() > 4096 {
                return Err("output patch resource bound".into());
            }
            if let Some(snapshot) = snapshot {
                snapshot.topology.validate_outputs(&outputs)?;
            }
            Ok(())
        }
        "pa_set" => {
            provider::keys(body, &["configuration_json", "program_buses"])?;
            let config = body["configuration_json"]
                .as_str()
                .ok_or("PA owner document")?;
            if config.is_empty()
                || config.len() > 48 * 1024
                || !serde_json::from_str::<Value>(config).is_ok_and(|v| v.is_object())
            {
                return Err("PA owner document shape/resource".into());
            }
            let buses = body["program_buses"].as_array().ok_or("program buses")?;
            if buses.is_empty()
                || buses.len() > 4096
                || buses.iter().any(|b| {
                    b.as_u64().is_none_or(|n| {
                        snapshot.is_some_and(|s| n >= (s.topology.monitors + 2) as u64)
                    })
                })
            {
                return Err("program bus inventory".into());
            }
            Ok(())
        }
        _ => Err("unknown structural command".into()),
    }
}

/// Detached draft. PA numeric/boolean fields retain the owner's names and units;
/// physical routing selects explicit advertised sources for existing output sockets.
#[derive(Clone, Debug)]
pub struct Draft {
    pub kind: String,
    pub document: Value,
    pub fields: Vec<String>,
    pub selected: usize,
    pub revision: String,
    pub generation: u64,
}
impl Draft {
    pub fn new(snapshot: &Snapshot, scope: &str, generation: u64) -> Result<Self, String> {
        let (kind, document) = match scope {
            "pa_configuration" => (
                "pa_set",
                serde_json::json!({"configuration":serde_json::from_str::<Value>(snapshot.pa_configuration_json.as_deref().ok_or("PA owner configuration unavailable")?).map_err(|e|e.to_string())?,"program_buses":snapshot.pa_program_buses}),
            ),
            "output_routes" => (
                "output_patch",
                serde_json::json!({"outputs":snapshot.topology.outputs}),
            ),
            _ => return Err("PA or output-routes scope required".into()),
        };
        let mut fields = Vec::new();
        if kind == "output_patch" {
            fields.extend(
                (0..snapshot.topology.outputs.len()).map(|n| format!("/outputs/{n}/source")),
            );
        } else {
            numeric_fields(&document, "", &mut fields);
        }
        if fields.is_empty() {
            return Err("no advertised editable fields".into());
        }
        Ok(Self {
            kind: kind.into(),
            document,
            fields,
            selected: 0,
            revision: snapshot.revision.clone(),
            generation,
        })
    }
    pub fn move_field(&mut self, delta: i32) {
        self.selected =
            (self.selected as i64 + i64::from(delta)).rem_euclid(self.fields.len() as i64) as usize;
    }
    pub fn text(&mut self, text: &str) -> Result<(), String> {
        if text.len() > 128 {
            return Err("field entry capacity".into());
        }
        let value: Value = serde_json::from_str(text).map_err(|e| e.to_string())?;
        let field = self
            .document
            .pointer_mut(&self.fields[self.selected])
            .ok_or("field")?;
        if (field.is_number() && !value.is_number()) || (field.is_boolean() && !value.is_boolean())
        {
            return Err("owner field type".into());
        }
        *field = value;
        Ok(())
    }
    pub fn adjust(&mut self, delta: i32, snapshot: &Snapshot) -> Result<(), String> {
        let field = self
            .document
            .pointer_mut(&self.fields[self.selected])
            .ok_or("field")?;
        if self.kind == "output_patch" {
            let mut sources = vec![
                Value::Null,
                serde_json::json!({"kind":"main","channel":0}),
                serde_json::json!({"kind":"main","channel":1}),
            ];
            sources.extend(
                (0..snapshot.topology.monitors)
                    .map(|index| serde_json::json!({"kind":"monitor","index":index})),
            );
            sources.extend(
                (0..snapshot.topology.pa_outputs)
                    .map(|index| serde_json::json!({"kind":"pa","index":index})),
            );
            let current = sources
                .iter()
                .position(|v| v == field)
                .ok_or("source no longer advertised")?;
            *field = sources
                [(current as i64 + i64::from(delta)).rem_euclid(sources.len() as i64) as usize]
                .clone();
        } else if let Some(value) = field.as_bool() {
            *field = Value::Bool(!value);
        } else if let Some(value) = field.as_i64() {
            *field = value
                .checked_add(i64::from(delta))
                .ok_or("field overflow")?
                .into();
        } else if let Some(value) = field.as_f64() {
            *field = serde_json::Number::from_f64(value + f64::from(delta))
                .ok_or("nonfinite field")?
                .into();
        } else {
            return Err("field not adjustable".into());
        }
        Ok(())
    }
    pub fn body(&self) -> Result<Value, String> {
        if self.kind == "pa_set" {
            Ok(
                serde_json::json!({"configuration_json":serde_json::to_string(&self.document["configuration"]).map_err(|e|e.to_string())?,"program_buses":self.document["program_buses"]}),
            )
        } else {
            Ok(self.document.clone())
        }
    }
}
fn numeric_fields(value: &Value, path: &str, fields: &mut Vec<String>) {
    match value {
        Value::Number(_) | Value::Bool(_) => fields.push(path.into()),
        Value::Array(values) => {
            for (i, value) in values.iter().enumerate() {
                numeric_fields(value, &format!("{path}/{i}"), fields);
            }
        }
        Value::Object(values) => {
            for (key, value) in values {
                numeric_fields(
                    value,
                    &format!("{path}/{}", key.replace('~', "~0").replace('/', "~1")),
                    fields,
                );
            }
        }
        _ => (),
    }
}

pub fn decode_snapshot(bytes: &[u8]) -> Result<Snapshot, String> {
    let value = provider::parse_document(bytes)?;
    provider::keys(
        &value,
        &[
            "show_id",
            "epoch",
            "revision",
            "frame",
            "topology",
            "clock",
            "outputs_quiesced",
            "pa_configuration_json",
            "pa_program_buses",
            "pa_status",
            "pa_capabilities",
        ],
    )?;
    let snapshot: Snapshot = serde_json::from_value(value).map_err(|e| e.to_string())?;
    snapshot.validate()?;
    Ok(snapshot)
}
