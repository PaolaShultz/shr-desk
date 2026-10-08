//! Operator editing of the PA owner's existing program-input EQ. No DSP.
use serde::{
    Deserialize,
    de::{self, MapAccess, SeqAccess, Visitor},
};
use serde_json::Value;

// The transport's integer-only parser cannot parse the opaque PA document.
// Retain finite floats here, but never erase duplicate owner members in Value.
struct OwnerJson(Value);
impl<'de> Deserialize<'de> for OwnerJson {
    fn deserialize<D: serde::Deserializer<'de>>(d: D) -> Result<Self, D::Error> {
        struct V;
        impl<'de> Visitor<'de> for V {
            type Value = OwnerJson;
            fn expecting(&self, f: &mut std::fmt::Formatter) -> std::fmt::Result {
                f.write_str("PA owner JSON without duplicate members")
            }
            fn visit_bool<E: de::Error>(self, v: bool) -> Result<OwnerJson, E> {
                Ok(OwnerJson(v.into()))
            }
            fn visit_i64<E: de::Error>(self, v: i64) -> Result<OwnerJson, E> {
                Ok(OwnerJson(v.into()))
            }
            fn visit_u64<E: de::Error>(self, v: u64) -> Result<OwnerJson, E> {
                Ok(OwnerJson(v.into()))
            }
            fn visit_f64<E: de::Error>(self, v: f64) -> Result<OwnerJson, E> {
                serde_json::Number::from_f64(v)
                    .map(|n| OwnerJson(n.into()))
                    .ok_or_else(|| E::custom("nonfinite PA value"))
            }
            fn visit_str<E: de::Error>(self, v: &str) -> Result<OwnerJson, E> {
                Ok(OwnerJson(v.into()))
            }
            fn visit_unit<E: de::Error>(self) -> Result<OwnerJson, E> {
                Ok(OwnerJson(Value::Null))
            }
            fn visit_seq<A: SeqAccess<'de>>(self, mut a: A) -> Result<OwnerJson, A::Error> {
                let mut values = Vec::new();
                while let Some(v) = a.next_element::<OwnerJson>()? {
                    values.push(v.0);
                }
                Ok(OwnerJson(Value::Array(values)))
            }
            fn visit_map<A: MapAccess<'de>>(self, mut a: A) -> Result<OwnerJson, A::Error> {
                let mut values = serde_json::Map::new();
                while let Some(k) = a.next_key::<String>()? {
                    if values.contains_key(&k) {
                        return Err(de::Error::custom("duplicate PA member"));
                    }
                    values.insert(k, a.next_value::<OwnerJson>()?.0);
                }
                Ok(OwnerJson(Value::Object(values)))
            }
        }
        d.deserialize_any(V)
    }
}
pub(crate) fn parse_owner(text: &str) -> Result<Value, String> {
    if text.is_empty() || text.len() > 48 * 1024 {
        return Err("PA owner document resource bound".into());
    }
    serde_json::from_str::<OwnerJson>(text)
        .map(|v| v.0)
        .map_err(|e| e.to_string())
}

pub const GEQ_HZ: [f64; 31] = [
    20., 25., 31.5, 40., 50., 63., 80., 100., 125., 160., 200., 250., 315., 400., 500., 630., 800.,
    1000., 1250., 1600., 2000., 2500., 3150., 4000., 5000., 6300., 8000., 10000., 12500., 16000.,
    20000.,
];

