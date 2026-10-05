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
    pub snapshot: Option<RenderedSnapshot>,
    pub processing: Option<crate::processing::Snapshot>,
    pub processing_age_ms: Option<u64>,
    pub processing_status: String,
    pub structural: Option<crate::structure::Snapshot>,
    pub structural_final: Option<crate::structure::Reply>,
    pub structural_fresh: bool,
    pub processing_final: Option<crate::processing::Reply>,
    pub fresh: bool,
    pub status: String,
    pub review: Option<(u64, String)>,
    pub received: Instant,
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
        let child = thread::spawn(move || worker(config, rx, l, g, s, a));
        Self {
            tx,
            latest,
            generation,
            stop,
            child: Some(child),
            authorization,
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
        self.stop.store(true, Ordering::Release);
        if let Some(c) = self.child.take() {
            let _ = c.join();
        }
    }
}
fn worker(
    config: Config,
    rx: Receiver<Request>,
    latest: Arc<Latest>,
    generation: Arc<AtomicU64>,
    stop: Arc<AtomicBool>,
    authorization: Arc<AtomicBool>,
) {
    let mut op: Option<Operator> = None;
    let mut structural_final: Option<crate::structure::Reply> = None;
    let mut g = generation.load(Ordering::Acquire);
    let mut serial = 0u64;
    let mut review: Option<(u64, String)> = None;
    let mut status = "provider unavailable; read-only attach".to_string();
    let mut processing_requested = false;
    let mut processing_enabled = false;
    let mut processing_status = "unavailable; GP07 probe not enabled".to_string();
    let mut processing_poll = Instant::now();
    let mut connect = true;
    let mut reconnects = 0u64;
    while !stop.load(Ordering::Acquire) {
        let current = generation.load(Ordering::Acquire);
        if current != g {
            g = current;
            review = None;
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
                Ok(mut o) => match o.refresh() {
                    Ok(()) => {
                        status = "provider attached read-only; explicit G grant required".into();
                        op = Some(o)
                    }
                    Err(e) => status = format!("UNAVAILABLE: {e}"),
                },
                Err(e) => status = format!("UNAVAILABLE: {e}"),
            }
        }
        if let Ok(r) = rx.recv_timeout(Duration::from_millis(40)) {
            // A recovery request can wake recv after its generation was revoked.
            // Synchronize before comparing so a fresh reconnect is never discarded.
            let current = generation.load(Ordering::Acquire);
            if current != g {
                g = current;
                review = None;
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
                        | Operation::EnableProcessing
                )
            {
                status = "ROLE UNAVAILABLE: new provider writes refused".into();
                continue;
            }
            if matches!(r.operation, Operation::EnableProcessing) {
                processing_requested = true;
                processing_enabled = true;
                processing_status = "awaiting capability snapshot".into();
            } else if matches!(
                r.operation,
                Operation::Reconnect | Operation::LegacyReconnect
            ) {
                if matches!(r.operation, Operation::LegacyReconnect) {
                    processing_requested = false;
                    processing_status = "disabled by explicit legacy GP03 reconnect".into();
                }
                op = None;
                review = None;
                reconnects += 1;
                processing_enabled = processing_requested;
                connect = true;
                status = "reconnect discards intents; fresh writer/read-only".into();
            } else if let Some(o) = &mut op {
                let affects_status = !matches!(r.operation, Operation::InputReleased);
                o.guard(generation.clone(), g);
                if affects_status {
                    *latest.update.lock().unwrap() = Some(Update {
                        generation: g,
                        snapshot: o.session.snapshot.clone(),
                        processing: o.session.processing.clone(),
                        processing_age_ms: o.session.processing_age(o.now()),
                        processing_status: processing_status.clone(),
                        structural: o.session.structural.clone(),
                        structural_final: o.session.structural_final.clone(),
                        structural_fresh: o.session.structural_fresh(o.now()),
                        processing_final: o.session.processing_final.clone(),
                        fresh: o.session.fresh(o.now()),
                        status: format!(
                            "PENDING {}; awaiting provider confirmation",
                            match &r.operation {
                                Operation::EnableProcessing => "capability query",
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
                        received: Instant::now(),
                    });
                }
                let result = (|| {
                    if let Some(revision) = &r.revision {
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
                        Operation::EnableProcessing => unreachable!(),
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
                        Operation::Reconnect | Operation::LegacyReconnect => unreachable!(),
                    }
                })();
                match result {
                    Ok(()) if affects_status => status = o.session.last_result.clone(),
                    Ok(()) => {}
                    Err(e) => {
                        status = format!("REFUSED/UNCERTAIN: {e}");
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
            if review.is_some() && o.review_valid() && generation.load(Ordering::Acquire) == g {
                *latest.update.lock().unwrap() = Some(Update {
                    generation: g,
                    snapshot: o.session.snapshot.clone(),
                    processing: o.session.processing.clone(),
                    processing_age_ms: o.session.processing_age(o.now()),
                    processing_status: processing_status.clone(),
                    structural: o.session.structural.clone(),
                    structural_final: o.session.structural_final.clone(),
                    structural_fresh: o.session.structural_fresh(o.now()),
                    processing_final: o.session.processing_final.clone(),
                    fresh: o.session.fresh(o.now()),
                    status: status.clone(),
                    review: review.clone(),
                    received: Instant::now(),
                });
            }
            if authorization.load(Ordering::Acquire)
                && o.session.renewal_due(o.now())
                && let Err(e) = o.mutate("renew", json!({}))
            {
                status = format!("LEASE UNCERTAIN: {e}");
            }
            if config.wire_version == 2
                && matches!(config.scope.as_str(), "pa_configuration" | "output_routes")
                && let Err(error) = o.refresh_structural()
            {
                status = format!("Structural state unavailable: {error}");
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
            if let Err(e) = o.refresh() {
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
        if generation.load(Ordering::Acquire) != g {
            continue;
        }
        let update = Update {
            generation: g,
            snapshot: op.as_ref().and_then(|o| o.session.snapshot.clone()),
            processing: op.as_ref().and_then(|o| o.session.processing.clone()),
            processing_age_ms: op.as_ref().and_then(|o| o.session.processing_age(o.now())),
            processing_status: processing_status.clone(),
            structural: op
                .as_ref()
                .and_then(|o| o.session.structural.clone())
                .or_else(|| structural_final.as_ref().and_then(|r| r.snapshot.clone())),
            structural_final: structural_final.clone(),
            structural_fresh: op
                .as_ref()
                .is_some_and(|o| o.session.structural_fresh(o.now())),
            processing_final: op.as_ref().and_then(|o| o.session.processing_final.clone()),
            fresh: op.as_ref().is_some_and(|o| o.session.fresh(o.now())),
            status: status.clone(),
            review: review.clone(),
            received: Instant::now(),
        };
        *latest.update.lock().unwrap() = Some(update);
    }
    // No release/recall is sent on UI exit. Engine owns persistent held state.
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
#[derive(Clone, Debug)]
pub struct ProcessingDraft {
    pub input: String,
    pub config: crate::processing::Config,
    revision: String,
    generation: u64,
}
pub struct Frontend {
    pub provider: Provider,
    pub state: Option<Update>,
    module_config: Config,
    module_client: Option<crate::modules::Worker>,
    pub modules: Option<crate::modules::Update>,
    pub selected: usize,
    topology_page: Option<usize>,
    pub processing_draft: Option<ProcessingDraft>,
    pub structural_draft: Option<crate::structure::Draft>,
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
            selected: 0,
            topology_page: None,
            processing_draft: None,
            structural_draft: None,
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
        }
    }
    /// Explicit GP03-only fresh attachment; drops drafts, authority and queued intents.
    pub fn reconnect_legacy(&mut self) -> Result<(), String> {
        self.fence();
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
        self.processing_draft = None;
        self.structural_draft = None;
        self.processing_entry.clear();
        self.state = None;
        self.leds = None;
        self.review_id = None;
        self.review_page = 0;
        self.review_seen.clear();
    }
    pub fn enqueue(&mut self, event: Event) -> Result<(), String> {
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
            self.processing_draft = None;
            self.processing_entry.clear();
            self.message = "Processing draft cancelled: context/revision/freshness changed".into();
        }
        if self.structural_draft.as_ref().is_some_and(|d| {
            !self.fresh()
                || !self.state.as_ref().is_some_and(|u| {
                    u.structural_fresh
                        && u.snapshot
                            .as_ref()
                            .is_some_and(|s| s.authority.revision == d.revision)
                })
                || d.generation != self.provider.generation()
        }) {
            self.structural_draft = None;
            self.processing_entry.clear();
            self.message = "Structural draft cancelled: authority/freshness changed".into();
        }
        while let Some(event) = self.queue.pop_front() {
            if let Event::Controller { action, generation } = event {
                if generation == self.provider.generation()
                    && self.focused
                    && self.device_ready
                    && self.width > 0
                    && self.height > 0
                {
                    let opens_editor =
                        matches!(action, Action::ProcessingEdit | Action::StructureEdit);
                    if let Err(e) = self.action(action) {
                        self.message = e;
                    }
                    // Match keyboard release behavior: unsent local field gestures
                    // must not fill the provider queue or delay paired observations.
                    if (self.processing_draft.is_none() && self.structural_draft.is_none())
                        || opens_editor
                    {
                        let _ = self.provider.send(None, Operation::InputReleased);
                    }
                }
                continue;
            }
            if let Event::Key { key, pressed } = event {
                if !pressed {
                    self.pressed.remove(&key);
                    self.blocked.remove(&key);
                    if self.focused
                        && ((self.processing_draft.is_none() && self.structural_draft.is_none())
                            || matches!(key.as_str(), "E" | "F9"))
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
        self.state
            .as_ref()
            .is_some_and(|s| s.fresh && s.received.elapsed() <= Duration::from_millis(250))
    }
    fn accept_update(&mut self, update: Update) {
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
    fn send(&mut self, operation: Operation) -> Result<(), String> {
        if self.role_required && !self.provider.authorization.load(Ordering::Acquire) {
            return Err("live GP09 role lease required; keyboard-only read-only surface".into());
        }
        if !self.fresh() {
            return Err("provider stale/unavailable; operation refused".into());
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
            return self.provider.send(None, Operation::Reconnect);
        }
        if matches!(key, "G" | "Q") && self.processing_draft.is_some() {
            return Err("Apply or Cancel processing draft before writer changes".into());
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
        if self.structural_draft.is_some() {
            match key {
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
    fn action(&mut self, action: Action) -> Result<(), String> {
        match action {
            Action::StructureEdit => {
                if self.processing_draft.is_some()
                    || self.structural_draft.is_some()
                    || self.state.as_ref().is_some_and(|u| u.review.is_some())
                {
                    return Err("Apply or Cancel existing edit first".into());
                }
                let state = self
                    .state
                    .as_ref()
                    .filter(|u| u.structural_fresh && self.fresh())
                    .ok_or("fresh structural readback required")?;
                self.structural_draft = Some(crate::structure::Draft::new(
                    state.structural.as_ref().ok_or("structural unavailable")?,
                    &self.scope,
                    self.provider.generation(),
                )?);
                self.processing_entry.clear();
                Ok(())
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
            Action::StructureAdjust(delta) => {
                let snapshot = self
                    .state
                    .as_ref()
                    .filter(|u| u.structural_fresh)
                    .and_then(|u| u.structural.as_ref())
                    .ok_or("fresh structural readback required")?;
                self.structural_draft
                    .as_mut()
                    .ok_or("F9 opens structural editor")?
                    .adjust(delta, snapshot)
            }
            Action::StructureApply => {
                if !self.processing_entry.is_empty() {
                    return Err("accept field before Apply".into());
                }
                let draft = self
                    .structural_draft
                    .as_ref()
                    .ok_or("no structural draft")?;
                let operation = Operation::ReviewStructure {
                    kind: draft.kind.clone(),
                    body: draft.body()?,
                };
                self.send(operation)?;
                self.structural_draft = None;
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
                if self.page != Page::Channel || self.scope != "foh" || !self.processing_ready() {
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
        if let Some(draft) = &self.structural_draft {
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
                        draft.document.pointer(path).unwrap()
                    ),
                    "#e4e8e9",
                );
            }
            line(
                816,
                format!(
                    "FIELD {}/{} / entry {}",
                    draft.selected + 1,
                    draft.fields.len(),
                    self.processing_entry
                ),
                "#66dfd3",
            );
            line(
                864,
                "U/I field | J/K +/-1 owner unit or toggle/cycle source | numeric text then Enter"
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
                "PageUp/PageDown patch | F9 scope editor | Z mute / X rearm (PA scope, reviewed) | F1/F2/F6 pages".into(),
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
                if self.page == Page::Analysis {
                    u.status.clone()
                } else if u.status.starts_with("grant applied") {
                    "Writer granted / controls available after input release".into()
                } else if u.status.starts_with("renew applied") {
                    "Writer lease renewed".into()
                } else if u.status.contains(" applied revision ") {
                    "Change confirmed by provider".into()
                } else {
                    u.status.clone()
                },
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
                                    "GP07 FOH: raw -> EQ -> compressor -> mute/fader/pan | Monitors: raw -> mute -> sends | {}",
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
                            line(996, "F5 reconnect | F8 explicit legacy GP03 reconnect | fresh writer/read-only, no replay".into(), "#9caebc");
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
            "F5 reconnect | F8 legacy GP03 reconnect | fresh writer, no replay | REC/PA/FX writes unavailable"
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
            generation: 1,
            snapshot: Some(
                crate::audio::decode_snapshot(&serde_json::to_vec(&corpus["initial"]).unwrap())
                    .unwrap(),
            ),
            processing: None,
            processing_age_ms: None,
            processing_status: "disabled".into(),
            structural: None,
            structural_final: None,
            structural_fresh: false,
            processing_final: None,
            fresh: true,
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
            generation: 1,
            snapshot: Some(snapshot),
            processing: None,
            processing_age_ms: None,
            processing_status: "disabled".into(),
            structural: None,
            structural_final: None,
            structural_fresh: false,
            processing_final: None,
            fresh: true,
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
            generation: 1,
            snapshot: Some(raw),
            processing,
            processing_age_ms: Some(0),
            processing_status: "fixture layout only".into(),
            structural: None,
            structural_final: None,
            structural_fresh: false,
            processing_final: None,
            fresh: true,
            status: "fixture layout only".into(),
            review: None,
            received: Instant::now(),
        });
        f.page = Page::Channel;
        f
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
        assert!(f.processing_draft.is_none());
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
        assert!(keyboard.processing_draft.is_none());
        assert!(keyboard.state.is_none());
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
        f.enqueue(Event::Focus(false)).unwrap();
        assert!(f.processing_draft.is_none());
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
