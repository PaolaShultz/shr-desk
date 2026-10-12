//! Terminal lifecycle shared by both provider clients. No owner commands here.
pub use ratatui;
use ratatui::{
    Terminal,
    backend::CrosstermBackend,
    crossterm::{
        cursor::{Hide, Show},
        event::{
            DisableFocusChange, DisableMouseCapture, EnableFocusChange, EnableMouseCapture,
            KeyboardEnhancementFlags, PopKeyboardEnhancementFlags, PushKeyboardEnhancementFlags,
        },
        execute,
        terminal::{
            EnterAlternateScreen, LeaveAlternateScreen, disable_raw_mode, enable_raw_mode,
            supports_keyboard_enhancement,
        },
    },
};
use std::io::{self, IsTerminal};
pub struct Console {
    pub terminal: Terminal<CrosstermBackend<io::Stdout>>,
    pub release_events: bool,
    pub material: crate::Material,
}
impl Console {
    pub fn open() -> io::Result<Self> {
        if !io::stdin().is_terminal() || !io::stdout().is_terminal() {
            return Err(io::Error::other("interactive terminal required"));
        }
        enable_raw_mode()?;
        let mut pushed_keys = false;
        let result = (|| {
            let mut out = io::stdout();
            execute!(
                out,
                EnterAlternateScreen,
                Hide,
                EnableFocusChange,
                EnableMouseCapture
            )?;
            let release_events = supports_keyboard_enhancement().unwrap_or(false);
            if release_events {
                pushed_keys = true;
                execute!(
                    out,
                    PushKeyboardEnhancementFlags(
                        KeyboardEnhancementFlags::DISAMBIGUATE_ESCAPE_CODES
                            | KeyboardEnhancementFlags::REPORT_EVENT_TYPES
                            | KeyboardEnhancementFlags::REPORT_ALL_KEYS_AS_ESCAPE_CODES
                    )
                )?;
            }
            let terminal = Terminal::new(CrosstermBackend::new(out))?;
            // Explicit launch contract. Never infer installed colour font support from TERM.
            let material = if std::env::var("SHR_PANEL_FONT").as_deref() == Ok("SHR Panel Mono:0.3")
            {
                crate::Material::Glyph
            } else {
                crate::Material::Text
            };
            Ok(Self {
                terminal,
                release_events,
                material,
            })
        })();
        if result.is_err() {
            if pushed_keys {
                let _ = execute!(io::stdout(), PopKeyboardEnhancementFlags);
            }
            let _ = execute!(
                io::stdout(),
                DisableMouseCapture,
                DisableFocusChange,
                Show,
                LeaveAlternateScreen
            );
            let _ = disable_raw_mode();
        }
        result
    }
}
impl Drop for Console {
    fn drop(&mut self) {
        if self.release_events {
            let _ = execute!(io::stdout(), PopKeyboardEnhancementFlags);
        }
        let _ = execute!(
            io::stdout(),
            DisableMouseCapture,
            DisableFocusChange,
            Show,
            LeaveAlternateScreen
        );
        let _ = disable_raw_mode();
    }
}
