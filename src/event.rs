use std::{io, time::Duration};

use crossterm::event::{self, Event, KeyCode, KeyEventKind, KeyModifiers};

use crate::app::{Action, Mode, Step};

pub fn read_action(timeout: Duration, mode: Mode, active_step: Step) -> io::Result<Option<Action>> {
    if !event::poll(timeout)? {
        return Ok(None);
    }

    let action = match event::read()? {
        Event::Paste(value) => action_from_paste(value, mode),
        Event::Key(key) if key.kind == KeyEventKind::Press => {
            action_from_key(key.code, key.modifiers, mode, active_step)
        }
        _ => None,
    };

    Ok(action)
}

fn action_from_key(
    code: KeyCode,
    modifiers: KeyModifiers,
    mode: Mode,
    active_step: Step,
) -> Option<Action> {
    if code == KeyCode::Char('c') && modifiers.contains(KeyModifiers::CONTROL) {
        return Some(Action::Quit);
    }

    if code == KeyCode::Esc {
        return Some(Action::SetMode(Mode::Normal));
    }

    match mode {
        Mode::Normal => action_from_normal_mode_key(code, active_step),
        Mode::Insert => action_from_insert_mode_key(code, active_step),
        Mode::Visual => None,
    }
}

fn action_from_normal_mode_key(code: KeyCode, active_step: Step) -> Option<Action> {
    match code {
        KeyCode::Char('q') => Some(Action::Quit),
        KeyCode::Char('j') => Some(Action::MoveStepNext),
        KeyCode::Char('k') => Some(Action::MoveStepPrevious),
        KeyCode::Char('i') => Some(Action::SetMode(Mode::Insert)),
        KeyCode::Char('o') if active_step == Step::BrowserApproval => Some(Action::OpenBrowser),
        KeyCode::Char('y') if active_step == Step::BrowserApproval => {
            Some(Action::CopyAuthorizationUrl)
        }
        KeyCode::Char('r') if active_step == Step::BrowserApproval => {
            Some(Action::RestartAuthorization)
        }
        KeyCode::Enter => action_from_enter(active_step),
        _ => None,
    }
}

fn action_from_insert_mode_key(code: KeyCode, active_step: Step) -> Option<Action> {
    match code {
        KeyCode::Enter => action_from_enter(active_step),
        KeyCode::Backspace => Some(Action::Backspace),
        KeyCode::Char(character) => Some(Action::Insert(character)),
        _ => None,
    }
}

fn action_from_enter(active_step: Step) -> Option<Action> {
    match active_step {
        Step::NameDevice | Step::BrowserApproval => Some(Action::ContinueStep),
        Step::Finish => Some(Action::SubmitCallback),
    }
}

fn action_from_paste(value: String, mode: Mode) -> Option<Action> {
    if mode == Mode::Insert {
        Some(Action::Paste(value))
    } else {
        None
    }
}

#[cfg(test)]
mod tests {
    use crossterm::event::{KeyCode, KeyModifiers};

    use super::{action_from_key, action_from_paste};
    use crate::app::{Action, Mode, Step};

    #[test]
    fn normal_mode_routes_navigation_and_mode_keys() {
        assert_eq!(
            action_from_key(
                KeyCode::Char('j'),
                KeyModifiers::NONE,
                Mode::Normal,
                Step::NameDevice
            ),
            Some(Action::MoveStepNext)
        );
        assert_eq!(
            action_from_key(
                KeyCode::Char('k'),
                KeyModifiers::NONE,
                Mode::Normal,
                Step::BrowserApproval
            ),
            Some(Action::MoveStepPrevious)
        );
        assert_eq!(
            action_from_key(
                KeyCode::Char('i'),
                KeyModifiers::NONE,
                Mode::Normal,
                Step::BrowserApproval
            ),
            Some(Action::SetMode(Mode::Insert))
        );
    }

    #[test]
    fn esc_always_returns_to_normal_mode() {
        assert_eq!(
            action_from_key(
                KeyCode::Esc,
                KeyModifiers::NONE,
                Mode::Insert,
                Step::NameDevice
            ),
            Some(Action::SetMode(Mode::Normal))
        );
        assert_eq!(
            action_from_key(
                KeyCode::Esc,
                KeyModifiers::NONE,
                Mode::Visual,
                Step::BrowserApproval
            ),
            Some(Action::SetMode(Mode::Normal))
        );
    }

    #[test]
    fn insert_mode_routes_text_input_and_backspace() {
        assert_eq!(
            action_from_key(
                KeyCode::Char('j'),
                KeyModifiers::NONE,
                Mode::Insert,
                Step::NameDevice
            ),
            Some(Action::Insert('j'))
        );
        assert_eq!(
            action_from_key(
                KeyCode::Backspace,
                KeyModifiers::NONE,
                Mode::Insert,
                Step::BrowserApproval
            ),
            Some(Action::Backspace)
        );
    }

    #[test]
    fn enter_behavior_changes_by_step() {
        assert_eq!(
            action_from_key(
                KeyCode::Enter,
                KeyModifiers::NONE,
                Mode::Insert,
                Step::NameDevice
            ),
            Some(Action::ContinueStep)
        );
        assert_eq!(
            action_from_key(
                KeyCode::Enter,
                KeyModifiers::NONE,
                Mode::Normal,
                Step::BrowserApproval
            ),
            Some(Action::ContinueStep)
        );
        assert_eq!(
            action_from_key(
                KeyCode::Enter,
                KeyModifiers::NONE,
                Mode::Normal,
                Step::Finish
            ),
            Some(Action::SubmitCallback)
        );
    }

    #[test]
    fn paste_only_routes_in_insert_mode() {
        assert_eq!(
            action_from_paste("devnull://auth/callback".to_owned(), Mode::Insert),
            Some(Action::Paste("devnull://auth/callback".to_owned()))
        );
        assert_eq!(
            action_from_paste("devnull://auth/callback".to_owned(), Mode::Normal),
            None
        );
    }
}
