//! Real-provider frontend. No Simulator, device discovery or physical controller I/O.
use crate::{
    actions::{self, Action},
    audio::RenderedSnapshot,
    local_audio::Operator,
    model::{Mode, Page},
    render::{Primitive, Scene},
};
use serde_json::{Value, json};
use std::{
    collections::{BTreeSet, VecDeque},
    path::PathBuf,
    sync::{
        Arc, Mutex,
        atomic::{AtomicBool, AtomicU64, Ordering},
        mpsc::{self, Receiver, SyncSender, TrySendError},
    },
    thread::{self, JoinHandle},
    time::{Duration, Instant},
};

#[derive(Clone)]
pub struct Config {
    /// Explicit C-AUDIO/rendered version: 1 legacy, 2 dynamic.
    pub wire_version: u8,
    pub remote: Option<crate::remote::Config>,
    pub endpoint: PathBuf,
    pub show: String,
    pub epoch: u64,
    pub writer: String,
    pub scope: String,
}
#[derive(Clone, Debug)]
pub enum Operation {
    SwitchScope(String),
    EnableLiveEq,
    ReviewLiveEq(Value),
    EnableSends,
    ReviewTap(Value),
    EnableBrain,
    ReviewDevice(Box<crate::brain_device::Config>, (u64, u64)),
    BrainPress(u64),
    ReviewBrain {
        device: Option<Box<crate::audio::MonitorDevicePin>>,
        kind: String,
        body: Value,
    },
    ReviewStructure {
        kind: String,
        body: Value,
    },
    EnableProcessing,
    ReviewProcessing {
        input: String,
        config: crate::processing::Config,
    },
    Grant,
    ReleaseWriter,
    Set {
        target: Value,
        value: Value,
    },
    ReviewSet {
        target: Value,
        value: Value,
    },
    Preview(Value),
    Mode {
        mode: String,
        bounds: Value,
    },
    Confirm(u64),
    Cancel,
    InputReleased,
    Reconnect,
    LegacyReconnect,
}
#[derive(Clone, Debug)]
struct Request {
    generation: u64,
    revision: Option<String>,
    operation: Operation,
}
#[derive(Clone, Debug)]
pub struct Update {
    pub generation: u64,
    pub attachment_generation: u64,
    pub snapshot: Option<RenderedSnapshot>,
    pub device: Option<crate::brain_device::Snapshot>,
    pub device_final: Option<crate::brain_device::Reply>,
    pub device_fresh: bool,
    pub device_age_ms: Option<u64>,
    pub brain: Option<crate::brain::Snapshot>,
    pub brain_final: Option<crate::brain::Reply>,
    pub brain_fresh: bool,
    pub brain_age_ms: Option<u64>,
    pub held_status: Option<crate::held_proof::Status>,
    pub held_baseline_ready: bool,
    pub held_transport_authenticated: bool,
    pub brain_status: String,
    pub live_eq: Option<crate::live_eq::Snapshot>,
    pub live_eq_age_ms: Option<u64>,
    pub live_eq_final: Option<crate::live_eq::Reply>,
    pub sends: Option<crate::sends::Snapshot>,
    pub sends_age_ms: Option<u64>,
    pub sends_final: Option<crate::sends::Reply>,
    pub processing: Option<crate::processing::Snapshot>,
    pub processing_age_ms: Option<u64>,
    pub processing_status: String,
    pub structural: Option<crate::structure::Snapshot>,
    pub structural_final: Option<crate::structure::Reply>,
    pub structural_fresh: bool,
    pub structural_age_ms: Option<u64>,
    pub processing_final: Option<crate::processing::Reply>,
    pub fresh: bool,
    pub snapshot_age_ms: Option<u64>,
    pub status: String,
    /// Result of the last explicit operation; health polls cannot replace it.
    pub last_operation: Option<String>,
    /// Remaining lifetime of an actually confirmed scoped lease, never inferred from freshness.
    pub writer_lease_remaining_ms: Option<u64>,
    pub review: Option<(u64, String)>,
    pub received: Instant,
}
impl Update {
    fn observation_fresh(&self, valid: bool, age: Option<u64>) -> bool {
        valid
            && age.is_some_and(|age| {
                Duration::from_millis(age).saturating_add(self.received.elapsed())
                    <= Duration::from_millis(250)
            })
    }
    fn raw_fresh(&self) -> bool {
        self.observation_fresh(self.fresh, self.snapshot_age_ms)
    }
    fn brain_is_fresh(&self) -> bool {
        self.raw_fresh() && self.observation_fresh(self.brain_fresh, self.brain_age_ms)
    }
    fn device_is_fresh(&self) -> bool {
        self.raw_fresh() && self.observation_fresh(self.device_fresh, self.device_age_ms)
    }
    fn structure_is_fresh(&self) -> bool {
        self.raw_fresh() && self.observation_fresh(self.structural_fresh, self.structural_age_ms)
    }

    pub fn writer_granted(&self) -> bool {
        self.writer_lease_remaining_ms
            .is_some_and(|remaining| self.received.elapsed().as_millis() < u128::from(remaining))
    }
}
/// One coalesced provider-state slot, independent of input and desired LED state.
#[derive(Default)]
struct Latest {
    update: Mutex<Option<Update>>,
}
pub struct Provider {
    tx: SyncSender<Request>,
    latest: Arc<Latest>,
    generation: Arc<AtomicU64>,
    stop: Arc<AtomicBool>,
    child: Option<JoinHandle<()>>,
    authorization: Arc<AtomicBool>,
    brain_signal: Arc<crate::brain::HoldSignal>,
}
impl Provider {
    pub fn start(config: Config) -> Self {
        let (tx, rx) = mpsc::sync_channel(8);
        let latest = Arc::new(Latest::default());
        let generation = Arc::new(AtomicU64::new(1));
        let stop = Arc::new(AtomicBool::new(false));
        let (l, g, s) = (latest.clone(), generation.clone(), stop.clone());
        let authorization = Arc::new(AtomicBool::new(true));
        let a = authorization.clone();
        let brain_signal = Arc::new(crate::brain::HoldSignal::default());
        let b = brain_signal.clone();
        let child = thread::spawn(move || worker(config, rx, l, g, s, (a, b)));
        Self {
            tx,
            latest,
            generation,
            stop,
            child: Some(child),
            authorization,
            brain_signal,
        }
    }
    fn send(&self, revision: Option<String>, operation: Operation) -> Result<(), String> {
        self.tx
            .try_send(Request {
                generation: self.generation.load(Ordering::Acquire),
                revision,
                operation,
            })
            .map_err(|e| match e {
                TrySendError::Full(_) => "provider request queue overflow; intent refused".into(),
                TrySendError::Disconnected(_) => "provider worker unavailable".into(),
            })
    }
    pub fn take(&self) -> Option<Update> {
        self.latest.update.lock().unwrap().take()
    }
    pub fn fence(&self) -> u64 {
        self.brain_signal.release();
        self.generation
            .fetch_update(Ordering::AcqRel, Ordering::Acquire, |g| g.checked_add(1))
            .expect("generation exhausted")
            + 1
    }
    pub fn generation(&self) -> u64 {
        self.generation.load(Ordering::Acquire)
    }
}
impl Drop for Provider {
    fn drop(&mut self) {
        self.brain_signal.release();
        self.stop.store(true, Ordering::Release);
        if let Some(c) = self.child.take() {
            let _ = c.join();
        }
    }
}
// Per-thread test seam at the actual worker boundaries; absent from production.
#[cfg(test)]
type HeldWorkerHook = Box<dyn FnMut(&str, Option<&Update>) -> bool>;
#[cfg(test)]
thread_local! {
    static HELD_WORKER_SEED: std::cell::RefCell<Option<Operator>> = const { std::cell::RefCell::new(None) };
    static HELD_WORKER_HOOK: std::cell::RefCell<Option<HeldWorkerHook>> = const { std::cell::RefCell::new(None) };
}
#[cfg(test)]
fn held_worker_event(event: &str, latest: &Latest) -> bool {
    HELD_WORKER_HOOK.with_borrow_mut(|hook| {
        hook.as_mut()
            .is_some_and(|h| h(event, latest.update.lock().unwrap().as_ref()))
    })
}
#[cfg(test)]
pub(crate) fn exercise_held_worker(
    operator: Operator,
    signal: Arc<crate::brain::HoldSignal>,
    action: &'static str,
) -> (Vec<String>, Update) {
    let snapshot = operator.session.snapshot.as_ref().unwrap();
    let config = Config {
        wire_version: 2,
        remote: None,
        endpoint: PathBuf::new(),
        show: snapshot.authority.show_id.clone(),
        epoch: snapshot.authority.epoch.parse().unwrap(),
        writer: "worker-test".into(),
        scope: "talkback_destinations".into(),
    };
    let (tx, rx) = mpsc::sync_channel(8);
    let latest = Arc::new(Latest::default());
    let generation = Arc::new(AtomicU64::new(1));
    let authorization = Arc::new(AtomicBool::new(true));
    let stop = Arc::new(AtomicBool::new(false));
    let events = Arc::new(Mutex::new(Vec::new()));
    let captured = Arc::new(Mutex::new(None));
    let (e, c, g, a, b, st) = (
        events.clone(),
        captured.clone(),
        generation.clone(),
        authorization.clone(),
        signal.clone(),
        stop.clone(),
    );
    let mut injected = false;
    HELD_WORKER_SEED.with_borrow_mut(|seed| *seed = Some(operator));
    HELD_WORKER_HOOK.with_borrow_mut(|hook| {
        *hook = Some(Box::new(move |event, update| {
            e.lock().unwrap().push(event.to_string());
            if event == "service" && !injected {
                injected = true;
                match action {
                    "release" => b.release(),
                    "generation" => {
                        g.fetch_add(1, Ordering::AcqRel);
                    }
                    "authorization" => a.store(false, Ordering::Release),
                    "queued" => tx
                        .try_send(Request {
                            generation: 0,
                            revision: None,
                            operation: Operation::InputReleased,
                        })
                        .unwrap(),
                    _ => {}
                }
            }
            if event == "published" {
                *c.lock().unwrap() = update.cloned();
            }
            if event == "input" {
                st.store(true, Ordering::Release);
            }
            event == "wait" && action != "queued"
        }))
    });
    worker(
        config,
        rx,
        latest,
        generation,
        stop,
        (authorization, signal),
    );
    HELD_WORKER_HOOK.with_borrow_mut(|hook| *hook = None);
    let events = events.lock().unwrap().clone();
    let update = captured
        .lock()
        .unwrap()
        .take()
        .expect("worker reached idle wait without publishing");
    (events, update)
}

fn worker(
    mut config: Config,
    rx: Receiver<Request>,
    latest: Arc<Latest>,
    generation: Arc<AtomicU64>,
    stop: Arc<AtomicBool>,
    signals: (Arc<AtomicBool>, Arc<crate::brain::HoldSignal>),
) {
    let (authorization, brain_signal) = signals;
    let mut op: Option<Operator> = None;
    let mut structural_final: Option<crate::structure::Reply> = None;
    let mut g = generation.load(Ordering::Acquire);
    let mut serial = 0u64;
    let mut review: Option<(u64, String)> = None;
    let mut status = "provider unavailable; read-only attach".to_string();
    let mut operation_error: Option<String> = None;
    let mut last_operation: Option<String> = None;
    let mut brain_enabled = false;
    let mut brain_status = "unavailable; explicit --brain-audio required".to_string();
    let mut held: Option<(u64, u64)> = None;
    let mut heartbeat = Instant::now();
    let mut brain_poll = Instant::now();
    let mut device_poll = Instant::now();
    let mut processing_requested = false;
    let mut processing_enabled = false;
    let mut processing_status = "unavailable; GP07 probe not enabled".to_string();
    let mut processing_poll = Instant::now();
    let mut sends_enabled = false;
    let mut sends_poll = Instant::now();
    let mut live_enabled = false;
    let mut live_poll = Instant::now();
    let mut connect = true;
    let mut reconnects = 0u64;
    let mut attachment_generation = g;
    #[cfg(test)]
    HELD_WORKER_SEED.with_borrow_mut(|seed| {
        if let Some(operator) = seed.take() {
            op = Some(operator);
            connect = false;
            held = Some((1, 1));
            heartbeat = Instant::now() - Duration::from_millis(20);
            brain_enabled = true;
        }
    });
    while !stop.load(Ordering::Acquire) {
        if held.is_some_and(|(id, _)| !brain_signal.live_id(id))
            || (held.is_some() && !authorization.load(Ordering::Acquire))
        {
            brain_signal.release();
            if let Some(o) = &mut op {
                let _ = o.close_brain();
            }
            held = None;
        }
        let current = generation.load(Ordering::Acquire);
        if current != g {
            g = current;
            review = None;
            if held.take().is_some() {
                brain_signal.release();
                if let Some(o) = &mut op {
                    let _ = o.close_brain();
                }
            }
            if let Some(o) = &mut op {
                o.cancel();
                if !authorization.load(Ordering::Acquire) {
                    o.session.disconnect();
                }
            }
        }
        if connect {
            connect = false;
            let writer = if reconnects == 0 {
                config.writer.clone()
            } else {
                format!("{}-r{}", config.writer, reconnects)
            };
            let connection = if let Some(remote) = &config.remote {
                Operator::connect_remote(remote, &config.show, config.epoch, &config.scope)
            } else {
                Operator::connect_version(
                    &config.endpoint,
                    &config.show,
                    config.epoch,
                    &writer,
                    &config.scope,
                    config.wire_version,
                )
            };
            match connection {
                Ok(mut o) => {
                    o.brain_signal(brain_signal.clone());
                    match o.refresh() {
                        Ok(()) => {
                            status =
                                "provider attached read-only; explicit G grant required".into();
                            op = Some(o)
                        }
                        Err(e) => status = format!("UNAVAILABLE: {e}"),
                    }
                }
                Err(e) => status = format!("UNAVAILABLE: {e}"),
            }
        }
        if let Some(o) = &mut op {
            o.guard(generation.clone(), g);
            service_worker_hold(
                o,
                &brain_signal,
                &mut held,
                &mut heartbeat,
                &mut brain_status,
            );
        }
        #[cfg(test)]
        held_worker_event("service", &latest);
        // Service completion (including closure/error) is visible before any idle
        // receive or passive work. Recheck asynchronous fences before publishing.
        if generation.load(Ordering::Acquire) != g {
            continue;
        }
        if held.is_some_and(|(id, _)| !brain_signal.live_id(id))
            || (held.is_some() && !authorization.load(Ordering::Acquire))
        {
            brain_signal.release();
            if let Some(o) = &mut op {
                let _ = o.close_brain();
            }
            held = None;
        }
        let received = Instant::now();
        let update = Update {
            generation: g,
            attachment_generation,
            last_operation: last_operation.clone(),
            writer_lease_remaining_ms: op.as_ref().and_then(confirmed_lease_remaining),
            snapshot: op.as_ref().and_then(|o| o.session.snapshot.clone()),
            device: op.as_ref().and_then(|o| o.session.device.clone()),
            device_final: op.as_ref().and_then(|o| o.session.device_final.clone()),
            device_fresh: op.as_ref().is_some_and(|o| o.session.device_fresh(o.now())),
            device_age_ms: op.as_ref().and_then(|o| o.session.device_age(o.now())),
            brain: op.as_ref().and_then(|o| o.session.brain.clone()),
            brain_final: op.as_ref().and_then(|o| o.session.brain_final.clone()),
            brain_fresh: op.as_ref().is_some_and(|o| o.session.brain_fresh(o.now())),
            brain_age_ms: op.as_ref().and_then(|o| o.session.brain_age(o.now())),
            held_status: op.as_ref().and_then(|o| o.held_status()),
            held_baseline_ready: op.as_ref().is_some_and(|o| o.held_baseline_ready()),
            held_transport_authenticated: op
                .as_ref()
                .is_some_and(|o| o.held_transport_authenticated()),
            brain_status: brain_status.clone(),
            live_eq: op.as_ref().and_then(|o| o.session.live_eq.clone()),
            live_eq_age_ms: op.as_ref().and_then(|o| o.session.live_eq_age(o.now())),
            live_eq_final: op.as_ref().and_then(|o| o.session.live_eq_final.clone()),
            sends: op.as_ref().and_then(|o| o.session.sends.clone()),
            sends_age_ms: op.as_ref().and_then(|o| o.session.sends_age(o.now())),
            sends_final: op.as_ref().and_then(|o| o.session.sends_final.clone()),
            processing: op.as_ref().and_then(|o| o.session.processing.clone()),
            processing_age_ms: op.as_ref().and_then(|o| o.session.processing_age(o.now())),
            processing_status: processing_status.clone(),
            structural: op
                .as_ref()
                .and_then(|o| o.session.structural.clone())
                .or_else(|| structural_final.as_ref().and_then(|r| r.snapshot.clone())),
            structural_final: structural_final.clone(),
            structural_age_ms: op.as_ref().and_then(|o| o.session.structural_age(o.now())),
            structural_fresh: op
                .as_ref()
                .is_some_and(|o| o.session.structural_fresh(o.now())),
            processing_final: op.as_ref().and_then(|o| o.session.processing_final.clone()),
            fresh: op.as_ref().is_some_and(|o| o.session.fresh(o.now())),
            snapshot_age_ms: op.as_ref().and_then(|o| o.session.snapshot_age(o.now())),
            status: status.clone(),
            review: review.clone(),
            received,
        };
        publish_provider_update(&latest, update, operation_error.as_deref());
        #[cfg(test)]
        held_worker_event("published", &latest);
        let receive_wait = if held.is_some() {
            (heartbeat + Duration::from_millis(20))
                .saturating_duration_since(Instant::now())
                .min(Duration::from_millis(5))
        } else {
            Duration::from_millis(40)
        };
        #[cfg(test)]
        if held_worker_event("wait", &latest) {
            break;
        }
        if let Ok(r) = rx.recv_timeout(receive_wait) {
            #[cfg(test)]
            held_worker_event("input", &latest);
            // A recovery request can wake recv after its generation was revoked.
            // Synchronize before comparing so a fresh reconnect is never discarded.
            let current = generation.load(Ordering::Acquire);
            if current != g {
                g = current;
                review = None;
                if held.take().is_some() {
                    brain_signal.release();
                    if let Some(o) = &mut op {
                        let _ = o.close_brain();
                    }
                }
                if let Some(o) = &mut op {
                    o.cancel();
                    if !authorization.load(Ordering::Acquire) {
                        o.session.disconnect();
                    }
                }
            }
            if r.generation != g || generation.load(Ordering::Acquire) != g {
                continue;
            }
            if !authorization.load(Ordering::Acquire)
                && !matches!(
                    r.operation,
                    Operation::InputReleased
                        | Operation::Cancel
                        | Operation::Reconnect
                        | Operation::LegacyReconnect
                        | Operation::EnableLiveEq
                        | Operation::EnableSends
                        | Operation::SwitchScope(_)
                        | Operation::EnableProcessing
                        | Operation::EnableBrain
                )
            {
                status = "ROLE UNAVAILABLE: new provider writes refused".into();
                continue;
            }
            if held.is_some() && matches!(r.operation, Operation::BrainPress(_)) {
                // Reject before the common revision refresh can block this hold.
                status = "PTT repeated press refused; existing gesture unchanged".into();
                continue;
            }
            // A held lane must never enter an unbounded ordinary action. End the
            // gesture before configuration/review/renew/reconnect work begins.
            if held.is_some()
                && !matches!(
                    r.operation,
                    Operation::InputReleased | Operation::BrainPress(_)
                )
            {
                brain_signal.release();
                if let Some(o) = &mut op
                    && let Err(e) = o.close_brain()
                {
                    status = format!("held close failed: {e}");
                    held = None;
                    continue;
                }
                held = None;
            }
            if !matches!(
                r.operation,
                Operation::InputReleased | Operation::BrainPress(_)
            ) {
                operation_error = None;
                last_operation = None;
            }
            if matches!(r.operation, Operation::EnableBrain) {
                brain_enabled = true;
                brain_status = "awaiting actual Brain readback".into();
            } else if matches!(r.operation, Operation::EnableLiveEq) {
                live_enabled = config.wire_version == 2;
            } else if matches!(r.operation, Operation::EnableSends) {
                sends_enabled = config.wire_version == 2;
            } else if matches!(r.operation, Operation::EnableProcessing) {
                processing_requested = true;
                processing_enabled = true;
                processing_status = "awaiting capability snapshot".into();
            } else if matches!(
                r.operation,
                Operation::Reconnect | Operation::LegacyReconnect | Operation::SwitchScope(_)
            ) {
                if matches!(r.operation, Operation::LegacyReconnect) {
                    processing_requested = false;
                    processing_status = "disabled by explicit legacy GP03 reconnect".into();
                }
                brain_signal.release();
                held = None;
                if let Some(o) = &mut op {
                    let _ = o.close_brain();
                }
                let mut release_error = None;
                if let Operation::SwitchScope(ref scope) = r.operation {
                    if let Some(o) = &mut op {
                        o.cancel();
                        if o.session.pending.is_none()
                            && confirmed_lease_remaining(o).is_some()
                            && let Err(error) = o.mutate_inner("release", json!({}))
                        {
                            release_error = Some(error);
                        }
                        o.session.disconnect();
                    }
                    config.scope = scope.clone();
                }
                op = None;
                review = None;
                reconnects += 1;
                attachment_generation = g;
                processing_enabled = processing_requested;
                connect = true;
                status=release_error.map_or_else(||"reconnect discards intents; fresh writer/read-only".into(),|e|format!("old lease release failed: {e}; attachment discarded; fresh read-only writer"));
            } else if let Some(o) = &mut op {
                let affects_status = !matches!(r.operation, Operation::InputReleased);
                o.guard(generation.clone(), g);
                if affects_status {
                    last_operation = Some("PENDING; awaiting provider confirmation".into());
                    let received = Instant::now();
                    *latest.update.lock().unwrap() = Some(Update {
                        generation: g,
                        attachment_generation,
                        last_operation: last_operation.clone(),
                        writer_lease_remaining_ms: confirmed_lease_remaining(o),
                        snapshot: o.session.snapshot.clone(),
                        device: o.session.device.clone(),
                        device_final: o.session.device_final.clone(),
                        device_fresh: o.session.device_fresh(o.now()),
                        device_age_ms: o.session.device_age(o.now()),
                        brain: o.session.brain.clone(),
                        brain_final: o.session.brain_final.clone(),
                        brain_fresh: o.session.brain_fresh(o.now()),
                        brain_age_ms: o.session.brain_age(o.now()),
                        held_status: o.held_status(),
                        held_baseline_ready: o.held_baseline_ready(),
                        held_transport_authenticated: o.held_transport_authenticated(),
                        brain_status: brain_status.clone(),
                        live_eq: o.session.live_eq.clone(),
                        live_eq_age_ms: o.session.live_eq_age(o.now()),
                        live_eq_final: o.session.live_eq_final.clone(),
                        sends: o.session.sends.clone(),
                        sends_age_ms: o.session.sends_age(o.now()),
                        sends_final: o.session.sends_final.clone(),
                        processing: o.session.processing.clone(),
                        processing_age_ms: o.session.processing_age(o.now()),
                        processing_status: processing_status.clone(),
                        structural: o.session.structural.clone(),
                        structural_final: o.session.structural_final.clone(),
                        structural_fresh: o.session.structural_fresh(o.now()),
                        structural_age_ms: o.session.structural_age(o.now()),
                        processing_final: o.session.processing_final.clone(),
                        fresh: o.session.fresh(o.now()),
                        snapshot_age_ms: o.session.snapshot_age(o.now()),
                        status: format!(
                            "PENDING {}; awaiting provider confirmation",
                            match &r.operation {
                                Operation::EnableLiveEq => "live EQ probe",
                                Operation::ReviewLiveEq(_) => "live EQ review",
                                Operation::EnableSends => "sends capability query",
                                Operation::ReviewTap(_) => "separate tap review",
                                Operation::SwitchScope(_) => "scope reattachment",
                                Operation::EnableProcessing => "capability query",
                                Operation::ReviewDevice(..) => "device configuration review",
                                Operation::EnableBrain => "Brain capability probe",
                                Operation::BrainPress(_) => "talkback press",
                                Operation::ReviewBrain { .. } => "Brain configuration review",
                                Operation::ReviewProcessing { .. } => "processing review",
                                Operation::ReviewStructure { .. } => "structural review",
                                Operation::Grant => "writer grant",
                                Operation::ReleaseWriter => "writer release",
                                Operation::Set { .. } => "parameter edit",
                                Operation::ReviewSet { .. } => "protected edit review",
                                Operation::Preview(_) => "engine release preview",
                                Operation::Mode { .. } => "mode review",
                                Operation::Confirm(_) => "reviewed operation",
                                Operation::Cancel => "review cancellation",
                                Operation::InputReleased => "input release",
                                Operation::Reconnect | Operation::LegacyReconnect => "reconnect",
                            }
                        ),
                        review: review.clone(),
                        received,
                    });
                }
                let local_result = match &r.operation {
                    Operation::ReviewDevice(..)
                    | Operation::ReviewBrain { .. }
                    | Operation::ReviewStructure { .. }
                    | Operation::ReviewLiveEq(_)
                    | Operation::ReviewTap(_)
                    | Operation::ReviewProcessing { .. }
                    | Operation::ReviewSet { .. }
                    | Operation::Preview(_)
                    | Operation::Mode { .. } => {
                        Some("REVIEW READY; explicit confirmation required")
                    }
                    Operation::Cancel => Some("CANCELLED; unsent review discarded"),
                    _ => None,
                };
                let result = (|| {
                    if !matches!(
                        r.operation,
                        Operation::InputReleased | Operation::BrainPress(_)
                    ) && let Some(revision) = &r.revision
                    {
                        o.refresh()?;
                        if o.session
                            .snapshot
                            .as_ref()
                            .is_none_or(|s| &s.authority.revision != revision)
                        {
                            return Err("observed revision changed; intent discarded".into());
                        }
                    }
                    if generation.load(Ordering::Acquire) != g {
                        return Err("input context changed; intent discarded".into());
                    }
                    match r.operation {
                        Operation::EnableLiveEq
                        | Operation::EnableSends
                        | Operation::EnableProcessing
                        | Operation::EnableBrain => unreachable!(),
                        Operation::ReviewDevice(config, identity) => {
                            if !brain_enabled {
                                return Err("Brain opt-in required".into());
                            }
                            brain_signal.release();
                            held = None;
                            o.stage(
                                "device_configure",
                                json!({"config":config,"device_identity":identity}),
                            )?;
                            serial = serial.checked_add(1).ok_or("review counter exhausted")?;
                            review = Some((serial, o.reviewed().unwrap_or_default()));
                            Ok(())
                        }
                        Operation::BrainPress(intent) => {
                            if !brain_enabled || !brain_signal.live_id(intent) || held.is_some() {
                                return Err("PTT edge expired/repeated".into());
                            }
                            let next = o.start_held()?;
                            held = Some((intent, next));
                            heartbeat = o.held_send_anchor().ok_or("hold send anchor missing")?;
                            Ok(())
                        }
                        Operation::ReviewBrain { kind, body, device } => {
                            if !brain_enabled {
                                return Err("Brain explicit opt-in required".into());
                            }
                            brain_signal.release();
                            held = None;
                            if crate::audio::monitor_armed(&kind, &body) {
                                o.session.validate_monitor_device(
                                    device
                                        .as_deref()
                                        .ok_or("reviewed monitor device required")?,
                                )?;
                            }
                            o.stage(&kind, body)?;
                            serial = serial.checked_add(1).ok_or("review counter exhausted")?;
                            review = Some((serial, o.reviewed().unwrap_or_default()));
                            Ok(())
                        }
                        Operation::ReviewLiveEq(body) => {
                            o.stage("master_eq_set", body)?;
                            serial = serial.checked_add(1).ok_or("review exhausted")?;
                            review = Some((serial, o.reviewed().unwrap_or_default()));
                            Ok(())
                        }
                        Operation::ReviewTap(body) => {
                            o.stage("send_tap_set", body)?;
                            serial = serial.checked_add(1).ok_or("review counter exhausted")?;
                            review = Some((serial, o.reviewed().unwrap_or_default()));
                            Ok(())
                        }
                        Operation::ReviewProcessing { input, config } => {
                            o.stage("processing_set", json!({"input": input, "config": config}))?;
                            serial = serial.checked_add(1).ok_or("review counter exhausted")?;
                            review = Some((serial, o.reviewed().unwrap_or_default()));
                            Ok(())
                        }
                        Operation::ReviewStructure { kind, body } => {
                            o.stage(&kind, body)?;
                            serial = serial.checked_add(1).ok_or("review counter exhausted")?;
                            review = Some((serial, o.reviewed().unwrap_or_default()));
                            Ok(())
                        }
                        Operation::Grant => crate::scopes::value(&config.scope)
                            .and_then(|scope| o.mutate_inner("grant", json!({"scope":scope}))),
                        Operation::ReleaseWriter => o.mutate_inner("release", json!({})),
                        Operation::Set { target, value } => o.mutate_inner(
                            "set",
                            json!({"targets":[{"target":target,"value":value}]}),
                        ),
                        Operation::ReviewSet { target, value } => {
                            o.stage("set", json!({"targets":[{"target":target,"value":value}]}))?;
                            serial = serial.checked_add(1).ok_or("review counter exhausted")?;
                            review = Some((serial, o.reviewed().unwrap_or_default()));
                            Ok(())
                        }
                        Operation::Preview(target) => {
                            o.mutate_inner("preview_release", json!({"targets":[target]}))?;
                            let p = o
                                .session
                                .preview(o.now())
                                .ok_or("engine preview unavailable")?
                                .clone();
                            o.stage("release_preview", json!({"token":p.token}))?;
                            serial = serial.checked_add(1).ok_or("review counter exhausted")?;
                            review = Some((
                                serial,
                                format!(
                                    "ENGINE ramp {} frames; destinations {}; release reviewed holds / revision {} / scope {} / show {} / epoch {}",
                                    p.ramp_frames,
                                    p.destinations
                                        .iter()
                                        .map(|d| format!(
                                            "{} {} {} -> {}",
                                            d.target.input,
                                            d.target.monitor.as_deref().unwrap_or("FOH"),
                                            d.target.parameter,
                                            parameter_display(&d.target.parameter, &d.value)
                                        ))
                                        .collect::<Vec<_>>()
                                        .join("; "),
                                    p.revision,
                                    p.scope,
                                    config.show,
                                    config.epoch
                                ),
                            ));
                            Ok(())
                        }
                        Operation::Mode { mode, bounds } => {
                            o.stage("set_mode", json!({"mode":mode,"bounds":bounds}))?;
                            serial = serial.checked_add(1).ok_or("review counter exhausted")?;
                            review = Some((serial, o.reviewed().unwrap_or_default()));
                            Ok(())
                        }
                        Operation::Confirm(id) => {
                            if review.as_ref().is_none_or(|(n, _)| *n != id) {
                                return Err("confirmation was not reviewed".into());
                            }
                            review = None;
                            o.confirm()
                        }
                        Operation::Cancel => {
                            review = None;
                            o.cancel();
                            Ok(())
                        }
                        Operation::InputReleased => {
                            o.session.input_released();
                            Ok(())
                        }
                        Operation::SwitchScope(_)
                        | Operation::Reconnect
                        | Operation::LegacyReconnect => unreachable!(),
                    }
                })();
                match result {
                    Ok(()) if affects_status => {
                        status = local_result
                            .map_or_else(|| o.session.last_result.clone(), str::to_string);
                        last_operation = Some(status.clone());
                    }
                    Ok(()) => {}
                    Err(e) => {
                        status = if e.starts_with("correlated completion:") {
                            format!("COMPLETED/READBACK UNAVAILABLE: {e}")
                        } else {
                            format!("REFUSED/UNCERTAIN: {e}")
                        };
                        operation_error = Some(status.clone());
                        last_operation = Some(status.clone());
                        review = None;
                        o.cancel();
                    }
                }
            } else if !matches!(r.operation, Operation::InputReleased | Operation::Cancel) {
                status = "UNAVAILABLE: operation refused; F5 explicit reconnect".into();
            }
        }
        if let Some(o) = &mut op {
            o.guard(generation.clone(), g);
            // Publish a valid new review before unrelated renewal/telemetry work.
            // Its displayed generation/revision/deadline remain checked at confirmation.
            if ((review.is_some() && o.review_valid())
                || (status.starts_with("REFUSED/UNCERTAIN:")
                    || status.starts_with("COMPLETED/READBACK UNAVAILABLE:")))
                && generation.load(Ordering::Acquire) == g
            {
                let received = Instant::now();
                *latest.update.lock().unwrap() = Some(Update {
                    generation: g,
                    attachment_generation,
                    last_operation: last_operation.clone(),
                    writer_lease_remaining_ms: confirmed_lease_remaining(o),
                    snapshot: o.session.snapshot.clone(),
                    device: o.session.device.clone(),
                    device_final: o.session.device_final.clone(),
                    device_fresh: o.session.device_fresh(o.now()),
                    device_age_ms: o.session.device_age(o.now()),
                    brain: o.session.brain.clone(),
                    brain_final: o.session.brain_final.clone(),
                    brain_fresh: o.session.brain_fresh(o.now()),
                    brain_age_ms: o.session.brain_age(o.now()),
                    held_status: o.held_status(),
                    held_baseline_ready: o.held_baseline_ready(),
                    held_transport_authenticated: o.held_transport_authenticated(),
                    brain_status: brain_status.clone(),
                    live_eq: o.session.live_eq.clone(),
                    live_eq_age_ms: o.session.live_eq_age(o.now()),
                    live_eq_final: o.session.live_eq_final.clone(),
                    sends: o.session.sends.clone(),
                    sends_age_ms: o.session.sends_age(o.now()),
                    sends_final: o.session.sends_final.clone(),
                    processing: o.session.processing.clone(),
                    processing_age_ms: o.session.processing_age(o.now()),
                    processing_status: processing_status.clone(),
                    structural: o.session.structural.clone(),
                    structural_final: o.session.structural_final.clone(),
                    structural_fresh: o.session.structural_fresh(o.now()),
                    structural_age_ms: o.session.structural_age(o.now()),
                    processing_final: o.session.processing_final.clone(),
                    fresh: o.session.fresh(o.now()),
                    snapshot_age_ms: o.session.snapshot_age(o.now()),
                    status: status.clone(),
                    review: review.clone(),
                    received,
                });
            }
            if held.is_none() {
                if authorization.load(Ordering::Acquire)
                    && o.session.renewal_due(o.now())
                    && let Err(e) = o.mutate("renew", json!({}))
                {
                    status = format!("LEASE UNCERTAIN: {e}");
                }
                if brain_enabled {
                    if device_poll.elapsed() >= Duration::from_millis(180) {
                        if let Err(e) = o.refresh_device() {
                            brain_status = format!("device unavailable: {e}");
                        }
                        device_poll = Instant::now();
                    }
                    if brain_poll.elapsed() >= Duration::from_millis(40) {
                        match o.refresh_brain() {
                            Ok(()) => brain_status = "actual readback received".into(),
                            Err(e) => {
                                brain_status = format!("STALE: {e}");
                                brain_signal.release();
                                held = None;
                                let _ = o.close_brain();
                            }
                        }
                        brain_poll = Instant::now();
                    }
                }
                if config.wire_version == 2
                    && matches!(config.scope.as_str(), "pa_configuration" | "output_routes")
                    && let Err(error) = o.refresh_structural()
                {
                    status = format!("Structural state unavailable: {error}");
                }
                if live_enabled && live_poll.elapsed() >= Duration::from_millis(80) {
                    if let Err(error) = o.refresh_live_eq() {
                        status = format!("Live EQ unavailable: {error}");
                    }
                    live_poll = Instant::now();
                }
                if sends_enabled && sends_poll.elapsed() >= Duration::from_millis(80) {
                    if let Err(error) = o.refresh_sends() {
                        status = format!("Sends unavailable: {error}");
                    }
                    sends_poll = Instant::now();
                }
                if processing_enabled && processing_poll.elapsed() >= Duration::from_millis(80) {
                    match o.refresh_processing() {
                        Ok(()) => processing_status = "capability confirmed".into(),
                        Err(e) => {
                            // A bounded read-only poll may overlap a context fence or
                            // lose its observation to a deadline. Keep polling without
                            // replaying mutations; explicit unsupported replies disable it.
                            processing_status = format!("STALE/UNAVAILABLE: {e}");
                            if e.starts_with("processing unavailable:") {
                                processing_enabled = false;
                            }
                        }
                    }
                    processing_poll = Instant::now();
                }
                if o.session.structural_final.is_some() {
                    structural_final = o.session.structural_final.clone();
                }
                if generation.load(Ordering::Acquire) != g {
                    // The next loop cancels the old context and installs its new guard.
                    // A healthy read-only connection/lease survives ordinary focus loss.
                    continue;
                }
                let refreshed = refresh_worker_context(o, &generation, g);
                if matches!(refreshed, Ok(false)) {
                    continue;
                }
                if let Err(e) = refreshed {
                    status = format!("STALE/UNCERTAIN: {e}; F5 reconnect, no replay");
                    review = None;
                    o.cancel();
                    o.session.disconnect();
                    op = None;
                } else if review.is_some() && !o.review_valid() {
                    review = None;
                    o.cancel();
                    status = "review invalidated by provider context/revision; review again".into();
                }
            }
        }
        if generation.load(Ordering::Acquire) != g {
            continue;
        }
    }
    brain_signal.release();
    if let Some(o) = &mut op {
        let _ = o.close_brain();
    }
    // Persistent mixer holds remain engine-owned; only ephemeral talkback closes.
}

