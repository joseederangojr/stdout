use std::io::{self, Stdout};

use crossterm::{
    cursor::Show,
    event::{DisableBracketedPaste, DisableMouseCapture, EnableBracketedPaste, EnableMouseCapture},
    execute,
    terminal::{self, EnterAlternateScreen, LeaveAlternateScreen},
};
use ratatui::{backend::CrosstermBackend, prelude::Frame, Terminal};

pub struct TerminalSession {
    terminal: Terminal<CrosstermBackend<Stdout>>,
    _guard: TerminalGuard,
}

impl TerminalSession {
    pub fn new() -> io::Result<Self> {
        let guard = TerminalGuard::enter()?;
        let mut terminal = Terminal::new(CrosstermBackend::new(io::stdout()))?;
        terminal.hide_cursor()?;

        Ok(Self {
            terminal,
            _guard: guard,
        })
    }

    pub fn draw(&mut self, render: impl FnOnce(&mut Frame<'_>)) -> io::Result<()> {
        self.terminal.draw(render).map(|_| ())
    }
}

struct TerminalGuard {
    raw_mode_enabled: bool,
    alternate_screen_enabled: bool,
    bracketed_paste_enabled: bool,
    mouse_capture_enabled: bool,
}

impl TerminalGuard {
    fn enter() -> io::Result<Self> {
        terminal::enable_raw_mode()?;
        let mut guard = Self {
            raw_mode_enabled: true,
            alternate_screen_enabled: false,
            bracketed_paste_enabled: false,
            mouse_capture_enabled: false,
        };
        execute!(io::stdout(), EnterAlternateScreen)?;
        guard.alternate_screen_enabled = true;
        execute!(io::stdout(), EnableBracketedPaste)?;
        guard.bracketed_paste_enabled = true;
        execute!(io::stdout(), EnableMouseCapture)?;
        guard.mouse_capture_enabled = true;
        Ok(guard)
    }
}

impl Drop for TerminalGuard {
    fn drop(&mut self) {
        if self.raw_mode_enabled {
            let _ = terminal::disable_raw_mode();
        }
        if self.mouse_capture_enabled {
            let _ = execute!(io::stdout(), DisableMouseCapture);
        }
        if self.bracketed_paste_enabled {
            let _ = execute!(io::stdout(), DisableBracketedPaste);
        }
        if self.alternate_screen_enabled {
            let _ = execute!(io::stdout(), LeaveAlternateScreen, Show);
        }
    }
}
