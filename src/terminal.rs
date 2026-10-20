//! Ratatui presentation over the real Desk frontend and its existing authority worker.
use crate::{
    frontend::{Config, Event, Frontend},
    render::{Primitive, Scene},
};
use shr_instrument_panel::{
    scene,
    terminal::{
        Console,
        ratatui::crossterm::event::{
            self, Event as Input, KeyCode, KeyEventKind, KeyModifiers, MouseButton, MouseEventKind,
        },
    },
};
use std::time::{Duration, Instant};

const FRAME_INTERVAL: Duration = Duration::from_millis(100);
// Preserve provider pumping/PTT heartbeat independently of presentation.
const PUMP_INTERVAL: Duration = Duration::from_millis(40);

struct PresentationClock {
    next: Instant,
}
impl PresentationClock {
    fn due(&self, now: Instant) -> bool {
        now >= self.next
    }
    fn submitted(&mut self, now: Instant) {
        // No catch-up frames after slow rendering or a suspended terminal.
        self.next = now + FRAME_INTERVAL;
    }
    fn wait(&self, now: Instant) -> Duration {
        self.next.saturating_duration_since(now).min(PUMP_INTERVAL)
    }
}

pub fn primitives(s: &Scene) -> Vec<scene::Primitive> {
    s.primitives
        .iter()
        .map(|p| match p {
            Primitive::Rect { x, y, w, h, fill } => scene::Primitive::Rect {
                x: *x,
                y: *y,
                w: *w,
                h: *h,
                fill: (*fill).into(),
            },
            Primitive::Text { x, y, value, color } => scene::Primitive::Text {
                x: *x,
                y: *y,
                value: value.clone(),
                color: (*color).into(),
            },
            Primitive::Line {
                x1,
                y1,
                x2,
                y2,
                color,
            } => scene::Primitive::Line {
                x1: *x1,
                y1: *y1,
                x2: *x2,
                y2: *y2,
                color: (*color).into(),
            },
        })
        .collect()
}
pub fn key_name(c: KeyCode) -> Option<String> {
    Some(match c {
        KeyCode::Char(c) => c.to_string(),
        KeyCode::F(n) => format!("F{n}"),
        KeyCode::Left => "Left".into(),
        KeyCode::Right => "Right".into(),
        KeyCode::Up => "Up".into(),
        KeyCode::Down => "Down".into(),
        KeyCode::Tab => "Tab".into(),
        KeyCode::PageUp => "PageUp".into(),
        KeyCode::PageDown => "PageDown".into(),
        KeyCode::Enter => "Enter".into(),
        KeyCode::Backspace => "Backspace".into(),
        KeyCode::Esc => "Esc".into(),
        _ => return None,
    })
}
pub fn run(
    config: Config,
    role: Option<crate::roles::Config>,
    processing: bool,
    brain_audio: bool,
) -> Result<(), String> {
    let mut console = Console::open().map_err(|e| e.to_string())?;
    let mut front = Frontend::new(config);
    front.require_role();
    if let Some(role) = role {
        front.attach_role(role);
    }
    if processing {
        front.enable_processing()?;
    }
    if brain_audio {
        front.enable_brain_audio()?;
    }
    let mut processing_probe = std::time::Instant::now();
    let mut presentation = PresentationClock {
        next: Instant::now(),
    };
    let mut visible = false;
    let mut hits = Vec::new();
    let mut panel_ui = crate::channel_panel::PanelUi::default();
    let materials_enabled = crate::channel_material::Materials::enabled();
    let mut materials = crate::channel_material::Materials::default();
    let result = (|| -> Result<(), String> {
        loop {
            front.pump();
            // Initial focus/role acquisition can fence a queued read-only probe.
            // Retry only an unstarted capability probe, never a mutation or refusal.
            if processing
                && processing_probe.elapsed() >= Duration::from_millis(300)
                && front
                    .state
                    .as_ref()
                    .is_some_and(|u| u.processing_status == "unavailable; GP07 probe not enabled")
            {
                front.enable_processing()?;
                processing_probe = std::time::Instant::now();
            }
            if presentation.due(Instant::now()) {
                let s = if front.channel_panel_visible() {
                    Vec::new()
                } else {
                    primitives(&front.scene())
                };
                let material = if materials_enabled && front.channel_panel_visible() {
                    shr_instrument_panel::Material::Glyph
                } else {
                    console.material
                };
                let mut placements = Vec::new();
                console
                    .terminal
                    .draw(|f| {
                        hits.clear();
                        visible = if front.channel_panel_visible() {
                            hits = crate::channel_panel::draw_with_ui(
                                &front,
                                f.buffer_mut(),
                                material,
                                &panel_ui,
                            );
                            if materials_enabled && !front.eq_fullscreen {
                                placements = crate::channel_material::prepare(f.buffer_mut());
                            }
                            if front.eq_fullscreen {
                                f.area().width >= 60 && f.area().height >= 18
                            } else {
                                f.area().width >= 160 && f.area().height >= 45
                            }
                        } else if s
                            .iter()
                            .filter(|p| matches!(p, scene::Primitive::Rect { .. }))
                            .count()
                            == 1
                            && !s.iter().any(|p| matches!(p, scene::Primitive::Line { .. }))
                        {
                            scene::instrument_text(
                                &s,
                                f.buffer_mut(),
                                material,
                                front.fresh(),
                                front.state.as_ref().is_some_and(|s| s.writer_granted()),
                                &[
                                    "F1 MIX",
                                    "F2 CHANNEL",
                                    "B METERS",
                                    "F6 HEALTH",
                                    "F11 EQ",
                                    "F12 SENDS",
                                ],
                            )
                        } else {
                            scene::draw(&s, f.buffer_mut(), material)
                        };
                    })
                    .map_err(|e| e.to_string())?;
                if materials_enabled {
                    materials
                        .present(placements, &mut std::io::stdout())
                        .map_err(|e| e.to_string())?;
                }
                if visible {
                    front.mark_presented();
                } else {
                    front.talkback_release();
                }
                presentation.submitted(Instant::now());
            }
            if event::poll(presentation.wait(Instant::now())).map_err(|e| e.to_string())? {
                match event::read().map_err(|e| e.to_string())? {
                    Input::Key(k)
                        if k.code == KeyCode::Char('c')
                            && k.modifiers.contains(KeyModifiers::CONTROL) =>
                    {
                        break;
                    }
                    Input::Key(k)
                        if (visible
                            || (k.code == KeyCode::F(11) && front.channel_panel_visible()))
                            && k.kind != KeyEventKind::Repeat =>
                    {
                        if k.code == KeyCode::Enter && !console.release_events {
                            front.message =
                                "Confirmation unavailable: terminal lacks release events".into();
                            continue;
                        }
                        if let Some(key) = key_name(k.code) {
                            if brain_audio
                                && !console.release_events
                                && key.eq_ignore_ascii_case("t")
                            {
                                front.message =
                                    "Talkback unavailable: terminal lacks release events".into();
                                continue;
                            }
                            let pressed = k.kind == KeyEventKind::Press;
                            if front.channel_panel_visible()
                                && (front.eq_fullscreen || key == "F11")
                            {
                                presentation.next = Instant::now();
                            }
                            let _ = front.enqueue(Event::Key {
                                key: key.clone(),
                                pressed,
                            });
                            // Ordinary legacy keypresses have no hold semantics. PTT above fails closed.
                            if pressed && !console.release_events {
                                let _ = front.enqueue(Event::Key {
                                    key,
                                    pressed: false,
                                });
                            }
                        }
                    }
                    Input::Mouse(mouse) if visible && front.channel_panel_visible() => {
                        if let Some(hit) = hits
                            .iter()
                            .find(|h| h.area.contains((mouse.column, mouse.row).into()))
                        {
                            match (&hit.target, mouse.kind) {
                                (
                                    crate::channel_panel::Target::Field(index),
                                    MouseEventKind::Down(MouseButton::Left),
                                ) => {
                                    if let Err(e) = front.channel_panel_field(*index) {
                                        front.message = e;
                                    }
                                }
                                (
                                    crate::channel_panel::Target::Key(key),
                                    MouseEventKind::Down(MouseButton::Left),
                                ) => {
                                    let _ = front.enqueue(Event::Key {
                                        key: (*key).into(),
                                        pressed: true,
                                    });
                                    let _ = front.enqueue(Event::Key {
                                        key: (*key).into(),
                                        pressed: false,
                                    });
                                }
                                (
                                    crate::channel_panel::Target::Menu(menu),
                                    MouseEventKind::Down(MouseButton::Left),
                                ) => {
                                    panel_ui.menu = *menu;
                                    panel_ui.secondary = false;
                                }
                                (
                                    crate::channel_panel::Target::Layer,
                                    MouseEventKind::Down(MouseButton::Left),
                                ) => {
                                    panel_ui.secondary = !panel_ui.secondary;
                                }
                                _ => {}
                            }
                        }
                    }
                    Input::FocusLost => {
                        let _ = front.enqueue(Event::Focus(false));
                    }
                    Input::FocusGained => {
                        let _ = front.enqueue(Event::Focus(true));
                    }
                    Input::Resize(w, h) => {
                        visible = false;
                        hits.clear();
                        front.fence();
                        let _ = front.enqueue(Event::Resize(u32::from(w) * 12, u32::from(h) * 24));
                    }
                    _ => {}
                }
            }
        }
        Ok(())
    })();
    let _ = front.enqueue(Event::Focus(false));
    let _ = front.enqueue(Event::DeviceLost);
    front.pump();
    result
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn input_bursts_cannot_bypass_presentation_deadline() {
        let start = Instant::now();
        let mut clock = PresentationClock { next: start };
        assert!(clock.due(start));
        clock.submitted(start);
        for ms in 0..100 {
            assert!(!clock.due(start + Duration::from_millis(ms)));
        }
        assert!(clock.due(start + FRAME_INTERVAL));
        assert_eq!(clock.wait(start), PUMP_INTERVAL);
        assert_eq!(
            clock.wait(start + Duration::from_millis(90)),
            Duration::from_millis(10)
        );
        assert_eq!(clock.wait(start + Duration::from_secs(1)), Duration::ZERO);
        // Rendering stalls skip missed frames rather than replaying them.
        let late = start + Duration::from_secs(1);
        clock.submitted(late);
        assert!(!clock.due(late));
        assert!(clock.due(late + FRAME_INTERVAL));
    }
}
