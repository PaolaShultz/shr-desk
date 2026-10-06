//! Reviewed C-AUDIO:1 commands and GP03-rendered:1 observations. No DSP.
use crate::provider::{self, Snapshot, Target};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use std::collections::BTreeSet;

fn fail<T>(message: &str) -> Result<T, String> {
    Err(message.into())
}
fn optional_counter(v: &Option<String>) -> Result<(), String> {
    if let Some(v) = v {
        provider::counter(v)?;
    }
    Ok(())
}
#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct Context {
    pub show_id: String,
    pub module: String,
    pub epoch: String,
    pub writer: Option<String>,
    pub lease: Option<String>,
    pub request_id: Option<String>,
    pub expected_revision: Option<String>,
}
impl Context {
    pub(crate) fn validate(&self) -> Result<(), String> {
        if !provider::uuid(&self.show_id) || self.module != "audio" {
            return fail("show/module");
        }
        provider::counter(&self.epoch)?;
        for v in [&self.lease, &self.request_id, &self.expected_revision] {
            optional_counter(v)?;
        }
        if self.writer.as_ref().is_some_and(|x| !provider::id(x)) {
            return fail("writer");
        }
        if self.writer.is_none() {
            if self.lease.is_some() || self.request_id.is_some() || self.expected_revision.is_some()
            {
                return fail("snapshot context");
            }
        } else if self
            .request_id
            .as_deref()
            .is_none_or(|s| provider::counter(s).is_err() || s == "0")
            || self.expected_revision.is_none()
            || self.lease.as_deref() == Some("0")
        {
            return fail("writer context");
        }
        Ok(())
    }
}
/// Rounded linear amplitude times 1e9, not an integer dB observation.
#[derive(Clone, Copy, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(transparent)]
pub struct Nanogain(pub u64);
#[derive(Clone, Debug, Deserialize, Serialize, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct Coefficients {
    pub input: String,
    pub current_nanogain: Vec<Nanogain>,
    pub ramp_target_nanogain: Vec<Nanogain>,
    pub held_nanogain: Vec<Option<Nanogain>>,
}
#[derive(Clone, Debug, Deserialize, Serialize, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct RenderedSnapshot {
    pub capability: String,
    pub capability_version: u8,
    pub rendered_application: bool,
    pub release_commit: bool,
    pub frame: String,
    pub faulted: bool,
    pub protection: String,
    pub meters: Value,
    pub authority: Snapshot,
    pub coefficients: Vec<Coefficients>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub topology: Option<crate::topology::Topology>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub clock: Option<crate::topology::Clock>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub resources: Option<crate::topology::Resources>,
}
impl RenderedSnapshot {
    fn validate(&self) -> Result<(), String> {
        capability(&self.capability, self.capability_version)?;
        if !self.rendered_application
            || !self.release_commit
            || self.protection != "offline-unprotected"
            || !self.meters.is_null()
        {
            return fail("render capability/protection/meters");
        }
        provider::counter(&self.frame)?;
        if self.capability_version == 1 {
            if self.topology.is_some() || self.clock.is_some() || self.resources.is_some() {
                return fail("legacy topology fields");
            }
        } else {
            let topology = self.topology.as_ref().ok_or("dynamic topology required")?;
            topology.validate()?;
            self.resources
                .as_ref()
                .ok_or("dynamic resources required")?
                .validate()?;
            let clock = self.clock.as_ref().ok_or("dynamic clock required")?;
            clock.validate()?;
            if clock.epoch != provider::counter(&self.authority.epoch)?
                || topology
                    .inputs
                    .iter()
                    .map(|p| &p.id)
                    .collect::<BTreeSet<_>>()
                    != self.authority.inputs.iter().collect()
                || topology.monitors != self.authority.monitors.len()
            {
                return fail("topology/authority/clock coherence");
            }
        }
        // The nested authority intentionally remains the separately reviewed GP02 metadata body.
        let bytes = serde_json::to_vec(&self.authority_value()).map_err(|e| e.to_string())?;
        let a = provider::decode_version(&bytes, self.capability_version)?;
        if a.page != 0
            || a.page_count != 1
            || a.parameters.len() != a.inputs.len() * (3 + a.monitors.len())
            || self.coefficients.len() != a.inputs.len()
        {
            return fail("incomplete rendered snapshot");
        }
        let mut seen = BTreeSet::new();
        for c in &self.coefficients {
            if !a.inputs.contains(&c.input) || !seen.insert(&c.input) {
                return fail("coefficient identity");
            }
            let width = 4 + a.monitors.len();
            if c.current_nanogain.len() != width
                || c.ramp_target_nanogain.len() != width
                || c.held_nanogain.len() != width
            {
                return fail("coefficient lane shape");
            }
            for (i, (current, target)) in c
                .current_nanogain
                .iter()
                .zip(&c.ramp_target_nanogain)
                .enumerate()
            {
                let max = if i == 0 || i >= 4 {
                    3_981_071_706
                } else {
                    1_000_000_000
                };
                if current.0 > max
                    || target.0 > max
                    || c.held_nanogain[i].is_some_and(|x| x.0 > max)
                {
                    return fail("nanogain range");
                }
            }
        }
        Ok(())
    }
    fn authority_value(&self) -> Value {
        serde_json::to_value(&self.authority).expect("serializable authority")
    }
}
fn capability(name: &str, version: u8) -> Result<(), String> {
    if name != "GP03-rendered" || !matches!(version, 1 | 2) {
        return fail("unsupported capability");
    }
    Ok(())
}
pub fn target_value(t: &Target) -> Value {
    let mut v = json!({"parameter":t.parameter,"input":t.input});
    if let Some(m) = &t.monitor {
        v["monitor"] = m.clone().into();
    }
    v
}
#[derive(Clone, Debug, Deserialize, Serialize, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct Destination {
    pub target: Target,
    pub value: Value,
}
#[derive(Clone, Debug, Deserialize, Serialize, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct Preview {
    pub token: String,
    pub revision: String,
    #[serde(with = "crate::scopes::one")]
    pub scope: String,
    pub remaining_ms: u64,
    pub ramp_frames: u64,
    pub destinations: Vec<Destination>,
}
impl Preview {
    fn validate(&self, version: u8) -> Result<(), String> {
        if !provider::id(&self.token)
            || !scope(&self.scope)
            || self.remaining_ms > 2000
            || self.ramp_frames != 240
            || self.destinations.is_empty()
            || self.destinations.len() > 64
        {
            return fail("preview domain/capacity");
        }
        provider::counter(&self.revision)?;
        let mut seen = BTreeSet::new();
        for d in &self.destinations {
            provider::target_version(&d.target, version)?;
            provider::value(&d.target, &d.value)?;
            if d.target.parameter == "mute"
                || target_scope(&d.target) != self.scope
                || !seen.insert(&d.target)
            {
                return fail("preview destination scope/duplicate");
            }
        }
        Ok(())
    }
}
#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct OutcomeBody {
    pub revision: String,
    pub reason: Option<String>,
    pub lease_remaining_ms: Option<u64>,
    pub granted_lease: Option<String>,
    #[serde(with = "crate::scopes::optional")]
    pub scope: Option<String>,
    pub preview: Option<Preview>,
    pub snapshot: Option<Snapshot>,
    pub effective_frame: Option<String>,
    pub ramp_frames: Option<u64>,
}
#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct Outcome {
    pub contract: String,
    pub version: u8,
    #[serde(flatten)]
    pub context: Context,
    pub kind: String,
    pub body: OutcomeBody,
}
#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct Reply {
    pub context: Context,
    pub capability: String,
    pub capability_version: u8,
    pub state: String,
    pub ticket: Option<String>,
    pub effective_frame: Option<String>,
    pub ramp_frames: Option<u64>,
    pub outcome: Option<Outcome>,
    pub snapshot: Option<RenderedSnapshot>,
}
pub fn decode_command_body(bytes: &[u8]) -> Result<Value, String> {
    provider::parse(bytes)
}
pub fn decode_snapshot(bytes: &[u8]) -> Result<RenderedSnapshot, String> {
    let v = provider::parse_document(bytes)?;
    if v["capability_version"] != 2 && bytes.len() > provider::MAX_BYTES {
        return fail("legacy frame capacity");
    }
    validate_snapshot_value(&v)?;
    let s: RenderedSnapshot = serde_json::from_value(v).map_err(|e| e.to_string())?;
    s.validate()?;
    Ok(s)
}
fn validate_snapshot_value(v: &Value) -> Result<(), String> {
    let mut fields = vec![
        "capability",
        "capability_version",
        "rendered_application",
        "release_commit",
        "frame",
        "faulted",
        "protection",
        "meters",
        "authority",
        "coefficients",
    ];
    if v["capability_version"] == 2 {
        fields.extend(["topology", "clock", "resources"]);
    }
    provider::keys(v, &fields)?;
    provider::decode_version(
        &serde_json::to_vec(&v["authority"]).map_err(|e| e.to_string())?,
        v["capability_version"].as_u64().ok_or("version")? as u8,
    )?;
    for c in v["coefficients"].as_array().ok_or("coefficients array")? {
        provider::keys(
            c,
            &[
                "input",
                "current_nanogain",
                "ramp_target_nanogain",
                "held_nanogain",
            ],
        )?;
    }
    Ok(())
}
pub fn decode_reply(bytes: &[u8]) -> Result<Reply, String> {
    let v = provider::parse_document(bytes)?;
    if v["capability_version"] != 2 && bytes.len() > provider::MAX_BYTES {
        return fail("legacy frame capacity");
    }
    provider::keys(
        &v,
        &[
            "context",
            "capability",
            "capability_version",
            "state",
            "ticket",
            "effective_frame",
            "ramp_frames",
            "outcome",
            "snapshot",
        ],
    )?;
    validate_context_value(&v["context"])?;
    if !v["snapshot"].is_null() {
        validate_snapshot_value(&v["snapshot"])?;
    }
    if !v["outcome"].is_null() {
        provider::keys(
            &v["outcome"],
            &[
                "contract",
                "version",
                "show_id",
                "module",
                "epoch",
                "writer",
                "lease",
                "request_id",
                "expected_revision",
                "kind",
                "body",
            ],
        )?;
        provider::keys(
            &v["outcome"]["body"],
            &[
                "revision",
                "reason",
                "lease_remaining_ms",
                "granted_lease",
                "scope",
                "preview",
                "snapshot",
                "effective_frame",
                "ramp_frames",
            ],
        )?;
        if !v["outcome"]["body"]["snapshot"].is_null() {
            provider::decode_version(
                &serde_json::to_vec(&v["outcome"]["body"]["snapshot"])
                    .map_err(|e| e.to_string())?,
                v["capability_version"].as_u64().ok_or("version")? as u8,
            )?;
        }
    }
    if !v["outcome"]["body"]["preview"].is_null() {
        let preview = &v["outcome"]["body"]["preview"];
        provider::keys(
            preview,
            &[
                "token",
                "revision",
                "scope",
                "remaining_ms",
                "ramp_frames",
                "destinations",
            ],
        )?;
        for d in preview["destinations"]
            .as_array()
            .ok_or("preview destinations")?
        {
            provider::keys(d, &["target", "value"])?;
            provider::keys(
                &d["target"],
                if d["target"]["parameter"] == "send" {
                    &["parameter", "input", "monitor"]
                } else {
                    &["parameter", "input"]
                },
            )?;
        }
    }
    let r: Reply = serde_json::from_value(v).map_err(|e| e.to_string())?;
    capability(&r.capability, r.capability_version)?;
    r.context.validate()?;
    optional_counter(&r.ticket)?;
    optional_counter(&r.effective_frame)?;
    if r.ticket.as_deref() == Some("0") || r.ramp_frames.is_some_and(|n| n != 240) {
        return fail("ticket/ramp");
    }
    match r.state.as_str() {
        "backpressure"
            if r.ticket.is_none()
                && r.effective_frame.is_none()
                && r.ramp_frames.is_none()
                && r.outcome.is_none()
                && r.snapshot.is_none() => {}
        "pending"
            if r.ticket.is_some()
                && r.effective_frame.is_some()
                && r.ramp_frames == Some(240)
                && r.outcome.is_none()
                && r.snapshot.is_none() => {}
        "final" if r.outcome.is_some() => {}
        _ => return fail("incoherent reply state"),
    }
    if let Some(o) = &r.outcome {
        if o.contract != "C-AUDIO"
            || o.version != r.capability_version
            || o.context != r.context
            || !matches!(
                o.kind.as_str(),
                "applied" | "rejected" | "conflict" | "busy"
            )
        {
            return fail("outcome identity/kind");
        }
        provider::counter(&o.body.revision)?;
        if let Some(s) = &o.body.snapshot
            && (s.show_id != r.context.show_id
                || s.epoch != r.context.epoch
                || s.revision != o.body.revision)
        {
            return fail("nested authority snapshot identity/revision");
        }
        optional_counter(&o.body.granted_lease)?;
        if o.body
            .lease_remaining_ms
            .is_some_and(|n| n == 0 || n > 2000)
            || o.body.granted_lease.as_deref() == Some("0")
            || o.body.scope.as_deref().is_some_and(|s| !scope(s))
            || o.body.effective_frame.is_some()
            || o.body.ramp_frames.is_some_and(|n| n != 240)
        {
            return fail("outcome lease/timing");
        }
        if o.kind == "applied" && o.body.reason.is_some()
            || o.kind != "applied" && o.body.reason.as_ref().is_none_or(|s| !provider::id(s))
        {
            return fail("outcome reason");
        }
        if o.body.ramp_frames.is_some()
            && (o.kind != "applied"
                || o.body.preview.is_some()
                || o.body.lease_remaining_ms.is_some()
                || r.ticket.is_some())
        {
            return fail("invalid proposal ramp metadata");
        }
        if o.kind != "applied"
            && (o.body.granted_lease.is_some()
                || o.body.scope.is_some()
                || o.body.lease_remaining_ms.is_some()
                || o.body.preview.is_some()
                || o.body.snapshot.is_some())
        {
            return fail("failure carried applied capability");
        }
        if o.body.lease_remaining_ms.is_some() != o.body.scope.is_some()
            || o.body.granted_lease.is_some()
                && (o.body.lease_remaining_ms != Some(2000)
                    || r.context.lease.is_some()
                    || r.context.request_id.as_deref() != Some("1"))
        {
            return fail("grant metadata combination");
        }
        if o.body.lease_remaining_ms.is_some() && o.body.lease_remaining_ms != Some(2000) {
            return fail("unsupported lease duration");
        }
        if let Some(p) = &o.body.preview {
            p.validate(r.capability_version)?;
            if p.revision != o.body.revision
                || r.context.writer.is_none()
                || r.context.lease.is_none()
                || o.body.scope.is_some()
                || o.body.snapshot.is_some()
            {
                return fail("preview outcome binding");
            }
        }
        if o.body.preview.is_some()
            && (o.body.granted_lease.is_some()
                || o.body.lease_remaining_ms.is_some()
                || r.context.lease.is_none())
        {
            return fail("preview grant combination");
        }
        if r.ticket.is_some() != r.effective_frame.is_some()
            || r.ticket.is_some() != r.ramp_frames.is_some()
        {
            return fail("final timing");
        }
        if let Some(s) = &r.snapshot {
            s.validate()?;
            if s.capability_version != r.capability_version {
                return fail("nested snapshot version mismatch");
            }
            if s.authority.show_id != r.context.show_id
                || s.authority.epoch != r.context.epoch
                || s.authority.revision != o.body.revision
            {
                return fail("snapshot outcome binding");
            }
            if let Some(f) = &r.effective_frame
                && provider::counter(&s.frame)? < provider::counter(f)?
            {
                return fail("snapshot before commit");
            }
        }
    }
    Ok(r)
}
pub(crate) fn validate_context_value(v: &Value) -> Result<(), String> {
    provider::keys(
        v,
        &[
            "show_id",
            "module",
            "epoch",
            "writer",
            "lease",
            "request_id",
            "expected_revision",
        ],
    )
}
fn scope(s: &str) -> bool {
    crate::scopes::valid(s)
}
fn target_scope(t: &Target) -> String {
    provider::target_scope(t)
}
#[derive(Clone, Debug, PartialEq)]
pub struct Request {
    pub version: u8,
    pub context: Context,
    pub kind: String,
    pub body: Value,
}
impl Request {
    pub fn encode(&self) -> Result<Vec<u8>, String> {
        self.context.validate()?;
        if !matches!(self.version, 1 | 2) {
            return fail("request version");
        }
        let c = &self.context;
        if self.kind.starts_with("processing_") && c.epoch == "0" {
            return fail("processing epoch must be nonzero");
        }
        if self.kind == "processing_snapshot" {
            provider::keys(&self.body, &[])?;
            if c.writer.is_some() {
                return fail("processing snapshot must be read-only");
            }
        } else if self.kind == "processing_set" {
            crate::processing::validate_body_version(&self.body, self.version)?;
            if c.writer.is_none() || c.lease.is_none() {
                return fail("processing mutation authority");
            }
        } else if self.kind.starts_with("processing_") {
            return fail("unknown processing request");
        }
        if crate::structure::is_kind(&self.kind) {
            if self.version != 2 {
                return fail("structural controls require dynamic authority");
            }
            crate::structure::validate_body(&self.kind, &self.body, None)?;
            if (self.kind == "structural_snapshot") != c.writer.is_none() {
                return fail("structural writer context");
            }
        }
        if crate::brain::is_kind(&self.kind) {
            if self.version != 2 || c.epoch == "0" {
                return fail("Brain requires C-AUDIO2 identity");
            }
            crate::brain::validate_body(&self.kind, &self.body, None, 4096, 4096)?;
            if (self.kind == "brain_snapshot") != c.writer.is_none() {
                return fail("Brain writer context");
            }
            if self.kind != "brain_snapshot"
                && (c.lease.is_none() || c.request_id.is_none() || c.expected_revision.is_none())
            {
                return fail("Brain mutation authority");
            }
        }
        if self.kind == "device_snapshot" {
            if self.version != 2 || c.writer.is_some() {
                return fail("device snapshot session");
            }
            return serde_json::to_vec(&json!({"contract":"GP15-device","version":1,"kind":"device_snapshot","writer":null})).map_err(|e|e.to_string());
        }
        if self.kind == "device_configure" {
            if self.version != 2
                || c.writer.is_none()
                || c.lease.is_none()
                || c.request_id.is_none()
                || c.expected_revision.is_none()
            {
                return fail("device configuration authority");
            }
            crate::provider::keys(&self.body, &["config", "device_identity"])?;
            let config = crate::brain_device::Config::decode(self.body["config"].clone())?;
            return serde_json::to_vec(&json!({"contract":"GP15-device","version":1,"show_id":c.show_id,"module":c.module,"epoch":c.epoch,"writer":c.writer,"lease":c.lease,"request_id":c.request_id,"expected_revision":c.expected_revision,"kind":"device_configure","body":{},"config":config})).map_err(|e|e.to_string());
        }
        let contract = if crate::brain::is_kind(&self.kind) {
            "GP15-brain"
        } else if crate::structure::is_kind(&self.kind) {
            "GP14-structure"
        } else if self.kind.starts_with("processing_") {
            "GP07-processing"
        } else {
            "C-AUDIO"
        };
        let v = json!({"contract":contract,"version":if matches!(contract,"GP14-structure"|"GP15-brain") {1} else if contract == "GP07-processing" {self.version + 1} else {self.version},"show_id":c.show_id,"module":c.module,"epoch":c.epoch,"writer":c.writer,"lease":c.lease,"request_id":c.request_id,"expected_revision":c.expected_revision,"kind":if contract=="GP15-brain" {crate::brain::wire_kind(&self.kind)} else {&self.kind},"body":self.body});
        let b = serde_json::to_vec(&v).map_err(|e| e.to_string())?;
        if b.len() > provider::MAX_BYTES {
            return fail("request capacity");
        }
        Ok(b)
    }
}
const RETRY_DELAYS_MS: [u64; 3] = [100, 250, 500];
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PendingState {
    Sent,
    Backpressure,
    Accepted,
    Uncertain,
}
#[derive(Clone, Debug)]
pub struct Pending {
    pub request: Request,
    pub first_send: u64,
    pub state: PendingState,
    pub ticket: Option<String>,
    timing: Option<(String, u64)>,
    observed_frame: u64,
    retry: usize,
}
#[derive(Clone, Debug)]
struct Lease {
    token: String,
    deadline: u64,
    renew_at: u64,
}
/// One writer, one immutable outstanding request, injected monotonic milliseconds.
#[derive(Clone)]
pub struct Session {
    version: u8,
    show: String,
    epoch: u64,
    writer: String,
    scope: String,
    next_id: u64,
    pub snapshot: Option<RenderedSnapshot>,
    receipt: Option<u64>,
    pub device: Option<crate::brain_device::Snapshot>,
    pub device_final: Option<crate::brain_device::Reply>,
    device_receipt: Option<u64>,
    device_ticket: Option<u64>,
    device_replies: std::collections::VecDeque<[u8; 32]>,
    pub brain: Option<crate::brain::Snapshot>,
    pub brain_final: Option<crate::brain::Reply>,
    brain_receipt: Option<u64>,
    brain_closing: Option<Request>,
    brain_replies: std::collections::VecDeque<[u8; 32]>,
    pub processing: Option<crate::processing::Snapshot>,
    processing_receipt: Option<u64>,
    pub structural: Option<crate::structure::Snapshot>,
    pub structural_final: Option<crate::structure::Reply>,
    structural_receipt: Option<u64>,
    // Bounded fingerprints of strictly validated replies in this connection only.
    structural_replies: std::collections::VecDeque<[u8; 32]>,
    /// Last correlated successful final, retained independently of polling snapshots.
    pub processing_final: Option<crate::processing::Reply>,
    lease: Option<Lease>,
    pub pending: Option<Pending>,
    pub last_result: String,
    needs_snapshot: bool,
    preview: Option<(Preview, u64, u64)>,
    generation: u64,
    armed: bool,
    context_exhausted: bool,
}
impl Session {
    pub fn new(show: &str, epoch: u64, writer: &str, scoped: &str) -> Result<Self, String> {
        Self::new_version(show, epoch, writer, scoped, 1)
    }
    pub fn new_version(
        show: &str,
        epoch: u64,
        writer: &str,
        scoped: &str,
        version: u8,
    ) -> Result<Self, String> {
        if !matches!(version, 1 | 2)
            || (version == 1 && !matches!(scoped, "foh" | "monitor1" | "monitor2"))
            || !provider::uuid(show)
            || !provider::id(writer)
            || !scope(scoped)
        {
            return fail("session identity/scope");
        }
        Ok(Self {
            version,
            show: show.into(),
            epoch,
            writer: writer.into(),
            scope: scoped.into(),
            next_id: 1,
            snapshot: None,
            receipt: None,
            device: None,
            device_final: None,
            device_receipt: None,
            device_ticket: None,
            device_replies: std::collections::VecDeque::new(),
            brain: None,
            brain_final: None,
            brain_receipt: None,
            brain_closing: None,
            brain_replies: std::collections::VecDeque::new(),
            processing: None,
            processing_receipt: None,
            structural: None,
            structural_final: None,
            structural_receipt: None,
            structural_replies: std::collections::VecDeque::new(),
            processing_final: None,
            lease: None,
            pending: None,
            last_result: "read-only; snapshot required".into(),
            needs_snapshot: true,
            preview: None,
            generation: 0,
            armed: false,
            context_exhausted: false,
        })
    }
    pub fn snapshot_request(&self) -> Request {
        Request {
            version: self.version,
            context: Context {
                show_id: self.show.clone(),
                module: "audio".into(),
                epoch: self.epoch.to_string(),
                writer: None,
                lease: None,
                request_id: None,
                expected_revision: None,
            },
            kind: "snapshot".into(),
            body: json!({}),
        }
    }
    pub fn ingest_snapshot(&mut self, s: RenderedSnapshot, now: u64) -> Result<bool, String> {
        s.validate()?;
        if s.capability_version != self.version {
            return fail("snapshot version differs from selected session");
        }
        if s.authority.show_id != self.show || provider::counter(&s.authority.epoch)? != self.epoch
        {
            return fail("wrong snapshot session");
        }
        if self.processing.as_ref().is_some_and(|p| {
            provider::counter(&s.authority.revision).unwrap()
                < provider::counter(&p.revision).unwrap()
        }) {
            return Ok(false);
        }
        if let Some(old) = &self.snapshot
            && (provider::counter(&s.authority.revision)?
                < provider::counter(&old.authority.revision)?
                || provider::counter(&s.authority.sequence)?
                    < provider::counter(&old.authority.sequence)?
                || provider::counter(&s.frame)? < provider::counter(&old.frame)?)
        {
            return Ok(false);
        }
        if self
            .snapshot
            .as_ref()
            .is_some_and(|old| old.authority.revision != s.authority.revision)
        {
            self.preview = None;
        }
        self.snapshot = Some(s);
        self.receipt = Some(now);
        self.needs_snapshot = false;
        Ok(true)
    }
    pub(crate) fn snapshot_age(&self, now: u64) -> Option<u64> {
        self.receipt.map(|t| now.saturating_sub(t))
    }
    pub fn fresh(&self, now: u64) -> bool {
        !self.context_exhausted
            && !self.needs_snapshot
            && self.receipt.is_some_and(|t| now >= t && now - t <= 250)
            && self.snapshot.as_ref().is_some_and(|s| !s.faulted)
    }
    pub fn input_released(&mut self) {
        self.armed = !self.context_exhausted;
    }
    pub fn context_changed(&mut self) {
        self.armed = false;
        self.preview = None;
        if let Some(next) = self.generation.checked_add(1) {
            self.generation = next;
        } else {
            self.context_exhausted = true;
        }
    }
    pub fn disconnect(&mut self) {
        self.pending = None;
        self.lease = None;
        self.receipt = None;
        self.device_receipt = None;
        self.device_ticket = None;
        self.device_replies.clear();
        self.brain_receipt = None;
        self.brain_closing = None;
        self.brain_replies.clear();
        self.brain_final = None;
        self.processing_receipt = None;
        self.structural_receipt = None;
        self.structural_replies.clear();
        self.processing_final = None;
        self.needs_snapshot = true;
        self.context_changed();
        self.last_result = "disconnected; pending intent discarded, mix unknown".into();
    }
    pub fn reconnect(&mut self, epoch: u64, writer: &str) -> Result<(), String> {
        if !provider::id(writer) || writer == self.writer {
            return fail("reconnect requires fresh writer");
        }
        self.disconnect();
        if self.epoch != epoch {
            self.snapshot = None;
            self.processing = None;
        }
        self.epoch = epoch;
        self.writer = writer.into();
        self.next_id = 1;
        Ok(())
    }
    pub fn generation(&self) -> u64 {
        self.generation
    }
    pub fn preview(&self, now: u64) -> Option<&Preview> {
        self.preview
            .as_ref()
            .filter(|(_, deadline, generation)| now < *deadline && *generation == self.generation)
            .map(|(p, _, _)| p)
    }
    pub fn lease_deadline(&self) -> Option<u64> {
        self.lease.as_ref().map(|l| l.deadline)
    }
    pub fn renewal_due(&self, now: u64) -> bool {
        self.pending.is_none()
            && self
                .lease
                .as_ref()
                .is_some_and(|l| now >= l.renew_at && now < l.deadline)
    }
    pub fn device_request(&self) -> Request {
        let mut r = self.snapshot_request();
        r.kind = "device_snapshot".into();
        r
    }
    pub fn device_fresh(&self, now: u64) -> bool {
        self.fresh(now)
            && self
                .device_receipt
                .is_some_and(|t| now >= t && now - t <= 250)
            && self
                .device
                .as_ref()
                .is_some_and(|s| s.connected && s.observation.is_some())
    }
    /// The pin is local intent metadata; GP15-device wire bytes remain unchanged.
    pub(crate) fn validate_device_intent(&self, body: &Value, now: u64) -> Result<(), String> {
        provider::keys(body, &["config", "device_identity"])?;
        let pin: (u64, u64) = serde_json::from_value(body["device_identity"].clone())
            .map_err(|_| "device identity pin required")?;
        if !self.device_fresh(now)
            || self
                .device
                .as_ref()
                .and_then(crate::brain_device::Snapshot::identity)
                != Some(pin)
        {
            return fail(
                "device epoch/map changed or stale; discard intent and review fresh configuration",
            );
        }
        crate::brain_device::Config::decode(body["config"].clone())?;
        Ok(())
    }
    pub(crate) fn invalidate_device(&mut self) {
        self.device_receipt = None;
    }
    pub fn dispatch_device(&mut self, bytes: &[u8], now: u64) -> Result<(), String> {
        use sha2::{Digest, Sha256};
        match crate::brain_device::decode(bytes)? {
            crate::brain_device::Message::Snapshot(s) => {
                if let (Some(old), Some(new)) = (
                    self.device.as_ref().and_then(|s| s.observation.as_ref()),
                    s.observation.as_ref(),
                ) {
                    if old.brain_epoch == new.brain_epoch
                        && old.brain_map == new.brain_map
                        && new.frame < old.frame
                    {
                        return fail("regressive device observation");
                    }
                    if old.brain_epoch == new.brain_epoch
                        && old.brain_map == new.brain_map
                        && new.frame == old.frame
                        && self.device.as_ref().and_then(|s| s.observed_ms) == s.observed_ms
                    {
                        // Repeated cached device reports do not refresh authority.
                        if !s.connected {
                            self.device_receipt = None;
                        }
                        self.device = Some(s);
                        return Ok(());
                    }
                }
                self.device_receipt = Some(now);
                self.device = Some(s);
                Ok(())
            }
            crate::brain_device::Message::Reply(r) => {
                let digest: [u8; 32] = Sha256::digest(bytes).into();
                if self.device_replies.contains(&digest) {
                    return Ok(());
                }
                let p = self
                    .pending
                    .as_ref()
                    .filter(|p| {
                        p.request.kind == "device_configure" && p.request.context == r.context
                    })
                    .ok_or("device reply correlation")?;
                if self.device_ticket.is_some_and(|t| t != r.ticket) {
                    return fail("device ticket changed");
                }
                let expected = provider::counter(
                    r.context
                        .expected_revision
                        .as_deref()
                        .ok_or("device revision")?,
                )?;
                let revision = provider::counter(&r.revision)?;
                if r.state == "pending" && revision != expected
                    || matches!(r.state.as_str(), "accepted_intent" | "applied_device")
                        && revision <= expected
                {
                    return fail("device source boundary revision");
                }
                if r.state == "applied_device"
                    && serde_json::to_value(&r.observation.as_ref().unwrap().config)
                        .map_err(|e| e.to_string())?
                        != p.request.body["config"]
                {
                    return fail("device applied mapping differs from request");
                }
                self.device_ticket = Some(r.ticket);
                self.last_result = format!(
                    "device {} ticket {} / revision {} / {:?}",
                    r.state, r.ticket, r.revision, r.reason
                );
                if matches!(r.state.as_str(), "applied_device" | "failed_device") {
                    self.pending = None;
                    self.device_ticket = None;
                    self.device_receipt = None;
                    self.needs_snapshot = true;
                }
                self.device_final = Some(r);
                if self.device_replies.len() == 128 {
                    self.device_replies.pop_front();
                }
                self.device_replies.push_back(digest);
                Ok(())
            }
        }
    }
    pub fn brain_request(&self) -> Request {
        let mut r = self.snapshot_request();
        r.kind = "brain_snapshot".into();
        r
    }
    pub fn brain_fresh(&self, now: u64) -> bool {
        self.fresh(now)
            && self
                .brain_receipt
                .is_some_and(|t| now >= t && now - t <= 250)
            && self.brain.as_ref().is_some_and(|b| {
                self.snapshot
                    .as_ref()
                    .is_some_and(|s| s.authority.revision == b.revision)
            })
    }
    pub(crate) fn invalidate_brain_observation(&mut self) {
        self.brain_receipt = None;
    }
    pub fn brain_age(&self, now: u64) -> Option<u64> {
        self.brain_receipt.map(|t| now.saturating_sub(t))
    }
    pub fn ingest_brain(
        &mut self,
        snapshot: crate::brain::Snapshot,
        now: u64,
    ) -> Result<bool, String> {
        let raw = self
            .snapshot
            .as_ref()
            .ok_or("Brain needs actual topology")?;
        snapshot.validate(raw.authority.inputs.len(), raw.authority.monitors.len())?;
        if self.version != 2 {
            return fail("Brain requires dynamic session");
        }
        if let Some(old) = &self.brain {
            if provider::counter(&snapshot.revision)? < provider::counter(&old.revision)?
                || provider::counter(&snapshot.frame)? < provider::counter(&old.frame)?
            {
                return fail("regressive Brain observation");
            }
            if snapshot.revision == old.revision && snapshot.frame == old.frame {
                return Ok(false);
            }
        }
        self.brain = Some(snapshot);
        self.brain_receipt = Some(now);
        Ok(true)
    }
    /// Closing has its own bounded correlation lane and never waits for an ordinary pending command.
    /// It cannot open a path, renew a hold, or restore expired authority.
    pub fn brain_close_request(&mut self, now: u64) -> Result<Request, String> {
        if let Some(r) = &self.brain_closing {
            return Ok(r.clone());
        }
        let lease = self
            .lease
            .as_ref()
            .filter(|l| now < l.deadline)
            .ok_or("no live close authority; provider deadman bounds closure")?;
        if self.version != 2 || self.scope != "talkback_destinations" {
            return fail("talkback close scope");
        }
        let id = self.next_id;
        self.next_id = id.checked_add(1).ok_or("request counter exhausted")?;
        let mut r = self.brain_request();
        if let Some(generation) = self.brain.as_ref().and_then(|s| s.held_generation.as_ref()) {
            r.kind = "brain_release".into();
            r.body = json!({"generation":generation});
        } else {
            r.kind = "brain_close".into();
        }
        r.context.writer = Some(self.writer.clone());
        r.context.lease = Some(lease.token.clone());
        r.context.request_id = Some(id.to_string());
        r.context.expected_revision = Some(
            self.snapshot
                .as_ref()
                .ok_or("snapshot")?
                .authority
                .revision
                .clone(),
        );
        r.encode()?;
        self.brain_closing = Some(r.clone());
        Ok(r)
    }
    pub fn dispatch_brain(&mut self, reply: crate::brain::Reply, now: u64) -> Result<(), String> {
        use sha2::{Digest, Sha256};
        let bytes = serde_json::to_vec(&reply).map_err(|e| e.to_string())?;
        let reply = crate::brain::decode_reply(&bytes)?;
        let fingerprint: [u8; 32] = Sha256::digest(&bytes).into();
        if self.brain_replies.contains(&fingerprint) {
            return Ok(());
        }
        if reply.context == self.brain_request().context {
            if reply.state != "snapshot" {
                return fail("Brain snapshot reply required");
            }
            self.ingest_brain(reply.snapshot.ok_or("Brain snapshot missing")?, now)?;
            return Ok(());
        }
        let closing = self
            .brain_closing
            .as_ref()
            .is_some_and(|r| r.context == reply.context);
        let ordinary = self.pending.as_ref().is_some_and(|p| {
            crate::brain::is_kind(&p.request.kind) && p.request.context == reply.context
        });
        if !closing && !ordinary {
            return fail("unknown Brain reply correlation");
        }
        let expected = provider::counter(
            reply
                .context
                .expected_revision
                .as_deref()
                .ok_or("Brain revision")?,
        )?;
        if reply.state == "pending" {
            if provider::counter(&reply.revision)? != expected {
                return fail("Brain pending revision");
            }
            if ordinary {
                self.pending.as_mut().unwrap().state = PendingState::Accepted;
            }
        } else {
            if reply.state != "final"
                || (reply.reason.is_none()
                    && (reply.applied_frame.is_none()
                        || provider::counter(&reply.revision)?
                            != expected.checked_add(1).ok_or("Brain revision exhausted")?))
            {
                return fail("Brain final revision/frame");
            }
            if let Some(snapshot) = reply.snapshot.clone() {
                self.ingest_brain(snapshot, now)?;
            } else {
                self.brain_receipt = None;
            }
            self.last_result = match &reply.reason {
                Some(r) => format!("Brain REFUSED: {r}"),
                None => format!(
                    "Brain applied revision {} at frame {}",
                    reply.revision,
                    reply.applied_frame.as_deref().unwrap_or("unknown")
                ),
            };
            self.brain_final = Some(reply);
            if closing {
                self.brain_closing = None;
            }
            if ordinary {
                self.pending = None;
            }
            self.needs_snapshot = true;
            self.preview = None;
        }
        if self.brain_replies.len() == 128 {
            self.brain_replies.pop_front();
        }
        self.brain_replies.push_back(fingerprint);
        Ok(())
    }
    pub fn structural_request(&self) -> Request {
        let mut request = self.snapshot_request();
        request.kind = "structural_snapshot".into();
        request
    }
    pub fn ingest_structural(
        &mut self,
        snapshot: crate::structure::Snapshot,
        now: u64,
    ) -> Result<bool, String> {
        snapshot.validate()?;
        if self.version != 2
            || snapshot.show_id != self.show
            || provider::counter(&snapshot.epoch)? != self.epoch
        {
            return fail("structural session");
        }
        if self.structural.as_ref().is_some_and(|old| {
            provider::counter(&snapshot.revision).unwrap()
                < provider::counter(&old.revision).unwrap()
                || provider::counter(&snapshot.frame).unwrap()
                    < provider::counter(&old.frame).unwrap()
        }) {
            return Ok(false);
        }
        if self.snapshot.as_ref().is_some_and(|raw| {
            provider::counter(&snapshot.revision).unwrap()
                < provider::counter(&raw.authority.revision).unwrap()
        }) {
            return Ok(false);
        }
        self.structural = Some(snapshot);
        self.structural_receipt = Some(now);
        Ok(true)
    }
    pub(crate) fn invalidate_structural_observation(&mut self) {
        self.structural_receipt = None;
    }
    pub fn structural_fresh(&self, now: u64) -> bool {
        self.fresh(now)
            && self
                .structural_receipt
                .is_some_and(|t| now >= t && now - t <= 250)
            && self.structural.as_ref().is_some_and(|s| {
                self.snapshot.as_ref().is_some_and(|raw| {
                    s.revision == raw.authority.revision
                        && raw.topology.as_ref() == Some(&s.topology)
                })
            })
    }
    /// Dispatch only the current transaction or an exact previously validated reply.
    /// Cached replies are not observations and cannot renew any receipt timestamp.
    pub fn dispatch_structural(
        &mut self,
        reply: crate::structure::Reply,
        now: u64,
    ) -> Result<(), String> {
        use sha2::{Digest, Sha256};
        let bytes = serde_json::to_vec(&reply).map_err(|e| e.to_string())?;
        let reply = crate::structure::decode_reply(&bytes)?;
        let fingerprint: [u8; 32] = Sha256::digest(&bytes).into();
        if self.pending.as_ref().is_some_and(|p| {
            crate::structure::is_kind(&p.request.kind) && p.request.context == reply.context
        }) {
            self.accept_structural(reply, now)?;
            if !self.structural_replies.contains(&fingerprint) {
                // Two replies per transaction, matching the producer's 64-request history.
                if self.structural_replies.len() == 128 {
                    self.structural_replies.pop_front();
                }
                self.structural_replies.push_back(fingerprint);
            }
            Ok(())
        } else if self.structural_replies.contains(&fingerprint) {
            Ok(())
        } else {
            fail("unknown or mismatched structural reply")
        }
    }
    pub fn accept_structural(
        &mut self,
        reply: crate::structure::Reply,
        now: u64,
    ) -> Result<(), String> {
        let reply = crate::structure::decode_reply(
            &serde_json::to_vec(&reply).map_err(|e| e.to_string())?,
        )?;
        let pending = self
            .pending
            .as_ref()
            .ok_or("no pending structural command")?;
        if !crate::structure::is_kind(&pending.request.kind)
            || pending.request.context != reply.context
            || self.version != 2
        {
            return fail("structural correlation");
        }
        let expected = provider::counter(
            reply
                .context
                .expected_revision
                .as_deref()
                .ok_or("structural expected revision")?,
        )?;
        if reply.state == "pending" {
            if provider::counter(&reply.revision)? != expected {
                return fail("structural pending revision");
            }
            self.pending.as_mut().unwrap().state = PendingState::Accepted;
            return Ok(());
        }
        if reply.reason.is_none()
            && (provider::counter(&reply.revision)?
                != expected.checked_add(1).ok_or("revision overflow")?
                || reply.effective_frame.is_none())
        {
            return fail("structural final boundary/revision");
        }
        if let Some(snapshot) = reply.snapshot.clone() {
            self.ingest_structural(snapshot, now)?;
        } else {
            // Boundary completion confirms the transaction, not a fresh readback.
            self.structural_receipt = None;
        }
        self.structural_final = Some(reply.clone());
        self.last_result = match reply.reason {
            Some(reason) => format!("structural REFUSED {reason}; confirmed state unchanged"),
            None => format!(
                "structural applied revision {} at frame {}; explicit rearm remains separate",
                reply.revision,
                reply.effective_frame.unwrap()
            ),
        };
        self.pending = None;
        self.needs_snapshot = true;
        self.preview = None;
        Ok(())
    }
    pub fn processing_request(&self) -> Request {
        let mut request = self.snapshot_request();
        request.kind = "processing_snapshot".into();
        request
    }
    pub fn ingest_processing(
        &mut self,
        s: crate::processing::Snapshot,
        now: u64,
    ) -> Result<bool, String> {
        s.validate_version(self.version + 1)?;
        if s.show_id != self.show || provider::counter(&s.epoch)? != self.epoch {
            return fail("wrong processing session");
        }
        if let Some(old) = &self.processing
            && (provider::counter(&s.revision)? < provider::counter(&old.revision)?
                || provider::counter(&s.sequence)? <= provider::counter(&old.sequence)?
                || provider::counter(&s.frame)? < provider::counter(&old.frame)?)
        {
            return Ok(false);
        }
        if let Some(raw) = &self.snapshot
            && provider::counter(&s.revision)? < provider::counter(&raw.authority.revision)?
        {
            return Ok(false);
        }
        if let Some(raw) = &self.snapshot {
            let ids: BTreeSet<_> = s.channels.iter().map(|c| &c.input).collect();
            if ids != raw.authority.inputs.iter().collect() {
                return fail("processing inventory differs from authority");
            }
        }
        self.processing = Some(s);
        self.processing_receipt = Some(now);
        Ok(true)
    }
    pub fn processing_fresh(&self, now: u64) -> bool {
        self.fresh(now)
            && self
                .processing_receipt
                .is_some_and(|t| now >= t && now - t <= 250)
            && self.processing.as_ref().is_some_and(|p| {
                !p.faulted
                    && p.channels.iter().all(|c| c.ready)
                    && self
                        .snapshot
                        .as_ref()
                        .is_some_and(|s| s.authority.revision == p.revision)
            })
    }
    pub fn processing_age(&self, now: u64) -> Option<u64> {
        self.processing_receipt.map(|t| now.saturating_sub(t))
    }
    pub fn accept_processing(
        &mut self,
        reply: crate::processing::Reply,
        now: u64,
    ) -> Result<(), String> {
        let r = crate::processing::decode_reply(
            &serde_json::to_vec(&reply).map_err(|e| e.to_string())?,
        )?;
        if r.version != self.version + 1 {
            return fail("processing reply version differs from session");
        }
        let mut candidate = self.clone();
        candidate.accept_processing_validated(r, now)?;
        *self = candidate;
        Ok(())
    }
    fn accept_processing_validated(
        &mut self,
        r: crate::processing::Reply,
        now: u64,
    ) -> Result<(), String> {
        let p = self
            .pending
            .as_ref()
            .ok_or("no pending processing request")?;
        if p.request.kind != "processing_set" || p.request.context != r.context {
            return fail("uncorrelated processing reply");
        }
        if r.effective_frame
            .as_deref()
            .is_some_and(|f| provider::counter(f).unwrap() <= p.observed_frame)
        {
            return fail("processing boundary not after observation");
        }
        if let Some(snapshot) = &r.snapshot {
            let input = p.request.body["input"]
                .as_str()
                .ok_or("processing pending input")?;
            let expected = crate::processing::decode_config(&p.request.body["config"])?;
            if snapshot
                .channels
                .iter()
                .find(|c| c.input == input)
                .is_none_or(|c| c.target != expected)
            {
                return fail("processing applied target differs from reviewed request");
            }
        }
        if let Some(ticket) = &p.ticket
            && r.reason.is_none()
            && (r.ticket.as_ref() != Some(ticket)
                || p.timing.as_ref()
                    != r.effective_frame
                        .as_ref()
                        .map(|f| (f.clone(), 240))
                        .as_ref())
        {
            return fail("processing pending/final timing mismatch");
        }
        if r.state == "backpressure" {
            if p.ticket.is_some() {
                return fail("pressure after admission");
            }
            // Nonadmission consumes no ID. Do not replay an edit automatically.
            self.next_id = provider::counter(p.request.context.request_id.as_deref().unwrap())?;
            self.pending = None;
            self.last_result =
                "processing backpressure; wait for fresh ready state and Apply again".into();
            return Ok(());
        }
        if r.state == "pending" {
            let p = self.pending.as_mut().unwrap();
            p.ticket = r.ticket;
            p.timing = r.effective_frame.map(|f| (f, 240));
            p.state = PendingState::Accepted;
            return Ok(());
        }
        if r.reason.is_none() {
            self.processing_final = Some(r.clone());
        }
        if let Some(snapshot) = r.snapshot {
            self.ingest_processing(snapshot, now)?;
        }
        self.last_result = match r.reason {
            Some(reason) => format!("processing REFUSED {reason}; confirmed settings unchanged"),
            None => format!(
                "processing_set applied revision {}; crossfade may still be active",
                r.revision
            ),
        };
        self.pending = None;
        self.needs_snapshot = true;
        self.preview = None;
        Ok(())
    }
    pub fn begin(&mut self, kind: &str, body: Value, now: u64) -> Result<Request, String> {
        if self.pending.is_some() {
            return fail("pending mutation");
        }
        if !self.fresh(now) {
            return fail("fresh complete compatible snapshot required");
        }
        if kind == "grant" {
            if self.next_id != 1
                || self.lease.is_some()
                || body != json!({"scope":crate::scopes::value(&self.scope)?})
            {
                return fail("invalid grant");
            }
        } else {
            if !self.lease.as_ref().is_some_and(|l| now < l.deadline) {
                return fail("lease expired");
            }
            if !self.armed && !matches!(kind, "renew" | "release") {
                return fail("release input and pickup first");
            }
        }
        self.validate_command(kind, &body, now)?;
        let id = self.next_id;
        self.next_id = id.checked_add(1).ok_or("request counter exhausted")?;
        let request = Request {
            version: self.version,
            context: Context {
                show_id: self.show.clone(),
                module: "audio".into(),
                epoch: self.epoch.to_string(),
                writer: Some(self.writer.clone()),
                lease: self.lease.as_ref().map(|l| l.token.clone()),
                request_id: Some(id.to_string()),
                expected_revision: Some(self.snapshot.as_ref().unwrap().authority.revision.clone()),
            },
            kind: kind.into(),
            body,
        };
        request.encode()?;
        self.pending = Some(Pending {
            request: request.clone(),
            first_send: now,
            state: PendingState::Sent,
            ticket: None,
            timing: None,
            observed_frame: provider::counter(&self.snapshot.as_ref().unwrap().frame)?,
            retry: 0,
        });
        Ok(request)
    }
    fn validate_command(&self, kind: &str, body: &Value, now: u64) -> Result<(), String> {
        match kind {
            "device_configure" => {
                if self.scope != "local_operator_monitor" || !self.device_fresh(now) {
                    return fail("fresh device observation and local operator lease required");
                }
                self.validate_device_intent(body, now)
            }
            kind if crate::brain::is_kind(kind) => {
                if kind == "brain_hold" && self.brain_closing.is_some() {
                    return fail("prior close must settle before a new hold");
                }
                if self.version != 2
                    || crate::brain::scope(kind) != Some(self.scope.as_str())
                    || !self.brain_fresh(now)
                {
                    return fail("fresh Brain readback and separate scope grant required");
                }
                if kind == "brain_monitor_set" && body["armed"] == true && !self.device_fresh(now) {
                    return fail(
                        "separate arm requires fresh actual device readback; audible readiness follows prefill",
                    );
                }
                let raw = self.snapshot.as_ref().unwrap();
                crate::brain::validate_body(
                    kind,
                    body,
                    self.brain.as_ref(),
                    raw.authority.inputs.len(),
                    raw.authority.monitors.len(),
                )
            }
            kind if crate::structure::is_kind(kind) => {
                if self.version != 2
                    || crate::structure::scope(kind) != Some(self.scope.as_str())
                    || !self.structural_fresh(now)
                {
                    return fail("fresh structural state and separate scoped grant required");
                }
                let snapshot = self.structural.as_ref().unwrap();
                if matches!(kind, "pa_set" | "output_patch") && !snapshot.outputs_quiesced {
                    return fail("quiesce outputs before structural prepare");
                }
                crate::structure::validate_body(kind, body, Some(snapshot))
            }
            "processing_set" => {
                if self.scope != "foh" || !self.processing_fresh(now) {
                    return fail("fresh ready GP07 processing and FOH lease required");
                }
                {
                    crate::processing::validate_body_version(body, self.version)?;
                    if !self
                        .snapshot
                        .as_ref()
                        .unwrap()
                        .authority
                        .inputs
                        .iter()
                        .any(|id| body["input"] == *id)
                    {
                        return fail("processing input absent");
                    }
                    Ok(())
                }
            }
            "grant" => provider::keys(body, &["scope"]),
            "renew" | "release" => provider::keys(body, &[]),
            "set" | "propose" | "preview_release" => {
                provider::keys(body, &["targets"])?;
                let a = body["targets"].as_array().ok_or("targets")?;
                if a.is_empty() || a.len() > if self.version == 1 { 40 } else { 64 } {
                    return fail("targets capacity");
                }
                let mut seen = BTreeSet::new();
                for e in a {
                    let v = if kind == "preview_release" {
                        e
                    } else {
                        provider::keys(e, &["target", "value"])?;
                        &e["target"]
                    };
                    let t: Target = serde_json::from_value(v.clone()).map_err(|e| e.to_string())?;
                    provider::keys(
                        v,
                        if t.parameter == "send" {
                            &["parameter", "input", "monitor"]
                        } else {
                            &["parameter", "input"]
                        },
                    )?;
                    self.snapshot
                        .as_ref()
                        .unwrap()
                        .authority
                        .validate_target(&t)?;
                    if target_scope(&t) != self.scope || !seen.insert(t.clone()) {
                        return fail("target scope/duplicate");
                    }
                    if kind != "preview_release" {
                        provider::value(&t, &e["value"])?;
                    } else if t.parameter == "mute" {
                        return fail("mute release unavailable");
                    }
                }
                Ok(())
            }
            "set_mode" => {
                provider::keys(body, &["mode", "bounds"])?;
                let mode = body["mode"].as_str().ok_or("mode")?;
                let a = body["bounds"].as_array().ok_or("bounds")?;
                if !matches!(mode, "manual" | "assist" | "auto")
                    || (mode == "auto") == a.is_empty()
                    || a.len() > if self.version == 1 { 40 } else { 64 }
                {
                    return fail("mode explicit bounds");
                }
                let mut seen = BTreeSet::new();
                for b in a {
                    provider::keys(b, &["target", "min", "max"])?;
                    let t: Target =
                        serde_json::from_value(b["target"].clone()).map_err(|e| e.to_string())?;
                    provider::keys(
                        &b["target"],
                        if t.parameter == "send" {
                            &["parameter", "input", "monitor"]
                        } else {
                            &["parameter", "input"]
                        },
                    )?;
                    self.snapshot
                        .as_ref()
                        .unwrap()
                        .authority
                        .validate_target(&t)?;
                    provider::value(&t, &b["min"])?;
                    provider::value(&t, &b["max"])?;
                    if t.parameter == "mute"
                        || target_scope(&t) != self.scope
                        || b["min"].as_i64() > b["max"].as_i64()
                        || !seen.insert(t)
                    {
                        return fail("bounds scope/order");
                    }
                }
                Ok(())
            }
            "cancel_preview" | "release_preview" => {
                provider::keys(body, &["token"])?;
                if !self.preview.as_ref().is_some_and(|(p, d, g)| {
                    body["token"] == p.token && now < *d && *g == self.generation
                }) {
                    return fail("no valid engine-bound preview");
                }
                Ok(())
            }
            _ => fail("unsupported command"),
        }
    }
    pub(crate) fn pending_authority_deadline(&self) -> Option<u64> {
        let p = self.pending.as_ref()?;
        if p.request.kind == "grant" {
            p.first_send.checked_add(2000)
        } else {
            self.lease.as_ref().map(|lease| lease.deadline)
        }
    }
    /// Next existing retry offset, or authority expiry once retries are exhausted.
    /// Reading the schedule never consumes a retry or extends its first-send time.
    pub(crate) fn retry_wake(&self, now: u64) -> Option<u64> {
        let p = self.pending.as_ref()?;
        if now < p.first_send
            || p.state == PendingState::Uncertain
            || matches!(
                p.request.kind.as_str(),
                "brain_hold" | "brain_heartbeat" | "brain_close"
            )
        {
            return None;
        }
        let expires = self.pending_authority_deadline()?;
        if now >= expires {
            return None;
        }
        Some(
            RETRY_DELAYS_MS
                .get(p.retry)
                .and_then(|delay| p.first_send.checked_add(*delay))
                .unwrap_or(expires)
                .min(expires),
        )
    }
    pub fn retry(&mut self, now: u64) -> Option<Request> {
        if self.pending.as_ref().is_some_and(|p| {
            p.request.kind == "device_configure"
                && self.validate_device_intent(&p.request.body, now).is_err()
        }) {
            self.pending.as_mut()?.state = PendingState::Uncertain;
            return None;
        }
        let p = self.pending.as_mut()?;
        if now < p.first_send {
            return None;
        }
        if p.request.kind != "grant" && !self.lease.as_ref().is_some_and(|l| now < l.deadline)
            || p.request.kind == "grant" && now - p.first_send >= 2000
        {
            p.state = PendingState::Uncertain;
            return None;
        }
        if p.state == PendingState::Uncertain
            || matches!(
                p.request.kind.as_str(),
                "brain_hold" | "brain_heartbeat" | "brain_close"
            )
        {
            return None;
        }
        let delay = RETRY_DELAYS_MS.get(p.retry)?;
        if now - p.first_send < *delay {
            return None;
        }
        p.retry += 1;
        Some(p.request.clone())
    }
    pub fn accept(&mut self, r: Reply, now: u64) -> Result<(), String> {
        // Public typed callers receive the same strict semantics as the wire path.
        let validated = decode_reply(&serde_json::to_vec(&r).map_err(|e| e.to_string())?)?;
        if validated.capability_version != self.version {
            return fail("reply version differs from session");
        }
        let mut candidate = self.clone();
        candidate.accept_validated(validated, now)?;
        *self = candidate;
        Ok(())
    }
    fn accept_validated(&mut self, r: Reply, now: u64) -> Result<(), String> {
        let p = self.pending.as_ref().ok_or("no pending request")?;
        if p.request.kind.starts_with("processing_") {
            return fail("cross-contract reply");
        }
        if r.context != p.request.context {
            return fail("reply identity differs; pending retained");
        }
        let terminal_refusal = r.state == "final"
            && r.outcome.as_ref().is_some_and(|o| o.kind != "applied")
            && r.ticket.is_none()
            && r.effective_frame.is_none()
            && r.ramp_frames.is_none();
        if let Some(old) = &p.ticket
            && !terminal_refusal
            && (r.ticket.as_ref() != Some(old)
                || p.timing.as_ref()
                    != r.effective_frame
                        .as_ref()
                        .zip(r.ramp_frames)
                        .map(|(f, n)| (f.clone(), n))
                        .as_ref())
        {
            return fail("wrong/missing admitted ticket/timing; pending retained");
        }
        if r.state == "pending" {
            let p = self.pending.as_mut().unwrap();
            p.state = PendingState::Accepted;
            p.ticket = r.ticket;
            p.timing = r.effective_frame.zip(r.ramp_frames);
            return Ok(());
        }
        if r.state == "backpressure" {
            if p.ticket.is_some() {
                return fail("backpressure after admission");
            }
            self.pending.as_mut().unwrap().state = PendingState::Backpressure;
            return Ok(());
        }
        let o = r.outcome.as_ref().ok_or("missing outcome")?;
        if o.kind == "applied" {
            match p.request.kind.as_str() {
                "grant"
                    if o.body.granted_lease.is_none()
                        || o.body.scope.as_deref() != Some(&self.scope)
                        || o.body.lease_remaining_ms != Some(2000) =>
                {
                    return fail("invalid grant outcome");
                }
                "renew"
                    if o.body.lease_remaining_ms != Some(2000)
                        || o.body.granted_lease.is_some()
                        || o.body.scope.as_deref() != Some(&self.scope) =>
                {
                    return fail("invalid renew outcome");
                }
                "preview_release" if o.body.preview.is_none() => {
                    return fail("missing engine preview");
                }
                "grant" | "renew" | "preview_release" => {}
                _ if o.body.granted_lease.is_some()
                    || o.body.scope.is_some()
                    || o.body.lease_remaining_ms.is_some()
                    || o.body.preview.is_some() =>
                {
                    return fail("unexpected outcome capability");
                }
                _ => {}
            }
        }
        let first = p.first_send;
        let request = p.request.clone();
        let kind = request.kind.clone();
        if o.kind == "applied" && matches!(kind.as_str(), "grant" | "renew") {
            let remaining = o.body.lease_remaining_ms.ok_or("missing lease duration")?;
            let token = if kind == "grant" {
                if o.body.scope.as_deref() != Some(&self.scope) {
                    return fail("grant scope");
                }
                o.body.granted_lease.clone().ok_or("missing grant token")?
            } else {
                self.lease.as_ref().ok_or("renew no lease")?.token.clone()
            };
            self.lease = Some(Lease {
                token,
                deadline: first.checked_add(remaining).ok_or("deadline overflow")?,
                renew_at: first.checked_add(500).ok_or("renew overflow")?,
            });
        }
        if let Some(s) = r.snapshot {
            self.ingest_snapshot(s, now)?;
        }
        if o.kind == "applied" && kind == "preview_release" {
            let preview = o.body.preview.as_ref().ok_or("missing preview")?;
            if preview.scope != self.scope
                || request.context.expected_revision.as_deref() != Some(&preview.revision)
            {
                return fail("preview scope/revision differs");
            }
            let requested = request.body["targets"]
                .as_array()
                .ok_or("preview request targets")?
                .iter()
                .map(|t| serde_json::from_value::<Target>(t.clone()).map_err(|e| e.to_string()))
                .collect::<Result<BTreeSet<_>, _>>()?;
            let returned = preview
                .destinations
                .iter()
                .map(|d| d.target.clone())
                .collect::<BTreeSet<_>>();
            if requested != returned {
                return fail("preview selected target mismatch");
            }
            let deadline = first
                .checked_add(preview.remaining_ms)
                .ok_or("preview deadline")?;
            if self
                .snapshot
                .as_ref()
                .is_some_and(|s| s.authority.revision == preview.revision)
                && now < deadline
            {
                self.preview = Some((preview.clone(), deadline, self.generation));
            } else {
                self.preview = None;
            }
        }
        if o.kind == "applied" && kind == "release" {
            self.lease = None;
        }
        if matches!(
            kind.as_str(),
            "set" | "set_mode" | "release_preview" | "release" | "propose"
        ) {
            self.needs_snapshot = true;
        }
        if matches!(
            kind.as_str(),
            "cancel_preview" | "release_preview" | "set_mode"
        ) {
            self.preview = None;
        }
        self.last_result = if kind == "propose" && o.kind == "applied" {
            format!(
                "proposal stored; rendered mix unchanged; revision {}",
                o.body.revision
            )
        } else {
            format!("{} {} revision {}", kind, o.kind, o.body.revision)
        };
        self.pending = None;
        Ok(())
    }
}
#[cfg(test)]
mod exhaustion_tests {
    use super::*;
    fn granted() -> Session {
        let corpus: Value =
            serde_json::from_str(include_str!("../tests/fixtures/gp03/v1/e03-rendered.json"))
                .unwrap();
        let mut s = Session::new(
            "11111111-1111-4111-8111-111111111111",
            9,
            "desk-corpus",
            "foh",
        )
        .unwrap();
        s.ingest_snapshot(
            decode_snapshot(&serde_json::to_vec(&corpus["initial"]).unwrap()).unwrap(),
            0,
        )
        .unwrap();
        s.begin("grant", json!({"scope":"foh"}), 0).unwrap();
        s.accept(
            decode_reply(&serde_json::to_vec(&corpus["grant_response"]).unwrap()).unwrap(),
            1,
        )
        .unwrap();
        s.input_released();
        s
    }
    #[test]
    fn retry_wake_preserves_offsets_first_send_and_expiry_without_advancing() {
        let mut s = granted();
        s.begin("renew", json!({}), 2).unwrap();
        let original = s.pending.as_ref().unwrap().request.clone();
        for (before, due, next) in [(2, 102, 252), (102, 252, 502), (252, 502, 2000)] {
            assert_eq!(s.retry_wake(before), Some(due));
            assert_eq!(s.retry_wake(before), Some(due));
            assert_eq!(s.retry(due).unwrap(), original);
            assert_eq!(s.pending.as_ref().unwrap().first_send, 2);
            assert_eq!(s.retry_wake(due), Some(next));
        }
        assert!(s.retry(503).is_none());
        assert_eq!(s.retry_wake(1999), Some(2000));
        assert_eq!(s.retry_wake(2000), None);
        assert!(s.retry(2000).is_none());
        let mut s = granted();
        s.begin("renew", json!({}), 2).unwrap();
        s.pending.as_mut().unwrap().state = PendingState::Uncertain;
        assert!(s.retry_wake(100).is_none());
        assert!(s.retry(100).is_none());
        let previous = granted();
        let mut s = Session::new(&previous.show, previous.epoch, "grant-wake", "foh").unwrap();
        s.ingest_snapshot(previous.snapshot.unwrap(), 2).unwrap();
        s.begin("grant", json!({"scope":"foh"}), 2).unwrap();
        assert_eq!(s.pending_authority_deadline(), Some(2002));
        assert_eq!(s.retry_wake(2), Some(102));
        assert!(s.retry_wake(2002).is_none());
    }
    #[test]
    fn request_exhaustion_does_not_wrap_or_create_pending() {
        let mut s = granted();
        s.next_id = u64::MAX;
        assert!(s.begin("renew", json!({}), 2).is_err());
        assert_eq!(s.next_id, u64::MAX);
        assert!(s.pending.is_none());
    }
    #[test]
    fn context_exhaustion_permanently_fences_gestures() {
        let mut s = granted();
        s.generation = u64::MAX;
        s.context_changed();
        s.input_released();
        assert!(!s.fresh(2));
        assert!(s.begin("renew", json!({}), 2).is_err());
    }
}
#[cfg(test)]
mod actual_refusal_test {
    use super::*;
    #[test]
    fn actual_admitted_preview_expiry_is_known_refusal_not_uncertain() {
        let c: Value = serde_json::from_str(include_str!(
            "../tests/fixtures/gp03/preview1/preview-flow.json"
        ))
        .unwrap();
        let nodes = c["expiry_with_live_lease"].as_array().unwrap();
        let node = |label: &str| nodes.iter().find(|n| n["label"] == label).unwrap();
        let mut s = Session::new(
            "11111111-1111-4111-8111-111111111111",
            9,
            "desk-preview-corpus",
            "foh",
        )
        .unwrap();
        let decode_s = |v: &Value| decode_snapshot(&serde_json::to_vec(v).unwrap()).unwrap();
        let decode_r = |v: &Value| decode_reply(&serde_json::to_vec(v).unwrap()).unwrap();
        s.ingest_snapshot(
            decode_s(&node("assist_tick_before_boundary")["snapshot"]),
            0,
        )
        .unwrap();
        s.begin("grant", json!({"scope":"foh"}), 0).unwrap();
        s.accept(decode_r(&node("grant")["response"]), 0).unwrap();
        s.input_released();
        let snapshot = decode_s(&node("expiry_tick_before_boundary")["snapshot"]);
        s.ingest_snapshot(snapshot.clone(), 1000).unwrap();
        s.next_id = 5;
        let request = s.begin("renew", json!({}), 1000).unwrap();
        assert_eq!(
            serde_json::from_slice::<Value>(&request.encode().unwrap()).unwrap(),
            node("renew_before_expiry")["request"]
        );
        s.accept(decode_r(&node("renew_before_expiry")["response"]), 1000)
            .unwrap();
        let preview = decode_r(&node("preview_to_expire")["response"])
            .outcome
            .unwrap()
            .body
            .preview
            .unwrap();
        s.preview = Some((preview, 2100, s.generation));
        s.ingest_snapshot(snapshot.clone(), 2099).unwrap();
        let request = s
            .begin(
                "release_preview",
                node("release_just_before_expiry")["request"]["body"].clone(),
                2099,
            )
            .unwrap();
        assert_eq!(
            serde_json::from_slice::<Value>(&request.encode().unwrap()).unwrap(),
            node("release_just_before_expiry")["request"]
        );
        s.accept(
            decode_r(&node("release_just_before_expiry")["response"]),
            2099,
        )
        .unwrap();
        assert_eq!(s.pending.as_ref().unwrap().state, PendingState::Accepted);
        let before = s.snapshot.clone();
        s.accept(
            decode_r(&node("expiry_tick_refused")["completions"][0]),
            2101,
        )
        .unwrap();
        assert!(s.pending.is_none());
        assert_eq!(s.snapshot, before);
        assert!(s.preview(2101).is_none());
        assert!(!s.fresh(2101));
        assert!(s.last_result.contains("conflict"));
        assert!(s.retry(2102).is_none());
        s.ingest_snapshot(snapshot, 2102).unwrap();
        assert!(s.fresh(2102));
    }
}

