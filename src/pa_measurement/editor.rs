//! Detached operator input; transport and PA analysis remain outside this module.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Action {
    Open,
    Refresh,
    Capture,
    CancelCapture,
    Result,
    Propose,
    Apply,
    Text(String),
    Submit,
    Scroll(i32),
}
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Editor {
    Capture,
    Cancel,
    Result,
    Propose,
    Apply,
}
impl Editor {
    pub fn help(self) -> &'static str {
        match self {
            Self::Capture => {
                "ID REFERENCE_INPUT MIC_CAPTURE_SLOT OUTPUT_INDEX POSITION_ID SAMPLES MAX_LAG_SAMPLES LOW_HZ HIGH_HZ"
            }
            Self::Cancel => "CAPTURE_ID (explicit cancellation, does not undo completed results)",
            Self::Result => "RESULT_ID (read only; includes uncertainty and every spectral bin)",
            Self::Propose => {
                "PROPOSAL_ID ANCHOR_CAPTURE TARGET_CAPTURE [ANCHOR TARGET ... up to8positions]"
            }
            Self::Apply => {
                "PROPOSAL_ID (fresh basis; muted whole configuration review; rearm separate)"
            }
        }
    }
}
#[derive(Clone, Debug, Default)]
pub struct State {
    pub open: bool,
    pub editor: Option<Editor>,
    pub text: String,
    pub scroll: usize,
    pub report: Vec<String>,
}
impl State {
    pub fn append(&mut self, s: &str) -> Result<(), String> {
        if self.editor.is_none()
            || self.text.len() + s.len() > 1024
            || !s.bytes().all(|c| c.is_ascii_graphic() || c == b' ')
        {
            return Err("measurement detached input bound".into());
        }
        self.text.push_str(s);
        Ok(())
    }
    pub fn scroll(&mut self, delta: i32) {
        self.scroll = (self.scroll as i64 + i64::from(delta) * 28).clamp(0, 4096) as usize;
    }
}
#[derive(Clone, Debug)]
pub enum Operation {
    Probe,
    Result(String),
    Review {
        kind: String,
        body: serde_json::Value,
    },
    Apply(String),
}
pub fn parse(editor: Editor, text: &str) -> Result<Operation, String> {
    use serde_json::json;
    let f: Vec<&str> = text.split_whitespace().collect();
    let n = |i: usize| {
        f.get(i)
            .ok_or("missing numeric field")?
            .parse::<u64>()
            .map_err(|_| "unsigned integer field required".to_string())
    };
    let op = match editor {
        Editor::Capture if f.len() == 9 => Operation::Review {
            kind: "capture_start".into(),
            body: json!({"id":f[0],"reference_input":f[1],"mic_capture_slot":n(2)?,"output_index":n(3)?,"position_id":f[4],"samples":n(5)?,"options":{"max_arrival_samples":n(6)?,"band_hz":[n(7)?,n(8)?]}}),
        },
        Editor::Cancel if f.len() == 1 => Operation::Review {
            kind: "capture_cancel".into(),
            body: json!({"id":f[0]}),
        },
        Editor::Result if f.len() == 1 => Operation::Result(f[0].into()),
        Editor::Apply if f.len() == 1 => Operation::Apply(f[0].into()),
        Editor::Propose if f.len() >= 3 && f.len() % 2 == 1 => Operation::Review {
            kind: "measurement_propose".into(),
            body: json!({"id":f[0],"pairs":f[1..].chunks(2).collect::<Vec<_>>()}),
        },
        _ => return Err(format!("Required fields: {}", editor.help())),
    };
    match &op {
        Operation::Review { kind, body } => super::wire::validate_body(kind, body, None)?,
        Operation::Result(id) | Operation::Apply(id) if !super::identifier(id) => {
            return Err("measurement identifier".into());
        }
        _ => (),
    };
    Ok(op)
}
