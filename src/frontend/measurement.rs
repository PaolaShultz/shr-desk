//! Detached measurement workflow over the one existing provider worker.
use super::*;
use crate::pa_measurement::editor::{Action as MeasureAction, Editor};
impl Frontend {
    pub(super) fn measurement_action(&mut self, action: MeasureAction) -> Result<(), String> {
        use crate::pa_measurement::editor::Operation as M;
        match action {
            MeasureAction::Open | MeasureAction::Refresh => {
                self.fx_ui.open = false;
                self.measurement_ui.open = true;
                self.send(Operation::Measurement(M::Probe))
            }
            MeasureAction::Capture
            | MeasureAction::CancelCapture
            | MeasureAction::Result
            | MeasureAction::Propose
            | MeasureAction::Apply => {
                self.measurement_ui.open = true;
                self.measurement_ui.editor = Some(match action {
                    MeasureAction::Capture => Editor::Capture,
                    MeasureAction::CancelCapture => Editor::Cancel,
                    MeasureAction::Result => Editor::Result,
                    MeasureAction::Propose => Editor::Propose,
                    _ => Editor::Apply,
                });
                self.measurement_ui.text.clear();
                self.measurement_ui.scroll = 0;
                Ok(())
            }
            MeasureAction::Text(s) => {
                if s == "\u{8}" {
                    self.measurement_ui.text.pop();
                    Ok(())
                } else {
                    self.measurement_ui.append(&s)
                }
            }
            MeasureAction::Scroll(n) => {
                self.measurement_ui.scroll(n);
                Ok(())
            }
            MeasureAction::Submit => {
                let editor = self
                    .measurement_ui
                    .editor
                    .ok_or("measurement editor absent")?;
                let command =
                    crate::pa_measurement::editor::parse(editor, &self.measurement_ui.text)?;
                self.send(Operation::Measurement(command))?;
                self.measurement_ui.editor = None;
                Ok(())
            }
        }
    }

    pub(super) fn measurement_key(&self, key: &str) -> Option<MeasureAction> {
        if !self.measurement_ui.open || self.state.as_ref().is_some_and(|u| u.review.is_some()) {
            return None;
        }
        if self.measurement_ui.editor.is_some() {
            return match key {
                "Enter" => Some(MeasureAction::Submit),
                "Backspace" => Some(MeasureAction::Text("\u{8}".into())),
                "Esc" | "Escape" => None,
                s if s.len() == 1 => Some(MeasureAction::Text(s.into())),
                _ => None,
            };
        }
        match key.to_uppercase().as_str() {
            "C" => Some(MeasureAction::Capture),
            "X" => Some(MeasureAction::CancelCapture),
            "R" => Some(MeasureAction::Result),
            "P" => Some(MeasureAction::Propose),
            "A" => Some(MeasureAction::Apply),
            "Y" => Some(MeasureAction::Refresh),
            "PAGEUP" => Some(MeasureAction::Scroll(-1)),
            "PAGEDOWN" => Some(MeasureAction::Scroll(1)),
            _ => None,
        }
    }
    pub(super) fn measurement_scene(&self) -> Scene {
        let mut scene = Scene::default();
        scene.primitives.push(Primitive::Rect {
            x: 0,
            y: 0,
            w: 1920,
            h: 1080,
            fill: "#10151d",
        });
        let mut lines=vec!["PA MEASUREMENT / SOFTWARE ONLY / no microphone or physical output opened".into(),
            "Y probe/refresh | C capture | X cancel capture | R result | P propose | A muted apply | PgUp/PgDn scroll".into(),
            "Source/reference/capture slots and PA outputs are different identities. Output/slot indices are ZERO BASED.".into()];
        if let Some(editor) = self.measurement_ui.editor {
            lines.push(editor.help().into());
            lines.push(format!("Detached input: {}", self.measurement_ui.text));
            lines.push("Enter requests exact review; then inspect every page and confirm separately. Esc cancels unsent content.".into());
        }
        if let Some(s) = &self.state {
            lines.extend(s.measurement.lines());
        }
        if self
            .state
            .as_ref()
            .is_none_or(|s| s.measurement.snapshot.is_none())
        {
            lines.extend(self.measurement_ui.report.iter().cloned());
        }
        lines.push(self.message.clone());
        let lines: Vec<String> = lines
            .into_iter()
            .flat_map(|line| {
                let chars: Vec<char> = line.chars().collect();
                chars
                    .chunks(152)
                    .map(|c| c.iter().collect())
                    .collect::<Vec<_>>()
            })
            .collect();
        for (n, line) in lines
            .iter()
            .skip(self.measurement_ui.scroll)
            .take(40)
            .enumerate()
        {
            scene.primitives.push(Primitive::Text {
                x: 24,
                y: 16 + n as u32 * 25,
                value: line.clone(),
                color: "#e4e8e9",
            });
        }
        scene
    }
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn ordinary_lowercase_commands_preserve_literal_ids_and_escape_fences() {
        let mut f = Frontend::new(Config {
            wire_version: 2,
            remote: None,
            endpoint: "/nonexistent/measurement-test.sock".into(),
            show: "11111111-1111-4111-8111-111111111111".into(),
            epoch: 1,
            writer: "measurement-test".into(),
            scope: "pa_configuration".into(),
        });
        f.measurement_ui.open = true;
        assert_eq!(f.measurement_key("c"), Some(MeasureAction::Capture));
        assert_eq!(f.measurement_key("p"), Some(MeasureAction::Propose));
        f.measurement_action(MeasureAction::Capture).unwrap();
        for c in "a input-01 16 0 p1 32768 2048 100 10000".chars() {
            let a = f.measurement_key(&c.to_string()).unwrap();
            f.measurement_action(a).unwrap();
        }
        assert!(f.measurement_ui.text.contains("input-01"));
        assert!(f.measurement_key("Esc").is_none());
        let retained = f.measurement_ui.text.clone();
        f.fence();
        assert_eq!(f.measurement_ui.text, retained);
        f.action(Action::Cancel).unwrap();
        assert!(f.measurement_ui.editor.is_none());
    }
}
#[cfg(test)]
mod page_regressions {
    use super::*;
    #[test]
    fn literal_w_in_measurement_entry_precedes_global_fx_and_pages_exclude_each_other(){
        let mut f=Frontend::new(Config{wire_version:2,remote:None,endpoint:"/nonexistent/page-test.sock".into(),show:"11111111-1111-4111-8111-111111111111".into(),epoch:1,writer:"page-test".into(),scope:"pa_configuration".into()});
        f.measurement_ui.open=true;f.measurement_action(MeasureAction::Capture).unwrap();
        for c in "row1".chars(){f.key(&c.to_string()).unwrap();}
        assert_eq!(f.measurement_ui.text,"row1");assert!(!f.fx_ui.open);
        f.measurement_ui.editor=None;let _=f.key("w");assert!(f.fx_ui.open);assert!(!f.measurement_ui.open);
        let _=f.key("y");assert!(f.measurement_ui.open);assert!(!f.fx_ui.open);
    }
}
