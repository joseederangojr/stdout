const MAX_CALLBACK_INPUT: usize = 2048;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Step {
    NameDevice,
    BrowserApproval,
    Finish,
}

impl Step {
    fn next(self) -> Self {
        match self {
            Self::NameDevice => Self::BrowserApproval,
            Self::BrowserApproval => Self::Finish,
            Self::Finish => Self::Finish,
        }
    }

    fn previous(self) -> Self {
        match self {
            Self::NameDevice => Self::NameDevice,
            Self::BrowserApproval => Self::NameDevice,
            Self::Finish => Self::BrowserApproval,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Mode {
    Normal,
    Insert,
    #[allow(dead_code)]
    Visual,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum EnterAction {
    Continue,
    SubmitCallback,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ContextualActions {
    pub enter: Option<EnterAction>,
    pub open_browser: bool,
    pub copy_authorization_url: bool,
    pub restart_authorization: bool,
}

pub struct App {
    should_quit: bool,
    active_step: Step,
    mode: Mode,
    name_device_complete: bool,
    browser_approval_complete: bool,
    device_name_input: String,
    device_name_suggestion: String,
    callback_input: String,
    message: Option<String>,
}

impl Default for App {
    fn default() -> Self {
        Self {
            should_quit: false,
            active_step: Step::NameDevice,
            mode: Mode::Insert,
            name_device_complete: false,
            browser_approval_complete: false,
            device_name_input: String::new(),
            device_name_suggestion: String::new(),
            callback_input: String::new(),
            message: None,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Action {
    Quit,
    OpenBrowser,
    CopyAuthorizationUrl,
    RestartAuthorization,
    SubmitCallback,
    ContinueStep,
    MoveStepNext,
    MoveStepPrevious,
    SetMode(Mode),
    Backspace,
    Insert(char),
    Paste(String),
}

impl App {
    pub fn update(&mut self, action: Action) {
        match action {
            Action::Quit => self.should_quit = true,
            Action::ContinueStep => {
                if self.active_step == Step::NameDevice {
                    self.name_device_complete = true;
                }
                self.active_step = self.active_step.next();
                self.mode = match self.active_step {
                    Step::BrowserApproval => Mode::Normal,
                    Step::Finish => Mode::Insert,
                    Step::NameDevice => self.mode,
                };
            }
            Action::MoveStepNext => {
                self.active_step = self.active_step.next();
            }
            Action::MoveStepPrevious => {
                self.active_step = self.active_step.previous();
            }
            Action::SetMode(mode) => self.mode = mode,
            Action::Backspace if self.mode == Mode::Insert => match self.active_step {
                Step::NameDevice => {
                    self.device_name_input.pop();
                    self.message = None;
                }
                Step::BrowserApproval => {}
                Step::Finish => {
                    self.callback_input.pop();
                    self.message = None;
                }
            },
            Action::Insert(character) if self.mode == Mode::Insert => match self.active_step {
                Step::NameDevice => {
                    self.device_name_input.push(character);
                    self.message = None;
                }
                Step::BrowserApproval => {}
                Step::Finish => {
                    if self.callback_input.chars().count() < MAX_CALLBACK_INPUT {
                        self.callback_input.push(character);
                    }
                    self.message = None;
                }
            },
            Action::Paste(value) if self.mode == Mode::Insert => match self.active_step {
                Step::NameDevice => {
                    self.device_name_input.push_str(&value);
                    self.message = None;
                }
                Step::BrowserApproval => {}
                Step::Finish => {
                    let available =
                        MAX_CALLBACK_INPUT.saturating_sub(self.callback_input.chars().count());
                    self.callback_input.extend(value.chars().take(available));
                    self.message = None;
                }
            },
            Action::OpenBrowser
            | Action::CopyAuthorizationUrl
            | Action::RestartAuthorization
            | Action::SubmitCallback
            | Action::Backspace
            | Action::Insert(_)
            | Action::Paste(_) => {}
        }
    }

    pub fn should_quit(&self) -> bool {
        self.should_quit
    }

    pub fn active_step(&self) -> Step {
        self.active_step
    }

    pub fn mode(&self) -> Mode {
        self.mode
    }

    pub fn device_name_input(&self) -> &str {
        &self.device_name_input
    }

    pub fn set_device_name_input(&mut self, value: impl Into<String>) {
        self.device_name_input = value.into();
    }

    pub fn device_name_suggestion(&self) -> &str {
        &self.device_name_suggestion
    }

    pub fn set_device_name_suggestion(&mut self, value: impl Into<String>) {
        self.device_name_suggestion = value.into();
    }

    pub fn callback_input(&self) -> &str {
        &self.callback_input
    }

    pub fn take_callback_input(&mut self) -> String {
        std::mem::take(&mut self.callback_input)
    }

    pub fn set_message(&mut self, message: Option<String>) {
        self.message = message;
    }

    pub fn message(&self) -> Option<&str> {
        self.message.as_deref()
    }

    pub fn is_step_complete(&self, step: Step) -> bool {
        match step {
            Step::NameDevice => self.name_device_complete,
            Step::BrowserApproval => self.browser_approval_complete,
            Step::Finish => false,
        }
    }

    pub fn mark_browser_approval_complete(&mut self) {
        self.browser_approval_complete = true;
    }

    pub fn contextual_actions(&self) -> ContextualActions {
        match self.active_step {
            Step::NameDevice => ContextualActions {
                enter: Some(EnterAction::Continue),
                open_browser: false,
                copy_authorization_url: false,
                restart_authorization: false,
            },
            Step::BrowserApproval => ContextualActions {
                enter: Some(EnterAction::Continue),
                open_browser: true,
                copy_authorization_url: true,
                restart_authorization: true,
            },
            Step::Finish => ContextualActions {
                enter: Some(EnterAction::SubmitCallback),
                open_browser: false,
                copy_authorization_url: false,
                restart_authorization: false,
            },
        }
    }
}

#[cfg(test)]
mod tests {
    use super::{Action, App, EnterAction, Mode, Step};

    #[test]
    fn default_state_starts_on_name_step_in_insert_mode() {
        let app = App::default();

        assert_eq!(app.active_step(), Step::NameDevice);
        assert_eq!(app.mode(), Mode::Insert);
        assert!(app.device_name_input().is_empty());
        assert!(app.callback_input().is_empty());
    }

    #[test]
    fn quit_action_stops_the_app() {
        let mut app = App::default();

        app.update(Action::Quit);

        assert!(app.should_quit());
    }

    #[test]
    fn step_navigation_is_clamped_between_first_and_last_step() {
        let mut app = App::default();

        app.update(Action::SetMode(Mode::Normal));
        app.update(Action::MoveStepPrevious);
        assert_eq!(app.active_step(), Step::NameDevice);

        app.update(Action::MoveStepNext);
        assert_eq!(app.active_step(), Step::BrowserApproval);
        assert_eq!(app.mode(), Mode::Normal);

        app.update(Action::MoveStepNext);
        assert_eq!(app.active_step(), Step::Finish);
        assert_eq!(app.mode(), Mode::Normal);

        app.update(Action::MoveStepNext);
        assert_eq!(app.active_step(), Step::Finish);
    }

    #[test]
    fn continue_step_sets_mode_for_browser_approval_and_finish() {
        let mut app = App::default();

        app.update(Action::ContinueStep);
        assert_eq!(app.active_step(), Step::BrowserApproval);
        assert_eq!(app.mode(), Mode::Normal);

        app.update(Action::ContinueStep);
        assert_eq!(app.active_step(), Step::Finish);
        assert_eq!(app.mode(), Mode::Insert);
    }

    #[test]
    fn name_and_callback_inputs_edit_in_their_respective_steps() {
        let mut app = App::default();

        app.update(Action::Paste("stdout-laptop".to_owned()));
        assert_eq!(app.device_name_input(), "stdout-laptop");
        assert_eq!(app.callback_input(), "");

        app.update(Action::ContinueStep);
        app.update(Action::Paste("ignored".to_owned()));
        app.update(Action::Backspace);
        assert_eq!(app.callback_input(), "");

        app.update(Action::ContinueStep);
        app.update(Action::Paste("devnull://auth/callback".to_owned()));
        app.update(Action::Backspace);
        assert_eq!(app.callback_input(), "devnull://auth/callbac");

        app.update(Action::SetMode(Mode::Normal));
        app.update(Action::Insert('x'));
        assert_eq!(app.callback_input(), "devnull://auth/callbac");
    }

    #[test]
    fn contextual_actions_follow_the_active_step() {
        let mut app = App::default();

        assert_eq!(app.contextual_actions().enter, Some(EnterAction::Continue));
        assert!(!app.contextual_actions().open_browser);
        assert!(!app.contextual_actions().restart_authorization);

        app.update(Action::ContinueStep);

        assert_eq!(app.contextual_actions().enter, Some(EnterAction::Continue));
        assert!(app.contextual_actions().open_browser);
        assert!(app.contextual_actions().copy_authorization_url);
        assert!(app.contextual_actions().restart_authorization);

        app.update(Action::ContinueStep);

        assert_eq!(
            app.contextual_actions().enter,
            Some(EnterAction::SubmitCallback)
        );
        assert!(!app.contextual_actions().open_browser);
        assert!(!app.contextual_actions().copy_authorization_url);
        assert!(!app.contextual_actions().restart_authorization);
    }

    #[test]
    fn navigation_does_not_mark_steps_complete() {
        let mut app = App::default();

        app.update(Action::MoveStepNext);
        app.update(Action::MoveStepNext);

        assert_eq!(app.active_step(), Step::Finish);
        assert!(!app.is_step_complete(Step::NameDevice));
        assert!(!app.is_step_complete(Step::BrowserApproval));
    }

    #[test]
    fn continue_step_only_marks_name_device_complete() {
        let mut app = App::default();

        app.update(Action::ContinueStep);
        app.update(Action::ContinueStep);

        assert!(app.is_step_complete(Step::NameDevice));
        assert!(!app.is_step_complete(Step::BrowserApproval));
    }

    #[test]
    fn browser_approval_completion_is_explicit() {
        let mut app = App::default();

        app.update(Action::ContinueStep);
        assert!(!app.is_step_complete(Step::BrowserApproval));

        app.mark_browser_approval_complete();

        assert!(app.is_step_complete(Step::BrowserApproval));
    }
}
