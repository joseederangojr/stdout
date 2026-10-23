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

pub fn read_action(
    timeout: Duration,
    mode: Mode,
    active_step: Step,
    signed_in: bool,
) -> io::Result<Option<Action>> {
    if !event::poll(timeout)? {
        return Ok(None);
    }

    match event::read()? {
        Event::Paste(value) => Ok(action_from_paste(value, mode, signed_in)),
        Event::Key(key) if key.kind == KeyEventKind::Press => {
            let action = if signed_in {
                action_from_signed_in_key(key.code, key.modifiers)
            } else {
                action_from_key(key.code, key.modifiers, mode, active_step)
            };
            Ok(action)
        }
        Event::Mouse(mouse) if signed_in => Ok(action_from_signed_in_mouse(mouse)),
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

    if code == KeyCode::Char('r') && modifiers == KeyModifiers::CONTROL {
        return None;
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

fn action_from_signed_in_key(code: KeyCode, modifiers: KeyModifiers) -> Option<Action> {
    if code == KeyCode::Char('c') && modifiers.contains(KeyModifiers::CONTROL) {
        return Some(Action::Quit);
    }

    if code == KeyCode::Esc {
        return Some(Action::SetMode(Mode::Normal));
    }

    if code == KeyCode::Char(',') && modifiers == KeyModifiers::CONTROL {
        return Some(Action::OpenConfigEditor);
    }

    if matches!(code, KeyCode::Char('r') | KeyCode::Char('R')) && modifiers == KeyModifiers::CONTROL
    {
        return Some(Action::RefreshLog);
    }

    match code {
        KeyCode::Char(character)
            if modifiers == KeyModifiers::NONE || modifiers == KeyModifiers::SHIFT =>
        {
            match character {
                's' | 'p' | 'r' | 'q' | 'y' | 'R' | 'o' => Some(Action::LogShortcut(character)),
                _ => Some(Action::NavigationKey(character)),
            }
        }
        KeyCode::Enter => Some(Action::LogEnter),
        KeyCode::Backspace => Some(Action::Backspace),
        KeyCode::Up => Some(Action::NavigationKey('k')),
        KeyCode::Down => Some(Action::NavigationKey('j')),
        _ => Some(Action::CancelNavigationPrefix),
    }
}

fn action_from_signed_in_mouse(mouse: MouseEvent) -> Option<Action> {
    match mouse.kind {
        MouseEventKind::ScrollUp | MouseEventKind::ScrollDown => Some(Action::LogWheel {
            column: mouse.column,
            row: mouse.row,
            up: mouse.kind == MouseEventKind::ScrollUp,
        }),
        MouseEventKind::Down(MouseButton::Left) => Some(Action::LogClick {
            column: mouse.column,
            row: mouse.row,
        }),
        _ => None,
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

fn action_from_paste(value: String, mode: Mode, signed_in: bool) -> Option<Action> {
    if signed_in || mode == Mode::Insert {
        Some(Action::Paste(value))
    } else {
        None
    }
}

#[cfg(test)]
mod tests {
    use crossterm::event::{KeyCode, KeyModifiers, MouseButton, MouseEvent, MouseEventKind};

    use super::{
        action_from_key, action_from_mouse, action_from_paste, action_from_signed_in_key,
        action_from_signed_in_mouse,
    };
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
            action_from_paste("devnull://auth/callback".to_owned(), Mode::Insert, false),
            Some(Action::Paste("devnull://auth/callback".to_owned()))
        );
        assert_eq!(
            action_from_paste("devnull://auth/callback".to_owned(), Mode::Normal, false),
            None
        );
    }

    #[test]
    fn signed_in_paste_and_backspace_forward_for_dialog_gating() {
        for mode in [Mode::Normal, Mode::Insert, Mode::Visual] {
            assert_eq!(
                action_from_paste("text\nfrom clipboard".to_owned(), mode, true),
                Some(Action::Paste("text\nfrom clipboard".to_owned()))
            );
        }
        assert_eq!(
            action_from_signed_in_key(KeyCode::Backspace, KeyModifiers::NONE),
            Some(Action::Backspace)
        );
    }

    #[test]
    fn signed_in_keys_without_dialog_remain_shortcuts() {
        assert_eq!(
            action_from_signed_in_key(KeyCode::Char('s'), KeyModifiers::NONE),
            Some(Action::LogShortcut('s'))
        );
        assert_eq!(
            action_from_signed_in_key(KeyCode::Char('g'), KeyModifiers::NONE),
            Some(Action::NavigationKey('g'))
        );
    }

    #[test]
    fn signed_in_key_dispatch_routes_navigation_and_log_shortcuts() {
        for (key, action) in [
            ('g', Action::NavigationKey('g')),
            ('l', Action::NavigationKey('l')),
            ('n', Action::NavigationKey('n')),
            ('S', Action::NavigationKey('S')),
            (',', Action::NavigationKey(',')),
            ('s', Action::LogShortcut('s')),
            ('p', Action::LogShortcut('p')),
            ('r', Action::LogShortcut('r')),
            ('q', Action::LogShortcut('q')),
            ('y', Action::LogShortcut('y')),
            ('R', Action::LogShortcut('R')),
            ('o', Action::LogShortcut('o')),
        ] {
            assert_eq!(
                action_from_signed_in_key(KeyCode::Char(key), KeyModifiers::NONE),
                Some(action)
            );
        }
        assert_eq!(
            action_from_signed_in_key(KeyCode::Char('R'), KeyModifiers::SHIFT),
            Some(Action::LogShortcut('R'))
        );
        assert_eq!(
            action_from_signed_in_key(KeyCode::Enter, KeyModifiers::NONE),
            Some(Action::LogEnter)
        );
        assert_eq!(
            action_from_signed_in_key(KeyCode::Up, KeyModifiers::NONE),
            Some(Action::NavigationKey('k'))
        );
        assert_eq!(
            action_from_signed_in_key(KeyCode::Down, KeyModifiers::NONE),
            Some(Action::NavigationKey('j'))
        );
        assert_eq!(
            action_from_signed_in_key(KeyCode::Esc, KeyModifiers::NONE),
            Some(Action::SetMode(Mode::Normal))
        );
        assert_eq!(
            action_from_signed_in_key(KeyCode::Char('c'), KeyModifiers::CONTROL),
            Some(Action::Quit)
        );
        assert_eq!(
            action_from_signed_in_key(KeyCode::Char('r'), KeyModifiers::CONTROL),
            Some(Action::RefreshLog)
        );
        assert_eq!(
            action_from_signed_in_key(KeyCode::Char(','), KeyModifiers::CONTROL),
            Some(Action::OpenConfigEditor)
        );
        assert_eq!(
            action_from_key(
                KeyCode::Char('c'),
                KeyModifiers::CONTROL,
                Mode::Insert,
                Step::NameDevice
            ),
            Some(Action::Quit)
        );
    }

    #[test]
    fn refresh_matches_only_signed_in_control_r() {
        for code in [KeyCode::Char('r'), KeyCode::Char('R')] {
            assert_eq!(
                action_from_signed_in_key(code, KeyModifiers::CONTROL),
                Some(Action::RefreshLog)
            );
            for modifiers in [
                KeyModifiers::ALT,
                KeyModifiers::CONTROL | KeyModifiers::SHIFT,
                KeyModifiers::CONTROL | KeyModifiers::ALT,
            ] {
                assert_ne!(
                    action_from_signed_in_key(code, modifiers),
                    Some(Action::RefreshLog)
                );
            }
        }
        for mode in [Mode::Normal, Mode::Insert, Mode::Visual] {
            assert_eq!(
                action_from_key(
                    KeyCode::Char('r'),
                    KeyModifiers::CONTROL,
                    mode,
                    Step::BrowserApproval,
                ),
                None
            );
        }
    }

    #[test]
    fn signed_in_mouse_dispatch_preserves_wheel_and_routes_left_down_only() {
        let mouse = |kind| MouseEvent {
            kind,
            column: 34,
            row: 12,
            modifiers: KeyModifiers::NONE,
        };
        assert_eq!(
            action_from_signed_in_mouse(mouse(MouseEventKind::ScrollUp)),
            Some(Action::LogWheel {
                column: 34,
                row: 12,
                up: true
            })
        );
        assert_eq!(
            action_from_signed_in_mouse(mouse(MouseEventKind::ScrollDown)),
            Some(Action::LogWheel {
                column: 34,
                row: 12,
                up: false
            })
        );
        assert_eq!(
            action_from_signed_in_mouse(mouse(MouseEventKind::Down(MouseButton::Left))),
            Some(Action::LogClick {
                column: 34,
                row: 12
            })
        );
        for kind in [
            MouseEventKind::Moved,
            MouseEventKind::Down(MouseButton::Right),
            MouseEventKind::Up(MouseButton::Left),
            MouseEventKind::Drag(MouseButton::Left),
            MouseEventKind::Drag(MouseButton::Right),
            MouseEventKind::ScrollLeft,
            MouseEventKind::ScrollRight,
        ] {
            assert_eq!(action_from_signed_in_mouse(mouse(kind)), None);
        }
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