/// Runs before dequeuing any event, including stale/repeated events. The same
/// service is exercised by the delayed-transport worker regressions.
pub(crate) fn service_worker_hold(
    operator: &mut Operator,
    signal: &crate::brain::HoldSignal,
    held: &mut Option<(u64, u64)>,
    heartbeat: &mut Instant,
    status: &mut String,
) {
    if let Some((intent, generation)) = *held {
        if !signal.live_id(intent) {
            signal.release();
            let _ = operator.close_brain();
            *held = None;
        } else if heartbeat.elapsed() >= Duration::from_millis(20) {
            match operator.service_held(generation, intent) {
                Ok(sent) => {
                    *heartbeat = sent;
                    *status = "held service readback received".into();
                }
                Err(error) => {
                    *status = format!("held service stopped: {error}");
                    signal.release();
                    *held = None;
                    let _ = operator.close_brain();
                }
            }
        }
    }
}

/// False means a pure guard cancellation: the worker retains the connection and
/// performs its ordinary generation transition before any next admission.
pub(crate) fn refresh_worker_context(
    operator: &mut crate::local_audio::Operator,
    generation: &AtomicU64,
    expected: u64,
) -> Result<bool, String> {
    match operator.refresh_classified() {
        Ok(()) => Ok(true),
        Err(crate::local_audio::BrainOperationError::Admission(_))
            if generation.load(Ordering::Acquire) != expected =>
        {
            Ok(false)
        }
        Err(error) => Err(error.message()),
    }
}

fn confirmed_lease_remaining(operator: &Operator) -> Option<u64> {
    operator
        .session
        .lease_deadline()?
        .checked_sub(operator.now())
        .filter(|remaining| *remaining > 0)
}

fn publish_provider_update(latest: &Latest, mut update: Update, operation_error: Option<&str>) {
    if let Some(error) = operation_error {
        update.status = error.into();
    }
    *latest.update.lock().unwrap() = Some(update);
}