#[derive(Clone, Debug)]
pub struct View {
    inputs: [usize; 2],
    /// 0 linked edits, 1 left only, 2 right only. Linking never copies old values.
    pub channel: usize,
    pub graphic: bool,
    max_hz: f64,
    baseline: Value,
}
impl View {
    pub fn new(document: &Value) -> Result<Self, String> {
        crate::provider::keys(document, &["configuration", "program_buses"])?;
        let config = &document["configuration"];
        crate::provider::keys(
            config,
            &[
                "version",
                "sample_rate",
                "max_block",
                "inputs",
                "nodes",
                "outputs",
            ],
        )?;
        if config["version"].as_u64() != Some(2) {
            return Err("Master EQ requires PA owner graph version 2".into());
        }
        let inputs = config["inputs"].as_array().ok_or("PA inputs unavailable")?;
        let buses = document["program_buses"]
            .as_array()
            .ok_or("PA bus map unavailable")?;
        if inputs.len() != buses.len() {
            return Err("PA input/bus map mismatch".into());
        }
        if buses.iter().any(|v| v.as_u64().is_none()) {
            return Err("PA program bus must be an integer identity".into());
        }
        let mut mapped = [0; 2];
        for (bus, index) in mapped.iter_mut().enumerate() {
            let matches: Vec<_> = buses
                .iter()
                .enumerate()
                .filter(|(_, v)| v.as_u64() == Some(bus as u64))
                .map(|(i, _)| i)
                .collect();
            if matches.len() != 1 {
                return Err("Master EQ needs exactly one PA input for each main L/R bus".into());
            }
            *index = matches[0];
        }
        let rate = config["sample_rate"]
            .as_u64()
            .ok_or("PA sample rate unavailable")?;
        if !(8000..=192000).contains(&rate) {
            return Err("PA sample rate unsupported".into());
        }
        let mut view = Self {
            inputs: mapped,
            channel: 0,
            graphic: false,
            max_hz: (rate as f64 * 0.45).min(20000.),
            baseline: document.clone(),
        };
        for graphic in [false, true] {
            view.graphic = graphic;
            for side in 1..=2 {
                view.channel = side;
                let input = &inputs[mapped[side - 1]];
                crate::provider::keys(
                    input,
                    &[
                        "gain_db",
                        "delay_ms",
                        "eq_enabled",
                        "eq",
                        "geq_enabled",
                        "geq_db",
                        "compressor",
                    ],
                )?;
                if input["eq"].as_array().is_none_or(|v| v.len() != 8)
                    || input["geq_db"].as_array().is_none_or(|v| v.len() != 31)
                {
                    return Err("Master EQ requires advertised 8 PEQ and 31 GEQ bands".into());
                }
                for band in input["eq"].as_array().unwrap() {
                    crate::provider::keys(band, &["kind", "hz", "db", "q", "slope"])?;
                }
                for path in view.fields() {
                    view.validate(&path, document.pointer(&path).ok_or("PA EQ field missing")?)?;
                }
            }
        }
        view.channel = 0;
        view.graphic = false;
        Ok(view)
    }
    pub(crate) fn matches_readback(&self, config: &str, buses: &[usize]) -> bool {
        parse_owner(config).is_ok_and(|c| {
            self.baseline == serde_json::json!({"configuration":c,"program_buses":buses})
        })
    }
    /// Validate all editable fields, and prove that routing/protection and every
    /// other owner setting still equal the original readback.
    pub(crate) fn validate_document(&self, document: &Value) -> Result<(), String> {
        let current = Self::new(document)?;
        if current.inputs != self.inputs {
            return Err("PA EQ identity changed".into());
        }
        let mut preserved = document.clone();
        for input in self.inputs {
            for field in ["eq_enabled", "eq", "geq_enabled", "geq_db"] {
                preserved["configuration"]["inputs"][input][field] =
                    self.baseline["configuration"]["inputs"][input][field].clone();
            }
        }
        if preserved != self.baseline {
            return Err("Master EQ cannot replace other owner settings".into());
        }
        Ok(())
    }
    pub fn channel_label(&self) -> &'static str {
        match self.channel {
            0 => "L+R linked edits",
            1 => "Left only",
            _ => "Right only",
        }
    }
    fn selected_input(&self) -> usize {
        self.inputs[usize::from(self.channel == 2)]
    }
    pub fn fields(&self) -> Vec<String> {
        let prefix = format!("/configuration/inputs/{}", self.selected_input());
        let mut fields = vec![format!(
            "{prefix}/{}_enabled",
            if self.graphic { "geq" } else { "eq" }
        )];
        if self.graphic {
            fields.extend((0..31).map(|i| format!("{prefix}/geq_db/{i}")));
        } else {
            for band in 0..8 {
                for field in ["kind", "hz", "db", "q", "slope"] {
                    fields.push(format!("{prefix}/eq/{band}/{field}"));
                }
            }
        }
        fields
    }
    pub fn label(&self, path: &str) -> String {
        let parts: Vec<_> = path.split('/').collect();
        match parts.get(4).copied() {
            Some("eq_enabled") => "Parametric EQ enabled".into(),
            Some("geq_enabled") => "Graphic EQ enabled".into(),
            Some("geq_db") => {
                let n = parts[5].parse::<usize>().unwrap();
                format!("GEQ {:>7} Hz / gain dB", GEQ_HZ[n])
            }
            Some("eq") => {
                let band = parts[5].parse::<usize>().unwrap() + 1;
                let name = match parts[6] {
                    "kind" => "type (bell / low_shelf / high_shelf)",
                    "hz" => "frequency Hz",
                    "db" => "gain dB",
                    "q" => "Q (bell)",
                    _ => "shelf slope",
                };
                format!("Band {band} / {name}")
            }
            _ => "Unknown EQ field".into(),
        }
    }
    fn peer_path(&self, path: &str) -> String {
        path.replacen(
            &format!("/inputs/{}/", self.inputs[0]),
            &format!("/inputs/{}/", self.inputs[1]),
            1,
        )
    }
    pub fn display(&self, document: &Value, path: &str) -> String {
        let left = document.pointer(path).unwrap_or(&Value::Null);
        if self.channel == 0 {
            let peer = self.peer_path(path);
            let right = document.pointer(&peer).unwrap_or(&Value::Null);
            if left != right {
                return format!("L {left} / R {right}");
            }
        }
        left.to_string()
    }
    fn validate(&self, path: &str, value: &Value) -> Result<(), String> {
        if !self.fields().iter().any(|p| p == path) {
            return Err("Not an editable master EQ field".into());
        }
        let valid = if path.ends_with("_enabled") {
            value.is_boolean()
        } else if path.ends_with("/kind") {
            matches!(value.as_str(), Some("bell" | "low_shelf" | "high_shelf"))
        } else {
            let (min, max) = if path.ends_with("/hz") {
                (20., self.max_hz)
            } else if path.ends_with("/q") {
                (0.1, 15.909)
            } else if path.ends_with("/slope") {
                (0.1, 1.)
            } else {
                (-12., 12.)
            };
            value
                .as_f64()
                .is_some_and(|v| v.is_finite() && (min..=max).contains(&v))
        };
        if valid {
            Ok(())
        } else {
            Err(format!("Invalid {} value", self.label(path)))
        }
    }
    pub fn set(&self, document: &mut Value, path: &str, value: Value) -> Result<(), String> {
        self.validate(path, &value)?;
        let mut paths = vec![path.to_owned()];
        if self.channel == 0 {
            paths.push(self.peer_path(path));
        }
        if paths.iter().any(|p| document.pointer(p).is_none()) {
            return Err("PA EQ identity changed".into());
        }
        let mut candidate = document.clone();
        for p in paths {
            *candidate.pointer_mut(&p).unwrap() = value.clone();
        }
        self.validate_document(&candidate)?;
        *document = candidate;
        Ok(())
    }
    pub fn adjust(&self, document: &mut Value, path: &str, delta: i32) -> Result<(), String> {
        let old = document.pointer(path).ok_or("PA EQ field missing")?;
        let value = if let Some(v) = old.as_bool() {
            Value::Bool(!v)
        } else if path.ends_with("/kind") {
            let kinds = ["bell", "low_shelf", "high_shelf"];
            let i = kinds
                .iter()
                .position(|v| Some(*v) == old.as_str())
                .ok_or("Unknown EQ type")?;
            Value::from(kinds[(i as i64 + i64::from(delta)).rem_euclid(3) as usize])
        } else {
            let step = if path.ends_with("/hz") {
                10.
            } else if path.ends_with("/q") || path.ends_with("/slope") {
                0.1
            } else {
                0.5
            };
            let n = old.as_f64().ok_or("Numeric EQ field required")? + f64::from(delta) * step;
            serde_json::Number::from_f64((n * 1000.).round() / 1000.)
                .ok_or("Nonfinite EQ value")?
                .into()
        };
        self.set(document, path, value)
    }
}

