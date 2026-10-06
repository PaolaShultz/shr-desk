//! Read-only C-AUDIO:1 GP02 client. Never constructs mutations or applies DSP.
use serde::{
    Deserialize, Serialize,
    de::{self, MapAccess, SeqAccess, Visitor},
};
use serde_json::Value;
use std::{
    collections::{BTreeMap, BTreeSet},
    fmt,
};
pub const MAX_BYTES: usize = 65536;
/// Negotiated GP14 immutable document budget; individual frames stay64KiB.
pub const MAX_DOCUMENT_BYTES: usize = 1024 * 1024;
pub const ASSEMBLY_MS: u64 = 2000;
pub const FRESH_MS: u64 = 250;
fn required_option<'de, D: serde::Deserializer<'de>, T: Deserialize<'de>>(
    d: D,
) -> Result<Option<T>, D::Error> {
    Option::deserialize(d)
}
#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq, PartialOrd, Ord)]
#[serde(deny_unknown_fields)]
pub struct Target {
    pub parameter: String,
    pub input: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub monitor: Option<String>,
}
#[derive(Clone, Debug, Deserialize, Serialize, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct Parameter {
    pub target: Target,
    #[serde(deserialize_with = "required_option")]
    pub actual: Option<Value>,
    pub target_value: Value,
    #[serde(deserialize_with = "required_option")]
    pub proposal: Option<Value>,
    #[serde(deserialize_with = "required_option")]
    pub hold: Option<Value>,
    #[serde(deserialize_with = "required_option")]
    pub owner: Option<String>,
}
#[derive(Clone, Debug, Deserialize, Serialize, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct AutoBound {
    pub target: Target,
    pub min: i64,
    pub max: i64,
}
#[derive(Clone, Debug, Deserialize, Serialize, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct Snapshot {
    pub show_id: String,
    pub epoch: String,
    pub revision: String,
    pub sequence: String,
    pub page: usize,
    pub page_count: usize,
    pub durability: String,
    pub validity: String,
    #[serde(deserialize_with = "required_option")]
    pub acquisition_frame: Option<String>,
    #[serde(deserialize_with = "required_option")]
    pub age_ms: Option<u64>,
    pub rendered_application: bool,
    pub release_commit: bool,
    pub session_history_capacity: usize,
    pub inputs: Vec<String>,
    pub monitors: Vec<String>,
    #[serde(with = "crate::scopes::modes")]
    pub modes: Vec<(String, String)>,
    pub automation_bounds: Vec<AutoBound>,
    pub parameters: Vec<Parameter>,
}
// A recursive visitor catches duplicates before Value could erase them. It also
// rejects floats; provider quantities are integer or boolean, never measured NaN.
struct Strict(Value);
impl<'de> Deserialize<'de> for Strict {
    fn deserialize<D: serde::Deserializer<'de>>(d: D) -> Result<Self, D::Error> {
        struct V;
        impl<'de> Visitor<'de> for V {
            type Value = Strict;
            fn expecting(&self, f: &mut fmt::Formatter) -> fmt::Result {
                f.write_str("bounded integer JSON")
            }
            fn visit_bool<E: de::Error>(self, v: bool) -> Result<Strict, E> {
                Ok(Strict(Value::Bool(v)))
            }
            fn visit_i64<E: de::Error>(self, v: i64) -> Result<Strict, E> {
                Ok(Strict(v.into()))
            }
            fn visit_u64<E: de::Error>(self, v: u64) -> Result<Strict, E> {
                Ok(Strict(v.into()))
            }
            fn visit_str<E: de::Error>(self, v: &str) -> Result<Strict, E> {
                Ok(Strict(v.into()))
            }
            fn visit_string<E: de::Error>(self, v: String) -> Result<Strict, E> {
                Ok(Strict(v.into()))
            }
            fn visit_unit<E: de::Error>(self) -> Result<Strict, E> {
                Ok(Strict(Value::Null))
            }
            fn visit_none<E: de::Error>(self) -> Result<Strict, E> {
                Ok(Strict(Value::Null))
            }
            fn visit_seq<A: SeqAccess<'de>>(self, mut a: A) -> Result<Strict, A::Error> {
                let mut v = Vec::new();
                while let Some(x) = a.next_element::<Strict>()? {
                    v.push(x.0);
                }
                Ok(Strict(Value::Array(v)))
            }
            fn visit_map<A: MapAccess<'de>>(self, mut a: A) -> Result<Strict, A::Error> {
                let mut v = serde_json::Map::new();
                while let Some(k) = a.next_key::<String>()? {
                    if v.contains_key(&k) {
                        return Err(de::Error::custom("duplicate field"));
                    }
                    v.insert(k, a.next_value::<Strict>()?.0);
                }
                Ok(Strict(Value::Object(v)))
            }
        }
        d.deserialize_any(V)
    }
}
pub(crate) fn parse(bytes: &[u8]) -> Result<Value, String> {
    parse_limit(bytes, MAX_BYTES)
}
pub(crate) fn parse_document(bytes: &[u8]) -> Result<Value, String> {
    parse_limit(bytes, MAX_DOCUMENT_BYTES)
}
fn parse_limit(bytes: &[u8], limit: usize) -> Result<Value, String> {
    if bytes.len() > limit {
        return Err(format!("message exceeds {limit} byte admission"));
    }
    let text = std::str::from_utf8(bytes).map_err(|_| "invalid UTF8")?;
    let (mut depth, mut string, mut escape) = (0usize, false, false);
    for b in text.bytes() {
        if string {
            if escape {
                escape = false;
            } else if b == b'\\' {
                escape = true;
            } else if b == b'"' {
                string = false;
            }
        } else {
            match b {
                b'"' => string = true,
                b'{' | b'[' => {
                    depth += 1;
                    if depth > 12 {
                        return Err("depth exceeds12".into());
                    }
                }
                b'}' | b']' => depth = depth.saturating_sub(1),
                _ => {}
            }
        }
    }
    serde_json::from_slice::<Strict>(bytes)
        .map(|x| x.0)
        .map_err(|e| e.to_string())
}
pub(crate) fn counter(s: &str) -> Result<u64, String> {
    if s.is_empty() || (s.len() > 1 && s.starts_with('0')) || !s.bytes().all(|b| b.is_ascii_digit())
    {
        return Err("noncanonical counter".into());
    }
    s.parse().map_err(|_| "counter overflow".into())
}
pub(crate) fn id(s: &str) -> bool {
    let b = s.as_bytes();
    !b.is_empty()
        && b.len() <= 64
        && (b[0].is_ascii_lowercase() || b[0].is_ascii_digit())
        && b.iter()
            .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || b"._-".contains(c))
}
pub(crate) fn uuid(s: &str) -> bool {
    s.len() == 36
        && s.bytes().enumerate().all(|(i, c)| {
            if [8, 13, 18, 23].contains(&i) {
                c == b'-'
            } else {
                c.is_ascii_digit() || (b'a'..=b'f').contains(&c)
            }
        })
}
pub(crate) fn keys(v: &Value, wanted: &[&str]) -> Result<(), String> {
    let o = v.as_object().ok_or("expected object")?;
    if o.len() != wanted.len() || wanted.iter().any(|k| !o.contains_key(*k)) {
        return Err("missing or unknown field".into());
    }
    Ok(())
}
pub(crate) fn target_scope(t: &Target) -> String {
    t.monitor
        .as_deref()
        .map_or_else(|| "foh".into(), |m| m.replace("monitor-", "monitor"))
}
pub(crate) fn target_version(t: &Target, version: u8) -> Result<(), String> {
    let n = t
        .input
        .strip_prefix("input-")
        .and_then(|n| n.parse::<u16>().ok())
        .ok_or("input identity")?;
    if n == 0 || t.input != format!("input-{n:02}") || (version == 1 && n > 8) {
        return Err("unknown input".into());
    }
    match (t.parameter.as_str(), t.monitor.as_deref()) {
        ("fader" | "pan" | "mute", None) => Ok(()),
        ("send", Some(m)) => {
            let n = m
                .strip_prefix("monitor-")
                .and_then(|n| n.parse::<u16>().ok())
                .ok_or("monitor identity")?;
            if n == 0 || m != format!("monitor-{n}") || (version == 1 && n > 2) {
                return Err("unknown monitor".into());
            }
            Ok(())
        }
        _ => Err("unknown target".into()),
    }
}
pub(crate) fn value(t: &Target, v: &Value) -> Result<(), String> {
    if t.parameter == "mute" {
        if v.is_boolean() {
            return Ok(());
        }
    } else if let Some(n) = v.as_i64()
        && if t.parameter == "pan" {
            (-100..=100).contains(&n)
        } else {
            (-60000..=12000).contains(&n) && n % 100 == 0
        }
    {
        return Ok(());
    }
    Err("parameter range/type/step".into())
}
impl Snapshot {
    fn validate(&self) -> Result<(), String> {
        self.validate_version(1)
    }
    pub(crate) fn validate_version(&self, version: u8) -> Result<(), String> {
        self.validate_version_inner(version, false)
    }
    /// Equivalent to validating this authority after its canonical JSON roundtrip,
    /// without allocating and reparsing that JSON. Public typed ingest retains the
    /// same serialized admission cap; optional Value::Null follows serde Option.
    pub(crate) fn validate_canonical_version(&self, version: u8) -> Result<(), String> {
        self.validate_version_inner(version, true)?;
        canonical_size_within(
            self,
            if version == 1 {
                MAX_BYTES
            } else {
                MAX_DOCUMENT_BYTES
            },
        )
    }
    fn validate_version_inner(
        &self,
        version: u8,
        canonical_null_options: bool,
    ) -> Result<(), String> {
        if !matches!(version, 1 | 2) {
            return Err("authority version".into());
        }
        let input_count = if version == 1 { 8 } else { self.inputs.len() };
        let monitor_count = if version == 1 { 2 } else { self.monitors.len() };
        if input_count == 0 || input_count > u16::MAX as usize || monitor_count > u16::MAX as usize
        {
            return Err("inventory protocol width".into());
        }
        let parameter_count = input_count
            .checked_mul(3 + monitor_count)
            .ok_or("parameter capacity")?;
        if !uuid(&self.show_id) {
            return Err("invalid show".into());
        }
        for s in [&self.epoch, &self.revision, &self.sequence] {
            counter(s)?;
        }
        if self.page_count == 0
            || self.page_count > 16
            || self.page >= self.page_count
            || self.parameters.len() > parameter_count
        {
            return Err("page capacity".into());
        }
        // GP02 capabilities are fixed and truthful; GP03 must be separately accepted.
        if self.durability != "volatile"
            || self.validity != "unavailable"
            || self.acquisition_frame.is_some()
            || self.age_ms.is_some()
            || self.rendered_application
            || self.release_commit
            || self.session_history_capacity != 1024
        {
            return Err("unsupported GP02 capability/observation".into());
        }
        let inputs: BTreeSet<_> = self.inputs.iter().cloned().collect();
        if self.inputs.len() != input_count
            || inputs != (1..=input_count).map(|n| format!("input-{n:02}")).collect()
            || self.monitors.len() != monitor_count
            || self.monitors.iter().cloned().collect::<BTreeSet<_>>()
                != (1..=monitor_count)
                    .map(|n| format!("monitor-{n}"))
                    .collect()
        {
            return Err("inventory capacity/identity".into());
        }
        let modes: BTreeMap<_, _> = self.modes.iter().cloned().collect();
        if self.modes.len() != monitor_count + 1
            || modes.keys().cloned().collect::<BTreeSet<_>>()
                != std::iter::once("foh".to_string())
                    .chain((1..=monitor_count).map(|n| format!("monitor{n}")))
                    .collect()
            || modes
                .values()
                .any(|v| !matches!(v.as_str(), "manual" | "assist" | "auto"))
        {
            return Err("invalid modes".into());
        }
        let mut seen = BTreeSet::new();
        if self.automation_bounds.len() > if version == 1 { 40 } else { 64 } {
            return Err("bound capacity".into());
        }
        for b in &self.automation_bounds {
            self.validate_target(&b.target)?;
            value(&b.target, &b.min.into())?;
            value(&b.target, &b.max.into())?;
            let scope = target_scope(&b.target);
            if b.target.parameter == "mute"
                || b.min > b.max
                || !seen.insert(b.target.clone())
                || modes.get(&scope).map(String::as_str) != Some("auto")
            {
                return Err("invalid automation bounds".into());
            }
        }
        for (scope, mode) in &modes {
            if mode == "auto"
                && !self
                    .automation_bounds
                    .iter()
                    .any(|b| *scope == target_scope(&b.target))
            {
                return Err("AUTO without bounds".into());
            }
        }
        seen.clear();
        for p in &self.parameters {
            self.validate_target(&p.target)?;
            value(&p.target, &p.target_value)?;
            let canonical_option = |v: &Option<Value>| {
                v.as_ref()
                    .is_some_and(|v| !canonical_null_options || !v.is_null())
            };
            if canonical_option(&p.actual) {
                return Err("GP02 actual must be unavailable".into());
            }
            for v in [&p.proposal, &p.hold].into_iter().flatten() {
                if !canonical_null_options || !v.is_null() {
                    value(&p.target, v)?;
                }
            }
            if p.owner.as_ref().is_some_and(|o| !id(o))
                || canonical_option(&p.hold) != p.owner.is_some()
                || !seen.insert(p.target.clone())
            {
                return Err("owner/hold or duplicate target".into());
            }
        }
        Ok(())
    }
    pub(crate) fn validate_target(&self, t: &Target) -> Result<(), String> {
        target_version(t, 2)?;
        if !self.inputs.contains(&t.input)
            || t.monitor
                .as_ref()
                .is_some_and(|m| !self.monitors.contains(m))
        {
            return Err("target absent from inventory".into());
        }
        Ok(())
    }
    fn metadata(&self) -> Self {
        let mut x = self.clone();
        x.page = 0;
        x.parameters.clear();
        x
    }
}
// Every semantically valid Value leaf is scalar integer/bool or canonical null.
// Remaining authority fields have fixed typed nesting below the wire depth cap.
fn canonical_size_within<T: Serialize>(value: &T, limit: usize) -> Result<(), String> {
    struct Counter {
        bytes: usize,
        limit: usize,
    }
    impl std::io::Write for Counter {
        fn write(&mut self, bytes: &[u8]) -> std::io::Result<usize> {
            self.bytes = self
                .bytes
                .checked_add(bytes.len())
                .filter(|n| *n <= self.limit)
                .ok_or_else(|| {
                    std::io::Error::new(
                        std::io::ErrorKind::InvalidData,
                        "canonical authority byte admission",
                    )
                })?;
            Ok(bytes.len())
        }
        fn flush(&mut self) -> std::io::Result<()> {
            Ok(())
        }
    }
    serde_json::to_writer(Counter { bytes: 0, limit }, value).map_err(|e| e.to_string())
}
/// Exact wire-only target shape checks, after duplicate/depth/integer parsing and
/// before serde can normalize an explicitly present optional null field.
pub(crate) fn validate_snapshot_shape(v: &Value) -> Result<(), String> {
    for list in ["parameters", "automation_bounds"] {
        if let Some(a) = v[list].as_array() {
            for p in a {
                let t = &p["target"];
                keys(
                    t,
                    if t["parameter"] == "send" {
                        &["parameter", "input", "monitor"]
                    } else {
                        &["parameter", "input"]
                    },
                )?;
            }
        }
    }
    Ok(())
}
/// This read-only boundary consumes standalone Snapshot bodies from GP02.
/// Mutable request/reply handling belongs to DS04 after GP03 acceptance.
pub fn decode(bytes: &[u8]) -> Result<Snapshot, String> {
    decode_version(bytes, 1)
}
pub(crate) fn decode_version(bytes: &[u8], version: u8) -> Result<Snapshot, String> {
    let v = if version == 2 {
        parse_document(bytes)?
    } else {
        parse(bytes)?
    };
    validate_snapshot_shape(&v)?;
    let s: Snapshot = serde_json::from_value(v).map_err(|e| e.to_string())?;
    s.validate_version(version)?;
    Ok(s)
}
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Freshness {
    Fresh,
    Stale,
    Unavailable,
}
struct Assembly {
    start: u64,
    metadata: Snapshot,
    pages: Vec<Option<Snapshot>>,
}
/// Trusted display state only. Snapshot file receipt is an explicit read-only event.
pub struct Client {
    show: String,
    epoch: u64,
    trusted: Option<Snapshot>,
    receipt: Option<u64>,
    connected: bool,
    assembly: Option<Assembly>,
    selected: Option<String>,
    generation: u64,
}
impl Client {
    pub fn new(show: &str, epoch: u64) -> Result<Self, String> {
        if !uuid(show) {
            return Err("invalid expected show".into());
        }
        Ok(Self {
            show: show.into(),
            epoch,
            trusted: None,
            receipt: None,
            connected: true,
            assembly: None,
            selected: None,
            generation: 0,
        })
    }
    pub fn snapshot(&self) -> Option<&Snapshot> {
        self.trusted.as_ref()
    }
    pub fn is_collecting(&self) -> bool {
        self.assembly.is_some()
    }
    pub fn selected(&self) -> Option<&str> {
        self.selected.as_deref()
    }
    pub fn select(&mut self, id: &str) -> Result<(), String> {
        if !self
            .trusted
            .as_ref()
            .is_some_and(|s| s.inputs.iter().any(|x| x == id))
        {
            return Err("unknown stable input".into());
        }
        self.selected = Some(id.into());
        Ok(())
    }
    pub fn generation(&self) -> u64 {
        self.generation
    }
    pub fn freshness(&self, now: u64) -> Freshness {
        if self.trusted.is_none() {
            Freshness::Unavailable
        } else if self.connected
            && self.assembly.is_none()
            && self
                .receipt
                .is_some_and(|t| now >= t && now - t <= FRESH_MS)
        {
            Freshness::Fresh
        } else {
            Freshness::Stale
        }
    }
    pub fn disconnect(&mut self) {
        self.connected = false;
        self.receipt = None;
        self.assembly = None;
        self.generation = self.generation.saturating_add(1);
    }
    /// New epoch is explicitly pinned by the caller; old state remains visibly stale.
    pub fn reconnect(&mut self, epoch: u64) {
        self.disconnect();
        self.epoch = epoch;
        self.connected = true;
    }
    pub fn ingest(&mut self, bytes: &[u8], now: u64) -> Result<bool, String> {
        let result = self.collect(bytes, now);
        if result.is_err() {
            self.assembly = None;
            self.receipt = None;
        }
        result
    }
    fn collect(&mut self, bytes: &[u8], now: u64) -> Result<bool, String> {
        if !self.connected {
            return Err("disconnected".into());
        }
        let s = decode(bytes)?;
        if s.show_id != self.show || counter(&s.epoch)? != self.epoch {
            return Err("wrong show/epoch".into());
        }
        if let Some(old) = &self.trusted
            && old.epoch == s.epoch
            && (counter(&s.sequence)? <= counter(&old.sequence)?
                || counter(&s.revision)? < counter(&old.revision)?)
        {
            return Err("old sequence/revision".into());
        }
        if let Some(a) = &self.assembly {
            if now < a.start || now - a.start > ASSEMBLY_MS {
                return Err("expired page assembly".into());
            }
            if a.metadata != s.metadata() {
                return Err("mixed snapshot pages".into());
            }
        } else {
            self.assembly = Some(Assembly {
                start: now,
                metadata: s.metadata(),
                pages: vec![None; s.page_count],
            });
        }
        let a = self.assembly.as_mut().unwrap();
        let page = s.page;
        if a.pages[page].is_some() {
            return Err("duplicate page".into());
        }
        a.pages[page] = Some(s);
        if a.pages.iter().any(Option::is_none) {
            return Ok(false);
        }
        let mut complete = a.metadata.clone();
        complete.page_count = 1;
        complete.parameters = a
            .pages
            .iter()
            .flatten()
            .flat_map(|p| p.parameters.clone())
            .collect();
        complete.validate()?;
        if complete.parameters.len() != 40 {
            return Err("incomplete parameter inventory".into());
        }
        if !self
            .selected
            .as_ref()
            .is_some_and(|id| complete.inputs.contains(id))
        {
            self.selected = complete.inputs.first().cloned();
        }
        self.trusted = Some(complete);
        self.receipt = Some(now);
        self.assembly = None;
        Ok(true)
    }
    pub fn lines(&self, now: u64) -> Vec<String> {
        let mut lines = vec![format!(
            "PROVIDER C-AUDIO:1 / READ ONLY / {:?}",
            self.freshness(now)
        )];
        let Some(s) = &self.trusted else {
            lines.push("Snapshot unavailable".into());
            return lines;
        };
        lines.push(format!(
            "show {} epoch {} revision {} sequence {}",
            s.show_id, s.epoch, s.revision, s.sequence
        ));
        lines
            .push("GP02 metadata only; actual/frame unavailable; rendered/release disabled".into());
        for p in &s.parameters {
            if Some(p.target.input.as_str()) == self.selected() {
                lines.push(format!(
                    "{} {}{} actual={} target={} proposal={} owner={} hold={}",
                    p.target.input,
                    p.target.parameter,
                    p.target
                        .monitor
                        .as_ref()
                        .map(|m| format!("/{m}"))
                        .unwrap_or_default(),
                    display(&p.actual),
                    p.target_value,
                    display(&p.proposal),
                    p.owner.as_deref().unwrap_or("unavailable"),
                    display(&p.hold)
                ));
            }
        }
        lines
    }
}
fn display(v: &Option<Value>) -> String {
    v.as_ref()
        .map(ToString::to_string)
        .unwrap_or_else(|| "unavailable".into())
}