#[derive(Clone, Debug)]
pub enum Event {
    /// Injected controller semantic action, stamped with the current input generation.
    Controller {
        action: Action,
        generation: u64,
    },
    Key {
        key: String,
        pressed: bool,
    },
    Focus(bool),
    Resize(u32, u32),
    DeviceLost,
    DeviceRestored,
}
fn parameter_display(parameter: &str, value: &Value) -> String {
    match parameter {
        "fader" | "send" => value.as_i64().map_or("unavailable".into(), |n| {
            format!("{:+.1} dB", n as f64 / 1000.0)
        }),
        "pan" => value.as_i64().map_or("unavailable".into(), |n| {
            if n == 0 {
                "center".into()
            } else {
                format!("{} {}%", if n < 0 { "left" } else { "right" }, n.abs())
            }
        }),
        "mute" => value.as_bool().map_or("unavailable".into(), |v| {
            if v { "on" } else { "off" }.into()
        }),
        _ => "unavailable".into(),
    }
}
fn optional_display(parameter: &str, value: &Option<Value>) -> String {
    value
        .as_ref()
        .map_or("none".into(), |v| parameter_display(parameter, v))
}
fn linear_display(n: crate::audio::Nanogain) -> String {
    format!("{}.{:09} x", n.0 / 1_000_000_000, n.0 % 1_000_000_000)
}
fn send_gain_display(n: crate::audio::Nanogain) -> String {
    if n.0 == 0 {
        "-inf dB".into()
    } else {
        format!("{:+.1} dB", 20.0 * (n.0 as f64 / 1_000_000_000.0).log10())
    }
}
fn tap_display(tap: crate::sends::Tap) -> &'static str {
    match tap {
        crate::sends::Tap::RawPostMute => "Raw / Post-mute",
        crate::sends::Tap::ProcessedPreFader => "Processed / Pre-fader",
        crate::sends::Tap::ProcessedPostFader => "Processed / Post-fader",
    }
}
fn observation_age_display(age: Option<u64>, received: Instant) -> String {
    age.map_or_else(
        || "unavailable".into(),
        |age| {
            format!(
                "{} ms",
                age.saturating_add(received.elapsed().as_millis() as u64)
            )
        },
    )
}
#[derive(Clone, Debug)]
pub struct ProcessingDraft {
    pub input: String,
    pub config: crate::processing::Config,
    revision: String,
    generation: u64,
}
#[derive(Clone, Debug)]
pub enum SendDraftValue {
    Tap(crate::sends::Tap),
    Level(i32),
}
#[derive(Clone, Debug)]
pub struct SendDraft {
    pub input: String,
    pub monitor: String,
    pub value: SendDraftValue,
    revision: String,
    generation: u64,
}
pub struct Frontend {
    pub provider: Provider,
    pub state: Option<Update>,
    module_config: Config,
    module_client: Option<crate::modules::Worker>,
    pub modules: Option<crate::modules::Update>,
    brain_enabled: bool,
    brain_page: bool,
    ptt_pressed: bool,
    brain_bus: usize,
    pub device_draft: Option<crate::brain_device::Config>,
    device_draft_context: Option<(String, u64, (u64, u64))>,
    device_entry: Option<String>,
    hold_midi: Option<crate::brain::HoldMidi>,
    pub live_page: bool,
    pub sends_page: bool,
    pub sends_channel: bool,
    pub selected_monitor: usize,
    pub send_draft: Option<SendDraft>,
    send_entry: Option<String>,
    pub selected: usize,
    topology_page: Option<usize>,
    pub processing_draft: Option<ProcessingDraft>,
    pub structural_draft: Option<crate::structure::Draft>,
    pub structure_text_entry: bool,
    pub processing_field: usize,
    pub processing_entry: String,
    pub page: Page,
    pub width: u32,
    pub height: u32,
    pub device_ready: bool,
    queue: VecDeque<Event>,
    pressed: BTreeSet<String>,
    blocked: BTreeSet<String>,
    focused: bool,
    pub message: String,
    /// Desired feedback only, latest slot; no physical output endpoint.
    pub leds: Option<crate::console::LedState>,
    mode_picker: bool,
    scope: String,
    review_id: Option<u64>,
    review_page: usize,
    review_seen: BTreeSet<usize>,
    role_required: bool,
    role_client: Option<crate::roles::Client>,
    pub role_status: Option<crate::roles::Status>,
    observed_generation: u64,
    attachment_fence: Option<u64>,
}
impl Frontend {
    pub fn new(config: Config) -> Self {
        let scope = config.scope.clone();
        Self {
            provider: Provider::start(config.clone()),
            state: None,
            module_config: config,
            module_client: None,
            modules: None,
            brain_enabled: false,
            brain_page: false,
            ptt_pressed: false,
            brain_bus: 0,
            device_draft: None,
            device_draft_context: None,
            device_entry: None,
            hold_midi: None,
            live_page: false,
            sends_page: false,
            sends_channel: false,
            selected_monitor: 0,
            send_draft: None,
            send_entry: None,
            selected: 0,
            topology_page: None,
            processing_draft: None,
            structural_draft: None,
            structure_text_entry: false,
            processing_field: 0,
            processing_entry: String::new(),
            page: Page::Mix,
            width: 1920,
            height: 1080,
            device_ready: true,
            queue: VecDeque::new(),
            pressed: BTreeSet::new(),
            blocked: BTreeSet::new(),
            focused: true,
            message: String::new(),
            leds: None,
            mode_picker: false,
            scope,
            review_id: None,
            review_page: 0,
            review_seen: BTreeSet::new(),
            role_required: false,
            role_client: None,
            role_status: None,
            observed_generation: 1,
            attachment_fence: None,
        }
    }
    pub fn enable_brain_audio(&mut self) -> Result<(), String> {
        if self.module_config.wire_version != 2 {
            return Err("--brain-audio requires explicit dynamic C-AUDIO2 session".into());
        }
        self.brain_enabled = true;
        self.provider.send(None, Operation::EnableBrain)
    }
    pub fn talkback_release(&mut self) {
        self.ptt_pressed = false;
        self.provider.brain_signal.release();
    }
    pub fn controller_removed(&mut self) {
        self.talkback_release();
        self.fence();
    }
    pub fn configure_talkback_controller(&mut self, channel: u8, note: u8) -> Result<(), String> {
        self.talkback_release();
        self.hold_midi = Some(crate::brain::HoldMidi::new(channel, note)?);
        Ok(())
    }
    pub fn inject_talkback_midi(&mut self, bytes: &[u8]) -> Result<(), String> {
        if let Some(pressed) = self.hold_midi.as_mut().and_then(|d| d.decode(bytes)) {
            self.inject_controller(if pressed {
                Action::TalkbackPress
            } else {
                Action::TalkbackRelease
            })?;
        }
        Ok(())
    }
    /// Explicit GP03-only fresh attachment; drops drafts, authority and queued intents.
    pub fn reconnect_legacy(&mut self) -> Result<(), String> {
        self.fence();
        self.attachment_fence = Some(self.provider.generation());
        self.provider.send(None, Operation::LegacyReconnect)
    }
    /// Explicit capability probe; legacy providers are never probed by default.
    pub fn enable_processing(&mut self) -> Result<(), String> {
        self.provider.send(None, Operation::EnableProcessing)
    }
    pub fn inject_controller(&mut self, action: Action) -> Result<(), String> {
        self.enqueue(Event::Controller {
            action,
            generation: self.provider.generation(),
        })
    }
    pub fn role_child_pid(&self) -> Option<u32> {
        self.role_client
            .as_ref()
            .and_then(crate::roles::Client::child_pid)
    }
    pub fn require_role(&mut self) {
        self.role_required = true;
        self.provider.authorization.store(false, Ordering::Release);
        self.fence();
    }
    pub fn attach_role(&mut self, config: crate::roles::Config) {
        self.require_role();
        self.role_status = None;
        self.role_client.take();
        self.role_client = Some(crate::roles::Client::start(
            config,
            self.provider.generation.clone(),
            self.provider.authorization.clone(),
        ));
    }
    pub fn fence(&mut self) {
        self.provider.fence();
        self.observed_generation = self.provider.generation();
        self.fence_local();
    }
    fn fence_local(&mut self) {
        self.talkback_release();
        if let Some(d) = &mut self.hold_midi {
            d.fence();
        }
        self.blocked.extend(self.queue.iter().filter_map(|e| {
            if let Event::Key { key, pressed: true } = e {
                Some(key.clone())
            } else {
                None
            }
        }));
        self.queue.clear();
        self.blocked.extend(self.pressed.iter().cloned());
        self.pressed.clear();
        self.mode_picker = false;
        // Detached content survives; no review, queued action or held authorization does.
        self.state = None;
        self.leds = None;
        self.review_id = None;
        self.review_page = 0;
        self.review_seen.clear();
    }
    pub fn enqueue(&mut self, event: Event) -> Result<(), String> {
        if matches!(&event,Event::Key{key,pressed:false} if key.eq_ignore_ascii_case("T"))
            || matches!(
                &event,
                Event::Controller {
                    action: Action::TalkbackRelease,
                    ..
                }
            )
        {
            self.talkback_release();
            self.queue.retain(|e| {
                !matches!(e,Event::Key{key,pressed:true} if key.eq_ignore_ascii_case("T"))
                    && !matches!(
                        e,
                        Event::Controller {
                            action: Action::TalkbackPress,
                            ..
                        }
                    )
            });
        }

        if matches!(&event,Event::Key{key,..} if key.len()>32) {
            return Err("key capacity".into());
        }
        // Recovery events must bypass congested input; they revoke before queued edits.
        match event {
            Event::Focus(value) => {
                if !value {
                    self.fence();
                }
                self.focused = value;
                return Ok(());
            }
            Event::DeviceLost => {
                self.fence();
                self.device_ready = false;
                return Ok(());
            }
            Event::DeviceRestored => {
                self.device_ready = true;
                return Ok(());
            }
            Event::Resize(w, h) => {
                if w == 0 || h == 0 {
                    self.fence();
                }
                self.width = w;
                self.height = h;
                return Ok(());
            }
            _ => {}
        }
        if self.queue.len() >= 64 {
            self.fence();
            self.message = "INPUT OVERFLOW: discarded intents; key release required".into();
            return Err(self.message.clone());
        }
        self.queue.push_back(event);
        Ok(())
    }
    pub fn pump(&mut self) {
        if self.ptt_pressed && self.focused && self.device_ready {
            self.provider.brain_signal.pulse();
        }
        if let Some(client) = &self.role_client
            && let Some(status) = client.take()
        {
            self.role_status = Some(status);
        }
        if self.provider.generation() != self.observed_generation {
            self.observed_generation = self.provider.generation();
            self.fence_local();
        }
        if let Some(u) = self.provider.take()
            && u.generation == self.provider.generation()
        {
            self.accept_update(u);
        }
        // Optional metadata gets its own connection and worker only after primary
        // attachment. No module query can consume control replies or block input.
        if self.module_config.remote.is_none()
            && self.module_config.wire_version == 1
            && self.module_client.is_none()
            && self.state.as_ref().is_some_and(|s| s.snapshot.is_some())
        {
            self.module_client = Some(crate::modules::Worker::start(self.module_config.clone()));
        }
        if let Some(client) = &self.module_client
            && let Some(update) = client.take()
        {
            self.modules = Some(update);
        }
        if self.processing_draft.as_ref().is_some_and(|d| {
            !self.processing_fresh()
                || d.generation != self.provider.generation()
                || self
                    .state
                    .as_ref()
                    .and_then(|s| s.processing.as_ref())
                    .is_none_or(|s| s.revision != d.revision)
                || self.page != Page::Channel
                || self.selected_input() != Some(d.input.as_str())
        }) {
            self.message =
                "Processing draft retained; fresh readback and new Apply/review required".into();
        }
        if self.structural_draft.as_ref().is_some_and(|d| {
            !self.fresh()
                || !self.state.as_ref().is_some_and(|u| {
                    u.structure_is_fresh()
                        && u.snapshot
                            .as_ref()
                            .is_some_and(|s| s.authority.revision == d.revision)
                })
                || d.generation != self.provider.generation()
        }) {
            self.message =
                "Structural draft retained; fresh readback and new Apply/review required".into();
        }
        if self
            .device_draft_context
            .as_ref()
            .is_some_and(|(r, g, identity)| {
                *g != self.provider.generation()
                    || !self.device_identity_matches(*identity)
                    || !self.fresh()
                    || self
                        .state
                        .as_ref()
                        .and_then(|u| u.snapshot.as_ref())
                        .is_none_or(|s| &s.authority.revision != r)
            })
        {
            self.message =
                "Device draft retained; reopen device editor to validate current identity".into();
        }
        while let Some(event) = self.queue.pop_front() {
            if let Event::Controller { action, generation } = event {
                if generation == self.provider.generation()
                    && self.focused
                    && self.device_ready
                    && self.width > 0
                    && self.height > 0
                {
                    let opens_editor = matches!(
                        action,
                        Action::SendTapEdit
                            | Action::SendLevelEdit
                            | Action::LiveEqEdit
                            | Action::ProcessingEdit
                            | Action::StructureEdit
                            | Action::MasterEqEdit
                            | Action::DeviceEdit
                    );
                    if let Err(e) = self.action(action) {
                        self.message = e;
                    }
                    // Match keyboard release behavior: unsent local field gestures
                    // must not fill the provider queue or delay paired observations.
                    if (self.processing_draft.is_none()
                        && self.structural_draft.is_none()
                        && self.device_draft.is_none()
                        && self.send_draft.is_none())
                        || opens_editor
                    {
                        let _ = self.provider.send(None, Operation::InputReleased);
                    }
                }
                continue;
            }
            if let Event::Key { key, pressed } = event {
                if !pressed {
                    if key.eq_ignore_ascii_case("T") {
                        self.talkback_release();
                    }
                    self.pressed.remove(&key);
                    self.blocked.remove(&key);
                    if self.focused
                        && ((self.processing_draft.is_none()
                            && self.structural_draft.is_none()
                            && self.device_draft.is_none()
                            && self.send_draft.is_none())
                            || matches!(key.as_str(), "E" | "S" | "L" | "F9" | "F11"))
                    {
                        let _ = self.provider.send(None, Operation::InputReleased);
                    }
                    continue;
                }
                if !self.focused
                    || self.width == 0
                    || self.height == 0
                    || !self.device_ready
                    || self.blocked.contains(&key)
                    || !self.pressed.insert(key.clone())
                {
                    continue;
                }
                if let Err(e) = self.key(&key) {
                    self.message = e;
                }
            }
        }
        use crate::midi::Color;
        let color = if !self.fresh() {
            Color::Red
        } else if self.state.as_ref().is_some_and(|s| s.review.is_some()) {
            Color::Yellow
        } else {
            Color::Green
        };
        self.leds = if self.role_required && !self.provider.authorization.load(Ordering::Acquire) {
            None
        } else {
            Some(crate::console::LedState {
                generation: self.provider.generation(),
                colors: [color; 8],
            })
        };
    }
    pub fn fresh(&self) -> bool {
        self.attachment_fence.is_none() && self.state.as_ref().is_some_and(|s| s.raw_fresh())
    }
    fn accept_update(&mut self, update: Update) {
        if let Some(required) = self.attachment_fence {
            if update.attachment_generation != required {
                return;
            }
            self.attachment_fence = None;
        }
        if let (Some(old), Some(new)) = (
            self.state.as_ref().and_then(|s| s.snapshot.as_ref()),
            update.snapshot.as_ref(),
        ) && (old.authority.inputs != new.authority.inputs
            || old.authority.monitors != new.authority.monitors
            || old.topology != new.topology)
        {
            // A bank position is presentation, never a stable provider identity.
            let selected = old
                .authority
                .inputs
                .get(self.selected)
                .and_then(|id| {
                    new.authority
                        .inputs
                        .iter()
                        .position(|candidate| candidate == id)
                })
                .unwrap_or(0);
            self.fence();
            self.selected = selected;
            let _ = self.provider.send(None, Operation::Cancel);
            // Require the worker to observe the new generation before edits resume.
            self.message = "Inventory changed: intents discarded; refreshing authority".into();
            return;
        }
        let id = update.review.as_ref().map(|(id, _)| *id);
        if id != self.review_id {
            self.review_id = id;
            self.review_page = 0;
            self.review_seen.clear();
        }
        self.state = Some(update);
    }
    pub fn processing_fresh(&self) -> bool {
        self.fresh()
            && self.state.as_ref().is_some_and(|u| {
                u.processing_age_ms.is_some_and(|age| {
                    age.saturating_add(u.received.elapsed().as_millis() as u64) <= 250
                }) && u.processing.as_ref().is_some_and(|p| {
                    u.snapshot
                        .as_ref()
                        .is_some_and(|s| s.authority.revision == p.revision)
                })
            })
    }
    pub fn processing_ready(&self) -> bool {
        self.processing_fresh()
            && self
                .state
                .as_ref()
                .and_then(|s| s.processing.as_ref())
                .is_some_and(|s| !s.faulted && s.channels.iter().all(|c| c.ready))
            && (!self.role_required || self.provider.authorization.load(Ordering::Acquire))
    }
    fn brain_confirmed(&self) -> Result<&crate::brain::Snapshot, String> {
        if !self.brain_enabled || !self.fresh() {
            return Err("Brain disabled/stale".into());
        }
        self.state
            .as_ref()
            .filter(|s| s.brain_is_fresh())
            .and_then(|s| s.brain.as_ref())
            .ok_or("fresh Brain readback required".into())
    }
    fn send(&mut self, mut operation: Operation) -> Result<(), String> {
        if self.role_required && !self.provider.authorization.load(Ordering::Acquire) {
            return Err("live GP09 role lease required; keyboard-only read-only surface".into());
        }
        if !self.fresh() {
            return Err("provider stale/unavailable; operation refused".into());
        }
        if let Operation::ReviewBrain { kind, body, device } = &mut operation
            && crate::audio::monitor_armed(kind, body)
        {
            *device = Some(Box::new(crate::audio::MonitorDevicePin::capture(
                self.state.as_ref().and_then(|u| u.device.as_ref()),
            )?));
        }
        let revision = self
            .state
            .as_ref()
            .and_then(|s| s.snapshot.as_ref())
            .map(|s| s.authority.revision.clone());
        if let Err(e) = self.provider.send(revision, operation) {
            self.fence();
            return Err(e);
        }
        Ok(())
    }
    fn key(&mut self, key: &str) -> Result<(), String> {
        if key == "F8" {
            return self.reconnect_legacy();
        }
        if key == "F5" {
            self.fence();
            self.attachment_fence = Some(self.provider.generation());
            return self.provider.send(None, Operation::Reconnect);
        }
        if let Some(entry) = &mut self.send_entry {
            match key {
                "Esc" => self.send_entry = None,
                "Backspace" => {
                    entry.pop();
                }
                "Enter" => {
                    let text = entry.clone();
                    self.action(Action::SendLevelText(text))?;
                    self.send_entry = None;
                }
                _ if key.chars().count() == 1
                    && key
                        .chars()
                        .all(|c| c.is_ascii_digit() || c == '.' || c == '-') =>
                {
                    if entry.len() >= 16 {
                        return Err("send entry capacity".into());
                    }
                    entry.push_str(key);
                }
                _ => {
                    return Err(
                        "send level entry: decimal dB; Enter accepts; Esc cancels entry".into(),
                    );
                }
            }
            return Ok(());
        }
        if let Some(entry) = &mut self.device_entry {
            match key {
                "Esc" => {
                    self.device_entry = None;
                }
                "Backspace" => {
                    entry.pop();
                }
                "Enter" => {
                    let text = entry.clone();
                    self.action(Action::DeviceText(text))?;
                    self.device_entry = None;
                }
                _ if key.chars().count() == 1 && !key.chars().any(char::is_control) => {
                    if entry.len() > 16000 {
                        return Err("device editor capacity".into());
                    }
                    entry.push_str(key);
                }
                _ => {}
            }
            return Ok(());
        }

        if self.structure_text_entry && self.structural_draft.is_some() {
            match key {
                "Esc" => {
                    self.structure_text_entry = false;
                    self.processing_entry.clear();
                }
                "Backspace" => {
                    self.processing_entry.pop();
                }
                "Enter" => {
                    let entry = self.processing_entry.clone();
                    if let Some(path) = entry.strip_prefix('@') {
                        self.action(Action::StructureImport(crate::structure::read_import(
                            std::path::Path::new(path),
                        )?))?;
                    } else {
                        self.action(Action::StructureText(entry))?;
                    }
                    self.structure_text_entry = false;
                }
                _ if key.chars().all(|c| !c.is_control()) && key.chars().count() == 1 => {
                    if self.processing_entry.len() + key.len() > 48 * 1024 {
                        return Err("owner JSON entry capacity".into());
                    }
                    self.processing_entry.push_str(key);
                }
                _ => return Err("JSON entry: Enter accepts, Esc cancels entry".into()),
            }
            return Ok(());
        }
        // Preserve raw text in queued events; normalize shortcuts only after
        // earlier events (F3/Enter) have changed the current editor mode.
        let normalized;
        let key = if key.chars().count() == 1 {
            normalized = key.to_uppercase();
            normalized.as_str()
        } else {
            key
        };
        if self
            .structural_draft
            .as_ref()
            .is_some_and(|d| d.master_eq.is_some())
        {
            match key {
                "C" => return self.action(Action::MasterEqChannel),
                "B" => return self.action(Action::MasterEqSection),
                _ => (),
            }
        }
        if self.brain_enabled {
            match key {
                "F3" if self.structural_draft.is_none() => return self.action(Action::BrainPage),
                "T" => return self.action(Action::TalkbackPress),
                _ => {}
            }
            if self.brain_page {
                if key == "E" {
                    return self.action(Action::DeviceEdit);
                }
                if self.device_draft.is_some() {
                    if key == "F4" {
                        return self.action(Action::DeviceApply);
                    }
                    if key == "F2" {
                        self.device_entry = Some(String::new());
                        return Ok(());
                    }
                    if key == "Esc" {
                        self.device_draft = None;
                        self.device_draft_context = None;
                        return Ok(());
                    }
                }
                let source = match key {
                    "0" => Some(crate::brain::Source::None),
                    "1" => Some(crate::brain::Source::Main),
                    "P" => Some(crate::brain::Source::Pfl {
                        input: self.selected,
                    }),
                    "L" => Some(crate::brain::Source::Afl {
                        input: self.selected,
                    }),
                    _ => None,
                };
                if let Some(source) = source {
                    return self.action(Action::BrainSource(source));
                }
                if key == "B" {
                    return self.action(Action::BrainArm);
                }
                if key == "D" {
                    return self.action(Action::BrainDim);
                }
                if key == "M" {
                    return self.action(Action::BrainMute);
                }
                if matches!(key, "+" | "-") {
                    let gain = self.brain_confirmed()?.monitor_gain_cdb;
                    return self.action(Action::BrainGain(
                        (gain + if key == "+" { 100 } else { -100 }).clamp(-9000, 0),
                    ));
                }
                if matches!(key, "U" | "I") {
                    let count = self
                        .state
                        .as_ref()
                        .and_then(|s| s.snapshot.as_ref())
                        .map_or(0, |s| s.authority.monitors.len());
                    self.brain_bus = if key == "U" {
                        self.brain_bus.saturating_sub(1)
                    } else {
                        self.brain_bus
                            .saturating_add(1)
                            .min(count.saturating_sub(1))
                    };
                    return Ok(());
                }
                if key == "O" {
                    return self.action(Action::BrainSource(crate::brain::Source::Monitor {
                        index: self.brain_bus,
                    }));
                }
                if matches!(key, "V" | "X" | "[" | "]") {
                    let b = self.brain_confirmed()?;
                    let (mut monitors, mut gain, mut mute) = (
                        b.talkback_monitors.clone(),
                        b.talkback_gain_cdb,
                        b.talkback_mute,
                    );
                    match key {
                        "V" => {
                            if let Some(i) = monitors.iter().position(|n| *n == self.brain_bus) {
                                monitors.remove(i);
                            } else {
                                monitors.push(self.brain_bus);
                                monitors.sort_unstable();
                            }
                        }
                        "X" => mute = !mute,
                        "[" => gain = (gain - 100).max(-9000),
                        "]" => gain = (gain + 100).min(0),
                        _ => {}
                    }
                    return self.action(Action::TalkbackConfigure {
                        monitors,
                        gain_cdb: gain,
                        mute,
                    });
                }
                if key == "F" {
                    let enabled = !self.brain_confirmed()?.talkback_foh;
                    return self.action(Action::TalkbackFoh(enabled));
                }
            }
        }
        if self.scope == "pa_configuration"
            && self.structural_draft.is_none()
            && self.review_id.is_none()
            && (key == "L" || (self.live_page && key == "E"))
        {
            return self.action(Action::LiveEqEdit);
        }
        if key == "G" {
            return self.send(Operation::Grant);
        }
        if key == "Q" {
            return self.send(Operation::ReleaseWriter);
        }
        if self.review_id.is_some() && matches!(key, "PageUp" | "PageDown") {
            let pages = self.review_lines().len().div_ceil(32).max(1);
            if key == "PageDown" {
                self.review_page = (self.review_page + 1).min(pages - 1);
            } else {
                self.review_page = self.review_page.saturating_sub(1);
            }
            return Ok(());
        }
        if self.sends_page && self.review_id.is_none() {
            match key {
                "F1" => {
                    self.send_cancel();
                    self.sends_channel = false;
                    return Ok(());
                }
                "F2" => {
                    self.send_cancel();
                    self.sends_channel = true;
                    return Ok(());
                }
                "F" => return self.action(Action::SwitchScope("foh".into())),
                "V" => return self.action(Action::SwitchScope("pa_configuration".into())),
                "U" => return self.action(Action::BrowseMonitor(-1)),
                "I" => return self.action(Action::BrowseMonitor(1)),
                "O" => {
                    return self.action(Action::SwitchScope(format!(
                        "monitor{}",
                        self.selected_monitor + 1
                    )));
                }
                "E" => return self.action(Action::SendTapEdit),
                "S" => {
                    self.action(Action::SendLevelEdit)?;
                    self.send_entry = Some(String::new());
                    return Ok(());
                }
                "F4" => return self.action(Action::SendApply),
                "1" => return self.action(Action::SendTap(crate::sends::Tap::RawPostMute)),
                "2" => return self.action(Action::SendTap(crate::sends::Tap::ProcessedPreFader)),
                "3" => return self.action(Action::SendTap(crate::sends::Tap::ProcessedPostFader)),
                _ => (),
            }
        }
        if self.structural_draft.is_some() {
            match key {
                "F3" => {
                    self.structure_text_entry = true;
                    self.processing_entry.clear();
                    return Ok(());
                }
                "U" => return self.action(Action::StructureField(-1)),
                "I" => return self.action(Action::StructureField(1)),
                "J" => return self.action(Action::StructureAdjust(-1)),
                "K" => return self.action(Action::StructureAdjust(1)),
                "F4" => return self.action(Action::StructureApply),
                "Backspace" => {
                    self.processing_entry.pop();
                    return Ok(());
                }
                "Enter" if !self.processing_entry.is_empty() => {
                    return self.action(Action::StructureText(self.processing_entry.clone()));
                }
                _ => (),
            }
            if key.len() == 1
                && key
                    .bytes()
                    .all(|b| b.is_ascii_digit() || b == b'.' || b == b'-')
            {
                if self.processing_entry.len() >= 128 {
                    return Err("field entry capacity".into());
                }
                self.processing_entry.push_str(key);
                return Ok(());
            }
        }
        if matches!(self.scope.as_str(), "pa_configuration" | "output_routes") {
            if key == "Z" {
                return self.action(Action::OutputMute);
            }
            if key == "X" {
                return self.action(Action::OutputRearm);
            }
        }
        if self.processing_draft.is_some() {
            if key == "Backspace" {
                self.processing_entry.pop();
                return Ok(());
            }
            if key.len() == 1
                && key
                    .bytes()
                    .all(|b| b.is_ascii_digit() || b == b'.' || b == b'-')
            {
                if self.processing_entry.len() >= 16 {
                    return Err("numeric entry capacity".into());
                }
                self.processing_entry.push_str(key);
                return Ok(());
            }
            if key == "Enter" && !self.processing_entry.is_empty() {
                return self.action(Action::ProcessingText(self.processing_entry.clone()));
            }
        }
        let action = actions::key_action(key).ok_or("unmapped key")?;
        self.action(action)
    }
    fn device_identity_matches(&self, identity: (u64, u64)) -> bool {
        self.fresh()
            && self.state.as_ref().is_some_and(|u| {
                u.device_is_fresh()
                    && u.device
                        .as_ref()
                        .and_then(crate::brain_device::Snapshot::identity)
                        == Some(identity)
            })
    }
    fn action(&mut self, action: Action) -> Result<(), String> {
        match action {
            Action::LiveEqEdit => {
                if self.scope != "pa_configuration" || self.module_config.wire_version != 2 {
                    return Err("live EQ requires explicit dynamic PA attachment".into());
                }
                if !self.live_page {
                    self.send_cancel();
                    self.live_page = true;
                    self.sends_page = false;
                    self.brain_page = false;
                    self.processing_draft = None;
                    self.structural_draft = None;
                    return self.provider.send(None, Operation::EnableLiveEq);
                }
                if self.structural_draft.is_some() || self.review_id.is_some() {
                    return Err("Apply or Cancel existing draft/review first".into());
                }
                if !self.live_eq_ready() || !self.state.as_ref().is_some_and(Update::writer_granted)
                {
                    return Err("fresh settled compatible live EQ readback and NEW explicit PA grant required".into());
                }
                let u = self
                    .state
                    .as_ref()
                    .filter(|u| u.structure_is_fresh())
                    .ok_or("fresh full PA readback required")?;
                self.structural_draft = Some(crate::structure::Draft::new_live_eq(
                    u.structural.as_ref().ok_or("PA unavailable")?,
                    u.live_eq.as_ref().ok_or("live EQ unavailable")?,
                    self.provider.generation(),
                )?);
                self.processing_entry.clear();
                Ok(())
            }
            Action::SendsPage => {
                if self.module_config.wire_version != 2 {
                    return Err("sends overview requires explicit dynamic attachment".into());
                }
                self.send_cancel();
                self.live_page = false;
                self.sends_page = true;
                self.brain_page = false;
                self.topology_page = None;
                self.processing_draft = None;
                self.structural_draft = None;
                self.provider.send(None, Operation::EnableSends)
            }
            Action::BrowseMonitor(delta) => {
                let count = self
                    .state
                    .as_ref()
                    .and_then(|u| u.snapshot.as_ref())
                    .ok_or("snapshot")?
                    .authority
                    .monitors
                    .len();
                if count == 0 {
                    return Err("no monitor inventory".into());
                }
                self.send_cancel();
                self.send_draft = None;
                self.send_entry = None;
                self.selected_monitor = (self.selected_monitor as i64 + i64::from(delta))
                    .rem_euclid(count as i64) as usize;
                Ok(())
            }
            Action::SwitchScope(scope) => {
                crate::scopes::value(&scope)?;
                let raw = self
                    .state
                    .as_ref()
                    .and_then(|u| u.snapshot.as_ref())
                    .ok_or("actual inventory required for reattachment")?;
                if !raw.authority.modes.iter().any(|(s, _)| s == &scope)
                    && !matches!(scope.as_str(), "pa_configuration" | "output_routes")
                {
                    return Err("scope not advertised".into());
                }
                self.fence();
                self.send_draft = None;
                self.send_entry = None;
                self.processing_draft = None;
                self.structural_draft = None;
                self.scope = scope.clone();
                self.module_config.scope = scope.clone();
                self.attachment_fence = Some(self.provider.generation());
                self.provider.send(None, Operation::SwitchScope(scope))?;
                self.message="Reattach read-only / new writer / fresh read and NEW explicit G grant required".into();
                Ok(())
            }
            Action::SendTapEdit | Action::SendLevelEdit => {
                if self.send_draft.is_some() || self.review_id.is_some() {
                    return Err("Apply or Cancel existing edit first".into());
                }
                let (input, monitor) = self.send_pair()?;
                if self.scope != format!("monitor{}", self.selected_monitor + 1)
                    || !self.state.as_ref().is_some_and(Update::writer_granted)
                {
                    return Err(
                        "O switches attachment; NEW G grant required for selected monitor".into(),
                    );
                }
                let raw = self.state.as_ref().unwrap().snapshot.as_ref().unwrap();
                let value = if matches!(action, Action::SendTapEdit) {
                    if !self.sends_ready() {
                        return Err("fresh settled raw/GP18 pair required".into());
                    }
                    SendDraftValue::Tap(
                        self.state
                            .as_ref()
                            .unwrap()
                            .sends
                            .as_ref()
                            .unwrap()
                            .send(&input, &monitor)
                            .ok_or("send absent")?
                            .target,
                    )
                } else {
                    let p = raw
                        .authority
                        .parameters
                        .iter()
                        .find(|p| {
                            p.target.input == input
                                && p.target.parameter == "send"
                                && p.target.monitor.as_deref() == Some(&monitor)
                        })
                        .ok_or("send level absent")?;
                    SendDraftValue::Level(
                        p.target_value.as_i64().ok_or("send level numeric")? as i32
                    )
                };
                self.send_draft = Some(SendDraft {
                    input,
                    monitor,
                    value,
                    revision: raw.authority.revision.clone(),
                    generation: self.provider.generation(),
                });
                Ok(())
            }
            Action::SendTap(tap) => {
                let d = self
                    .send_draft
                    .as_mut()
                    .ok_or("E opens separate tap draft")?;
                if !matches!(d.value, SendDraftValue::Tap(_)) {
                    return Err("level and tap reviews are separate".into());
                }
                d.value = SendDraftValue::Tap(tap);
                Ok(())
            }
            Action::SendLevelText(text) => {
                let d = self
                    .send_draft
                    .as_mut()
                    .ok_or("S opens exact level draft")?;
                if !matches!(d.value, SendDraftValue::Level(_)) {
                    return Err("level and tap reviews are separate".into());
                }
                if text.is_empty()
                    || text.len() > 16
                    || text
                        .chars()
                        .any(|c| !c.is_ascii_digit() && c != '.' && c != '-')
                {
                    return Err("decimal send dB required".into());
                }
                let value: f64 = text.parse().map_err(|_| "decimal send dB required")?;
                let scaled = value * 1000.;
                if !value.is_finite()
                    || !(-60000. ..=12000.).contains(&scaled)
                    || (scaled / 100. - (scaled / 100.).round()).abs() > 1e-8
                {
                    return Err("send level -60..+12dB in0.1dB steps".into());
                }
                let n = scaled.round() as i32;
                d.value = SendDraftValue::Level(n);
                Ok(())
            }
            Action::SendApply => {
                let d = self.send_draft.clone().ok_or("no send draft")?;
                let (input, monitor) = self.send_pair()?;
                if d.generation != self.provider.generation()
                    || d.input != input
                    || d.monitor != monitor
                    || self.scope != format!("monitor{}", self.selected_monitor + 1)
                    || !self.state.as_ref().is_some_and(Update::writer_granted)
                    || self
                        .state
                        .as_ref()
                        .and_then(|u| u.snapshot.as_ref())
                        .is_none_or(|s| s.authority.revision != d.revision)
                {
                    return Err("send draft context changed; cancel and reopen".into());
                }
                let operation = match d.value {
                    SendDraftValue::Tap(tap) => {
                        if !self.sends_ready() {
                            return Err("fresh settled paired sends required".into());
                        }
                        Operation::ReviewTap(json!({"input":input,"monitor":monitor,"tap":tap}))
                    }
                    SendDraftValue::Level(value) => Operation::ReviewSet {
                        target: json!({"input":input,"parameter":"send","monitor":monitor}),
                        value: json!(value),
                    },
                };
                self.send(operation)?;
                self.send_draft = None;
                self.send_entry = None;
                Ok(())
            }
            Action::DeviceEdit => {
                if !self.brain_enabled
                    || !self.fresh()
                    || self.state.as_ref().is_some_and(|u| u.review.is_some())
                {
                    return Err("fresh Brain device readback and no open review required".into());
                }
                let u = self
                    .state
                    .as_ref()
                    .filter(|u| u.device_is_fresh())
                    .ok_or("device stale/unavailable")?;
                if self.device_draft.is_none() {
                    self.device_draft = Some(
                        u.device
                            .as_ref()
                            .and_then(|d| d.observation.as_ref())
                            .ok_or("actual device configuration absent")?
                            .config
                            .clone(),
                    );
                }
                self.device_draft_context = Some((
                    u.snapshot
                        .as_ref()
                        .ok_or("snapshot")?
                        .authority
                        .revision
                        .clone(),
                    self.provider.generation(),
                    u.device
                        .as_ref()
                        .and_then(crate::brain_device::Snapshot::identity)
                        .ok_or("device identity unavailable")?,
                ));
                Ok(())
            }
            Action::DeviceText(text) => {
                if self.device_draft.is_none() {
                    return Err("open device draft first".into());
                }
                self.device_draft = Some(crate::brain_device::Config::decode(
                    crate::provider::parse_document(text.as_bytes())?,
                )?);
                Ok(())
            }
            Action::DeviceApply => {
                let config = self.device_draft.clone().ok_or("device draft required")?;
                let (revision, generation, identity) = self
                    .device_draft_context
                    .as_ref()
                    .ok_or("device draft context")?;
                if *generation != self.provider.generation()
                    || !self.device_identity_matches(*identity)
                    || self
                        .state
                        .as_ref()
                        .and_then(|u| u.snapshot.as_ref())
                        .is_none_or(|s| &s.authority.revision != revision)
                {
                    return Err("device draft context changed".into());
                }
                self.send(Operation::ReviewDevice(Box::new(config), *identity))?;
                self.device_draft = None;
                self.device_draft_context = None;
                Ok(())
            }
            Action::TalkbackRelease => {
                self.talkback_release();
                Ok(())
            }
            Action::TalkbackPress => {
                if self
                    .state
                    .as_ref()
                    .is_none_or(|s| !s.held_transport_authenticated)
                {
                    return Err(
                        "PTT unsupported: authenticated GP15-held-proof provider required".into(),
                    );
                }
                if !self.brain_enabled
                    || self.ptt_pressed
                    || !self.focused
                    || !self.device_ready
                    || !self
                        .state
                        .as_ref()
                        .is_some_and(|s| s.held_baseline_ready && s.writer_granted())
                {
                    return Err("fresh authorized Brain and new PTT edge required".into());
                }
                self.ptt_pressed = true;
                let intent = self.provider.brain_signal.press();
                if let Err(e) = self.send(Operation::BrainPress(intent)) {
                    self.talkback_release();
                    return Err(e);
                }
                Ok(())
            }
            Action::BrainPage => {
                if !self.brain_enabled {
                    return Err("Brain opt-in required".into());
                }
                self.brain_page = !self.brain_page;
                Ok(())
            }
            Action::BrainSource(source) => {
                let b = self.brain_confirmed()?.clone();
                self.send(Operation::ReviewBrain{device:None,kind:"brain_monitor_set".into(),body:json!({"source":source,"gain_cdb":b.monitor_gain_cdb,"mute":b.monitor_mute,"dim":b.monitor_dim,"armed":false})})
            }
            Action::BrainArm | Action::BrainDim | Action::BrainMute | Action::BrainGain(_) => {
                let b = self.brain_confirmed()?.clone();
                let gain = if let Action::BrainGain(n) = action {
                    n
                } else {
                    b.monitor_gain_cdb
                };
                self.send(Operation::ReviewBrain{device:None,kind:"brain_monitor_set".into(),body:json!({"source":b.source,"gain_cdb":gain,"mute":if matches!(action,Action::BrainMute){!b.monitor_mute}else{b.monitor_mute},"dim":if matches!(action,Action::BrainDim){!b.monitor_dim}else{b.monitor_dim},"armed":if matches!(action,Action::BrainArm){true}else{b.monitor_armed}})})
            }
            Action::TalkbackConfigure {
                monitors,
                gain_cdb,
                mute,
            } => {
                self.brain_confirmed()?;
                self.talkback_release();
                self.send(Operation::ReviewBrain {
                    device: None,
                    kind: "brain_talkback_set".into(),
                    body: json!({"monitors":monitors,"gain_cdb":gain_cdb,"mute":mute}),
                })
            }
            Action::TalkbackFoh(enabled) => {
                self.brain_confirmed()?;
                self.talkback_release();
                self.send(Operation::ReviewBrain {
                    device: None,
                    kind: "brain_talkback_foh".into(),
                    body: json!({"enabled":enabled}),
                })
            }
            Action::StructureEdit | Action::MasterEqEdit => {
                if self.processing_draft.is_some()
                    || self.structural_draft.is_some()
                    || self.state.as_ref().is_some_and(|u| u.review.is_some())
                {
                    return Err("Apply or Cancel existing edit first".into());
                }
                let state = self
                    .state
                    .as_ref()
                    .filter(|u| u.structure_is_fresh() && self.fresh())
                    .ok_or("fresh structural readback required")?;
                let snapshot = state.structural.as_ref().ok_or("structural unavailable")?;
                self.structural_draft = Some(if matches!(action, Action::MasterEqEdit) {
                    if self.scope != "pa_configuration" {
                        return Err("Master EQ requires the PA configuration scope".into());
                    }
                    let raw = &state
                        .snapshot
                        .as_ref()
                        .ok_or("Raw readback unavailable")?
                        .authority;
                    if raw.show_id != snapshot.show_id
                        || raw.epoch != snapshot.epoch
                        || raw.revision != snapshot.revision
                    {
                        return Err("Master EQ requires matching raw and PA readback".into());
                    }
                    if !snapshot.outputs_quiesced {
                        return Err(
                            "Mute outputs and wait for quiescence before opening Master EQ".into(),
                        );
                    }
                    crate::structure::Draft::new_master_eq(snapshot, self.provider.generation())?
                } else {
                    crate::structure::Draft::new(snapshot, &self.scope, self.provider.generation())?
                });
                self.sends_page = false;
                self.live_page = false;
                self.brain_page = false;
                self.topology_page = None;
                self.processing_entry.clear();
                Ok(())
            }
            Action::MasterEqChannel | Action::MasterEqSection => {
                if !self.processing_entry.is_empty() {
                    return Err("Accept or cancel the field entry before changing EQ view".into());
                }
                self.structural_draft
                    .as_mut()
                    .ok_or("F11 opens Master EQ")?
                    .eq_view(matches!(action, Action::MasterEqChannel))
            }
            Action::StructureField(delta) => {
                if !self.processing_entry.is_empty() {
                    return Err("accept or clear field entry first".into());
                }
                self.structural_draft
                    .as_mut()
                    .ok_or("F9 opens structural editor")?
                    .move_field(delta);
                Ok(())
            }
            Action::StructureText(text) => {
                self.structural_draft
                    .as_mut()
                    .ok_or("F9 opens structural editor")?
                    .text(&text)?;
                self.processing_entry.clear();
                Ok(())
            }
            Action::StructureImport(text) => {
                self.structural_draft
                    .as_mut()
                    .ok_or("F9 opens structural editor")?
                    .import(&text)?;
                self.processing_entry.clear();
                Ok(())
            }
            Action::StructureAdjust(delta) => {
                let snapshot = self
                    .state
                    .as_ref()
                    .filter(|u| u.structure_is_fresh())
                    .and_then(|u| u.structural.as_ref())
                    .ok_or("fresh structural readback required")?;
                self.structural_draft
                    .as_mut()
                    .ok_or("F9 opens structural editor")?
                    .adjust(delta, snapshot)
            }
            Action::StructureApply => {
                if !self.state.as_ref().is_some_and(Update::structure_is_fresh) {
                    return Err("fresh structural readback required for new review".into());
                }
                if !self.processing_entry.is_empty() {
                    return Err("accept field before Apply".into());
                }
                let draft = self
                    .structural_draft
                    .as_ref()
                    .ok_or("no structural draft")?;
                if let Some(context) = &draft.live_context {
                    if self.scope != "pa_configuration"
                        || !self.live_eq_ready()
                        || !self.state.as_ref().is_some_and(Update::writer_granted)
                        || draft.generation != self.provider.generation()
                        || self
                            .state
                            .as_ref()
                            .and_then(|u| u.live_eq.as_ref())
                            .is_none_or(|s| !s.same_context(context))
                    {
                        return Err("live EQ review context changed; cancel and reopen".into());
                    }
                    let operation = Operation::ReviewLiveEq(draft.body()?);
                    self.send(operation)?;
                    self.structural_draft = None;
                    self.structure_text_entry = false;
                    return Ok(());
                }
                if let Some(view) = &draft.master_eq {
                    if self.scope != "pa_configuration"
                        || !self.state.as_ref().is_some_and(Update::writer_granted)
                    {
                        return Err("Master EQ Apply requires current PA scope authority".into());
                    }
                    let current = self
                        .state
                        .as_ref()
                        .and_then(|u| u.structural.as_ref())
                        .ok_or("Structural readback unavailable")?;
                    if current.revision != draft.revision
                        || draft.generation != self.provider.generation()
                        || draft.master_context.as_ref()
                            != Some(&(current.show_id.clone(), current.epoch.clone()))
                        || !view.matches_readback(
                            current.pa_configuration_json.as_deref().unwrap_or(""),
                            &current.pa_program_buses,
                        )
                        || self
                            .state
                            .as_ref()
                            .and_then(|u| u.snapshot.as_ref())
                            .is_none_or(|s| {
                                s.authority.show_id != current.show_id
                                    || s.authority.epoch != current.epoch
                                    || s.authority.revision != current.revision
                            })
                    {
                        return Err(
                            "Master EQ context changed; cancel and reopen from current settings"
                                .into(),
                        );
                    }
                    if !current.outputs_quiesced {
                        return Err("Mute outputs and wait for quiescence before opening/applying Master EQ".into());
                    }
                }
                let operation = Operation::ReviewStructure {
                    kind: draft.kind.clone(),
                    body: draft.body()?,
                };
                self.send(operation)?;
                self.structural_draft = None;
                self.structure_text_entry = false;
                Ok(())
            }
            Action::OutputMute | Action::OutputRearm => {
                if self.structural_draft.is_some() {
                    return Err("Apply or Cancel structural draft first".into());
                }
                self.send(Operation::ReviewStructure {
                    kind: if matches!(action, Action::OutputMute) {
                        "output_mute"
                    } else {
                        "output_rearm"
                    }
                    .into(),
                    body: json!({}),
                })
            }
            Action::Topology => {
                self.send_cancel();
                self.topology_page = Some(0);
                Ok(())
            }
            Action::Bank(delta) if self.topology_page.is_some() => {
                let pages = self.topology_lines().len().div_ceil(25).max(1);
                self.topology_page = Some(
                    (self.topology_page.unwrap() as i64 + i64::from(delta)).rem_euclid(pages as i64)
                        as usize,
                );
                Ok(())
            }
            Action::ProcessingEdit => {
                if self.page != Page::Channel
                    || self.sends_page
                    || self.live_page
                    || self.brain_page
                    || self.topology_page.is_some()
                    || self.scope != "foh"
                    || !self.processing_ready()
                {
                    return Err(
                        "Processing editor requires Channel, FOH and fresh ready GP07".into(),
                    );
                }
                if self.mode_picker
                    || self.processing_draft.is_some()
                    || self.structural_draft.is_some()
                    || self.state.as_ref().is_some_and(|s| s.review.is_some())
                {
                    return Err("Apply/confirm or Cancel existing draft/review".into());
                }
                let s = self.state.as_ref().unwrap().processing.as_ref().unwrap();
                let c = s
                    .channels
                    .iter()
                    .find(|c| Some(c.input.as_str()) == self.selected_input())
                    .ok_or("processing input unavailable")?;
                self.processing_draft = Some(ProcessingDraft {
                    input: c.input.clone(),
                    config: c.target.clone(),
                    revision: s.revision.clone(),
                    generation: self.provider.generation(),
                });
                self.processing_field = 0;
                self.processing_entry.clear();
                self.message = "Local draft only. F4 Apply opens review; Esc Cancel".into();
                Ok(())
            }
            Action::ProcessingText(text) => {
                if !self.processing_ready() {
                    return Err("processing stale/not ready".into());
                }
                self.processing_draft
                    .as_mut()
                    .ok_or("E opens processing editor")?
                    .config
                    .set_text(crate::processing::FIELDS[self.processing_field], &text)?;
                self.processing_entry.clear();
                Ok(())
            }
            Action::ProcessingField(delta) => {
                if !self.processing_entry.is_empty() {
                    return Err("Enter accepts numeric entry; Backspace clears it".into());
                }
                if self.processing_draft.is_none() {
                    return Err("E opens processing editor".into());
                }
                self.processing_field = (self.processing_field as i64 + i64::from(delta))
                    .rem_euclid(crate::processing::FIELDS.len() as i64)
                    as usize;
                Ok(())
            }
            Action::ProcessingAdjust(delta) => {
                if !self.processing_ready() {
                    return Err("processing stale/not ready".into());
                }
                self.processing_draft
                    .as_mut()
                    .ok_or("E opens processing editor")?
                    .config
                    .adjust(crate::processing::FIELDS[self.processing_field], delta)
            }
            Action::ProcessingApply => {
                if !self.processing_entry.is_empty() {
                    return Err("Enter accepts numeric entry before Apply".into());
                }
                if !self.processing_ready() {
                    return Err("processing stale/not ready".into());
                }
                let d = self
                    .processing_draft
                    .as_ref()
                    .ok_or("no local processing draft")?
                    .clone();
                if self.selected_input() != Some(d.input.as_str())
                    || !self
                        .state
                        .as_ref()
                        .and_then(|u| u.processing.as_ref())
                        .is_some_and(|s| s.channels.iter().any(|c| c.input == d.input))
                {
                    return Err(
                        "retained draft input changed; select its original input or cancel".into(),
                    );
                }
                self.send(Operation::ReviewProcessing {
                    input: d.input,
                    config: d.config,
                })?;
                self.processing_draft = None;
                self.processing_entry.clear();
                self.message = "Apply requested review; no processing edit sent yet".into();
                Ok(())
            }
            Action::Page(page) => {
                self.live_page = false;
                self.sends_page = false;
                self.send_draft = None;
                self.send_entry = None;
                self.send_cancel();
                self.page = page;
                self.topology_page = None;
                Ok(())
            }
            Action::Move(delta) | Action::Bank(delta) => {
                let n = self
                    .state
                    .as_ref()
                    .and_then(|s| s.snapshot.as_ref())
                    .map_or(0, |s| s.authority.inputs.len());
                if n == 0 {
                    return Err("inputs unavailable".into());
                }
                let selected = if matches!(action, Action::Bank(_)) {
                    let bank_count = n.div_ceil(12);
                    let bank = (self.selected as i64 / 12 + i64::from(delta))
                        .rem_euclid(bank_count as i64) as usize;
                    (bank * 12 + self.selected % 12).min(n - 1)
                } else {
                    (self.selected as i64 + i64::from(delta)).rem_euclid(n as i64) as usize
                };
                self.send_cancel();
                self.send_draft = None;
                self.send_entry = None;
                self.selected = selected;
                Ok(())
            }
            Action::ModePicker => {
                if self.processing_draft.is_some() {
                    return Err("Apply or Cancel processing draft first".into());
                }
                self.mode_picker = true;
                self.message =
                    "Choose1 AUTO (accepted bounds),2 ASSIST,3 MANUAL then review/Enter".into();
                Ok(())
            }
            Action::ChooseMode(mode) => {
                if !self.mode_picker {
                    return Err("open A mode picker first".into());
                }
                self.mode_picker = false;
                self.scope_monitor()?;
                let bounds = if mode == Mode::Auto {
                    serde_json::to_value(
                        self.state
                            .as_ref()
                            .and_then(|s| s.snapshot.as_ref())
                            .ok_or("snapshot")?
                            .authority
                            .automation_bounds
                            .iter()
                            .filter(|b| {
                                self.scope_monitor()
                                    .is_ok_and(|monitor| b.target.monitor.as_deref() == monitor)
                            })
                            .collect::<Vec<_>>(),
                    )
                    .map_err(|e| e.to_string())?
                } else {
                    json!([])
                };
                self.send(Operation::Mode {
                    mode: format!("{mode:?}").to_lowercase(),
                    bounds,
                })
            }
            Action::Confirm => {
                if self.state.as_ref().is_none_or(|s| s.review.is_none()) {
                    return Err("no displayed reviewed confirmation".into());
                }
                let pages = self.review_lines().len().div_ceil(32).max(1);
                if (0..pages).any(|p| !self.review_seen.contains(&p)) {
                    return Err("review every displayed page before confirming".into());
                }
                let id = self
                    .state
                    .as_ref()
                    .and_then(|s| s.review.as_ref())
                    .map(|(id, _)| *id)
                    .ok_or("no displayed reviewed confirmation")?;
                self.send(Operation::Confirm(id))
            }
            Action::Cancel | Action::Back => {
                self.send_draft = None;
                self.send_entry = None;
                self.processing_draft = None;
                self.device_draft = None;
                self.device_draft_context = None;
                self.device_entry = None;
                self.structural_draft = None;
                self.structure_text_entry = false;
                self.processing_entry.clear();
                self.send_cancel();
                Ok(())
            }
            Action::Adjust(delta) => self.adjust("fader", i64::from(delta), false),
            Action::AdjustPan(delta) => self.adjust("pan", i64::from(delta), false),
            Action::Mute => self.adjust("mute", 0, true),
            Action::Hold => self.adjust("fader", 0, false),
            Action::Release => {
                if self.processing_draft.is_some() {
                    return Err("Apply or Cancel processing draft first".into());
                }
                let target = self.target("fader")?;
                self.send(Operation::Preview(target))
            }
            Action::Menu => {
                self.message =
                    "G grant / Q writer release / A mode / R engine release / F5 reconnect".into();
                Ok(())
            }
            Action::Edit(_) | Action::Select(_) | Action::Status => {
                Err("action unavailable in real provider mode".into())
            }
        }
    }
    fn send_cancel(&mut self) {
        // Synchronize the local generation immediately: the physical key-up after
        // navigation must release the fence, not be discarded by the next pump.
        self.fence();
        let _ = self.provider.send(None, Operation::Cancel);
    }
    fn selected_input(&self) -> Option<&str> {
        self.state
            .as_ref()?
            .snapshot
            .as_ref()?
            .authority
            .inputs
            .get(self.selected)
            .map(String::as_str)
    }
    fn scope_monitor(&self) -> Result<Option<&str>, String> {
        if self.scope == "foh" {
            return Ok(None);
        }
        let authority = &self
            .state
            .as_ref()
            .and_then(|s| s.snapshot.as_ref())
            .ok_or("snapshot unavailable")?
            .authority;
        if !authority
            .modes
            .iter()
            .any(|(scope, _)| scope == &self.scope)
        {
            return Err("scope not advertised".into());
        }
        let number = self
            .scope
            .strip_prefix("monitor")
            .ok_or("not a channel scope")?;
        authority
            .monitors
            .iter()
            .find(|id| id.strip_prefix("monitor-") == Some(number))
            .map(|id| Some(id.as_str()))
            .ok_or_else(|| "monitor scope unavailable".into())
    }
    fn target(&self, parameter: &str) -> Result<Value, String> {
        if self.sends_page
            && (self.scope != format!("monitor{}", self.selected_monitor + 1)
                || self.send_draft.is_some())
        {
            return Err(
                "browsing another monitor is read-only; O reattaches; G explicitly grants".into(),
            );
        }
        if self.scope != "foh" && parameter == "pan" {
            return Err("pan unavailable in monitor scope; send gain unchanged".into());
        }
        let s = self
            .state
            .as_ref()
            .and_then(|s| s.snapshot.as_ref())
            .ok_or("snapshot unavailable")?;
        let input = s
            .authority
            .inputs
            .get(self.selected)
            .ok_or("input unavailable")?;
        let monitor = self.scope_monitor()?;
        Ok(if monitor.is_none() || parameter == "mute" {
            json!({"input":input,"parameter":parameter})
        } else {
            json!({"input":input,"parameter":"send","monitor":monitor.ok_or("monitor scope unavailable")?})
        })
    }
    fn adjust(&mut self, parameter: &str, delta: i64, toggle: bool) -> Result<(), String> {
        if self.processing_draft.is_some()
            || self.mode_picker
            || self.state.as_ref().is_some_and(|s| s.review.is_some())
        {
            return Err("confirm/cancel existing review first".into());
        }
        let target = self.target(parameter)?;
        let s = self
            .state
            .as_ref()
            .and_then(|s| s.snapshot.as_ref())
            .ok_or("snapshot")?;
        let p = s
            .authority
            .parameters
            .iter()
            .find(|p| crate::audio::target_value(&p.target) == target)
            .ok_or("parameter unavailable")?;
        let value = if toggle {
            json!(!p.target_value.as_bool().ok_or("mute unavailable")?)
        } else {
            json!(
                p.target_value
                    .as_i64()
                    .ok_or("integer target unavailable")?
                    .checked_add(delta)
                    .ok_or("value overflow")?
            )
        };
        self.send(if toggle || delta == 0 {
            Operation::ReviewSet { target, value }
        } else {
            Operation::Set { target, value }
        })
    }
    pub fn synchronize_review(&mut self) {
        let id = self
            .state
            .as_ref()
            .and_then(|s| s.review.as_ref())
            .map(|(id, _)| *id);
        if id != self.review_id {
            self.review_id = id;
            self.review_page = 0;
            self.review_seen.clear();
        }
    }
    pub fn review_pages(&self) -> usize {
        self.review_lines().len().div_ceil(32).max(1)
    }
    fn review_lines(&self) -> Vec<String> {
        self.state
            .as_ref()
            .and_then(|s| s.review.as_ref())
            .map_or_else(Vec::new, |(_, text)| {
                text.split('\n')
                    .flat_map(|line| {
                        let chars: Vec<_> = line.chars().collect();
                        chars
                            .chunks(156)
                            .map(|c| c.iter().collect())
                            .collect::<Vec<String>>()
                    })
                    .collect()
            })
    }
    /// Called after an actual renderer submission, never just by input polling.
    pub fn mark_presented(&mut self) {
        if self.device_ready
            && self.width > 0
            && self.height > 0
            && self.fresh()
            && self.review_id.is_some()
        {
            self.review_seen.insert(self.review_page);
        }
    }
    fn topology_lines(&self) -> Vec<String> {
        let Some(snapshot) = self.state.as_ref().and_then(|s| s.snapshot.as_ref()) else {
            return vec!["Topology unavailable: waiting for fresh provider state".into()];
        };
        let Some(topology) = &snapshot.topology else {
            return vec!["Physical patch unavailable in selected legacy provider contract".into()];
        };
        let mut lines = vec![
            format!(
                "Map {} / revision {} / evidence {}",
                topology.identity, topology.map_revision, topology.mapping_evidence
            ),
            format!(
                "{} logical inputs / {} USB capture slots / {} USB playback slots / {} monitors / {} PA outputs",
                topology.inputs.len(),
                topology.capture_channels,
                topology.playback_channels,
                topology.monitors,
                topology.pa_outputs
            ),
        ];
        if let Some(resources) = &snapshot.resources {
            lines.push(format!("ADMITTED {} / {} bytes; work {} / {}; estimated snapshot {} / {} bytes; {} writers x {} replies",
                resources.admission.estimated_bytes, resources.render_budget.bytes, resources.admission.sample_operations, resources.render_budget.sample_operations,
                resources.admission.estimated_snapshot_bytes, resources.snapshot_assembly_bytes, resources.live_writer_capacity, resources.reply_history_per_writer));
        }
        for input in &topology.inputs {
            lines.push(format!(
                "INPUT {} <- USB capture slot {} <- physical {}",
                input.id, input.capture_slot, input.physical_port
            ));
        }
        for slot in &topology.measurement_slots {
            lines.push(format!(
                "MEASUREMENT USB capture slot {slot} / excluded from program inputs"
            ));
        }
        for output in &topology.outputs {
            let source = match output.source {
                None => "UNASSIGNED / SILENT".into(),
                Some(crate::topology::OutputSource::Main { channel }) => {
                    format!("main channel {}", channel + 1)
                }
                Some(crate::topology::OutputSource::Monitor { index }) => {
                    format!("monitor {}", index + 1)
                }
                Some(crate::topology::OutputSource::Pa { index }) => {
                    format!("PA output {}", index + 1)
                }
            };
            lines.push(format!(
                "OUTPUT {}: {source} -> USB playback slot {} -> physical {}",
                output.id, output.playback_slot, output.physical_port
            ));
        }
        lines
            .into_iter()
            .flat_map(|line| {
                line.chars()
                    .collect::<Vec<_>>()
                    .chunks(150)
                    .map(|chunk| chunk.iter().collect::<String>())
                    .collect::<Vec<_>>()
            })
            .collect()
    }
    pub fn scene(&self) -> Scene {
        let mut scene = Scene::default();
        scene.primitives.push(Primitive::Rect {
            x: 0,
            y: 0,
            w: 1920,
            h: 1080,
            fill: "#10151d",
        });
        let mut line = |y: u32, text: String, color: &'static str| {
            scene.primitives.push(Primitive::Text {
                x: 24,
                y,
                value: text.chars().take(156).collect(),
                color,
            });
        };
        if let Some((id, _)) = self.state.as_ref().and_then(|s| s.review.as_ref()) {
            let lines = self.review_lines();
            let pages = lines.len().div_ceil(32).max(1);
            line(
                12,
                format!(
                    "REVIEW {id} / page {} of {pages} / exact provider request",
                    self.review_page + 1
                ),
                "#f1bd6b",
            );
            line(
                48,
                if self.fresh() {
                    "FRESH / review every page before Enter".into()
                } else {
                    "STALE / CONFIRM DISABLED".into()
                },
                "#f47c85",
            );
            for (row, text) in lines
                .iter()
                .skip(self.review_page * 32)
                .take(32)
                .enumerate()
            {
                line(96 + row as u32 * 24, text.clone(), "#e4e8e9");
            }
            line(
                936,
                "PageUp/PageDown scroll review | Enter confirm after all pages | Esc cancel".into(),
                "#66dfd3",
            );
            line(972, self.message.clone(), "#f47c85");
            return scene;
        }
        if self.sends_page {
            return self.sends_scene();
        }
        if self.live_page && self.structural_draft.is_none() {
            return self.live_scene();
        }
        if let Some(d) = &self.device_draft {
            line(
                12,
                "LOCAL DEVICE DRAFT from confirmed configuration / no PCM in Desk".into(),
                "#66dfd3",
            );
            line(48,"F2 enter complete JSON replacement | F4 Apply complete review | Esc discard | physical evidence remains explicit".into(),"#f1bd6b");
            for (i, text) in d.review().unwrap_or_default().lines().take(33).enumerate() {
                line(96 + i as u32 * 24, text.into(), "#e4e8e9");
            }
            if let Some(entry) = &self.device_entry {
                line(924, format!("JSON entry: {entry}"), "#66dfd3");
            }
            line(984, self.message.clone(), "#f47c85");
            return scene;
        }
        if self.brain_page {
            line(
                12,
                format!(
                    "BRAIN AUDIO / scope {} / {}",
                    self.scope,
                    if self.fresh() { "fresh" } else { "STALE" }
                ),
                "#66dfd3",
            );
            line(48,"F3 return | T hold PTT, release closes | G grant | Q release writer | F5 reconnect read-only".into(),"#f1bd6b");
            if let Some(u) = &self.state {
                line(
                    84,
                    format!(
                        "AUTHORIZED {} | compact {} / ready {} | {}",
                        u.writer_granted(),
                        u.held_status.as_ref().is_some_and(|h| h.fresh()),
                        u.held_status
                            .as_ref()
                            .is_some_and(|h| h.fresh() && h.talkback_path_ready),
                        u.brain_status
                    ),
                    "#e4e8e9",
                );
                if let Some(b) = &u.brain {
                    line(
                        132,
                        format!(
                            "APPLIED revision {} frame {} | readiness {} | observation {}",
                            b.revision,
                            b.frame,
                            b.audible_path_ready,
                            if u.brain_is_fresh() && self.fresh() {
                                "FRESH"
                            } else {
                                "STALE"
                            }
                        ),
                        "#e4e8e9",
                    );
                    line(
                        180,
                        format!(
                            "ONE LISTEN SOURCE {:?} / selection generation {}",
                            b.source, b.selection_generation
                        ),
                        "#66dfd3",
                    );
                    line(216,"PFL post EQ/compressor, PRE mute/fader/pan, centered | AFL POST all, stereo".into(),"#e4e8e9");
                    line(
                        252,
                        format!(
                            "Monitor {:+.2} dB / mute {} / dim {} (-20 dB) / armed {}",
                            f64::from(b.monitor_gain_cdb) / 100.,
                            b.monitor_mute,
                            b.monitor_dim,
                            b.monitor_armed
                        ),
                        "#e4e8e9",
                    );
                    line(288,"0 none | 1 main | P selected PFL | L selected AFL | O bus | B separate arm | D dim | M mute | +/- gain".into(),"#f1bd6b");
                    let buses = u.snapshot.as_ref().map(|s| &s.authority.monitors);
                    line(
                        336,
                        format!(
                            "Bus cursor {}: {} | U/I browse actual buses | V toggle destination (review required)",
                            self.brain_bus,
                            buses
                                .and_then(|v| v.get(self.brain_bus))
                                .map_or("unavailable", String::as_str)
                        ),
                        "#66dfd3",
                    );
                    line(
                        372,
                        format!(
                            "TB destinations {:?} / separate FOH {} / mute {} / gain {:+.2} dB",
                            b.talkback_monitors,
                            b.talkback_foh,
                            b.talkback_mute,
                            f64::from(b.talkback_gain_cdb) / 100.
                        ),
                        "#e4e8e9",
                    );
                    line(408,"X TB mute | [/] TB gain | F protected FOH toggle (separate talkback_foh grant)".into(),"#f1bd6b");
                    line(
                        456,
                        format!(
                            "PTT requested {} / APPLIED generation {:?} / high {} / 50 ms heartbeat, 150 ms deadman, 5 ms fade",
                            self.ptt_pressed,
                            u.held_status
                                .as_ref()
                                .filter(|h| h.fresh())
                                .and_then(|h| h.generation.as_ref()),
                            b.hold_generation_counter
                        ),
                        "#e4e8e9",
                    );
                    line(
                        504,
                        format!(
                            "Sample peaks mic {:.6} / out {:.6} / monitor {:.6}; age {:?} ms",
                            b.microphone_peak_nano as f64 / 1e9,
                            b.outgoing_peak_nano as f64 / 1e9,
                            b.monitor_peak_nano as f64 / 1e9,
                            u.brain_age_ms
                                .map(|age| age
                                    .saturating_add(u.received.elapsed().as_millis() as u64))
                        ),
                        "#66dfd3",
                    );
                    line(552,"Performer taps follow separately confirmed GP18 sends; source selection never implicitly sums.".into(),"#e4e8e9");
                }
                if let Some(d) = &u.device {
                    if let Some(o) = &d.observation {
                        line(
                            600,
                            format!(
                                "DEVICE {} endpoint {} / epoch {} map {} / {} / E edit complete configuration",
                                o.config.device_id,
                                o.config.endpoint,
                                o.brain_epoch,
                                o.brain_map,
                                if u.device_is_fresh() && self.fresh() {
                                    "FRESH"
                                } else {
                                    "STALE"
                                }
                            ),
                            "#66dfd3",
                        );
                        line(
                            636,
                            format!(
                                "MIC {} socket {} USB slot {} | L {}:{} | R {}:{}",
                                o.config.microphone.id,
                                o.config.microphone.socket,
                                o.config.microphone.slot,
                                o.config.monitor[0].socket,
                                o.config.monitor[0].slot,
                                o.config.monitor[1].socket,
                                o.config.monitor[1].slot
                            ),
                            "#e4e8e9",
                        );
                        let b = &o.status["monitor_bridge"];
                        line(
                            672,
                            format!(
                                "Bridge ratio {} ppb / skew {} ppb / occupancy {} frames / target {} frames",
                                b["ratio_ppb"],
                                b["skew_ppb"],
                                b["occupancy_frames"],
                                b["target_frames"]
                            ),
                            "#e4e8e9",
                        );
                        line(
                            708,
                            format!(
                                "Latency nominal: queue {} us + filter {} us / mapping uncertainty {} milliframes",
                                b["queue_latency_nominal_us"],
                                b["filter_latency_nominal_us"],
                                b["physical_mapping_uncertainty_milliframes"]
                            ),
                            "#e4e8e9",
                        );
                        line(
                            744,
                            format!(
                                "Fault {} / underruns {} / overflows {} / rejected {} / physical lock {} / capture drops {} blocks",
                                b["fault"],
                                b["underruns"],
                                b["overflows"],
                                b["rejected"],
                                b["physical_clock_lock_verified"],
                                o.status["capture_queue_dropped"]
                                    .as_u64()
                                    .map_or_else(|| "--".into(), |n| n.to_string())
                            ),
                            "#e4e8e9",
                        );
                    } else {
                        line(
                            600,
                            "Device unavailable; no endpoint identity invented".into(),
                            "#f1bd6b",
                        );
                    }
                }
                if let Some(r) = &u.device_final {
                    line(
                        792,
                        format!(
                            "Device {} ticket {} revision {} / accepted intent is not applied device",
                            r.state, r.ticket, r.revision
                        ),
                        "#f1bd6b",
                    );
                }
                line(
                    864,
                    u.last_operation
                        .clone()
                        .unwrap_or_else(|| "No requested change".into()),
                    "#f1bd6b",
                );
            }
            line(936,"All configuration changes require complete review + Enter. Attachment cannot arm or recall a mix.".into(),"#e4e8e9");
            line(984, self.message.clone(), "#f47c85");
            return scene;
        }
        if let Some(draft) = &self.structural_draft {
            if let Some(view) = &draft.master_eq {
                let mut scene = view.scene(
                    &draft.document,
                    draft.selected,
                    &self.processing_entry,
                    &self.message,
                );
                if draft.live_context.is_some() {
                    for p in &mut scene.primitives {
                        if let Primitive::Text { value, .. } = p
                            && value.starts_with("MUTED SETUP")
                        {
                            *value="LIVE EQ ONLY / current readback + unsent draft / full review; never rearms outputs".into();
                        }
                    }
                }
                return scene;
            }
            line(
                12,
                format!(
                    "LOCAL {} DRAFT / scope {} / revision {}",
                    draft.kind, self.scope, draft.revision
                ),
                "#66dfd3",
            );
            line(
                48,
                "OWNER FIELD NAMES/UNITS / no change sent / complete provider validation on Apply"
                    .into(),
                "#f1bd6b",
            );
            let start = draft.selected / 28 * 28;
            for (row, path) in draft.fields.iter().skip(start).take(28).enumerate() {
                line(
                    108 + row as u32 * 24,
                    format!(
                        "{} {} = {}",
                        if start + row == draft.selected {
                            ">"
                        } else {
                            " "
                        },
                        path,
                        draft
                            .document
                            .pointer(path)
                            .unwrap()
                            .to_string()
                            .chars()
                            .take(100)
                            .collect::<String>()
                    )
                    .chars()
                    .take(156)
                    .collect(),
                    "#e4e8e9",
                );
            }
            line(
                816,
                format!(
                    "FIELD {}/{} / {} entry {}",
                    draft.selected + 1,
                    draft.fields.len(),
                    if self.structure_text_entry {
                        "JSON/import"
                    } else {
                        "numeric"
                    },
                    self.processing_entry
                        .chars()
                        .rev()
                        .take(100)
                        .collect::<Vec<_>>()
                        .into_iter()
                        .rev()
                        .collect::<String>()
                ),
                "#66dfd3",
            );
            line(
                864,
                "U/I field | J/K adjust | F3 JSON or @absolute/import.json, Enter accepts | F4 review"
                    .into(),
                "#9caebc",
            );
            line(900,"F4 Apply: complete protected review | Esc cancel | outputs must be quiesced before prepare".into(),"#f1bd6b");
            line(948, self.message.clone(), "#f47c85");
            return scene;
        }
        if let Some(page) = self.topology_page {
            let lines = self.topology_lines();
            let pages = lines.len().div_ceil(25).max(1);
            let page = page.min(pages - 1);
            line(
                12,
                format!(
                    "PHYSICAL PATCH / page {} of {pages} / scope {}",
                    page + 1,
                    self.scope
                ),
                "#66dfd3",
            );
            line(
                48,
                if self.fresh() {
                    "PROVIDER FRESH / mapping is a descriptor, not hardware verification"
                } else {
                    "STALE / unavailable / edits disabled"
                }
                .into(),
                "#f1bd6b",
            );
            if let Some(clock) = self
                .state
                .as_ref()
                .and_then(|s| s.snapshot.as_ref())
                .and_then(|s| s.clock.as_ref())
            {
                line(
                    96,
                    format!(
                        "CLOCK {} / {} Hz / epoch {} / source next frame {} / {:?}",
                        clock.domain, clock.sample_rate, clock.epoch, clock.next_frame, clock.state
                    ),
                    "#e4e8e9",
                );
                line(
                    132,
                    format!(
                        "ADAT {:?} / physical mapping {} / fault {}",
                        clock.adat_lock,
                        if clock.mapping_verified {
                            "provider reports verified"
                        } else {
                            "UNVERIFIED"
                        },
                        clock.fault.as_deref().unwrap_or("none reported")
                    ),
                    "#f1bd6b",
                );
            } else {
                line(
                    96,
                    "Common-clock observation unavailable / physical lock UNKNOWN".into(),
                    "#f1bd6b",
                );
            }
            line(180, "Logical identities and physical sockets are separate. USB slots below are zero-based.".into(), "#9caebc");
            for (row, text) in lines.iter().skip(page * 25).take(25).enumerate() {
                line(228 + row as u32 * 24, text.clone(), "#e4e8e9");
            }
            line(
                900,
                "PageUp/PageDown patch | F9 scope editor | F11 Master EQ | Z mute / X rearm (PA scope, reviewed)".into(),
                "#66dfd3",
            );
            line(948, self.message.clone(), "#f47c85");
            return scene;
        }
        line(
            12,
            format!(
                "SHR DESK / {} / {} / {}",
                if self.state.as_ref().is_some_and(|u| u.processing.is_some()) {
                    "REAL GP03 + GP07 FOH PROCESSING"
                } else {
                    "REAL GP03 RAW MIXER"
                },
                match self.page {
                    Page::Mix => "Mix",
                    Page::Channel => "Channel",
                    Page::Analysis => "Health / analysis unavailable",
                },
                self.scope
            ),
            "#66dfd3",
        );
        line(
            48,
            if self.fresh() {
                if self.state.as_ref().is_some_and(|u| u.processing.is_some()) {
                    "PROVIDER FRESH / OFFLINE UNPROTECTED / SIGNAL METERS UNAVAILABLE".into()
                } else {
                    "PROVIDER FRESH / RAW MIXER OFFLINE UNPROTECTED / METERS UNAVAILABLE".into()
                }
            } else {
                "STALE / UNAVAILABLE / MIX UNKNOWN / EDITS DISABLED".into()
            },
            "#f1bd6b",
        );
        line(
            168,
            self.role_status.as_ref().map_or_else(
                || {
                    if self.role_required {
                        "ROLE UNAVAILABLE / READ-ONLY".into()
                    } else {
                        "HEADLESS PROVIDER CHECK / no physical role claim".into()
                    }
                },
                |r| {
                    if self.page != Page::Analysis {
                        return r.message.clone();
                    }
                    format!(
                        "{} / acquisition {} / registry {}",
                        r.message,
                        r.generation.as_deref().unwrap_or("unavailable"),
                        r.registry_generation.as_deref().unwrap_or("unavailable")
                    )
                },
            ),
            "#f1bd6b",
        );
        if let Some(u) = &self.state {
            line(
                84,
                u.last_operation.as_ref().map_or_else(
                    || u.status.clone(),
                    |operation| {
                        if operation == &u.status {
                            operation.clone()
                        } else {
                            format!("{operation} / health: {}", u.status)
                        }
                    },
                ),
                "#e4e8e9",
            );
            if let Some(s) = &u.snapshot {
                line(
                    120,
                    if self.page == Page::Analysis {
                        format!(
                            "show {} / epoch {} / revision {} / frame {}",
                            s.authority.show_id, s.authority.epoch, s.authority.revision, s.frame
                        )
                    } else {
                        format!(
                            "Selected {} / bank {} / {} inputs / F6 provider health",
                            s.authority
                                .inputs
                                .get(self.selected)
                                .map_or("unavailable", String::as_str),
                            self.selected / 12 + 1,
                            s.authority.inputs.len()
                        )
                    },
                    "#9caebc",
                );
                line(
                    144,
                    format!(
                        "CONFIRMED MODE {} / scope {} / measured dB unavailable",
                        s.authority
                            .modes
                            .iter()
                            .find(|(scope, _)| scope == &self.scope)
                            .map_or("unavailable", |(_, mode)| mode.as_str())
                            .to_uppercase(),
                        self.scope
                    ),
                    "#f1bd6b",
                );
                if self.page == Page::Analysis {
                    line(192,"ANALYSIS UNAVAILABLE: no accepted measurement subscription; no fixture graphs".into(),"#9caebc");
                }
                if self.page == Page::Channel {
                    if let Some(processing) = &u.processing {
                        if let Some(channel) = processing
                            .channels
                            .iter()
                            .find(|c| Some(c.input.as_str()) == self.selected_input())
                        {
                            line(
                                204,
                                format!(
                                    "GP07 FOH: EQ -> compressor -> shared mute/fader/pan | Monitors: separately selected GP18 taps | {}",
                                    if self.processing_fresh() {
                                        "FRESH"
                                    } else {
                                        "STALE / EDITS DISABLED"
                                    }
                                ),
                                "#66dfd3",
                            );
                            line(
                                240,
                                format!(
                                    "{} / {} / {} / GR {}",
                                    channel.input,
                                    if processing.faulted {
                                        "FAULTED / EDITS DISABLED".to_string()
                                    } else if channel.ready {
                                        "READY".to_string()
                                    } else {
                                        format!(
                                            "TRANSITION {} frames remaining",
                                            channel.transition_remaining_frames
                                        )
                                    },
                                    if self.processing_draft.is_some() {
                                        "LOCAL DRAFT (not sent)"
                                    } else {
                                        "PROVIDER CONFIRMED"
                                    },
                                    if !self.processing_fresh() {
                                        "unavailable (stale)".into()
                                    } else if processing.faulted {
                                        "unavailable (fault)".into()
                                    } else if !channel.ready {
                                        "unavailable (transition)".into()
                                    } else if channel.target.compressor_bypass {
                                        "bypassed".into()
                                    } else {
                                        channel.gain_reduction_mdb.map_or(
                                            "unavailable".into(),
                                            |n| {
                                                format!(
                                                    "{:.1} dB attenuation (excludes makeup)",
                                                    n as f64 / 1000.0
                                                )
                                            },
                                        )
                                    }
                                ),
                                "#f1bd6b",
                            );
                            line(276, "    CONFIRMED SETTLED                               CONFIRMED TARGET                                 LOCAL DRAFT".into(), "#9caebc");
                            // Four stable band rows, then unchanged dynamics. Each endpoint
                            // is visible in a separate column; no band identity sorting.
                            let rows: [&[usize]; 12] = [
                                &[0],
                                &[1, 2, 3, 4],
                                &[5, 6, 7, 8],
                                &[9, 10, 11, 12],
                                &[13, 14, 15, 16],
                                &[17],
                                &[18],
                                &[19],
                                &[20],
                                &[21],
                                &[22],
                                &[23],
                            ];
                            for (row, indices) in rows.iter().enumerate() {
                                let display = |c: &crate::processing::Config| {
                                    if indices.len() == 1 {
                                        c.display(crate::processing::FIELDS[indices[0]])
                                    } else {
                                        let band = row;
                                        let (hz, gain, q, bypass) = c.bands()[band - 1];
                                        format!(
                                            "Band {band} bell {hz}Hz {:+.1}dB Q{:.1} {}",
                                            gain as f64 / 1000.0,
                                            q as f64 / 1000.0,
                                            if bypass { "BYPASS" } else { "ON" }
                                        )
                                    }
                                };
                                let selected = self.processing_draft.is_some()
                                    && indices.contains(&self.processing_field);
                                line(
                                    312 + row as u32 * 24,
                                    format!(
                                        "{} {:<48} {:<48} {}",
                                        if selected { ">" } else { " " },
                                        display(&channel.current),
                                        display(&channel.target),
                                        self.processing_draft
                                            .as_ref()
                                            .map_or("--".into(), |d| display(&d.config))
                                    ),
                                    if selected { "#66dfd3" } else { "#e4e8e9" },
                                );
                            }
                            if let Some(draft) = &self.processing_draft {
                                line(
                                    624,
                                    format!(
                                        "EDIT FIELD {}/24: {} (local only)",
                                        self.processing_field + 1,
                                        draft.config.display(
                                            crate::processing::FIELDS[self.processing_field]
                                        )
                                    ),
                                    "#66dfd3",
                                );
                            }
                            let tap_label = u.sends.as_ref().and_then(|ss| ss.channels.iter().find(|c| c.input == channel.input)).and_then(|c| c.sends.get(self.selected_monitor)).map(|send| format!("Monitor{} tap {} -> {} / {} frames / shared mute; post-fader before pan", self.selected_monitor+1, send.current.name(), send.target.name(),send.transition_remaining_frames)).unwrap_or_else(|| if self.module_config.wire_version == 1 {"Legacy monitor taps: raw_post_mute".into()} else {"Monitor tap unavailable: F12 queries GP18; no processing authority from monitor browsing".into()});
                            line(672, tap_label, "#9caebc");
                            line(708, "During transition output blends settled and target branches; GR is detector feedback, not a level meter".into(), "#9caebc");
                            line(
                                744,
                                format!(
                                    "CONFIRMED {} {}",
                                    self.scope,
                                    s.authority
                                        .parameters
                                        .iter()
                                        .filter(|p| p.target.input == channel.input
                                            && self.scope_monitor().is_ok_and(|monitor| p
                                                .target
                                                .monitor
                                                .as_deref()
                                                == monitor))
                                        .map(|p| format!(
                                            "{} {} / hold {}",
                                            p.target.parameter,
                                            parameter_display(&p.target.parameter, &p.target_value),
                                            optional_display(&p.target.parameter, &p.hold)
                                        ))
                                        .collect::<Vec<_>>()
                                        .join(" | ")
                                ),
                                "#9caebc",
                            );
                            line(
                                780,
                                format!("Processing: {}", u.processing_status),
                                "#f1bd6b",
                            );
                            line(
                                816,
                                format!(
                                    "Numeric entry: {} (display units; bypass 0=enabled / 1=bypassed) | Enter accepts value",
                                    self.processing_entry
                                ),
                                "#66dfd3",
                            );
                            line(852, self.message.clone(), "#f47c85");
                            line(888, "E Edit (FOH only) | U/I previous/next field | J/K -/+ one step (bypass toggles) | N/P -/+ 100 steps".into(), "#66dfd3");
                            line(924, "F4 Apply -> displayed review -> Enter Confirm | Esc Cancel | arrows select channel | F1/F2/F6 pages".into(), "#66dfd3");
                            line(960, "G grant configured scope | Q release writer | +/- fader | [ ] pan | M mute | H hold | R release | A mode".into(), "#9caebc");
                            line(996, "F5 reconnect | F8 legacy | F12 sends / explicit scope switch | fresh writer/read-only, no replay".into(), "#9caebc");
                            return scene;
                        }
                    } else {
                        line(
                            240,
                            format!("CHANNEL PROCESSING UNAVAILABLE: {}", u.processing_status),
                            "#f1bd6b",
                        );
                    }
                }
                let bank_start = self.selected / 12 * 12;
                for (row, (i, input)) in s
                    .authority
                    .inputs
                    .iter()
                    .enumerate()
                    .skip(bank_start)
                    .take(12)
                    .enumerate()
                {
                    if self.page == Page::Analysis
                        || self.page == Page::Channel && i != self.selected
                    {
                        continue;
                    }
                    let params: Vec<_> = s
                        .authority
                        .parameters
                        .iter()
                        .filter(|p| {
                            &p.target.input == input
                                && self
                                    .scope_monitor()
                                    .is_ok_and(|monitor| p.target.monitor.as_deref() == monitor)
                        })
                        .collect();
                    line(
                        192 + row as u32 * 32,
                        format!(
                            "{} {} {}",
                            if i == self.selected { ">" } else { " " },
                            input,
                            params
                                .iter()
                                .map(|p| format!(
                                    "{} {} / hold {}",
                                    p.target.parameter,
                                    parameter_display(&p.target.parameter, &p.target_value),
                                    optional_display(&p.target.parameter, &p.hold)
                                ))
                                .collect::<Vec<_>>()
                                .join(" / ")
                        ),
                        "#e4e8e9",
                    );
                }
                if let Some(input) = s.authority.inputs.get(self.selected) {
                    for (i, p) in s
                        .authority
                        .parameters
                        .iter()
                        .filter(|p| &p.target.input == input)
                        .take(5)
                        .enumerate()
                    {
                        let bound = s
                            .authority
                            .automation_bounds
                            .iter()
                            .find(|b| b.target == p.target)
                            .map(|b| {
                                format!(
                                    "{} to {}",
                                    parameter_display(&p.target.parameter, &json!(b.min)),
                                    parameter_display(&p.target.parameter, &json!(b.max))
                                )
                            })
                            .unwrap_or_else(|| "unavailable".into());
                        line(
                            600 + i as u32 * 24,
                            format!(
                                "{} {} / target {} / proposal {} / hold {} / owner {} / bounds {}",
                                p.target.parameter,
                                p.target.monitor.as_deref().unwrap_or("FOH"),
                                parameter_display(&p.target.parameter, &p.target_value),
                                optional_display(&p.target.parameter, &p.proposal),
                                optional_display(&p.target.parameter, &p.hold),
                                p.owner.as_deref().unwrap_or("unowned"),
                                bound
                            ),
                            "#9caebc",
                        );
                    }
                }
                if let Some(c) = s
                    .authority
                    .inputs
                    .get(self.selected)
                    .and_then(|input| s.coefficients.iter().find(|c| &c.input == input))
                {
                    line(
                        756,
                        if self.page == Page::Analysis {
                            format!(
                                "ACTUAL LINEAR NANO: {:?}",
                                c.current_nanogain.iter().map(|n| n.0).collect::<Vec<_>>()
                            )
                        } else {
                            format!(
                                "RENDERED GAIN {} / RAMP TARGET {} (linear gain, measured dB unavailable)",
                                linear_display(c.current_nanogain[0]),
                                linear_display(c.ramp_target_nanogain[0])
                            )
                        },
                        "#66dfd3",
                    );
                    line(
                        792,
                        if self.page == Page::Analysis {
                            format!(
                                "RAMP TARGET NANO: {:?}",
                                c.ramp_target_nanogain
                                    .iter()
                                    .map(|n| n.0)
                                    .collect::<Vec<_>>()
                            )
                        } else {
                            format!(
                                "RENDERED PAN L {} / R {} / MUTE {} / {} monitor lanes (select monitor scope for send)",
                                linear_display(c.current_nanogain[1]),
                                linear_display(c.current_nanogain[2]),
                                if c.current_nanogain[3].0 == 0 {
                                    "on"
                                } else {
                                    "off"
                                },
                                s.authority.monitors.len()
                            )
                        },
                        "#9caebc",
                    );
                }
            }
        }
        let module_fresh = self.modules.as_ref().is_some_and(|m| m.fresh());
        line(
            840,
            if module_fresh {
                self.modules
                    .as_ref()
                    .unwrap()
                    .status
                    .as_ref()
                    .unwrap()
                    .compact()
            } else {
                "MODULES UNAVAILABLE / STALE / raw GP03 mixer remains separate".into()
            },
            "#f1bd6b",
        );
        if self.page == Page::Analysis {
            if let Some(m) = &self.modules {
                if module_fresh {
                    for (i, text) in m
                        .status
                        .as_ref()
                        .unwrap()
                        .lines()
                        .into_iter()
                        .take(13)
                        .enumerate()
                    {
                        line(240 + i as u32 * 24, text, "#9caebc");
                    }
                } else {
                    line(240, m.message.clone(), "#f47c85");
                }
            } else {
                line(
                    240,
                    "GP05 module health pending/unavailable".into(),
                    "#9caebc",
                );
            }
        }
        line(864, self.message.clone(), "#f47c85");
        line(936,"G grant | Q writer release | +/- fader | [ ] pan | M mute | H hold | R preview release".into(),"#9caebc");
        line(
            972,
            "A mode | 1/2/3 choose | Enter confirm | Esc cancel | arrows select | F1/F2/F6 pages"
                .into(),
            "#9caebc",
        );
        line(
            1008,
            "F5 reconnect | F8 legacy reconnect | F7 patch | F9 PA/routes editor | F11 muted EQ | F12 sends | PA: L live EQ"
                .into(),
            "#9caebc",
        );
        scene
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn complete_large_review_requires_all_rendered_pages() {
        let mut f = Frontend::new(Config {
            wire_version: 1,
            remote: None,
            endpoint: "/nonexistent/desk-test.sock".into(),
            show: "11111111-1111-4111-8111-111111111111".into(),
            epoch: 9,
            writer: "review-test".into(),
            scope: "foh".into(),
        });
        let corpus: Value =
            serde_json::from_str(include_str!("../tests/fixtures/gp03/v1/e03-rendered.json"))
                .unwrap();
        let review=(0..80).map(|i|format!("target input-{i:02} current -6000 target -3000 delta3000 show/epoch/revision/scope ")).collect::<String>();
        f.state = Some(Update {
            device: None,
            device_final: None,
            device_fresh: false,
            device_age_ms: Some(0),
            brain: None,
            brain_final: None,
            brain_fresh: false,
            brain_age_ms: None,
            held_status: None,
            held_baseline_ready: false,
            held_transport_authenticated: false,
            brain_status: "disabled".into(),
            generation: 1,
            attachment_generation: 1,
            last_operation: None,
            writer_lease_remaining_ms: None,
            snapshot: Some(
                crate::audio::decode_snapshot(&serde_json::to_vec(&corpus["initial"]).unwrap())
                    .unwrap(),
            ),
            live_eq: None,
            live_eq_age_ms: None,
            live_eq_final: None,
            sends: None,
            sends_age_ms: None,
            sends_final: None,
            processing: None,
            processing_age_ms: None,
            processing_status: "disabled".into(),
            structural: None,
            structural_final: None,
            structural_fresh: false,
            structural_age_ms: Some(0),
            processing_final: None,
            fresh: true,
            snapshot_age_ms: Some(0),
            status: "review".into(),
            review: Some((42, review.clone())),
            received: Instant::now(),
        });
        f.synchronize_review();
        assert!(f.key("Enter").unwrap_err().contains("every displayed page"));
        let mut recovered = String::new();
        let pages = f.review_lines().len().div_ceil(32);
        for page in 0..pages {
            let s = f.scene();
            assert!(s.in_bounds());
            for p in s.primitives {
                if let Primitive::Text { y, value, .. } = p
                    && (96..864).contains(&y)
                {
                    recovered.push_str(&value);
                }
            }
            f.mark_presented();
            assert!(f.review_seen.contains(&page));
            if page + 1 < pages {
                f.key("PageDown").unwrap();
            }
        }
        assert_eq!(recovered, review);
        assert_eq!(f.review_seen.len(), pages);
    }
    #[test]
    fn active_bank_and_channel_selection_stay_visible_without_detail_overlap() {
        let mut f = Frontend::new(Config {
            wire_version: 1,
            remote: None,
            endpoint: "/nonexistent/desk-bank-test.sock".into(),
            show: "11111111-1111-4111-8111-111111111111".into(),
            epoch: 9,
            writer: "bank-test".into(),
            scope: "foh".into(),
        });
        let corpus: Value =
            serde_json::from_str(include_str!("../tests/fixtures/gp03/v1/e03-rendered.json"))
                .unwrap();
        let mut snapshot =
            crate::audio::decode_snapshot(&serde_json::to_vec(&corpus["initial"]).unwrap())
                .unwrap();
        // Layout-only future-capacity fixture, not an accepted GP03 snapshot.
        snapshot.authority.inputs = (1..=36).map(|n| format!("input-{n:02}")).collect();
        f.state = Some(Update {
            device: None,
            device_final: None,
            device_fresh: false,
            device_age_ms: Some(0),
            brain: None,
            brain_final: None,
            brain_fresh: false,
            brain_age_ms: None,
            held_status: None,
            held_baseline_ready: false,
            held_transport_authenticated: false,
            brain_status: "disabled".into(),
            generation: 1,
            attachment_generation: 1,
            last_operation: None,
            writer_lease_remaining_ms: None,
            snapshot: Some(snapshot),
            live_eq: None,
            live_eq_age_ms: None,
            live_eq_final: None,
            sends: None,
            sends_age_ms: None,
            sends_final: None,
            processing: None,
            processing_age_ms: None,
            processing_status: "disabled".into(),
            structural: None,
            structural_final: None,
            structural_fresh: false,
            structural_age_ms: Some(0),
            processing_final: None,
            fresh: true,
            snapshot_age_ms: Some(0),
            status: "layout fixture".into(),
            review: None,
            received: Instant::now(),
        });
        f.selected = 13;
        let scene = f.scene();
        assert!(scene.in_bounds());
        let rows: Vec<_> = scene
            .primitives
            .iter()
            .filter_map(|p| {
                if let Primitive::Text { y, value, .. } = p
                    && (192..600).contains(y)
                {
                    Some((*y, value))
                } else {
                    None
                }
            })
            .collect();
        assert_eq!(rows.len(), 12);
        assert!(rows.iter().any(|(_, s)| s.starts_with("> input-14")));
        assert!(rows.iter().all(|(y, _)| y + 24 < 600));
        f.page = Page::Channel;
        let scene = f.scene();
        assert!(
            scene
                .primitives
                .iter()
                .any(|p| matches!(p,Primitive::Text{value,..} if value.starts_with("> input-14")))
        );
    }
}