#[cfg(test)]
mod structural_reply_tests {
    use super::*;

    #[test]
    fn completed_structural_duplicates_do_not_refresh_or_complete_new_commands() {
        let snapshot = crate::structure::decode_snapshot(include_bytes!(
            "../tests/fixtures/gp14/v1/structure-16.json"
        ))
        .unwrap();
        let mut session = Session::new_version(
            &snapshot.show_id,
            snapshot.epoch.parse().unwrap(),
            "duplicate-test",
            "pa_configuration",
            2,
        )
        .unwrap();
        let mut request = session.structural_request();
        request.kind = "output_mute".into();
        request.context.writer = Some("duplicate-test".into());
        request.context.lease = Some("1".into());
        request.context.request_id = Some("1".into());
        request.context.expected_revision = Some(snapshot.revision.clone());
        let pending = Pending {
            request: request.clone(),
            first_send: 0,
            state: PendingState::Sent,
            ticket: None,
            timing: None,
            observed_frame: 0,
            retry: 0,
        };
        session.pending = Some(pending.clone());
        let reply = crate::structure::Reply {
            contract: "GP14-structure".into(),
            version: 1,
            context: request.context.clone(),
            state: "pending".into(),
            reason: None,
            effective_frame: None,
            revision: snapshot.revision.clone(),
            snapshot: None,
        };
        session.dispatch_structural(reply.clone(), 1).unwrap();
        let mut final_reply = reply.clone();
        final_reply.state = "final".into();
        final_reply.revision = (snapshot.revision.parse::<u64>().unwrap() + 1).to_string();
        final_reply.effective_frame = Some("480".into());
        let mut final_snapshot = snapshot;
        final_snapshot.revision = final_reply.revision.clone();
        final_reply.snapshot = Some(final_snapshot);
        session.dispatch_structural(final_reply.clone(), 2).unwrap();
        assert!(session.pending.is_none());
        for now in [20, 300] {
            session.dispatch_structural(reply.clone(), now).unwrap();
            session
                .dispatch_structural(final_reply.clone(), now)
                .unwrap();
            assert_eq!(session.structural_receipt, Some(2));
        }
        let mut next = pending;
        next.request.context.request_id = Some("2".into());
        session.pending = Some(next);
        session.dispatch_structural(reply.clone(), 301).unwrap();
        session
            .dispatch_structural(final_reply.clone(), 302)
            .unwrap();
        assert_eq!(
            session
                .pending
                .as_ref()
                .unwrap()
                .request
                .context
                .request_id
                .as_deref(),
            Some("2")
        );
        assert_eq!(session.structural_receipt, Some(2));
        let mut wrong = final_reply.clone();
        wrong.context.writer = Some("unknown-writer".into());
        assert!(session.dispatch_structural(wrong, 303).is_err());
        let mut fresh = final_reply.clone();
        fresh.context.request_id = Some("3".into());
        assert!(session.dispatch_structural(fresh, 304).is_err());
        let mut changed = final_reply.clone();
        changed.effective_frame = Some("528".into());
        assert!(session.dispatch_structural(changed, 305).is_err());
        session.disconnect();
        assert!(session.dispatch_structural(final_reply, 306).is_err());
    }
}

