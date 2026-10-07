//! Shared offline interaction table. No engine or device bindings.
use crate::midi::{Action as MidiAction, Decoder, Pickup, Profile};
use crate::model::{Command, Desk, Edit, Mode, Page, Rejection};
use std::collections::BTreeSet;

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Action {
    MasterEqEdit,
    MasterEqChannel,
    MasterEqSection,
    DeviceEdit,
    DeviceText(String),
    DeviceApply,
    BrainPage,
    BrainSource(crate::brain::Source),
    BrainArm,
    BrainDim,
    BrainMute,
    BrainGain(i32),
    TalkbackConfigure {
        monitors: Vec<usize>,
        gain_cdb: i32,
        mute: bool,
    },
    TalkbackFoh(bool),
    TalkbackPress,
    TalkbackRelease,
    Topology,
    StructureEdit,
    StructureField(i32),
    StructureText(String),
    StructureImport(String),
    StructureAdjust(i32),
    StructureApply,
    OutputMute,
    OutputRearm,
    ProcessingEdit,
    ProcessingText(String),
    ProcessingField(i32),
    ProcessingAdjust(i32),
    ProcessingApply,
    Select(u32),
    Move(i32),
    Bank(i32),
    Page(Page),
    Edit(Edit),
    Adjust(i32),
    AdjustPan(i16),
    Mute,
    Hold,
    Release,
    ModePicker,
    ChooseMode(Mode),
    Confirm,
    Cancel,
    Back,
    Status,
    Menu,
}
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Intent {
    Edit(Edit),
    ModePicker,
    Menu,
}
/// Detached from confirmed state, pinned to the entire observed context.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Draft {
    pub intent: Intent,
    pub selected: u32,
    pub page: Page,
    pub epoch: u64,
    pub revision: u64,
    pub generation: u64,
}
impl Draft {
    fn new(d: &Desk, intent: Intent) -> Self {
        Self {
            intent,
            selected: d.selected,
            page: d.page,
            epoch: d.confirmed.epoch,
            revision: d.confirmed.revision,
            generation: d.context_generation,
        }
    }
    fn valid(&self, d: &Desk) -> bool {
        self.selected == d.selected
            && self.page == d.page
            && self.epoch == d.confirmed.epoch
            && self.revision == d.confirmed.revision
            && self.generation == d.context_generation
    }
}
/// Keyboard presses and MIDI pads consult these same semantic entries.
pub const BASE_PADS: [Action; 8] = [
    Action::Page(Page::Mix),
    Action::Page(Page::Channel),
    Action::Page(Page::Analysis),
    Action::Mute,
    Action::Hold,
    Action::Release,
    Action::ModePicker,
    Action::Menu,
];
pub fn pad_action(d: &Desk, pad: usize) -> Option<Action> {
    if matches!(d.draft.as_ref().map(|x| &x.intent), Some(Intent::Menu)) {
        return [
            Action::Bank(-1),
            Action::Bank(1),
            Action::Page(Page::Mix),
            Action::Page(Page::Channel),
            Action::Page(Page::Analysis),
            Action::Status,
            Action::Cancel,
            Action::Back,
        ]
        .get(pad)
        .cloned();
    }
    if matches!(
        d.draft.as_ref().map(|x| &x.intent),
        Some(Intent::ModePicker)
    ) {
        return [
            Action::ChooseMode(Mode::Auto),
            Action::ChooseMode(Mode::Assist),
            Action::ChooseMode(Mode::Manual),
            Action::Cancel,
            Action::Status,
            Action::Status,
            Action::Status,
            Action::Back,
        ]
        .get(pad)
        .cloned();
    }
    if d.draft.is_some() {
        return [
            Action::Confirm,
            Action::Cancel,
            Action::Status,
            Action::Status,
            Action::Status,
            Action::Status,
            Action::Status,
            Action::Back,
        ]
        .get(pad)
        .cloned();
    }
    BASE_PADS.get(pad).cloned()
}
pub fn key_action(key: &str) -> Option<Action> {
    Some(match key {
        "E" => Action::ProcessingEdit,
        "U" => Action::ProcessingField(-1),
        "I" => Action::ProcessingField(1),
        "J" => Action::ProcessingAdjust(-1),
        "K" => Action::ProcessingAdjust(1),
        "N" => Action::ProcessingAdjust(-100),
        "P" => Action::ProcessingAdjust(100),
        "F4" => Action::ProcessingApply,
        "F7" => Action::Topology,
        "F9" => Action::StructureEdit,
        "F11" => Action::MasterEqEdit,
        "F1" => Action::Page(Page::Mix),
        "F2" => Action::Page(Page::Channel),
        "F6" => Action::Page(Page::Analysis),
        "Left" | "Up" => Action::Move(-1),
        "Right" | "Down" | "Tab" => Action::Move(1),
        "PageUp" => Action::Bank(-1),
        "PageDown" => Action::Bank(1),
        "+" => Action::Adjust(1000),
        "-" => Action::Adjust(-1000),
        "[" => Action::AdjustPan(-1),
        "]" => Action::AdjustPan(1),
        "M" => Action::Mute,
        "H" => Action::Hold,
        "R" => Action::Release,
        "A" => Action::ModePicker,
        "F10" => Action::Menu,
        "Enter" => Action::Confirm,
        "Esc" => Action::Back,
        "1" => Action::ChooseMode(Mode::Auto),
        "2" => Action::ChooseMode(Mode::Assist),
        "3" => Action::ChooseMode(Mode::Manual),
        _ => return None,
    })
}
pub fn apply(d: &mut Desk, action: Action) -> Result<Option<Command>, String> {
    match action {
        Action::DeviceEdit
        | Action::DeviceText(_)
        | Action::DeviceApply
        | Action::BrainPage
        | Action::BrainSource(_)
        | Action::BrainArm
        | Action::BrainDim
        | Action::BrainMute
        | Action::BrainGain(_)
        | Action::TalkbackConfigure { .. }
        | Action::TalkbackFoh(_)
        | Action::TalkbackPress
        | Action::TalkbackRelease
        | Action::Topology
        | Action::StructureEdit
        | Action::MasterEqEdit
        | Action::MasterEqChannel
        | Action::MasterEqSection
        | Action::StructureField(_)
        | Action::StructureText(_)
        | Action::StructureImport(_)
        | Action::StructureAdjust(_)
        | Action::StructureApply
        | Action::OutputMute
        | Action::OutputRearm
        | Action::ProcessingEdit
        | Action::ProcessingText(_)
        | Action::ProcessingField(_)
        | Action::ProcessingAdjust(_)
        | Action::ProcessingApply => return Err("processing requires real GP07 provider".into()),
        Action::Select(id) => {
            if !d.select(id) {
                return Err("unknown channel".into());
            }
        }
        Action::Move(delta) | Action::Bank(delta) => {
            let step = if matches!(action, Action::Bank(_)) {
                12
            } else {
                1
            };
            let index = d
                .confirmed
                .channels
                .iter()
                .position(|c| c.id == d.selected)
                .ok_or("no channel")?;
            let next = (index as i64 + i64::from(delta) * step)
                .clamp(0, d.confirmed.channels.len().saturating_sub(1) as i64);
            let id = d.confirmed.channels[next as usize].id;
            d.select(id);
        }
        Action::Page(page) => d.set_page(page),
        Action::Cancel => {
            d.invalidate_context();
            d.last_result = "Draft cancelled".into();
        }
        Action::Back => {
            if d.draft.is_some() {
                d.invalidate_context();
            } else {
                d.set_page(Page::Mix);
            }
        }
        Action::Status => d.last_result = "SIMULATION status / hardware unopened".into(),
        Action::Confirm => {
            let draft = d.draft.as_ref().ok_or("no draft")?;
            if !draft.valid(d) {
                d.invalidate_context();
                return Err("stale draft".into());
            }
            let Intent::Edit(edit) = draft.intent.clone() else {
                return Err("choose a mode first".into());
            };
            d.draft = None;
            return d.begin(edit).map(Some).map_err(|e| format!("{e:?}"));
        }
        other => {
            if !d.connected {
                return Err(format!("{:?}", Rejection::Disconnected));
            }
            if d.pending().is_some() {
                return Err(format!("{:?}", Rejection::Busy));
            }
            if d.draft.is_some()
                && !matches!(
                    other,
                    Action::ChooseMode(_) | Action::Adjust(_) | Action::AdjustPan(_)
                )
            {
                return Err("another draft is open; confirm or cancel first".into());
            }
            let channel = d.selected;
            let intent = match other {
                Action::ModePicker => Intent::ModePicker,
                Action::Menu => Intent::Menu,
                Action::ChooseMode(mode) => {
                    let draft = d.draft.as_ref().ok_or("open mode picker first")?;
                    if !draft.valid(d) {
                        d.invalidate_context();
                        return Err("stale mode picker".into());
                    }
                    if !matches!(
                        draft.intent,
                        Intent::ModePicker | Intent::Edit(Edit::Mode(_))
                    ) {
                        return Err(
                            "open mode picker first; confirm or cancel current draft".into()
                        );
                    }
                    Intent::Edit(Edit::Mode(mode))
                }
                Action::Hold => Intent::Edit(Edit::Hold { channel }),
                Action::Release => Intent::Edit(Edit::Release { channel }),
                Action::Mute => Intent::Edit(Edit::Mute {
                    channel,
                    value: !d.selected().ok_or("no channel")?.muted,
                }),
                Action::Edit(edit) => Intent::Edit(edit),
                Action::AdjustPan(delta) => {
                    if d.page != Page::Mix {
                        return Err("pan unavailable; select Mix".into());
                    }
                    let base = match d.draft.as_ref() {
                        Some(draft) if draft.valid(d) => match draft.intent {
                            Intent::Edit(Edit::Pan { value, .. }) => value,
                            _ => return Err("another draft is open".into()),
                        },
                        Some(_) => {
                            d.invalidate_context();
                            return Err("stale draft".into());
                        }
                        None => d.selected().ok_or("no channel")?.pan,
                    };
                    Intent::Edit(Edit::Pan {
                        channel,
                        value: base.saturating_add(delta).clamp(-100, 100),
                    })
                }
                Action::Adjust(delta) => {
                    if d.page != Page::Mix {
                        return Err("edits unavailable on this draft; select Mix".into());
                    }
                    let base = match d.draft.as_ref() {
                        Some(draft) if draft.valid(d) => match draft.intent {
                            Intent::Edit(Edit::Gain { mdb, .. }) => mdb,
                            _ => return Err("another draft is open".into()),
                        },
                        Some(_) => {
                            d.invalidate_context();
                            return Err("stale draft".into());
                        }
                        None => d.selected().ok_or("no channel")?.gain_mdb,
                    };
                    Intent::Edit(Edit::Gain {
                        channel,
                        mdb: base.saturating_add(delta).clamp(-90000, 12000),
                    })
                }
                _ => unreachable!(),
            };
            let detail = match &intent {
                Intent::Edit(Edit::Release { channel }) => {
                    let c = d
                        .confirmed
                        .channels
                        .iter()
                        .find(|c| c.id == *channel)
                        .ok_or("no channel")?;
                    format!(
                        "CH {channel} gain {} -> {} mdb delta {:+}; simulated immediate, no engine ramp",
                        c.gain_mdb,
                        c.proposed_mdb,
                        c.proposed_mdb - c.gain_mdb
                    )
                }
                Intent::ModePicker => {
                    "P1 Auto / P2 Assist / P3 Manual / P4 Cancel / P8 Back".into()
                }
                Intent::Menu => {
                    "P1 Bank- / P2 Bank+ / P3 Mix / P4 Channel / P5 Analysis / P7 Cancel / P8 Back"
                        .into()
                }
                _ => format!("{intent:?}"),
            };
            d.draft = Some(Draft::new(d, intent));
            d.last_result = if matches!(
                d.draft.as_ref().unwrap().intent,
                Intent::ModePicker | Intent::Menu
            ) {
                format!("PREVIEW {detail} (SIMULATION)")
            } else {
                format!("PREVIEW {detail}; P1/Enter Confirm, P2/Esc Cancel (SIMULATION)")
            };
        }
    }
    Ok(None)
}

