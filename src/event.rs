use std::{io, time::Duration};

use crossterm::event::{self, Event, KeyCode, KeyEventKind, KeyModifiers};

use crate::app::Action;

pub fn read_action(timeout: Duration, input_is_empty: bool) -> io::Result<Option<Action>> {
    if !event::poll(timeout)? {
        return Ok(None);
    }

    let action = match event::read()? {
        Event::Paste(value) => Some(Action::Paste(value)),
        Event::Key(key) if key.kind == KeyEventKind::Press => {
            if key.code == KeyCode::Char('c') && key.modifiers.contains(KeyModifiers::CONTROL) {
                Some(Action::Quit)
            } else {
                match key.code {
                    KeyCode::Esc => Some(Action::Quit),
                    KeyCode::Char('q') if input_is_empty => Some(Action::Quit),
                    KeyCode::Char('o') if input_is_empty => Some(Action::OpenBrowser),
                    KeyCode::Char('r') if input_is_empty => Some(Action::RestartAuthorization),
                    KeyCode::Enter => Some(Action::SubmitCallback),
                    KeyCode::Backspace => Some(Action::Backspace),
                    KeyCode::Char(character) => Some(Action::Insert(character)),
                    _ => None,
                }
            }
        }
        _ => None,
    };

    Ok(action)
}
