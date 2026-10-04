use crate::model::{Desk, Page};
use std::fmt::Write;

pub const WIDTH: u32 = 1920;
pub const HEIGHT: u32 = 1080;
const BG: &str = "#10151d";
const PANEL: &str = "#192330";
const EDGE: &str = "#3c4f63";
const TEXT: &str = "#e4e8e9";
const DIM: &str = "#9caebc";
const CYAN: &str = "#66dfd3";
const AMBER: &str = "#f1bd6b";
const RED: &str = "#f47c85";

/// One fixed scene backend for review; a future GPU backend consumes the same primitives.
#[derive(Clone, Debug)]
pub enum Primitive {
    Rect {
        x: u32,
        y: u32,
        w: u32,
        h: u32,
        fill: &'static str,
    },
    Text {
        x: u32,
        y: u32,
        value: String,
        color: &'static str,
    },
    Line {
        x1: u32,
        y1: u32,
        x2: u32,
        y2: u32,
        color: &'static str,
    },
}
#[derive(Default)]
pub struct Scene {
    pub primitives: Vec<Primitive>,
}
impl Scene {
    fn rect(&mut self, x: u32, y: u32, w: u32, h: u32, fill: &'static str) {
        self.primitives.push(Primitive::Rect { x, y, w, h, fill });
    }
    fn text(&mut self, x: u32, y: u32, value: impl Into<String>, color: &'static str) {
        self.primitives.push(Primitive::Text {
            x,
            y,
            value: value.into(),
            color,
        });
    }
    fn line(&mut self, x1: u32, y1: u32, x2: u32, y2: u32, color: &'static str) {
        self.primitives.push(Primitive::Line {
            x1,
            y1,
            x2,
            y2,
            color,
        });
    }
    fn panel(&mut self, x: u32, y: u32, w: u32, h: u32, title: &str) {
        self.rect(x, y, w, h, EDGE);
        self.rect(x + 1, y + 1, w - 2, h - 2, PANEL);
        self.text(x + 12, y + 8, title, DIM);
        self.line(x, y + 40, x + w, y + 40, EDGE);
    }
    pub fn in_bounds(&self) -> bool {
        self.primitives.iter().all(|p| match p {
            Primitive::Rect { x, y, w, h, .. } => x + w <= WIDTH && y + h <= HEIGHT,
            Primitive::Text { x, y, value, .. } => {
                *x + value.chars().count() as u32 * 12 <= WIDTH && y + 24 <= HEIGHT
            }
            Primitive::Line { x1, y1, x2, y2, .. } => {
                *x1 <= WIDTH && *x2 <= WIDTH && *y1 <= HEIGHT && *y2 <= HEIGHT
            }
        })
    }
}

