//! Explicit trusted local Unix endpoint only; no device or TCP transport.
use crate::audio::{self, Session};
use serde_json::{Value, json};
use std::{
    path::Path,
    time::{Duration, Instant},
};

mod measurement;
mod fx;
mod transport;
#[cfg(test)]
use crate::audio::Request;
#[cfg(test)]
use std::{io::Write, os::unix::net::UnixStream};
pub use transport::{AuthorityConnection, Transport, TransportTiming};
use transport::{PollFd, poll};
pub(crate) use transport::{trace_add, trace_timing_enabled, trace_us};

struct Draft {
    measurement_basis: Option<crate::pa_measurement::wire::Basis>,
    fx_basis: Option<crate::fx::Observation>,
    monitor_device: Option<audio::MonitorDevicePin>,
    kind: String,
    body: Value,
    revision: String,
    generation: u64,
}
#[derive(Clone, Copy, Debug, Default)]
struct ProbeFrameTiming {
    bytes: u64,
    // receive wall, contract discriminator/dispatch, raw decode, telemetry/ingest
    micros: [u64; 4],
    kind: u8,
    receive_start_us: u64,
    transport_before: Option<TransportTiming>,
    transport_after: Option<TransportTiming>,
}
#[derive(Debug, Default)]
struct ProbeTiming {
    totals: [u64; 4],
    sends_us: u64,
    decision_us: u64,
    empty_receive_us: u64,
    frames: u64,
    overwritten: u64,
    overflow: bool,
    tail: [ProbeFrameTiming; 4],
}
impl ProbeTiming {
    fn record(&mut self, frame: ProbeFrameTiming) {
        for (total, amount) in self.totals.iter_mut().zip(frame.micros) {
            trace_add(total, amount, &mut self.overflow);
        }
        if self.frames >= 4 {
            trace_add(&mut self.overwritten, 1, &mut self.overflow);
        }
        self.tail[(self.frames % 4) as usize] = frame;
        trace_add(&mut self.frames, 1, &mut self.overflow);
    }
}
pub(crate) enum BrainOperationError {
    Admission(String),
    Fault(String),
}
impl BrainOperationError {
    pub(crate) fn message(self) -> String {
        match self {
            Self::Admission(s) | Self::Fault(s) => s,
        }
    }
}
impl From<String> for BrainOperationError {
    fn from(value: String) -> Self {
        Self::Fault(value)
    }
}
impl From<&str> for BrainOperationError {
    fn from(value: &str) -> Self {
        Self::Fault(value.into())
    }
}
#[derive(Default)]
struct BrainProbes {
    next: u64,
    pending: std::collections::VecDeque<(u64, Instant, u64)>,
    matched: Option<(u64, Instant, crate::brain::Snapshot)>,
    invalid: bool,
    first_fault: Option<String>,
}
struct HeldBaseline {
    identity: crate::held_proof::Identity,
    template: crate::held_proof::Request,
    dimensions: [u32; 6],
}
struct HeldQuery {
    request: crate::held_proof::Request,
    sent: Instant,
    generation: u64,
    dimensions: [u32; 6],
}
type FxNotice = Box<dyn FnMut(&Session) + Send>;
pub struct Operator {
    fx_notice: Option<FxNotice>,
    paired_nonce: u64,
    paired_enabled: bool,
    maintenance_replies: std::collections::VecDeque<crate::lease_maintenance::Reply>,
    held_baseline: Option<HeldBaseline>,
    held_query: Option<HeldQuery>,
    held_matched: Option<crate::held_proof::Matched>,
    held_reuse: Option<(u64, [u32; 6])>,
    held_nonce: u64,
    held_highwater: u64,
    held_observation: Option<crate::held_proof::Status>,
    held_refusal: Option<String>,
    brain_probes: BrainProbes,
    last_brain_send: Option<Instant>,
    trace_timing: bool,
    transport: Box<dyn AuthorityConnection>,
    pub(crate) session: Session,
    start: Instant,
    draft: Option<Draft>,
    scope: String,
    brain_signal: Option<std::sync::Arc<crate::brain::HoldSignal>>,
    guard: Option<(std::sync::Arc<std::sync::atomic::AtomicU64>, u64)>,
}
impl Operator {
    pub fn connect(
        endpoint: &Path,
        show: &str,
        epoch: u64,
        writer: &str,
        scope: &str,
    ) -> Result<Self, String> {
        Self::connect_version(endpoint, show, epoch, writer, scope, 1)
    }
    pub fn connect_remote(
        config: &crate::remote::Config,
        show: &str,
        epoch: u64,
        scope: &str,
    ) -> Result<Self, String> {
        let connection = crate::remote::Connection::connect(config, scope)?;
        if connection.source_epoch != epoch {
            return Err("authenticated source epoch differs from expected epoch".into());
        }
        let writer = connection.writer.clone();
        // QUIC has already reassembled the outer Response into a complete payload.
        // Applying the Unix frame assembler again would reject documents >64 KiB.
        Self::from_document_connection(Box::new(connection), show, epoch, &writer, scope, 2)
    }
    pub fn connect_version(
        endpoint: &Path,
        show: &str,
        epoch: u64,
        writer: &str,
        scope: &str,
        version: u8,
    ) -> Result<Self, String> {
        Self::from_connection_version(
            Box::new(Transport::connect(endpoint)?),
            show,
            epoch,
            writer,
            scope,
            version,
        )
    }
    /// Attach a previously authenticated connection read-only. Callers must use
    /// the authenticated writer identity; no grants or previous intent are copied.
    pub fn from_connection(
        transport: Box<dyn AuthorityConnection>,
        show: &str,
        epoch: u64,
        writer: &str,
        scope: &str,
    ) -> Result<Self, String> {
        Self::from_connection_version(transport, show, epoch, writer, scope, 1)
    }
    pub fn from_connection_version(
        transport: Box<dyn AuthorityConnection>,
        show: &str,
        epoch: u64,
        writer: &str,
        scope: &str,
        version: u8,
    ) -> Result<Self, String> {
        let transport = if version == 2 {
            Box::new(crate::pages::Connection::new(transport)) as Box<dyn AuthorityConnection>
        } else {
            transport
        };
        Self::from_document_connection(transport, show, epoch, writer, scope, version)
    }
    fn from_document_connection(
        transport: Box<dyn AuthorityConnection>,
        show: &str,
        epoch: u64,
        writer: &str,
        scope: &str,
        version: u8,
    ) -> Result<Self, String> {
        Ok(Self {
            fx_notice: None,
            paired_nonce: 0,
            paired_enabled: false,
            maintenance_replies: std::collections::VecDeque::new(),
            transport,
            session: Session::new_version(show, epoch, writer, scope, version)?,
            start: Instant::now(),
            draft: None,
            scope: scope.into(),
            guard: None,
            brain_signal: None,
            brain_probes: BrainProbes::default(),
            held_baseline: None,
            held_query: None,
            held_matched: None,
            held_reuse: None,
            held_nonce: 0,
            held_highwater: 0,
            held_observation: None,
            held_refusal: None,
            last_brain_send: None,
            trace_timing: trace_timing_enabled(),
        })
    }
    pub(crate) fn cancel(&mut self) {
        // Keep FIFO ordinals for replies already in flight, but none of their
        // observations may authorize input in the next controller generation.
        self.brain_probes.matched = None;
        self.held_matched = None;
        self.held_reuse = None;
        self.held_observation = None;
        self.draft = None;
        self.session.context_changed();
    }
    pub(crate) fn guard(
        &mut self,
        generation: std::sync::Arc<std::sync::atomic::AtomicU64>,
        expected: u64,
    ) {
        self.guard = Some((generation, expected));
    }
    pub(crate) fn brain_signal(&mut self, signal: std::sync::Arc<crate::brain::HoldSignal>) {
        self.brain_signal = Some(signal);
    }
    fn check_guard(&mut self) -> Result<(), String> {
        if self
            .brain_signal
            .as_ref()
            .is_some_and(|s| s.close.swap(false, std::sync::atomic::Ordering::AcqRel))
        {
            // Best effort bounded priority close, even when ordinary mutation is pending.
            // Heartbeats stop at the UI signal immediately; provider deadman is final bound.
            if let Err(error) = self.close_brain() {
                self.invalidate_brain_probes(&error);
                return Err(error);
            }
        }
        if self
            .guard
            .as_ref()
            .is_some_and(|(g, n)| g.load(std::sync::atomic::Ordering::Acquire) != *n)
        {
            Err("UNCERTAIN: input context revoked; submitted operation may have applied; no further send/retry".into())
        } else {
            Ok(())
        }
    }
    pub(crate) fn review_valid(&self) -> bool {
        self.draft.as_ref().is_some_and(|d| {
            d.fx_basis.as_ref().is_none_or(|b| self.session.fx.snapshot.as_ref().and_then(|s|s.observation.as_ref()).is_some_and(|o|b.same_basis(o)) && !self.session.fx.unknown && self.session.fx_fresh(self.now())) && d.measurement_basis.as_ref().is_none_or(|b| {
                self.session
                    .measurement
                    .snapshot
                    .as_ref()
                    .and_then(|s| s.current_basis.as_ref())
                    == Some(b)
            }) && self.session.generation() == d.generation
                && d.monitor_device
                    .as_ref()
                    .is_none_or(|pin| self.session.validate_monitor_device(pin).is_ok())
                && (d.kind != "master_eq_set" || self.session.live_eq_fresh(self.now()))
                && (d.kind != "send_tap_set" || self.session.sends_fresh(self.now()))
                && (d.kind != "processing_set" || self.session.processing_fresh(self.now()))
                && (!crate::structure::is_kind(&d.kind)
                    || self.session.structural_fresh(self.now()))
                && (d.kind != "device_configure"
                    || self
                        .session
                        .validate_device_intent(&d.body, self.now())
                        .is_ok())
                && (!crate::brain::is_kind(&d.kind) || self.session.brain_fresh(self.now()))
                && (d.kind != "release_preview"
                    || self
                        .session
                        .preview(self.now())
                        .is_some_and(|p| d.body["token"] == p.token))
                && self
                    .session
                    .snapshot
                    .as_ref()
                    .is_some_and(|s| s.authority.revision == d.revision)
        })
    }
    pub(crate) fn reviewed(&self) -> Option<String> {
        self.draft.as_ref().map(|d| {
            if let Some(b)=&d.fx_basis { let m:crate::fx::Mutation=serde_json::from_value(d.body.clone()).expect("validated FX review"); return format!("{}\nAuthority revision {} / scope {} / show {} / epoch {}",crate::fx::review_lines(b,&m),d.revision,self.scope,self.session.snapshot_request().context.show_id,self.session.snapshot_request().context.epoch); }
            if d.kind=="pa_set" {return format!("MUTED WHOLE PA CONFIGURATION / revision {} / program buses {} / explicit rearm remains separate\n{}",d.revision,d.body["program_buses"],d.body["configuration_json"].as_str().and_then(|s|serde_json::from_str::<Value>(s).ok()).and_then(|v|serde_json::to_string_pretty(&v).ok()).unwrap_or_default());}
            if d.kind=="device_configure" {return format!("DEVICE CONFIGURATION / revision {} / separate rearm required\n{}",d.revision,serde_json::to_string_pretty(&d.body["config"]).unwrap_or_default());}
            if d.kind == "processing_set" {
                let config = crate::processing::decode_config(&d.body["config"]).expect("validated draft");
                return format!("APPLY channel {} / FOH EQ then compressor / processed sends also follow this DSP / revision {} / show {} / epoch {}\n{}\n240-frame output crossfade; Enter confirms complete replacement; Esc cancels",
                    d.body["input"], d.revision, self.session.snapshot_request().context.show_id, self.session.snapshot_request().context.epoch,
                    crate::processing::FIELDS.iter().map(|f| config.display(*f)).collect::<Vec<_>>().join("\n"));
            }
            format!(
                "{} {} / revision {} / scope {} / show {} / epoch {}",
                d.kind,
                d.body,
                d.revision,
                self.scope,
                self.session.snapshot_request().context.show_id,
                self.session.snapshot_request().context.epoch
            )
        })
    }
    pub(crate) fn now(&self) -> u64 {
        self.start
            .elapsed()
            .as_millis()
            .try_into()
            .unwrap_or(u64::MAX)
    }
    fn telemetry(&mut self, r: &audio::Reply) -> Result<bool, String> {
        if r.context != self.session.snapshot_request().context {
            return Ok(false);
        }
        if r.state != "final" || r.outcome.as_ref().is_none_or(|o| o.kind != "applied") {
            return Err("snapshot telemetry outcome".into());
        }
        let snapshot = r.snapshot.clone().ok_or("rendered telemetry missing")?;
        self.session.ingest_snapshot(snapshot, self.now())
    }
    /// Dispatch by contract before touching the shared correlation domain.
    fn processing_document(
        &mut self,
        document: crate::provider::StrictDocument,
    ) -> Result<Option<crate::provider::StrictDocument>, String> {
        let contract = match document.value().get("contract") {
            None | Some(serde_json::Value::Null) => None,
            Some(serde_json::Value::String(s)) => Some(s.as_str()),
            _ => return Err("contract discriminator type".into()),
        };
        if contract == Some("GP07-processing")
            && document.value()["version"] != 4
            && document.admitted_bytes() > crate::provider::MAX_BYTES
        {
            return Err("legacy processing capacity".into());
        }
        if matches!(
            contract,
            Some(
                "GP21-fx"
                    | "GP20-measurement"
                    | "GP15-device"
                    | "GP15-brain"
                    | "GP15-held-proof"
                    | "GP15-lease-maintenance"
                    | "GP14-structure"
                    | "GP07-processing"
                    | "GP18-sends"
                    | "GP18-master-eq"
            )
        ) {
            self.processing_frame(&document.into_bytes()?)?;
            Ok(None)
        } else {
            Ok(Some(document))
        }
    }
    fn processing_frame(&mut self, bytes: &[u8]) -> Result<bool, String> {
        // Discriminator only: skipped content is never trusted. The selected
        // strict decoder still checks every field, duplicate and depth bound.
        #[derive(serde::Deserialize)]
        struct Contract {
            contract: Option<String>,
        }
        let tag: Contract = serde_json::from_slice(bytes).map_err(|e| e.to_string())?;
        if tag.contract.as_deref() == Some(crate::fx::CONTRACT) {
            let v=crate::provider::parse_document(bytes)?;
            if v["state"] == "snapshot" { crate::fx::Snapshot::decode(v)?; return Ok(true); }
            if v["state"] == "authorization_replayed" && self.session.pending.as_ref().is_some_and(|p|p.request.kind=="fx_configure") { self.session.fx_unknown("authorization replay is not DSP application"); return Err("FX authorization replay".into()); }
            self.session.dispatch_fx(crate::fx::Reply::decode(v)?)?; return Ok(true);
        }
        if tag.contract.as_deref() == Some(crate::pa_measurement::wire::CONTRACT) {
            self.session
                .dispatch_measurement(crate::pa_measurement::wire::Reply::decode(bytes)?)?;
            return Ok(true);
        }
        if tag.contract.as_deref() == Some(crate::live_eq::CONTRACT) {
            let reply = crate::live_eq::decode_reply(bytes)?;
            self.session.dispatch_live_eq(reply, self.now())?;
            return Ok(true);
        }
        if tag.contract.as_deref() == Some(crate::sends::CONTRACT) {
            let reply = crate::sends::decode_reply(bytes)?;
            self.session.dispatch_sends(reply, self.now())?;
            return Ok(true);
        }
        if tag.contract.as_deref() == Some(crate::lease_maintenance::CONTRACT) {
            let reply = crate::lease_maintenance::Reply::decode(bytes)?;
            if self.maintenance_replies.contains(&reply) {
                // Exact completed retries never extend expiry or observation age.
                return Ok(true);
            }
            return Err("unknown or changed completed maintenance reply".into());
        }
        if tag.contract.as_deref() == Some(crate::held_proof::CONTRACT) {
            let query = self.held_query.as_ref().ok_or("unmatched held proof")?;
            let reply = crate::held_proof::Reply::decode(bytes, query.dimensions)?;
            if reply.context != query.request {
                return Err("held proof query/context mismatch".into());
            }
            let query = self.held_query.take().unwrap();
            if query.generation != self.session.generation() {
                return Ok(true);
            }
            if reply.witness.is_none() {
                self.held_baseline = None;
                self.held_matched = None;
                self.held_reuse = None;
                self.held_observation = None;
                self.held_refusal = Some(format!("held proof refused: {:?}", reply.reason));
                return Ok(true);
            }
            let matched = crate::held_proof::Matched::admit(
                reply,
                &query.request,
                query.sent,
                query.generation,
            )?;
            self.check_held_floors(&matched)?;
            let w = matched.witness();
            let highwater = crate::provider::counter(&w.brain.hold_generation_counter)?;
            self.held_highwater = highwater;
            self.held_observation = Some(crate::held_proof::Status {
                generation: w.brain.held_generation.clone(),
                source_frame: w.source_frame.clone(),
                revision: w.revision.clone(),
                talkback_path_ready: w.brain.talkback_path_ready,
                media_authorized: w.media_authorized,
                observed: query.sent,
                valid_until: query.sent
                    + Duration::from_millis(u64::from(w.lease_remaining_ms).min(50)),
            });
            self.held_matched = Some(matched);
            // Only service_held may mark its completed final query reusable.
            self.held_reuse = None;
            return Ok(true);
        }
        if tag.contract.as_deref() == Some("GP15-device") {
            self.session.dispatch_device(bytes, self.now())?;
            return Ok(true);
        }
        if tag.contract.as_deref() == Some("GP15-brain") {
            let result: Result<(), String> = (|| {
                let reply = crate::brain::decode_reply(bytes)?;
                let snapshot = (reply.state == "snapshot")
                    .then(|| reply.snapshot.clone())
                    .flatten();
                self.session.dispatch_brain(reply, self.now())?;
                if let Some(snapshot) = snapshot {
                    if self.brain_probes.invalid {
                        return Err("Brain probe provenance invalid".into());
                    }
                    let (id, sent, generation) = self
                        .brain_probes
                        .pending
                        .pop_front()
                        .ok_or("unmatched Brain snapshot probe")?;
                    if generation == self.session.generation() {
                        self.brain_probes.matched = Some((id, sent, snapshot));
                    }
                }
                Ok(())
            })();
            if let Err(e) = result {
                self.invalidate_brain_probes(&e);
                let _ = self.close_brain();
                return Err(e);
            }
            return Ok(true);
        }
        if tag.contract.as_deref() == Some("GP14-structure") {
            let reply = crate::structure::decode_reply(bytes)?;
            if reply.context == self.session.structural_request().context {
                if reply.state != "snapshot" {
                    return Err(format!("structural query unavailable: {:?}", reply.reason));
                }
                self.session.ingest_structural(
                    reply
                        .snapshot
                        .ok_or_else(|| format!("structural unavailable: {:?}", reply.reason))?,
                    self.now(),
                )?;
            } else {
                self.session.dispatch_structural(reply, self.now())?;
            }
            return Ok(true);
        }
        if tag.contract.as_deref() != Some("GP07-processing") {
            return Ok(false);
        }
        let r = crate::processing::decode_reply(bytes)?;
        if r.version != audio::processing_version(self.session.snapshot_request().version) {
            return Err("processing version differs from selected session".into());
        }
        if r.context == self.session.processing_request().context {
            if let Some(s) = r.snapshot {
                self.session.ingest_processing(s, self.now())?;
            } else {
                return Err(format!("processing unavailable: {:?}", r.reason));
            }
        } else if self
            .session
            .pending
            .as_ref()
            .is_some_and(|p| p.request.kind == "processing_set" && p.request.context == r.context)
        {
            self.session.accept_processing(r, self.now())?;
        } else if r.context.show_id != self.session.snapshot_request().context.show_id
            || r.context.epoch != self.session.snapshot_request().context.epoch
        {
            return Err("unrelated processing session".into());
        }
        // Cached old replies never ingest snapshots or renew freshness.
        Ok(true)
    }
    pub(crate) fn refresh_device(&mut self) -> Result<(), String> {
        self.check_guard()?;
        let deadline = Instant::now() + Duration::from_millis(250);
        let sent = self.now();
        self.transport
            .send_frame_until(&self.session.device_request().encode()?, deadline)?;
        for _ in 0..64 {
            self.check_guard()?;
            if Instant::now() >= deadline {
                break;
            }
            if let Some(bytes) = self.transport.receive_until(deadline)? {
                let v: serde_json::Value =
                    serde_json::from_slice(&bytes).map_err(|e| e.to_string())?;
                let observation = v["contract"] == "GP15-device" && v["state"] == "snapshot";
                if Instant::now() >= deadline {
                    break;
                }
                if !self.processing_frame(&bytes)? {
                    let r = audio::decode_reply(&bytes)?;
                    self.telemetry(&r)?;
                }
                if observation {
                    self.session.anchor_device_receipt(sent);
                    self.check_guard()?;
                    if Instant::now() >= deadline {
                        break;
                    }
                    return Ok(());
                }
            }
        }
        self.session.invalidate_device();
        Err("device observation deadline".into())
    }
    fn invalidate_brain_probes(&mut self, reason: &str) {
        self.brain_probes
            .first_fault
            .get_or_insert_with(|| reason.into());
        self.held_baseline = None;
        self.held_matched = None;
        self.held_reuse = None;
        self.held_observation = None;
        self.held_query = None;
        self.brain_probes.invalid = true;
        self.brain_probes.pending.clear();
        self.brain_probes.matched = None;
        self.session.invalidate_brain_observation();
        if let Some(signal) = &self.brain_signal {
            signal.release();
        }
    }
    fn send_brain_probe(&mut self, deadline: Instant) -> Result<u64, String> {
        if self.brain_probes.invalid || self.brain_probes.pending.len() >= 64 {
            self.invalidate_brain_probes("Brain probe provenance/queue bound");
            let _ = self.close_brain();
            return Err(self.brain_probes.first_fault.clone().unwrap());
        }
        let id = self
            .brain_probes
            .next
            .checked_add(1)
            .ok_or("Brain probe exhausted")?;
        let bytes = self.session.brain_request().encode()?;
        let sent = Instant::now();
        self.brain_probes.next = id;
        self.brain_probes
            .pending
            .push_back((id, sent, self.session.generation()));
        if let Err(e) = self.transport.send_frame_until(&bytes, deadline) {
            self.invalidate_brain_probes(&e);
            let _ = self.close_brain();
            return Err(e);
        }
        Ok(id)
    }
    fn read_paired(
        &mut self,
        deadline: Instant,
        remaining: &mut usize,
    ) -> Result<(), BrainOperationError> {
        self.brain_guard()?;
        if Instant::now() >= deadline || *remaining == 0 {
            return Err("paired original budget exhausted".into());
        }
        let identity = self
            .transport
            .held_identity()
            .ok_or("paired authenticated attachment unavailable")?;
        self.paired_nonce = self
            .paired_nonce
            .checked_add(1)
            .ok_or("paired nonce exhausted")?;
        let request = self.session.paired_request(&identity, self.paired_nonce)?;
        let bytes = request.encode()?;
        let sent = self.now();
        let generation = self.session.generation();
        let deadline = deadline.min(Instant::now() + Duration::from_millis(250));
        self.transport.send_frame_until(&bytes, deadline)?;
        while Instant::now() < deadline && *remaining > 0 {
            self.brain_guard()?;
            if let Some(document) = self.transport.receive_document_until(deadline)? {
                *remaining -= 1;
                if document.value()["contract"] == crate::paired_readback::CONTRACT {
                    let reply = crate::paired_readback::Reply::decode_document(document)?;
                    self.brain_guard()?;
                    if Instant::now() >= deadline
                        || self.transport.held_identity().as_ref() != Some(&identity)
                    {
                        return Err("paired deadline/attachment changed".into());
                    }
                    let mut candidate = self.session.clone();
                    candidate.accept_paired(reply, &request, sent, generation, self.now())?;
                    self.brain_guard()?;
                    if Instant::now() >= deadline {
                        return Err("paired validation deadline".into());
                    }
                    self.session = candidate;
                    self.pin_held_baseline()?;
                    return Ok(());
                }
                if let Some(document) = self.processing_document(document)? {
                    // Old raw observations are validated but cannot install half a pair.
                    let reply = audio::decode_reply_document(document)?;
                    if reply.context != self.session.snapshot_request().context {
                        self.session.accept(reply, self.now())?;
                    }
                }
            } else {
                std::thread::sleep(
                    deadline
                        .saturating_duration_since(Instant::now())
                        .min(Duration::from_millis(1)),
                );
            }
        }
        Err("paired deadline/document budget".into())
    }
    fn maintain(
        &mut self,
        deadline: Instant,
        remaining: &mut usize,
        held: bool,
    ) -> Result<(), String> {
        let result = self.maintain_inner(deadline, remaining, held);
        if result.is_err() {
            // Unknown outcomes cannot be guessed with a new ID, and late replies
            // cannot re-admit authority after cancellation or transport failure.
            self.session.disconnect();
            self.held_baseline = None;
            self.held_matched = None;
            self.held_reuse = None;
            self.held_observation = None;
        }
        result
    }
    fn maintain_inner(
        &mut self,
        deadline: Instant,
        remaining: &mut usize,
        held: bool,
    ) -> Result<(), String> {
        // Selecting GP15 is not successful readback. A failed explicit probe
        // remains terminal even on the renewal path that deliberately skips refresh.
        if let Some(reason) = &self.brain_probes.first_fault {
            return Err(reason.clone());
        }
        self.probe_guard(held)
            .map_err(BrainOperationError::message)?;
        let identity = self
            .transport
            .held_identity()
            .ok_or("atomic maintenance unsupported attachment")?;
        let deadline = deadline
            .min(Instant::now() + Duration::from_millis(1900))
            .min(
                self.start
                    + Duration::from_millis(
                        self.session
                            .lease_deadline()
                            .ok_or("maintenance no lease")?,
                    ),
            );
        loop {
            self.probe_guard(held)
                .map_err(BrainOperationError::message)?;
            if Instant::now() >= deadline || *remaining == 0 {
                return Err("maintenance operation budget".into());
            }
            let request = self.session.begin_maintenance(&identity, self.now())?;
            let bytes = request.encode()?;
            let operation_deadline = deadline.min(
                self.start
                    + Duration::from_millis(self.session.maintenance.as_ref().unwrap().deadline),
            );
            self.transport.send_frame_until(
                &bytes,
                operation_deadline.min(Instant::now() + Duration::from_millis(200)),
            )?;
            loop {
                self.probe_guard(held)
                    .map_err(BrainOperationError::message)?;
                if self.transport.held_identity().as_ref() != Some(&identity) {
                    return Err("maintenance attachment changed".into());
                }
                let now = self.now();
                let p = self
                    .session
                    .maintenance
                    .as_mut()
                    .ok_or("maintenance canceled")?;
                if Instant::now() >= deadline || now >= p.deadline || *remaining == 0 {
                    return Err("maintenance uncertain deadline".into());
                }
                let wakes = [100, 250, 500];
                if p.retry < wakes.len() && now >= p.first_send + wakes[p.retry] {
                    p.retry += 1;
                    self.transport
                        .send_frame_until(&bytes, operation_deadline)?;
                    continue;
                }
                let wake = if p.retry < wakes.len() {
                    (self.start + Duration::from_millis(p.first_send + wakes[p.retry]))
                        .min(operation_deadline)
                } else {
                    operation_deadline
                };
                if let Some(document) = self.transport.receive_document_until(wake)? {
                    *remaining -= 1;
                    if document.value()["contract"] == crate::lease_maintenance::CONTRACT {
                        let reply = crate::lease_maintenance::Reply::decode_document(document)?;
                        // Validate received identity before a simultaneous key-up
                        // can classify this as cancellation. Do not install it yet.
                        if !self.maintenance_replies.contains(&reply)
                            && self
                                .session
                                .maintenance
                                .as_ref()
                                .is_none_or(|p| p.request != reply.context)
                        {
                            return Err("maintenance reply context mismatch".into());
                        }
                        self.probe_guard(held)
                            .map_err(BrainOperationError::message)?;
                        if Instant::now() >= deadline
                            || self.transport.held_identity().as_ref() != Some(&identity)
                        {
                            return Err("maintenance decode deadline/attachment".into());
                        }
                        if self.maintenance_replies.contains(&reply) {
                            continue;
                        }
                        let reason = self.session.accept_maintenance(reply.clone(), self.now())?;
                        if self.maintenance_replies.len() == 64 {
                            self.maintenance_replies.pop_front();
                        }
                        self.maintenance_replies.push_back(reply);
                        match reason {
                            None => return Ok(()),
                            Some(reason) if reason == "unavailable" => {
                                // Only this explicit terminal result permits a new ID.
                                std::thread::sleep(
                                    deadline
                                        .saturating_duration_since(Instant::now())
                                        .min(Duration::from_millis(1)),
                                );
                                break;
                            }
                            Some(reason) => return Err(format!("maintenance refused: {reason}")),
                        }
                    }
                    if let Some(document) = self.processing_document(document)? {
                        let reply = audio::decode_reply_document(document)?;
                        if reply.context != self.session.snapshot_request().context {
                            return Err("unexpected maintenance reply".into());
                        }
                    }
                } else {
                    std::thread::sleep(
                        wake.saturating_duration_since(Instant::now())
                            .min(Duration::from_millis(1)),
                    );
                }
            }
        }
    }
    pub(crate) fn refresh_brain(&mut self) -> Result<(), String> {
        self.refresh_brain_until(Instant::now() + Duration::from_millis(250))
    }
    fn finish_brain_operation(
        &mut self,
        result: Result<(), BrainOperationError>,
    ) -> Result<(), String> {
        match result {
            Ok(()) => Ok(()),
            Err(BrainOperationError::Admission(reason)) => {
                // Keep all in-flight FIFO identities. A new controller generation
                // cannot use their observations as its matched authorization.
                self.cancel();
                self.session.invalidate_brain_observation();
                if let Some(signal) = &self.brain_signal {
                    signal.release();
                }
                if let Err(error) = self.close_brain() {
                    self.invalidate_brain_probes(&error);
                    return Err(error);
                }
                Err(reason)
            }
            Err(BrainOperationError::Fault(reason)) => {
                self.invalidate_brain_probes(&reason);
                let _ = self.close_brain();
                Err(self.brain_probes.first_fault.clone().unwrap())
            }
        }
    }
    fn brain_guard(&mut self) -> Result<(), BrainOperationError> {
        if self
            .brain_signal
            .as_ref()
            .is_some_and(|s| s.close.swap(false, std::sync::atomic::Ordering::AcqRel))
        {
            self.close_brain().map_err(BrainOperationError::Fault)?;
        }
        if self
            .guard
            .as_ref()
            .is_some_and(|(g, n)| g.load(std::sync::atomic::Ordering::Acquire) != *n)
        {
            return Err(BrainOperationError::Admission(
                "Brain input context revoked".into(),
            ));
        }
        Ok(())
    }
    fn refresh_brain_until(&mut self, deadline: Instant) -> Result<(), String> {
        let result = self.refresh_brain_inner(deadline);
        self.finish_brain_operation(result)
    }
    fn refresh_brain_inner(&mut self, deadline: Instant) -> Result<(), BrainOperationError> {
        if self.transport.held_identity().is_some() {
            self.paired_enabled = true;
            self.read_paired(deadline, &mut 64)
        } else {
            self.refresh_brain_budget(deadline, &mut 64, false)
        }
    }
    fn probe_guard(&mut self, held_mode: bool) -> Result<(), BrainOperationError> {
        if held_mode {
            self.held_guard()
        } else {
            self.brain_guard()
        }
    }
    fn refresh_brain_budget(
        &mut self,
        deadline: Instant,
        remaining: &mut usize,
        held_mode: bool,
    ) -> Result<(), BrainOperationError> {
        let began = Instant::now();
        let mut timing = self.trace_timing.then(ProbeTiming::default);
        let transport_before = timing
            .as_ref()
            .and_then(|_| self.transport.timing_snapshot());
        let budget = deadline.saturating_duration_since(began);
        self.probe_guard(held_mode)?;
        let send_started = timing.as_ref().map(|_| Instant::now());
        let mut probe = self.send_brain_probe(deadline)?;
        let mut requested_revision = None;
        if held_mode && !self.session.fresh(self.now()) {
            // A pending final already made raw readback necessary. Pipeline it
            // behind our timestamped Brain query rather than waiting for that
            // reply first. Raw has no query identity: only exact revision pairing
            // with the independently matched Brain probe can authorize anything.
            self.probe_guard(held_mode)?;
            requested_revision = self.session.brain.as_ref().map(|b| b.revision.clone());
            self.transport
                .send_frame_until(&self.session.snapshot_request().encode()?, deadline)?;
        }
        if let (Some(t), Some(start)) = (&mut timing, send_started) {
            trace_add(&mut t.sends_us, trace_us(start.elapsed()), &mut t.overflow);
        }
        let mut loops = 0usize;
        let mut processed = 0usize;
        for _ in 0..64 {
            loops += 1;
            self.probe_guard(held_mode)?;
            if Instant::now() >= deadline {
                break;
            }
            if *remaining == 0 {
                break;
            }
            let receive_started = timing.as_ref().map(|_| Instant::now());
            let frame_transport_before = timing
                .as_ref()
                .and_then(|_| self.transport.timing_snapshot());
            if let Some(document) = self.transport.receive_document_until(deadline)? {
                let mut frame = ProbeFrameTiming {
                    bytes: document.admitted_bytes() as u64,
                    receive_start_us: receive_started
                        .map_or(0, |start| trace_us(start.saturating_duration_since(began))),
                    transport_before: frame_transport_before,
                    transport_after: timing
                        .as_ref()
                        .and_then(|_| self.transport.timing_snapshot()),
                    ..Default::default()
                };
                if let Some(start) = receive_started {
                    frame.micros[0] = trace_us(start.elapsed());
                }
                let dispatch_started = timing.as_ref().map(|_| Instant::now());
                *remaining -= 1;
                let raw_document = self.processing_document(document)?;
                let contract = raw_document.is_none();
                if let Some(start) = dispatch_started {
                    frame.micros[1] = trace_us(start.elapsed());
                }
                frame.kind = if contract { 1 } else { 2 };
                if let Some(document) = raw_document {
                    let decode_started = timing.as_ref().map(|_| Instant::now());
                    let r = audio::decode_reply_document(document)?;
                    if let Some(start) = decode_started {
                        frame.micros[2] = trace_us(start.elapsed());
                    }
                    let ingest_started = timing.as_ref().map(|_| Instant::now());
                    self.telemetry(&r)?;
                    if let Some(start) = ingest_started {
                        frame.micros[3] = trace_us(start.elapsed());
                    }
                }
                if let Some(t) = &mut timing {
                    t.record(frame);
                }
                let decision_started = timing.as_ref().map(|_| Instant::now());
                processed += 1;
                // Validate bytes/correlation before observing concurrent cancellation:
                // a focus change must never hide malformed or partial wire data.
                self.probe_guard(held_mode)?;
                if Instant::now() >= deadline {
                    break;
                }
                // Another writer may advance the common revision between our
                // Brain reply and its raw pair. Raw cannot regress: obtain a new
                // read-only probe within this operation's original frame/time budget.
                if self
                    .brain_probes
                    .matched
                    .as_ref()
                    .is_some_and(|(id, _, matched)| {
                        *id == probe
                            && self.session.snapshot.as_ref().is_some_and(|raw| {
                                raw.authority.revision.parse::<u64>().unwrap()
                                    > matched.revision.parse::<u64>().unwrap()
                            })
                    })
                {
                    let started = timing.as_ref().map(|_| Instant::now());
                    probe = self.send_brain_probe(deadline)?;
                    if let (Some(t), Some(start)) = (&mut timing, started) {
                        trace_add(&mut t.sends_us, trace_us(start.elapsed()), &mut t.overflow);
                    }
                }
                if let Some(brain) = &self.session.brain
                    && (!self.session.fresh(self.now())
                        || self.session.snapshot.as_ref().is_none_or(|raw| {
                            raw.authority.revision.parse::<u64>().unwrap()
                                < brain.revision.parse::<u64>().unwrap()
                        }))
                    && requested_revision.as_ref() != Some(&brain.revision)
                {
                    requested_revision = Some(brain.revision.clone());
                    let started = timing.as_ref().map(|_| Instant::now());
                    self.transport
                        .send_frame_until(&self.session.snapshot_request().encode()?, deadline)?;
                    if let (Some(t), Some(start)) = (&mut timing, started) {
                        trace_add(&mut t.sends_us, trace_us(start.elapsed()), &mut t.overflow);
                    }
                }
                if self.session.brain_fresh(self.now())
                    && self
                        .brain_probes
                        .matched
                        .as_ref()
                        .is_some_and(|(id, _, snapshot)| {
                            *id == probe
                                && self
                                    .session
                                    .snapshot
                                    .as_ref()
                                    .is_some_and(|raw| raw.authority.revision == snapshot.revision)
                        })
                {
                    self.pin_held_baseline()?;
                    return Ok(());
                }
                if let (Some(t), Some(start)) = (&mut timing, decision_started) {
                    trace_add(
                        &mut t.decision_us,
                        trace_us(start.elapsed()),
                        &mut t.overflow,
                    );
                }
            } else if let (Some(t), Some(start)) = (&mut timing, receive_started) {
                trace_add(
                    &mut t.empty_receive_us,
                    trace_us(start.elapsed()),
                    &mut t.overflow,
                );
            }
        }
        self.probe_guard(held_mode)?;
        let at = Instant::now();
        let now = self.now();
        let pending = self.brain_probes.pending.iter().find(|p| p.0 == probe);
        let matched = self.brain_probes.matched.as_ref();
        // Only validated numeric identities/revisions and bounded scalar state:
        // never include a packet, credential, destination list or unbounded queue.
        let mut failure = format!(
            "Brain paired observation deadline: elapsed_ms={} budget_ms={} processed={} loops={} probe={} probe_age_ms={:?} probe_generation={:?} generation={} matched_id={:?} matched_age_ms={:?} matched_revision={:?} matched_generation={:?} raw_revision={:?} raw_age_ms={:?} raw_fresh={} brain_revision={:?} brain_age_ms={:?} brain_fresh={} requested_revision={:?} pending={}",
            at.saturating_duration_since(began).as_millis(),
            budget.as_millis(),
            processed,
            loops,
            probe,
            pending
                .map(|p| at.saturating_duration_since(p.1).as_millis())
                .or_else(|| matched
                    .filter(|m| m.0 == probe)
                    .map(|m| at.saturating_duration_since(m.1).as_millis())),
            pending.map(|p| p.2).or_else(|| matched
                .filter(|m| m.0 == probe)
                .map(|_| self.session.generation())),
            self.session.generation(),
            matched.map(|m| m.0),
            matched.map(|m| at.saturating_duration_since(m.1).as_millis()),
            matched.map(|m| m.2.revision.as_str()),
            matched.map(|_| self.session.generation()),
            self.session
                .snapshot
                .as_ref()
                .map(|r| r.authority.revision.as_str()),
            self.session.snapshot_age(now),
            self.session.fresh(now),
            self.session.brain.as_ref().map(|b| b.revision.as_str()),
            self.session.brain_age(now),
            self.session.brain_fresh(now),
            requested_revision,
            self.brain_probes.pending.len(),
        );
        if let Some(t) = timing {
            use std::fmt::Write;
            let _ = write!(
                failure,
                " timing_totals_us={:?} sends_us={} decision_us={} empty_receive_us={} frames={} overwritten={} overflow={} transport_before={transport_before:?} transport_after={:?}",
                t.totals,
                t.sends_us,
                t.decision_us,
                t.empty_receive_us,
                t.frames,
                t.overwritten,
                t.overflow,
                self.transport.timing_snapshot()
            );
            for (slot, frame) in t.tail.iter().enumerate() {
                let _ = write!(
                    failure,
                    " tail{slot}=(bytes={},kind={},start_us={},stages_us={:?},before={:?},after={:?})",
                    frame.bytes,
                    frame.kind,
                    frame.receive_start_us,
                    frame.micros,
                    frame.transport_before,
                    frame.transport_after
                );
            }
        }
        Err(failure.into())
    }
    fn pin_held_baseline(&mut self) -> Result<(), String> {
        if self.scope != "talkback_destinations"
            || !self.session.can_pin_held_baseline(self.now())
            || !self.session.brain_fresh(self.now())
        {
            return Ok(());
        }
        let Some(identity) = self.transport.held_identity() else {
            return Ok(());
        };
        let raw = self.session.snapshot.as_ref().ok_or("held baseline raw")?;
        let topology = raw.topology.as_ref().ok_or("held baseline topology")?;
        if identity.epoch != raw.authority.epoch
            || crate::provider::counter(&identity.map)? != topology.map_revision
        {
            return Err("held baseline attachment/map".into());
        }
        let dimensions = [
            raw.authority.inputs.len(),
            raw.authority.monitors.len(),
            topology.pa_outputs,
            topology.capture_channels,
            topology.playback_channels,
            topology.sample_rate as usize,
        ]
        .map(|n| u32::try_from(n).map_err(|_| "held baseline dimensions"));
        let dimensions = [
            dimensions[0]?,
            dimensions[1]?,
            dimensions[2]?,
            dimensions[3]?,
            dimensions[4]?,
            dimensions[5]?,
        ];
        let digest = crate::held_proof::configuration_digest(
            &raw.authority.show_id,
            &identity,
            dimensions,
            self.session.brain.as_ref().unwrap(),
        )?;
        let template = self.session.held_query(&identity, &digest, 1, self.now())?;
        self.held_baseline = Some(HeldBaseline {
            identity,
            template,
            dimensions,
        });
        Ok(())
    }
    pub(crate) fn held_transport_authenticated(&self) -> bool {
        self.transport.held_identity().is_some()
    }
    pub(crate) fn held_baseline_ready(&self) -> bool {
        self.held_baseline.as_ref().is_some_and(|b| {
            self.transport.held_identity().as_ref() == Some(&b.identity)
                && self.session.snapshot.as_ref().is_some_and(|raw| {
                    raw.authority.epoch == b.identity.epoch
                        && raw.topology.as_ref().is_some_and(|t| {
                            b.identity.map == t.map_revision.to_string()
                                && [
                                    raw.authority.inputs.len() as u64,
                                    raw.authority.monitors.len() as u64,
                                    t.pa_outputs as u64,
                                    t.capture_channels as u64,
                                    t.playback_channels as u64,
                                    u64::from(t.sample_rate),
                                ] == b.dimensions.map(u64::from)
                        })
                        && self.session.brain.as_ref().is_some_and(|brain| {
                            crate::held_proof::configuration_digest(
                                &raw.authority.show_id,
                                &b.identity,
                                b.dimensions,
                                brain,
                            )
                            .as_ref()
                                == Ok(&b.template.expected_config_digest)
                        })
                })
                && self
                    .session
                    .held_query(
                        &b.identity,
                        &b.template.expected_config_digest,
                        1,
                        self.now(),
                    )
                    .as_ref()
                    == Ok(&b.template)
        })
    }
    pub(crate) fn held_status(&self) -> Option<crate::held_proof::Status> {
        self.held_observation
            .as_ref()
            .filter(|s| s.fresh())
            .cloned()
    }
    fn compact_read(
        &mut self,
        deadline: Instant,
        remaining: &mut usize,
    ) -> Result<(), BrainOperationError> {
        self.held_guard()?;
        if self.brain_probes.invalid {
            return Err(self
                .brain_probes
                .first_fault
                .clone()
                .unwrap_or("held proof invalid".into())
                .into());
        }
        if !self.held_baseline_ready() {
            self.held_baseline = None;
            return Err(BrainOperationError::Admission("held proof unsupported or accepted baseline missing; full readback and new gesture required".into()));
        }
        self.held_matched = None;
        self.held_reuse = None;
        // Drain a canceled query before allocating a new nonce. Never resynchronize.
        while self.held_query.is_some() {
            self.compact_receive(deadline, remaining)?;
        }
        self.held_guard()?;
        if let Some(reason) = self.held_refusal.take() {
            return Err(BrainOperationError::Admission(reason));
        }
        if Instant::now() >= deadline || *remaining == 0 {
            return Err("held proof observation deadline/frame bound".into());
        }
        let baseline = self
            .held_baseline
            .as_ref()
            .ok_or("held baseline invalidated")?;
        self.held_nonce = self
            .held_nonce
            .checked_add(1)
            .ok_or("held query nonce exhausted")?;
        let request = self.session.held_query(
            &baseline.identity,
            &baseline.template.expected_config_digest,
            self.held_nonce,
            self.now(),
        )?;
        let sent = Instant::now();
        self.held_query = Some(HeldQuery {
            request: request.clone(),
            sent,
            generation: self.session.generation(),
            dimensions: baseline.dimensions,
        });
        self.transport
            .send_frame_until(&request.encode()?, deadline)?;
        while self.held_query.is_some() {
            self.compact_receive(deadline, remaining)?;
        }
        self.held_guard()?;
        if let Some(reason) = self.held_refusal.take() {
            return Err(BrainOperationError::Admission(reason));
        }
        if Instant::now() >= deadline
            || self
                .held_matched
                .as_ref()
                .is_none_or(|m| !m.fresh() || m.generation() != self.session.generation())
        {
            return Err("held proof observation deadline/context".into());
        }
        Ok(())
    }
    fn compact_receive(
        &mut self,
        deadline: Instant,
        remaining: &mut usize,
    ) -> Result<(), BrainOperationError> {
        self.held_guard()?;
        if Instant::now() >= deadline || *remaining == 0 {
            return Err("held proof observation deadline/frame bound".into());
        }
        if let Some(document) = self.transport.receive_document_until(deadline)? {
            *remaining -= 1;
            if let Some(raw) = self.processing_document(document)? {
                let reply = audio::decode_reply_document(raw)?;
                self.telemetry(&reply)?;
            }
            // Strict bytes/correlation take priority over simultaneous cancellation.
            self.held_guard()?;
        } else {
            self.held_guard()?;
            std::thread::sleep(
                deadline
                    .saturating_duration_since(Instant::now())
                    .min(Duration::from_millis(1)),
            );
        }
        self.held_guard()?;
        if Instant::now() >= deadline {
            return Err("held proof observation deadline".into());
        }
        Ok(())
    }
    fn check_held_floors(&self, matched: &crate::held_proof::Matched) -> Result<(), String> {
        let w = matched.witness();
        let highwater = crate::provider::counter(&w.brain.hold_generation_counter)?;
        let known = self
            .session
            .brain
            .as_ref()
            .map(|b| crate::provider::counter(&b.hold_generation_counter))
            .transpose()?
            .unwrap_or(0);
        if highwater < known.max(self.held_highwater) {
            return Err("held proof generation highwater regressed".into());
        }
        let revision = crate::provider::counter(&w.revision)?;
        let frame = crate::provider::counter(&w.source_frame)?;
        if self.session.snapshot.as_ref().is_some_and(|s| {
            revision < crate::provider::counter(&s.authority.revision).unwrap()
                || frame < crate::provider::counter(&s.frame).unwrap()
        }) || self.session.brain.as_ref().is_some_and(|s| {
            revision < crate::provider::counter(&s.revision).unwrap()
                || frame < crate::provider::counter(&s.frame).unwrap()
        }) {
            return Err("held proof regresses known authority".into());
        }
        if self.held_observation.as_ref().is_some_and(|old| {
            crate::provider::counter(&w.revision).unwrap()
                < crate::provider::counter(&old.revision).unwrap()
                || crate::provider::counter(&w.source_frame).unwrap()
                    < crate::provider::counter(&old.source_frame).unwrap()
        }) {
            return Err("regressive held proof".into());
        }
        Ok(())
    }
    /// Only the unused final proof from this live gesture is a reuse candidate.
    /// Invalid context is terminal; only age/absence/superseded floors may query again.
    fn reuse_post_proof(
        &mut self,
        intent: u64,
        generation: u64,
    ) -> Result<bool, BrainOperationError> {
        self.held_gesture_guard(intent)?;
        if self.brain_probes.invalid || self.brain_probes.first_fault.is_some() {
            return Err(self
                .brain_probes
                .first_fault
                .clone()
                .unwrap_or("held proof invalid".into())
                .into());
        }
        if let Some(reason) = &self.held_refusal {
            return Err(BrainOperationError::Admission(reason.clone()));
        }
        if self.held_query.is_some()
            || self.session.pending.is_some()
            || self.session.maintenance.is_some()
        {
            return Err(BrainOperationError::Admission(
                "held reuse outstanding operation".into(),
            ));
        }
        if !self.held_baseline_ready() {
            self.held_baseline = None;
            return Err(BrainOperationError::Admission(
                "held baseline changed; full readback and new gesture required".into(),
            ));
        }
        let Some(proof) = self.held_matched.as_ref() else {
            return Ok(false);
        };
        let Some((old_intent, dimensions)) = self.held_reuse else {
            self.held_matched = None;
            return Ok(false);
        };
        let baseline = self.held_baseline.as_ref().unwrap();
        let current = self.session.held_query(
            &baseline.identity,
            &baseline.template.expected_config_digest,
            crate::provider::counter(&proof.request().query_id)?,
            self.now(),
        )?;
        if old_intent != intent
            || dimensions != baseline.dimensions
            || proof.generation() != self.session.generation()
            || current != *proof.request()
        {
            return Err(BrainOperationError::Admission(
                "held reuse context changed".into(),
            ));
        }
        let w = proof.witness();
        if generation == 0
            || w.brain.held_generation.as_deref() != Some(generation.to_string().as_str())
            || !w.media_authorized
            || w.brain.talkback_mute
            || (w.brain.talkback_foh && !w.foh_authorized)
        {
            return Err(BrainOperationError::Admission(
                "held reuse generation/authorization".into(),
            ));
        }
        if !proof.fresh() || self.check_held_floors(proof).is_err() {
            self.held_matched = None;
            self.held_reuse = None;
            return Ok(false);
        }
        Ok(true)
    }
    fn proved_request(
        &mut self,
        kind: &str,
        generation: Option<u64>,
    ) -> Result<(audio::Request, u64, Instant), BrainOperationError> {
        self.held_guard()?;
        let proof = self.held_matched.take().ok_or("held own proof missing")?;
        self.held_reuse = None;
        self.check_held_floors(&proof)?;
        if !self.held_baseline_ready()
            || self.held_baseline.as_ref().is_none_or(|b| {
                b.template.expected_config_digest != proof.request().expected_config_digest
            })
        {
            return Err(BrainOperationError::Admission(
                "held baseline changed".into(),
            ));
        }
        let deadline = (Instant::now() + Duration::from_millis(20))
            .min(proof.sent() + Duration::from_millis(50))
            .min(
                proof.sent() + Duration::from_millis(u64::from(proof.witness().lease_remaining_ms)),
            );
        let (request, generation) = self
            .session
            .begin_proved_held(kind, &proof, generation, self.now())
            .map_err(BrainOperationError::Admission)?;
        let deadline = deadline.min(
            self.start
                + Duration::from_millis(
                    self.session
                        .pending_authority_deadline()
                        .ok_or("held authority expired")?,
                ),
        );
        if Instant::now() >= deadline {
            return Err("held authority expired".into());
        }
        Ok((request, generation, deadline))
    }
    pub(crate) fn start_held(&mut self) -> Result<u64, String> {
        let result = (|| -> Result<u64, BrainOperationError> {
            self.compact_read(Instant::now() + Duration::from_millis(30), &mut 64)?;
            let (request, generation, deadline) = self.proved_request("brain_hold", None)?;
            let sent = Instant::now();
            self.transport
                .send_frame_until(&request.encode()?, deadline)?;
            self.last_brain_send = Some(sent);
            Ok(generation)
        })();
        match result {
            Ok(g) => Ok(g),
            Err(e) => Err(self.finish_brain_operation(Err(e)).unwrap_err()),
        }
    }
    pub(crate) fn held_send_anchor(&self) -> Option<Instant> {
        self.last_brain_send
    }
    fn held_guard(&mut self) -> Result<(), BrainOperationError> {
        self.brain_guard()?;
        if self
            .brain_signal
            .as_ref()
            .is_some_and(|signal| !signal.live())
        {
            return Err(BrainOperationError::Admission("held input released".into()));
        }
        Ok(())
    }
    fn held_gesture_guard(&mut self, intent: u64) -> Result<(), BrainOperationError> {
        self.held_guard()?;
        if self
            .brain_signal
            .as_ref()
            .is_none_or(|s| !s.live_id(intent))
        {
            return Err(BrainOperationError::Admission(
                "held gesture changed".into(),
            ));
        }
        Ok(())
    }
    // Only correlated completion is accepted. Empty first-byte polls are wakes;
    // partial frame errors remain fatal and are never retried here.
    fn settle_held_pending(
        &mut self,
        deadline: Instant,
        remaining: &mut usize,
    ) -> Result<(), BrainOperationError> {
        while self.session.pending.is_some() {
            self.held_guard()?;
            if Instant::now() >= deadline || *remaining == 0 {
                return Err("held completion budget exhausted".into());
            }
            let receive_deadline = deadline.min(
                self.start
                    + Duration::from_millis(
                        self.session
                            .pending_authority_deadline()
                            .ok_or("held authority expired")?,
                    ),
            );
            if Instant::now() >= receive_deadline {
                return Err("held authority expired".into());
            }
            if let Some(document) = self.transport.receive_document_until(receive_deadline)? {
                *remaining -= 1;
                let expected_brain = self
                    .session
                    .pending
                    .as_ref()
                    .filter(|p| matches!(p.request.kind.as_str(), "brain_hold" | "brain_heartbeat"))
                    .map(|p| p.request.context.clone());
                if let Some(document) = self.processing_document(document)? {
                    let reply = audio::decode_reply_document(document)?;
                    if reply.context == self.session.snapshot_request().context {
                        self.telemetry(&reply)?;
                    } else {
                        let refused = reply.state == "final"
                            && reply.outcome.as_ref().is_some_and(|o| o.kind != "applied");
                        self.session.accept(reply, self.now())?;
                        if refused {
                            return Err("held renewal refused".into());
                        }
                    }
                }
                if self.session.pending.is_none()
                    && let Some(expected) = expected_brain
                    && let Some(final_reply) = self.session.brain_final.as_ref()
                    && final_reply.context == expected
                    && let Some(reason) = &final_reply.reason
                {
                    return Err(BrainOperationError::Admission(format!(
                        "held Brain refused: {reason}"
                    )));
                }
                self.held_guard()?;
            } else {
                std::thread::sleep(
                    deadline
                        .saturating_duration_since(Instant::now())
                        .min(Duration::from_millis(1)),
                );
            }
        }
        if Instant::now() >= deadline {
            return Err("held completion deadline".into());
        }
        Ok(())
    }
    /// The real worker's entire held maintenance path. No passive250ms poll or
    /// ordinary1900ms mutation wait is reachable while this service owns the hold.
    pub(crate) fn service_held(&mut self, generation: u64, intent: u64) -> Result<Instant, String> {
        let began = Instant::now();
        let initial_anchor = self.last_brain_send;
        let result = (|| -> Result<Instant, BrainOperationError> {
            if self.brain_probes.invalid || self.brain_probes.first_fault.is_some() {
                return Err(self
                    .brain_probes
                    .first_fault
                    .clone()
                    .unwrap_or("held proof invalid".into())
                    .into());
            }
            let deadline =
                self.last_brain_send.ok_or("held send anchor missing")? + Duration::from_millis(50);
            if Instant::now() >= deadline {
                return Err("held service arrived late".into());
            }
            self.held_gesture_guard(intent)?;
            let mut remaining = 64;
            self.settle_held_pending(deadline, &mut remaining)?;
            if !self.reuse_post_proof(intent, generation)? {
                self.compact_read(
                    deadline.min(Instant::now() + Duration::from_millis(30)),
                    &mut remaining,
                )?;
            }
            self.held_gesture_guard(intent)?;
            let (request, _, probe_deadline) =
                self.proved_request("brain_heartbeat", Some(generation))?;
            self.held_gesture_guard(intent)?;
            let sent = Instant::now();
            self.transport
                .send_frame_until(&request.encode()?, deadline.min(probe_deadline))?;
            self.last_brain_send = Some(sent);
            // Completion and due renewal use the NEXT period, anchored to actual
            // send start, not to work completion or a fresh per-read timeout.
            let next = sent + Duration::from_millis(50);
            self.settle_held_pending(next, &mut remaining)?;
            self.held_gesture_guard(intent)?;
            if self.session.renewal_due(self.now()) {
                self.maintain(
                    next.min(Instant::now() + Duration::from_millis(20)),
                    &mut remaining,
                    true,
                )?;
            }
            self.held_gesture_guard(intent)?;
            self.compact_read(
                next.min(Instant::now() + Duration::from_millis(30)),
                &mut remaining,
            )?;
            self.held_gesture_guard(intent)?;
            self.held_reuse = Some((intent, self.held_baseline.as_ref().unwrap().dimensions));
            Ok(sent)
        })();
        match result {
            Ok(sent) => Ok(sent),
            Err(error) => {
                let timing = format!(
                    "held_elapsed_ms={} initial_send_age_ms={:?} last_send_age_ms={:?}",
                    began.elapsed().as_millis(),
                    initial_anchor.map(|t| t.elapsed().as_millis()),
                    self.last_brain_send.map(|t| t.elapsed().as_millis())
                );
                let error = match error {
                    BrainOperationError::Admission(reason) => {
                        BrainOperationError::Admission(format!("{reason}; {timing}"))
                    }
                    BrainOperationError::Fault(reason) => {
                        BrainOperationError::Fault(format!("{reason}; {timing}"))
                    }
                };
                let result = self.finish_brain_operation(Err(error));
                if let Some(signal) = &self.brain_signal {
                    signal.release();
                }
                let _ = self.close_brain();
                result.map(|()| Instant::now())
            }
        }
    }
    pub(crate) fn close_brain(&mut self) -> Result<(), String> {
        self.held_matched = None;
        self.held_reuse = None;
        self.held_baseline = None;
        self.held_observation = None;
        if self
            .session
            .brain
            .as_ref()
            .is_none_or(|b| b.held_generation.is_none())
            && self.session.pending.as_ref().is_none_or(|p| {
                !matches!(p.request.kind.as_str(), "brain_hold" | "brain_heartbeat")
            })
        {
            return Ok(());
        }
        let r = self.session.brain_close_request(self.now())?;
        self.transport
            .send_frame_until(&r.encode()?, Instant::now() + Duration::from_millis(20))
    }
    pub(crate) fn refresh_structural(&mut self) -> Result<(), String> {
        let result = self.refresh_structural_inner();
        if result.is_err() {
            // A later raw-only poll cannot turn a failed pair into fresh structure.
            self.session.invalidate_structural_observation();
        }
        result
    }
    fn refresh_structural_inner(&mut self) -> Result<(), String> {
        self.check_guard()?;
        let started = Instant::now();
        let deadline = started + Duration::from_millis(250);
        let trace = std::env::var_os("GP14_TIMING").is_some();
        // Raw state can expire while the structural response is in flight. Request
        // both observations within one budget; never wait for an unrequested raw reply.
        self.transport
            .send_frame_until(&self.session.snapshot_request().encode()?, deadline)?;
        if trace {
            eprintln!("GP14 pair raw_sent_us={}", started.elapsed().as_micros());
        }
        self.transport
            .send_frame_until(&self.session.structural_request().encode()?, deadline)?;
        if trace {
            eprintln!(
                "GP14 pair sent elapsed_us={}",
                started.elapsed().as_micros()
            );
        }
        let (mut raw, mut structural) = (false, false);
        let mut count = 0;
        while Instant::now() < deadline && count < 64 {
            self.check_guard()?;
            if let Some(bytes) = self.transport.receive_until(deadline)? {
                count += 1;
                let received = started.elapsed();
                #[derive(serde::Deserialize)]
                struct Tag {
                    contract: Option<String>,
                    state: Option<String>,
                }
                let tag: Tag = serde_json::from_slice(&bytes).map_err(|e| e.to_string())?;
                if tag.contract.as_deref() == Some("GP14-structure")
                    && tag.state.as_deref() == Some("snapshot")
                {
                    let reply = crate::structure::decode_reply(&bytes)?;
                    if reply.context != self.session.structural_request().context {
                        return Err("structural query context".into());
                    }
                    if trace {
                        eprintln!(
                            "GP14 structural decoded_us={} incoming_revision={} incoming_frame={:?}",
                            started.elapsed().as_micros(),
                            reply.revision,
                            reply.snapshot.as_ref().map(|s| &s.frame)
                        );
                    }
                    if Instant::now() >= deadline {
                        break;
                    }
                    structural |= self.session.ingest_structural(
                        reply.snapshot.ok_or("structural readback missing")?,
                        self.now(),
                    )?;
                } else if !self.processing_frame(&bytes)? {
                    let reply = audio::decode_reply(&bytes)?;
                    if trace {
                        eprintln!(
                            "GP14 raw decoded_us={} incoming={:?}",
                            started.elapsed().as_micros(),
                            reply.snapshot.as_ref().map(|s| (
                                &s.authority.sequence,
                                &s.authority.revision,
                                &s.frame
                            ))
                        );
                    }
                    if Instant::now() >= deadline {
                        break;
                    }
                    raw |= self.telemetry(&reply)?;
                }
                if trace {
                    eprintln!(
                        "GP14 pair frame={count} bytes={} contract={:?} received_us={} decoded_us={} total_us={} raw={raw} structural={structural} coherent_fresh={}",
                        bytes.len(),
                        tag.contract,
                        received.as_micros(),
                        (started.elapsed() - received).as_micros(),
                        started.elapsed().as_micros(),
                        self.session.structural_fresh(self.now())
                    );
                    eprintln!(
                        "GP14 admitted raw={:?} structural={:?}",
                        self.session.snapshot.as_ref().map(|s| (
                            &s.authority.sequence,
                            &s.authority.revision,
                            &s.frame
                        )),
                        self.session
                            .structural
                            .as_ref()
                            .map(|s| (&s.revision, &s.frame))
                    );
                }
                if raw
                    && structural
                    && Instant::now() < deadline
                    && self.session.structural_fresh(self.now())
                {
                    return Ok(());
                }
            }
        }
        Err(format!(
            "structural snapshot deadline/queue bound (frames={count} elapsed_ms={} raw={raw} structural={structural} coherent_fresh={})",
            started.elapsed().as_millis(),
            self.session.structural_fresh(self.now())
        ))
    }
    pub(crate) fn refresh_processing(&mut self) -> Result<(), String> {
        self.check_guard()?;
        self.transport.send(&self.session.processing_request())?;
        let previous = self.session.processing.as_ref().map(|s| s.sequence.clone());
        let deadline = Instant::now() + Duration::from_millis(250);
        let mut count = 0;
        let mut latest_raw = None;
        while Instant::now() < deadline && count < 64 {
            self.check_guard()?;
            if let Some(bytes) = self.transport.receive_until(deadline)? {
                count += 1;
                if !self.processing_frame(&bytes)? {
                    latest_raw = Some(bytes);
                }
                if self.session.processing.as_ref().map(|s| &s.sequence) != previous.as_ref() {
                    if let Some(bytes) = latest_raw {
                        self.telemetry(&audio::decode_reply(&bytes)?)?;
                    }
                    return Ok(());
                }
            }
        }
        if let Some(bytes) = latest_raw {
            self.telemetry(&audio::decode_reply(&bytes)?)?;
        }
        Err(format!(
            "processing snapshot deadline/queue bound ({count} frames)"
        ))
    }
    pub(crate) fn refresh(&mut self) -> Result<(), String> {
        self.refresh_classified()
            .map_err(BrainOperationError::message)
    }
    pub(crate) fn refresh_classified(&mut self) -> Result<(), BrainOperationError> {
        self.refresh_required(false)
    }
    fn refresh_after_final(&mut self) -> Result<(), String> {
        // A correlated completion and observation admission are separate results.
        // Never turn failed/partial readback into successful mutation admission.
        let completed = self.session.last_result.clone();
        let generation = self.session.generation();
        match self.refresh_required(true) {
            Ok(()) => Ok(()),
            Err(error) => {
                self.session.invalidate_raw_observation();
                self.session.invalidate_brain_observation();
                let same_context = self.session.generation() == generation
                    && self
                        .guard
                        .as_ref()
                        .is_none_or(|(g, n)| g.load(std::sync::atomic::Ordering::Acquire) == *n);
                let reason = self.finish_brain_operation(Err(error)).unwrap_err();
                if same_context {
                    let result = format!(
                        "correlated completion: {completed}; fresh post-final readback unavailable: {reason}"
                    );
                    self.session.last_result = result.clone();
                    Err(result)
                } else {
                    Err(reason)
                }
            }
        }
    }
    fn refresh_required(&mut self, required_readback: bool) -> Result<(), BrainOperationError> {
        if let Some(reason) = &self.brain_probes.first_fault {
            return Err(BrainOperationError::Fault(reason.clone()));
        }
        if self.paired_enabled {
            return self.read_paired(Instant::now() + Duration::from_millis(250), &mut 64);
        }
        // One total budget covers queued replies, a final's required readback,
        // solicited frames and decoding. Never replay a mutation here.
        let deadline = Instant::now() + Duration::from_millis(250);
        let baseline = required_readback
            .then(|| self.session.snapshot.clone())
            .flatten();
        let mut remaining = 64usize;
        let mut requested = None;
        let mut next_frame = None;
        let mut continuation = required_readback;
        let mut first_batch = true;
        loop {
            self.brain_guard()?;
            if Instant::now() >= deadline {
                return Err(BrainOperationError::Admission(
                    "snapshot deadline/queue bound".into(),
                ));
            }
            let mut frames = Vec::new();
            if let Some(bytes) = next_frame.take() {
                frames.push(bytes);
            }
            let mut drained = false;
            while frames.len() < remaining && Instant::now() < deadline {
                if let Some(bytes) = self.transport.receive_available_until(deadline)? {
                    frames.push(bytes);
                } else {
                    drained = true;
                    break;
                }
            }
            // Prove the queue drained before admitting any state from this batch.
            if !drained {
                return Err(BrainOperationError::Admission(
                    "snapshot backlog saturated; freshness not admitted".into(),
                ));
            }
            // Legacy unsolicited raw-only batches must not trigger a query when
            // all their observations are stale/regressive. Initial quiet stale
            // attachment and actual contract readback may wait for a new frame.
            if first_batch && frames.is_empty() {
                continuation = true;
            }
            first_batch = false;
            remaining -= frames.len();
            let mut raw_frames = Vec::new();
            for bytes in frames {
                if Instant::now() >= deadline {
                    return Err(BrainOperationError::Admission(
                        "snapshot deadline/queue bound".into(),
                    ));
                }
                // Stateful replies retain wire order, including priority close.
                if self.processing_frame(&bytes)? {
                    continuation = true;
                } else {
                    raw_frames.push(bytes);
                }
            }
            for bytes in raw_frames.into_iter().rev() {
                if Instant::now() >= deadline {
                    return Err(BrainOperationError::Admission(
                        "snapshot deadline/queue bound".into(),
                    ));
                }
                let r = audio::decode_reply(&bytes)?;
                if Instant::now() >= deadline {
                    return Err(BrainOperationError::Admission(
                        "snapshot deadline/queue bound".into(),
                    ));
                }
                if r.context == self.session.snapshot_request().context {
                    if required_readback
                        && (r.state != "final"
                            || r.outcome.as_ref().is_none_or(|o| o.kind != "applied")
                            || r.snapshot.is_none())
                    {
                        return Err("snapshot telemetry outcome/readback".into());
                    }
                    let advancing = baseline.as_ref().is_none_or(|old| {
                        r.snapshot.as_ref().is_some_and(|new| {
                            crate::provider::counter(&new.authority.sequence).unwrap()
                                > crate::provider::counter(&old.authority.sequence).unwrap()
                                && crate::provider::counter(&new.frame).unwrap()
                                    >= crate::provider::counter(&old.frame).unwrap()
                        })
                    });
                    if advancing && self.telemetry(&r)? {
                        break;
                    }
                } else if r.context.show_id != self.session.snapshot_request().context.show_id
                    || r.context.epoch != self.session.snapshot_request().context.epoch
                {
                    return Err("unrelated snapshot session".into());
                }
            }
            self.brain_guard()?;
            let brain_observed = self.session.brain_age(self.now()).is_some()
                || (required_readback && self.session.brain.is_some());
            if Instant::now() < deadline
                && self.session.fresh(self.now())
                && (!brain_observed || self.session.brain_fresh(self.now()))
            {
                return Ok(());
            }
            if (!continuation && !brain_observed) || remaining == 0 || Instant::now() >= deadline {
                return Err(BrainOperationError::Admission(
                    "snapshot deadline/queue bound".into(),
                ));
            }
            // A valid final can arrive before its raw observation. Request the
            // missing pair once per observed revision, inside the original budget.
            let revisions = (
                self.session
                    .snapshot
                    .as_ref()
                    .map(|s| s.authority.revision.clone()),
                self.session.brain.as_ref().map(|s| s.revision.clone()),
            );
            if requested.as_ref() != Some(&revisions) {
                self.transport
                    .send_frame_until(&self.session.snapshot_request().encode()?, deadline)?;
                if brain_observed {
                    self.send_brain_probe(deadline)?;
                }
                requested = Some(revisions);
            }
            self.brain_guard()?;
            if Instant::now() >= deadline {
                return Err(BrainOperationError::Admission(
                    "snapshot deadline/queue bound".into(),
                ));
            }
            // Include the delayed frame with the next drained FIFO batch, so
            // contract ordering and newest-first raw coalescing stay identical.
            next_frame = self.transport.receive_until(deadline)?;
        }
    }
    fn uses_atomic_maintenance(&self) -> bool {
        // Explicit GP15 selection belongs to this Operator attachment. A generic
        // authenticated/held identity alone must not change legacy renewal.
        self.paired_enabled
            || matches!(
                self.scope.as_str(),
                "local_operator_monitor" | "talkback_destinations" | "talkback_foh"
            )
    }
    pub(crate) fn mutate(&mut self, kind: &str, body: Value) -> Result<(), String> {
        if kind == "renew" && self.uses_atomic_maintenance() {
            return self.mutate_inner(kind, body);
        }
        self.refresh()?;
        if self.session.renewal_due(self.now()) && kind != "renew" {
            self.mutate_inner("renew", json!({}))?;
            self.refresh()?;
        }
        self.mutate_inner(kind, body)
    }
    pub(crate) fn mutate_inner(&mut self, kind: &str, body: Value) -> Result<(), String> {
        if kind == "fx_configure" { return self.mutate_fx(body); }
        let deadline = Instant::now() + Duration::from_millis(1900);
        if kind == "renew" && self.uses_atomic_maintenance() {
            return self.maintain(deadline, &mut 64, false);
        }
        self.mutate_ordinary_inner(kind, body, deadline)
    }
    fn mutate_ordinary_inner(
        &mut self,
        kind: &str,
        body: Value,
        deadline: Instant,
    ) -> Result<(), String> {
        let r = self.session.begin(kind, body, self.now())?;
        self.check_guard()?;
        self.transport.send_frame_until(
            &r.encode()?,
            deadline
                .min(Instant::now() + Duration::from_millis(200))
                .min(
                    self.start
                        + Duration::from_millis(
                            self.session
                                .pending_authority_deadline()
                                .ok_or("pending authority unavailable")?,
                        ),
                ),
        )?;
        loop {
            self.check_guard()?;
            if Instant::now() >= deadline {
                return Err("pending/uncertain deadline; no applied claim".into());
            }
            let authority_deadline = self.start
                + Duration::from_millis(
                    self.session
                        .pending_authority_deadline()
                        .ok_or("pending authority unavailable")?,
                );
            if Instant::now() >= authority_deadline {
                return Err("pending authority expired; no further retry".into());
            }
            // Service the existing byte-identical retry schedule before blocking.
            // A silent backpressure peer must not hide the100/250/500ms wakes.
            if let Some(retry) = self.session.retry(self.now()) {
                self.transport
                    .send_frame_until(&retry.encode()?, deadline.min(authority_deadline))?;
            }
            let now = self.now();
            let wake = self
                .session
                .retry_wake(now)
                .ok_or("pending authority expired/uncertain; no further retry")?;
            if wake <= now {
                // At most the three existing retries can already be due.
                continue;
            }
            let receive_deadline = deadline.min(self.start + Duration::from_millis(wake));
            match self.transport.receive_until(receive_deadline) {
                Ok(Some(b)) => {
                    if self.processing_frame(&b)? {
                        if self.session.pending.is_none() {
                            if kind == "device_configure"
                                && self
                                    .session
                                    .device_final
                                    .as_ref()
                                    .is_some_and(|r| r.state != "applied_device")
                            {
                                return Err(
                                    "device configuration failed; actual device not applied".into(),
                                );
                            }
                            if crate::brain::is_kind(kind)
                                && let Some(reason) = self
                                    .session
                                    .brain_final
                                    .as_ref()
                                    .and_then(|r| r.reason.as_ref())
                            {
                                return Err(format!("Brain refused: {reason}"));
                            }
                            if crate::pa_measurement::wire::mutation(kind)
                                && let Some(reason) = self
                                    .session
                                    .measurement
                                    .final_reply
                                    .as_ref()
                                    .and_then(|r| r.reason.as_ref())
                            {
                                return Err(format!("measurement refused: {reason}"));
                            }
                            // Structural map commits may deliberately close the old
                            // authenticated session after its final. Preserve that
                            // correlated completion independently of the next refresh.
                            if !crate::structure::is_kind(kind) {
                                self.refresh_after_final()?;
                            }
                            return Ok(());
                        }
                    } else {
                        match audio::decode_reply(&b) {
                            Ok(reply) => {
                                if self.telemetry(&reply)? {
                                } else if self
                                    .session
                                    .pending
                                    .as_ref()
                                    .is_some_and(|p| p.request.context == reply.context)
                                {
                                    let renewal_refusal =
                                        if kind == "renew" && reply.state == "final" {
                                            reply
                                                .outcome
                                                .as_ref()
                                                .filter(|o| o.kind != "applied")
                                                .map(|o| format!("{}: {:?}", o.kind, o.body.reason))
                                        } else {
                                            None
                                        };
                                    if let Err(e) = self.session.accept(reply, self.now()) {
                                        eprintln!("refused reply; pending retained: {e}");
                                    }
                                    if self.session.pending.is_none() {
                                        if let Some(reason) = renewal_refusal {
                                            return Err(format!("renewal refused: {reason}"));
                                        }
                                        if !self.session.fresh(self.now()) {
                                            self.refresh_after_final()?;
                                        }
                                        return Ok(());
                                    }
                                }
                            }
                            Err(e) => eprintln!("invalid reply refused; pending retained: {e}"),
                        }
                    }
                }
                Ok(None) => {
                    // Abstract/fake connections may return before their deadline.
                    // Avoid spinning without delaying the next scheduled wake.
                    let pause = receive_deadline
                        .saturating_duration_since(Instant::now())
                        .min(Duration::from_millis(10));
                    if !pause.is_zero() {
                        std::thread::sleep(pause);
                    }
                }
                Err(e) => {
                    self.session.disconnect();
                    return Err(format!("uncertain operation; transport closed: {e}"));
                }
            }
            // No-first-byte timeout is a scheduler wake. Partial-frame timeout
            // remains an error above and retires the session; it is never retried.
        }
    }
    fn wait(&mut self, ms: u64) -> Result<(), String> {
        if ms > 2000 {
            return Err("wait maximum2000ms".into());
        }
        let deadline = Instant::now() + Duration::from_millis(ms);
        while Instant::now() < deadline {
            self.refresh()?;
            if self.session.renewal_due(self.now()) {
                self.mutate_inner("renew", json!({}))?;
            }
            std::thread::sleep(
                deadline
                    .saturating_duration_since(Instant::now())
                    .min(Duration::from_millis(50)),
            );
        }
        self.refresh()
    }
    pub(crate) fn stage(&mut self, kind: &str, body: Value) -> Result<(), String> {
        if self.draft.is_some() {
            return Err("confirm or cancel existing draft".into());
        }
        let monitor_device = if audio::monitor_armed(kind, &body) {
            Some(audio::MonitorDevicePin::capture(
                self.session.device.as_ref(),
            )?)
        } else {
            None
        };
        if kind == "device_configure" {
            self.session.validate_device_intent(&body, self.now())?;
        }
        // Pin the queued review's context before any refresh can observe a newer
        // revision. The frontend already checked its original queued revision.
        let processing_context = if kind == "processing_set"
            || matches!(kind, "send_tap_set" | "master_eq_set")
            || crate::structure::is_kind(kind)
            || crate::brain::is_kind(kind)
            || kind == "device_configure"
        {
            Some((
                self.session
                    .snapshot
                    .as_ref()
                    .ok_or("snapshot")?
                    .authority
                    .revision
                    .clone(),
                self.session.generation(),
            ))
        } else {
            None
        };
        if !self.session.fresh(self.now()) {
            self.refresh()?;
        }
        if let Some((revision, generation)) = processing_context {
            if kind == "processing_set" {
                crate::processing::validate_body_version(
                    &body,
                    self.session.snapshot_request().version,
                )?;
                self.refresh_processing()?;
            } else if matches!(kind, "send_tap_set" | "master_eq_set") {
                if kind == "master_eq_set" {
                    self.refresh_live_eq()?;
                } else {
                    self.refresh_sends()?;
                }
            } else if kind == "device_configure" {
                self.refresh_device()?;
                self.session.validate_device_intent(&body, self.now())?;
            } else if crate::brain::is_kind(kind) {
                self.refresh_brain()?;
                let raw = self.session.snapshot.as_ref().ok_or("snapshot")?;
                crate::brain::validate_body(
                    kind,
                    &body,
                    self.session.brain.as_ref(),
                    raw.authority.inputs.len(),
                    raw.authority.monitors.len(),
                )?;
            } else {
                self.refresh_structural()?;
                crate::structure::validate_body(kind, &body, self.session.structural.as_ref())?;
            }
            self.refresh()?;
            if let Some(pin) = &monitor_device {
                self.refresh_monitor_device(pin)?;
            }
            if self.session.generation() != generation
                || self
                    .session
                    .snapshot
                    .as_ref()
                    .is_none_or(|s| s.authority.revision != revision)
            {
                return Err("processing review context changed during paired refresh".into());
            }
            if (kind == "master_eq_set" && !self.session.live_eq_fresh(self.now()))
                || (kind == "send_tap_set" && !self.session.sends_fresh(self.now()))
                || (kind == "processing_set" && !self.session.processing_fresh(self.now()))
                || (crate::structure::is_kind(kind) && !self.session.structural_fresh(self.now()))
            {
                return Err("fresh paired module state required".into());
            }
        }
        // The raw drain may include a newer device observation too.
        if kind == "device_configure" {
            self.session.validate_device_intent(&body, self.now())?;
        }
        let s = self.session.snapshot.as_ref().ok_or("snapshot")?;
        println!(
            "CONFIRM exact scope/context/targets: {kind} {body} scope={} show={} epoch={} revision={} frame={}",
            self.scope, s.authority.show_id, s.authority.epoch, s.authority.revision, s.frame
        );
        self.draft = Some(Draft {
            measurement_basis: None,
            fx_basis: None,
            monitor_device,
            kind: kind.into(),
            body,
            revision: s.authority.revision.clone(),
            generation: self.session.generation(),
        });
        Ok(())
    }
    fn refresh_monitor_device(&mut self, pin: &audio::MonitorDevicePin) -> Result<(), String> {
        // Check before and after I/O: refresh must never adopt a replacement.
        self.session.validate_monitor_device(pin)?;
        self.refresh_device()?;
        self.check_guard()?;
        self.session.validate_monitor_device(pin)?;
        if !self.session.device_fresh(self.now()) {
            return Err("fresh actual monitor device observation required".into());
        }
        Ok(())
    }
    pub(crate) fn confirm(&mut self) -> Result<(), String> {
        let d = self
            .draft
            .take()
            .ok_or("no confirmation draft; held/repeated confirm refused")?;
        if !self.session.fresh(self.now()) {
            self.refresh()?;
        }
        let s = self.session.snapshot.as_ref().ok_or("snapshot")?;
        if s.authority.revision != d.revision || self.session.generation() != d.generation {
            return Err("context/revision changed; draft discarded".into());
        }
        if self.session.renewal_due(self.now()) {
            self.mutate_inner("renew", json!({}))?;
            self.refresh()?;
            if self
                .session
                .snapshot
                .as_ref()
                .is_none_or(|s| s.authority.revision != d.revision)
                || self.session.generation() != d.generation
            {
                return Err("reviewed confirmation context changed during renewal".into());
            }
        }
        if d.kind == "processing_set"
            || matches!(d.kind.as_str(), "send_tap_set" | "master_eq_set")
            || crate::structure::is_kind(&d.kind)
            || crate::brain::is_kind(&d.kind)
            || d.kind == "device_configure"
        {
            if d.kind == "processing_set" {
                self.refresh_processing()?;
            } else if matches!(d.kind.as_str(), "send_tap_set" | "master_eq_set") {
                if d.kind == "master_eq_set" {
                    self.refresh_live_eq()?;
                } else {
                    self.refresh_sends()?;
                }
            } else if d.kind == "device_configure" {
                self.refresh_device()?;
                self.session.validate_device_intent(&d.body, self.now())?;
            } else if crate::brain::is_kind(&d.kind) {
                self.refresh_brain()?;
            } else {
                self.refresh_structural()?;
            }
            self.refresh()?;
            if let Some(pin) = &d.monitor_device {
                self.refresh_monitor_device(pin)?;
            }
            if self.session.generation() != d.generation
                || self
                    .session
                    .snapshot
                    .as_ref()
                    .is_none_or(|s| s.authority.revision != d.revision)
            {
                return Err("processing confirmation context changed during paired refresh".into());
            }
        }
        if let Some(basis) = &d.fx_basis {
            self.refresh_fx(false)?;
            if !self.session.fx_fresh(self.now()) || self.session.fx.unknown || self.session.fx.snapshot.as_ref().and_then(|s|s.observation.as_ref()).is_none_or(|o|!basis.same_basis(o)) || self.session.snapshot.as_ref().is_none_or(|s|s.authority.revision!=d.revision) || self.session.generation()!=d.generation { return Err("FX confirmation basis/context changed; new review required".into()); }
        }
        if let Some(basis) = &d.measurement_basis {
            self.refresh_measurement(None)?;
            if self
                .session
                .measurement
                .snapshot
                .as_ref()
                .and_then(|s| s.current_basis.as_ref())
                != Some(basis)
                || self
                    .session
                    .snapshot
                    .as_ref()
                    .is_none_or(|s| s.authority.revision != d.revision)
                || self.session.generation() != d.generation
            {
                return Err(
                    "measurement review basis changed; explicit new review required".into(),
                );
            }
        }
        // No refresh between the final revision check and begin; the request pins it.
        self.mutate_inner(&d.kind, d.body)
    }
    pub fn status(&self) -> Value {
        let s = self.session.snapshot.as_ref();
        json!({"mode":"real-local-offline-provider","scope":self.scope,"faulted":s.map(|s|s.faulted),"authority":s.map(|s|json!({"show_id":s.authority.show_id,"epoch":s.authority.epoch,"inputs":s.authority.inputs,"monitors":s.authority.monitors,"modes":s.authority.modes,"parameters":s.authority.parameters.iter().map(|p|json!({"target":audio::target_value(&p.target),"actual_integer_db":null,"target_value":p.target_value,"proposal":p.proposal,"hold":p.hold,"owner":p.owner})).collect::<Vec<_>>()})),"fresh":self.session.fresh(self.now()),"lease_deadline_local_ms":self.session.lease_deadline(),"result":self.session.last_result,"pending":self.session.pending.as_ref().map(|p|format!("{:?}",p.state)),"frame":s.map(|s|&s.frame),"revision":s.map(|s|&s.authority.revision),"protection":"offline-unprotected","meters":null,"coefficients":s.map(|s|s.coefficients.iter().map(|c|json!({"input":c.input,"current_nanogain":c.current_nanogain.iter().map(|n|n.0).collect::<Vec<_>>(),"ramp_target_nanogain":c.ramp_target_nanogain.iter().map(|n|n.0).collect::<Vec<_>>()})).collect::<Vec<_>>())})
    }
}
/// Bounded script input avoids a blocking stdin read starving a granted lease.
/// No writer is granted before the operator's explicit `grant` command.
pub use batch::{run, run_measurement};
mod batch;

