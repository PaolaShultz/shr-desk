//! Bounded, headless console runtime. No devices, transport or engine authority.
use crate::{
    actions::{self, Action, Inputs},
    midi::Color,
    model::{Command, Desk, Snapshot},
    render,
};
use std::collections::VecDeque;

#[derive(Clone, Debug)]
pub enum Event {
    Key { key: String, pressed: bool },
    TextFocus(bool),
    Text(String),
    Midi(Vec<u8>),
    Resize { width: u32, height: u32 },
    Focus(bool),
    DeviceLost,
    DeviceRestored,
}
#[derive(Clone, Debug)]
pub struct Envelope {
    pub generation: u64,
    pub event: Event,
}
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct LedState {
    pub generation: u64,
    pub colors: [Color; 8],
}
/// Independent single-slot desired-state queue: slow LED consumers never block input.
#[derive(Default)]
pub struct LedMailbox {
    latest: Option<LedState>,
}
impl LedMailbox {
    pub fn offer(&mut self, state: LedState) {
        self.latest = Some(state);
    }
    pub fn take(&mut self, generation: u64) -> Option<LedState> {
        self.latest.take().filter(|s| s.generation == generation)
    }
}
pub trait Renderer {
    fn resize(&mut self, width: u32, height: u32);
    fn device_lost(&mut self);
    fn restore(&mut self);
    fn present(&mut self, desk: &Desk) -> Option<String>;
}
/// Uses existing scene/font primitives; zero-sized or lost surfaces suspend presentation.
pub struct HeadlessRenderer {
    width: u32,
    height: u32,
    available: bool,
}
impl Default for HeadlessRenderer {
    fn default() -> Self {
        Self {
            width: 1920,
            height: 1080,
            available: true,
        }
    }
}
impl Renderer for HeadlessRenderer {
    fn resize(&mut self, width: u32, height: u32) {
        self.width = width;
        self.height = height;
    }
    fn device_lost(&mut self) {
        self.available = false;
    }
    fn restore(&mut self) {
        self.available = true;
    }
    fn present(&mut self, desk: &Desk) -> Option<String> {
        if !self.available || self.width == 0 || self.height == 0 {
            return None;
        }
        Some(render::svg(&render::scene(desk)).replacen(
            "width=\"1920\" height=\"1080\"",
            &format!("width=\"{}\" height=\"{}\"", self.width, self.height),
            1,
        ))
    }
}
#[derive(Default)]
pub struct PumpResult {
    pub commands: Vec<Command>,
    pub refusals: Vec<String>,
    pub frame: Option<String>,
}
pub struct Console<R: Renderer = HeadlessRenderer> {
    pub desk: Desk,
    inputs: Inputs,
    queue: VecDeque<Envelope>,
    capacity: usize,
    generation: u64,
    assigned: bool,
    focused: bool,
    dirty: bool,
    pub leds: LedMailbox,
    renderer: R,
}
impl<R: Renderer> Console<R> {
    pub fn new(desk: Desk, renderer: R, capacity: usize) -> Self {
        assert!(capacity > 0);
        Self {
            desk,
            inputs: Inputs::default(),
            queue: VecDeque::with_capacity(capacity),
            capacity,
            generation: 0,
            assigned: false,
            focused: true,
            dirty: true,
            leds: LedMailbox::default(),
            renderer,
        }
    }
    pub fn generation(&self) -> u64 {
        self.generation
    }
    pub fn queued(&self) -> usize {
        self.queue.len()
    }
    fn fence(&mut self) {
        self.queue.retain(|envelope| {
            matches!(
                envelope.event,
                Event::Resize { .. } | Event::Focus(_) | Event::DeviceLost | Event::DeviceRestored
            )
        });
        self.inputs.context_loss(&mut self.desk);
        self.dirty = true;
    }
    /// Synthetic headless assignment only: no C-ROLE registry validation, native
    /// acceptance, physical binding or state recall. Exhaustion permanently revokes
    /// assignment so an old generation can never become valid again.
    pub fn assign(&mut self, audio_role: bool) {
        let next = self.generation.checked_add(1);
        self.assigned = audio_role && next.is_some();
        if let Some(next) = next {
            self.generation = next;
        } else {
            self.desk.last_result =
                "synthetic role generation exhausted; assignment revoked".into();
        }
        self.fence();
        self.leds.latest = None;
    }
    pub fn disconnect(&mut self) {
        self.desk.disconnect();
        self.fence();
    }
    pub fn reconnect(&mut self, snapshot: Snapshot) {
        self.desk.reconnect(snapshot);
        self.fence();
    }
    /// Transition to an explicit read-only provider: discard every pending local
    /// simulator intent and reset shared release/pickup gates. Provider snapshots
    /// are never converted into synthetic measured channel state.
    pub fn reconnect_provider(&mut self, client: &mut crate::provider::Client, epoch: u64) {
        self.disconnect();
        client.reconnect(epoch);
    }
    pub fn enqueue(&mut self, envelope: Envelope) -> Result<(), String> {
        if !self.assigned || envelope.generation != self.generation {
            return Err("stale or absent audio role assignment".into());
        }
        if let Event::Midi(bytes) = &envelope.event
            && bytes.len() > 3
        {
            return Err("only bounded complete MIDI messages supported".into());
        }
        if let Event::Text(text) = &envelope.event
            && text.len() > 64
        {
            return Err("text action too long".into());
        }
        if let Event::Key { key, .. } = &envelope.event
            && key.len() > 32
        {
            return Err("key name too long".into());
        }
        if self.queue.len() == self.capacity {
            self.fence();
            self.desk.last_result =
                "INPUT OVERFLOW: drafts cancelled; release and pickup required".into();
            return Err("input queue overflow".into());
        }
        self.queue.push_back(envelope);
        Ok(())
    }
    pub fn pump(&mut self) -> PumpResult {
        let mut result = PumpResult::default();
        while let Some(envelope) = self.queue.pop_front() {
            if !self.assigned || envelope.generation != self.generation {
                continue;
            }
            let outcome = match envelope.event {
                Event::Key { key, pressed } => {
                    if !self.focused && pressed {
                        continue;
                    }
                    self.inputs.key(&mut self.desk, &key, pressed)
                }
                Event::Midi(bytes) => {
                    if !self.focused {
                        continue;
                    }
                    self.inputs.midi(&mut self.desk, &bytes)
                }
                Event::Text(text) => {
                    if !self.focused {
                        continue;
                    }
                    let action = match text.trim() {
                        "hold" => Some(Action::Hold),
                        "release" => Some(Action::Release),
                        "mute" => Some(Action::Mute),
                        "confirm" => Some(Action::Confirm),
                        "cancel" => Some(Action::Cancel),
                        "mode" => Some(Action::ModePicker),
                        "auto" => Some(Action::ChooseMode(crate::model::Mode::Auto)),
                        "assist" => Some(Action::ChooseMode(crate::model::Mode::Assist)),
                        "manual" => Some(Action::ChooseMode(crate::model::Mode::Manual)),
                        "mix" => Some(Action::Page(crate::model::Page::Mix)),
                        "channel" => Some(Action::Page(crate::model::Page::Channel)),
                        "analysis" => Some(Action::Page(crate::model::Page::Analysis)),
                        _ => None,
                    };
                    match action {
                        Some(action) => actions::apply(&mut self.desk, action),
                        None => Err("text action unavailable".into()),
                    }
                }
                Event::TextFocus(value) => {
                    if self.inputs.text_focus != value {
                        self.inputs.context_loss(&mut self.desk);
                    }
                    self.inputs.text_focus = value;
                    Ok(None)
                }
                Event::Resize { width, height } => {
                    self.renderer.resize(width, height);
                    Ok(None)
                }
                Event::Focus(value) => {
                    self.focused = value;
                    if !value {
                        self.fence();
                    }
                    Ok(None)
                }
                Event::DeviceLost => {
                    self.renderer.device_lost();
                    self.fence();
                    Ok(None)
                }
                Event::DeviceRestored => {
                    self.renderer.restore();
                    Ok(None)
                }
            };
            match outcome {
                Ok(Some(command)) => result.commands.push(command),
                Ok(None) => {}
                Err(error) => {
                    self.desk.last_result = format!("REFUSED: {error}");
                    result.refusals.push(error);
                }
            }
            self.dirty = true;
        }
        if self.dirty {
            result.frame = self.renderer.present(&self.desk);
            self.feedback();
            self.dirty = false;
        }
        result
    }
    /// Call after a correlated provider response updates Desk. Input never applies commands.
    pub fn refresh(&mut self) {
        self.dirty = true;
    }
    fn feedback(&mut self) {
        if !self.assigned {
            return;
        }
        let colors = crate::midi::pad_colors(&self.desk);
        self.leds.offer(LedState {
            generation: self.generation,
            colors,
        });
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn role_generation_exhaustion_permanently_revokes_assignment() {
        let mut c = Console::new(
            Desk::new(Snapshot::fixture()),
            HeadlessRenderer::default(),
            8,
        );
        c.generation = u64::MAX - 1;
        c.assign(true);
        assert_eq!(c.generation(), u64::MAX);
        c.assign(true);
        assert!(!c.assigned);
        c.assign(true);
        assert_eq!(c.generation(), u64::MAX);
        assert!(
            c.enqueue(Envelope {
                generation: u64::MAX,
                event: Event::Text("hold".into())
            })
            .is_err()
        );
    }
}