pub fn scene(desk: &Desk) -> Scene {
    let mut s = Scene::default();
    s.rect(0, 0, WIDTH, HEIGHT, BG);
    s.rect(0, 0, WIDTH, 48, PANEL);
    s.text(24, 12, "SHR DESK / GIGPIES", CYAN);
    s.text(
        300,
        12,
        format!("{:?} / FOH", desk.page).to_uppercase(),
        TEXT,
    );
    s.text(
        576,
        12,
        format!("SIMULATION / {:?}", desk.confirmed.mode).to_uppercase(),
        AMBER,
    );
    s.text(
        960,
        12,
        if desk.connected {
            "LINK: SIMULATED"
        } else {
            "LINK: LOST / STALE"
        },
        if desk.connected { DIM } else { RED },
    );
    s.text(1272, 12, "REC: UNAVAILABLE", DIM);
    s.text(
        1536,
        12,
        format!(
            "REV {:04} / CH {:02}",
            desk.confirmed.revision, desk.selected
        ),
        TEXT,
    );
    match desk.page {
        Page::Mix => mix(&mut s, desk),
        Page::Channel => channel(&mut s, desk),
        Page::Analysis => analysis(&mut s, desk),
    }
    s.rect(0, 936, 1920, 144, PANEL);
    s.line(0, 936, 1920, 936, EDGE);
    for i in 0..16 {
        let value = if desk.page != Page::Mix {
            "--".into()
        } else if i < 12 {
            desk.confirmed
                .channels
                .get(desk.bank_start() + i)
                .map_or("--".into(), |c| {
                    format!("CH{:02} {:+.1}", c.id, f64::from(c.gain_mdb) / 1000.0)
                })
        } else {
            match i {
                12 => desk.selected().map_or("--".into(), |c| {
                    format!("SEL {:+.1}", f64::from(c.gain_mdb) / 1000.0)
                }),
                13 => desk
                    .selected()
                    .map_or("--".into(), |c| format!("PAN {:+}", c.pan)),
                14 => "BUS --".into(),
                _ => "MAIN --".into(),
            }
        };
        s.text(
            24 + (i % 8) as u32 * 234,
            944 + (i / 8) as u32 * 24,
            format!("K{:02} {value}", i + 1),
            DIM,
        );
    }
    let pads = [
        "1 MIX",
        "2 CHANNEL",
        "3 ANALYSIS",
        "4 MUTE",
        "5 HOLD",
        "6 RELEASE",
        "7 MODE",
        "8 STATUS",
    ];
    let colors = crate::midi::pad_colors(desk);
    for (i, label) in pads.iter().enumerate() {
        use crate::midi::Color;
        let (background, foreground) = match colors[i] {
            Color::Cyan => ("#205251", CYAN),
            Color::Red => ("#773e47", RED),
            Color::Yellow => ("#473929", AMBER),
            Color::Off => ("#202933", DIM),
            _ => ("#293b4b", TEXT),
        };
        let x = 24 + i as u32 * 234;
        s.rect(x, 996, 222, 32, background);
        s.text(x + 12, 1000, *label, foreground);
    }
    s.text(
        24,
        1044,
        desk.last_result.chars().take(120).collect::<String>(),
        if desk.connected { AMBER } else { RED },
    );
    s
}

fn mix(s: &mut Scene, d: &Desk) {
    let start = d.bank_start();
    s.text(
        24,
        60,
        format!(
            "INPUTS {:02}-{:02} / 12 CH PER OCTAVE",
            start + 1,
            (start + 12).min(d.confirmed.channels.len())
        ),
        DIM,
    );
    s.text(780, 60, "KEYS SELECT / NEVER PLAY", CYAN);
    for (i, c) in d.confirmed.channels.iter().skip(start).take(12).enumerate() {
        let x = 12 + i as u32 * 96;
        s.rect(x, 96, 90, 816, if c.id == d.selected { CYAN } else { EDGE });
        s.rect(
            x + 2,
            98,
            86,
            812,
            if c.id == d.selected { "#203d43" } else { PANEL },
        );
        s.text(
            x + 9,
            112,
            format!("{:02}", c.id),
            if c.id == d.selected { CYAN } else { TEXT },
        );
        s.text(x + 3, 144, c.name.chars().take(7).collect::<String>(), TEXT);
        s.text(
            x + 3,
            168,
            c.name.chars().skip(7).take(7).collect::<String>(),
            TEXT,
        );
        s.text(
            x + 9,
            192,
            if c.held { "HOLD" } else { "AUTO" },
            if c.held { AMBER } else { DIM },
        );
        s.text(x + 9, 232, "PEAK", DIM);
        // Fixture meter, not derived from gain or claimed to measure audio.
        s.rect(x + 12, 280, 12, 264, BG);
        if d.connected {
            let h = 100 + (i as u32 * 47) % 148;
            s.rect(x + 12, 544 - h, 12, h, CYAN);
            s.text(x + 9, 552, format!("-{:02}", 12 + i % 9), DIM);
        } else {
            s.text(x + 9, 552, "---", RED);
        }
        s.line(x + 60, 280, x + 60, 708, EDGE);
        let y = 708 - (((c.gain_mdb + 90000) as u32) * 428 / 102000);
        s.rect(x + 40, y, 40, 16, if c.held { AMBER } else { TEXT });
        s.text(
            x + 3,
            728,
            format!("{:>6.1}", f64::from(c.gain_mdb) / 1000.0),
            TEXT,
        );
        s.text(x + 9, 768, format!("P{:+03}", c.pan), DIM);
        s.rect(
            x + 6,
            816,
            78,
            40,
            if c.muted { "#773e47" } else { "#283847" },
        );
        s.text(
            x + 9,
            824,
            if c.muted { "MUTED" } else { "OPEN" },
            if c.muted { RED } else { DIM },
        );
        s.text(x + 9, 876, format!("K{:02}", i + 1), DIM);
    }
    s.panel(1176, 96, 180, 816, "MAIN / LR");
    s.text(1188, 160, "UNAVAILABLE", AMBER);
    s.text(1188, 208, "METERS --", DIM);
    s.text(1188, 256, "PFL --", DIM);
    s.text(1188, 304, "MON --", DIM);
    s.text(1188, 672, "STATUS --", DIM);
    s.text(1188, 720, "PA OWNS", DIM);
    s.text(1188, 752, "OUTPUT", DIM);
    s.panel(1368, 96, 540, 816, "SELECTED CHANNEL / CONFIRMED");
    if let Some(c) = d.selected() {
        s.text(1392, 160, format!("{:02}  {}", c.id, c.name), CYAN);
        s.text(
            1392,
            216,
            format!("FADER   {:+.1} dB", f64::from(c.gain_mdb) / 1000.0),
            TEXT,
        );
        s.text(
            1392,
            264,
            format!("OWNER   {}", if c.held { "HUMAN HOLD" } else { "AUTOMIX" }),
            AMBER,
        );
        s.text(
            1392,
            312,
            format!("TARGET  {:+.1} dB", f64::from(c.proposed_mdb) / 1000.0),
            DIM,
        );
        s.text(1392, 360, "A move holds this fader.", TEXT);
        s.text(1392, 396, "Release returns it to Auto.", DIM);
    }
    s.line(1392, 456, 1884, 456, EDGE);
    s.text(1392, 480, "SIGNAL PATH / PLANNED", DIM);
    for (i, t) in [
        "INPUT > HPF > EQ > DYNAMICS",
        "FADER > BUS > PA PROTECTION",
        "SENDS > SHR FX > WET RETURNS",
    ]
    .iter()
    .enumerate()
    {
        s.text(1392, 528 + i as u32 * 48, *t, DIM);
    }
    s.text(1392, 744, "MIXING RUNS IN THE CORE.", CYAN);
    s.text(1392, 792, "UI restart recalls nothing.", TEXT);
    s.text(1392, 852, "36 synthetic input identities", DIM);
}