#[cfg(test)]
mod refresh_tests;

#[cfg(test)]
mod gp07_stage_tests;

#[cfg(test)]
mod remote_document_tests;

#[cfg(test)]
mod structural_pair_tests;

#[cfg(test)]
mod device_fence_tests;

#[cfg(test)]
mod brain_fifo_tests;

#[cfg(test)]
mod timing_trace_tests;

#[cfg(test)]
mod monitor_refresh_tests;

impl Operator {
    /// GP18 has no nonce: drain old documents before issuing both queries,
    /// require advancing independent sequences and matching global revision.
    /// Install the complete pair together, anchored to the original send time.
    pub(crate) fn refresh_sends(&mut self) -> Result<(), String> {
        self.check_guard()?;
        let deadline = Instant::now() + Duration::from_millis(250);
        let mut budget = 64usize;
        while let Some(document) = self
            .transport
            .receive_available_until(deadline)?
            .map(|b| crate::provider::StrictDocument::parse(&b))
            .transpose()?
        {
            if budget == 0 || Instant::now() >= deadline {
                return Err("sends backlog bound".into());
            }
            budget -= 1;
            if let Some(raw) = self.processing_document(document)? {
                let r = audio::decode_reply_document(raw)?;
                // Validate/drain unsolicited traffic without admitting its age.
                if r.context != self.session.snapshot_request().context
                    && self
                        .session
                        .pending
                        .as_ref()
                        .is_some_and(|p| p.request.context == r.context)
                {
                    self.session.accept(r, self.now())?;
                }
            }
        }
        let sent = self.now();
        self.transport
            .send_frame_until(&self.session.snapshot_request().encode()?, deadline)?;
        self.transport
            .send_frame_until(&self.session.sends_request().encode()?, deadline)?;
        let mut raw = None;
        let mut sends = None;
        while budget > 0 && Instant::now() < deadline {
            self.check_guard()?;
            let Some(document) = self.transport.receive_document_until(deadline)? else {
                continue;
            };
            budget -= 1;
            if document.value()["contract"] == crate::sends::CONTRACT {
                let r = crate::sends::decode_reply(&document.into_bytes()?)?;
                if r.context == self.session.sends_request().context {
                    let s = r
                        .snapshot
                        .ok_or_else(|| format!("sends unavailable: {:?}", r.reason))?;
                    if self.session.sends.as_ref().is_none_or(|old| {
                        crate::provider::counter(&s.sequence).unwrap()
                            > crate::provider::counter(&old.sequence).unwrap()
                            && crate::provider::counter(&s.frame).unwrap()
                                >= crate::provider::counter(&old.frame).unwrap()
                    }) {
                        sends = Some(s);
                    }
                } else {
                    self.session.dispatch_sends(r, self.now())?;
                }
            } else if let Some(document) = self.processing_document(document)? {
                let r = audio::decode_reply_document(document)?;
                if r.context == self.session.snapshot_request().context {
                    if r.state != "final" || r.outcome.as_ref().is_none_or(|o| o.kind != "applied")
                    {
                        return Err("raw sends query outcome".into());
                    }
                    let snapshot = r.snapshot.ok_or("raw sends query missing snapshot")?;
                    let mut candidate = self.session.clone();
                    let progress = self.session.snapshot.as_ref().is_none_or(|old| {
                        crate::provider::counter(&snapshot.authority.sequence).unwrap()
                            > crate::provider::counter(&old.authority.sequence).unwrap()
                            && crate::provider::counter(&snapshot.frame).unwrap()
                                >= crate::provider::counter(&old.frame).unwrap()
                    });
                    if progress && candidate.ingest_snapshot(snapshot.clone(), sent)? {
                        raw = Some(snapshot);
                    }
                } else if self
                    .session
                    .pending
                    .as_ref()
                    .is_some_and(|p| p.request.context == r.context)
                {
                    self.session.accept(r, self.now())?;
                }
            }
            if let (Some(r), Some(s)) = (&raw, &sends)
                && r.authority.revision == s.revision
            {
                let mut candidate = self.session.clone();
                if !candidate.ingest_snapshot(r.clone(), sent)?
                    || !candidate.ingest_sends(s.clone(), sent)?
                {
                    return Err("duplicate sends pair".into());
                }
                if Instant::now() >= deadline {
                    return Err("sends decode deadline".into());
                }
                self.check_guard()?;
                self.session = candidate;
                return Ok(());
            }
        }
        Err("sends paired read deadline/queue bound".into())
    }
}

