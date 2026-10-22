use std::{io, time::Duration};

use crossterm::{
    event::{
        self, Event, KeyCode, KeyEventKind, KeyModifiers, MouseButton, MouseEvent, MouseEventKind,
    },
    terminal,
};

use crate::{
    app::{Action, Mode, Step},
    ui,
};

pub fn read_action(timeout: Duration, mode: Mode, active_step: Step) -> io::Result<Option<Action>> {
    if !event::poll(timeout)? {
        return Ok(None);
    }

    match event::read()? {
        Event::Paste(value) => Ok(action_from_paste(value, mode)),
        Event::Key(key) if key.kind == KeyEventKind::Press => {
            Ok(action_from_key(key.code, key.modifiers, mode, active_step))
        }
        Event::Mouse(mouse) => {
            let (width, height) = terminal::size()?;
            Ok(action_from_mouse(mouse, width, height))
        }
        _ => Ok(None),
    }
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

fn action_from_mouse(mouse: MouseEvent, width: u16, height: u16) -> Option<Action> {
    if mouse.kind != MouseEventKind::Down(MouseButton::Left) {
        return None;
    }

    ui::step_at_mouse_position(width, height, mouse.column, mouse.row).map(Action::SelectStep)
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
    use crossterm::event::{KeyCode, KeyModifiers, MouseButton, MouseEvent, MouseEventKind};

    use super::{action_from_key, action_from_mouse, action_from_paste};
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
        assert_eq!(
            action_from_key(
                KeyCode::Char('o'),
                KeyModifiers::NONE,
                Mode::Normal,
                Step::BrowserApproval
            ),
            Some(Action::OpenBrowser)
        );
        assert_eq!(
            action_from_key(
                KeyCode::Char('y'),
                KeyModifiers::NONE,
                Mode::Normal,
                Step::BrowserApproval
            ),
            Some(Action::CopyAuthorizationUrl)
        );
        assert_eq!(
            action_from_key(
                KeyCode::Char('y'),
                KeyModifiers::NONE,
                Mode::Normal,
                Step::NameDevice
            ),
            None
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

    #[test]
    fn left_mouse_down_selects_step_from_centered_rows() {
        let mouse = MouseEvent {
            kind: MouseEventKind::Down(MouseButton::Left),
            column: 10,
            row: 8,
            modifiers: KeyModifiers::NONE,
        };

        assert_eq!(
            action_from_mouse(mouse, 80, 24),
            Some(Action::SelectStep(Step::BrowserApproval))
        );
    }

    #[test]
    fn mouse_outside_step_rows_is_ignored() {
        let mouse = MouseEvent {
            kind: MouseEventKind::Down(MouseButton::Left),
            column: 1,
            row: 1,
            modifiers: KeyModifiers::NONE,
        };

        assert_eq!(action_from_mouse(mouse, 80, 24), None);
    }

    #[test]
    fn non_left_click_mouse_event_is_ignored() {
        let mouse = MouseEvent {
            kind: MouseEventKind::Moved,
            column: 10,
            row: 8,
            modifiers: KeyModifiers::NONE,
        };

        assert_eq!(action_from_mouse(mouse, 80, 24), None);
    }
}
