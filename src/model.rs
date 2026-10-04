pub use crate::actions;

use std::collections::VecDeque;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Mode {
    Auto,
    Assist,
    Manual,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Page {
    Mix,
    Channel,
    Analysis,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Channel {
    pub id: u32,
    pub name: String,
    pub gain_mdb: i32,
    pub proposed_mdb: i32,
    pub pan: i16,
    pub muted: bool,
    pub held: bool,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Snapshot {
    pub epoch: u64,
    pub revision: u64,
    pub mode: Mode,
    pub channels: Vec<Channel>,
}

impl Snapshot {
    pub fn fixture() -> Self {
        let names = [
            "Kick", "Snare", "Hi hat", "Tom 1", "Tom 2", "OH L", "OH R", "Bass", "Guitar L",
            "Guitar R", "Lead vox", "Back vox",
        ];
        Self {
            epoch: 1,
            revision: 0,
            mode: Mode::Auto,
            channels: (0..36)
                .map(|i| Channel {
                    id: i + 1,
                    name: if i < 12 {
                        names[i as usize].to_string()
                    } else {
                        format!("Input {}", i + 1)
                    },
                    gain_mdb: -6000 - (i as i32 % 5) * 1000,
                    proposed_mdb: -6000 - (i as i32 % 5) * 1000,
                    pan: match i {
                        5 | 8 => -65,
                        6 | 9 => 65,
                        _ => 0,
                    },
                    muted: false,
                    held: false,
                })
                .collect(),
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Edit {
    Gain { channel: u32, mdb: i32 },
    Pan { channel: u32, value: i16 },
    Mute { channel: u32, value: bool },
    Hold { channel: u32 },
    Release { channel: u32 },
    Mode(Mode),
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Command {
    pub id: u64,
    pub epoch: u64,
    pub expected_revision: u64,
    pub edit: Edit,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Rejection {
    Disconnected,
    Busy,
    Epoch,
    Revision,
    UnknownChannel,
    Range,
    Manual,
    ReusedId,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Ack {
    pub id: u64,
    pub epoch: u64,
    pub result: Result<u64, Rejection>,
}

/// Test authority only; intentionally not a live engine contract or wire protocol.
pub struct Simulator {
    pub state: Snapshot,
    seen: VecDeque<(Command, Ack)>,
    high_id: u64,
}

impl Default for Simulator {
    fn default() -> Self {
        Self {
            state: Snapshot::fixture(),
            seen: VecDeque::new(),
            high_id: 0,
        }
    }
}

impl Simulator {
    pub fn apply(&mut self, command: &Command) -> Ack {
        if command.epoch != self.state.epoch {
            return Ack {
                id: command.id,
                epoch: self.state.epoch,
                result: Err(Rejection::Epoch),
            };
        }
        if let Some((prior, ack)) = self.seen.iter().find(|(c, _)| c.id == command.id) {
            return if prior == command {
                ack.clone()
            } else {
                Ack {
                    id: command.id,
                    epoch: self.state.epoch,
                    result: Err(Rejection::ReusedId),
                }
            };
        }
        let result = if command.id <= self.high_id {
            Err(Rejection::ReusedId)
        } else if command.expected_revision != self.state.revision {
            Err(Rejection::Revision)
        } else {
            self.edit(&command.edit)
        };
        let ack = Ack {
            id: command.id,
            epoch: self.state.epoch,
            result,
        };
        self.high_id = self.high_id.max(command.id);
        if self.seen.len() == 64 {
            self.seen.pop_front();
        }
        self.seen.push_back((command.clone(), ack.clone()));
        ack
    }

    fn edit(&mut self, edit: &Edit) -> Result<u64, Rejection> {
        if let Edit::Mode(mode) = edit {
            // Changing automation mode always freezes current values. Release is explicit.
            if *mode != self.state.mode {
                for c in &mut self.state.channels {
                    c.held = true;
                }
            }
            self.state.mode = *mode;
        } else {
            let id = match *edit {
                Edit::Gain { channel, .. }
                | Edit::Pan { channel, .. }
                | Edit::Mute { channel, .. }
                | Edit::Hold { channel }
                | Edit::Release { channel } => channel,
                Edit::Mode(_) => unreachable!(),
            };
            let c = self
                .state
                .channels
                .iter_mut()
                .find(|c| c.id == id)
                .ok_or(Rejection::UnknownChannel)?;
            match *edit {
                Edit::Gain { mdb, .. } => {
                    if !(-90000..=12000).contains(&mdb) {
                        return Err(Rejection::Range);
                    }
                    c.gain_mdb = mdb;
                    c.held = true;
                }
                Edit::Pan { value, .. } => {
                    if !(-100..=100).contains(&value) {
                        return Err(Rejection::Range);
                    }
                    c.pan = value;
                }
                Edit::Mute { value, .. } => c.muted = value,
                Edit::Hold { .. } => c.held = true,
                Edit::Release { .. } => {
                    if self.state.mode != Mode::Auto {
                        return Err(Rejection::Manual);
                    }
                    c.held = false;
                    c.gain_mdb = c.proposed_mdb;
                }
                Edit::Mode(_) => unreachable!(),
            }
        }
        self.state.revision += 1;
        Ok(self.state.revision)
    }

    pub fn propose(&mut self, channel: u32, mdb: i32) -> Result<(), Rejection> {
        if !(-90000..=12000).contains(&mdb) {
            return Err(Rejection::Range);
        }
        let c = self
            .state
            .channels
            .iter_mut()
            .find(|c| c.id == channel)
            .ok_or(Rejection::UnknownChannel)?;
        c.proposed_mdb = mdb;
        if self.state.mode == Mode::Auto && !c.held {
            c.gain_mdb = mdb;
        }
        self.state.revision += 1;
        Ok(())
    }
}

pub struct Desk {
    pub confirmed: Snapshot,
    pub selected: u32,
    pub page: Page,
    pub connected: bool,
    pending: Option<Command>,
    next_id: u64,
    pub last_result: String,
    pub draft: Option<actions::Draft>,
    pub context_generation: u64,
    pub release_generation: u64,
}

impl Desk {
    pub fn new(snapshot: Snapshot) -> Self {
        let selected = snapshot.channels.first().map_or(0, |c| c.id);
        Self {
            confirmed: snapshot,
            selected,
            page: Page::Mix,
            connected: true,
            pending: None,
            draft: None,
            context_generation: 0,
            release_generation: 0,
            next_id: 1,
            last_result: "SIMULATION: no live engine connected".into(),
        }
    }
    pub fn selected(&self) -> Option<&Channel> {
        self.confirmed
            .channels
            .iter()
            .find(|c| c.id == self.selected)
    }
    pub fn select(&mut self, id: u32) -> bool {
        if self.confirmed.channels.iter().any(|c| c.id == id) {
            self.invalidate_context();
            self.selected = id;
            true
        } else {
            false
        }
    }
    pub fn invalidate_context(&mut self) {
        self.draft = None;
        self.context_generation = self.context_generation.wrapping_add(1);
    }
    pub fn set_page(&mut self, page: Page) {
        self.invalidate_context();
        self.page = page;
    }
    pub fn bank_start(&self) -> usize {
        self.confirmed
            .channels
            .iter()
            .position(|c| c.id == self.selected)
            .unwrap_or(0)
            / 12
            * 12
    }
    pub fn begin(&mut self, edit: Edit) -> Result<Command, Rejection> {
        if !self.connected {
            return Err(Rejection::Disconnected);
        }
        if self.pending.is_some() {
            return Err(Rejection::Busy);
        }
        let c = Command {
            id: self.next_id,
            epoch: self.confirmed.epoch,
            expected_revision: self.confirmed.revision,
            edit,
        };
        self.next_id += 1;
        self.pending = Some(c.clone());
        self.last_result = format!("PENDING command {}", c.id);
        Ok(c)
    }
    /// ACK alone never changes confirmed values; a correlated snapshot is required.
    pub fn resolve(&mut self, ack: &Ack, snapshot: Snapshot) -> bool {
        let Some(pending) = &self.pending else {
            return false;
        };
        if ack.id != pending.id
            || ack.epoch != pending.epoch
            || snapshot.epoch != ack.epoch
            || snapshot.revision < self.confirmed.revision
        {
            return false;
        }
        if let Ok(revision) = ack.result
            && (revision <= pending.expected_revision || revision > snapshot.revision)
        {
            return false;
        }
        self.last_result = match ack.result {
            Ok(r) => format!("APPLIED revision {r} (simulated)"),
            Err(e) => format!("REJECTED {e:?}; state refreshed"),
        };
        self.invalidate_context();
        self.confirmed = snapshot;
        self.pending = None;
        true
    }
    pub fn disconnect(&mut self) {
        self.release_generation = self.release_generation.wrapping_add(1);
        self.invalidate_context();
        self.connected = false;
        self.pending = None;
        self.last_result = "DISCONNECTED / state stale / pending discarded".into();
    }
    pub fn reconnect(&mut self, snapshot: Snapshot) {
        self.release_generation = self.release_generation.wrapping_add(1);
        self.invalidate_context();
        self.confirmed = snapshot;
        self.pending = None;
        self.connected = true;
        if !self
            .confirmed
            .channels
            .iter()
            .any(|c| c.id == self.selected)
        {
            self.selected = self.confirmed.channels.first().map_or(0, |c| c.id);
        }
        self.last_result = "SIMULATION resynchronized; no commands replayed".into();
    }
    pub fn pending(&self) -> Option<&Command> {
        self.pending.as_ref()
    }
}