#[cfg(test)]
mod processing_tests {
    use super::*;
    pub(super) fn master_surface() -> (Frontend, Receiver<Request>) {
        let (mut f, rx) = brain_surface();
        f.scope = "pa_configuration".into();
        let mut snapshot = crate::structure::decode_snapshot(include_bytes!(
            "../tests/fixtures/gp14/v1/structure-16.json"
        ))
        .unwrap();
        // Altered bus map is a UI test input, not a new accepted producer fixture.
        snapshot.pa_program_buses = vec![0, 1];
        snapshot.outputs_quiesced = true;
        let u = f.state.as_mut().unwrap();
        u.snapshot.as_mut().unwrap().authority.revision = snapshot.revision.clone();
        u.structural = Some(snapshot);
        u.structural_fresh = true;
        u.structural_age_ms = Some(0);
        u.received = Instant::now();
        (f, rx)
    }
    #[test]
    fn master_eq_keyboard_edits_are_local_and_review_uses_existing_owner_transaction() {
        let (mut f, rx) = master_surface();
        f.key("F11").unwrap();
        assert!(!f.brain_page);
        let original = f.structural_draft.as_ref().unwrap().document.clone();
        assert!(f.scene().in_bounds());
        f.key("B").unwrap();
        f.key("I").unwrap();
        f.key("F3").unwrap();
        for key in ["-", "2", ".", "5", "Enter"] {
            f.key(key).unwrap();
        }
        let draft = f.structural_draft.as_ref().unwrap();
        for side in 0..2 {
            assert_eq!(
                draft.document["configuration"]["inputs"][side]["geq_db"][0],
                json!(-2.5)
            );
        }
        assert_eq!(
            draft.document["configuration"]["outputs"],
            original["configuration"]["outputs"]
        );
        assert!(rx.try_recv().is_err(), "field editing sends nothing");
        assert!(f.scene().in_bounds());
        for _ in 0..31 {
            f.key("I").unwrap();
            assert!(f.scene().in_bounds());
        }
        let body = f.structural_draft.as_ref().unwrap().body().unwrap();
        f.state.as_mut().unwrap().received = Instant::now();
        f.key("F4").unwrap();
        let request = rx.try_recv().unwrap();
        assert!(
            matches!(request.operation, Operation::ReviewStructure{kind,body:b} if kind=="pa_set" && b==body)
        );
        assert!(f.structural_draft.is_none());
    }
    #[test]
    fn master_eq_refuses_unquiesced_changed_or_stale_context_without_losing_draft() {
        for reason in [
            "scope",
            "stale",
            "revision",
            "generation",
            "epoch",
            "unquiesced",
            "raw",
            "authority",
            "changed-readback",
            "changed-map",
            "changed-scope",
        ] {
            let (mut f, rx) = master_surface();
            if reason == "scope" {
                f.scope = "foh".into();
                assert!(f.key("F11").is_err());
                continue;
            }
            f.key("F11").unwrap();
            let original = f.structural_draft.as_ref().unwrap().document.clone();
            let u = f.state.as_mut().unwrap();
            match reason {
                "stale" => u.structural_age_ms = Some(251),
                "revision" => u.structural.as_mut().unwrap().revision = "900".into(),
                "generation" => {
                    f.provider.generation.fetch_add(1, Ordering::AcqRel);
                }
                "epoch" => u.structural.as_mut().unwrap().epoch = "900".into(),
                "unquiesced" => u.structural.as_mut().unwrap().outputs_quiesced = false,
                "raw" => u.snapshot.as_mut().unwrap().authority.revision = "900".into(),
                "authority" => u.writer_lease_remaining_ms = Some(0),
                "changed-readback" => {
                    let current = u.structural.as_mut().unwrap();
                    let mut c: Value =
                        serde_json::from_str(current.pa_configuration_json.as_ref().unwrap())
                            .unwrap();
                    c["inputs"][0]["gain_db"] = json!(1.);
                    current.pa_configuration_json = Some(c.to_string());
                }
                "changed-map" => u.structural.as_mut().unwrap().pa_program_buses = vec![1, 0],
                "changed-scope" => f.scope = "output_routes".into(),
                _ => unreachable!(),
            }
            assert!(f.key("F4").is_err(), "{reason}");
            assert_eq!(f.structural_draft.as_ref().unwrap().document, original);
            assert!(rx.try_recv().is_err());
        }
    }
    #[test]
    fn master_eq_detached_text_survives_focus_loss_but_old_context_cannot_apply() {
        let (mut f, rx) = master_surface();
        f.key("F11").unwrap();
        f.key("F3").unwrap();
        f.key("t").unwrap();
        let mut fresh = f.state.clone().unwrap();
        f.enqueue(Event::Focus(false)).unwrap();
        assert_eq!(f.processing_entry, "t");
        assert!(f.structural_draft.as_ref().unwrap().master_eq.is_some());
        // Clear explicit text entry; neither focus recovery nor a fresh timestamp
        // restores the old generation's permission to replace PA settings.
        f.key("Esc").unwrap();
        fresh.received = Instant::now();
        f.state = Some(fresh);
        assert!(f.key("F4").is_err());
        assert!(
            !rx.try_iter()
                .any(|r| matches!(r.operation, Operation::ReviewStructure { .. }))
        );
    }
    #[test]
    fn master_eq_open_requires_quiescence_and_reconnect_never_replays_or_rearms() {
        let (mut f, rx) = master_surface();
        f.state
            .as_mut()
            .unwrap()
            .structural
            .as_mut()
            .unwrap()
            .outputs_quiesced = false;
        assert!(f.key("F11").is_err());
        assert!(f.structural_draft.is_none());
        f.state
            .as_mut()
            .unwrap()
            .structural
            .as_mut()
            .unwrap()
            .outputs_quiesced = true;
        f.key("F11").unwrap();
        f.key("I").unwrap();
        f.action(Action::StructureText("high_shelf".into()))
            .unwrap();
        let original = f.structural_draft.as_ref().unwrap().document.clone();
        let mut fresh = f.state.clone().unwrap();
        f.key("F5").unwrap();
        assert!(matches!(
            rx.try_recv().unwrap().operation,
            Operation::Reconnect
        ));
        fresh.received = Instant::now();
        f.state = Some(fresh);
        assert_eq!(f.structural_draft.as_ref().unwrap().document, original);
        assert!(f.key("F4").is_err());
        assert!(rx.try_recv().is_err());
        let mut after_cancel = f.state.clone().unwrap();
        f.key("Esc").unwrap();
        // Cancel may send Cancel, but it cannot send PA settings or rearm.
        assert!(
            rx.try_iter()
                .all(|r| matches!(r.operation, Operation::Cancel))
        );
        after_cancel.received = Instant::now();
        after_cancel.generation = f.provider.generation();
        after_cancel.attachment_generation = f.attachment_fence.unwrap();
        f.accept_update(after_cancel);
        f.key("F11").unwrap();
        f.key("F4").unwrap();
        assert!(
            matches!(rx.try_recv().unwrap().operation, Operation::ReviewStructure { kind, .. } if kind == "pa_set")
        );
        assert!(rx.try_recv().is_err(), "PA Apply never queues rearm");
    }
    #[test]
    fn master_eq_complete_owner_body_must_be_presented_before_confirmation() {
        let (mut f, rx) = master_surface();
        f.key("F11").unwrap();
        let body = f.structural_draft.as_ref().unwrap().body().unwrap();
        f.key("F4").unwrap();
        assert!(matches!(
            rx.try_recv().unwrap().operation,
            Operation::ReviewStructure { .. }
        ));
        // Match the existing operator's GP14 review format, including the exact
        // escaped owner JSON. Presentation must cover every character of it.
        let text =
            format!("pa_set {body} / revision 1 / scope pa_configuration / show test / epoch 9");
        f.state.as_mut().unwrap().review = Some((42, text.clone()));
        f.synchronize_review();
        assert!(f.review_pages() > 1);
        assert!(f.key("Enter").is_err());
        let mut recovered = String::new();
        for page in 0..f.review_pages() {
            f.state.as_mut().unwrap().received = Instant::now();
            for primitive in f.scene().primitives {
                if let Primitive::Text { y, value, .. } = primitive
                    && (96..864).contains(&y)
                {
                    recovered.push_str(&value);
                }
            }
            f.mark_presented();
            if page + 1 < f.review_pages() {
                assert!(f.key("Enter").is_err());
                f.key("PageDown").unwrap();
            }
        }
        assert_eq!(recovered, text);
        f.state.as_mut().unwrap().received = Instant::now();
        f.key("Enter").unwrap();
        assert!(matches!(
            rx.try_recv().unwrap().operation,
            Operation::Confirm(42)
        ));
        assert!(rx.try_recv().is_err());
    }
    pub(super) fn brain_surface() -> (Frontend, Receiver<Request>) {
        let mut f = surface();
        let (tx, rx) = mpsc::sync_channel(8);
        f.provider = Provider {
            tx,
            latest: Arc::new(Latest::default()),
            generation: Arc::new(AtomicU64::new(1)),
            stop: Arc::new(AtomicBool::new(false)),
            child: None,
            authorization: Arc::new(AtomicBool::new(true)),
            brain_signal: Arc::new(crate::brain::HoldSignal::default()),
        };
        f.module_config.wire_version = 2;
        f.brain_enabled = true;
        f.brain_page = true;
        let b =
            crate::brain::decode_reply(include_bytes!("../tests/fixtures/gp15/v1/hold-final.json"))
                .unwrap();
        let u = f.state.as_mut().unwrap();
        u.brain = b.snapshot;
        u.brain_fresh = true;
        u.brain_age_ms = Some(0);
        u.held_baseline_ready = true;
        u.held_transport_authenticated = true;
        u.writer_lease_remaining_ms = Some(2000);
        u.received = Instant::now();
        (f, rx)
    }
    #[test]
    fn brain_keyboard_controller_edges_and_same_batch_keyup_do_not_resurrect() {
        let (mut f, rx) = brain_surface();
        f.enqueue(Event::Key {
            key: "t".into(),
            pressed: true,
        })
        .unwrap();
        f.enqueue(Event::Key {
            key: "t".into(),
            pressed: false,
        })
        .unwrap();
        f.pump();
        assert!(!f.ptt_pressed);
        assert!(!f.provider.brain_signal.live());
        assert!(
            rx.try_iter()
                .all(|r| !matches!(r.operation, Operation::BrainPress(_)))
        );
        f.enqueue(Event::Key {
            key: "t".into(),
            pressed: true,
        })
        .unwrap();
        f.pump();
        let first = rx
            .try_iter()
            .find_map(|r| {
                if let Operation::BrainPress(id) = r.operation {
                    Some(id)
                } else {
                    None
                }
            })
            .unwrap();
        assert!(f.provider.brain_signal.live_id(first));
        f.enqueue(Event::Key {
            key: "t".into(),
            pressed: true,
        })
        .unwrap();
        f.pump();
        assert!(
            rx.try_iter()
                .all(|r| !matches!(r.operation, Operation::BrainPress(_)))
        );
        f.enqueue(Event::Key {
            key: "t".into(),
            pressed: false,
        })
        .unwrap();
        assert!(!f.provider.brain_signal.live_id(first));
        f.pump();
        f.inject_controller(Action::TalkbackPress).unwrap();
        f.pump();
        let second = rx
            .try_iter()
            .find_map(|r| {
                if let Operation::BrainPress(id) = r.operation {
                    Some(id)
                } else {
                    None
                }
            })
            .unwrap();
        assert_ne!(first, second);
        assert!(!f.provider.brain_signal.live_id(first));
        f.inject_controller(Action::TalkbackRelease).unwrap();
        assert!(!f.provider.brain_signal.live());
        f.controller_removed();
        assert!(!f.ptt_pressed);
    }
    #[test]
    fn brain_focus_loss_and_lost_ui_pump_close_without_waiting_for_queue() {
        let (mut f, _rx) = brain_surface();
        f.action(Action::TalkbackPress).unwrap();
        assert!(f.ptt_pressed);
        f.enqueue(Event::Focus(false)).unwrap();
        assert!(!f.provider.brain_signal.live());
        assert!(!f.ptt_pressed);
        assert!(f.action(Action::TalkbackPress).is_err());
        let (mut f, _rx) = brain_surface();
        f.action(Action::TalkbackPress).unwrap();
        std::thread::sleep(Duration::from_millis(105));
        f.pump();
        assert!(!f.provider.brain_signal.live());
    }
    #[test]
    fn every_armed_monitor_action_pins_the_ui_device_before_queueing() {
        for action in [
            Action::BrainArm,
            Action::BrainGain(-1800),
            Action::BrainDim,
            Action::BrainMute,
        ] {
            let (mut f, rx) = brain_surface();
            let crate::brain_device::Message::Snapshot(d) = crate::brain_device::decode(
                include_bytes!("../tests/fixtures/gp15/device-v1/snapshot-unarmed.json"),
            )
            .unwrap() else {
                panic!()
            };
            let pin = crate::audio::MonitorDevicePin::capture(Some(&d)).unwrap();
            let u = f.state.as_mut().unwrap();
            u.device = Some(d);
            u.device_fresh = false; // Worker refreshes; the queued identity must not change.
            u.brain.as_mut().unwrap().monitor_armed = true;
            f.action(action).unwrap();
            let request = rx.try_recv().unwrap();
            let Operation::ReviewBrain { body, device, .. } = request.operation else {
                panic!()
            };
            assert_eq!(body["armed"], true);
            assert_eq!(device.as_deref(), Some(&pin));
            f.state
                .as_mut()
                .unwrap()
                .device
                .as_mut()
                .unwrap()
                .observation
                .as_mut()
                .unwrap()
                .brain_epoch += 1;
            assert_ne!(
                device.as_deref(),
                Some(
                    &crate::audio::MonitorDevicePin::capture(
                        f.state.as_ref().unwrap().device.as_ref()
                    )
                    .unwrap()
                )
            );
        }
        let (mut f, rx) = brain_surface();
        assert!(f.action(Action::BrainArm).is_err());
        assert!(rx.try_recv().is_err());
    }
    #[test]
    fn device_restart_and_map_change_retain_content_but_require_new_identity_review() {
        for change_epoch in [true, false] {
            let (mut f, rx) = brain_surface();
            let crate::brain_device::Message::Snapshot(d) = crate::brain_device::decode(
                include_bytes!("../tests/fixtures/gp15/device-v1/snapshot-unarmed.json"),
            )
            .unwrap() else {
                panic!()
            };
            let u = f.state.as_mut().unwrap();
            u.device = Some(d);
            u.device_fresh = true;
            f.action(Action::DeviceEdit).unwrap();
            let retained_endpoint = f.device_draft.as_ref().unwrap().endpoint.clone();
            let o = f
                .state
                .as_mut()
                .unwrap()
                .device
                .as_mut()
                .unwrap()
                .observation
                .as_mut()
                .unwrap();
            if change_epoch {
                o.brain_epoch += 1;
            } else {
                o.brain_map += 1;
            }
            o.config.endpoint = "fake:replacement".into();
            assert!(
                f.action(Action::DeviceApply)
                    .unwrap_err()
                    .contains("context changed")
            );
            assert!(rx.try_recv().is_err());
            f.pump();
            assert_eq!(f.device_draft.as_ref().unwrap().endpoint, retained_endpoint);
            f.action(Action::DeviceEdit).unwrap();
            assert_eq!(f.device_draft.as_ref().unwrap().endpoint, retained_endpoint);
            f.action(Action::DeviceApply).unwrap();
            assert!(
                rx.try_iter()
                    .any(|r| matches!(r.operation, Operation::ReviewDevice(..)))
            );
        }
    }
    #[test]
    fn brain_actual_snapshot_scene_and_device_draft_are_truthful_and_fit() {
        let (mut f, rx) = brain_surface();
        let crate::brain_device::Message::Snapshot(mut d) = crate::brain_device::decode(
            include_bytes!("../tests/fixtures/gp15/device-v1/snapshot-unarmed.json"),
        )
        .unwrap() else {
            panic!()
        };
        d.observation.as_mut().unwrap().status["capture_queue_dropped"] = serde_json::json!(17);
        let u = f.state.as_mut().unwrap();
        u.device = Some(d);
        u.device_fresh = true;
        u.brain_fresh = false;
        u.fresh = false;
        u.brain_age_ms = Some(7);
        u.held_status = Some(crate::held_proof::Status {
            generation: Some("1".into()),
            source_frame: "480".into(),
            revision: "9".into(),
            talkback_path_ready: true,
            media_authorized: true,
            observed: Instant::now(),
            valid_until: Instant::now() + Duration::from_millis(50),
        });
        let scene = f.scene();
        assert!(scene.primitives.iter().any(
            |p| matches!(p,Primitive::Text{value,..} if value.contains("compact true / ready true"))
        ));
        assert!(
            scene.primitives.iter().any(
                |p| matches!(p,Primitive::Text{value,..} if value.contains("observation STALE"))
            )
        );
        assert!(scene.primitives.iter().any(|p| matches!(p,Primitive::Text{value,..} if value.contains("Sample peaks") && value.contains("age"))));
        f.state.as_mut().unwrap().fresh = true;

        assert!(scene.primitives.iter().any(|p|matches!(p,Primitive::Text{value,..}if value.contains("ratio")&&value.contains("ppb"))));
        assert!(scene.primitives.iter().any(
            |p| matches!(p, Primitive::Text { value, .. } if value.contains("capture drops 17"))
        ));
        f.state
            .as_mut()
            .unwrap()
            .device
            .as_mut()
            .unwrap()
            .observation
            .as_mut()
            .unwrap()
            .status
            .as_object_mut()
            .unwrap()
            .remove("capture_queue_dropped");
        assert!(f.scene().primitives.iter().any(
            |p| matches!(p, Primitive::Text { value, .. } if value.contains("capture drops --"))
        ));
        f.action(Action::DeviceEdit).unwrap();
        assert_eq!(f.device_draft.as_ref().unwrap().endpoint, "fake:brain");
        assert!(rx.try_recv().is_err());
        let text = f.device_draft.as_ref().unwrap().review().unwrap();
        f.action(Action::DeviceText(text)).unwrap();
        f.action(Action::DeviceApply).unwrap();
        assert!(
            rx.try_iter()
                .any(|r| matches!(r.operation, Operation::ReviewDevice(..)))
        );
        #[cfg(feature = "native")]
        if std::env::var_os("VK_DRIVER_FILES").is_some() {
            for (w, h) in [(1920, 1080), (960, 540), (540, 960), (3840, 2160)] {
                crate::native::offscreen_at(&scene, w, h).unwrap();
            }
        }
    }
    fn surface() -> Frontend {
        let mut f = Frontend::new(Config {
            wire_version: 1,
            remote: None,
            endpoint: "/nonexistent/gp07-layout.sock".into(),
            show: "11111111-1111-4111-8111-111111111111".into(),
            epoch: 9,
            writer: "gp07-layout".into(),
            scope: "foh".into(),
        });
        let corpus: Value =
            serde_json::from_str(include_str!("../tests/fixtures/gp03/v1/e03-rendered.json"))
                .unwrap();
        let mut raw =
            crate::audio::decode_snapshot(&serde_json::to_vec(&corpus["initial"]).unwrap())
                .unwrap();
        raw.authority.revision = "0".into();
        let processing = crate::processing::decode_reply(include_bytes!(
            "../tests/fixtures/gp07/v2/snapshot-reply.json"
        ))
        .unwrap()
        .snapshot;
        f.state = Some(Update {
            device: None,
            device_final: None,
            device_fresh: false,
            device_age_ms: Some(0),
            brain: None,
            brain_final: None,
            brain_fresh: false,
            brain_age_ms: None,
            held_status: None,
            held_baseline_ready: false,
            held_transport_authenticated: false,
            brain_status: "disabled".into(),
            generation: 1,
            attachment_generation: 1,
            last_operation: None,
            writer_lease_remaining_ms: None,
            snapshot: Some(raw),
            live_eq: None,
            live_eq_age_ms: None,
            live_eq_final: None,
            sends: None,
            sends_age_ms: None,
            sends_final: None,
            processing,
            processing_age_ms: Some(0),
            processing_status: "fixture layout only".into(),
            structural: None,
            structural_final: None,
            structural_fresh: false,
            structural_age_ms: Some(0),
            processing_final: None,
            fresh: true,
            snapshot_age_ms: Some(0),
            status: "fixture layout only".into(),
            review: None,
            received: Instant::now(),
        });
        f.page = Page::Channel;
        f
    }
    #[test]
    fn detached_content_survives_focus_resize_disconnect_and_revision_changes() {
        let (mut f, rx) = brain_surface();
        f.brain_page = false;
        f.action(Action::ProcessingEdit).unwrap();
        f.processing_entry = "6.".into();
        let config = f.processing_draft.as_ref().unwrap().config.clone();
        let snapshot = crate::structure::decode_snapshot(include_bytes!(
            "../tests/fixtures/gp14/v1/structure-16.json"
        ))
        .unwrap();
        f.structural_draft =
            Some(crate::structure::Draft::new(&snapshot, "output_routes", 1).unwrap());
        let document = f.structural_draft.as_ref().unwrap().document.clone();
        let crate::brain_device::Message::Snapshot(device) = crate::brain_device::decode(
            include_bytes!("../tests/fixtures/gp15/device-v1/snapshot-unarmed.json"),
        )
        .unwrap() else {
            panic!()
        };
        f.device_draft = Some(device.observation.unwrap().config);
        f.device_entry = Some("{unfinished".into());
        let device_config = f.device_draft.clone();
        for event in [
            Event::Resize(1000, 700),
            Event::Focus(false),
            Event::Focus(true),
            Event::Resize(0, 0),
            Event::DeviceLost,
        ] {
            f.enqueue(event).unwrap();
            assert_eq!(f.processing_draft.as_ref().unwrap().config, config);
            assert_eq!(f.structural_draft.as_ref().unwrap().document, document);
            assert_eq!(f.device_draft, device_config);
            assert_eq!(f.processing_entry, "6.");
            assert_eq!(f.device_entry.as_deref(), Some("{unfinished"));
        }
        assert!(rx.try_recv().is_err());
        assert!(f.action(Action::ProcessingApply).is_err());
        assert!(f.action(Action::StructureApply).is_err());
        assert!(f.action(Action::DeviceApply).is_err());
        f.key("F5").unwrap();
        assert!(matches!(
            rx.try_recv().unwrap().operation,
            Operation::Reconnect
        ));
        assert!(rx.try_recv().is_err());
        let mut update = surface().state.unwrap();
        update.generation = f.provider.generation();
        update.attachment_generation = f.provider.generation();
        update.snapshot.as_mut().unwrap().authority.revision = "5".into();
        update.processing.as_mut().unwrap().revision = "5".into();
        f.accept_update(update);
        f.pump();
        assert_eq!(f.processing_draft.as_ref().unwrap().config, config);
        assert_eq!(f.structural_draft.as_ref().unwrap().document, document);
        assert_eq!(f.device_draft, device_config);
        assert!(f.state.as_ref().unwrap().review.is_none());
        assert!(
            rx.try_recv().is_err(),
            "no automatic submission after recovery"
        );
        f.action(Action::Cancel).unwrap();
        assert!(f.processing_draft.is_none());
        assert!(f.structural_draft.is_none());
        assert!(f.device_draft.is_none());
        assert!(f.device_entry.is_none());
        assert!(f.processing_entry.is_empty());
        assert!(matches!(
            rx.try_recv().unwrap().operation,
            Operation::Cancel
        ));
    }
    #[test]
    fn retained_processing_draft_recovery_requests_new_review_only() {
        let (mut f, rx) = brain_surface();
        f.brain_page = false;
        f.action(Action::ProcessingEdit).unwrap();
        f.action(Action::ProcessingField(2)).unwrap();
        f.action(Action::ProcessingText("6.1".into())).unwrap();
        let config = f.processing_draft.as_ref().unwrap().config.clone();
        let mut update = f.state.as_ref().unwrap().clone();
        f.enqueue(Event::Focus(false)).unwrap();
        assert!(f.action(Action::ProcessingApply).is_err());
        f.enqueue(Event::Focus(true)).unwrap();
        update.generation = f.provider.generation();
        update.received = Instant::now();
        update.snapshot.as_mut().unwrap().authority.revision = "5".into();
        update.processing.as_mut().unwrap().revision = "5".into();
        update.review = None;
        f.accept_update(update);
        assert!(rx.try_recv().is_err());
        f.action(Action::ProcessingApply).unwrap();
        let request = rx.try_recv().unwrap();
        assert_eq!(request.revision.as_deref(), Some("5"));
        assert!(
            matches!(request.operation, Operation::ReviewProcessing { config: sent, .. } if sent == config)
        );
        assert!(
            rx.try_recv().is_err(),
            "Apply requests review, never confirmation"
        );
    }
    #[test]
    fn publication_preserves_raw_brain_device_and_structure_observation_lifetimes() {
        let (mut f, _) = brain_surface();
        let crate::brain_device::Message::Snapshot(device) = crate::brain_device::decode(
            include_bytes!("../tests/fixtures/gp15/device-v1/snapshot-unarmed.json"),
        )
        .unwrap() else {
            panic!()
        };
        let u = f.state.as_mut().unwrap();
        u.device = Some(device);
        u.device_fresh = true;
        u.structural_fresh = true;
        u.snapshot_age_ms = Some(240);
        u.brain_age_ms = Some(240);
        u.device_age_ms = Some(240);
        u.structural_age_ms = Some(240);
        u.received = Instant::now();
        assert!(
            u.raw_fresh() && u.brain_is_fresh() && u.device_is_fresh() && u.structure_is_fresh()
        );
        u.received = Instant::now() - Duration::from_millis(20);
        assert!(
            !u.raw_fresh()
                && !u.brain_is_fresh()
                && !u.device_is_fresh()
                && !u.structure_is_fresh()
        );
        // A younger raw observation cannot extend older independent observations.
        u.snapshot_age_ms = Some(0);
        assert!(u.raw_fresh());
        assert!(!u.brain_is_fresh() && !u.device_is_fresh() && !u.structure_is_fresh());
        assert!(f.fresh());
        let text = f
            .scene()
            .primitives
            .into_iter()
            .filter_map(|p| match p {
                Primitive::Text { value, .. } => Some(value),
                _ => None,
            })
            .collect::<Vec<_>>()
            .join("\n");
        assert!(text.contains("observation STALE"));
        assert!(
            text.lines()
                .any(|line| line.starts_with("DEVICE ") && line.contains(" / STALE / "))
        );
    }
    #[test]
    fn confirmed_grant_survives_health_coalescing_but_expiry_and_disconnect_revoke_it() {
        struct NoIo;
        impl crate::local_audio::AuthorityConnection for NoIo {
            fn send_frame_until(&mut self, _: &[u8], _: Instant) -> Result<(), String> {
                panic!("no I/O")
            }
            fn receive_until(&mut self, _: Instant) -> Result<Option<Vec<u8>>, String> {
                panic!("no I/O")
            }
            fn receive_available(&mut self) -> Result<Option<Vec<u8>>, String> {
                panic!("no I/O")
            }
        }
        let corpus: Value =
            serde_json::from_str(include_str!("../tests/fixtures/gp03/v1/e03-rendered.json"))
                .unwrap();
        let mut operator = Operator::from_connection(
            Box::new(NoIo),
            "11111111-1111-4111-8111-111111111111",
            9,
            "desk-corpus",
            "foh",
        )
        .unwrap();
        operator
            .session
            .ingest_snapshot(
                crate::audio::decode_snapshot(&serde_json::to_vec(&corpus["initial"]).unwrap())
                    .unwrap(),
                0,
            )
            .unwrap();
        assert!(confirmed_lease_remaining(&operator).is_none());
        operator
            .session
            .begin("grant", json!({"scope":"foh"}), 0)
            .unwrap();
        assert!(confirmed_lease_remaining(&operator).is_none());
        operator
            .session
            .accept(
                crate::audio::decode_reply(&serde_json::to_vec(&corpus["grant_response"]).unwrap())
                    .unwrap(),
                1,
            )
            .unwrap();
        let mut update = surface().state.take().unwrap();
        update.last_operation = Some(operator.session.last_result.clone());
        update.writer_lease_remaining_ms = confirmed_lease_remaining(&operator);
        update.status = "Structural state unavailable: bounded poll failed".into();
        update.fresh = false;
        let latest = Latest::default();
        publish_provider_update(&latest, update, None);
        let mut final_update = latest.update.lock().unwrap().take().unwrap();
        assert!(
            final_update
                .last_operation
                .as_ref()
                .unwrap()
                .starts_with("grant applied")
        );
        assert!(final_update.status.contains("poll failed"));
        assert!(!final_update.fresh);
        assert!(final_update.writer_granted());
        final_update.received = Instant::now() - Duration::from_secs(2);
        assert!(!final_update.writer_granted());
        operator.session.disconnect();
        assert!(confirmed_lease_remaining(&operator).is_none());
    }
    #[test]
    fn final_worker_update_keeps_stage_error_after_failed_health_poll() {
        let f = surface();
        let latest = Latest::default();
        let mut update = f.state.as_ref().unwrap().clone();
        update.status = "PENDING structural review".into();
        publish_provider_update(&latest, update.clone(), None);
        let operation_error = "REFUSED/UNCERTAIN: staged context changed";
        update.status = "Structural state unavailable: structural snapshot deadline".into();
        update.fresh = false;
        update.structural_fresh = false;
        // Read only the final coalesced update, as a slow frontend would.
        publish_provider_update(&latest, update.clone(), Some(operation_error));
        let final_update = latest.update.lock().unwrap().take().unwrap();
        assert_eq!(final_update.status, operation_error);
        assert!(!final_update.fresh);
        assert!(!final_update.structural_fresh);
        publish_provider_update(&latest, update, None); // next explicit operation clears sticky error
        assert!(
            latest
                .update
                .lock()
                .unwrap()
                .as_ref()
                .unwrap()
                .status
                .starts_with("Structural state unavailable")
        );
    }
    #[test]
    fn batched_raw_keys_apply_editor_mode_at_dispatch() {
        let snapshot = crate::structure::decode_snapshot(include_bytes!(
            "../tests/fixtures/gp14/v1/structure-16.json"
        ))
        .unwrap();
        let mut f = surface();
        let (tx, _rx) = mpsc::sync_channel(8);
        f.provider = Provider {
            tx,
            latest: Arc::new(Latest::default()),
            generation: Arc::new(AtomicU64::new(1)),
            stop: Arc::new(AtomicBool::new(false)),
            child: None,
            authorization: Arc::new(AtomicBool::new(true)),
            brain_signal: Arc::new(crate::brain::HoldSignal::default()),
        };
        let state = f.state.as_mut().unwrap();
        state.structural_fresh = true;
        state.snapshot.as_mut().unwrap().authority.revision = snapshot.revision.clone();
        state.received = Instant::now();
        f.structural_draft =
            Some(crate::structure::Draft::new(&snapshot, "pa_configuration", 1).unwrap());
        let draft = f.structural_draft.as_mut().unwrap();
        draft.selected = draft
            .fields
            .iter()
            .position(|p| p == "/configuration/outputs/0/source")
            .unwrap();
        let selected = draft.selected;
        let mut keys = vec!["F3".to_string()];
        keys.extend(r#"{"node":0}"#.chars().map(|c| c.to_string()));
        keys.extend(["Enter".into(), "i".into()]);
        for key in keys {
            for pressed in [true, false] {
                f.enqueue(Event::Key {
                    key: key.clone(),
                    pressed,
                })
                .unwrap();
            }
        }
        f.pump();
        let draft = f.structural_draft.as_ref().unwrap();
        assert_eq!(
            draft.document["configuration"]["outputs"][0]["source"],
            json!({"node":0})
        );
        assert_eq!(
            draft.selected,
            selected + 1,
            "lowercase shortcut after Enter normalizes at dispatch"
        );
        assert!(!f.structure_text_entry);
    }
    #[test]
    fn structural_json_keyboard_and_semantic_import_preserve_complete_owner_intent() {
        let snapshot = crate::structure::decode_snapshot(include_bytes!(
            "../tests/fixtures/gp14/v1/structure-16.json"
        ))
        .unwrap();
        let mut f = surface();
        f.structural_draft =
            Some(crate::structure::Draft::new(&snapshot, "pa_configuration", 1).unwrap());
        let path = "/configuration/outputs/0/source";
        let draft = f.structural_draft.as_mut().unwrap();
        draft.selected = draft.fields.iter().position(|p| p == path).unwrap();
        f.key("F3").unwrap();
        for c in r#"{"node":0}"#.chars() {
            f.key(&c.to_string()).unwrap();
        }
        f.key("Enter").unwrap();
        assert!(!f.structure_text_entry);
        assert_eq!(
            f.structural_draft
                .as_ref()
                .unwrap()
                .document
                .pointer(path)
                .unwrap(),
            &json!({"node":0})
        );
        let document =
            serde_json::to_string(&f.structural_draft.as_ref().unwrap().document).unwrap();
        f.action(Action::StructureImport(document)).unwrap();
        assert!(f.scene().in_bounds());
        f.key("F3").unwrap();
        f.key("Q").unwrap(); // text, never a writer release
        assert_eq!(f.processing_entry, "Q");
        f.key("Esc").unwrap();
        assert!(f.structural_draft.is_some());
        assert!(f.processing_entry.is_empty());
    }
    #[test]
    fn processing_selection_uses_identity_after_independent_inventory_reorder() {
        let mut f = surface();
        let u = f.state.as_mut().unwrap();
        u.snapshot.as_mut().unwrap().authority.inputs.swap(0, 7);
        u.processing.as_mut().unwrap().channels.reverse();
        f.key("E").unwrap();
        assert_eq!(f.processing_draft.as_ref().unwrap().input, "input-08");
        assert_eq!(f.target("fader").unwrap()["input"], "input-08");
        // Removing the selected processing identity must not edit another channel.
        f.processing_draft = None;
        f.state
            .as_mut()
            .unwrap()
            .processing
            .as_mut()
            .unwrap()
            .channels
            .retain(|c| c.input != "input-08");
        assert!(f.key("E").is_err());
    }
    #[test]
    fn changed_inventory_preserves_identity_and_revokes_queued_edits() {
        let mut f = surface();
        f.selected = 7;
        f.key("E").unwrap();
        let mut update = f.state.as_ref().unwrap().clone();
        update.snapshot.as_mut().unwrap().authority.inputs.reverse();
        f.inject_controller(Action::ProcessingAdjust(1)).unwrap();
        let generation = f.provider.generation();
        f.accept_update(update.clone());
        assert_eq!(f.selected, 0);
        assert_eq!(f.processing_draft.as_ref().unwrap().input, "input-08");
        assert!(f.queue.is_empty());
        assert!(f.state.is_none());
        assert!(f.provider.generation() > generation);
        update.generation = f.provider.generation();
        f.accept_update(update);
        assert_eq!(f.selected_input(), Some("input-08"));
    }
    #[test]
    fn monitor_scope_resolution_never_aliases_an_unknown_scope() {
        let mut f = surface();
        f.scope = "monitor3".into();
        assert!(f.target("fader").is_err());
        let authority = &mut f
            .state
            .as_mut()
            .unwrap()
            .snapshot
            .as_mut()
            .unwrap()
            .authority;
        // Consumer model test only; legacy wire fixtures retain their exact shape.
        authority.monitors.push("monitor-3".into());
        authority.modes.push(("monitor3".into(), "manual".into()));
        assert_eq!(f.target("fader").unwrap()["monitor"], "monitor-3");
        assert!(f.target("pan").is_err());
        f.scope = "pa".into();
        assert!(f.target("fader").is_err());
        assert!(f.target("mute").is_err());
    }
    #[test]
    fn partial_bank_navigation_wraps_banks_without_skipping_the_first_strip() {
        let mut f = surface();
        let mut update = f.state.as_ref().unwrap().clone();
        update.snapshot.as_mut().unwrap().authority.inputs =
            (1..=49).map(|n| format!("strip-{n}")).collect();
        for expected in [12, 24, 36, 48, 0] {
            f.state = Some(update.clone());
            f.action(Action::Bank(1)).unwrap();
            assert_eq!(f.selected, expected);
        }
        f.state = Some(update.clone());
        f.action(Action::Bank(-1)).unwrap();
        assert_eq!(f.selected, 48);
        f.state = Some(update);
        f.action(Action::Bank(i32::MAX)).unwrap();
        assert!(f.selected < 49);
    }
    #[test]
    fn high_channel_selection_and_banking_are_presentation_dimensions() {
        for count in [16, 17, 32, 33, 48, 49] {
            let mut f = surface();
            f.state
                .as_mut()
                .unwrap()
                .snapshot
                .as_mut()
                .unwrap()
                .authority
                .inputs = (1..=count).map(|n| format!("strip-{n}")).collect();
            for number in [16, 17, 32, 33, 48].into_iter().filter(|n| *n <= count) {
                f.selected = number - 1;
                assert_eq!(
                    f.target("fader").unwrap()["input"],
                    format!("strip-{number}")
                );
                f.page = Page::Mix;
                assert!(f.scene().in_bounds());
                assert!(f.scene().primitives.iter().any(|p| matches!(p,
                    Primitive::Text {value, ..} if value.starts_with(&format!("> strip-{number} ")))));
            }
            f.selected = count - 1;
            f.action(Action::Move(1)).unwrap();
            assert_eq!(f.selected, 0);
        }
    }
    #[test]
    fn processing_keyboard_and_controller_semantics_edit_same_detached_draft() {
        let mut keyboard = surface();
        let mut controller = surface();
        keyboard.key("E").unwrap();
        controller.action(Action::ProcessingEdit).unwrap();
        keyboard.key("I").unwrap();
        keyboard.key("I").unwrap();
        controller.action(Action::ProcessingField(2)).unwrap();
        keyboard.key("6").unwrap();
        keyboard.key(".").unwrap();
        keyboard.key("1").unwrap();
        keyboard.key("Enter").unwrap();
        controller
            .action(Action::ProcessingText("6.1".into()))
            .unwrap();
        assert_eq!(
            keyboard.processing_draft.as_ref().unwrap().config,
            controller.processing_draft.as_ref().unwrap().config
        );
        assert_eq!(
            keyboard.state.as_ref().unwrap().processing,
            controller.state.as_ref().unwrap().processing
        );
        assert!(keyboard.scene().in_bounds());
        let lines: Vec<_> = keyboard
            .scene()
            .primitives
            .into_iter()
            .filter_map(|p| {
                if let Primitive::Text { value, .. } = p {
                    Some(value)
                } else {
                    None
                }
            })
            .collect();
        assert!(lines.iter().any(|s| s.contains("LOCAL DRAFT")));
        assert!(lines.iter().any(|s| s.contains("Band 1 gain +6.1 dB")));
        keyboard.key("Right").unwrap();
        assert!(keyboard.processing_draft.is_some());
        assert!(keyboard.state.is_none());
        assert!(keyboard.action(Action::ProcessingApply).is_err());
    }
    #[test]
    fn navigation_release_is_not_lost_to_generation_synchronization() {
        let mut f = surface();
        f.pressed.insert("Right".into());
        f.key("Right").unwrap();
        assert!(f.blocked.contains("Right"));
        assert_eq!(f.observed_generation, f.provider.generation());
        f.enqueue(Event::Key {
            key: "Right".into(),
            pressed: false,
        })
        .unwrap();
        f.pump();
        assert!(!f.blocked.contains("Right"));
    }
    #[test]
    fn processing_stale_role_loss_and_review_fences_disable_edits() {
        let mut f = surface();
        f.state.as_mut().unwrap().processing_age_ms = Some(251);
        assert!(f.key("E").is_err());
        f.state.as_mut().unwrap().processing_age_ms = Some(0);
        f.key("E").unwrap();
        f.key("K").unwrap();
        let retained = f.processing_draft.as_ref().unwrap().config.clone();
        f.processing_entry = "6.".into();
        f.enqueue(Event::Focus(false)).unwrap();
        assert_eq!(f.processing_draft.as_ref().unwrap().config, retained);
        assert_eq!(f.processing_entry, "6.");
        assert!(f.action(Action::ProcessingApply).is_err());
        let mut f = surface();
        f.require_role();
        assert!(f.key("E").is_err());
        let mut f = surface();
        f.state.as_mut().unwrap().review = Some((1, "Processing review".into()));
        f.synchronize_review();
        assert!(f.key("E").is_err());
        assert!(f.key("Enter").unwrap_err().contains("every displayed"));
        f.fence();
        assert!(f.key("Enter").is_err());
    }
    #[test]
    fn four_band_extreme_values_all_endpoints_fit_without_truncation_or_overlap() {
        let mut f = surface();
        let p = f.state.as_mut().unwrap().processing.as_mut().unwrap();
        let mut value = serde_json::to_value(&p.channels[0].target).unwrap();
        for band in 1..=4 {
            value[format!("band{band}_hz")] = json!(20000);
            value[format!("band{band}_gain_mdb")] = json!(-12000);
            value[format!("band{band}_q_milli")] = json!(10000);
            value[format!("band{band}_bypass")] = json!(true);
        }
        p.channels[0].current = crate::processing::decode_config(&value).unwrap();
        p.channels[0].target = p.channels[0].current.clone();
        f.key("E").unwrap();
        for i in 0..24 {
            assert_eq!(f.processing_field, i);
            let scene = f.scene();
            let mut rows = Vec::new();
            for primitive in scene.primitives {
                if let Primitive::Text { x, y, value, .. } = primitive {
                    assert!(x + value.chars().count() as u32 * 12 <= 1920, "{value}");
                    if (312..600).contains(&y) {
                        rows.push((y, value));
                    }
                }
            }
            assert_eq!(rows.len(), 12);
            for (band, (_, row)) in rows.iter().enumerate().take(5).skip(1) {
                let expected = format!("Band {band} bell 20000Hz -12.0dB Q10.0 BYPASS");
                assert_eq!(
                    row.matches(&expected).count(),
                    3,
                    "all three endpoints must be fully visible: {row}"
                );
            }
            assert!(rows.windows(2).all(|r| r[1].0 >= r[0].0 + 24));
            f.key("I").unwrap();
        }
        assert_eq!(f.processing_field, 0);
    }
    #[test]
    fn zero_size_review_never_counts_as_presented_and_all24_values_are_reviewable() {
        let mut f = surface();
        let config = f
            .state
            .as_ref()
            .unwrap()
            .processing
            .as_ref()
            .unwrap()
            .channels[0]
            .target
            .clone();
        let fields = crate::processing::FIELDS
            .iter()
            .map(|field| config.display(*field))
            .collect::<Vec<_>>();
        f.state.as_mut().unwrap().review = Some((10, fields.join("\n")));
        f.synchronize_review();
        f.width = 0;
        let _ = crate::raster::rgba(&f.scene());
        f.mark_presented();
        assert!(f.review_seen.is_empty());
        assert!(f.action(Action::Confirm).is_err());
        f.width = 540;
        f.height = 960;
        let scene = f.scene();
        for field in fields {
            assert!(
                scene
                    .primitives
                    .iter()
                    .any(|p| matches!(p,Primitive::Text{value,..} if value==&field)),
                "{field}"
            );
        }
        let _ = crate::raster::rgba(&scene);
        f.state.as_mut().unwrap().received = Instant::now();
        f.mark_presented();
        assert_eq!(f.review_seen.len(), 1);
    }
    #[test]
    fn injected_local_field_burst_does_not_fill_provider_release_queue() {
        let mut f = surface();
        let (tx, rx) = mpsc::sync_channel(8);
        f.provider = Provider {
            tx,
            latest: Arc::new(Latest::default()),
            generation: Arc::new(AtomicU64::new(1)),
            stop: Arc::new(AtomicBool::new(false)),
            child: None,
            authorization: Arc::new(AtomicBool::new(true)),
            brain_signal: Arc::new(crate::brain::HoldSignal::default()),
        };
        f.state.as_mut().unwrap().received = Instant::now();
        f.inject_controller(Action::ProcessingEdit).unwrap();
        f.pump();
        assert!(matches!(
            rx.try_recv().unwrap().operation,
            Operation::InputReleased
        ));
        for _ in 0..24 {
            f.inject_controller(Action::ProcessingField(1)).unwrap();
            f.pump();
        }
        f.inject_controller(Action::ProcessingText("0".into()))
            .unwrap();
        f.pump();
        assert!(
            rx.try_recv().is_err(),
            "local fields must not send provider traffic"
        );
        assert!(!f.processing_draft.as_ref().unwrap().config.eq_bypass);
        f.inject_controller(Action::ProcessingApply).unwrap();
        f.pump();
        assert!(matches!(
            rx.try_recv().unwrap().operation,
            Operation::ReviewProcessing { .. }
        ));
        assert!(matches!(
            rx.try_recv().unwrap().operation,
            Operation::InputReleased
        ));
        assert!(rx.try_recv().is_err());
    }
}

