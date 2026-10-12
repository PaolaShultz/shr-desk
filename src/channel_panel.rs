//! Selected-channel equipment layout over confirmed provider data, never UI DSP state.
use crate::{frontend::Frontend, processing::FIELDS};
use shr_instrument_panel::terminal::ratatui::{
    buffer::Buffer,
    layout::Rect,
    style::Color,
    widgets::{
        Widget,
        canvas::{Canvas, Line},
    },
};
use shr_instrument_panel::{Material, rack::*};
#[derive(Clone, Debug)]
pub enum Target {
    Key(&'static str),
    Field(usize),
}
#[derive(Clone, Debug)]
pub struct Hit {
    pub area: Rect,
    pub target: Target,
}
fn key(
    b: &mut Buffer,
    h: &mut Vec<Hit>,
    a: Rect,
    label: &str,
    k: &'static str,
    on: bool,
    m: Material,
) {
    button(b, a, label, on, k == "M" && on, m);
    h.push(Hit {
        area: a,
        target: Target::Key(k),
    });
}
fn field(
    b: &mut Buffer,
    h: &mut Vec<Hit>,
    a: Rect,
    label: &str,
    value: &str,
    index: usize,
    selected: bool,
) {
    text(
        b,
        a.x,
        a.y,
        a.width,
        label,
        if selected { CYAN } else { DIM },
    );
    fill(
        b,
        Rect::new(a.x, a.y + 1, a.width, 1),
        if selected {
            Color::Rgb(32, 69, 81)
        } else {
            WELL
        },
    );
    text(
        b,
        a.x + 1,
        a.y + 1,
        a.width - 1,
        value,
        if selected { CYAN } else { TEXT },
    );
    h.push(Hit {
        area: a,
        target: Target::Field(index),
    });
}
pub fn draw(f: &Frontend, b: &mut Buffer, m: Material) -> Vec<Hit> {
    let mut hits = Vec::new();
    if b.area.width < 160 || b.area.height < 45 {
        text(
            b,
            0,
            0,
            b.area.width,
            "Resize to 160 x 45 cells. Controls paused.",
            TEXT,
        );
        return hits;
    }
    let Some(u) = &f.state else { return hits };
    let Some(raw) = &u.snapshot else { return hits };
    let Some(input) = raw.authority.inputs.get(f.selected) else {
        return hits;
    };
    let parameter = |name: &str| {
        raw.authority.parameters.iter().find(|p| {
            p.target.input == *input && p.target.parameter == name && p.target.monitor.is_none()
        })
    };
    let fader = parameter("fader");
    let pan = parameter("pan");
    let mute = parameter("mute");
    fill(b, b.area, Color::Rgb(19, 23, 27));
    fill(b, Rect::new(0, 0, 160, 2), Color::Rgb(91, 26, 34));
    text(b, 2, 0, 20, "SHR / DESK", TEXT);
    text(
        b,
        24,
        0,
        60,
        &format!("CHANNEL {:02}   /   {}", f.selected + 1, input),
        TEXT,
    );
    text(
        b,
        2,
        1,
        145,
        "SELECTED CHANNEL     INPUT → FOUR-BAND EQ → COMPRESSOR → MUTE / FADER / PAN → FOH",
        Color::Rgb(220, 177, 179),
    );
    indicator(b, 110, 0, "green", f.fresh() && !raw.faulted, m);
    text(
        b,
        114,
        0,
        16,
        if raw.faulted {
            "OWNER FAULT"
        } else if f.fresh() {
            "OWNER FRESH"
        } else {
            "OWNER STALE"
        },
        TEXT,
    );
    text(
        b,
        132,
        0,
        27,
        if !f.fresh() || raw.faulted || f.role_status.as_ref().is_some_and(|r| !r.live) {
            "WRITES BLOCKED"
        } else if u.writer_granted() {
            "WRITER GRANTED"
        } else {
            "READ ONLY"
        },
        AMBER,
    );
    text(
        b,
        2,
        3,
        156,
        &format!(
            "{}  /  epoch {}  /  revision {}  /  no physical output qualification",
            raw.protection, raw.authority.epoch, raw.authority.revision
        ),
        AMBER,
    );
    for (x, label, k) in [
        (2, "MIX", "F1"),
        (15, "CHANNEL", "F2"),
        (32, "HEALTH", "F6"),
        (48, "SENDS", "F12"),
        (77, "◀ CHANNEL", "Left"),
        (88, "CHANNEL ▶", "Right"),
        (102, "◀ BANK", "PageUp"),
        (111, "BANK ▶", "PageDown"),
    ] {
        text(
            b,
            x,
            2,
            label.chars().count() as u16,
            label,
            if k == "F2" { CYAN } else { DIM },
        );
        hits.push(Hit {
            area: Rect::new(x, 2, label.chars().count() as u16, 1),
            target: Target::Key(k),
        });
    }
    panel(b, Rect::new(1, 4, 84, 25), "01 / PARAMETRIC EQUALIZER");
    panel(b, Rect::new(86, 4, 48, 25), "02 / COMPRESSOR");
    panel(b, Rect::new(135, 4, 24, 36), "03 / OUTPUT · FOH");
    let proc = u.processing.as_ref();
    let channel = proc.and_then(|p| p.channels.iter().find(|c| c.input == *input));
    if let Some(c) = channel {
        let cfg = f.processing_draft.as_ref().map_or(&c.target, |d| &d.config);
        let selected = |i| f.processing_draft.is_some() && f.processing_field == i;
        let status = if !f.processing_fresh() {
            "STALE · retained settings"
        } else if proc.is_some_and(|p| p.faulted) {
            "FAULT · edits blocked"
        } else if !c.ready {
            "TRANSITION · target settings"
        } else if f.processing_draft.is_some() {
            "LOCAL DRAFT · not applied"
        } else {
            "CONFIRMED TARGET"
        };
        text(
            b,
            3,
            6,
            48,
            status,
            if f.processing_draft.is_some() {
                AMBER
            } else {
                CYAN
            },
        );
        field(
            b,
            &mut hits,
            Rect::new(65, 5, 17, 2),
            "EQUALIZER",
            if cfg.eq_bypass { "BYPASS" } else { "IN" },
            0,
            selected(0),
        );
        indicator(
            b,
            60,
            6,
            "green",
            f.processing_fresh() && !c.current.eq_bypass,
            m,
        );
        indicator(
            b,
            108,
            6,
            "green",
            f.processing_fresh() && !c.current.compressor_bypass,
            m,
        );
        well(b, Rect::new(3, 8, 79, 10));
        let graph = Rect::new(9, 9, 71, 7);
        if let Ok(bank) = crate::eq_response::channel_coefficients(cfg, proc.unwrap().sample_rate) {
            let points: Vec<_> = (0..=280)
                .map(|i| {
                    let x = i as f64 / 280. * 3.;
                    let hz = 20. * 10f64.powf(x);
                    let y = crate::eq_response::response(&bank, 48000, hz).map_or(0., |v| v.0);
                    (x, y)
                })
                .collect();
            Canvas::default()
                .background_color(WELL)
                .x_bounds([0., 3.])
                .y_bounds([-18., 18.])
                .paint(|ctx| {
                    for db in [-12., 0., 12.] {
                        ctx.draw(&Line {
                            x1: 0.,
                            x2: 3.,
                            y1: db,
                            y2: db,
                            color: Color::Rgb(45, 61, 67),
                        });
                    }
                    for hz in [100f64, 1000., 10000.] {
                        let x = (hz / 20.).log10();
                        ctx.draw(&Line {
                            x1: x,
                            x2: x,
                            y1: -18.,
                            y2: 18.,
                            color: Color::Rgb(37, 49, 58),
                        });
                    }
                    for p in points.windows(2) {
                        ctx.draw(&Line {
                            x1: p[0].0,
                            y1: p[0].1,
                            x2: p[1].0,
                            y2: p[1].1,
                            color: if f.processing_draft.is_some() {
                                AMBER
                            } else {
                                CYAN
                            },
                        });
                    }
                })
                .render(graph, b);
        }
        text(b, 4, 10, 5, "+12", DIM);
        text(b, 4, 12, 5, "  0", DIM);
        text(b, 4, 14, 5, "-12", DIM);
        for (hz, label) in [
            (20_f64, "20"),
            (100., "100"),
            (1000., "1k"),
            (10000., "10k"),
            (20000., "20k"),
        ] {
            let x = 9 + ((hz / 20.).log10() / 3. * 70.).round() as u16;
            text(
                b,
                x.saturating_sub((label.len() / 2) as u16),
                16,
                5,
                label,
                DIM,
            );
        }
        text(
            b,
            5,
            8,
            66,
            "EQ RESPONSE / ±18 dB window · calculated settings",
            DIM,
        );
        for (i, (hz, g, q, bypass)) in cfg.bands().iter().enumerate() {
            let x = 3 + i as u16 * 20;
            let base = 1 + i * 4;
            text(
                b,
                x,
                18,
                18,
                &format!("BAND {} / BELL", i + 1),
                [
                    CYAN,
                    Color::Rgb(164, 202, 139),
                    AMBER,
                    Color::Rgb(190, 165, 222),
                ][i],
            );
            encoder(b, x, 19, (*hz as f64 / 20.).log10() / 3., m);
            field(
                b,
                &mut hits,
                Rect::new(x + 7, 19, 11, 2),
                "Hz",
                &hz.to_string(),
                base,
                selected(base),
            );
            field(
                b,
                &mut hits,
                Rect::new(x, 23, 9, 2),
                "GAIN dB",
                &format!("{:+.1}", *g as f64 / 1000.),
                base + 1,
                selected(base + 1),
            );
            field(
                b,
                &mut hits,
                Rect::new(x + 10, 23, 8, 2),
                "Q",
                &format!("{:.1}", *q as f64 / 1000.),
                base + 2,
                selected(base + 2),
            );
            field(
                b,
                &mut hits,
                Rect::new(x, 26, 18, 2),
                "BAND STATE",
                if *bypass { "BYPASS" } else { "IN" },
                base + 3,
                selected(base + 3),
            );
            let settled_bypass = c.current.bands()[i].3;
            indicator(
                b,
                x + 14,
                26,
                "green",
                f.processing_fresh() && !c.current.eq_bypass && !settled_bypass,
                m,
            );
            hits.push(Hit {
                area: Rect::new(x, 19, 6, 3),
                target: Target::Field(base),
            });
        }
        field(
            b,
            &mut hits,
            Rect::new(112, 5, 19, 2),
            "COMPRESSOR",
            if cfg.compressor_bypass {
                "BYPASS"
            } else {
                "IN"
            },
            17,
            selected(17),
        );
        text(b, 88, 6, 22, "GAIN REDUCTION / dB", DIM);
        well(b, Rect::new(88, 8, 43, 3));
        if f.processing_fresh() && !proc.unwrap().faulted && c.ready {
            if c.target.compressor_bypass {
                text(b, 90, 9, 39, "BYPASSED", DIM);
            } else if let Some(gr) = c.gain_reduction_mdb {
                let n = ((gr as f64 / 24000.).clamp(0., 1.) * 30.).round() as u16;
                for x in 0..30 {
                    text(
                        b,
                        90 + x,
                        9,
                        1,
                        "▰",
                        if x < n { AMBER } else { Color::Rgb(41, 52, 59) },
                    );
                }
                text(b, 122, 9, 8, &format!("{:.1}", gr as f64 / 1000.), AMBER);
            } else {
                text(b, 90, 9, 39, "NO DETECTOR TELEMETRY", DIM);
            }
        } else {
            text(b, 90, 9, 39, "STALE / NO LIVE GR", AMBER);
        }
        text(b, 90, 11, 39, "0          12           24 dB", DIM);
        let knobs = [
            (
                "THRESH dBFS",
                format!("{:.1}", cfg.threshold_mdb as f64 / 1000.),
                18,
                (cfg.threshold_mdb as f64 + 60000.) / 60000.,
            ),
            (
                "RATIO",
                format!("{:.1}:1", cfg.ratio_milli as f64 / 1000.),
                19,
                (cfg.ratio_milli as f64 - 1000.) / 19000.,
            ),
            (
                "ATTACK ms",
                format!("{:.1}", cfg.attack_us as f64 / 1000.),
                21,
                cfg.attack_us as f64 / 200000.,
            ),
            (
                "RELEASE ms",
                cfg.release_ms.to_string(),
                22,
                cfg.release_ms as f64 / 2000.,
            ),
            (
                "KNEE dB",
                format!("{:.1}", cfg.knee_mdb as f64 / 1000.),
                20,
                cfg.knee_mdb as f64 / 18000.,
            ),
            (
                "MAKEUP dB",
                format!("{:+.1}", cfg.makeup_mdb as f64 / 1000.),
                23,
                (cfg.makeup_mdb as f64 + 12000.) / 24000.,
            ),
        ];
        for (i, (label, value, index, n)) in knobs.iter().enumerate() {
            let x = 88 + (i as u16 % 2) * 22;
            let y = 13 + (i as u16 / 2) * 5;
            encoder(b, x, y + 1, *n, m);
            field(
                b,
                &mut hits,
                Rect::new(x + 7, y + 1, 13, 2),
                label,
                value,
                *index,
                selected(*index),
            );
            hits.push(Hit {
                area: Rect::new(x, y, 21, 4),
                target: Target::Field(*index),
            });
        }
    } else {
        // No decorative pretend processors when this owner has no processing path.
        fill(b, Rect::new(1, 4, 133, 25), FACE);
        text(
            b,
            3,
            6,
            125,
            "Channel processing has not been connected. Reconnect with --processing to probe the owner.",
            AMBER,
        );
    }
    let db = fader
        .and_then(|p| p.target_value.as_i64())
        .map(|v| v as f64 / 1000.);
    let pv = pan.and_then(|p| p.target_value.as_i64()).unwrap_or(0);
    text(b, 137, 6, 20, "PAN", DIM);
    encoder(b, 138, 7, (pv as f64 + 100.) / 200., m);
    text(
        b,
        146,
        8,
        11,
        &if pv == 0 {
            "CENTER".into()
        } else {
            format!("{} {}", if pv < 0 { "L" } else { "R" }, pv.abs())
        },
        TEXT,
    );
    hits.push(Hit {
        area: Rect::new(137, 6, 20, 4),
        target: Target::Key("C"),
    });
    text(b, 137, 11, 20, "LEVEL TARGET", DIM);
    well(b, Rect::new(137, 12, 20, 3));
    text(
        b,
        139,
        13,
        17,
        &db.map_or("—".into(), |v| format!("{v:+.1} dB")),
        TEXT,
    );
    if let Some(db) = db {
        fader_draw(b, Rect::new(137, 16, 20, 17), db, m);
    }
    hits.push(Hit {
        area: Rect::new(137, 12, 20, 21),
        target: Target::Key("D"),
    });
    key(
        b,
        &mut hits,
        Rect::new(137, 33, 20, 3),
        if mute.is_some_and(|p| p.target_value.as_bool() == Some(true)) {
            "MUTED"
        } else {
            "MUTE"
        },
        "M",
        mute.is_some_and(|p| p.target_value.as_bool() == Some(true)),
        m,
    );
    key(
        b,
        &mut hits,
        Rect::new(137, 36, 20, 3),
        if fader.is_some_and(|p| p.hold.is_some()) {
            "HELD"
        } else {
            "HOLD"
        },
        "H",
        fader.is_some_and(|p| p.hold.is_some()),
        m,
    );
    indicator(
        b,
        138,
        34,
        "red",
        f.fresh() && mute.is_some_and(|p| p.target_value.as_bool() == Some(true)),
        m,
    );
    indicator(
        b,
        138,
        37,
        "yellow",
        f.fresh() && fader.is_some_and(|p| p.hold.is_some()),
        m,
    );
    panel(b, Rect::new(1, 30, 84, 10), "04 / MONITOR SENDS");
    let sends: Vec<_> = raw
        .authority
        .parameters
        .iter()
        .filter(|p| p.target.input == *input && p.target.parameter == "send")
        .collect();
    for (i, p) in sends.iter().take(2).enumerate() {
        let x = 3 + (i as u16 % 2) * 40;
        let y = 32 + (i as u16 / 2) * 3;
        let value = p.target_value.as_i64().map(|v| v as f64 / 1000.);
        text(
            b,
            x,
            y,
            23,
            p.target.monitor.as_deref().unwrap_or("monitor"),
            TEXT,
        );
        text(
            b,
            x + 25,
            y,
            12,
            &value.map_or("—".into(), |v| format!("{v:+.1} dB")),
            CYAN,
        );
        for n in 0..36 {
            text(
                b,
                x + n,
                y + 1,
                1,
                "━",
                if value.is_some_and(|v| n as f64 / 36. < (v + 60.) / 72.) {
                    Color::Rgb(104, 163, 171)
                } else {
                    Color::Rgb(22, 29, 34)
                },
            );
        }
        hits.push(Hit {
            area: Rect::new(x, y, 37, 2),
            target: Target::Key("F12"),
        });
    }
    if let Some(tap) = f
        .meters
        .snapshot
        .as_ref()
        .and_then(|s| s.taps.iter().find(|t| t.id == format!("{input}:raw")))
    {
        let live = f.meters.fresh(std::time::Instant::now()) && tap.valid;
        text(
            b,
            3,
            35,
            77,
            &format!(
                "INPUT SAMPLE PEAK  {}",
                if !live {
                    "STALE".into()
                } else if tap.silent {
                    "-inf dBFS".into()
                } else {
                    tap.peak_millidbfs.map_or("NO SAMPLE".into(), |p| {
                        format!("{:.1} dBFS", p as f64 / 1000.)
                    })
                }
            ),
            DIM,
        );
        let n = if live {
            tap.peak_millidbfs
                .map(|p| ((p as f64 / 1000. + 60.) / 60.).clamp(0., 1.))
                .unwrap_or(0.)
        } else {
            0.
        };
        for x in 0..76 {
            text(
                b,
                3 + x,
                36,
                1,
                "▰",
                if (x as f64 / 76.) < n {
                    if x > 69 {
                        RED
                    } else if x > 60 {
                        AMBER
                    } else {
                        CYAN
                    }
                } else {
                    Color::Rgb(23, 31, 37)
                },
            );
        }
        text(
            b,
            3,
            37,
            77,
            "-60                  -40                       -20                   0 dBFS",
            DIM,
        );
    }
    text(
        b,
        3,
        38,
        78,
        &format!(
            "{} destinations · select SENDS for levels and taps",
            sends.len()
        ),
        DIM,
    );
    panel(b, Rect::new(86, 30, 48, 10), "05 / EDIT & OWNERSHIP");
    if let Some(c) = channel {
        let index = f.processing_field.min(23);
        text(
            b,
            88,
            32,
            43,
            &format!("SETTLED  {}", c.current.display(FIELDS[index])),
            DIM,
        );
        text(
            b,
            88,
            33,
            43,
            &format!("TARGET   {}", c.target.display(FIELDS[index])),
            TEXT,
        );
        if let Some(d) = &f.processing_draft {
            text(
                b,
                88,
                34,
                43,
                &format!("DRAFT    {}", d.config.display(FIELDS[index])),
                AMBER,
            );
            text(
                b,
                88,
                35,
                43,
                &format!("ENTRY    {}", f.processing_entry),
                CYAN,
            );
        } else {
            text(b, 88, 35, 43, "Select a parameter to edit", CYAN);
        }
    }
    text(
        b,
        88,
        37,
        43,
        &format!(
            "FADER OWNER  {}",
            fader.and_then(|p| p.owner.as_deref()).unwrap_or("unowned")
        ),
        DIM,
    );
    text(
        b,
        88,
        38,
        43,
        &format!(
            "MODE  {}",
            raw.authority
                .modes
                .iter()
                .find(|(s, _)| s == "foh")
                .map_or("—", |(_, m)| m.as_str())
        ),
        AMBER,
    );
    for (x, w, label, k, on) in [
        (1, 23, "EDIT PROCESSING", "E", false),
        (25, 22, "REVIEW / APPLY", "F4", f.processing_draft.is_some()),
        (48, 19, "CANCEL", "Esc", false),
        (68, 20, "RELEASE HOLD", "R", false),
        (89, 21, "WRITER GRANT", "G", u.writer_granted()),
        (111, 23, "SENDS", "F12", false),
        (135, 24, "RECONNECT", "F5", false),
    ] {
        key(b, &mut hits, Rect::new(x, 40, w, 3), label, k, on, m);
    }
    let focus = f
        .processing_draft
        .as_ref()
        .map(|_| FIELDS[f.processing_field.min(23)]);
    indicator(b, 2, 43, "blue", focus.is_some(), m);
    text(
        b,
        6,
        43,
        75,
        &focus.map_or("NO PARAMETER SELECTED".into(), |field| {
            format!(
                "SELECTED  {}",
                f.processing_draft.as_ref().unwrap().config.display(field)
            )
        }),
        if focus.is_some() { CYAN } else { DIM },
    );
    indicator(b, 88, 43, "yellow", f.processing_draft.is_some(), m);
    text(
        b,
        92,
        43,
        40,
        if f.processing_draft.is_some() {
            "LOCAL DRAFT · NOT APPLIED"
        } else {
            "NO PENDING DRAFT"
        },
        DIM,
    );
    let message = if f.processing_draft.is_some() {
        f.message.as_str()
    } else {
        u.last_operation
            .as_deref()
            .unwrap_or(if f.message.is_empty() {
                &u.status
            } else {
                &f.message
            })
    };
    let message = if f.processing_draft.is_some() && message.starts_with("Local draft only.") {
        "Local draft only. Review before applying; Cancel discards edits."
    } else {
        message
    };
    text(b, 2, 44, 156, message, AMBER);
    hits
}
fn fader_draw(b: &mut Buffer, a: Rect, db: f64, m: Material) {
    shr_instrument_panel::rack::fader(b, a, db, m);
}
