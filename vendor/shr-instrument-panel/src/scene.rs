//! Transitional cell-native consumption of the consoles' existing semantic scenes.
//! Coordinates preserve the established 160x45 review coverage. No pixel buffer.
use crate::{Material, tile};
use ratatui::{
    buffer::Buffer,
    layout::Rect,
    style::{Color, Style},
};
#[derive(Clone, Debug)]
pub enum Primitive {
    Rect {
        x: u32,
        y: u32,
        w: u32,
        h: u32,
        fill: String,
    },
    Text {
        x: u32,
        y: u32,
        value: String,
        color: String,
    },
    Line {
        x1: u32,
        y1: u32,
        x2: u32,
        y2: u32,
        color: String,
    },
}
fn colour(s: &str) -> Color {
    let n = s
        .strip_prefix('#')
        .and_then(|s| u32::from_str_radix(s, 16).ok())
        .unwrap_or(0xf1f4f7);
    Color::Rgb((n >> 16) as u8, (n >> 8) as u8, n as u8)
}
/// Returns false when the full established review canvas cannot be presented.
pub fn draw(scene: &[Primitive], b: &mut Buffer, material: Material) -> bool {
    if b.area.width < 160 || b.area.height < 45 {
        b.set_stringn(
            b.area.x,
            b.area.y,
            "Resize to 160 x 45 cells. Controls paused; provider state retained.",
            b.area.width as usize,
            Style::default().fg(Color::White).bg(Color::Black),
        );
        return false;
    }
    let area = Rect::new(b.area.x, b.area.y, 160, 45);
    let ox = area.x;
    let oy = area.y;
    for p in scene {
        match p {
            Primitive::Rect { x, y, w, h, fill } => {
                let c = colour(fill);
                let x0 = ((x + 6) / 12).min(160);
                let x1 = ((x.saturating_add(*w) + 6) / 12).min(160);
                let y0 = (*y).min(1080);
                let y1 = y.saturating_add(*h).min(1080);
                for yy in y0 / 24..y1.div_ceil(24) {
                    for xx in x0..x1 {
                        let top = y0.max(yy * 24) - yy * 24;
                        let bottom = y1.min((yy + 1) * 24) - yy * 24;
                        let cell = &mut b[(ox + xx as u16, oy + yy as u16)];
                        if top == 0 && bottom == 24 {
                            cell.set_char(' ').set_bg(c);
                        } else if bottom == 24 {
                            let n = ((24 - top + 1) / 3).min(8);
                            if n > 0 {
                                cell.set_char(
                                    [' ', '▁', '▂', '▃', '▄', '▅', '▆', '▇', '█'][n as usize],
                                )
                                .set_fg(c);
                            }
                        } else if top == 0 {
                            let n = ((bottom + 1) / 3).min(8);
                            if n > 0 {
                                let bg = cell.bg;
                                cell.set_char(
                                    ['█', '▇', '▆', '▅', '▄', '▃', '▂', '▁', ' '][n as usize],
                                )
                                .set_fg(bg)
                                .set_bg(c);
                            }
                        } else {
                            let n = ((bottom - top + 1) / 3).clamp(1, 8);
                            cell.set_char(
                                [' ', '▁', '▂', '▃', '▄', '▅', '▆', '▇', '█'][n as usize],
                            )
                            .set_fg(c);
                        }
                    }
                }
                if material == Material::Glyph && *w >= 120 && *h >= 72 && *w < 1920 && *h < 1080 {
                    let top = (y0 / 24) as u16;
                    let bottom = y1.div_ceil(24).saturating_sub(1) as u16;
                    if x1 > x0 + 2 && bottom > top + 1 {
                        for xx in x0..x1 {
                            let suffix = if xx == x0 {
                                "_left"
                            } else if xx == x1 - 1 {
                                "_right"
                            } else {
                                ""
                            };
                            b[(ox + xx as u16, oy + top)]
                                .set_char(tile(&format!("raised_top{suffix}")));
                            b[(ox + xx as u16, oy + bottom)]
                                .set_char(tile(&format!("raised_bottom{suffix}")));
                        }
                        for yy in top + 1..bottom {
                            b[(ox + x0 as u16, oy + yy)].set_char(tile("raised_left"));
                            b[(ox + (x1 - 1) as u16, oy + yy)].set_char(tile("raised_right"));
                        }
                    }
                }
            }
            Primitive::Line {
                x1,
                y1,
                x2,
                y2,
                color,
            } => {
                let (x0, y0, x1, y1) = (
                    *x1 as i32 / 12,
                    *y1 as i32 / 24,
                    *x2 as i32 / 12,
                    *y2 as i32 / 24,
                );
                let n = (x1 - x0).abs().max((y1 - y0).abs()).max(1);
                for i in 0..=n {
                    let (x, y) = (x0 + (x1 - x0) * i / n, y0 + (y1 - y0) * i / n);
                    if (0..160).contains(&x) && (0..45).contains(&y) {
                        let c = if material == Material::Glyph && y0 == y1 {
                            tile("well_top")
                        } else if y0 == y1 {
                            '─'
                        } else if x0 == x1 {
                            '│'
                        } else {
                            '•'
                        };
                        b[(ox + x as u16, oy + y as u16)]
                            .set_char(c)
                            .set_fg(colour(color));
                    }
                }
            }
            Primitive::Text { .. } => {}
        }
    }
    // Labels always win over snapped decorations. Exact source strings retained.
    for p in scene {
        if let Primitive::Text { x, y, value, color } = p {
            let (xx, yy) = (x / 12, y / 24);
            if xx < 160 && yy < 45 {
                let bg = scene
                    .iter()
                    .rev()
                    .find_map(|p| {
                        if let Primitive::Rect {
                            x: rx,
                            y: ry,
                            w,
                            h,
                            fill,
                        } = p
                        {
                            if rx <= x
                                && ry <= y
                                && *x < rx.saturating_add(*w)
                                && *y < ry.saturating_add(*h)
                            {
                                Some(colour(fill))
                            } else {
                                None
                            }
                        } else {
                            None
                        }
                    })
                    .unwrap_or(Color::Reset);
                b.set_stringn(
                    ox + xx as u16,
                    oy + yy as u16,
                    value,
                    (160 - xx) as usize,
                    Style::default().fg(colour(color)).bg(bg),
                );
            }
        }
    }
    true
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn refuses_partial_review() {
        for (w, h) in [(159, 45), (160, 44), (0, 0)] {
            let mut b = Buffer::empty(Rect::new(0, 0, w, h));
            assert!(!draw(&[], &mut b, Material::Text));
        }
    }
    #[test]
    fn exact_text_survives_decorations() {
        let mut b = Buffer::empty(Rect::new(0, 0, 160, 45));
        let s = vec![
            Primitive::Text {
                x: 12,
                y: 24,
                value: "HOLD -12.3 dB STALE".into(),
                color: "#ffffff".into(),
            },
            Primitive::Line {
                x1: 0,
                y1: 24,
                x2: 1920,
                y2: 24,
                color: "#ff0000".into(),
            },
        ];
        assert!(draw(&s, &mut b, Material::Glyph));
        assert_eq!(b[(1, 1)].symbol(), "H");
        assert_eq!(b[(5, 1)].symbol(), " ");
    }
}