fn channel(s: &mut Scene, d: &Desk) {
    s.panel(12, 72, 432, 840, "CHANNEL / HUMAN + AUTOMIX");
    if let Some(c) = d.selected() {
        s.text(36, 144, format!("{:02} {}", c.id, c.name), CYAN);
        s.text(
            36,
            204,
            format!("APPLIED  {:+.1} dB", f64::from(c.gain_mdb) / 1000.0),
            TEXT,
        );
        s.text(
            36,
            252,
            format!("PROPOSED {:+.1} dB", f64::from(c.proposed_mdb) / 1000.0),
            AMBER,
        );
        s.text(36, 300, format!("HOLD     {}", c.held), TEXT);
    }
    s.text(36, 384, "TAKEOVER IS PER PARAMETER", CYAN);
    s.text(36, 432, "Manual fader move: hold", DIM);
    s.text(36, 480, "No timed return to Auto", DIM);
    s.text(36, 528, "Mode change keeps levels", DIM);
    s.text(36, 576, "Release needs Auto mode", DIM);
    s.text(36, 792, "EQ / dynamics: design only", AMBER);
    s.panel(
        456,
        72,
        1452,
        480,
        "EQ + SPECTRUM / SCHEMATIC / NOT AN AUDIO MEASUREMENT",
    );
    graph_grid(s, 492, 144, 1368, 312);
    for i in 0..228 {
        let x = 492 + i * 6;
        let y = 300.0 + 35.0 * ((f64::from(i) - 80.0) / 32.0).sin();
        s.line(
            x,
            y as u32,
            x + 6,
            (300.0 + 35.0 * ((f64::from(i + 1) - 80.0) / 32.0).sin()) as u32,
            CYAN,
        );
    }
    s.text(
        492,
        480,
        "20 Hz          100 Hz          1 kHz          5 kHz          20 kHz / log axis",
        DIM,
    );
    s.panel(456, 564, 708, 348, "DYNAMICS / CAPABILITY PENDING");
    s.text(480, 636, "GATE    threshold / attack / hold / release", DIM);
    s.text(480, 684, "COMP    threshold / ratio / knee / timing", DIM);
    s.text(480, 756, "GR -- dB  /  DETECTOR --  /  BYPASS --", DIM);
    s.text(
        480,
        828,
        "No editable control without engine support",
        AMBER,
    );
    s.panel(1176, 564, 732, 348, "SENDS / PERFORMER MONITORS / PLANNED");
    for (i, t) in [
        "BUS        LEVEL     TAP       OWNER",
        "Vocal IEM  --        PRE       PA",
        "Band wedge --        PRE       PA",
        "Vocal verb --        POST      SHR FX",
    ]
    .iter()
    .enumerate()
    {
        s.text(1200, 636 + i as u32 * 48, *t, DIM);
    }
}