#[cfg(test)]
mod actual_structural_exchange_tests {
    use super::*;
    #[test]
    fn every_actual_structural_reply_correlates_without_invented_final_readback() {
        for count in [16, 32, 48] {
            let path = format!(
                "{}/tests/fixtures/gp14/final/structure-replies-{count}.json",
                env!("CARGO_MANIFEST_DIR")
            );
            let corpus: Value = serde_json::from_slice(&std::fs::read(path).unwrap()).unwrap();
            let mut session = None;
            let mut old_final: Option<crate::structure::Reply> = None;
            for exchange in corpus["exchanges"].as_array().unwrap() {
                let reply = crate::structure::decode_reply(
                    &serde_json::to_vec(&exchange["reply"]).unwrap(),
                )
                .unwrap();
                let epoch = reply.context.epoch.parse::<u64>().unwrap();
                if session.as_ref().is_none_or(|s: &Session| s.epoch != epoch) {
                    session = Some(
                        Session::new_version(
                            &reply.context.show_id,
                            epoch,
                            "corpus-correlation",
                            "pa_configuration",
                            2,
                        )
                        .unwrap(),
                    );
                    old_final = None;
                }
                let session = session.as_mut().unwrap();
                if reply.state == "snapshot" {
                    session
                        .ingest_structural(reply.snapshot.clone().unwrap(), 0)
                        .unwrap();
                    for field in ["writer", "lease", "request_id", "expected_revision"] {
                        let mut bad = exchange["reply"].clone();
                        bad["context"][field] = "1".into();
                        assert!(
                            crate::structure::decode_reply(&serde_json::to_vec(&bad).unwrap())
                                .is_err()
                        );
                    }
                    for (field, value) in [
                        ("reason", json!("bad")),
                        ("effective_frame", json!("48")),
                        ("snapshot", Value::Null),
                        ("state", json!("unknown")),
                    ] {
                        let mut bad = exchange["reply"].clone();
                        bad[field] = value;
                        assert!(
                            crate::structure::decode_reply(&serde_json::to_vec(&bad).unwrap())
                                .is_err()
                        );
                    }
                    continue;
                }
                if session.pending.is_none() {
                    session.pending = Some(Pending {
                        request: Request {
                            version: 2,
                            context: reply.context.clone(),
                            kind: exchange["request"]["kind"].as_str().unwrap().into(),
                            body: exchange["request"]["body"].clone(),
                        },
                        first_send: 0,
                        state: PendingState::Sent,
                        ticket: None,
                        timing: None,
                        observed_frame: 0,
                        retry: 0,
                    });
                }
                if let Some(old) = &old_final {
                    session.dispatch_structural(old.clone(), 300).unwrap();
                    assert_eq!(
                        session.pending.as_ref().unwrap().request.context,
                        reply.context
                    );
                }
                session.dispatch_structural(reply.clone(), 1).unwrap();
                if reply.state == "final" {
                    assert!(
                        reply.snapshot.is_none(),
                        "actual producer sends boundary metadata, followed by explicit readback"
                    );
                    assert!(session.structural_receipt.is_none());
                    assert!(session.pending.is_none());
                    session.dispatch_structural(reply.clone(), 400).unwrap();
                    assert!(session.structural_receipt.is_none());
                    old_final = Some(reply);
                }
            }
        }
    }
}