#[cfg(test)]
mod canonical_authority_tests {
    use super::*;
    use serde_json::json;
    fn authority(version: u8) -> Snapshot {
        let bytes: &[u8] = if version == 1 {
            include_bytes!("../tests/fixtures/gp03/v1/e03-rendered.json")
        } else {
            include_bytes!("../tests/fixtures/gp15/v1/raw-snapshot-16-1.json")
        };
        let value: Value = serde_json::from_slice(bytes).unwrap();
        serde_json::from_value(if version == 1 {
            value["initial"]["authority"].clone()
        } else {
            value["authority"].clone()
        })
        .unwrap()
    }
    fn old_roundtrip(s: &Snapshot, version: u8) -> Result<Snapshot, String> {
        let value = serde_json::to_value(s).unwrap();
        decode_version(&serde_json::to_vec(&value).unwrap(), version)
    }
    #[test]
    fn canonical_authority_matches_old_roundtrip_for_typed_nullable_values() {
        for version in [1, 2] {
            let original = authority(version);
            assert!(old_roundtrip(&original, version).is_ok());
            original.validate_canonical_version(version).unwrap();
            for field in ["actual", "proposal", "hold"] {
                for value in [Value::Null, json!(0), json!(0.5), json!({"nested":[1]})] {
                    for owner in [None, Some("desk".to_string())] {
                        let mut candidate = original.clone();
                        let p = &mut candidate.parameters[0];
                        match field {
                            "actual" => p.actual = Some(value.clone()),
                            "proposal" => p.proposal = Some(value.clone()),
                            _ => p.hold = Some(value.clone()),
                        }
                        p.owner = owner;
                        let owner_debug = p.owner.clone();
                        assert_eq!(
                            candidate.validate_canonical_version(version).is_ok(),
                            old_roundtrip(&candidate, version).is_ok(),
                            "v{version} {field} {value} owner={:?}",
                            owner_debug
                        );
                    }
                }
            }
            let mut null_actual = original.clone();
            null_actual.parameters[0].actual = Some(Value::Null);
            assert!(
                null_actual.validate_version(version).is_err(),
                "direct typed semantic entry not broadened"
            );
            null_actual.validate_canonical_version(version).unwrap();
            assert!(original.validate_canonical_version(3).is_err());
        }
    }
    #[test]
    fn canonical_typed_authority_rejects_semantically_valid_oversize_snapshot() {
        let mut candidate = authority(2);
        candidate.inputs = (1..=48).map(|n| format!("input-{n:02}")).collect();
        candidate.monitors = (1..=128).map(|n| format!("monitor-{n}")).collect();
        candidate.modes = std::iter::once(("foh".into(), "manual".into()))
            .chain((1..=128).map(|n| (format!("monitor{n}"), "manual".into())))
            .collect();
        candidate.automation_bounds.clear();
        candidate.parameters.clear();
        for input in &candidate.inputs {
            for (parameter, monitor) in ["fader", "pan", "mute"]
                .into_iter()
                .map(|p| (p, None))
                .chain(candidate.monitors.iter().map(|m| ("send", Some(m.clone()))))
            {
                let value = if parameter == "mute" {
                    json!(false)
                } else {
                    json!(0)
                };
                candidate.parameters.push(Parameter {
                    target: Target {
                        parameter: parameter.into(),
                        input: input.clone(),
                        monitor,
                    },
                    actual: None,
                    target_value: value.clone(),
                    proposal: Some(value.clone()),
                    hold: Some(value),
                    owner: Some("d".repeat(64)),
                });
            }
        }
        candidate.validate_version(2).unwrap();
        let bytes = serde_json::to_vec(&candidate).unwrap();
        assert!(
            bytes.len() > MAX_DOCUMENT_BYTES,
            "fixture must cross actual1MiB cap"
        );
        assert!(decode_version(&bytes, 2).is_err());
        assert!(
            candidate.validate_canonical_version(2).is_err(),
            "public typed admission must retain canonical byte bound"
        );
    }
    #[test]
    fn canonical_counting_writer_keeps_exact_version_byte_caps() {
        for cap in [MAX_BYTES, MAX_DOCUMENT_BYTES] {
            let exact = "x".repeat(cap - 2); // canonical JSON quotes add exactly two bytes
            assert_eq!(serde_json::to_vec(&exact).unwrap().len(), cap);
            canonical_size_within(&exact, cap).unwrap();
            assert!(canonical_size_within(&(exact + "x"), cap).is_err());
        }
    }
}
