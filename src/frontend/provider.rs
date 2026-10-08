//! Bounded provider queue, single worker and coalesced publication.
//! Operator retains session authority; UI generation fences never create another session.
use super::{Config, Operation, Update, parameter_display};
use crate::local_audio::Operator;
use serde_json::json;
#[cfg(test)]
use std::path::PathBuf;
use std::{
    sync::{
        Arc, Mutex,
        atomic::{AtomicBool, AtomicU64, AtomicUsize, Ordering},
        mpsc::{self, Receiver, SyncSender, TrySendError},
    },
    thread::{self, JoinHandle},
    time::{Duration, Instant},
};

#[derive(Debug)]
pub(super) struct PendingEdit(Arc<AtomicUsize>);
impl PendingEdit {
    pub(super) fn new(count: &Arc<AtomicUsize>) -> Self {
        count.fetch_add(1, Ordering::AcqRel);
        Self(count.clone())
    }
}
impl Clone for PendingEdit {
    fn clone(&self) -> Self {
        Self::new(&self.0)
    }
}
impl Drop for PendingEdit {
    fn drop(&mut self) {
        self.0.fetch_sub(1, Ordering::AcqRel);
    }
}
#[derive(Clone, Debug)]
pub(super) struct Request {
    pub(super) generation: u64,
    pub(super) revision: Option<String>,
    pub(super) operation: Operation,
    pub(super) _pending: Option<PendingEdit>,
}
/// One coalesced provider-state slot, independent of input and desired LED state.
#[derive(Default)]
pub(super) struct Latest {
    pub(super) update: Mutex<Option<Update>>,
}
pub struct Provider {
    pub(super) tx: SyncSender<Request>,
    pub(super) pending_edits: Arc<AtomicUsize>,
    pub(super) latest: Arc<Latest>,
    pub(super) generation: Arc<AtomicU64>,
    pub(super) stop: Arc<AtomicBool>,
    pub(super) child: Option<JoinHandle<()>>,
    pub(super) authorization: Arc<AtomicBool>,
    pub(super) brain_signal: Arc<crate::brain::HoldSignal>,
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
            pending_edits: Arc::new(AtomicUsize::new(0)),
            latest,
            generation,
            stop,
            child: Some(child),
            authorization,
            brain_signal,
        }
    }
    pub(super) fn send(
        &self,
        revision: Option<String>,
        operation: Operation,
    ) -> Result<(), String> {
        self.tx
            .try_send(Request {
                generation: self.generation.load(Ordering::Acquire),
                revision,
                _pending: (!matches!(operation, Operation::InputReleased))
                    .then(|| PendingEdit::new(&self.pending_edits)),
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
                            _pending: None,
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
                            o.stage("set", json!({"targets":[{"target":target.clone(),"value":value.clone()}]}))?;
                            serial = serial.checked_add(1).ok_or("review counter exhausted")?;
                            review = Some((
                                serial,
                                format!(
                                    "{} {} {} proposed {}\n{}",
                                    target["input"].as_str().unwrap_or("--"),
                                    target["monitor"].as_str().unwrap_or("FOH"),
                                    target["parameter"].as_str().unwrap_or("--"),
                                    parameter_display(
                                        target["parameter"].as_str().unwrap_or(""),
                                        &value
                                    ),
                                    o.reviewed().unwrap_or_default()
                                ),
                            ));
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

pub(super) fn confirmed_lease_remaining(operator: &Operator) -> Option<u64> {
    operator
        .session
        .lease_deadline()?
        .checked_sub(operator.now())
        .filter(|remaining| *remaining > 0)
}

pub(super) fn publish_provider_update(
    latest: &Latest,
    mut update: Update,
    operation_error: Option<&str>,
) {
    if let Some(error) = operation_error {
        update.status = error.into();
    }
    *latest.update.lock().unwrap() = Some(update);
}