fn graph_grid(s: &mut Scene, x: u32, y: u32, w: u32, h: u32) {
    s.rect(x, y, w, h, BG);
    for i in 0..=4 {
        s.line(x, y + i * h / 4, x + w, y + i * h / 4, EDGE);
    }
    for i in 0..=8 {
        s.line(x + i * w / 8, y, x + i * w / 8, y + h, EDGE);
    }
}

fn analysis(s: &mut Scene, d: &Desk) {
    s.text(
        24,
        60,
        format!(
            "CH {:02} / SYNTHETIC DISPLAY FIXTURES / NO AUDIO SUBSCRIPTION",
            d.selected
        ),
        AMBER,
    );
    s.panel(12, 108, 1248, 336, "SPECTRUM / ILLUSTRATIVE DATA / dBFS");
    graph_grid(s, 84, 180, 1128, 204);
    s.text(24, 180, "0", DIM);
    s.text(24, 348, "-90", DIM);
    for i in 0..188 {
        let peak = 90.0 * (-((f64::from(i) - 104.0) / 5.0).powi(2)).exp();
        let h = (18.0 + peak + 9.0 * (f64::from(i) * 0.7).sin().abs()) as u32;
        s.rect(84 + i * 6, 384 - h, 4, h, CYAN);
    }
    s.text(
        84,
        396,
        "20 Hz              100 Hz              1 kHz              20 kHz",
        DIM,
    );
    s.panel(
        12,
        456,
        1248,
        456,
        "SPECTROGRAM / SYNTHETIC INTENSITY GRID / NO FFT CLAIM",
    );
    for t in 0..112 {
        for f in 0..24 {
            let n = (t * 11 + f * 7) % 23;
            let color = if f == 9 || f == 10 {
                "#e3b06a"
            } else if (f == 16 || f == 17) && t % 28 < 12 {
                "#66cbbc"
            } else if n < 6 {
                "#2e656e"
            } else {
                "#172d3d"
            };
            s.rect(84 + t * 10, 528 + f * 12, 10, 12, color);
        }
    }
    s.text(
        84,
        840,
        "TIME ->     4 s fixture span / high frequencies at top",
        DIM,
    );
    s.panel(1272, 108, 636, 564, "STEREO FIELD / GENERATED L = R");
    let (cx, cy) = (1590, 372);
    s.line(cx - 180, cy, cx + 180, cy, EDGE);
    s.line(cx, cy - 156, cx, cy + 156, EDGE);
    s.line(cx - 144, cy + 144, cx + 144, cy - 144, CYAN);
    s.text(1320, 576, "CORRELATION +1.00 / SYNTHETIC", CYAN);
    s.text(1320, 624, "L/R axes / no phase meter feed", DIM);
    s.panel(1272, 684, 636, 228, "SUBSCRIPTION / PLANNED");
    s.text(1296, 744, "SOURCE: none / TIMESTAMP: --", DIM);
    s.text(1296, 792, "FFT: -- / WINDOW: -- / AGE: --", DIM);
    s.text(1296, 840, "Stale data must be labelled.", AMBER);
}

