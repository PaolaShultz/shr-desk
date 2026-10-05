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
    pub current_nanogain: [Nanogain; 6],
    pub ramp_target_nanogain: [Nanogain; 6],
    pub held_nanogain: [Option<Nanogain>; 6],
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
        // The nested authority intentionally remains the separately reviewed GP02 metadata body.
        let bytes = serde_json::to_vec(&self.authority_value()).map_err(|e| e.to_string())?;
        let a = provider::decode(&bytes)?;
        if a.page != 0
            || a.page_count != 1
            || a.parameters.len() != 40
            || self.coefficients.len() != 8
        {
            return fail("incomplete rendered snapshot");
        }
        let mut seen = BTreeSet::new();
        for c in &self.coefficients {
            if !a.inputs.contains(&c.input) || !seen.insert(&c.input) {
                return fail("coefficient identity");
            }
            for (i, (current, target)) in c
                .current_nanogain
                .iter()
                .zip(&c.ramp_target_nanogain)
                .enumerate()
            {
                let max = if [0, 4, 5].contains(&i) {
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
        // Snapshot is a read-only type; serialization is explicit and never changes its contract.
        let a = &self.authority;
        json!({"show_id":a.show_id,"epoch":a.epoch,"revision":a.revision,"sequence":a.sequence,"page":a.page,"page_count":a.page_count,"durability":a.durability,"validity":a.validity,"acquisition_frame":a.acquisition_frame,"age_ms":a.age_ms,"rendered_application":a.rendered_application,"release_commit":a.release_commit,"session_history_capacity":a.session_history_capacity,"inputs":a.inputs,"monitors":a.monitors,"modes":a.modes,"automation_bounds":a.automation_bounds.iter().map(|b|json!({"target":target_value(&b.target),"min":b.min,"max":b.max})).collect::<Vec<_>>(),"parameters":a.parameters.iter().map(|p|json!({"target":target_value(&p.target),"actual":p.actual,"target_value":p.target_value,"proposal":p.proposal,"hold":p.hold,"owner":p.owner})).collect::<Vec<_>>()})
    }
}
fn capability(name: &str, version: u8) -> Result<(), String> {
    if name != "GP03-rendered" || version != 1 {
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
    pub scope: String,
    pub remaining_ms: u64,
    pub ramp_frames: u64,
    pub destinations: Vec<Destination>,
}
impl Preview {
    fn validate(&self) -> Result<(), String> {
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
            provider::target(&d.target)?;
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
    let v = provider::parse(bytes)?;
    validate_snapshot_value(&v)?;
    let s: RenderedSnapshot = serde_json::from_value(v).map_err(|e| e.to_string())?;
    s.validate()?;
    Ok(s)
}
fn validate_snapshot_value(v: &Value) -> Result<(), String> {
    provider::keys(
        v,
        &[
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
        ],
    )?;
    provider::decode(&serde_json::to_vec(&v["authority"]).map_err(|e| e.to_string())?)?;
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
    let v = provider::parse(bytes)?;
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
            provider::decode(
                &serde_json::to_vec(&v["outcome"]["body"]["snapshot"])
                    .map_err(|e| e.to_string())?,
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
            || o.version != 1
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
            p.validate()?;
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
    matches!(s, "foh" | "monitor1" | "monitor2")
}
fn target_scope(t: &Target) -> &str {
    match t.monitor.as_deref() {
        Some("monitor-1") => "monitor1",
        Some("monitor-2") => "monitor2",
        _ => "foh",
    }
}
#[derive(Clone, Debug, PartialEq)]
pub struct Request {
    pub context: Context,
    pub kind: String,
    pub body: Value,
}
impl Request {
    pub fn encode(&self) -> Result<Vec<u8>, String> {
        self.context.validate()?;
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
            crate::processing::validate_body(&self.body)?;
            if c.writer.is_none() || c.lease.is_none() {
                return fail("processing mutation authority");
            }
        } else if self.kind.starts_with("processing_") {
            return fail("unknown processing request");
        }
        let contract = if self.kind.starts_with("processing_") {
            "GP07-processing"
        } else {
            "C-AUDIO"
        };
        let v = json!({"contract":contract,"version":1,"show_id":c.show_id,"module":c.module,"epoch":c.epoch,"writer":c.writer,"lease":c.lease,"request_id":c.request_id,"expected_revision":c.expected_revision,"kind":self.kind,"body":self.body});
        let b = serde_json::to_vec(&v).map_err(|e| e.to_string())?;
        if b.len() > provider::MAX_BYTES {
            return fail("request capacity");
        }
        Ok(b)
    }
}
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
    show: String,
    epoch: u64,
    writer: String,
    scope: String,
    next_id: u64,
    pub snapshot: Option<RenderedSnapshot>,
    receipt: Option<u64>,
    pub processing: Option<crate::processing::Snapshot>,
    processing_receipt: Option<u64>,
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
        if !provider::uuid(show) || !provider::id(writer) || !scope(scoped) {
            return fail("session identity/scope");
        }
        Ok(Self {
            show: show.into(),
            epoch,
            writer: writer.into(),
            scope: scoped.into(),
            next_id: 1,
            snapshot: None,
            receipt: None,
            processing: None,
            processing_receipt: None,
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
        self.processing_receipt = None;
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
        s.validate()?;
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
            if self.next_id != 1 || self.lease.is_some() || body != json!({"scope":self.scope}) {
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
            "processing_set" => {
                if self.scope != "foh" || !self.processing_fresh(now) {
                    return fail("fresh ready GP07 processing and FOH lease required");
                }
                crate::processing::validate_body(body)
            }
            "grant" => provider::keys(body, &["scope"]),
            "renew" | "release" => provider::keys(body, &[]),
            "set" | "propose" | "preview_release" => {
                provider::keys(body, &["targets"])?;
                let a = body["targets"].as_array().ok_or("targets")?;
                if a.is_empty() || a.len() > 40 {
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
                    provider::target(&t)?;
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
                    || a.len() > 40
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
                    provider::target(&t)?;
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
    pub fn retry(&mut self, now: u64) -> Option<Request> {
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
        let delay = [100, 250, 500].get(p.retry)?;
        if now - p.first_send < *delay {
            return None;
        }
        p.retry += 1;
        Some(p.request.clone())
    }
    pub fn accept(&mut self, r: Reply, now: u64) -> Result<(), String> {
        // Public typed callers receive the same strict semantics as the wire path.
        let validated = decode_reply(&serde_json::to_vec(&r).map_err(|e| e.to_string())?)?;
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