pub struct Inputs {
    decoder: Decoder,
    pickup: [Pickup; 16],
    generation: u64,
    release_generation: u64,
    keys: BTreeSet<String>,
    pub text_focus: bool,
}
impl Default for Inputs {
    fn default() -> Self {
        Self {
            decoder: Decoder::new(Profile::fixture()).unwrap(),
            pickup: std::array::from_fn(|_| Pickup::default()),
            generation: 0,
            release_generation: 0,
            keys: BTreeSet::new(),
            text_focus: false,
        }
    }
}
impl Inputs {
    fn sync(&mut self, d: &Desk) {
        if self.release_generation != d.release_generation {
            self.decoder.require_release();
            self.release_generation = d.release_generation;
        }
        if self.generation != d.context_generation {
            self.pickup = std::array::from_fn(|_| Pickup::default());
            self.generation = d.context_generation;
        }
    }
    pub fn context_loss(&mut self, d: &mut Desk) {
        d.invalidate_context();
        self.decoder.require_release();
        // Fence every supported key, including presses lost in an overflowed queue.
        for key in [
            "F1", "F2", "F6", "Left", "Up", "Right", "Down", "Tab", "PageUp", "PageDown", "+", "-",
            "[", "]", "M", "H", "R", "A", "F10", "Enter", "Esc", "1", "2", "3",
        ] {
            self.keys.insert(key.into());
        }
        self.text_focus = false;
        self.sync(d);
    }
    pub fn key(
        &mut self,
        d: &mut Desk,
        key: &str,
        pressed: bool,
    ) -> Result<Option<Command>, String> {
        self.sync(d);
        if !pressed {
            self.keys.remove(key);
            return Ok(None);
        }
        if key_action(key).is_none() {
            return Err("key unavailable".into());
        }
        if !self.keys.insert(key.into()) || self.text_focus {
            return Ok(None);
        }
        match key_action(key) {
            Some(a) => apply(d, a),
            None => Err("key unavailable".into()),
        }
    }
    pub fn midi(&mut self, d: &mut Desk, bytes: &[u8]) -> Result<Option<Command>, String> {
        self.sync(d);
        match self.decoder.decode(bytes) {
            Some(MidiAction::Select(id)) => apply(d, Action::Select(id)),
            Some(MidiAction::Pad(p)) => apply(d, pad_action(d, p).ok_or("pad unavailable")?),
            Some(MidiAction::Rotary { index, value }) => {
                if d.page != Page::Mix {
                    return Err("rotaries unavailable on this draft; select Mix".into());
                }
                if d.draft.is_some() {
                    return Err("confirm or cancel draft first".into());
                }
                let c = if index < 12 {
                    d.confirmed.channels.get(d.bank_start() + index)
                } else {
                    d.selected()
                }
                .ok_or("no target")?;
                let (parameter, current, normalized, edit) = match index {
                    0..=12 => (
                        0,
                        c.gain_mdb,
                        ((c.gain_mdb + 90000) * 127 / 102000) as u8,
                        Edit::Gain {
                            channel: c.id,
                            mdb: i32::from(value) * 102000 / 127 - 90000,
                        },
                    ),
                    13 => (
                        1,
                        i32::from(c.pan),
                        ((i32::from(c.pan) + 100) * 127 / 200) as u8,
                        Edit::Pan {
                            channel: c.id,
                            value: i16::from(value) * 200 / 127 - 100,
                        },
                    ),
                    _ => return Err("rotary unavailable".into()),
                };
                if self.pickup[index].accept((c.id, parameter, current), normalized, value) {
                    // Continuous Mix edits are immediate; protected actions use drafts.
                    apply(d, Action::Edit(edit))?;
                    apply(d, Action::Confirm)
                } else {
                    d.last_result = format!("PICKUP K{:02}: approach {normalized}/127", index + 1);
                    Ok(None)
                }
            }
            None => Ok(None),
        }
    }
}
