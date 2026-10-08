//! GP21 actual remote stereo owner; GP05 remains a separate read-only fallback.
use crate::{audio::Context, provider};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
pub const CONTRACT: &str = "GP21-fx";
pub const DEFAULT_LEAD: u64 = 4800;
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Binding {
    pub session: String,
    pub source_epoch: String,
    pub capability_generation: String,
    pub map_generation: String,
    pub owner_instance: String,
    pub library_sha256: String,
    pub abi_version: u32,
    pub config_size: u32,
    pub status_size: u32,
    pub capabilities_size: u32,
    pub channels: [String; 2],
}
impl Binding {
    pub fn validate(&self) -> Result<(), String> {
        for c in [
            &self.session,
            &self.source_epoch,
            &self.capability_generation,
            &self.map_generation,
            &self.owner_instance,
        ] {
            if provider::counter(c)? == 0 {
                return Err("FX lifetime counter".into());
            }
        }
        if self.abi_version != 2
            || self.config_size != 104
            || self.status_size != 168
            || self.capabilities_size != 128
            || self.channels != ["foh-left", "foh-right"]
            || self.library_sha256.len() != 64
            || !self
                .library_sha256
                .bytes()
                .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
        {
            return Err("FX binding/ABI/channel order".into());
        }
        Ok(())
    }
}
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Channel {
    pub delay_ms: f64,
    pub feedback: f64,
    pub damping: f64,
    pub wet_gain: f64,
    pub bypass: bool,
}
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Configuration {
    pub channels: [Channel; 2],
}
impl Configuration {
    pub fn decode(text: &str) -> Result<Self, String> {
        let v = crate::master_eq::parse_owner_bounded(text, 4096)?;
        let c: Self = serde_json::from_value(v).map_err(|e| e.to_string())?;
        for c in &c.channels {
            if !c.delay_ms.is_finite()
                || !(1.0..=500.0).contains(&c.delay_ms)
                || !c.feedback.is_finite()
                || !(0.0..=0.85).contains(&c.feedback)
                || !c.damping.is_finite()
                || !(0.0..=0.99).contains(&c.damping)
                || !c.wet_gain.is_finite()
                || !(0.0..=1.0).contains(&c.wet_gain)
            {
                return Err("FX finite parameter bounds".into());
            }
        }
        Ok(c)
    }
}
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Observation {
    pub binding: Binding,
    pub generation: String,
    pub settled_generation: String,
    pub applied_source_frame: String,
    pub settled_source_frame: String,
    pub next_source_frame: String,
    pub reset_count: String,
    pub remaining_frames: [u32; 2],
    pub owner_json: String,
}
impl Observation {
    pub fn validate(&self) -> Result<(), String> {
        self.binding.validate()?;
        Configuration::decode(&self.owner_json)?;
        for c in [
            &self.generation,
            &self.settled_generation,
            &self.applied_source_frame,
            &self.settled_source_frame,
            &self.next_source_frame,
            &self.reset_count,
        ] {
            provider::counter(c)?;
        }
        if provider::counter(&self.settled_generation)? > provider::counter(&self.generation)?
            || self.remaining_frames.iter().any(|n| *n > 960)
        {
            return Err("FX transition evidence".into());
        }
        Ok(())
    }
    pub fn settled(&self) -> bool {
        self.generation == self.settled_generation && self.remaining_frames == [0, 0]
    }
    /// Source progress can advance; all material reviewed owner state must remain exact.
    pub fn same_basis(&self, other: &Self) -> bool {
        self.binding == other.binding
            && self.generation == other.generation
            && self.reset_count == other.reset_count
            && self.applied_source_frame == other.applied_source_frame
            && self.settled_source_frame == other.settled_source_frame
            && self.settled()
            && other.settled()
            && Configuration::decode(&self.owner_json).ok()
                == Configuration::decode(&other.owner_json).ok()
    }
}
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Mutation {
    pub binding: Binding,
    pub expected_generation: String,
    pub expected_reset_count: String,
    pub lead_frames: String,
    pub configuration_json: Option<String>,
    pub panic_mask: Option<u32>,
}
impl Mutation {
    pub fn from_basis(
        o: &Observation,
        configuration_json: Option<String>,
        panic_mask: Option<u32>,
    ) -> Self {
        Self {
            binding: o.binding.clone(),
            expected_generation: o.generation.clone(),
            expected_reset_count: o.reset_count.clone(),
            lead_frames: DEFAULT_LEAD.to_string(),
            configuration_json,
            panic_mask,
        }
    }
    pub fn validate(&self) -> Result<(), String> {
        self.binding.validate()?;
        let g = provider::counter(&self.expected_generation)?;
        let r = provider::counter(&self.expected_reset_count)?;
        let lead = provider::counter(&self.lead_frames)?;
        if !(48..=48000).contains(&lead) || !lead.is_multiple_of(48) {
            return Err("FX lead policy".into());
        }
        match (&self.configuration_json, self.panic_mask) {
            (Some(c), None) if g < u64::MAX => {
                Configuration::decode(c)?;
            }
            (None, Some(1..=3)) if r < u64::MAX => (),
            _ => return Err("FX exact operation".into()),
        };
        Ok(())
    }
    pub fn validate_basis(&self, o: &Observation) -> Result<(), String> {
        self.validate()?;
        if self.binding != o.binding
            || self.expected_generation != o.generation
            || self.expected_reset_count != o.reset_count
            || !o.settled()
        {
            return Err("FX reviewed owner basis changed".into());
        }
        Ok(())
    }
}
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Snapshot {
    pub contract: String,
    pub version: u8,
    pub state: String,
    pub show_id: String,
    pub epoch: String,
    pub revision: String,
    pub frame: String,
    pub available: bool,
    pub observation: Option<Observation>,
    pub pending: Option<String>,
}
impl Snapshot {
    pub fn decode(v: Value) -> Result<Self, String> {
        let s: Self = serde_json::from_value(v).map_err(|e| e.to_string())?;
        if s.contract != CONTRACT || s.version != 1 || s.state != "snapshot" {
            return Err("FX snapshot envelope".into());
        }
        provider::counter(&s.epoch)?;
        provider::counter(&s.revision)?;
        provider::counter(&s.frame)?;
        if let Some(t) = &s.pending {
            if provider::counter(t)? == 0 {
                return Err("FX pending ticket".into());
            }
        }
        if let Some(o) = &s.observation {
            o.validate()?;
            if o.binding.source_epoch != s.epoch
                || provider::counter(&o.next_source_frame)? > provider::counter(&s.frame)?
            {
                return Err("FX source frame/epoch".into());
            }
        }
        if s.available && s.observation.is_none() {
            return Err("available FX missing owner".into());
        }
        Ok(s)
    }
}
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Reply {
    pub contract: String,
    pub version: u8,
    pub state: String,
    pub reason: Option<String>,
    pub ticket: String,
    pub apply_frame: String,
    pub context: Context,
    pub revision: String,
    pub observation: Option<Observation>,
}
impl Reply {
    pub fn decode(v: Value) -> Result<Self, String> {
        let r: Self = serde_json::from_value(v).map_err(|e| e.to_string())?;
        r.context.validate()?;
        if r.contract != CONTRACT
            || r.version != 1
            || !matches!(
                r.state.as_str(),
                "preparing"
                    | "permitted"
                    | "applied"
                    | "settled"
                    | "refused"
                    | "cancelled"
                    | "unknown"
            )
            || r.context.writer.is_none()
            || r.context.lease.is_none()
            || provider::counter(&r.ticket)? == 0
            || provider::counter(&r.apply_frame)? == 0
            || !provider::counter(&r.apply_frame)?.is_multiple_of(48)
            || r.reason.as_ref().is_some_and(|s| s.len() > 1024)
        {
            return Err("FX reply envelope".into());
        }
        let rev = provider::counter(&r.revision)?;
        let expected = provider::counter(
            r.context
                .expected_revision
                .as_deref()
                .ok_or("FX expected revision")?,
        )?;
        if matches!(r.state.as_str(), "permitted" | "applied" | "settled")
            && (r.reason.is_some() || rev <= expected)
        {
            return Err("FX authorization revision".into());
        }
        if let Some(o) = &r.observation {
            o.validate()?;
        }
        Ok(r)
    }
}
#[derive(Clone, Debug, Default)]
pub struct State {
    pub snapshot: Option<Snapshot>,
    pub receipt: Option<u64>,
    pub evidence: Vec<Reply>,
    pub unknown: bool,
}
impl State {
    pub fn lines(&self) -> Vec<String> {
        let mut lines = vec![
            "GP21 actual Brain FOH stereo owner / GP05 fixed fallback is separate read-only health"
                .into(),
        ];
        if let Some(s) = &self.snapshot {
            lines.push(format!("Capability available={} / show {} epoch {} authority rev {} source frame {} pending {:?}",s.available,s.show_id,s.epoch,s.revision,s.frame,s.pending));
            if let Some(o) = &s.observation {
                let b = &o.binding;
                lines.push(format!("Ordered foh-left / foh-right | session {} source {} map {} capability {} owner {}",b.session,b.source_epoch,b.map_generation,b.capability_generation,b.owner_instance));
                lines.push(format!(
                    "Library {} ABI {} sizes {}/{}/{}",
                    b.library_sha256,
                    b.abi_version,
                    b.config_size,
                    b.status_size,
                    b.capabilities_size
                ));
                lines.push(format!("Generation {} settled {} reset {} | applied {} settled {} next {} remaining {:?}",o.generation,o.settled_generation,o.reset_count,o.applied_source_frame,o.settled_source_frame,o.next_source_frame,o.remaining_frames));
                if let Ok(c) = Configuration::decode(&o.owner_json) {
                    for (i, c) in c.channels.iter().enumerate() {
                        lines.push(format!("{} targets: delay {} ms feedback {} damping coefficient {} wet_gain {} bypass {}",b.channels[i],c.delay_ms,c.feedback,c.damping,c.wet_gain,c.bypass));
                    }
                }
            }
        } else {
            lines.push(
                "UNAVAILABLE / not explicitly probed, old provider or owner unavailable".into(),
            );
        }
        if let Some(r) = self.evidence.last() {
            lines.push(format!(
                "Retained {} ticket {} apply frame {} authority {} reason {:?}",
                r.state, r.ticket, r.apply_frame, r.revision, r.reason
            ));
        }
        if self.unknown {
            lines.push(
                "UNKNOWN: no DSP success claim; explicit fresh owner probe and new review required"
                    .into(),
            );
        }
        lines.push("Software lead policy 4800 frames. Bypass settles controls over20ms then drains tails; settlement is not silence. Panic clears history, not permanent mute.".into());
        lines
    }
}
pub fn body(m: &Mutation) -> Result<Value, String> {
    m.validate()?;
    serde_json::to_value(m).map_err(|e| e.to_string())
}
pub fn validate_body(kind: &str, v: &Value) -> Result<(), String> {
    match kind {
        "fx_snapshot" => provider::keys(v, &[]),
        "fx_configure" => {
            let m: Mutation = serde_json::from_value(v.clone()).map_err(|e| e.to_string())?;
            m.validate()
        }
        _ => Err("FX command".into()),
    }
}
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Action {
    Open,
    Probe,
    Scope,
    Select(u8),
    Edit,
    Text(String),
    Submit,
    Bypass,
    Panic(u8),
}
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Operation {
    Probe,
    Review {
        basis: Observation,
        configuration_json: Option<String>,
        panic_mask: Option<u32>,
    },
}
#[derive(Clone, Debug, Default)]
pub struct Editor {
    pub open: bool,
    pub selected: u8,
    pub text: Option<String>,
    pub basis: Option<Observation>,
    pub report: Vec<String>,
}
pub fn edit_channel(basis: &Observation, selected: u8, text: &str) -> Result<String, String> {
    let words: Vec<_> = text.split_whitespace().collect();
    if words.len() != 4 || selected > 1 {
        return Err("Enter delay_ms feedback damping wet_gain (four finite numbers)".into());
    }
    let values = words
        .iter()
        .map(|s| s.parse::<f64>().map_err(|_| "FX numeric entry".to_string()))
        .collect::<Result<Vec<_>, _>>()?;
    let mut c = Configuration::decode(&basis.owner_json)?;
    let channel = &mut c.channels[selected as usize];
    channel.delay_ms = values[0];
    channel.feedback = values[1];
    channel.damping = values[2];
    channel.wet_gain = values[3];
    let text = serde_json::to_string(&c).map_err(|e| e.to_string())?;
    Configuration::decode(&text)?;
    Ok(text)
}
pub fn review_lines(b: &Observation, m: &Mutation) -> String {
    format!(
        "Actual Brain stereo FX / ordered foh-left, foh-right\nReviewed basis {}\nReviewed complete target {} panic selected-mask {:?}\nBinding {}\nExpected owner generation {} reset {} / software lead {} source frames\nPartner channel preserved exactly. Bypass drains tails after20ms control settlement; settlement is not silence. Panic clears selected history, not permanent mute.\nPERMIT is irreversible authorization only. Applied and settled need exact ticket/frame/owner evidence; session loss after permit is UNKNOWN.",
        b.owner_json,
        m.configuration_json
            .as_deref()
            .unwrap_or("unchanged controls"),
        m.panic_mask,
        json!(m.binding),
        m.expected_generation,
        m.expected_reset_count,
        m.lead_frames
    )
}