/// Dense equipment plate for text-only owner pages. Every original text run is
/// retained; rich meter/graph pages continue through `draw` without data loss.
/// Returns false instead of marking an over-capacity review as presented.
pub fn instrument_text(
    scene: &[Primitive],
    b: &mut Buffer,
    material: Material,
    fresh: bool,
    writable: bool,
    navigation: &[&str],
) -> bool {
    use crate::{Button, ControlState, INK, Led, PLATE, WELL};
    use ratatui::widgets::Widget;
    if b.area.width < 160 || b.area.height < 45 {
        return draw(scene, b, material);
    }
    let mut lines: Vec<_> = scene
        .iter()
        .filter_map(|p| {
            if let Primitive::Text { x, y, value, color } = p {
                Some((*x, *y, value.as_str(), color.as_str()))
            } else {
                None
            }
        })
        .collect();
    lines.sort_by_key(|(x, y, _, _)| (*y, *x));
    // Rows may contain several aligned runs. Preserve that horizontal geometry.
    let mut ys: Vec<_> = lines.iter().map(|(_, y, _, _)| *y / 24).collect();
    ys.sort_unstable();
    ys.dedup();
    if ys.len() > 36 {
        return draw(scene, b, material);
    }
    b.set_style(b.area, Style::default().bg(PLATE).fg(INK));
    let width = b.area.width.min(160);
    let x = b.area.x;
    let y = b.area.y;
    let nav_width = (width - 2) / navigation.len().max(1) as u16;
    for (i, label) in navigation.iter().enumerate() {
        Button {
            label,
            state: ControlState::Raised,
            material,
        }
        .render(
            Rect::new(x + 1 + i as u16 * nav_width, y, nav_width - 1, 3),
            b,
        );
    }
    Led {
        colour: "green",
        on: fresh,
        label: "PROVIDER FRESH",
        material,
    }
    .render(Rect::new(x + 2, y + 3, 26, 1), b);
    Led {
        colour: "red",
        on: !fresh,
        label: "STALE / UNAVAILABLE",
        material,
    }
    .render(Rect::new(x + 32, y + 3, 30, 1), b);
    Led {
        colour: "yellow",
        on: writable,
        label: "OWNER WRITER GRANT",
        material,
    }
    .render(Rect::new(x + 70, y + 3, 35, 1), b);
    for yy in y + 5..y + 5 + ys.len() as u16 + 2 {
        for xx in x + 1..x + width - 1 {
            b[(xx, yy)].set_char(' ').set_bg(WELL);
        }
    }
    if material == Material::Glyph {
        for xx in x + 1..x + width - 1 {
            b[(xx, y + 5)].set_char(tile("well_top"));
            b[(xx, y + 6 + ys.len() as u16)].set_char(tile("well_bottom"));
        }
    }
    for (sx, sy, value, color) in lines {
        let row = ys.binary_search(&(sy / 24)).unwrap() as u16;
        let col = (sx / 12).clamp(2, 158) as u16;
        b.set_stringn(
            x + col,
            y + 6 + row,
            value,
            (width - col) as usize,
            Style::default().fg(colour(color)).bg(WELL),
        );
    }
    true
}

