//! Device-free bitmap scene backend. Same PSF glyphs and primitives as SVG.
use crate::render::{Font, HEIGHT, Primitive, Scene, WIDTH};
pub fn rgba(scene: &Scene) -> Vec<u8> {
    let mut pixels = vec![0; (WIDTH * HEIGHT * 4) as usize];
    let font = Font::default();
    fn color(s: &str) -> [u8; 4] {
        let n = u32::from_str_radix(s.trim_start_matches('#'), 16).unwrap_or(0);
        [(n >> 16) as u8, (n >> 8) as u8, n as u8, 255]
    }
    fn put(p: &mut [u8], x: i64, y: i64, c: [u8; 4]) {
        if x >= 0 && y >= 0 && x < i64::from(WIDTH) && y < i64::from(HEIGHT) {
            let i = (y as usize * WIDTH as usize + x as usize) * 4;
            p[i..i + 4].copy_from_slice(&c);
        }
    }
    for primitive in &scene.primitives {
        match primitive {
            Primitive::Rect { x, y, w, h, fill } => {
                let c = color(fill);
                for yy in *y..y.saturating_add(*h).min(HEIGHT) {
                    for xx in *x..x.saturating_add(*w).min(WIDTH) {
                        put(&mut pixels, xx.into(), yy.into(), c);
                    }
                }
            }
            Primitive::Text {
                x,
                y,
                value,
                color: ink,
            } => {
                for (i, ch) in value.chars().enumerate() {
                    let bits = font
                        .glyphs
                        .iter()
                        .find(|(c, _)| *c == ch)
                        .or_else(|| font.glyphs.iter().find(|(c, _)| *c == '?'));
                    if let Some((_, bits)) = bits {
                        for yy in 0..24 {
                            for xx in 0..12 {
                                if bits[yy * 2 + xx / 8] & (0x80 >> (xx % 8)) != 0 {
                                    put(
                                        &mut pixels,
                                        i64::from(*x) + (i * 12 + xx) as i64,
                                        i64::from(*y) + yy as i64,
                                        color(ink),
                                    );
                                }
                            }
                        }
                    }
                }
            }
            Primitive::Line {
                x1,
                y1,
                x2,
                y2,
                color: ink,
            } => {
                let (mut x, mut y) = (i64::from(*x1), i64::from(*y1));
                let (endx, endy) = (i64::from(*x2), i64::from(*y2));
                let dx = (endx - x).abs();
                let dy = -(endy - y).abs();
                let sx = if x < endx { 1 } else { -1 };
                let sy = if y < endy { 1 } else { -1 };
                let mut error = dx + dy;
                loop {
                    put(&mut pixels, x, y, color(ink));
                    if x == endx && y == endy {
                        break;
                    }
                    let e = 2 * error;
                    if e >= dy {
                        error += dy;
                        x += sx;
                    }
                    if e <= dx {
                        error += dx;
                        y += sy;
                    }
                }
            }
        }
    }
    pixels
}
/// Write a portable, bounded RGB review artifact without image dependencies.
pub fn ppm(scene: &Scene, path: &std::path::Path) -> Result<(), String> {
    use std::io::Write;
    let mut file = std::fs::File::create(path).map_err(|e| e.to_string())?;
    write!(file, "P6\n{WIDTH} {HEIGHT}\n255\n").map_err(|e| e.to_string())?;
    let pixels = rgba(scene);
    let rgb: Vec<u8> = pixels
        .chunks_exact(4)
        .flat_map(|p| p[..3].iter().copied())
        .collect();
    file.write_all(&rgb).map_err(|e| e.to_string())
}
