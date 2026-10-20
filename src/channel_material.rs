//! Bounded Channel-only Kitty material experiment. Ratatui remains sole text/input owner.
//! Assets contain no labels or values. Upload once; subsequent frames diff placements.
use shr_instrument_panel::{
    assets,
    terminal::ratatui::{buffer::Buffer, style::Color},
};
use std::{
    collections::BTreeSet,
    io::{self, Write},
};
#[path = "channel_material_assets.rs"]
mod authored;

const FIRST_ID: usize = 0x534800;
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Placement {
    asset: usize,
    x: u16,
    y: u16,
    layer: i32,
}
fn add(out: &mut Vec<Placement>, name: &str, x: u16, y: u16, layer: i32) {
    if let Some(asset) = authored::ASSETS.iter().position(|a| a.0 == name) {
        out.push(Placement { asset, x, y, layer });
    }
}
fn glyph(s: &str) -> Option<&'static str> {
    let c = s.chars().next()? as u32;
    let i = c.checked_sub(assets::BASE)? as usize;
    assets::ASSETS.get(i).map(|a| a.0)
}
const WELLS: &[(u16, u16, u16, u16)] = &[
    (3, 3, 74, 19),
    (83, 3, 14, 19),
    (103, 3, 14, 19),
    (123, 3, 14, 19),
    (143, 3, 14, 19),
];
fn plate(x: u16, y: u16) -> bool {
    (0..44).contains(&y)
        && ((1..79).contains(&x) || (81..139).contains(&x) || (141..159).contains(&x))
}
fn fixed_well(x: u16, y: u16) -> bool {
    WELLS
        .iter()
        .any(|(xx, yy, w, h)| (*xx..xx + w).contains(&x) && (*yy..yy + h).contains(&y))
}
/// Convert only the selected Channel visual buffer. Hit targets and authority stay upstream.
pub fn prepare(b: &mut Buffer) -> Vec<Placement> {
    if b.area.width < 160 || b.area.height < 45 {
        return vec![];
    }
    let mut out = vec![];
    for (name, x, y) in [("eq", 1, 0), ("comp", 81, 0), ("output", 141, 0)] {
        add(&mut out, name, x, y, -10);
    }
    for &(x, y, w, h) in WELLS {
        add(&mut out, &format!("well_{w}_{h}"), x, y, -9);
    }
    // Numeric field wells are actual populated field backgrounds, never invented data.
    for y in 0..45 {
        let mut x = 0;
        while x < 160 {
            if fixed_well(x, y) {
                x += 1;
                continue;
            }
            let bg = b[(x, y)].bg;
            if bg == shr_instrument_panel::rack::WELL || bg == Color::Rgb(32, 69, 81) {
                let start = x;
                while x < 160 && b[(x, y)].bg == bg {
                    x += 1;
                }
                add(&mut out, &format!("well_{}_1", x - start), start, y, -8);
            } else {
                x += 1;
            }
        }
    }
    for col in 0..8 {
        let x = 1 + col * 20;
        let kind = match b[(x, 44)].bg {
            Color::Rgb(113, 37, 43) => "alarm",
            Color::Rgb(57, 78, 89) => "pressed",
            _ => "metal",
        };
        add(&mut out, &format!("button_{kind}_18"), x, 44, -8);
    }
    for y in 0..45 {
        for x in 0..160 {
            if let Some(name) = glyph(b[(x, y)].symbol())
                && let Some(base) = name.strip_suffix("_0_0")
            {
                add(&mut out, base, x, y, -7);
            }
        }
    }
    // Four-cell meter bars use cached lens strips; text fallback keeps its glyphs.
    for (x, start, end) in [(85, 4, 20), (105, 4, 20), (125, 4, 20), (145, 4, 20)] {
        for y in start..end {
            if b[(x, y)].symbol() != "▰" {
                continue;
            }
            let name = match b[(x, y)].fg {
                Color::Rgb(59, 224, 113) => "meter_green_on",
                Color::Rgb(25, 59, 40) => "meter_green_off",
                Color::Rgb(72, 30, 34) => "meter_red_off",
                Color::Rgb(69, 53, 28) => "meter_amber_off",
                c if c == shr_instrument_panel::rack::RED => "meter_red_on",
                c if c == shr_instrument_panel::rack::AMBER => "meter_amber_on",
                _ => "meter_amber_off",
            };
            add(&mut out, name, x, y, -7);
            for xx in x..x + 4 {
                b[(xx, y)].set_char(' ');
            }
        }
    }
    for y in 0..45 {
        for x in 0..160 {
            let c = &mut b[(x, y)];
            let in_well = fixed_well(x, y)
                || c.bg == shr_instrument_panel::rack::WELL
                || c.bg == Color::Rgb(32, 69, 81);
            if glyph(c.symbol()).is_some() {
                c.set_char(' ');
            }
            // Remove old cell-rule bevels from bitmap wells; retain graphs/meters/text.
            if in_well && matches!(c.symbol(), "▔" | "▁") {
                c.set_char(' ');
            }
            if plate(x, y) && !in_well {
                c.fg = match c.fg {
                    Color::Rgb(119, 219, 225) => Color::Rgb(0, 64, 74),
                    Color::Rgb(241, 190, 99) => Color::Rgb(93, 46, 0),
                    _ => Color::Rgb(19, 29, 38),
                };
            }

            // All material surfaces lie between terminal background and ordinary text.
            // Rio's negative-z path is explicitly qualified by the preview contract.
            c.set_bg(Color::Reset);
        }
    }
    // Button text contrast follows its actual raised/latched state.
    for p in &out {
        let (name, w, _, _) = authored::ASSETS[p.asset];
        if name.starts_with("button_") {
            for x in p.x..p.x + w {
                for y in p.y..p.y + 1 {
                    b[(x, y)].set_fg(if name.starts_with("button_metal") {
                        Color::Rgb(19, 29, 38)
                    } else {
                        Color::Rgb(225, 242, 250)
                    });
                }
            }
        }
    }
    out
}
#[derive(Default)]
pub struct Materials {
    uploaded: BTreeSet<usize>,
    placed: Vec<Placement>,
    pub bytes: usize,
}
impl Materials {
    pub fn enabled() -> bool {
        std::env::var("SHR_CHANNEL_MATERIALS").as_deref() == Ok("rio-kitty-v1")
    }
    pub fn present(&mut self, next: Vec<Placement>, out: &mut impl Write) -> io::Result<()> {
        let mut commands = Vec::new();
        // Cursor is restored so the text backend's cursor model is not invalidated.
        for (i, p) in self.placed.iter().enumerate() {
            if next.get(i) != Some(p) {
                write!(
                    commands,
                    "\x1b_Ga=d,d=i,i={},p={},q=2\x1b\\",
                    FIRST_ID + p.asset,
                    i + 1
                )?;
            }
        }
        for (i, p) in next.iter().enumerate() {
            if self.placed.get(i) == Some(p) {
                continue;
            }
            let (_, w, h, data) = authored::ASSETS[p.asset];
            if self.uploaded.insert(p.asset) {
                for (chunk_no, chunk) in data.as_bytes().chunks(4096).enumerate() {
                    let more = usize::from((chunk_no + 1) * 4096 < data.len());
                    if chunk_no == 0 {
                        write!(
                            commands,
                            "\x1b_Ga=t,f=100,i={},q=2,m={more};",
                            FIRST_ID + p.asset
                        )?;
                    } else {
                        write!(commands, "\x1b_Gm={more};")?;
                    }
                    commands.extend_from_slice(chunk);
                    commands.extend_from_slice(b"\x1b\\");
                }
            }
            write!(
                commands,
                "\x1b7\x1b[{};{}H\x1b_Ga=p,i={},p={},c={w},r={h},z={},C=1,q=2\x1b\\\x1b8",
                p.y + 1,
                p.x + 1,
                FIRST_ID + p.asset,
                i + 1,
                p.layer
            )?;
        }
        out.write_all(&commands)?;
        out.flush()?;
        self.bytes += commands.len();
        self.placed = next;
        Ok(())
    }
    pub fn clear(&mut self, out: &mut impl Write) -> io::Result<()> {
        for i in &self.uploaded {
            write!(out, "\x1b_Ga=d,d=I,i={},q=2\x1b\\", FIRST_ID + i)?;
        }
        out.flush()?;
        self.uploaded.clear();
        self.placed.clear();
        Ok(())
    }
}
impl Drop for Materials {
    fn drop(&mut self) {
        let _ = self.clear(&mut io::stdout());
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn idle_sends_no_assets_or_placements_and_changes_reuse_cache() {
        let mut m = Materials::default();
        let mut sink = vec![];
        let mut p = vec![];
        add(&mut p, "encoder_0", 3, 19, -7);
        m.present(p.clone(), &mut sink).unwrap();
        assert!(!sink.is_empty());
        sink.clear();
        m.present(p.clone(), &mut sink).unwrap();
        assert!(sink.is_empty());
        p[0].x += 1;
        m.present(p, &mut sink).unwrap();
        let s = String::from_utf8(sink).unwrap();
        assert!(s.contains("a=p"));
        assert!(!s.contains("a=t"));
        m.clear(&mut vec![]).unwrap();
    }
}
