//! Scope labels for presentation, with the producer's exact externally tagged wire form.
use serde::{Deserialize, Serialize};
use serde_json::Value;

#[derive(Deserialize, Serialize)]
#[serde(rename_all = "snake_case", deny_unknown_fields)]
enum Wire {
    Foh,
    Monitor1,
    Monitor2,
    Monitor(u16),
    PaConfiguration,
    OutputRoutes,
}
impl Wire {
    fn label(self) -> Result<String, String> {
        Ok(match self {
            Self::Foh => "foh".into(),
            Self::Monitor1 => "monitor1".into(),
            Self::Monitor2 => "monitor2".into(),
            Self::Monitor(n) if n >= 3 => format!("monitor{n}"),
            Self::Monitor(_) => return Err("noncanonical monitor scope".into()),
            Self::PaConfiguration => "pa_configuration".into(),
            Self::OutputRoutes => "output_routes".into(),
        })
    }
    fn from_label(label: &str) -> Result<Self, String> {
        Ok(match label {
            "foh" => Self::Foh,
            "monitor1" => Self::Monitor1,
            "monitor2" => Self::Monitor2,
            "pa_configuration" => Self::PaConfiguration,
            "output_routes" => Self::OutputRoutes,
            _ => {
                let n: u16 = label
                    .strip_prefix("monitor")
                    .ok_or("scope")?
                    .parse()
                    .map_err(|_| "monitor scope")?;
                if n < 3 || label != format!("monitor{n}") {
                    return Err("noncanonical scope".into());
                }
                Self::Monitor(n)
            }
        })
    }
}
pub fn value(label: &str) -> Result<Value, String> {
    serde_json::to_value(Wire::from_label(label)?).map_err(|e| e.to_string())
}
pub fn valid(label: &str) -> bool {
    Wire::from_label(label).is_ok()
}
pub mod one {
    use super::*;
    pub fn serialize<S: serde::Serializer>(label: &str, s: S) -> Result<S::Ok, S::Error> {
        Wire::from_label(label)
            .map_err(serde::ser::Error::custom)?
            .serialize(s)
    }
    pub fn deserialize<'de, D: serde::Deserializer<'de>>(d: D) -> Result<String, D::Error> {
        Wire::deserialize(d)?
            .label()
            .map_err(serde::de::Error::custom)
    }
}
pub mod optional {
    use super::*;
    pub fn serialize<S: serde::Serializer>(
        label: &Option<String>,
        s: S,
    ) -> Result<S::Ok, S::Error> {
        label
            .as_deref()
            .map(Wire::from_label)
            .transpose()
            .map_err(serde::ser::Error::custom)?
            .serialize(s)
    }
    pub fn deserialize<'de, D: serde::Deserializer<'de>>(d: D) -> Result<Option<String>, D::Error> {
        Option::<Wire>::deserialize(d)?
            .map(Wire::label)
            .transpose()
            .map_err(serde::de::Error::custom)
    }
}
pub mod modes {
    use super::*;
    pub fn serialize<S: serde::Serializer>(
        modes: &[(String, String)],
        s: S,
    ) -> Result<S::Ok, S::Error> {
        modes
            .iter()
            .map(|(scope, mode)| Ok((Wire::from_label(scope)?, mode)))
            .collect::<Result<Vec<_>, String>>()
            .map_err(serde::ser::Error::custom)?
            .serialize(s)
    }
    pub fn deserialize<'de, D: serde::Deserializer<'de>>(
        d: D,
    ) -> Result<Vec<(String, String)>, D::Error> {
        Vec::<(Wire, String)>::deserialize(d)?
            .into_iter()
            .map(|(scope, mode)| Ok((scope.label()?, mode)))
            .collect::<Result<_, String>>()
            .map_err(serde::de::Error::custom)
    }
}
