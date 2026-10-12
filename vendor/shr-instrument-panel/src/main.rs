use ratatui::{
    Terminal,
    backend::CrosstermBackend,
    crossterm::{
        cursor::{Hide, Show},
        execute,
        terminal::{EnterAlternateScreen, LeaveAlternateScreen, disable_raw_mode, enable_raw_mode},
    },
    layout::Rect,
    style::{Color, Style},
    widgets::{Paragraph, Widget},
};
use shr_instrument_panel::{
    protocol::{Input, Session},
    *,
};
use std::{
    io::{self, IsTerminal, Write},
    time::Instant,
};
struct Restore;
impl Drop for Restore {
    fn drop(&mut self) {
        let _ = execute!(io::stdout(), Show, LeaveAlternateScreen);
        let _ = execute!(
            io::stdout(),
            ratatui::crossterm::event::DisableMouseCapture,
            ratatui::crossterm::event::DisableFocusChange
        );
        let _ = disable_raw_mode();
    }
}
fn main() -> io::Result<()> {
    if !io::stdin().is_terminal() || !io::stdout().is_terminal() {
        return Err(io::Error::other(
            "specimen requires an interactive terminal",
        ));
    }
    enable_raw_mode()?;
    let _restore = Restore;
    execute!(io::stdout(), EnterAlternateScreen, Hide)?;
    let mut input = Input::default();
    let mut session = Session::new();
    let startup = Instant::now();
    let requested = std::env::args().any(|a| a == "--glyphs");
    let font_mode = std::env::args().any(|a| a == "--font");
    let supported = if requested {
        match session.register(&mut io::stdout(), &mut input) {
            Ok(v) => v,
            Err(e) => {
                let _ = session.clear(&mut io::stdout(), &mut input);
                return Err(e);
            }
        }
    } else {
        false
    };
    if requested && !supported {
        session.clear(&mut io::stdout(), &mut input)?;
    }
    if !requested {
        execute!(
            io::stdout(),
            ratatui::crossterm::event::EnableMouseCapture,
            ratatui::crossterm::event::EnableFocusChange
        )?;
    }
    let material = if supported || font_mode {
        Material::Glyph
    } else {
        Material::Text
    };
    let registration = startup.elapsed();
    let mut terminal = Terminal::new(CrosstermBackend::new(io::stdout()))?;
    let mut selected = 0usize;
    let mut latched = false;
    let mut level = 0.64f64;
    let mut animate = true;
    let result = (|| -> io::Result<()> {
        loop {
            let t = startup.elapsed().as_secs_f64();
            terminal.draw(|f| {
                let a = f.area();
                let b = f.buffer_mut();
                b.set_style(a, Style::default().bg(PLATE).fg(INK));
                if a.width < 72 || a.height < 24 {
                    Paragraph::new("PANEL SPECIMEN: resize to at least 72 x 24. Q exits.")
                        .render(a, b);
                    return;
                }
                let title = format!(
                    " SHR / INSTRUMENT PANEL     SPECIMEN DATA / NO PROVIDER     {}",
                    if font_mode {
                        "LOCAL FONT COLRv0"
                    } else if supported {
                        "PROTOCOL COLRv0"
                    } else {
                        "READABLE FALLBACK"
                    }
                );
                b.set_stringn(
                    1,
                    0,
                    title,
                    (a.width - 2) as usize,
                    Style::default().bg(WELL).fg(INK),
                );
                b.set_stringn(
                    2,
                    2,
                    "CONTROL STATES   /   selection is not owner confirmation",
                    (a.width - 4) as usize,
                    Style::default().fg(INK),
                );
                let states = [
                    ControlState::Raised,
                    ControlState::Pressed,
                    ControlState::Focused,
                    ControlState::Disabled,
                    ControlState::Latched,
                    ControlState::Warning,
                    ControlState::Unavailable,
                ];
                let labels = [
                    "READY", "PRESSED", "FOCUS", "DISABLED", "LATCHED", "WARNING", "NO DATA",
                ];
                let width = (a.width - 4) / 7;
                for i in 0..7 {
                    Button {
                        label: labels[i],
                        state: states[i],
                        material,
                    }
                    .render(Rect::new(2 + i as u16 * width, 3, width - 1, 3), b);
                }
                for (i, c) in ["green", "yellow", "red", "blue"].iter().enumerate() {
                    let x = 3 + i as u16 * ((a.width - 6) / 4);
                    Led {
                        colour: c,
                        on: true,
                        label: c,
                        material,
                    }
                    .render(Rect::new(x, 7, 18, 1), b);
                    Led {
                        colour: c,
                        on: false,
                        label: c,
                        material,
                    }
                    .render(Rect::new(x, 8, 18, 1), b);
                }
                b.set_string(
                    2,
                    10,
                    " RECESSED DISPLAY / SYNTHETIC CALIBRATION",
                    Style::default().fg(INK).bg(WELL),
                );
                b.set_string(
                    3,
                    11,
                    format!(
                        "LEVEL {:5.1} %   /   SAMPLE PEAK SCALE -60 .. 0 dBFS",
                        level * 100.0
                    ),
                    Style::default().fg(Color::Rgb(157, 231, 190)).bg(WELL),
                );
                bar(
                    b,
                    Rect::new(3, 12, a.width - 8, 1),
                    Some(level),
                    Color::Rgb(73, 221, 132),
                );
                for (i, label) in ["-60", "-40", "-20", "0"].iter().enumerate() {
                    let x = 3 + (a.width - 9) * i as u16 / 3;
                    b.set_string(x, 13, *label, Style::default().fg(INK).bg(WELL));
                }
                let cols = ((a.width - 26) / 2).min(64);
                let graph_height = a.height.saturating_sub(23);
                for i in 0..cols {
                    let v = if animate {
                        ((t * 2.0 + i as f64 * 0.41).sin() * 0.35 + 0.5).clamp(0.0, 1.0)
                    } else {
                        (i % 8) as f64 / 8.0
                    };
                    for y in 0..graph_height {
                        let n = (v * f64::from(graph_height) * 8.0
                            - f64::from(graph_height - 1 - y) * 8.0)
                            .round()
                            .clamp(0.0, 8.0) as usize;
                        let c = if material == Material::Glyph {
                            tile(&format!("red_level_{n}"))
                        } else {
                            [' ', '▁', '▂', '▃', '▄', '▅', '▆', '▇', '█'][n]
                        };
                        b[(3 + i * 2, 15 + y)]
                            .set_char(c)
                            .set_style(Style::default().fg(Color::Rgb(233, 62, 69)).bg(WELL));
                    }
                }
                b.set_string(
                    3,
                    a.height - 8,
                    "SPECTRUM TILE EXERCISE / synthetic bins, no Hz claim",
                    Style::default().fg(INK),
                );
                let x = a.width - 15;
                for y in 15..15 + graph_height {
                    b[(x, y)].set_char(if material == Material::Glyph {
                        tile(
                            if y == 14 + graph_height
                                - (level * f64::from(graph_height - 1)).round() as u16
                            {
                                "fader_cap"
                            } else {
                                "fader_track"
                            },
                        )
                    } else if y
                        == 14 + graph_height - (level * f64::from(graph_height - 1)).round() as u16
                    {
                        '█'
                    } else {
                        '│'
                    });
                }
                b.set_string(
                    x + 2,
                    17,
                    format!("{:3.0}%", level * 100.0),
                    Style::default().fg(INK),
                );
                if a.height >= 29 {
                    for i in 0..3 {
                        Button {
                            label: ["HOLD", "APPLY", "CANCEL"][i],
                            state: if i == selected {
                                ControlState::Focused
                            } else if i == 0 && latched {
                                ControlState::Latched
                            } else {
                                ControlState::Raised
                            },
                            material,
                        }
                        .render(Rect::new(3 + i as u16 * 19, a.height - 6, 18, 3), b);
                    }
                }
                b.set_stringn(
                    2,
                    a.height - 2,
                    format!(
                        "TAB focus | SPACE latch | +/- fader | A animate | Q exit  reg {:.1}ms",
                        registration.as_secs_f64() * 1000.0
                    ),
                    (a.width - 4) as usize,
                    Style::default().fg(INK).bg(WELL),
                );
            })?;
            if requested {
                input.read(40)?;
            } else {
                use ratatui::crossterm::event::{
                    self, Event, KeyCode, KeyEventKind, KeyModifiers, MouseEventKind,
                };
                if event::poll(std::time::Duration::from_millis(40))? {
                    match event::read()? {
                        Event::Key(k) if k.kind == KeyEventKind::Press => match k.code {
                            KeyCode::Char('c') if k.modifiers.contains(KeyModifiers::CONTROL) => {
                                input.keys.push_back(3)
                            }
                            KeyCode::Char(c) if c.is_ascii() => input.keys.push_back(c as u8),
                            KeyCode::Tab => input.keys.push_back(b'\t'),
                            KeyCode::Enter => input.keys.push_back(b' '),
                            KeyCode::Up => input.keys.push_back(b'+'),
                            KeyCode::Down => input.keys.push_back(b'-'),
                            _ => {}
                        },
                        Event::Mouse(m)
                            if matches!(
                                m.kind,
                                MouseEventKind::Down(_) | MouseEventKind::Drag(_)
                            ) =>
                        {
                            let size = terminal.size()?;
                            if size.width >= 72 && size.height >= 29 {
                                if m.row >= size.height - 6
                                    && m.row < size.height - 3
                                    && m.column >= 3
                                    && m.column < 60
                                {
                                    selected = ((m.column - 3) / 19).min(2) as usize;
                                    latched = !latched;
                                }
                                if m.column >= size.width - 17
                                    && m.row >= 15
                                    && m.row < size.height - 8
                                {
                                    level =
                                        1.0 - f64::from(m.row - 15) / f64::from(size.height - 24);
                                }
                            }
                        }
                        Event::FocusLost => animate = false,
                        _ => {}
                    }
                }
            }
            while let Some(k) = input.keys.pop_front() {
                match k {
                    b'q' | 3 => return Ok(()),
                    b'\t' => selected = (selected + 1) % 3,
                    b' ' => latched = !latched,
                    b'+' | b'=' => level = (level + 0.02).min(1.0),
                    b'-' => level = (level - 0.02).max(0.0),
                    b'a' => animate = !animate,
                    _ => {}
                }
            }
        }
    })();
    let cleanup = session.clear(&mut io::stdout(), &mut input);
    io::stdout().flush()?;
    result.and(cleanup)
}