/// Bundled, unmodified Debian PSF2 font. Glyph bits and Unicode table are parsed
/// rather than assuming glyph index equals Unicode codepoint.
pub struct Font {
    pub glyphs: Vec<(char, Vec<u8>)>,
}
impl Default for Font {
    fn default() -> Self {
        let data = include_bytes!("../assets/fonts/Uni2-TerminusBold24x12.psf");
        let num = u32::from_le_bytes(data[16..20].try_into().unwrap()) as usize;
        let size = u32::from_le_bytes(data[20..24].try_into().unwrap()) as usize;
        let mut glyphs = Vec::new();
        for (index, mapping) in data[32 + num * size..]
            .split(|b| *b == 255)
            .take(num)
            .enumerate()
        {
            // PSF sequence entries begin at FE; aliases before that map independently.
            let aliases = mapping.split(|b| *b == 254).next().unwrap();
            if let Ok(text) = std::str::from_utf8(aliases) {
                for c in text.chars() {
                    glyphs.push((c, data[32 + index * size..32 + (index + 1) * size].to_vec()));
                }
            }
        }
        Self { glyphs }
    }
}
impl Font {
    pub fn supports(&self, c: char) -> bool {
        self.glyphs.iter().any(|(ch, _)| *ch == c)
    }
    fn path(&self, c: char) -> String {
        let Some((_, bits)) = self
            .glyphs
            .iter()
            .find(|(ch, _)| *ch == c)
            .or_else(|| self.glyphs.iter().find(|(ch, _)| *ch == '?'))
        else {
            return String::new();
        };
        let mut path = String::new();
        for y in 0..24 {
            for x in 0..12 {
                if bits[y * 2 + x / 8] & (0x80 >> (x % 8)) != 0 {
                    write!(path, "M{x} {y}h1v1h-1z").unwrap();
                }
            }
        }
        path
    }
}

pub fn svg(scene: &Scene) -> String {
    let font = Font::default();
    let mut chars = std::collections::BTreeSet::new();
    for p in &scene.primitives {
        if let Primitive::Text { value, .. } = p {
            chars.extend(value.chars());
        }
    }
    let mut out = format!(
        "<svg xmlns=\"http://www.w3.org/2000/svg\" width=\"{WIDTH}\" height=\"{HEIGHT}\" viewBox=\"0 0 {WIDTH} {HEIGHT}\" role=\"img\"><title>SHR Desk offline simulation</title><defs>"
    );
    for c in chars {
        writeln!(out, "<path id=\"g{}\" d=\"{}\"/>", c as u32, font.path(c)).unwrap();
    }
    out.push_str("</defs><g shape-rendering=\"crispEdges\">\n");
    for p in &scene.primitives {
        match p {
            Primitive::Rect { x, y, w, h, fill } => writeln!(
                out,
                "<rect x=\"{x}\" y=\"{y}\" width=\"{w}\" height=\"{h}\" fill=\"{fill}\"/>"
            )
            .unwrap(),
            Primitive::Line {
                x1,
                y1,
                x2,
                y2,
                color,
            } => writeln!(
                out,
                "<path d=\"M{x1} {y1}L{x2} {y2}\" fill=\"none\" stroke=\"{color}\"/>"
            )
            .unwrap(),
            Primitive::Text { x, y, value, color } => {
                writeln!(
                    out,
                    "<g fill=\"{color}\"><title>{}</title>",
                    value
                        .replace('&', "&amp;")
                        .replace('<', "&lt;")
                        .replace('>', "&gt;")
                )
                .unwrap();
                for (i, c) in value.chars().enumerate() {
                    writeln!(
                        out,
                        "<use href=\"#g{}\" x=\"{}\" y=\"{y}\"/>",
                        c as u32,
                        x + i as u32 * 12
                    )
                    .unwrap();
                }
                out.push_str("</g>\n");
            }
        }
    }
    out.push_str("</g></svg>\n");
    out
}

/// Provider values stay nullable; no synthetic meter/graph or simulator conversion.
pub fn provider_scene(client: &crate::provider::Client, now_ms: u64) -> Scene {
    let mut s = Scene::default();
    s.rect(0, 0, WIDTH, HEIGHT, BG);
    for (row, line) in client.lines(now_ms).iter().enumerate() {
        // The wire ID bounds exceed a full-HD row; clip presentation only.
        let line: String = line.chars().take(156).collect();
        s.text(
            24,
            24 + row as u32 * 36,
            line,
            if row == 0 { CYAN } else { TEXT },
        );
    }
    s.text(
        24,
        1008,
        "READ ONLY / no devices / no audio observations / no engine writes",
        DIM,
    );
    s
}
