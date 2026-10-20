//! Full-width RTA with discrete EQ control points and nominal Q-width brackets.
use crate::{
    channel_panel::{Hit, Target},
    frontend::Frontend,
    spectrum::View,
};
use shr_instrument_panel::{
    rack::*,
    terminal::ratatui::{buffer::Buffer, layout::Rect, style::Color},
};

pub fn graph_area(area: Rect) -> Rect {
    Rect::new(
        area.x + 1,
        area.y + 1,
        area.width.saturating_sub(2),
        area.height.saturating_sub(1),
    )
}
/// Nominal Q width: geometric centre f, bandwidth f/Q. This control geometry
/// does not claim digital filter half-gain crossings or a measured response.
pub fn q_width(f: f64, q: f64) -> (f64, f64) {
    let span = (4. + 1. / (q * q)).sqrt();
    (f * (span - 1. / q) / 2., f * (span + 1. / q) / 2.)
}

/// Fixed dBFS colour stops. Quiet levels never normalize to the red end.
pub fn level_colour(db: f64) -> Color {
    let stops = [
        (-96., [22., 58., 34.]),
        (-72., [38., 105., 48.]),
        (-54., [135., 127., 34.]),
        (-36., [170., 82., 27.]),
        (-18., [185., 40., 35.]),
        (0., [210., 48., 38.]),
    ];
    let db = db.clamp(-96., 0.);
    for pair in stops.windows(2) {
        if db <= pair[1].0 {
            let t = (db - pair[0].0) / (pair[1].0 - pair[0].0);
            let c: [u8; 3] = std::array::from_fn(|i| {
                (pair[0].1[i] + t * (pair[1].1[i] - pair[0].1[i])).round() as u8
            });
            return Color::Rgb(c[0], c[1], c[2]);
        }
    }
    Color::Rgb(210, 48, 38)
}
pub fn draw(f: &Frontend, b: &mut Buffer, observation: Option<&View>) -> Vec<Hit> {
    let mut hits = vec![];
    let area = b.area;
    fill(b, area, Color::Rgb(19, 23, 27));
    if area.width < 60 || area.height < 18 {
        text(
            b,
            area.x,
            area.y,
            area.width,
            "Fullscreen EQ / RTA needs 60 x 18 cells",
            TEXT,
        );
        return hits;
    }
    let Some(u) = &f.state else {
        return hits;
    };
    let Some(raw) = &u.snapshot else {
        return hits;
    };
    let Some(input) = raw.authority.inputs.get(f.selected) else {
        return hits;
    };
    let Some(proc) = &u.processing else {
        return hits;
    };
    let Some(channel) = proc.channels.iter().find(|c| &c.input == input) else {
        return hits;
    };
    let cfg = f
        .processing_draft
        .as_ref()
        .map_or(&channel.target, |d| &d.config);
    let source = f.analysis_source.as_deref().unwrap_or("—");
    let choice = f.analysis_sources.iter().find(|s| s.id == source);
    let observation = observation.filter(|s| s.fresh() && s.source == source);
    let tap = match choice.map(|s| s.tap.as_str()) {
        Some("raw-pre-processing") => "raw",
        Some("main-pre-pa") => "pre-PA",
        Some("physical-output-post-patch") => "post-patch",
        _ => "—",
    };
    let band = f.processing_field.saturating_sub(1).div_euclid(4).min(3);
    let (hz, gain, q, _) = cfg.bands()[band];
    // One compact, contextual label. No telemetry panels, legends or help prose.
    let label = if f.processing_draft.is_some() {
        format!(
            "EQ {}* · {}Hz {:+.1}dB Q{:.2}{} · F11 ×",
            input.escape_debug(),
            hz,
            gain as f64 / 1000.,
            q as f64 / 1000.,
            if f.processing_entry.is_empty() {
                String::new()
            } else {
                format!(" ← {}", f.processing_entry)
            }
        )
    } else {
        format!(
            "RTA {}/{tap}{}{} · EQ {} · F11 ×",
            source.escape_debug(),
            if observation.is_none() { " ∅" } else { "" },
            if observation.is_some_and(|s| s.fft_bins.is_none()) {
                " v1"
            } else {
                ""
            },
            input.escape_debug()
        )
    };
    text(b, area.x + 1, area.y, area.width - 2, &label, TEXT);
    hits.push(Hit {
        area: Rect::new(area.x + 1, area.y, area.width - 2, 1),
        target: Target::Key("F11"),
    });
    let graph = graph_area(area);
    panel(b, Rect::new(area.x, graph.y, area.width, graph.height), "");
    // Labels remain outside the full interior width.
    let colours = [
        Color::Rgb(91, 192, 255),
        Color::Rgb(244, 211, 85),
        Color::Rgb(231, 127, 236),
        Color::Rgb(105, 235, 157),
    ];
    fill(b, graph, WELL);
    // Full cell columns make the measured spectrum the primary visual. The colour
    // at each height belongs to the fixed dBFS scale, never the current maximum.
    if let Some(s) = observation {
        let bins = s
            .fft_bins
            .as_ref()
            .map(|v| crate::spectrum::aggregate(v, s.rate, graph.width as usize))
            .unwrap_or_else(|| {
                (0..graph.width as usize)
                    .map(|x| s.bins[x * 64 / graph.width as usize])
                    .collect()
            });
        let peaks = s
            .fft_peaks
            .as_ref()
            .map(|v| crate::spectrum::aggregate(v, s.rate, graph.width as usize))
            .unwrap_or_else(|| {
                (0..graph.width as usize)
                    .map(|x| s.peaks[x * 64 / graph.width as usize])
                    .collect()
            });
        for (x, db) in bins.iter().enumerate() {
            let height =
                ((f64::from(*db) / 1000. + 96.) / 96.).clamp(0., 1.) * f64::from(graph.height);
            for row in 0..graph.height {
                let fraction = (height - f64::from(row)).clamp(0., 1.);
                if fraction <= 0. {
                    break;
                }
                let level = -96. + (f64::from(row) + 0.5) * 96. / f64::from(graph.height);
                b[(graph.x + x as u16, graph.bottom() - 1 - row)]
                    .set_symbol(if fraction >= 0.75 {
                        "█"
                    } else if fraction >= 0.25 {
                        "▄"
                    } else {
                        "▁"
                    })
                    .set_fg(level_colour(level));
            }
            if peaks[x] > *db + 1500 && peaks[x] > -96000 {
                let y = ((f64::from(peaks[x]) / 1000. + 96.) / 96.).clamp(0., 1.)
                    * f64::from(graph.height - 1);
                b[(graph.x + x as u16, graph.bottom() - 1 - y.round() as u16)]
                    .set_symbol("▔")
                    .set_fg(Color::Rgb(110, 130, 118));
            }
        }
    }
    let selected = f.processing_field.saturating_sub(1).div_euclid(4).min(3);
    // Draw the selected band last so coincident points remain selectable/readable.
    for i in (0..4)
        .filter(|i| *i != selected)
        .chain(std::iter::once(selected))
    {
        let (hz, gain, q, bypass) = cfg.bands()[i];
        let (lo, hi) = q_width(hz as f64, q as f64 / 1000.);
        let column = |hz: f64| {
            graph.x
                + (((hz.clamp(20., 20000.) / 20.).log10() / 3.) * f64::from(graph.width - 1))
                    .round() as u16
        };
        let left = column(lo);
        let right = column(hi);
        let centre = column(hz as f64);
        let y = graph.y
            + ((18. - gain as f64 / 1000.).clamp(0., 36.) / 36. * f64::from(graph.height - 1))
                .round() as u16;
        let inactive = cfg.eq_bypass || bypass;
        for x in left..=right {
            b[(x, y)]
                .set_symbol(if x == left {
                    "├"
                } else if x == right {
                    "┤"
                } else if inactive {
                    "┄"
                } else if i == selected {
                    "━"
                } else {
                    "─"
                })
                .set_fg(colours[i])
                .set_bg(WELL);
        }
        b[(centre, y)]
            .set_symbol(if inactive { "○" } else { "●" })
            .set_fg(colours[i])
            .set_bg(WELL);
        hits.push(Hit {
            area: Rect::new(
                left,
                y.saturating_sub(1).max(graph.y),
                right - left + 1,
                2.min(graph.bottom() - y + 1),
            ),
            target: Target::Field(1 + i * 4),
        });
    }
    hits
}

#[cfg(test)]
mod tests {
    #[test]
    fn q_width_preserves_centre_and_bandwidth_and_narrows_with_q() {
        for f in [20., 120., 2000., 20000.] {
            let mut previous = f * 100.;
            for q in [0.1, 0.5, 1., 2.4, 10.] {
                let (lo, hi) = super::q_width(f, q);
                assert!((lo * hi - f * f).abs() < f * f * 1e-10);
                assert!((hi - lo - f / q).abs() < f * 1e-10);
                assert!(hi - lo < previous);
                previous = hi - lo;
            }
        }
    }
}
