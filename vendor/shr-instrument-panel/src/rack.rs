//! Compact equipment surfaces. All geometry is reusable cell tiles; values stay text.
use crate::{Material, tile};
use ratatui::{
    buffer::Buffer,
    layout::Rect,
    style::{Color, Style},
};
pub const FACE: Color = Color::Rgb(43, 47, 52);
pub const TEXT: Color = Color::Rgb(230, 234, 236);
pub const DIM: Color = Color::Rgb(155, 166, 176);
pub const CYAN: Color = Color::Rgb(119, 219, 225);
pub const AMBER: Color = Color::Rgb(241, 190, 99);
pub const RED: Color = Color::Rgb(240, 98, 102);
pub const WELL: Color = Color::Rgb(12, 19, 24);
pub fn fill(b: &mut Buffer, a: Rect, color: Color) {
    for y in a.y..a.bottom() {
        for x in a.x..a.right() {
            if let Some(c) = b.cell_mut((x, y)) {
                c.set_char(' ').set_bg(color).set_fg(TEXT);
            }
        }
    }
}
pub fn text(b: &mut Buffer, x: u16, y: u16, w: u16, s: &str, c: Color) {
    if b.area.contains((x, y).into()) {
        b.set_stringn(
            x,
            y,
            s,
            w.min(b.area.right() - x) as usize,
            Style::default().fg(c),
        );
    }
}
pub fn panel(b: &mut Buffer, a: Rect, label: &str) {
    fill(b, a, FACE);
    fill(b, Rect::new(a.x, a.y, a.width, 1), Color::Rgb(78, 85, 92));
    fill(
        b,
        Rect::new(a.x, a.bottom() - 1, a.width, 1),
        Color::Rgb(23, 27, 32),
    );
    text(b, a.x + 1, a.y, a.width - 2, label, TEXT);
}
pub fn well(b: &mut Buffer, a: Rect) {
    fill(b, a, WELL);
    for x in a.x..a.right() {
        if let Some(c) = b.cell_mut((x, a.y)) {
            c.set_char('▔').set_fg(Color::Rgb(4, 8, 12));
        }
        if let Some(c) = b.cell_mut((x, a.bottom() - 1)) {
            c.set_char('▁').set_fg(Color::Rgb(94, 104, 113));
        }
    }
}
pub fn mosaic(b: &mut Buffer, x: u16, y: u16, w: u16, h: u16, name: &str) {
    for r in 0..h {
        for c in 0..w {
            if let Some(cell) = b.cell_mut((x + c, y + r)) {
                cell.set_char(tile(&format!("{name}_{r}_{c}")));
            }
        }
    }
}
pub fn encoder(b: &mut Buffer, x: u16, y: u16, value: f64, m: Material) {
    if m == Material::Glyph {
        mosaic(
            b,
            x,
            y,
            6,
            3,
            &format!("encoder_{}", (value.clamp(0., 1.) * 16.).round() as u8),
        );
    } else {
        text(b, x, y, 6, "╭────╮", DIM);
        text(b, x, y + 1, 6, "│ ◉  │", TEXT);
        text(b, x, y + 2, 6, "╰────╯", DIM);
    }
}
pub fn button(b: &mut Buffer, a: Rect, label: &str, active: bool, alarm: bool, m: Material) {
    let name = if alarm {
        "redkey"
    } else if active {
        "active"
    } else {
        "key"
    };
    let face = if alarm {
        Color::Rgb(113, 37, 43)
    } else if active {
        Color::Rgb(57, 78, 89)
    } else {
        Color::Rgb(87, 94, 103)
    };
    fill(b, a, face);
    for y in 0..a.height {
        for x in 0..a.width {
            let edge = match (x == 0, x == a.width - 1, y == 0, y == a.height - 1) {
                (true, _, true, _) => "tl",
                (_, true, true, _) => "tr",
                (true, _, _, true) => "bl",
                (_, true, _, true) => "br",
                (_, _, true, _) => "top",
                (_, _, _, true) => "bottom",
                (true, _, _, _) => "left",
                (_, true, _, _) => "right",
                _ => continue,
            };
            if let Some(c) = b.cell_mut((a.x + x, a.y + y)) {
                c.set_char(if m == Material::Glyph {
                    tile(&format!("{name}_{edge}"))
                } else if y == 0 || y == a.height - 1 {
                    '─'
                } else {
                    '│'
                });
            }
        }
    }
    let length = label.chars().count() as u16;
    text(
        b,
        a.x + (a.width.saturating_sub(length)) / 2,
        a.y + 1,
        a.width.saturating_sub(2),
        label,
        TEXT,
    );
}
pub fn lamp(b: &mut Buffer, x: u16, y: u16, on: bool, color: Color) {
    // Lens, dark socket and explicit caption are separate; no spill into neighbours.
    if let Some(c) = b.cell_mut((x, y)) {
        c.set_char(if on { '●' } else { '○' })
            .set_fg(if on { color } else { DIM })
            .set_bg(Color::Rgb(18, 23, 28));
    }
}
/// Three-cell physical lens; illumination never extends outside its allocated cells.
pub fn indicator(b: &mut Buffer, x: u16, y: u16, colour: &str, on: bool, m: Material) {
    let colour = match colour {
        "green" | "yellow" | "red" | "blue" => colour,
        _ => "blue",
    };
    if m == Material::Glyph {
        mosaic(
            b,
            x,
            y,
            3,
            1,
            &format!("lens_{colour}_{}", if on { "on" } else { "off" }),
        );
    } else {
        text(
            b,
            x,
            y,
            3,
            if on { "[●]" } else { "[○]" },
            if on {
                match colour {
                    "green" => Color::Green,
                    "yellow" => AMBER,
                    "red" => RED,
                    _ => CYAN,
                }
            } else {
                DIM
            },
        );
    }
}
pub fn fader(b: &mut Buffer, a: Rect, db: f64, m: Material) {
    fill(b, a, FACE);
    let x = a.x + 4;
    for y in a.y..a.bottom() {
        text(b, x, y, 2, "▕▏", Color::Rgb(9, 13, 17));
    }
    let pos =
        |v: f64| a.y + ((12. - v.clamp(-60., 12.)) / 72. * f64::from(a.height - 2)).round() as u16;
    for v in [12., 0., -12., -24., -36., -48., -60.] {
        let y = pos(v);
        text(b, a.x + 9, y, 8, &format!("─ {v:>3.0}"), DIM);
    }
    let y = pos(db);
    if m == Material::Glyph {
        mosaic(b, a.x + 2, y, 6, 2, "slider");
    } else {
        text(b, a.x + 2, y, 6, "╞════╡", TEXT);
    }
}