impl Frontend {
    fn send_pair(&self) -> Result<(String, String), String> {
        if !self.fresh() {
            return Err("fresh actual raw inventory required".into());
        }
        let raw = self.state.as_ref().unwrap().snapshot.as_ref().unwrap();
        Ok((
            raw.authority
                .inputs
                .get(self.selected)
                .ok_or("selected input unavailable")?
                .clone(),
            raw.authority
                .monitors
                .get(self.selected_monitor)
                .ok_or("selected monitor unavailable")?
                .clone(),
        ))
    }
    pub fn sends_ready(&self) -> bool {
        self.fresh()
            && self.state.as_ref().is_some_and(|u| {
                u.sends_age_ms.is_some_and(|age| {
                    age.saturating_add(u.received.elapsed().as_millis() as u64) <= 250
                }) && u.sends.as_ref().is_some_and(|s| {
                    !s.faulted
                        && s.channels.iter().flat_map(|c| &c.sends).all(|s| s.ready)
                        && u.snapshot.as_ref().is_some_and(|r| {
                            r.authority.revision == s.revision && r.authority.monitors == s.monitors
                        })
                })
            })
    }
}

impl Frontend {
    fn sends_scene(&self) -> Scene {
        let mut s = Scene::default();
        s.primitives.push(Primitive::Rect {
            x: 0,
            y: 0,
            w: 1920,
            h: 1080,
            fill: "#10151d",
        });
        let mut line = |y: u32, v: String, color: &'static str| {
            s.primitives.push(Primitive::Text {
                x: 24,
                y,
                value: v.chars().take(156).collect(),
                color,
            });
        };
        let cyan = "#66dfd3";
        let dim = "#9caebc";
        let white = "#e4e8e9";
        let amber = "#f1bd6b";
        line(
            12,
            format!(
                "SENDS / {} / attachment scope {}",
                if self.sends_channel {
                    "PER CHANNEL"
                } else {
                    "OVERVIEW"
                },
                self.scope
            ),
            cyan,
        );
        line(
            48,
            format!(
                "RAW {} / TAPS {} / monitor{} / {}",
                if self.fresh() {
                    "fresh"
                } else {
                    "stale / last confirmed"
                },
                if self.sends_ready() {
                    "fresh + settled"
                } else {
                    "stale/unavailable/fading"
                },
                self.selected_monitor + 1,
                if self.state.as_ref().is_some_and(Update::writer_granted) {
                    "EXPLICIT WRITER GRANTED"
                } else {
                    "READ ONLY / G GRANT REQUIRED"
                }
            ),
            amber,
        );
        line(96,"Every tap obeys shared INPUT mute. Processed taps follow EQ+compression. Post-fader remains before pan.".into(),dim);
        line(132,"Send level and tap have SEPARATE reviews; no combined atomic operation; no trim or FX send here.".into(),dim);
        if let Some(raw) = self.state.as_ref().and_then(|u| u.snapshot.as_ref()) {
            line(
                180,
                format!(
                    "{} inputs / {} monitors / {} independent channel banks / selected {}",
                    raw.authority.inputs.len(),
                    raw.authority.monitors.len(),
                    raw.authority.inputs.len().div_ceil(12),
                    self.selected_input().unwrap_or("unavailable")
                ),
                cyan,
            );
            line(
                216,
                format!(
                    "{:<27}{:<23}{:<29}{}",
                    "PAIR",
                    "CURRENT / TARGET dB",
                    "HOLD / MODE",
                    "CURRENT TAP -> COMMITTED TARGET / TRANSITION"
                ),
                dim,
            );
            let rows: Vec<_> = if self.sends_channel {
                raw.authority
                    .monitors
                    .iter()
                    .skip(self.selected_monitor / 12 * 12)
                    .take(12)
                    .filter_map(|m| self.selected_input().map(|i| (i, m.as_str())))
                    .collect()
            } else {
                raw.authority
                    .inputs
                    .iter()
                    .skip(self.selected / 12 * 12)
                    .take(12)
                    .filter_map(|i| {
                        raw.authority
                            .monitors
                            .get(self.selected_monitor)
                            .map(|m| (i.as_str(), m.as_str()))
                    })
                    .collect()
            };
            for (n, (input, monitor)) in rows.into_iter().enumerate() {
                let p = raw.authority.parameters.iter().find(|p| {
                    p.target.input == input
                        && p.target.parameter == "send"
                        && p.target.monitor.as_deref() == Some(monitor)
                });
                let target = raw
                    .coefficients
                    .iter()
                    .find(|c| c.input == input)
                    .and_then(|c| {
                        raw.authority
                            .monitors
                            .iter()
                            .position(|m| m == monitor)
                            .and_then(|i| c.current_nanogain.get(i + 4))
                    });
                let tap = self
                    .state
                    .as_ref()
                    .and_then(|u| u.sends.as_ref())
                    .and_then(|s| s.send(input, monitor));
                let scope = format!("monitor{}", monitor.strip_prefix("monitor-").unwrap_or("?"));
                let mode = raw
                    .authority
                    .modes
                    .iter()
                    .find(|(s, _)| s == &scope)
                    .map(|(_, m)| m.as_str())
                    .unwrap_or("?");
                line(
                    264 + n as u32 * 36,
                    format!(
                        "{:<27}{:<23}{:<29}{}",
                        format!(
                            "{} {input} / {monitor}",
                            if Some(input) == self.selected_input()
                                && monitor == format!("monitor-{}", self.selected_monitor + 1)
                            {
                                ">"
                            } else {
                                " "
                            }
                        ),
                        format!(
                            "{} / {}",
                            target.map_or_else(|| "--".into(), |n| send_gain_display(*n)),
                            p.map_or_else(
                                || "--".into(),
                                |p| parameter_display("send", &p.target_value)
                            )
                        ),
                        format!(
                            "{} / {mode}",
                            p.and_then(|p| p.hold.as_ref())
                                .map_or_else(|| "none".into(), |h| h.to_string())
                        ),
                        tap.map_or_else(
                            || "GP18 unavailable / no tap invented".into(),
                            |t| format!(
                                "{} -> {} / {}f{}{}",
                                tap_display(t.current),
                                tap_display(t.target),
                                t.transition_remaining_frames,
                                if t.ready { " settled" } else { " fading" },
                                if self.sends_ready() {
                                    ""
                                } else {
                                    " / LAST CONFIRMED"
                                }
                            )
                        )
                    ),
                    white,
                );
            }
        } else {
            line(
                264,
                "Inventory unavailable; no fixture-derived capacity or authority".into(),
                amber,
            );
        }
        if let Some(u) = &self.state {
            line(
                732,
                format!(
                    "GP18 {} / raw age {} / tap age {} / operation {}",
                    if self.sends_ready() {
                        "settled"
                    } else {
                        "unavailable/stale/fading"
                    },
                    observation_age_display(u.snapshot_age_ms, u.received),
                    observation_age_display(u.sends_age_ms, u.received),
                    u.last_operation.as_deref().unwrap_or("none")
                ),
                amber,
            );
            if let Some(final_reply) = &u.sends_final {
                line(
                    768,
                    format!(
                        "Committed tap ticket {} / revision {} / effective frame {} / fade {}f",
                        final_reply.ticket.as_deref().unwrap_or("--"),
                        final_reply.revision,
                        final_reply.effective_frame.as_deref().unwrap_or("--"),
                        final_reply.ramp_frames.unwrap_or(0)
                    ),
                    amber,
                );
            }
        }
        line(
            816,
            self.send_draft.as_ref().map_or_else(
                || "LOCAL DRAFT: none".into(),
                |d| {
                    format!(
                        "LOCAL UNSENT {} / {} / {} / pinned revision {}",
                        d.input,
                        d.monitor,
                        match d.value {
                            SendDraftValue::Tap(t) => tap_display(t).into(),
                            SendDraftValue::Level(v) => parameter_display("send", &Value::from(v)),
                        },
                        d.revision
                    )
                },
            ),
            cyan,
        );
        if let Some(entry) = &self.send_entry {
            line(
                852,
                format!("Exact send LEVEL dB: {entry} / Enter accepts field; Esc cancels entry"),
                cyan,
            );
        }
        line(900,"F1 overview / F2 channel | arrows channel / PageUp/Down bank | U/I monitor browse (read only)".into(),dim);
        line(936,"O reattach selected monitor / F reattach FOH / V reattach PA | NEW G grant | Q release writer only".into(),dim);
        line(972,"E tap draft:1 raw /2 processed pre /3 processed post | S exact level | F4 Apply/review | Esc cancel".into(),dim);
        line(1032, self.message.clone(), "#f47c85");
        s
    }
}
impl Frontend {
    fn live_eq_observation_fresh(&self) -> bool {
        self.fresh()
            && self.state.as_ref().is_some_and(|u| {
                u.live_eq_age_ms.is_some_and(|age| {
                    age.saturating_add(u.received.elapsed().as_millis() as u64) <= 250
                }) && u.live_eq.as_ref().is_some_and(|s| {
                    u.snapshot.as_ref().is_some_and(|raw| {
                        raw.authority.revision == s.revision
                            && raw.topology.as_ref().is_some_and(|t| {
                                s.map_revision.parse::<u64>().ok() == Some(t.map_revision)
                            })
                    })
                })
            })
    }
    pub fn live_eq_ready(&self) -> bool {
        self.live_eq_observation_fresh()
            && self
                .state
                .as_ref()
                .and_then(|u| u.live_eq.as_ref())
                .is_some_and(crate::live_eq::Snapshot::editable)
    }
    fn live_scene(&self) -> Scene {
        let mut s = Scene::default();
        s.primitives.push(Primitive::Rect {
            x: 0,
            y: 0,
            w: 1920,
            h: 1080,
            fill: "#10151d",
        });
        let mut line = |y: u32, v: String, color: &'static str| {
            s.primitives.push(Primitive::Text {
                x: 24,
                y,
                value: v.chars().take(156).collect(),
                color,
            })
        };
        line(
            12,
            "LIVE MASTER EQ / optional owner extension / no rearm or full graph replacement".into(),
            "#66dfd3",
        );
        line(
            60,
            format!(
                "{} / PA scope {} / {}",
                if self.live_eq_observation_fresh() {
                    "FRESH PAIRED READBACK"
                } else {
                    "STALE / LAST CONFIRMED / EDITS DISABLED"
                },
                self.scope,
                if self.state.as_ref().is_some_and(Update::writer_granted) {
                    "explicit grant"
                } else {
                    "READ ONLY / G grant required"
                }
            ),
            "#f1bd6b",
        );
        if let Some(live) = self.state.as_ref().and_then(|u| u.live_eq.as_ref()) {
            line(
                108,
                format!(
                    "Supported {} / available {} / settled {} / outer source+PA fault {} / recovery required {}",
                    live.live_supported,
                    live.live_available,
                    live.settled,
                    live.fault_latched,
                    live.source_recovery_required
                ),
                "#e4e8e9",
            );
            line(
                156,
                format!(
                    "Owner {} / graph {} / EQ {} / map {} / remaining {} frames / retirement storage {}",
                    live.owner_instance,
                    live.graph_generation,
                    live.eq_generation,
                    live.map_revision,
                    live.transition_remaining_frames,
                    live.retirement_occupied
                ),
                "#e4e8e9",
            );
            line(
                204,
                format!(
                    "Program buses {:?} / main {} / raw age {} / EQ age {} / reason {}",
                    live.program_buses,
                    live.master_input_indices.map_or_else(
                        || "unavailable".into(),
                        |indices| format!("L input {} / R input {}", indices[0], indices[1])
                    ),
                    self.state
                        .as_ref()
                        .map_or("unavailable".into(), |u| observation_age_display(
                            u.snapshot_age_ms,
                            u.received
                        )),
                    self.state
                        .as_ref()
                        .map_or("unavailable".into(), |u| observation_age_display(
                            u.live_eq_age_ms,
                            u.received
                        )),
                    live.unavailable_reason.as_deref().unwrap_or("none")
                ),
                "#e4e8e9",
            );
            if let Ok(owner) = live.owner() {
                line(
                    252,
                    format!(
                        "CALCULATED EQ RESPONSE / {} CURRENT cyan, COMMITTED TARGET amber / static endpoints",
                        if self.live_eq_observation_fresh() {
                            ""
                        } else {
                            "LAST CONFIRMED"
                        }
                    ),
                    "#9caebc",
                );
                line(288,"During a fade these curves are NOT the exact time-varying transfer. No spectrum/protection/room measurement.".into(),"#9caebc");
                for (row, state) in ["current", "target"].into_iter().enumerate() {
                    let summary = owner[state]
                        .as_array()
                        .unwrap()
                        .iter()
                        .map(|input| {
                            format!(
                                "input {} / PEQ {} / GEQ {} / band1 {} Hz {:+.3} dB",
                                input["input_index"],
                                input["eq_enabled"],
                                input["geq_enabled"],
                                input["eq"][0]["hz"],
                                input["eq"][0]["db"].as_f64().unwrap()
                            )
                        })
                        .collect::<Vec<_>>()
                        .join(" | ");
                    line(
                        336 + row as u32 * 36,
                        format!("{state}: {summary}"),
                        if row == 0 { "#66dfd3" } else { "#f1bd6b" },
                    );
                }
                // Separate L/R panels; endpoint settings only, no sample histories.
                let mut curves = Vec::new();
                for side in 0..2 {
                    let x = 24 + side as u32 * 960;
                    curves.push(Primitive::Text {
                        x,
                        y: 456,
                        color: "#9caebc",
                        value: format!(
                            "MAIN {} / owner input {} / response dB / frequency Hz",
                            if side == 0 { "L" } else { "R" },
                            live.master_input_indices.unwrap()[side]
                        ),
                    });
                    for (tick, origin, label) in crate::eq_response::frequency_axis(x, 840, 20000.)
                    {
                        curves.push(Primitive::Line {
                            x1: tick,
                            y1: 480,
                            x2: tick,
                            y2: 768,
                            color: "#3c4f63",
                        });
                        curves.push(Primitive::Text {
                            x: origin,
                            y: 804,
                            value: label.into(),
                            color: "#9caebc",
                        });
                    }
                }
                for state in ["current", "target"] {
                    for side in 0..2 {
                        let bank =
                            crate::eq_response::coefficients(&owner[state][side], 48000).unwrap();
                        let x = 24 + side as u32 * 960;
                        let y = 480;
                        let w = 840;
                        let h = 288;
                        let color = if state == "current" {
                            "#66dfd3"
                        } else {
                            "#f1bd6b"
                        };
                        let mut prev = None;
                        for n in 0..=240 {
                            let hz = 20. * 1000_f64.powf(f64::from(n) / 240.);
                            let (db, _) = crate::eq_response::response(&bank, 48000, hz).unwrap();
                            let p = (
                                x + n as u32 * w / 240,
                                y + ((48. - db.clamp(-48., 48.)) / 96. * f64::from(h)) as u32,
                            );
                            if let Some((px, py)) = prev {
                                curves.push(Primitive::Line {
                                    x1: px,
                                    y1: py,
                                    x2: p.0,
                                    y2: p.1,
                                    color,
                                });
                            }
                            prev = Some(p);
                        }
                    }
                }
                line(420,"Owner bank order displayed by input_index; exact program bus map above identifies L/R.".into(),"#9caebc");
                line(852,"Curves clipped at +/-48dB; current/target are normalized owner settings calculations only.".into(),"#9caebc");
                line(936,"E edit only when fresh, compatible and settled | G explicit PA grant | F11 separate MUTED setup".into(),"#66dfd3");
                line(984, self.message.clone(), "#f47c85");
                s.primitives.extend(curves);
                return s;
            }
        } else {
            line(252,"Live EQ readback unavailable; optional-library absence leaves F11 muted setup working.".into(),"#f1bd6b");
        }
        line(936,"E edit requires fresh settled owner readback | F11 muted setup | F12 sends / explicit scope navigation".into(),"#66dfd3");
        line(984, self.message.clone(), "#f47c85");
        s
    }
}
#[cfg(test)]
mod gp18_ui_tests {
    use super::*;
    fn sends_surface() -> (Frontend, Receiver<Request>) {
        let (mut f, rx) = super::processing_tests::brain_surface();
        f.brain_page = false;
        f.brain_enabled = false;
        f.sends_page = true;
        f.selected_monitor = 2;
        f.scope = "monitor3".into();
        let u = f.state.as_mut().unwrap();
        u.snapshot = Some(
            crate::audio::decode_snapshot(include_bytes!(
                "../tests/fixtures/gp18/v1-corrected/raw-baseline.json"
            ))
            .unwrap(),
        );
        u.sends = crate::sends::decode_reply(include_bytes!(
            "../tests/fixtures/gp18/v1-corrected/baseline.json"
        ))
        .unwrap()
        .snapshot;
        u.sends_age_ms = Some(0);
        u.processing = None;
        u.received = Instant::now();
        (f, rx)
    }
    #[test]
    fn independent_inventories_separate_level_tap_reviews_and_scope_switch_do_not_mutate_holds() {
        let (mut f, rx) = sends_surface();
        let original = f.state.as_ref().unwrap().snapshot.clone();
        assert!(f.scene().in_bounds());
        f.key("E").unwrap();
        f.key("2").unwrap();
        assert!(rx.try_recv().is_err());
        f.key("F4").unwrap();
        assert!(
            matches!(rx.try_recv().unwrap().operation,Operation::ReviewTap(ref b) if b["input"]=="input-01"&&b["monitor"]=="monitor-3"&&b["tap"]=="processed_pre_fader")
        );
        assert_eq!(f.state.as_ref().unwrap().snapshot, original);
        f.key("S").unwrap();
        for key in ["-", "6", ".", "1", "Enter"] {
            f.key(key).unwrap();
        }
        assert!(matches!(
            f.send_draft.as_ref().unwrap().value,
            SendDraftValue::Level(-6100)
        ));
        f.key("F4").unwrap();
        assert!(
            matches!(rx.try_recv().unwrap().operation,Operation::ReviewSet{ref target,ref value} if target["monitor"]=="monitor-3"&&*value==json!(-6100))
        );
        f.state.as_mut().unwrap().review = Some((99, "queued tap / monitor3".into()));
        f.synchronize_review();
        f.action(Action::SwitchScope("foh".into())).unwrap();
        assert!(f.state.is_none());
        assert!(f.review_seen.is_empty());
        assert!(
            matches!(rx.try_recv().unwrap().operation,Operation::SwitchScope(ref s) if s=="foh")
        );
        assert!(rx.try_iter().all(|r| !matches!(
            r.operation,
            Operation::Grant
                | Operation::Set { .. }
                | Operation::Mode { .. }
                | Operation::Preview(_)
        )));
    }
    #[test]
    fn sends_display_separates_raw_tap_age_and_elapsed_last_confirmed_values() {
        let (mut f, _rx) = sends_surface();
        let u = f.state.as_mut().unwrap();
        u.snapshot_age_ms = Some(0);
        u.sends_age_ms = Some(240);
        u.received = Instant::now() - Duration::from_millis(20);
        assert!(f.fresh());
        assert!(!f.sends_ready());
        let texts: Vec<_> = f
            .scene()
            .primitives
            .into_iter()
            .filter_map(|p| match p {
                Primitive::Text { value, .. } => Some(value),
                _ => None,
            })
            .collect();
        assert!(texts.iter().any(|s| s.contains("RAW fresh / TAPS stale")));
        assert!(
            texts
                .iter()
                .any(|s| s.contains("Raw / Post-mute") && s.contains("LAST CONFIRMED"))
        );
        assert!(texts.iter().any(|s| s.contains("-60.0 dB / -60.0 dB")));
        assert!(
            texts
                .iter()
                .any(|s| s.contains("raw age ") && s.contains("tap age "))
        );
        assert!(!texts.iter().any(|s| s.contains("Some(")));
        assert!(f.action(Action::SendTapEdit).is_err());
    }
    #[test]
    fn response_frequency_labels_have_disjoint_text_bounds_in_both_sections() {
        let (mut f, _rx) = super::processing_tests::master_surface();
        f.key("F11").unwrap();
        for (graphic, y) in [(false, 684), (true, 852)] {
            if graphic {
                f.key("B").unwrap();
            }
            let mut labels: Vec<_> = f
                .scene()
                .primitives
                .into_iter()
                .filter_map(|p| match p {
                    Primitive::Text {
                        x, y: yy, value, ..
                    } if yy == y => Some((x, value.len() as u32 * 12)),
                    _ => None,
                })
                .collect();
            labels.sort_unstable();
            assert_eq!(labels.len(), 5);
            assert!(
                labels
                    .windows(2)
                    .all(|pair| pair[0].0 + pair[0].1 + 12 <= pair[1].0)
            );
            assert!(labels.iter().all(|(x, width)| x + width <= 1920));
        }
    }
    #[test]
    fn reattachment_ignores_old_attachment_even_after_input_generation_advances() {
        let (mut f, _rx) = sends_surface();
        let mut old = f.state.clone().unwrap();
        f.action(Action::SwitchScope("foh".into())).unwrap();
        old.generation = f.provider.generation();
        f.accept_update(old.clone());
        assert!(f.state.is_none());
        assert!(!f.fresh());
        assert!(f.key("G").is_err());
        old.attachment_generation = f.provider.generation();
        old.writer_lease_remaining_ms = None;
        f.accept_update(old);
        assert!(f.fresh());
        assert!(!f.state.as_ref().unwrap().writer_granted());
    }
    #[test]
    fn monitor_browse_is_read_only_text_precedes_shortcuts_and_focus_revokes_apply() {
        let (mut f, _rx) = sends_surface();
        f.selected_monitor = 1;
        assert!(f.action(Action::SendTapEdit).is_err());
        assert!(f.action(Action::Adjust(1000)).is_err());
        f.selected_monitor = 2;
        f.key("S").unwrap();
        assert!(f.key("O").is_err());
        assert_eq!(f.scope, "monitor3");
        f.key("Esc").unwrap();
        f.action(Action::SendLevelText("-6.125".into()))
            .unwrap_err();
        f.action(Action::SendLevelText("12.0".into())).unwrap();
        f.enqueue(Event::Focus(false)).unwrap();
        assert!(f.send_draft.is_some());
        assert!(f.action(Action::SendApply).is_err());
    }
    #[test]
    fn all_master_fields_and_all_graphic_positions_fit_native_scene() {
        let (mut f, _rx) = super::processing_tests::master_surface();
        f.key("F11").unwrap();
        for _ in 0..41 {
            assert!(f.scene().in_bounds());
            f.key("I").unwrap();
        }
        f.key("B").unwrap();
        for _ in 0..32 {
            assert!(f.scene().in_bounds());
            f.key("I").unwrap();
        }
    }
    #[test]
    fn live_scene_independently_expired_raw_or_extension_is_last_confirmed() {
        for expire_raw in [false, true] {
            let (mut f, _) = super::processing_tests::master_surface();
            f.brain_page = false;
            f.live_page = true;
            let corpus: Vec<Value> = serde_json::from_slice(include_bytes!(
                "../tests/fixtures/master-eq/v1/producer.json"
            ))
            .unwrap();
            let live = crate::live_eq::Snapshot::decode(
                corpus
                    .into_iter()
                    .find(|v| v["label"] == "baseline")
                    .unwrap()["snapshot"]
                    .clone(),
            )
            .unwrap();
            let u = f.state.as_mut().unwrap();
            u.snapshot = Some(
                crate::audio::decode_snapshot(include_bytes!(
                    "../tests/fixtures/gp18/v1-corrected/raw-baseline.json"
                ))
                .unwrap(),
            );
            u.snapshot.as_mut().unwrap().authority.revision = live.revision.clone();
            u.snapshot
                .as_mut()
                .unwrap()
                .topology
                .as_mut()
                .unwrap()
                .map_revision = live.map_revision.parse().unwrap();
            u.live_eq = Some(live);
            u.live_eq_age_ms = Some(0);
            u.snapshot_age_ms = Some(0);
            u.received = Instant::now();
            let labels: Vec<_> = f
                .scene()
                .primitives
                .into_iter()
                .filter_map(|p| match p {
                    Primitive::Text {
                        x, y: 804, value, ..
                    } => Some((x, value)),
                    _ => None,
                })
                .collect();
            assert_eq!(labels.len(), 10);
            assert!(labels.contains(&(487, "1k".into())));
            assert!(labels.contains(&(1447, "1k".into())));
            for pane in labels.chunks(5) {
                assert!(
                    pane.windows(2)
                        .all(|p| p[0].0 + p[0].1.len() as u32 * 12 + 12 <= p[1].0)
                );
            }
            assert!(f.live_eq_ready());
            if expire_raw {
                f.state.as_mut().unwrap().snapshot_age_ms = Some(251);
            } else {
                f.state.as_mut().unwrap().live_eq_age_ms = Some(251);
            }
            assert!(!f.live_eq_ready());
            assert!(f.action(Action::LiveEqEdit).is_err());
            assert!(f.scene().primitives.iter().any(|p|matches!(p,Primitive::Text{value,..} if value.contains("STALE / LAST CONFIRMED"))));
        }
    }
    #[test]
    fn processing_controller_cannot_open_hidden_under_sends_but_visible_keys_work() {
        let (mut f, rx) = sends_surface();
        f.scope = "foh".into();
        f.page = Page::Channel;
        let u = f.state.as_mut().unwrap();
        u.processing = crate::processing::decode_reply(include_bytes!(
            "../tests/fixtures/gp18/v1-corrected/gp07v4-ready.json"
        ))
        .unwrap()
        .snapshot;
        u.processing_age_ms = Some(0);
        u.snapshot.as_mut().unwrap().authority.revision =
            u.processing.as_ref().unwrap().revision.clone();
        assert!(f.action(Action::ProcessingEdit).is_err());
        assert!(f.processing_draft.is_none());
        assert!(
            f.scene()
                .primitives
                .iter()
                .any(|p| matches!(p,Primitive::Text{value,..} if value.contains("SENDS")))
        );
        let mut fresh = f.state.clone().unwrap();
        f.action(Action::Page(Page::Channel)).unwrap();
        fresh.generation = f.provider.generation();
        fresh.received = Instant::now();
        f.accept_update(fresh);
        f.key("E").unwrap();
        f.key("I").unwrap();
        assert!(f.processing_draft.is_some());
        assert!(
            f.scene()
                .primitives
                .iter()
                .any(|p| matches!(p,Primitive::Text{value,..} if value.contains("EDIT FIELD")))
        );
        f.key("F4").unwrap();
        assert!(
            rx.try_iter()
                .any(|r| matches!(r.operation, Operation::ReviewProcessing { .. }))
        );
    }
    #[test]
    #[ignore = "explicit current frontend offline preview gallery; no connected engine or display"]
    fn current_frontend_offline_gallery() {
        let output = std::path::PathBuf::from(
            std::env::var_os("SHR_DESK_OFFLINE_GALLERY").expect("explicit output directory"),
        );
        std::fs::create_dir_all(&output).unwrap();
        let save = |name: &str, f: &mut Frontend| {
            f.state.as_mut().unwrap().received = Instant::now();
            let mut scene = f.scene();
            scene.primitives.push(Primitive::Rect {
                x: 0,
                y: 1032,
                w: 1920,
                h: 48,
                fill: "#10151d",
            });
            scene.primitives.push(Primitive::Text{x:24,y:1044,value:"OFFLINE / SIMULATED fixture-driven current Frontend scene / NO CONNECTED ENGINE".into(),color:"#f47c85"});
            assert!(scene.in_bounds(), "{name}");
            #[cfg(feature = "native")]
            if std::env::var_os("VK_DRIVER_FILES").is_some() {
                for (w, h) in [(1920, 1080), (960, 540), (728, 1024)] {
                    println!(
                        "{name}: {}",
                        crate::native::offscreen_at(&scene, w, h).unwrap()
                    );
                }
            }
            std::fs::write(
                output.join(format!("{name}.svg")),
                crate::render::svg(&scene),
            )
            .unwrap();
            crate::raster::ppm(&scene, &output.join(format!("{name}.ppm"))).unwrap();
            // Each next offline gesture starts from the same explicit simulated fixture.
            f.state.as_mut().unwrap().received = Instant::now();
        };
        let (mut f, _rx) = sends_surface();
        f.selected = 16;
        save("sends-overview-monitor3", &mut f);
        f.sends_channel = true;
        f.action(Action::SendTapEdit).unwrap();
        f.action(Action::SendTap(crate::sends::Tap::ProcessedPreFader))
            .unwrap();
        save("channel-sends", &mut f);
        f.sends_page = false;
        f.send_draft = None;
        f.scope = "foh".into();
        f.page = Page::Channel;
        let u = f.state.as_mut().unwrap();
        u.processing = crate::processing::decode_reply(include_bytes!(
            "../tests/fixtures/gp18/v1-corrected/gp07v4-ready.json"
        ))
        .unwrap()
        .snapshot;
        u.snapshot.as_mut().unwrap().authority.revision =
            u.processing.as_ref().unwrap().revision.clone();
        u.processing_age_ms = Some(0);
        u.received = Instant::now();
        let channel = u
            .processing
            .as_mut()
            .unwrap()
            .channels
            .iter_mut()
            .find(|c| c.input == "input-17")
            .unwrap();
        channel.current.eq_bypass = false;
        channel.current.band2_hz = 1250;
        channel.current.band2_gain_mdb = 4500;
        channel.current.band2_bypass = false;
        channel.current.compressor_bypass = false;
        channel.current.threshold_mdb = -24000;
        channel.current.ratio_milli = 3000;
        channel.current.validate().unwrap();
        channel.target = channel.current.clone();
        channel.gain_reduction_mdb = None;
        save("channel-eq-compressor", &mut f);
        let (mut f, _rx) = super::processing_tests::master_surface();
        f.key("F11").unwrap();
        f.action(Action::StructureField(3)).unwrap();
        f.action(Action::StructureText("6.125".into())).unwrap();
        f.action(Action::StructureField(3)).unwrap();
        f.action(Action::StructureText("high_shelf".into()))
            .unwrap();
        f.action(Action::StructureField(1)).unwrap();
        f.action(Action::StructureText("4000".into())).unwrap();
        f.action(Action::StructureField(1)).unwrap();
        f.action(Action::StructureText("-3.5".into())).unwrap();
        f.action(Action::StructureField(-5)).unwrap();
        save("master-parametric", &mut f);
        f.key("B").unwrap();
        f.action(Action::StructureField(-3)).unwrap();
        f.action(Action::StructureText("true".into())).unwrap();
        f.action(Action::StructureField(18)).unwrap();
        f.action(Action::StructureText("-4.5".into())).unwrap();
        save("master-graphic", &mut f);
        let corpus: Vec<Value> = serde_json::from_slice(include_bytes!(
            "../tests/fixtures/master-eq/v1/producer.json"
        ))
        .unwrap();
        f.structural_draft = None;
        f.live_page = true;
        f.state.as_mut().unwrap().snapshot = Some(
            crate::audio::decode_snapshot(include_bytes!(
                "../tests/fixtures/gp18/v1-corrected/raw-baseline.json"
            ))
            .unwrap(),
        );
        for (label, name) in [
            ("transition", "live-current-target"),
            ("settled", "live-settled"),
            ("settled", "live-stale"),
            ("unavailable", "live-unavailable"),
        ] {
            let live = crate::live_eq::Snapshot::decode(
                corpus.iter().find(|r| r["label"] == label).unwrap()["snapshot"].clone(),
            )
            .unwrap();
            let u = f.state.as_mut().unwrap();
            let raw = u.snapshot.as_mut().unwrap();
            raw.authority.show_id = live.show_id.clone();
            raw.authority.epoch = live.epoch.clone();
            raw.clock.as_mut().unwrap().epoch = live.epoch.parse().unwrap();
            raw.authority.revision = live.revision.clone();
            raw.topology.as_mut().unwrap().map_revision = live.map_revision.parse().unwrap();
            u.snapshot_age_ms = Some(0);
            u.live_eq_age_ms = Some(if name == "live-stale" { 251 } else { 0 });
            u.live_eq = Some(live);
            save(name, &mut f);
        }
    }
}
