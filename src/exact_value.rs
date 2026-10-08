//! Exact FOH values, parsed in integer arithmetic without rounding or clamping.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Parameter {
    Fader,
    Pan,
}
impl Parameter {
    pub fn name(self) -> &'static str {
        match self {
            Self::Fader => "fader",
            Self::Pan => "pan",
        }
    }
    pub fn parse(self, text: &str) -> Result<i32, String> {
        if text.is_empty() || text.len() > 16 {
            return Err("exact entry requires 1..16 ASCII characters".into());
        }
        let unsigned = text
            .strip_prefix('-')
            .or_else(|| text.strip_prefix('+'))
            .unwrap_or(text);
        let negative = text.starts_with('-');
        let (whole, fraction) = unsigned.split_once('.').unwrap_or((unsigned, ""));
        if whole.is_empty()
            || !whole.bytes().all(|b| b.is_ascii_digit())
            || !fraction.bytes().all(|b| b.is_ascii_digit())
            || (unsigned.contains('.') && fraction.is_empty())
            || (self == Self::Pan && unsigned.contains('.'))
            || (self == Self::Fader && fraction.len() > 1)
        {
            return Err(
                "fader: decimal dB with at most one fractional digit; pan: signed integer".into(),
            );
        }
        let mut value: i32 = whole.parse().map_err(|_| "exact value overflow")?;
        if self == Self::Fader {
            value = value
                .checked_mul(1000)
                .and_then(|v| v.checked_add(fraction.parse::<i32>().unwrap_or(0) * 100))
                .ok_or("exact value overflow")?;
        }
        if negative {
            value = -value;
        }
        let range = match self {
            Self::Fader => -60000..=12000,
            Self::Pan => -100..=100,
        };
        if !range.contains(&value) {
            return Err("fader -60.0..+12.0 dB step0.1; pan -100..100 step1 (L/center/R)".into());
        }
        Ok(value)
    }
    pub fn display(self, value: i32) -> String {
        match self {
            Self::Fader => format!("{:+.1} dB", f64::from(value) / 1000.),
            Self::Pan => {
                if value == 0 {
                    "0 / center (pan %)".into()
                } else {
                    format!(
                        "{value:+} / {} {}%",
                        if value < 0 { "L" } else { "R" },
                        value.abs()
                    )
                }
            }
        }
    }
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn exact_ranges_steps_and_syntax() {
        for (text, value) in [
            ("-60.0", -60000),
            ("+12.0", 12000),
            ("-6.1", -6100),
            ("0", 0),
            ("-0.1", -100),
        ] {
            assert_eq!(Parameter::Fader.parse(text).unwrap(), value);
        }
        for text in [
            "",
            "NaN",
            "inf",
            "1e0",
            " 1",
            "1 ",
            ".1",
            "1.",
            "--1",
            "1.01",
            "1.00",
            "-60.1",
            "12.1",
            "9999999999999999",
            "00000000000000000",
        ] {
            assert!(Parameter::Fader.parse(text).is_err(), "{text}");
        }
        for (text, value) in [("-100", -100), ("100", 100), ("+31", 31), ("0", 0)] {
            assert_eq!(Parameter::Pan.parse(text).unwrap(), value);
        }
        for text in ["1.0", "101", "-101", "L20", "center", "+", "-", "1e1"] {
            assert!(Parameter::Pan.parse(text).is_err(), "{text}");
        }
        assert!(Parameter::Pan.display(-20).contains("L 20%"));
        assert!(Parameter::Pan.display(20).contains("R 20%"));
        assert!(Parameter::Pan.display(0).contains("center"));
    }
}
