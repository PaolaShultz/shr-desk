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
    pub endpoint: PathBuf,
    pub show: String,
    pub epoch: u64,
    pub writer: String,
    pub scope: String,
}
#[derive(Clone, Debug)]
pub enum Operation {
    Grant,
    ReleaseWriter,
    Set { target: Value, value: Value },
    ReviewSet { target: Value, value: Value },
    Preview(Value),
    Mode { mode: String, bounds: Value },
    Confirm(u64),
    Cancel,
    InputReleased,
    Reconnect,
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
    let mut g = generation.load(Ordering::Acquire);
    let mut serial = 0u64;
    let mut review: Option<(u64, String)> = None;
    let mut status = "provider unavailable; read-only attach".to_string();
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
            match Operator::connect(
                &config.endpoint,
                &config.show,
                config.epoch,
                &writer,
                &config.scope,
            ) {
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
                    Operation::InputReleased | Operation::Cancel | Operation::Reconnect
                )
            {
                status = "ROLE UNAVAILABLE: new provider writes refused".into();
                continue;
            }
            if matches!(r.operation, Operation::Reconnect) {
                op = None;
                review = None;
                reconnects += 1;
                connect = true;
                status = "reconnect discards intents; fresh writer/read-only".into();
            } else if let Some(o) = &mut op {
                let affects_status = !matches!(r.operation, Operation::InputReleased);
                o.guard(generation.clone(), g);
                if affects_status {
                    *latest.update.lock().unwrap() = Some(Update {
                        generation: g,
                        snapshot: o.session.snapshot.clone(),
                        fresh: o.session.fresh(o.now()),
                        status: format!(
                            "PENDING {}; awaiting provider confirmation",
                            match &r.operation {
                                Operation::Grant => "writer grant",
                                Operation::ReleaseWriter => "writer release",
                                Operation::Set { .. } => "parameter edit",
                                Operation::ReviewSet { .. } => "protected edit review",
                                Operation::Preview(_) => "engine release preview",
                                Operation::Mode { .. } => "mode review",
                                Operation::Confirm(_) => "reviewed operation",
                                Operation::Cancel => "review cancellation",
                                Operation::InputReleased => "input release",
                                Operation::Reconnect => "reconnect",
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
                        Operation::Grant => o.mutate_inner("grant", json!({"scope":config.scope})),
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
                        Operation::Reconnect => unreachable!(),
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
    Key { key: String, pressed: bool },
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
pub struct Frontend {
    pub provider: Provider,
    pub state: Option<Update>,
    module_config: Config,
    module_client: Option<crate::modules::Worker>,
    pub modules: Option<crate::modules::Update>,
    pub selected: usize,
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
            let id = u.review.as_ref().map(|(id, _)| *id);
            if id != self.review_id {
                self.review_id = id;
                self.review_page = 0;
                self.review_seen.clear();
            }
            self.state = Some(u);
        }
        // Optional metadata gets its own connection and worker only after primary
        // attachment. No module query can consume control replies or block input.
        if self.module_client.is_none() && self.state.as_ref().is_some_and(|s| s.snapshot.is_some())
        {
            self.module_client = Some(crate::modules::Worker::start(self.module_config.clone()));
        }
        if let Some(client) = &self.module_client
            && let Some(update) = client.take()
        {
            self.modules = Some(update);
        }
        while let Some(event) = self.queue.pop_front() {
            if let Event::Key { key, pressed } = event {
                if !pressed {
                    self.pressed.remove(&key);
                    self.blocked.remove(&key);
                    if self.focused {
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
        if key == "F5" {
            self.fence();
            return self.provider.send(None, Operation::Reconnect);
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
        let action = actions::key_action(key).ok_or("unmapped key")?;
        match action {
            Action::Page(page) => {
                self.send_cancel();
                self.page = page;
                Ok(())
            }
            Action::Move(delta) | Action::Bank(delta) => {
                let delta = if matches!(action, Action::Bank(_)) {
                    delta * 12
                } else {
                    delta
                };
                let n = self
                    .state
                    .as_ref()
                    .and_then(|s| s.snapshot.as_ref())
                    .map_or(0, |s| s.authority.inputs.len());
                if n == 0 {
                    return Err("inputs unavailable".into());
                }
                self.send_cancel();
                self.selected =
                    (self.selected as i64 + i64::from(delta)).rem_euclid(n as i64) as usize;
                Ok(())
            }
            Action::ModePicker => {
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
                                b.target.monitor.as_deref()
                                    == match self.scope.as_str() {
                                        "monitor1" => Some("monitor-1"),
                                        "monitor2" => Some("monitor-2"),
                                        _ => None,
                                    }
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
        self.mode_picker = false;
        self.provider.fence();
        self.state = None;
        let _ = self.provider.send(None, Operation::Cancel);
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
        Ok(if self.scope == "foh" || parameter == "mute" {
            json!({"input":input,"parameter":parameter})
        } else {
            json!({"input":input,"parameter":"send","monitor":if self.scope=="monitor1"{"monitor-1"}else{"monitor-2"}})
        })
    }
    fn adjust(&mut self, parameter: &str, delta: i64, toggle: bool) -> Result<(), String> {
        if self.mode_picker || self.state.as_ref().is_some_and(|s| s.review.is_some()) {
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
    fn review_lines(&self) -> Vec<String> {
        self.state
            .as_ref()
            .and_then(|s| s.review.as_ref())
            .map_or_else(Vec::new, |(_, text)| {
                let chars: Vec<_> = text.chars().collect();
                chars.chunks(156).map(|c| c.iter().collect()).collect()
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
        line(
            12,
            format!(
                "SHR DESK / REAL GP03 RAW MIXER / {} / {}",
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
                "PROVIDER FRESH / RAW MIXER OFFLINE UNPROTECTED / METERS UNAVAILABLE".into()
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
                                && p.target.monitor.as_deref()
                                    == match self.scope.as_str() {
                                        "monitor1" => Some("monitor-1"),
                                        "monitor2" => Some("monitor-2"),
                                        _ => None,
                                    }
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
                            format!("ACTUAL LINEAR NANO: {:?}", c.current_nanogain.map(|n| n.0))
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
                                c.ramp_target_nanogain.map(|n| n.0)
                            )
                        } else {
                            format!(
                                "RENDERED PAN L {} / R {} / MUTE {} / MON1 {} / MON2 {}",
                                linear_display(c.current_nanogain[1]),
                                linear_display(c.current_nanogain[2]),
                                if c.current_nanogain[3].0 == 0 {
                                    "on"
                                } else {
                                    "off"
                                },
                                linear_display(c.current_nanogain[4]),
                                linear_display(c.current_nanogain[5])
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
            "F5 explicit reconnect (fresh writer, no replay) | REC/PA/FX writable controls and analysis unavailable"
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
