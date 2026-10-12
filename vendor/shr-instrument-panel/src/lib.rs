//! Shared, cell-native instrument controls. No device or authority logic.
pub mod assets;
pub mod protocol;
use ratatui::{
    buffer::Buffer,
    layout::Rect,
    style::{Color, Style},
    widgets::Widget,
};

pub const PLATE: Color = Color::Rgb(49, 55, 63);
pub const INK: Color = Color::Rgb(241, 244, 247);
pub const WELL: Color = Color::Rgb(9, 13, 18);
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Material {
    Text,
    Glyph,
}
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ControlState {
    Raised,
    Pressed,
    Focused,
    Disabled,
    Latched,
    Warning,
    Unavailable,
}
impl ControlState {
    pub fn name(self) -> &'static str {
        match self {
            Self::Raised => "raised",
            Self::Pressed => "pressed",
            Self::Focused => "focused",
            Self::Disabled => "disabled",
            Self::Latched => "latched",
            Self::Warning => "warning",
            Self::Unavailable => "unavailable",
        }
    }
    fn face(self) -> Color {
        match self {
            Self::Raised | Self::Focused => Color::Rgb(89, 98, 109),
            Self::Pressed => Color::Rgb(39, 46, 55),
            Self::Disabled => Color::Rgb(57, 62, 69),
            Self::Latched => Color::Rgb(54, 76, 63),
            Self::Warning => Color::Rgb(98, 70, 39),
            Self::Unavailable => Color::Rgb(39, 44, 51),
        }
    }
}
pub fn tile(name: &str) -> char {
    char::from_u32(
        assets::BASE
            + assets::ASSETS
                .iter()
                .position(|(n, _)| *n == name)
                .expect("registered asset") as u32,
    )
    .unwrap()
}
fn cell(b: &mut Buffer, x: u16, y: u16, c: char, style: Style) {
    if let Some(v) = b.cell_mut((x, y)) {
        v.set_char(c).set_style(style);
    }
}
pub struct Button<'a> {
    pub label: &'a str,
    pub state: ControlState,
    pub material: Material,
}
impl Widget for Button<'_> {
    fn render(self, area: Rect, b: &mut Buffer) {
        let a = area.intersection(b.area);
        if a.width < 3 || a.height < 3 {
            return;
        }
        let st = Style::default().fg(INK).bg(self.state.face());
        b.set_style(a, st);
        for y in a.y..a.bottom() {
            for x in a.x..a.right() {
                let edge = if y == a.y {
                    Some("top")
                } else if y == a.bottom() - 1 {
                    Some("bottom")
                } else if x == a.x {
                    Some("left")
                } else if x == a.right() - 1 {
                    Some("right")
                } else {
                    None
                };
                let ch = match edge {
                    Some(e) if self.material == Material::Glyph => tile(&format!(
                        "{}_{}{}",
                        self.state.name(),
                        e,
                        if e == "top" || e == "bottom" {
                            if x == a.x {
                                "_left"
                            } else if x == a.right() - 1 {
                                "_right"
                            } else {
                                ""
                            }
                        } else {
                            ""
                        }
                    )),
                    Some("top" | "bottom") => '─',
                    Some(_) => '│',
                    None => ' ',
                };
                cell(b, x, y, ch, st);
            }
        }
        let mark = match self.state {
            ControlState::Focused => ">",
            ControlState::Pressed => "v",
            ControlState::Latched => "*",
            ControlState::Disabled => "-",
            ControlState::Warning => "!",
            ControlState::Unavailable => "?",
            _ => " ",
        };
        b.set_stringn(
            a.x + 1,
            a.y + 1,
            format!("{mark} {}", self.label),
            (a.width - 2) as usize,
            st,
        );
    }
}
pub struct Led<'a> {
    pub colour: &'a str,
    pub on: bool,
    pub label: &'a str,
    pub material: Material,
}
impl Widget for Led<'_> {
    fn render(self, a: Rect, b: &mut Buffer) {
        let a = a.intersection(b.area);
        if a.is_empty() {
            return;
        }
        let c = match self.colour {
            "green" => Color::Green,
            "yellow" => Color::Yellow,
            "red" => Color::Red,
            _ => Color::Blue,
        };
        let symbol = if self.material == Material::Glyph {
            tile(&format!(
                "{}_{}",
                self.colour,
                if self.on { "on" } else { "off" }
            ))
        } else if self.on {
            '●'
        } else {
            '○'
        };
        cell(b, a.x, a.y, symbol, Style::default().fg(c).bg(PLATE));
        if a.width > 1 {
            b.set_stringn(
                a.x + 1,
                a.y,
                format!(" {} {}", if self.on { "ON " } else { "OFF" }, self.label),
                (a.width - 1) as usize,
                Style::default().fg(INK).bg(PLATE),
            );
        }
    }
}
/// Fractional bars accept normalized caller data. None is unavailable, never zero.
pub fn bar(b: &mut Buffer, a: Rect, value: Option<f64>, colour: Color) {
    let a = a.intersection(b.area);
    if a.is_empty() {
        return;
    }
    b.set_style(a, Style::default().bg(WELL).fg(colour));
    let Some(v) = value.filter(|v| v.is_finite() && (0.0..=1.0).contains(v)) else {
        b.set_stringn(
            a.x,
            a.y,
            "-- unavailable",
            a.width as usize,
            Style::default().fg(INK).bg(WELL),
        );
        return;
    };
    let steps = (v * f64::from(a.width) * 8.0).round() as u32;
    for x in 0..a.width {
        let n = steps.saturating_sub(u32::from(x) * 8).min(8) as usize;
        cell(
            b,
            a.x + x,
            a.y,
            [' ', '▏', '▎', '▍', '▌', '▋', '▊', '▉', '█'][n],
            Style::default().fg(colour).bg(WELL),
        );
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn controls_clip_at_every_small_size() {
        for w in 0..32 {
            for h in 0..9 {
                let mut b = Buffer::empty(Rect::new(0, 0, w, h));
                Button {
                    label: "HOLD 12.0 dB",
                    state: ControlState::Focused,
                    material: Material::Glyph,
                }
                .render(Rect::new(0, 0, 32, 9), &mut b);
                assert_eq!(b.content.len(), w as usize * h as usize);
            }
        }
    }
    #[test]
    fn physical_lenses_clip_and_keep_neighbours_intact() {
        for colour in ["green", "yellow", "red", "blue"] {
            for on in [false, true] {
                for width in 0..8 {
                    let mut b = Buffer::empty(Rect::new(0, 0, width, 1));
                    rack::indicator(&mut b, 2, 0, colour, on, Material::Glyph);
                    for x in 0..width {
                        if !(2..5).contains(&x) {
                            assert_eq!(b[(x, 0)].symbol(), " ");
                        } else {
                            assert_eq!(
                                b[(x, 0)].symbol(),
                                tile(&format!(
                                    "lens_{colour}_{}_0_{}",
                                    if on { "on" } else { "off" },
                                    x - 2
                                ))
                                .to_string()
                            );
                        }
                    }
                }
            }
        }
    }
    #[test]
    fn unavailable_is_not_silence() {
        let a = Rect::new(0, 0, 16, 1);
        let mut b = Buffer::empty(a);
        bar(&mut b, a, None, Color::Green);
        assert_eq!(b[(0, 0)].symbol(), "-");
        bar(&mut b, a, Some(0.0), Color::Green);
        assert!(b.content.iter().all(|c| c.symbol() == " "));
    }
    #[test]
    fn tiles_are_single_cell_and_unique() {
        let mut seen = std::collections::BTreeSet::new();
        for (name, _) in assets::ASSETS {
            let c = tile(name);
            assert!(seen.insert(c));
            let line = ratatui::text::Line::raw(c.to_string());
            assert_eq!(line.width(), 1);
        }
    }
    #[test]
    fn fractional_bar_endpoints() {
        let a = Rect::new(0, 0, 8, 1);
        for i in 0..=64 {
            let mut b = Buffer::empty(a);
            bar(&mut b, a, Some(i as f64 / 64.0), Color::Green);
            assert_eq!(
                b.content.iter().filter(|c| c.symbol() != " ").count(),
                (i as usize).div_ceil(8)
            );
        }
    }
}

pub mod scene;
pub mod terminal;

pub mod rack;
