//! Calculated PA EQ display response. No sample processing, history or audio engine.
//! Coefficient convention matches the accepted owner's EQ-v1 golden corpus.
use serde_json::Value;
use std::f64::consts::PI;
const IDENTITY: [f64; 5] = [1., 0., 0., 0., 0.];
fn band(kind: &str, hz: f64, db: f64, q: f64, slope: f64, rate: u32) -> [f64; 5] {
    let a = 10_f64.powf(db / 40.);
    let w = 2. * PI * hz / f64::from(rate);
    let c = w.cos();
    let alpha = w.sin() / (2. * q);
    let (b, d) = if kind == "bell" {
        (
            [1. + alpha * a, -2. * c, 1. - alpha * a],
            [1. + alpha / a, -2. * c, 1. - alpha / a],
        )
    } else {
        let alpha = w.sin() / 2. * ((a + 1. / a) * (1. / slope - 1.) + 2.).sqrt();
        let t = 2. * a.sqrt() * alpha;
        if kind == "low_shelf" {
            (
                [
                    a * ((a + 1.) - (a - 1.) * c + t),
                    2. * a * ((a - 1.) - (a + 1.) * c),
                    a * ((a + 1.) - (a - 1.) * c - t),
                ],
                [
                    (a + 1.) + (a - 1.) * c + t,
                    -2. * ((a - 1.) + (a + 1.) * c),
                    (a + 1.) + (a - 1.) * c - t,
                ],
            )
        } else {
            (
                [
                    a * ((a + 1.) + (a - 1.) * c + t),
                    -2. * a * ((a - 1.) + (a + 1.) * c),
                    a * ((a + 1.) + (a - 1.) * c - t),
                ],
                [
                    (a + 1.) - (a - 1.) * c + t,
                    2. * ((a - 1.) - (a + 1.) * c),
                    (a + 1.) - (a - 1.) * c - t,
                ],
            )
        }
    };
    [
        b[0] / d[0],
        b[1] / d[0],
        b[2] / d[0],
        d[1] / d[0],
        d[2] / d[0],
    ]
}
/// Settings-only calculation; rejects incomplete/invalid EQ instead of drawing unity.
/// The input may be a validated owner graph input or an EQ-only owner setting.
pub fn coefficients(input: &Value, rate: u32) -> Result<Vec<[f64; 5]>, String> {
    if !(8000..=192000).contains(&rate) {
        return Err("EQ display sample rate".into());
    }
    let enabled = input["eq_enabled"].as_bool().ok_or("EQ enable missing")?;
    let graphic = input["geq_enabled"].as_bool().ok_or("GEQ enable missing")?;
    let eq = input["eq"]
        .as_array()
        .filter(|v| v.len() == 8)
        .ok_or("8 EQ bands required")?;
    let geq = input["geq_db"]
        .as_array()
        .filter(|v| v.len() == 31)
        .ok_or("31 GEQ bands required")?;
    let mut bank = Vec::with_capacity(39);
    let number = |v: &Value, min: f64, max: f64| {
        v.as_f64()
            .filter(|n| n.is_finite() && (min..=max).contains(n))
            .ok_or("EQ display range")
    };
    for e in eq {
        crate::provider::keys(e, &["kind", "hz", "db", "q", "slope"])?;
        let kind = e["kind"]
            .as_str()
            .filter(|k| matches!(*k, "bell" | "low_shelf" | "high_shelf"))
            .ok_or("EQ display type")?;
        let hz = number(&e["hz"], 20., (0.45 * f64::from(rate)).min(20000.))?;
        let db = number(&e["db"], -12., 12.)?;
        let q = number(&e["q"], 0.1, 15.909)?;
        let slope = number(&e["slope"], 0.1, 1.)?;
        bank.push(if enabled {
            band(kind, hz, db, q, slope, rate)
        } else {
            IDENTITY
        });
    }
    for (n, hz) in crate::master_eq::GEQ_HZ.iter().enumerate() {
        let db = number(&geq[n], -12., 12.)?;
        bank.push(if graphic && *hz <= 0.45 * f64::from(rate) {
            band("bell", *hz, db, 4.318, 1., rate)
        } else {
            IDENTITY
        });
    }
    if bank.iter().any(|c| {
        c.iter().any(|n| !n.is_finite())
            || c[4].abs() >= 1.
            || 1. + c[3] + c[4] <= 0.
            || 1. - c[3] + c[4] <= 0.
    }) {
        return Err("EQ display unstable coefficients".into());
    }
    Ok(bank)
}
/// Static magnitude and phase of one settings bank. Never a running crossfade,
/// measured spectrum, compressor, crossover, protection or room response.
pub fn response(bank: &[[f64; 5]], rate: u32, hz: f64) -> Result<(f64, f64), String> {
    if rate == 0 || !hz.is_finite() || hz < 0. || hz >= f64::from(rate) / 2. {
        return Err("EQ display frequency".into());
    }
    let w = 2. * PI * hz / f64::from(rate);
    let mut magnitude = 0.;
    let mut phase = 0.;
    for c in bank {
        let nr = c[0] + c[1] * w.cos() + c[2] * (2. * w).cos();
        let ni = -c[1] * w.sin() - c[2] * (2. * w).sin();
        let dr = 1. + c[3] * w.cos() + c[4] * (2. * w).cos();
        let di = -c[3] * w.sin() - c[4] * (2. * w).sin();
        magnitude += 10. * ((nr * nr + ni * ni) / (dr * dr + di * di)).log10();
        phase += ni.atan2(nr) - di.atan2(dr);
    }
    if !magnitude.is_finite() || !phase.is_finite() {
        return Err("EQ display nonfinite response".into());
    }
    Ok((magnitude, (phase + PI).rem_euclid(2. * PI) - PI))
}

/// Log-frequency tick and text origins for the bitmap-font response axes.
/// Callers supply validated rate bounds and charts wider than one label.
pub(crate) fn frequency_axis(x: u32, width: u32, max_hz: f64) -> Vec<(u32, u32, &'static str)> {
    [
        (20_f64, "20"),
        (100., "100"),
        (1000., "1k"),
        (10000., "10k"),
        (20000., "20k"),
    ]
    .into_iter()
    .filter(|(hz, _)| *hz <= max_hz)
    .map(|(hz, label)| {
        let tick = x + ((hz / 20.).ln() / (max_hz / 20.).ln() * f64::from(width)) as u32;
        let text_width = label.len() as u32 * 12;
        let origin = tick
            .saturating_sub(text_width / 2)
            .clamp(x, x + width - text_width);
        (tick, origin, label)
    })
    .collect()
}
