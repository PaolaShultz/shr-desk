//! Atomic extension of an existing lease. This reply is neither a grant nor held proof.
use crate::provider;
use serde::{Deserialize, Serialize};
pub const CONTRACT: &str = "GP15-lease-maintenance";
pub const MAX_BYTES: usize = 8192;
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
    pub maintenance_id: String,
    #[serde(with = "crate::scopes::one")]
    pub scope: String,
    pub lease: String,
}
pub(crate) fn identity(
    show: &str,
    module: &str,
    writer: &str,
    counters: &[&str],
) -> Result<(), String> {
    if !provider::uuid(show) || module != "audio" || !provider::id(writer) {
        return Err("atomic identity".into());
    }
    for n in counters {
        if provider::counter(n)? == 0 {
            return Err("atomic zero identity".into());
        }
    }
    Ok(())
}
impl Request {
    pub fn validate(&self) -> Result<(), String> {
        identity(
            &self.show_id,
            &self.module,
            &self.writer,
            &[
                &self.epoch,
                &self.authenticated_session,
                &self.capability_generation,
                &self.map_generation,
                &self.maintenance_id,
                &self.lease,
            ],
        )?;
        if self.contract != CONTRACT
            || self.version != 1
            || self.kind != "maintain"
            || !crate::scopes::valid(&self.scope)
        {
            return Err("maintenance contract/scope".into());
        }
        Ok(())
    }
    pub fn encode(&self) -> Result<Vec<u8>, String> {
        self.validate()?;
        let bytes = serde_json::to_vec(self).map_err(|e| e.to_string())?;
        if bytes.len() > MAX_BYTES {
            return Err("maintenance capacity".into());
        }
        Ok(bytes)
    }
}
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Outcome {
    pub revision: String,
    pub source_frame: String,
    pub lease_remaining_ms: u32,
}
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Reply {
    pub contract: String,
    pub version: u32,
    pub state: String,
    pub reason: Option<String>,
    pub context: Request,
    pub result: Option<Outcome>,
}
impl Reply {
    pub fn decode(bytes: &[u8]) -> Result<Self, String> {
        if bytes.len() > MAX_BYTES {
            return Err("maintenance capacity".into());
        }
        Self::decode_document(provider::StrictDocument::parse(bytes)?)
    }
    pub(crate) fn decode_document(document: provider::StrictDocument) -> Result<Self, String> {
        let (v, admitted_bytes) = document.into_parts();
        if admitted_bytes > MAX_BYTES {
            return Err("maintenance capacity".into());
        }
        provider::keys(
            &v,
            &[
                "contract", "version", "state", "reason", "context", "result",
            ],
        )?;
        // Request contains no optional fields; serde requires every field.
        let r: Self = serde_json::from_value(v).map_err(|e| e.to_string())?;
        r.validate()?;
        Ok(r)
    }
    pub fn validate(&self) -> Result<(), String> {
        self.context.validate()?;
        if self.contract != CONTRACT || self.version != 1 {
            return Err("maintenance contract".into());
        }
        match (self.state.as_str(), self.reason.as_deref(), &self.result) {
            ("maintained", None, Some(result)) => {
                provider::counter(&result.revision)?;
                provider::counter(&result.source_frame)?;
                if result.lease_remaining_ms != 2000 {
                    return Err("maintenance duration".into());
                }
            }
            (
                "refused",
                Some(
                    "identity" | "permission" | "scope" | "lease" | "clock" | "unavailable"
                    | "reused_id" | "expired_id" | "capacity",
                ),
                None,
            ) => (),
            _ => return Err("maintenance reply shape".into()),
        }
        Ok(())
    }
}
#[derive(Clone)]
pub(crate) struct Pending {
    pub request: Request,
    pub first_send: u64,
    pub generation: u64,
    pub deadline: u64,
    pub retry: usize,
}
