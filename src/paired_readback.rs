//! One committed raw/Brain boundary, correlated to an authenticated connection nonce.
use crate::{audio, brain, provider};
use serde::{Deserialize, Serialize};
pub const CONTRACT: &str = "GP15-paired-readback";
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Request {
    pub contract: String,
    pub version: u32,
    pub kind: String,
    pub show_id: String,
    pub module: String,
    pub epoch: String,
    pub authenticated_session: String,
    pub writer: String,
    pub capability_generation: String,
    pub map_generation: String,
    pub query_id: String,
}
impl Request {
    pub fn validate(&self) -> Result<(), String> {
        crate::lease_maintenance::identity(
            &self.show_id,
            &self.module,
            &self.writer,
            &[
                &self.epoch,
                &self.authenticated_session,
                &self.capability_generation,
                &self.map_generation,
                &self.query_id,
            ],
        )?;
        if self.contract != CONTRACT || self.version != 1 || self.kind != "readback" {
            return Err("paired contract".into());
        }
        Ok(())
    }
    pub fn encode(&self) -> Result<Vec<u8>, String> {
        self.validate()?;
        let bytes = serde_json::to_vec(self).map_err(|e| e.to_string())?;
        if bytes.len() > provider::MAX_DOCUMENT_BYTES {
            return Err("paired capacity".into());
        }
        Ok(bytes)
    }
}
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Reply {
    pub contract: String,
    pub version: u32,
    pub state: String,
    pub reason: Option<String>,
    pub context: Request,
    pub raw: Option<audio::RenderedSnapshot>,
    pub brain: Option<brain::Snapshot>,
}
impl Reply {
    pub fn decode(bytes: &[u8]) -> Result<Self, String> {
        Self::decode_document(provider::StrictDocument::parse(bytes)?)
    }
    pub(crate) fn decode_document(document: provider::StrictDocument) -> Result<Self, String> {
        let (v, admitted_bytes) = document.into_parts();
        if admitted_bytes > provider::MAX_DOCUMENT_BYTES {
            return Err("paired capacity".into());
        }
        provider::keys(
            &v,
            &[
                "contract", "version", "state", "reason", "context", "raw", "brain",
            ],
        )?;
        if !v["raw"].is_null() {
            audio::validate_snapshot_value(&v["raw"])?;
        }
        if !v["brain"].is_null() {
            brain::validate_snapshot_value(&v["brain"])?;
        }
        let r: Self = serde_json::from_value(v).map_err(|e| e.to_string())?;
        r.validate()?;
        Ok(r)
    }
    pub fn validate(&self) -> Result<(), String> {
        self.context.validate()?;
        if self.contract != CONTRACT || self.version != 1 {
            return Err("paired contract".into());
        }
        match (
            self.state.as_str(),
            self.reason.as_deref(),
            &self.raw,
            &self.brain,
        ) {
            ("snapshot", None, Some(raw), Some(brain)) => {
                raw.validate()?;
                brain.validate(raw.authority.inputs.len(), raw.authority.monitors.len())?;
                if raw.capability_version != 2
                    || raw.authority.show_id != self.context.show_id
                    || raw.authority.epoch != self.context.epoch
                    || raw.authority.revision != brain.revision
                    || raw.frame != brain.frame
                    || raw
                        .topology
                        .as_ref()
                        .is_none_or(|t| t.map_revision.to_string() != self.context.map_generation)
                {
                    return Err("paired boundary/identity mismatch".into());
                }
            }
            ("refused", Some("identity" | "query_id" | "capacity" | "clock"), None, None) => (),
            _ => return Err("paired reply shape".into()),
        }
        Ok(())
    }
}