impl View {
    /// Same native scene and bundled-font renderer as the connected frontend.
    /// Both curves are static settings calculations; neither is a spectrum.
    pub fn scene(
        &self,
        document: &Value,
        selected: usize,
        entry: &str,
        message: &str,
    ) -> crate::render::Scene {
        use crate::render::{Primitive, Scene};
        let mut s = Scene::default();
        s.primitives.push(Primitive::Rect {
            x: 0,
            y: 0,
            w: 1920,
            h: 1080,
            fill: "#10151d",
        });
        fn text(s: &mut Scene, x: u32, y: u32, v: impl Into<String>, color: &'static str) {
            let max = (1920 - x) / 12;
            s.primitives.push(Primitive::Text {
                x,
                y,
                value: v.into().chars().take(max as usize).collect(),
                color,
            });
        }
        fn line(s: &mut Scene, x1: u32, y1: u32, x2: u32, y2: u32, color: &'static str) {
            s.primitives.push(Primitive::Line {
                x1,
                y1,
                x2,
                y2,
                color,
            });
        }
        let cyan = "#66dfd3";
        let dim = "#9caebc";
        let white = "#e4e8e9";
        let amber = "#f1bd6b";
        text(
            &mut s,
            24,
            12,
            format!("MASTER EQ / {} / LOCAL DRAFT", self.channel_label()),
            cyan,
        );
        text(
            &mut s,
            24,
            60,
            "MUTED SETUP / current readback + unsent draft / full review; separate rearm",
            amber,
        );
        text(
            &mut s,
            24,
            108,
            "Main L/R -> PA program EQ -> graph/protection. Direct main/monitor routes bypass this EQ.",
            dim,
        );
        let fields = self.fields();
        let selected = selected.min(fields.len() - 1);
        let path = &fields[selected];
        let band = if selected == 0 {
            None
        } else {
            Some(if self.graphic {
                selected - 1
            } else {
                (selected - 1) / 5
            })
        };
        let prefix = format!("/configuration/inputs/{}", self.selected_input());
        let enabled = format!(
            "{prefix}/{}_enabled",
            if self.graphic { "geq" } else { "eq" }
        );
        text(
            &mut s,
            24,
            168,
            format!(
                "{} ENABLE {}{}",
                if self.graphic {
                    "GRAPHIC"
                } else {
                    "PARAMETRIC"
                },
                self.display(document, &enabled),
                if selected == 0 { " < selected" } else { "" }
            ),
            cyan,
        );
        if !self.graphic {
            for n in 0..8 {
                let y = 216 + n as u32 * 64;
                let color = if band == Some(n) { cyan } else { white };
                text(
                    &mut s,
                    24,
                    y,
                    format!(
                        "{} B{} {:<12} {:>8} Hz / {:>6} dB",
                        if band == Some(n) { ">" } else { " " },
                        n + 1,
                        self.display(document, &format!("{prefix}/eq/{n}/kind")),
                        self.display(document, &format!("{prefix}/eq/{n}/hz")),
                        self.display(document, &format!("{prefix}/eq/{n}/db"))
                    ),
                    color,
                );
                text(
                    &mut s,
                    72,
                    y + 24,
                    format!(
                        "Q {} / shelf slope {}",
                        self.display(document, &format!("{prefix}/eq/{n}/q")),
                        self.display(document, &format!("{prefix}/eq/{n}/slope"))
                    ),
                    dim,
                );
            }
        } else {
            // All31 centres and both stored L/R gains remain visible, regardless
            // of rate or enable. Above-rate bands are explicit identity filters.
            for (n, hz) in GEQ_HZ.iter().enumerate() {
                let x = 24 + n as u32 * 60;
                let gain = document["configuration"]["inputs"][self.selected_input()]["geq_db"][n]
                    .as_f64()
                    .unwrap_or(0.);
                let active = *hz <= self.max_hz
                    && document.pointer(&enabled).and_then(Value::as_bool) == Some(true);
                let color = if band == Some(n) {
                    cyan
                } else if active {
                    white
                } else {
                    dim
                };
                line(&mut s, x + 20, 240, x + 20, 456, "#3c4f63");
                line(&mut s, x + 4, 348, x + 40, 348, dim);
                let y = (348. - gain * 9.).round().clamp(240., 456.) as u32;
                s.primitives.push(Primitive::Rect {
                    x: x + 4,
                    y: y.saturating_sub(3),
                    w: 36,
                    h: 6,
                    fill: color,
                });
                text(
                    &mut s,
                    x,
                    480 + (n as u32 % 2) * 24,
                    if *hz >= 1000. {
                        format!("{}k", hz / 1000.)
                    } else {
                        format!("{hz}")
                    },
                    color,
                );
                text(&mut s, x, 552, format!("{gain:+.1}"), color);
                if !active {
                    text(&mut s, x, 600, "ID", dim);
                }
            }
            text(
                &mut s,
                24,
                648,
                "ID: disabled section or centre above0.45 x rate; stored gain retained, filter response is unity.",
                dim,
            );
        }
        let plot = if self.graphic {
            (24u32, 720u32, 1800u32, 108u32)
        } else {
            (1008u32, 252u32, 840u32, 408u32)
        };
        let (x, y, w, h) = plot;
        text(
            &mut s,
            x,
            y - 48,
            "CALCULATED EQ RESPONSE (Hz) / current grey / draft L cyan / R amber",
            dim,
        );
        let rate = document["configuration"]["sample_rate"]
            .as_u64()
            .unwrap_or(0) as u32;
        let max_hz = (f64::from(rate) * 0.45).min(20000.);
        for db in [-48., -24., 0., 24., 48.] {
            let yy = y + ((48. - db) / 96. * f64::from(h)) as u32;
            line(&mut s, x, yy, x + w, yy, "#3c4f63");
            text(
                &mut s,
                x,
                yy.saturating_sub(24),
                format!("{db:+.0} dB"),
                dim,
            );
        }
        for (tick, origin, label) in crate::eq_response::frequency_axis(x, w, max_hz) {
            line(&mut s, tick, y, tick, y + h, "#3c4f63");
            text(&mut s, origin, y + h + 24, label, dim);
        }
        for (settings, color) in [(&self.baseline, dim), (document, cyan)] {
            for side in 0..2 {
                let color = if std::ptr::eq(settings, document) && side == 1 {
                    amber
                } else {
                    color
                };
                let input = &settings["configuration"]["inputs"][self.inputs[side]];
                if let Ok(bank) = crate::eq_response::coefficients(input, rate) {
                    let mut previous = None;
                    for n in 0..=240 {
                        let hz = 20. * (max_hz / 20.).powf(f64::from(n) / 240.);
                        if let Ok((db, _)) = crate::eq_response::response(&bank, rate, hz) {
                            let p = (
                                x + n as u32 * w / 240,
                                y + ((48. - db.clamp(-48., 48.)) / 96. * f64::from(h)) as u32,
                            );
                            if let Some((px, py)) = previous {
                                line(&mut s, px, py, p.0, p.1, color);
                            }
                            previous = Some(p);
                        }
                    }
                } else {
                    text(
                        &mut s,
                        x,
                        y,
                        "Response unavailable: invalid owner settings",
                        amber,
                    );
                }
            }
        }
        let detail_y = if self.graphic { 888 } else { 756 };
        text(
            &mut s,
            24,
            detail_y,
            format!("Selected: {}", self.label(path)),
            cyan,
        );
        text(
            &mut s,
            24,
            detail_y + 24,
            format!(
                "Current {} | Draft {} | Entry {}",
                self.display(&self.baseline, path),
                self.display(document, path),
                entry
            ),
            white,
        );
        text(
            &mut s,
            24,
            960,
            "B PEQ/GEQ | C linked/left/right | U/I field | J/K adjust | F3 exact entry, Enter accepts",
            dim,
        );
        text(
            &mut s,
            24,
            996,
            "F4 complete Apply/review | Esc cancel | EQ response excludes dynamics, room and protection",
            amber,
        );
        text(&mut s, 24, 1032, message, "#f47c85");
        s
    }
}
impl View {
    pub(crate) fn patch(&self, document: &Value) -> Result<Value, String> {
        self.validate_document(document)?;
        let indices = self.inputs;
        let settings:Vec<_>=indices.into_iter().map(|index| {
            let i=&document["configuration"]["inputs"][index];
            serde_json::json!({"input_index":index,"eq_enabled":i["eq_enabled"],"eq":i["eq"],"geq_enabled":i["geq_enabled"],"geq_db":i["geq_db"]})
        }).collect();
        Ok(serde_json::json!({"version":1,"inputs":settings}))
    }
}