impl Operator {
    pub(crate) fn refresh_live_eq(&mut self) -> Result<(), String> {
        self.check_guard()?;
        let deadline = Instant::now() + Duration::from_millis(250);
        let mut budget = 64usize;
        while let Some(document) = self
            .transport
            .receive_available_until(deadline)?
            .map(|b| crate::provider::StrictDocument::parse(&b))
            .transpose()?
        {
            if budget == 0 || Instant::now() >= deadline {
                return Err("live EQ backlog bound".into());
            }
            budget -= 1;
            if let Some(raw) = self.processing_document(document)? {
                let r = audio::decode_reply_document(raw)?;
                // Validate/drain unsolicited traffic without admitting its age.
                if r.context != self.session.snapshot_request().context
                    && self
                        .session
                        .pending
                        .as_ref()
                        .is_some_and(|p| p.request.context == r.context)
                {
                    self.session.accept(r, self.now())?;
                }
            }
        }
        let sent = self.now();
        self.transport
            .send_frame_until(&self.session.snapshot_request().encode()?, deadline)?;
        self.transport
            .send_frame_until(&self.session.live_eq_request().encode()?, deadline)?;
        let mut raw = None;
        let mut sends = None;
        while budget > 0 && Instant::now() < deadline {
            self.check_guard()?;
            let Some(document) = self.transport.receive_document_until(deadline)? else {
                continue;
            };
            budget -= 1;
            if document.value()["contract"] == crate::live_eq::CONTRACT {
                let r = crate::live_eq::decode_reply(&document.into_bytes()?)?;
                if r.context == self.session.live_eq_request().context {
                    let s = r
                        .master_eq
                        .ok_or_else(|| format!("live EQ unavailable: {:?}", r.reason))?;
                    if self.session.live_eq.as_ref().is_none_or(|old| {
                        crate::provider::counter(&s.frame).unwrap()
                            > crate::provider::counter(&old.frame).unwrap()
                            && crate::provider::counter(&s.frame).unwrap()
                                >= crate::provider::counter(&old.frame).unwrap()
                    }) {
                        sends = Some(s);
                    }
                } else {
                    self.session.dispatch_live_eq(r, self.now())?;
                }
            } else if let Some(document) = self.processing_document(document)? {
                let r = audio::decode_reply_document(document)?;
                if r.context == self.session.snapshot_request().context {
                    if r.state != "final" || r.outcome.as_ref().is_none_or(|o| o.kind != "applied")
                    {
                        return Err("raw sends query outcome".into());
                    }
                    let snapshot = r.snapshot.ok_or("raw sends query missing snapshot")?;
                    let mut candidate = self.session.clone();
                    let progress = self.session.snapshot.as_ref().is_none_or(|old| {
                        crate::provider::counter(&snapshot.authority.sequence).unwrap()
                            > crate::provider::counter(&old.authority.sequence).unwrap()
                            && crate::provider::counter(&snapshot.frame).unwrap()
                                >= crate::provider::counter(&old.frame).unwrap()
                    });
                    if progress && candidate.ingest_snapshot(snapshot.clone(), sent)? {
                        raw = Some(snapshot);
                    }
                } else if self
                    .session
                    .pending
                    .as_ref()
                    .is_some_and(|p| p.request.context == r.context)
                {
                    self.session.accept(r, self.now())?;
                }
            }
            if let (Some(r), Some(s)) = (&raw, &sends)
                && r.authority.revision == s.revision
            {
                let mut candidate = self.session.clone();
                if !candidate.ingest_snapshot(r.clone(), sent)?
                    || !candidate.ingest_live_eq(s.clone(), sent)?
                {
                    return Err("duplicate sends pair".into());
                }
                if Instant::now() >= deadline {
                    return Err("live EQ decode deadline".into());
                }
                self.check_guard()?;
                self.session = candidate;
                return Ok(());
            }
        }
        Err("live EQ paired read deadline/queue bound".into())
    }
}
#[cfg(test)]
mod gp18_pair_tests;

#[cfg(test)]
mod required_final_readback_tests;
