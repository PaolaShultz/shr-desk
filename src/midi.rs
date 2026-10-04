//! Pure MIDI message translation. This module never opens or writes a device.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Action {
    Select(u32),
    Pad(usize),
    Rotary { index: usize, value: u8 },
}

#[derive(Clone, Debug)]
pub struct Profile {
    pub key_channel: u8,
    pub pad_channel: u8,
    pub rotary_channel: u8,
    pub anchor_note: u8,
    pub channel_count: u32,
    pub pad_notes: [u8; 8],
    pub rotary_ccs: [u8; 16],
}

impl Profile {
    /// Fixture only. Actual MiniLab memory/messages must be learned, not assumed.
    pub fn fixture() -> Self {
        Self {
            key_channel: 0,
            pad_channel: 9,
            rotary_channel: 0,
            anchor_note: 48,
            channel_count: 36,
            pad_notes: [36, 37, 38, 39, 40, 41, 42, 43],
            rotary_ccs: [
                16, 17, 18, 19, 20, 21, 22, 23, 24, 25, 26, 27, 28, 29, 30, 31,
            ],
        }
    }
    pub fn validate(&self) -> bool {
        let unique = |xs: &[u8]| {
            xs.iter()
                .enumerate()
                .all(|(i, x)| *x < 128 && !xs[..i].contains(x))
        };
        self.key_channel < 16
            && self.pad_channel < 16
            && self.rotary_channel < 16
            && self.anchor_note < 128
            && self.channel_count > 0
            && self.channel_count <= u32::from(128 - self.anchor_note)
            && unique(&self.pad_notes)
            && unique(&self.rotary_ccs)
            && (self.key_channel != self.pad_channel
                || self.pad_notes.iter().all(|n| {
                    *n < self.anchor_note || u32::from(*n - self.anchor_note) >= self.channel_count
                }))
    }
}

pub struct Decoder {
    profile: Profile,
    down: [bool; 8],
}

impl Decoder {
    pub fn new(profile: Profile) -> Option<Self> {
        profile.validate().then_some(Self {
            profile,
            down: [false; 8],
        })
    }
    pub fn reset(&mut self) {
        self.down = [false; 8];
    }
    /// Focus/reconnect requires a release before a pad can act again.
    pub fn require_release(&mut self) {
        self.down = [true; 8];
    }
    /// Input is one complete MIDI 1.0 channel message, not a raw running-status stream.
    pub fn decode(&mut self, bytes: &[u8]) -> Option<Action> {
        if bytes.len() != 3 || bytes[0] < 0x80 || bytes[1] > 127 || bytes[2] > 127 {
            return None;
        }
        let channel = bytes[0] & 15;
        let kind = bytes[0] & 0xf0;
        let number = bytes[1];
        let value = bytes[2];
        if matches!(kind, 0x80 | 0x90)
            && channel == self.profile.pad_channel
            && let Some(i) = self.profile.pad_notes.iter().position(|n| *n == number)
        {
            let pressed = kind == 0x90 && value != 0;
            let edge = pressed && !self.down[i];
            self.down[i] = pressed;
            return edge.then_some(Action::Pad(i));
        }
        if kind == 0x90 && value > 0 && channel == self.profile.key_channel {
            let offset = number.checked_sub(self.profile.anchor_note)?;
            if u32::from(offset) < self.profile.channel_count {
                return Some(Action::Select(u32::from(offset) + 1));
            }
        }
        if kind == 0xb0 && channel == self.profile.rotary_channel {
            return self
                .profile
                .rotary_ccs
                .iter()
                .position(|c| *c == number)
                .map(|index| Action::Rotary { index, value });
        }
        None
    }
}

/// Explicit encoding; never autodetect one relative mode from ambiguous messages.
#[derive(Clone, Copy, Debug)]
pub enum Relative {
    TwosComplement,
    BinaryOffset,
    SignedBit,
}
impl Relative {
    pub fn delta(self, value: u8) -> Option<i16> {
        if value > 127 {
            return None;
        }
        let v = i16::from(value);
        Some(
            match self {
                Self::TwosComplement => match value {
                    0 | 64 => 0,
                    1..=63 => v,
                    _ => v - 128,
                },
                Self::BinaryOffset => v - 64,
                Self::SignedBit => {
                    if value < 64 {
                        v
                    } else {
                        -(v - 64)
                    }
                }
            }
            .clamp(-8, 8),
        )
    }
}

#[derive(Clone, Debug, Default)]
pub struct Pickup {
    identity: Option<(u32, u8, i32)>,
    previous: Option<u8>,
    caught: bool,
}
impl Pickup {
    /// Rearm on target, parameter or authoritative value change. Caller updates the
    /// anchor after its own accepted edits, or conservatively rearms as this demo does.
    pub fn accept(&mut self, identity: (u32, u8, i32), target: u8, value: u8) -> bool {
        if target > 127 || value > 127 {
            return false;
        }
        if self.identity != Some(identity) {
            self.identity = Some(identity);
            self.previous = None;
            self.caught = false;
        }
        let close = value.abs_diff(target) <= 1;
        let crossed = self.previous.is_some_and(|p| {
            (i16::from(p) - i16::from(target)) * (i16::from(value) - i16::from(target)) <= 0
        });
        self.caught |= close || crossed;
        self.previous = Some(value);
        self.caught
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Color {
    Off,
    Red,
    Green,
    Yellow,
    Blue,
    Purple,
    Cyan,
    White,
}

/// Desired transient feedback for the base page. No hardware output is performed.
/// Applied state drives toggles; a pending command is explicitly yellow.
pub fn pad_colors(desk: &crate::model::Desk) -> [Color; 8] {
    use crate::model::{Edit, Mode, Page};
    let mut colors = [Color::Blue; 8];
    colors[match desk.page {
        Page::Mix => 0,
        Page::Channel => 1,
        Page::Analysis => 2,
    }] = Color::Cyan;
    if let Some(c) = desk.selected() {
        if c.muted {
            colors[3] = Color::Red;
        }
        if c.held {
            colors[4] = Color::Yellow;
        }
        colors[5] = if c.held && desk.confirmed.mode == Mode::Auto {
            Color::Blue
        } else {
            Color::Off
        };
    }
    if !desk.connected {
        colors[3..7].fill(Color::Off);
        colors[7] = Color::Yellow;
    } else if let Some(command) = desk.pending() {
        let index = match command.edit {
            Edit::Mute { .. } => Some(3),
            Edit::Hold { .. } => Some(4),
            Edit::Release { .. } => Some(5),
            Edit::Mode(_) => Some(6),
            _ => None,
        };
        if let Some(index) = index {
            colors[index] = Color::Yellow;
        }
    }
    colors
}
impl Color {
    fn byte(self) -> u8 {
        match self {
            Self::Off => 0,
            Self::Red => 1,
            Self::Green => 4,
            Self::Yellow => 5,
            Self::Blue => 16,
            Self::Purple => 17,
            Self::Cyan => 20,
            Self::White => 127,
        }
    }
}
/// Researched mkII transient LED message. Encoding is not physical acceptance.
pub fn pad_packet(bank: u8, pad: u8, color: Color) -> Option<[u8; 12]> {
    if bank > 1 || pad > 7 {
        return None;
    }
    Some([
        0xf0,
        0,
        0x20,
        0x6b,
        0x7f,
        0x42,
        2,
        0,
        0x10,
        0x70 + bank * 8 + pad,
        color.byte(),
        0xf7,
    ])
}