#[cfg(test)]
mod brain_safety_tests {
    use super::*;
    // Explicit adversarial state derived from the actual GP14 topology. This is
    // not a GP15 producer fixture or an acceptance claim.
    fn session(scope: &str) -> Session {
        let v: Value =
            serde_json::from_str(include_str!("../tests/fixtures/gp14/v1/profile-48.json"))
                .unwrap();
        let raw = decode_reply(&serde_json::to_vec(&v["snapshot"]).unwrap())
            .unwrap()
            .snapshot
            .unwrap();
        let mut s = Session::new_version(
            &raw.authority.show_id,
            provider::counter(&raw.authority.epoch).unwrap(),
            "brain-test",
            scope,
            2,
        )
        .unwrap();
        s.ingest_snapshot(raw, 0).unwrap();
        s.lease = Some(Lease {
            token: "7".into(),
            deadline: 2000,
            renew_at: 1000,
        });
        s.next_id = 2;
        s.input_released();
        let b = crate::brain::Snapshot {
            source: crate::brain::Source::Main,
            selection_generation: "1".into(),
            monitor_gain_cdb: -1200,
            monitor_mute: true,
            monitor_dim: false,
            monitor_armed: false,
            talkback_monitors: vec![0],
            talkback_foh: false,
            talkback_gain_cdb: -1200,
            talkback_mute: false,
            hold_generation_counter: "0".into(),
            held_generation: None,
            hold_deadline_ms: None,
            audible_path_ready: true,
            talkback_path_ready: true,
            monitor_path_ready: true,
            microphone_peak_nano: 0,
            outgoing_peak_nano: 0,
            monitor_peak_nano: 0,
            frame: s.snapshot.as_ref().unwrap().frame.clone(),
            revision: s.snapshot.as_ref().unwrap().authority.revision.clone(),
            heartbeat_ms: 50,
            deadman_ms: 150,
            fade_frames: 240,
        };
        s.ingest_brain(b, 0).unwrap();
        s
    }
    #[test]
    fn brain_priority_close_preserves_pending_and_stops_at_expired_authority() {
        let mut s = session("talkback_destinations");
        let hold = s.begin("brain_hold", json!({"generation":"1"}), 0).unwrap();
        let close = s.brain_close_request(1).unwrap();
        assert_ne!(close.context.request_id, hold.context.request_id);
        assert_eq!(s.pending.as_ref().unwrap().request, hold);
        assert_eq!(s.brain_close_request(2).unwrap(), close);
        let mut fresh = session("talkback_destinations");
        assert!(fresh.brain_close_request(2000).is_err());
        assert!(
            session("local_operator_monitor")
                .brain_close_request(1)
                .is_err()
        );
        s.disconnect();
        assert!(s.brain_close_request(3).is_err());
        assert!(s.pending.is_none());
    }
    #[test]
    fn brain_correlation_refuses_wrong_context_and_duplicate_cannot_refresh() {
        let mut s = session("talkback_destinations");
        let r = s.begin("brain_hold", json!({"generation":"1"}), 0).unwrap();
        let revision =
            provider::counter(r.context.expected_revision.as_ref().unwrap()).unwrap() + 1;
        let mut b = s.brain.clone().unwrap();
        b.revision = revision.to_string();
        b.frame = (provider::counter(&b.frame).unwrap() + 48).to_string();
        b.held_generation = Some("1".into());
        b.hold_generation_counter = "1".into();
        b.hold_deadline_ms = Some("150".into());
        let reply = crate::brain::Reply {
            contract: "GP15-brain".into(),
            version: 1,
            state: "final".into(),
            reason: None,
            context: r.context.clone(),
            revision: revision.to_string(),
            applied_frame: Some(b.frame.clone()),
            snapshot: Some(b),
        };
        let mut bad = reply.clone();
        bad.context.request_id = Some("999".into());
        assert!(s.dispatch_brain(bad, 1).is_err());
        assert!(s.pending.is_some());
        let mut bad = reply.clone();
        bad.revision = (revision + 1).to_string();
        bad.snapshot.as_mut().unwrap().revision = bad.revision.clone();
        assert!(s.dispatch_brain(bad, 1).is_err());
        s.dispatch_brain(reply.clone(), 1).unwrap();
        assert!(s.pending.is_none());
        assert_eq!(s.brain_receipt, Some(1));
        s.dispatch_brain(reply, 220).unwrap();
        assert_eq!(s.brain_receipt, Some(1));
        assert!(s.needs_snapshot);
        s.invalidate_brain_observation();
        assert!(!s.brain_fresh(2));
    }
    #[test]
    fn brain_source_arm_requires_separate_readback_and_monitor_readiness() {
        let mut s = session("local_operator_monitor");
        let mut body = json!({"source":{"kind":"pfl","input":47},"gain_cdb":-1200,"mute":false,"dim":false,"armed":true});
        assert!(s.begin("brain_monitor_set", body.clone(), 0).is_err());
        body["armed"] = json!(false);
        assert!(s.begin("brain_monitor_set", body, 0).is_ok());
        let mut s = session("local_operator_monitor");
        s.brain.as_mut().unwrap().monitor_path_ready = false;
        assert!(s.begin("brain_monitor_set",json!({"source":{"kind":"main"},"gain_cdb":-1200,"mute":false,"dim":false,"armed":true}),0).is_err());
        assert!(
            session("talkback_destinations")
                .begin("brain_talkback_foh", json!({"enabled":true}), 0)
                .is_err()
        );
    }
}
