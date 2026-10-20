//! Selected Channel follows the MiniLab's two rotary rows and eight pad columns.
//! Provider state/actions retain authority; the layout is not a learned MIDI map.
use crate::frontend::Frontend;
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
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum Menu {
    #[default]
    Channel,
    Band(usize),
    Compressor,
    Output,
}
#[derive(Clone, Copy, Debug, Default)]
pub struct PanelUi {
    pub menu: Menu,
    pub secondary: bool,
}
#[derive(Clone, Debug)]
pub enum Target {
    Key(&'static str),
    Field(usize),
    Menu(Menu),
    Layer,
}
#[derive(Clone, Debug)]
pub struct Hit {
    pub area: Rect,
    pub target: Target,
}
fn readout(b: &mut Buffer, h: &mut Vec<Hit>, a: Rect, value: &str, target: Target, selected: bool) {
    fill(
        b,
        a,
        if selected {
            Color::Rgb(32, 69, 81)
        } else {
            WELL
        },
    );
    text(
        b,
        a.x + 1,
        a.y,
        a.width.saturating_sub(2),
        value,
        if selected { CYAN } else { TEXT },
    );
    h.push(Hit { area: a, target });
}
fn pot(
    b: &mut Buffer,
    h: &mut Vec<Hit>,
    (column, row): (u16, u16),
    label: &str,
    (value, n): (&str, f64),
    (target, selected): (Target, bool),
    m: Material,
) {
    let x = column * 20;
    let y = if row == 0 { 24 } else { 33 };
    text(
        b,
        x + 2,
        y - 1,
        18,
        &format!("{:02} {label}", column + 1 + row * 8),
        if selected { CYAN } else { DIM },
    );
    encoder(b, x + 6, y, n, m);
    h.push(Hit {
        area: Rect::new(x + 4, y, 12, 4),
        target: target.clone(),
    });
    readout(
        b,
        h,
        Rect::new(x + 2, y + 5, 16, 1),
        value,
        target,
        selected,
    );
}
fn pad(
    b: &mut Buffer,
    h: &mut Vec<Hit>,
    (col, number): (u16, u16),
    label: &str,
    target: Target,
    (colour, on): (&str, bool),
    m: Material,
) {
    let x = 1 + col * 20;
    let area = Rect::new(x, 44, 18, 1);
    let face = if colour == "red" && on {
        Color::Rgb(113, 37, 43)
    } else if on {
        Color::Rgb(57, 78, 89)
    } else {
        Color::Rgb(87, 94, 103)
    };
    fill(b, area, face);
    let label = match label {
        "REVIEW / APPLY" => "REVIEW",
        "WRITER GRANT" => "GRANT",
        "RELEASE HOLD" => "RELEASE",
        other => other,
    };
    text(b, x + 1, 44, 2, &format!("{number:02}"), TEXT);
    text(b, x + 4, 44, 11, label, TEXT);
    indicator(b, x + 15, 44, colour, on, m);
    h.push(Hit { area, target });
}
// Digital peak scale: green below -12 dBFS, amber to -4, red near full scale.
// Dim coloured segments describe the scale, never a valid observation.
fn meter_colour(db: i32, lit: bool) -> Color {
    match (db >= -4000, db >= -12000, lit) {
        (true, _, true) => RED,
        (true, _, false) => Color::Rgb(72, 30, 34),
        (_, true, true) => AMBER,
        (_, true, false) => Color::Rgb(69, 53, 28),
        (_, _, true) => Color::Rgb(59, 224, 113),
        (_, _, false) => Color::Rgb(25, 59, 40),
    }
}
/// Default entry retained for fallback and focused workflow regression tests.
/// Observed raw input levels and independently fresh owner gain reduction.
/// Reduction fills downward; missing observations leave an unlit scale and dash.
fn channel_meter(
    b: &mut Buffer,
    x: u16,
    label: &str,
    value: Option<i32>,
    reduction: bool,
    silent: bool,
) {
    well(b, Rect::new(x, 3, 14, 19));
    text(b, x + (14 - label.len() as u16) / 2, 3, 12, label, DIM);
    let amount = value.map_or(0., |v| {
        if reduction {
            (f64::from(v) / 24000.).clamp(0., 1.)
        } else {
            ((f64::from(v) + 60000.) / 60000.).clamp(0., 1.)
        }
    });
    for row in 0..16u16 {
        let threshold = if reduction {
            f64::from(row) / 15.
        } else {
            f64::from(15 - row) / 15.
        };
        let colour = if value.is_some() && !silent && amount > 0. && threshold <= amount {
            if reduction {
                AMBER
            } else {
                meter_colour(-i32::from(row) * 4000, true)
            }
        } else if reduction {
            Color::Rgb(29, 39, 45)
        } else {
            meter_colour(-i32::from(row) * 4000, false)
        };
        text(b, x + 2, 4 + row, 4, "▰▰▰▰", colour);
    }
    let scale: &[(u16, &str)] = if reduction {
        &[(4, " 0"), (9, " 8"), (14, "16"), (19, "24")]
    } else {
        &[
            (4, "  0"),
            (5, " -4"),
            (7, "-12"),
            (10, "-24"),
            (13, "-36"),
            (16, "-48"),
            (19, "-60"),
        ]
    };
    for &(row, level) in scale {
        text(
            b,
            x + 8,
            row,
            5,
            level,
            if reduction {
                DIM
            } else {
                meter_colour(-i32::from(row - 4) * 4000, true)
            },
        );
    }
    let reading = if silent {
        "−inf".into()
    } else {
        value.map_or("—".into(), |v| format!("{:.1}", f64::from(v) / 1000.))
    };
    text(
        b,
        x + 2,
        20,
        11,
        &reading,
        if reduction { AMBER } else { CYAN },
    );
}

pub fn draw(f: &Frontend, b: &mut Buffer, m: Material) -> Vec<Hit> {
    draw_with_ui(f, b, m, &PanelUi::default())
}
pub fn draw_with_ui(f: &Frontend, b: &mut Buffer, m: Material, ui: &PanelUi) -> Vec<Hit> {
    let spectrum = f.spectrum.as_ref().and_then(|w| w.view());
    draw_observation(f, b, m, ui, spectrum.as_ref())
}
/// Draw an already validated observation. Explicit benchmark fixtures use this same renderer.
pub fn draw_observation(
    f: &Frontend,
    b: &mut Buffer,
    m: Material,
    ui: &PanelUi,
    spectrum: Option<&crate::spectrum::View>,
) -> Vec<Hit> {
    if f.eq_fullscreen {
        return crate::fullscreen_eq::draw(f, b, spectrum);
    }
    let mut hits = Vec::new();
    if b.area.width < 160 || b.area.height < 45 {
        text(
            b,
            0,
            0,
            b.area.width,
            "Resize to 160 x 45, or F11 fullscreen EQ/RTA (60 x 18).",
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
    let muted = mute.is_some_and(|p| p.target_value.as_bool() == Some(true));
    let held = fader.is_some_and(|p| p.hold.is_some());
    fill(b, b.area, Color::Rgb(19, 23, 27));
    panel(b, Rect::new(1, 0, 78, 44), "");
    panel(b, Rect::new(81, 0, 58, 44), "");
    panel(b, Rect::new(141, 0, 18, 44), "");
    text(
        b,
        3,
        1,
        32,
        &format!("{:02} / {}", f.selected + 1, input),
        DIM,
    );
    text(
        b,
        36,
        1,
        20,
        if ui.secondary {
            "PADS 9–16"
        } else {
            "PADS 1–8"
        },
        CYAN,
    );
    hits.push(Hit {
        area: Rect::new(36, 1, 20, 1),
        target: Target::Layer,
    });
    let proc = u.processing.as_ref();
    let channel = proc.and_then(|p| p.channels.iter().find(|c| c.input == *input));
    if let Some(c) = channel {
        let cfg = f.processing_draft.as_ref().map_or(&c.target, |d| &d.config);
        let selected = |i| f.processing_draft.is_some() && f.processing_field == i;
        indicator(
            b,
            60,
            1,
            "green",
            f.processing_fresh() && !c.current.eq_bypass,
            m,
        );
        readout(
            b,
            &mut hits,
            Rect::new(64, 1, 13, 1),
            if !f.processing_fresh() {
                "EQ STALE"
            } else if cfg.eq_bypass {
                "EQ BYPASS"
            } else {
                "EQ IN"
            },
            Target::Field(0),
            selected(0),
        );
        readout(
            b,
            &mut hits,
            Rect::new(3, 2, 38, 1),
            "F11 Fullscreen EQ / RTA",
            Target::Key("F11"),
            false,
        );
        well(b, Rect::new(3, 3, 74, 19));
        let graph = Rect::new(8, 4, 63, 16);
        if let Ok(bank) = crate::eq_response::channel_coefficients(cfg, proc.unwrap().sample_rate) {
            let points: Vec<_> = (0..=280)
                .map(|i| {
                    let x = i as f64 / 280. * 3.;
                    let hz = 20. * 10f64.powf(x);
                    let y = crate::eq_response::response(&bank, proc.unwrap().sample_rate, hz)
                        .map_or(0., |v| v.0);
                    (x, y)
                })
                .collect();
            Canvas::default()
                .background_color(WELL)
                .x_bounds([0., 3.])
                .y_bounds([-18., 18.])
                .paint(|ctx| {
                    if let Some(s) = &spectrum {
                        let bins = s
                            .fft_bins
                            .as_ref()
                            .map(|v| crate::spectrum::aggregate(v, s.rate, 64))
                            .unwrap_or_else(|| s.bins.clone());
                        for (i, db) in bins.iter().enumerate() {
                            let x = (i as f64 + 0.5) * 3. / 64.;
                            let y =
                                -18. + (f64::from(*db) / 1000. + 96.).clamp(0., 96.) * 36. / 96.;
                            for offset in [-0.012, -0.006, 0., 0.006, 0.012] {
                                for step in 0..48 {
                                    let db = -96. + step as f64 * 2.;
                                    let bottom = -18. + (db + 96.) * 36. / 96.;
                                    if y > bottom {
                                        ctx.draw(&Line {
                                            x1: x + offset,
                                            x2: x + offset,
                                            y1: bottom,
                                            y2: y.min(bottom + 0.75),
                                            color: crate::fullscreen_eq::level_colour(db),
                                        });
                                    }
                                }
                            }
                            let py = -18.
                                + (f64::from(s.peaks[i]) / 1000. + 96.).clamp(0., 96.) * 36. / 96.;
                            ctx.draw(&Line {
                                x1: x - 0.012,
                                x2: x + 0.012,
                                y1: py,
                                y2: py,
                                color: Color::Rgb(97, 123, 137),
                            });
                        }
                    }
                    for db in [-12., -6., 0., 6., 12.] {
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
        for (y, s) in [
            (4, "+18"),
            (7, "+12"),
            (9, " +6"),
            (12, "  0"),
            (14, " -6"),
            (17, "-12"),
            (19, "-18"),
        ] {
            text(b, 4, y, 4, s, DIM);
        }
        for (hz, label) in [
            (20f64, "20"),
            (100., "100"),
            (1000., "1k"),
            (10000., "10k"),
            (20000., "20k"),
        ] {
            let x = 8 + ((hz / 20.).log10() / 3. * 62.).round() as u16;
            text(
                b,
                x.saturating_sub(label.len() as u16 / 2),
                20,
                5,
                label,
                DIM,
            );
        }
        for (i, (hz, g, q, bypass)) in cfg.bands().iter().enumerate() {
            let x = i as u16 * 20;
            let base = 1 + i * 4;
            pot(
                b,
                &mut hits,
                (i as u16, 0),
                "FREQ / Hz",
                (&hz.to_string(), (*hz as f64 / 20.).log10() / 3.),
                (Target::Field(base), selected(base)),
                m,
            );
            pot(
                b,
                &mut hits,
                (i as u16, 1),
                "GAIN / dB",
                (
                    &format!("{:+.1}", *g as f64 / 1000.),
                    (*g as f64 + 18000.) / 36000.,
                ),
                (Target::Field(base + 1), selected(base + 1)),
                m,
            );
            readout(
                b,
                &mut hits,
                Rect::new(x + 2, 40, 8, 1),
                &format!("Q {:.1}", *q as f64 / 1000.),
                Target::Field(base + 2),
                selected(base + 2),
            );
            readout(
                b,
                &mut hits,
                Rect::new(x + 11, 40, 7, 1),
                if *bypass { "BYP" } else { "IN" },
                Target::Field(base + 3),
                selected(base + 3),
            );
            indicator(
                b,
                x + 14,
                42,
                "green",
                f.processing_fresh() && !c.current.eq_bypass && !c.current.bands()[i].3,
                m,
            );
        }
        indicator(
            b,
            121,
            1,
            "green",
            f.processing_fresh() && !c.current.compressor_bypass,
            m,
        );
        readout(
            b,
            &mut hits,
            Rect::new(125, 1, 12, 1),
            if !f.processing_fresh() {
                "COMP STALE"
            } else if cfg.compressor_bypass {
                "BYPASS"
            } else {
                "COMP IN"
            },
            Target::Field(17),
            selected(17),
        );
        let gr = if f.processing_fresh() && c.ready {
            c.gain_reduction_mdb
        } else {
            None
        };
        let raw_tap = f
            .meters
            .snapshot
            .as_ref()
            .and_then(|s| s.taps.iter().find(|t| t.id == format!("{input}:raw")))
            .filter(|t| t.valid && f.meters.fresh(std::time::Instant::now()));
        for (x, label, value, reduction) in [
            (83, "PEAK", raw_tap.and_then(|t| t.peak_millidbfs), false),
            (103, "RMS", raw_tap.and_then(|t| t.rms_millidbfs), false),
            (123, "GR", gr.map(|v| v as i32), true),
        ] {
            channel_meter(
                b,
                x,
                label,
                value,
                reduction,
                !reduction && raw_tap.is_some_and(|t| t.silent),
            );
        }
        text(b, 84, 22, 32, "dBFS · raw input", DIM);
        text(b, 124, 22, 12, "dB reduction", DIM);
        text(
            b,
            4,
            22,
            74,
            &spectrum.map_or_else(
                || {
                    format!(
                        "RTA {} / unavailable",
                        f.analysis_source.as_deref().unwrap_or("--").escape_debug()
                    )
                },
                |s| format!("RTA {} / {} / dBFS -96..0", s.source.escape_debug(), s.tap),
            ),
            DIM,
        );
        let knobs = [
            (
                4,
                0,
                "THRESH / dB",
                format!("{:.1}", cfg.threshold_mdb as f64 / 1000.),
                18,
                (cfg.threshold_mdb as f64 + 60000.) / 60000.,
            ),
            (
                5,
                0,
                "RATIO",
                format!("{:.1}:1", cfg.ratio_milli as f64 / 1000.),
                19,
                (cfg.ratio_milli as f64 - 1000.) / 19000.,
            ),
            (
                6,
                0,
                "KNEE / dB",
                format!("{:.1}", cfg.knee_mdb as f64 / 1000.),
                20,
                cfg.knee_mdb as f64 / 18000.,
            ),
            (
                4,
                1,
                "ATTACK / ms",
                format!("{:.1}", cfg.attack_us as f64 / 1000.),
                21,
                cfg.attack_us as f64 / 200000.,
            ),
            (
                5,
                1,
                "RELEASE / ms",
                cfg.release_ms.to_string(),
                22,
                cfg.release_ms as f64 / 2000.,
            ),
            (
                6,
                1,
                "MAKEUP / dB",
                format!("{:+.1}", cfg.makeup_mdb as f64 / 1000.),
                23,
                (cfg.makeup_mdb as f64 + 12000.) / 24000.,
            ),
        ];
        for (col, row, label, value, index, n) in knobs {
            pot(
                b,
                &mut hits,
                (col, row),
                label,
                (&value, n),
                (Target::Field(index), selected(index)),
                m,
            );
        }
        text(
            b,
            83,
            40,
            53,
            &format!(
                "FADER OWNER  {}",
                fader.and_then(|p| p.owner.as_deref()).unwrap_or("unowned")
            ),
            DIM,
        );
        text(
            b,
            83,
            42,
            53,
            &format!(
                "MODE  {}",
                raw.authority
                    .modes
                    .iter()
                    .find(|(s, _)| s == "foh")
                    .map_or("—", |(_, m)| m.as_str())
            ),
            DIM,
        );
    }
    text(b, 143, 1, 14, "OUT dBFS", DIM);
    // Actual strip result after EQ/compression, before channel mute/fader/pan.
    // Never infer output by subtracting instantaneous GR from raw peak.
    let tap = f
        .meters
        .snapshot
        .as_ref()
        .and_then(|s| s.taps.iter().find(|t| t.id == format!("{input}:processed")))
        .filter(|t| t.valid && f.meters.fresh(std::time::Instant::now()));
    channel_meter(
        b,
        143,
        "PEAK",
        tap.and_then(|t| t.peak_millidbfs),
        false,
        tap.is_some_and(|t| t.silent),
    );
    text(b, 143, 22, 14, "PRE-FADER", DIM);
    let pv = pan.and_then(|p| p.target_value.as_i64());
    pot(
        b,
        &mut hits,
        (7, 0),
        "PAN",
        (
            &pv.map_or("—".into(), |v| {
                if v == 0 {
                    "CENTER".into()
                } else {
                    format!("{} {}", if v < 0 { "L" } else { "R" }, v.abs())
                }
            }),
            pv.map_or(0.5, |v| (v as f64 + 100.) / 200.),
        ),
        (Target::Key("C"), false),
        m,
    );
    let db = fader
        .and_then(|p| p.target_value.as_i64())
        .map(|v| v as f64 / 1000.);
    pot(
        b,
        &mut hits,
        (7, 1),
        "LEVEL / dB",
        (
            &db.map_or("—".into(), |v| format!("{v:+.1}")),
            db.map_or(0., |v| (v + 60.) / 72.),
        ),
        (Target::Key("D"), false),
        m,
    );
    text(b, 143, 40, 14, if muted { "MUTED" } else { "UNMUTED" }, DIM);
    indicator(b, 154, 40, "red", f.fresh() && muted, m);
    text(b, 143, 42, 14, if held { "HELD" } else { "HOLD OFF" }, DIM);
    indicator(b, 154, 42, "yellow", f.fresh() && held, m);
    // Numeric entry stays visible in its existing well, not a duplicate ENTRY row.
    if f.processing_draft.is_some() && !f.processing_entry.is_empty() {
        for hit in &hits {
            if matches!(hit.target,Target::Field(i) if i==f.processing_field)
                && hit.area.height == 1
            {
                fill(b, hit.area, Color::Rgb(32, 69, 81));
                text(
                    b,
                    hit.area.x + 1,
                    hit.area.y,
                    hit.area.width.saturating_sub(2),
                    &f.processing_entry,
                    CYAN,
                );
            }
        }
    }
    let cmds: Vec<(&str, Target, &str, bool)> = if ui.secondary {
        vec![
            ("MIX", Target::Key("F1"), "blue", false),
            ("CHANNEL", Target::Menu(Menu::Channel), "blue", true),
            ("SENDS", Target::Key("F12"), "blue", false),
            ("HEALTH", Target::Key("F6"), "blue", false),
            (
                "WRITER GRANT",
                Target::Key("G"),
                "green",
                f.fresh() && u.writer_granted(),
            ),
            (
                "RELEASE HOLD",
                Target::Key("R"),
                "yellow",
                f.fresh() && held,
            ),
            (
                "CANCEL",
                Target::Key("Esc"),
                "yellow",
                f.processing_draft.is_some(),
            ),
            ("RECONNECT", Target::Key("F5"), "red", !f.fresh()),
        ]
    } else {
        match ui.menu {
            Menu::Channel => (0..4)
                .map(|i| {
                    (
                        ["BAND 1", "BAND 2", "BAND 3", "BAND 4"][i],
                        Target::Menu(Menu::Band(i)),
                        "green",
                        channel.is_some_and(|c| {
                            f.processing_fresh() && !c.current.eq_bypass && !c.current.bands()[i].3
                        }),
                    )
                })
                .chain([
                    (
                        "COMPRESSOR",
                        Target::Menu(Menu::Compressor),
                        "green",
                        channel
                            .is_some_and(|c| f.processing_fresh() && !c.current.compressor_bypass),
                    ),
                    (
                        "REVIEW / APPLY",
                        Target::Key("F4"),
                        "yellow",
                        f.processing_draft.is_some(),
                    ),
                    (
                        if held { "HELD" } else { "HOLD" },
                        Target::Key("H"),
                        "yellow",
                        f.fresh() && held,
                    ),
                    (
                        "OUTPUT",
                        Target::Menu(Menu::Output),
                        if muted { "red" } else { "blue" },
                        f.fresh() && muted,
                    ),
                ])
                .collect(),
            Menu::Band(i) => {
                let base = 1 + i.min(3) * 4;
                vec![
                    (
                        "FREQUENCY",
                        Target::Field(base),
                        "blue",
                        f.processing_field == base && f.processing_draft.is_some(),
                    ),
                    (
                        "GAIN",
                        Target::Field(base + 1),
                        "blue",
                        f.processing_field == base + 1 && f.processing_draft.is_some(),
                    ),
                    (
                        "Q",
                        Target::Field(base + 2),
                        "blue",
                        f.processing_field == base + 2 && f.processing_draft.is_some(),
                    ),
                    (
                        "BAND BYPASS",
                        Target::Field(base + 3),
                        "blue",
                        f.processing_field == base + 3 && f.processing_draft.is_some(),
                    ),
                    (
                        "EQ BYPASS",
                        Target::Field(0),
                        "blue",
                        f.processing_field == 0 && f.processing_draft.is_some(),
                    ),
                    (
                        "REVIEW / APPLY",
                        Target::Key("F4"),
                        "yellow",
                        f.processing_draft.is_some(),
                    ),
                    ("CANCEL", Target::Key("Esc"), "yellow", false),
                    ("BACK", Target::Menu(Menu::Channel), "blue", false),
                ]
            }
            Menu::Compressor => [
                "THRESHOLD",
                "RATIO",
                "KNEE",
                "ATTACK",
                "RELEASE",
                "MAKEUP",
                "BYPASS",
            ]
            .into_iter()
            .zip([18, 19, 20, 21, 22, 23, 17])
            .map(|(s, i)| {
                (
                    s,
                    Target::Field(i),
                    "blue",
                    f.processing_draft.is_some() && f.processing_field == i,
                )
            })
            .chain([("BACK", Target::Menu(Menu::Channel), "blue", false)])
            .collect(),
            Menu::Output => vec![
                ("PAN", Target::Key("C"), "blue", false),
                ("LEVEL", Target::Key("D"), "blue", false),
                (
                    if muted { "UNMUTE" } else { "MUTE" },
                    Target::Key("M"),
                    "red",
                    f.fresh() && muted,
                ),
                ("HOLD", Target::Key("H"), "yellow", f.fresh() && held),
                (
                    "RELEASE HOLD",
                    Target::Key("R"),
                    "yellow",
                    f.fresh() && held,
                ),
                ("SENDS", Target::Key("F12"), "blue", false),
                ("CANCEL", Target::Key("Esc"), "yellow", false),
                ("BACK", Target::Menu(Menu::Channel), "blue", false),
            ],
        }
    };
    for (i, (label, target, colour, on)) in cmds.into_iter().enumerate() {
        pad(
            b,
            &mut hits,
            (i as u16, i as u16 + if ui.secondary { 9 } else { 1 }),
            label,
            target,
            (colour, on),
            m,
        );
    }
    hits
}