#[cfg(test)]
mod instrument_tests {
    use super::*;
    #[test]
    fn dense_layout_keeps_all_review_lines_and_health_words() {
        let lines: Vec<_> = (0..32)
            .map(|i| Primitive::Text {
                x: 24,
                y: 24 * i,
                value: format!("REVIEW {i:02} / target -12.3 dB / HOLD / STALE"),
                color: "#ffffff".into(),
            })
            .collect();
        let mut b = Buffer::empty(Rect::new(0, 0, 160, 45));
        assert!(instrument_text(
            &lines,
            &mut b,
            Material::Glyph,
            false,
            false,
            &["F1 MIX"]
        ));
        for i in 0..32 {
            let actual: String = (2..44).map(|x| b[(x, 6 + i)].symbol()).collect();
            assert!(actual.starts_with(&format!("REVIEW {i:02} / target -12.3 dB / HOLD / STALE")));
        }
    }
    #[test]
    fn material_does_not_change_label_data() {
        let s = vec![Primitive::Text {
            x: 24,
            y: 24,
            value: "UNKNOWN / NULL / NO OUTPUT".into(),
            color: "#ffffff".into(),
        }];
        for m in [Material::Text, Material::Glyph] {
            let mut b = Buffer::empty(Rect::new(0, 0, 160, 45));
            assert!(instrument_text(&s, &mut b, m, false, false, &["F1 STAGE"]));
            let actual: String = (2..28).map(|x| b[(x, 6)].symbol()).collect();
            assert_eq!(actual, "UNKNOWN / NULL / NO OUTPUT");
        }
    }
}
