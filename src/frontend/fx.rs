//! Detached FX editing uses the existing frontend worker and material-review pages.
use super::*;
use crate::fx::{Action as F, Operation as O};
impl Frontend {
    pub(super) fn fx_action(&mut self, a: F) -> Result<(), String> {
        match a {
            F::Scope => self.action(Action::SwitchScope("fx_configuration".into())),
            F::Open | F::Probe => {
                self.measurement_ui.open = false;
                self.fx_ui.open = true;
                self.fx_ui.text = None;
                self.fx_ui.basis = None;
                self.send(Operation::Fx(O::Probe))
            }
            F::Select(n) => {
                if n > 1 {
                    return Err("FX left/right selection".into());
                }
                self.fx_ui.selected = n;
                self.fx_ui.text = None;
                self.fx_ui.basis = None;
                Ok(())
            }
            F::Text(t) => {
                let text = self
                    .fx_ui
                    .text
                    .as_mut()
                    .ok_or("FX detached editor absent")?;
                if t == "\u{8}" {
                    text.pop();
                } else if text.len() + t.len() <= 128
                    && t.chars().all(|c| c.is_ascii_graphic() || c == ' ')
                {
                    text.push_str(&t);
                } else {
                    return Err("FX text capacity".into());
                }
                Ok(())
            }
            F::Edit | F::Bypass | F::Panic(_) => {
                if self.provider.pending_edits.load(Ordering::Acquire) != 0
                    || self.state.as_ref().is_some_and(|u| u.review.is_some())
                {
                    return Err("wait for pending operation/review".into());
                }
                let u = self.state.as_ref().ok_or("FX provider absent")?;
                if !self.fresh() || u.fx.unknown || !u.fx_is_fresh() {
                    return Err(
                        "FX explicit fresh probe required; retained state does not authorize"
                            .into(),
                    );
                }
                let basis =
                    u.fx.snapshot
                        .as_ref()
                        .and_then(|s| s.observation.clone())
                        .ok_or("FX owner unavailable")?;
                if !basis.settled() {
                    return Err("FX owner still transitioning".into());
                }
                self.fx_ui.basis = Some(basis.clone());
                match a {
                    F::Edit => {
                        self.fx_ui.text = Some(String::new());
                        Ok(())
                    }
                    F::Bypass => {
                        let mut c = crate::fx::Configuration::decode(&basis.owner_json)?;
                        let i = self.fx_ui.selected as usize;
                        c.channels[i].bypass = !c.channels[i].bypass;
                        self.send(Operation::Fx(O::Review {
                            basis: Box::new(basis),
                            configuration_json: Some(
                                serde_json::to_string(&c).map_err(|e| e.to_string())?,
                            ),
                            panic_mask: None,
                        }))
                    }
                    F::Panic(mask) => {
                        if !(1..=3).contains(&mask) {
                            return Err("FX panic mask".into());
                        }
                        self.send(Operation::Fx(O::Review {
                            basis: Box::new(basis),
                            configuration_json: None,
                            panic_mask: Some(u32::from(mask)),
                        }))
                    }
                    _ => unreachable!(),
                }
            }
            F::Submit => {
                let basis = self
                    .fx_ui
                    .basis
                    .clone()
                    .ok_or("FX editor invalidated; probe and edit again")?;
                let text = self.fx_ui.text.as_ref().ok_or("FX editor absent")?;
                let config = crate::fx::edit_channel(&basis, self.fx_ui.selected, text)?;
                self.send(Operation::Fx(O::Review {
                    basis: Box::new(basis),
                    configuration_json: Some(config),
                    panic_mask: None,
                }))?;
                self.fx_ui.text = None;
                Ok(())
            }
        }
    }
    pub(super) fn fx_key(&self, key: &str) -> Option<F> {
        if !self.fx_ui.open || self.state.as_ref().is_some_and(|s| s.review.is_some()) {
            return None;
        }
        if self.fx_ui.text.is_some() {
            return match key {
                "Enter" => Some(F::Submit),
                "Backspace" => Some(F::Text("\u{8}".into())),
                "Esc" | "Escape" => None,
                s if s.len() == 1 => Some(F::Text(s.into())),
                _ => None,
            };
        }
        match key.to_ascii_uppercase().as_str() {
            "W" => Some(F::Probe),
            "X" => Some(F::Scope),
            "L" => Some(F::Select(0)),
            "R" => Some(F::Select(1)),
            "E" => Some(F::Edit),
            " " | "SPACE" => Some(F::Bypass),
            "P" => Some(F::Panic(1 << self.fx_ui.selected)),
            "B" => Some(F::Panic(3)),
            _ => None,
        }
    }
    pub(super) fn fx_scene(&self) -> Scene {
        let mut scene = Scene::default();
        scene.primitives.push(Primitive::Rect {
            x: 0,
            y: 0,
            w: 1920,
            h: 1080,
            fill: "#10151d",
        });
        let mut lines=vec!["ACTUAL BRAIN FOH STEREO FX / one stereo instance / software control".into(),"W explicit probe | X attach FX scope read-only | L/R select | E numeric entry | Space bypass | P selected panic | B both panic | G grant | Q release | Esc cancel".into(),format!("Selected {}",if self.fx_ui.selected==0{"foh-left"}else{"foh-right"})];
        if let Some(text) = &self.fx_ui.text {
            lines.push("Detached numbers: delay_ms(1..500) feedback(0..0.85) damping coefficient(0..0.99) wet_gain(0..1)".into());
            lines.push(format!("Literal entry: {text}"));
            lines.push("Enter requests full material review; inspect all pages and confirm separately. Esc discards unsent text.".into());
        }
        if let Some(s) = &self.state {
            lines.push(format!(
                "Fresh authorization={} / attached scope {} / lease remaining {:?} ms",
                s.fx_is_fresh() && !s.fx.unknown,
                self.scope,
                s.writer_lease_remaining_ms
            ));
            lines.extend(s.fx.lines());
        } else {
            lines.push("Unavailable until explicit owner probe".into());
        }
        if self.state.as_ref().is_none_or(|s| s.fx.snapshot.is_none()) {
            lines.extend(self.fx_ui.report.iter().cloned());
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
        for (i, line) in lines.into_iter().take(40).enumerate() {
            scene.primitives.push(Primitive::Text {
                x: 24,
                y: 16 + i as u32 * 25,
                value: line,
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
    fn fx_native_space_escape_and_literal_lowercase_entry() {
        let mut f = Frontend::new(Config {
            wire_version: 2,
            remote: None,
            endpoint: "/nonexistent/fx-test.sock".into(),
            show: "11111111-1111-4111-8111-111111111111".into(),
            epoch: 1,
            writer: "fx-test".into(),
            scope: "fx_configuration".into(),
        });
        f.fx_ui.open = true;
        assert_eq!(f.fx_key("l"), Some(F::Select(0)));
        assert_eq!(f.fx_key(" "), Some(F::Bypass));
        assert_eq!(f.fx_key("b"), Some(F::Panic(3)));
        f.fx_ui.text = Some(String::new());
        f.fx_action(F::Text("1e2 0.2 0.3 0.4".into())).unwrap();
        assert!(f.fx_ui.text.as_ref().unwrap().starts_with("1e2"));
        assert!(f.fx_key("Esc").is_none());
        f.fence();
        assert!(f.fx_ui.basis.is_none());
        f.action(Action::Cancel).unwrap();
        assert!(f.fx_ui.text.is_none());
    }
}
#[cfg(test)]
mod scene_tests {
    use super::*;
    #[test]
    #[ignore = "explicit new private FX scene directory; optional CPU lavapipe, no window"]
    fn fx_offline_scene_cpu_readback() {
        let root = std::path::PathBuf::from(std::env::var("SHR_DESK_FX_SCENES").unwrap());
        std::fs::create_dir(&root).unwrap();
        let mut f = Frontend::new(Config {
            wire_version: 2,
            remote: None,
            endpoint: "/nonexistent/fx-scene.sock".into(),
            show: "11111111-1111-4111-8111-111111111111".into(),
            epoch: 9,
            writer: "fx-scene".into(),
            scope: "fx_configuration".into(),
        });
        f.fx_ui.open = true;
        f.fx_ui.report = crate::fx::State {
            snapshot: Some(
                crate::fx::Snapshot::decode(
                    serde_json::from_str::<Value>(include_str!(
                        "../../tests/fixtures/gp21/v1/owner-relay.json"
                    ))
                    .unwrap()["snapshot"]
                        .clone(),
                )
                .unwrap(),
            ),
            ..Default::default()
        }
        .lines();
        f.message =
            "OFFLINE fixture scene; simulated retained owner evidence, not a connected engine"
                .into();
        let scene = f.scene();
        assert!(scene.in_bounds());
        std::fs::write(root.join("fx-page.svg"), crate::render::svg(&scene)).unwrap();
        crate::raster::ppm(&scene, &root.join("fx-page.ppm")).unwrap();
        #[cfg(feature = "native")]
        for (w, h) in [(1920, 1080), (960, 540), (728, 1024)] {
            println!("{}", crate::native::offscreen_at(&scene, w, h).unwrap());
        }
    }
}
#[cfg(test)]
mod literal_release_regression {
    use super::*;
    #[test]
    fn detached_numeric_key_releases_do_not_fill_provider_queue() {
        let mut f = Frontend::new(Config {
            wire_version: 2,
            remote: None,
            endpoint: "/nonexistent/literal-test.sock".into(),
            show: "11111111-1111-4111-8111-111111111111".into(),
            epoch: 1,
            writer: "literal-test".into(),
            scope: "fx_configuration".into(),
        });
        let (tx, rx) = std::sync::mpsc::sync_channel(8);
        f.provider.tx = tx;
        f.fx_ui.open = true;
        f.fx_ui.text = Some(String::new());
        for key in "1e2 0.2 0.3 0.4".chars() {
            for pressed in [true, false] {
                f.enqueue(Event::Key {
                    key: key.to_string(),
                    pressed,
                })
                .unwrap();
                f.pump();
            }
        }
        assert_eq!(f.fx_ui.text.as_deref(), Some("1e2 0.2 0.3 0.4"));
        assert_eq!(rx.try_iter().count(), 0);
        f.fx_ui.open = false;
        f.fx_ui.text = None;
        f.measurement_ui.open = true;
        f.measurement_ui.editor = Some(crate::pa_measurement::editor::Editor::Capture);
        for key in "row1".chars() {
            for pressed in [true, false] {
                f.enqueue(Event::Key {
                    key: key.to_string(),
                    pressed,
                })
                .unwrap();
                f.pump();
            }
        }
        assert_eq!(f.measurement_ui.text, "row1");
        assert_eq!(rx.try_iter().count(), 0);
    }
}
