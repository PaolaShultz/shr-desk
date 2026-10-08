//! Operator state and rendering over the real-provider worker boundary.
//! No Simulator, device discovery or physical controller I/O.
mod fx;
mod measurement;
mod provider;
#[cfg(test)]
use crate::local_audio::Operator;
use crate::{
    actions::{self, Action},
    audio::RenderedSnapshot,
    model::{Mode, Page},
    render::{Primitive, Scene},
};
pub use provider::Provider;
#[cfg(test)]
pub(crate) use provider::exercise_held_worker;
#[cfg(test)]
use provider::{Latest, Request, confirmed_lease_remaining, publish_provider_update};
#[cfg(test)]
pub(crate) use provider::{refresh_worker_context, service_worker_hold};
use serde_json::{Value, json};
#[cfg(test)]
use std::sync::{
    Arc,
    atomic::{AtomicBool, AtomicU64, AtomicUsize},
    mpsc::{self, Receiver},
};
use std::{
    collections::{BTreeSet, VecDeque},
    path::PathBuf,
    sync::atomic::Ordering,
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
    Fx(crate::fx::Operation),
    Measurement(crate::pa_measurement::editor::Operation),
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
pub struct Update {
    pub measurement: crate::pa_measurement::wire::State,
    pub fx: crate::fx::State,
    pub fx_age_ms: Option<u64>,
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
    pub fn fx_is_fresh(&self) -> bool {
        self.raw_fresh()
            && self.fx_age_ms.is_some_and(|age| {
                age.saturating_add(self.received.elapsed().as_millis() as u64) <= 250
            })
            && self.fx.snapshot.as_ref().is_some_and(|s| {
                s.available
                    && s.pending.is_none()
                    && s.observation
                        .as_ref()
                        .is_some_and(crate::fx::Observation::settled)
                    && self.snapshot.as_ref().is_some_and(|r| {
                        s.revision == r.authority.revision
                            && s.epoch == r.authority.epoch
                            && s.show_id == r.authority.show_id
                    })
            })
    }
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
#[derive(Clone, Debug)]
pub struct ExactDraft {
    pub input: String,
    pub parameter: crate::exact_value::Parameter,
    pub text: String,
    pub value: Option<i32>,
    revision: String,
    generation: u64,
    show: String,
    epoch: String,
    scope: String,
    validated: bool,
    review_requested: bool,
    confirmation_requested: bool,
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
    pub exact_draft: Option<ExactDraft>,
    pub fx_ui: crate::fx::Editor,
    pub measurement_ui: crate::pa_measurement::editor::State,
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
            exact_draft: None,
            fx_ui: Default::default(),
            measurement_ui: Default::default(),
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
        self.fx_ui.basis = None;
        self.provider.fence();
        self.observed_generation = self.provider.generation();
        self.fence_local();
    }
    fn fence_local(&mut self) {
        self.fx_ui.basis = None;
        if let Some(s) = &self.state
            && s.fx.snapshot.is_some()
        {
            self.fx_ui.report = s.fx.lines();
            self.fx_ui.report.insert(0,"RETAINED FX EVIDENCE / stale after input-context change / explicit probe and NEW review required".into());
        }
        if let Some(s) = &self.state
            && s.measurement.snapshot.is_some()
        {
            self.measurement_ui.report = s.measurement.lines();
            self.measurement_ui.report.insert(0,"RETAINED DESCRIPTION / stale after input-context change / explicit probe and new review required".into());
        }
        self.talkback_release();
        if let Some(d) = &mut self.exact_draft {
            d.validated = false;
            d.review_requested = false;
            d.confirmation_requested = false;
        }
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
                        Action::ExactEdit(_)
                            | Action::SendTapEdit
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
                        && self.send_draft.is_none()
                        && self.exact_draft.is_none())
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
                            && self.send_draft.is_none()
                            && self.exact_draft.is_none())
                            || matches!(
                                key.as_str(),
                                "E" | "S" | "L" | "D" | "C" | "d" | "c" | "F9" | "F11"
                            ))
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
        if self
            .exact_draft
            .as_ref()
            .is_some_and(|d| d.confirmation_requested)
            && update.review.is_none()
            && self.exact_draft.as_ref().is_some_and(|d| {
                d.revision
                    .parse::<u64>()
                    .ok()
                    .and_then(|r| r.checked_add(1))
                    .is_some_and(|r| {
                        update.last_operation.as_deref()
                            == Some(format!("set applied revision {r}").as_str())
                    })
            })
        {
            self.exact_draft = None;
        }
        if let Some(d) = &mut self.exact_draft
            && d.review_requested
            && update.review.is_none()
            && update
                .last_operation
                .as_deref()
                .is_some_and(|r| r.starts_with("REFUSED/") || r.starts_with("COMPLETED/"))
        {
            d.validated = false;
            d.review_requested = false;
            d.confirmation_requested = false;
        }
        self.state = Some(update);
    }
    /// Current explicitly selected authority scope; attachment freshness is separate.
    pub fn scope(&self) -> &str {
        &self.scope
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
        if self.fx_ui.open && matches!(key, "Esc" | "Escape") {
            return self.action(Action::Cancel);
        }
        if self.fx_ui.open
            && self.fx_ui.text.is_none()
            && self.state.as_ref().is_none_or(|s| s.review.is_none())
            && key.eq_ignore_ascii_case("g")
        {
            return self.send(Operation::Grant);
        }
        if self.fx_ui.open
            && self.fx_ui.text.is_none()
            && self.state.as_ref().is_none_or(|s| s.review.is_none())
            && key.eq_ignore_ascii_case("q")
        {
            return self.send(Operation::ReleaseWriter);
        }
        if let Some(a) = self.fx_key(key) {
            return self.action(Action::Fx(a));
        }
        if let Some(action) = self.measurement_key(key) {
            return self.action(Action::Measurement(action));
        }
        if key == "F8" {
            return self.reconnect_legacy();
        }
        if key == "F5" {
            self.fence();
            self.attachment_fence = Some(self.provider.generation());
            return self.provider.send(None, Operation::Reconnect);
        }
        if self.exact_draft.is_some() {
            let normalized = key.to_ascii_uppercase();
            match normalized.as_str() {
                "G" => return self.send(Operation::Grant),
                "Q" => return self.send(Operation::ReleaseWriter),
                "ESC" => return self.action(Action::Cancel),
                "F4" => return self.action(Action::ExactApply),
                "D" => return self.action(Action::ExactEdit(crate::exact_value::Parameter::Fader)),
                "C" => return self.action(Action::ExactEdit(crate::exact_value::Parameter::Pan)),
                _ => (),
            }
            if !self.exact_draft.as_ref().unwrap().review_requested {
                match key {
                    "Enter" => {
                        return self.action(Action::ExactText(
                            self.exact_draft.as_ref().unwrap().text.clone(),
                        ));
                    }
                    "Backspace" => {
                        let d = self.exact_draft.as_mut().unwrap();
                        d.text.pop();
                        d.value = None;
                        return Ok(());
                    }
                    _ if key.len() == 1
                        && key
                            .bytes()
                            .all(|b| b.is_ascii_digit() || matches!(b, b'.' | b'-' | b'+')) =>
                    {
                        let d = self.exact_draft.as_mut().unwrap();
                        if d.text.len() >= 16 {
                            return Err("exact entry capacity16".into());
                        }
                        d.text.push_str(key);
                        d.value = None;
                        return Ok(());
                    }
                    _ => (),
                }
            }
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
        if matches!(
            self.scope.as_str(),
            "pa_configuration" | "output_routes" | "fx_configuration"
        ) {
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
        if self.scope == "foh"
            && !self.sends_page
            && !self.brain_page
            && !self.live_page
            && self.topology_page.is_none()
            && matches!(self.page, Page::Mix | Page::Channel)
            && self.processing_draft.is_none()
            && self.structural_draft.is_none()
        {
            match key {
                "D" => return self.action(Action::ExactEdit(crate::exact_value::Parameter::Fader)),
                "C" => return self.action(Action::ExactEdit(crate::exact_value::Parameter::Pan)),
                _ => (),
            }
        }
        if key.eq_ignore_ascii_case("w") {
            return self.action(Action::Fx(crate::fx::Action::Open));
        }
        if key.eq_ignore_ascii_case("y") {
            return self.action(Action::Measurement(
                crate::pa_measurement::editor::Action::Open,
            ));
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
        if matches!(
            action,
            Action::Page(_) | Action::SendsPage | Action::BrainPage | Action::Topology
        ) {
            self.measurement_ui.open = false;
            self.fx_ui.open = false;
        }
        if self.exact_draft.is_some()
            && !matches!(
                action,
                Action::ExactEdit(_)
                    | Action::ExactText(_)
                    | Action::ExactApply
                    | Action::Confirm
                    | Action::Cancel
                    | Action::Back
                    | Action::Move(_)
                    | Action::Bank(_)
                    | Action::Page(_)
                    | Action::SwitchScope(_)
                    | Action::Menu
                    | Action::Status
            )
        {
            return Err("Apply/confirm or Esc cancel exact draft first".into());
        }
        match action {
            Action::Fx(a) => self.fx_action(a),
            Action::Measurement(action) => self.measurement_action(action),
            Action::ExactEdit(parameter) => {
                if self.scope != "foh"
                    || !matches!(self.page, Page::Mix | Page::Channel)
                    || self.sends_page
                    || self.brain_page
                    || self.live_page
                    || self.topology_page.is_some()
                    || !self.fresh()
                    || self.mode_picker
                    || self.provider.pending_edits.load(Ordering::Acquire) != 0
                    || self
                        .provider
                        .latest
                        .update
                        .lock()
                        .unwrap()
                        .as_ref()
                        .is_some_and(|u| {
                            u.review.is_some()
                                || u.last_operation
                                    .as_deref()
                                    .is_some_and(|r| r.starts_with("PENDING"))
                        })
                    || self.processing_draft.is_some()
                    || self.structural_draft.is_some()
                    || self.device_draft.is_some()
                    || self.send_draft.is_some()
                    || self.state.as_ref().is_some_and(|u| {
                        u.review.is_some()
                            || u.last_operation
                                .as_deref()
                                .is_some_and(|r| r.starts_with("PENDING"))
                    })
                    || self
                        .exact_draft
                        .as_ref()
                        .is_some_and(|d| d.review_requested)
                {
                    return Err("exact editor requires fresh FOH Mix/Channel and no other draft/review/pending".into());
                }
                let input = self.selected_input().ok_or("input unavailable")?.to_owned();
                let raw = self.state.as_ref().unwrap().snapshot.as_ref().unwrap();
                if !raw.authority.parameters.iter().any(|p| {
                    p.target.input == input
                        && p.target.parameter == parameter.name()
                        && p.target.monitor.is_none()
                }) {
                    return Err("parameter unavailable".into());
                }
                let retained = self.exact_draft.as_ref().filter(|d| {
                    d.input == input
                        && d.parameter == parameter
                        && d.show == raw.authority.show_id
                        && d.epoch == raw.authority.epoch
                });
                if self.exact_draft.is_some() && retained.is_none() {
                    return Err("exact draft identity changed; Esc cancels before reopening".into());
                }
                self.exact_draft = Some(ExactDraft {
                    input,
                    parameter,
                    text: retained.map_or_else(String::new, |d| d.text.clone()),
                    value: retained.and_then(|d| d.value),
                    revision: raw.authority.revision.clone(),
                    generation: self.provider.generation(),
                    show: raw.authority.show_id.clone(),
                    epoch: raw.authority.epoch.clone(),
                    scope: self.scope.clone(),
                    validated: true,
                    review_requested: false,
                    confirmation_requested: false,
                });
                self.message = "Detached exact draft: Enter accepts text; F4 review; presented review + Enter sends".into();
                Ok(())
            }
            Action::ExactText(text) => {
                let d = self.exact_draft.as_mut().ok_or("D/C opens exact draft")?;
                if d.review_requested {
                    return Err("review already queued; Esc cancels only unsent intent".into());
                }
                let value = d.parameter.parse(&text)?;
                d.text = text;
                d.value = Some(value);
                self.message = "Exact text accepted locally; F4 opens complete review".into();
                Ok(())
            }
            Action::ExactApply => {
                let d = self.exact_draft.as_ref().ok_or("no exact draft")?;
                if d.review_requested || !self.exact_context_valid(d) {
                    return Err("exact draft context changed; select original input and D/C revalidate, then NEW F4 review".into());
                }
                let value = d.value.ok_or("Enter accepts exact text before F4 review")?;
                if d.parameter.parse(&d.text)? != value {
                    return Err("exact text changed; accept again".into());
                }
                let target = json!({"input":d.input,"parameter":d.parameter.name()});
                self.send(Operation::ReviewSet {
                    target,
                    value: json!(value),
                })?;
                self.exact_draft.as_mut().unwrap().review_requested = true;
                Ok(())
            }
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
                    && !matches!(
                        scope.as_str(),
                        "pa_configuration" | "output_routes" | "fx_configuration"
                    )
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
                if let Some(d) = &self.exact_draft
                    && (!d.review_requested
                        || d.confirmation_requested
                        || !self.exact_context_valid(d))
                {
                    return Err("exact review context changed; revalidate original identity and request NEW review".into());
                }
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
                self.send(Operation::Confirm(id))?;
                if let Some(d) = &mut self.exact_draft {
                    d.confirmation_requested = true;
                }
                Ok(())
            }
            Action::Cancel | Action::Back => {
                self.fx_ui.text = None;
                self.fx_ui.basis = None;
                if self.measurement_ui.editor.take().is_some() {
                    self.measurement_ui.text.clear();
                } else if self.state.as_ref().is_none_or(|s| s.review.is_none()) {
                    self.measurement_ui.open = false;
                }
                let submitted = self
                    .exact_draft
                    .as_ref()
                    .is_some_and(|d| d.confirmation_requested);
                self.exact_draft = None;
                self.send_draft = None;
                self.send_entry = None;
                self.processing_draft = None;
                self.device_draft = None;
                self.device_draft_context = None;
                self.device_entry = None;
                self.structural_draft = None;
                self.structure_text_entry = false;
                self.processing_entry.clear();
                let fx_submitted = self.state.as_ref().is_some_and(|u| {
                    u.fx.unknown || u.status.starts_with("PENDING reviewed operation")
                });
                self.send_cancel();
                if self.fx_ui.open && fx_submitted {
                    self.message="Unsent FX content discarded; sent FX operation is not cancelled or undone; retain unknown and obtain explicit fresh owner readback".into();
                }
                if submitted {
                    self.message = "Unsent draft discarded; submitted mutation is NOT cancelled or undone; fresh readback required".into();
                }
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
                    "D exact fader dB / C exact pan L-center-R / Enter accept / F4 review / G grant / Q release / F5 reconnect".into();
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
    fn exact_context_valid(&self, d: &ExactDraft) -> bool {
        d.validated
            && d.generation == self.provider.generation()
            && self.selected_input() == Some(d.input.as_str())
            && self.scope == d.scope
            && self.scope == "foh"
            && self.focused
            && self.device_ready
            && self.width > 0
            && self.height > 0
            && self.fresh()
            && (!self.role_required || self.provider.authorization.load(Ordering::Acquire))
            && matches!(self.page, Page::Mix | Page::Channel)
            && !self.sends_page
            && !self.brain_page
            && !self.live_page
            && self.topology_page.is_none()
            && self.state.as_ref().is_some_and(|u| {
                u.writer_granted()
                    && u.snapshot.as_ref().is_some_and(|raw| {
                        raw.authority.revision == d.revision
                            && raw.authority.show_id == d.show
                            && raw.authority.epoch == d.epoch
                    })
            })
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
        if self.fx_ui.open {
            return self.fx_scene();
        }
        if self.measurement_ui.open {
            return self.measurement_scene();
        }
        if let Some(d) = &self.exact_draft {
            line(
                12,
                format!(
                    "DETACHED FOH EXACT DRAFT / {} / {}",
                    d.input,
                    d.parameter.name()
                ),
                "#66dfd3",
            );
            line(60, "Local text and proposed value are separate from committed target, current coefficients and pending".into(), "#e4e8e9");
            line(108, format!("Entry: {}", d.text), "#66dfd3");
            line(
                156,
                format!(
                    "Proposed: {}",
                    d.value
                        .map_or_else(|| "not accepted".into(), |v| d.parameter.display(v))
                ),
                "#f1bd6b",
            );
            line(204, "Fader -60.0..+12.0 dB step0.1 | Pan -100..100 integer: negative L / 0 center / positive R".into(), "#e4e8e9");
            if let Some(raw) = self.state.as_ref().and_then(|u| u.snapshot.as_ref()) {
                if let Some(p) = raw.authority.parameters.iter().find(|p| {
                    p.target.input == d.input
                        && p.target.parameter == d.parameter.name()
                        && p.target.monitor.is_none()
                }) {
                    line(
                        252,
                        format!(
                            "Committed target: {} / hold {} / owner {}",
                            parameter_display(d.parameter.name(), &p.target_value),
                            optional_display(d.parameter.name(), &p.hold),
                            p.owner.as_deref().unwrap_or("none")
                        ),
                        "#e4e8e9",
                    );
                }
                if let Some(c) = raw.coefficients.iter().find(|c| c.input == d.input) {
                    line(
                        300,
                        format!(
                            "Current coefficients (linear, not meters): {}",
                            c.current_nanogain
                                .iter()
                                .map(|n| linear_display(*n))
                                .collect::<Vec<_>>()
                                .join(" / ")
                        ),
                        "#e4e8e9",
                    );
                }
            }
            line(
                348,
                format!(
                    "Pending/review requested: {} / context validated: {} / fresh: {}",
                    d.review_requested,
                    self.exact_context_valid(d),
                    self.fresh()
                ),
                "#f1bd6b",
            );
            line(888,"Enter accepts text only | F4 NEW complete review | presented review + Enter confirms | Esc unsent cancel".into(),"#66dfd3");
            line(936,"D/C revalidate original identity | G explicit grant | Q release | F5 reconnect read-only".into(),"#e4e8e9");
            line(984, self.message.clone(), "#f47c85");
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
mod tests;

#[cfg(test)]
mod processing_tests;

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
mod gp18_ui_tests;
