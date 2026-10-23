pub use crate::api::ReportReason;

const MAX_CALLBACK_INPUT: usize = 2048;
const MAX_COMMIT_BODY: usize = 4000;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ComposeKind {
    Reply,
    Quote,
}

impl ReportReason {
    pub const ALL: [Self; 4] = [Self::Spam, Self::Harassment, Self::Unsafe, Self::Other];

    pub fn label(self) -> &'static str {
        match self {
            Self::Spam => "spam",
            Self::Harassment => "harassment",
            Self::Unsafe => "unsafe",
            Self::Other => "other",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum LogInteraction {
    Detail {
        target: String,
        handle: String,
        body: String,
        scroll: u16,
    },
    Compose {
        kind: ComposeKind,
        target: String,
        handle: String,
        body: String,
        error: Option<String>,
    },
    Report {
        target: String,
        handle: String,
        reason: Option<ReportReason>,
        confirming: bool,
        error: Option<String>,
    },
    PastelConfirm {
        target: String,
        handle: String,
        error: Option<String>,
    },
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum LogIntent {
    Commit {
        kind: ComposeKind,
        target: String,
        body: String,
    },
    Report {
        target: String,
        reason: ReportReason,
    },
    Pastel {
        target: String,
    },
}

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
    log_interaction: Option<LogInteraction>,
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
            log_interaction: None,
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
    SelectStep(Step),
    NavigationKey(char),
    // Main resolves shortcuts only on Log with a focused commit; the key stays intact
    // so it can take precedence as the second key of a navigation sequence.
    LogShortcut(char),
    LogEnter,
    RefreshLog,
    OpenConfigEditor,
    LogWheel { column: u16, row: u16, up: bool },
    LogClick { column: u16, row: u16 },
    CancelNavigationPrefix,
    SetMode(Mode),
    Backspace,
    Insert(char),
    Paste(String),
}

impl App {
    pub fn update(&mut self, action: Action) {
        match action {
            Action::Quit => self.should_quit = true,
            Action::LogShortcut('q') if self.log_interaction.is_none() => self.should_quit = true,
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
            Action::SelectStep(step) => {
                self.active_step = step;
                self.mode = Mode::Visual;
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
            | Action::NavigationKey(_)
            | Action::LogShortcut(_)
            | Action::LogEnter
            | Action::RefreshLog
            | Action::OpenConfigEditor
            | Action::LogWheel { .. }
            | Action::LogClick { .. }
            | Action::CancelNavigationPrefix
            | Action::Backspace
            | Action::Insert(_)
            | Action::Paste(_) => {}
        }
    }

    pub fn begin_detail(&mut self, target: &str, handle: &str, body: &str) {
        self.log_interaction = Some(LogInteraction::Detail {
            target: target.to_owned(),
            handle: handle.to_owned(),
            body: body.to_owned(),
            scroll: 0,
        });
    }

    pub fn begin_compose(&mut self, kind: ComposeKind, target: &str, handle: &str) {
        self.log_interaction = Some(LogInteraction::Compose {
            kind,
            target: target.to_owned(),
            handle: handle.to_owned(),
            body: String::new(),
            error: None,
        });
    }

    pub fn begin_report(&mut self, target: &str, handle: &str) {
        self.log_interaction = Some(LogInteraction::Report {
            target: target.to_owned(),
            handle: handle.to_owned(),
            reason: None,
            confirming: false,
            error: None,
        });
    }

    pub fn begin_pastel(&mut self, target: &str, handle: &str) {
        self.log_interaction = Some(LogInteraction::PastelConfirm {
            target: target.to_owned(),
            handle: handle.to_owned(),
            error: None,
        });
    }

    pub fn log_interaction(&self) -> Option<&LogInteraction> {
        self.log_interaction.as_ref()
    }

    /// The caller performs the POST only for a returned intent. Keep the draft on failure;
    /// call finish_log_interaction only after a successful response.
    pub fn update_log(&mut self, action: Action) -> Option<LogIntent> {
        if matches!(action, Action::SetMode(Mode::Normal)) {
            self.log_interaction = None;
            return None;
        }
        if let Some(LogInteraction::Detail {
            target,
            handle,
            scroll,
            ..
        }) = self.log_interaction.as_mut()
        {
            let (target, handle) = (target.clone(), handle.clone());
            match action {
                Action::NavigationKey('j') => *scroll = scroll.saturating_add(1),
                Action::NavigationKey('k') => *scroll = scroll.saturating_sub(1),
                Action::LogShortcut('r') | Action::NavigationKey('r') => {
                    self.begin_compose(ComposeKind::Reply, &target, &handle);
                }
                Action::LogShortcut('q') | Action::NavigationKey('q') => {
                    self.begin_compose(ComposeKind::Quote, &target, &handle);
                }
                Action::LogShortcut('R') | Action::NavigationKey('R') => {
                    self.begin_report(&target, &handle);
                }
                _ => {}
            }
            return None;
        }
        let interaction = self.log_interaction.as_mut()?;
        match interaction {
            LogInteraction::Compose {
                kind,
                target,
                body,
                error,
                ..
            } => match action {
                Action::LogEnter => {
                    if body.trim().is_empty() {
                        *error = Some("Write a message before submitting.".to_owned());
                        None
                    } else {
                        Some(LogIntent::Commit {
                            kind: *kind,
                            target: target.clone(),
                            body: body.clone(),
                        })
                    }
                }
                Action::Backspace => {
                    body.pop();
                    *error = None;
                    None
                }
                Action::Paste(text) => {
                    extend_commit_body(body, &text);
                    *error = None;
                    None
                }
                Action::NavigationKey(key) | Action::LogShortcut(key) | Action::Insert(key) => {
                    extend_commit_body(body, &key.to_string());
                    *error = None;
                    None
                }
                _ => None,
            },
            LogInteraction::Report {
                target,
                reason,
                confirming,
                error,
                ..
            } => match action {
                Action::LogEnter => {
                    if let Some(selected) = *reason {
                        if *confirming {
                            Some(LogIntent::Report {
                                target: target.clone(),
                                reason: selected,
                            })
                        } else {
                            *confirming = true;
                            None
                        }
                    } else {
                        *error = Some("Select a reason (1–4) first.".to_owned());
                        None
                    }
                }
                Action::NavigationKey(key) | Action::LogShortcut(key) | Action::Insert(key) => {
                    if let Some(selected) = key
                        .to_digit(10)
                        .and_then(|digit| digit.checked_sub(1))
                        .and_then(|index| ReportReason::ALL.get(index as usize))
                        .copied()
                    {
                        *reason = Some(selected);
                        *confirming = false;
                        *error = None;
                    }
                    None
                }
                _ => None,
            },
            LogInteraction::PastelConfirm { target, .. } => match action {
                Action::LogEnter => Some(LogIntent::Pastel {
                    target: target.clone(),
                }),
                _ => None,
            },
            LogInteraction::Detail { .. } => None,
        }
    }

    pub fn finish_log_interaction(&mut self) {
        self.log_interaction = None;
    }

    pub fn log_error(&mut self, message: impl Into<String>) {
        match self.log_interaction.as_mut() {
            Some(
                LogInteraction::Compose { error, .. }
                | LogInteraction::Report { error, .. }
                | LogInteraction::PastelConfirm { error, .. },
            ) => *error = Some(message.into()),
            Some(LogInteraction::Detail { .. }) | None => {}
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

fn extend_commit_body(body: &mut String, value: &str) {
    let remaining = MAX_COMMIT_BODY.saturating_sub(body.chars().count());
    body.extend(
        value
            .chars()
            .filter_map(|character| match character {
                '\n' | '\r' | '\t' => Some(' '),
                character if character.is_control() => None,
                character => Some(character),
            })
            .take(remaining),
    );
}

#[cfg(test)]
mod tests {
    use super::{
        Action, App, ComposeKind, EnterAction, LogIntent, LogInteraction, Mode, ReportReason, Step,
    };

    #[test]
    fn reply_and_quote_require_explicit_nonblank_submission_and_keep_draft_on_failure() {
        let mut app = App::default();
        app.begin_compose(ComposeKind::Reply, "uuid", "ada");
        assert_eq!(app.update_log(Action::LogEnter), None);
        app.update_log(Action::Paste("  Hi\n🌍".to_owned()));
        app.update_log(Action::Backspace);
        app.update_log(Action::NavigationKey('!'));
        assert_eq!(
            app.update_log(Action::LogEnter),
            Some(LogIntent::Commit {
                kind: ComposeKind::Reply,
                target: "uuid".to_owned(),
                body: "  Hi !".to_owned(),
            })
        );
        app.log_error("offline");
        assert!(
            matches!(app.log_interaction(), Some(LogInteraction::Compose { error: Some(error), .. }) if error == "offline")
        );
        app.update_log(Action::SetMode(Mode::Normal));
        assert!(app.log_interaction().is_none());
        app.begin_compose(ComposeKind::Quote, "other", "lin");
        app.update(Action::LogShortcut('q'));
        assert!(!app.should_quit());
        app.update_log(Action::LogShortcut('q'));
        assert_eq!(
            app.update_log(Action::LogEnter),
            Some(LogIntent::Commit {
                kind: ComposeKind::Quote,
                target: "other".to_owned(),
                body: "q".to_owned(),
            })
        );
        assert!(!app.should_quit());
    }

    #[test]
    fn compose_enforces_body_limit_and_sanitizes_paste() {
        let mut app = App::default();
        app.begin_compose(ComposeKind::Reply, "uuid", "ada");
        app.update_log(Action::Paste(format!("{}\n\t\0", "x".repeat(3999))));
        assert_eq!(
            app.update_log(Action::LogEnter),
            Some(LogIntent::Commit {
                kind: ComposeKind::Reply,
                target: "uuid".to_owned(),
                body: format!("{} ", "x".repeat(3999)),
            })
        );
    }

    #[test]
    fn focused_detail_routes_keyboard_to_reply_quote_report_and_scroll() {
        let mut app = App::default();
        app.begin_detail("uuid", "ada", "long body");
        app.update_log(Action::NavigationKey('j'));
        assert!(matches!(
            app.log_interaction(),
            Some(LogInteraction::Detail { scroll: 1, .. })
        ));
        app.update_log(Action::LogShortcut('r'));
        assert!(matches!(
            app.log_interaction(),
            Some(LogInteraction::Compose {
                kind: ComposeKind::Reply,
                ..
            })
        ));
        app.update_log(Action::SetMode(Mode::Normal));
        app.begin_detail("uuid", "ada", "long body");
        app.update_log(Action::LogShortcut('q'));
        assert!(matches!(
            app.log_interaction(),
            Some(LogInteraction::Compose {
                kind: ComposeKind::Quote,
                ..
            })
        ));
        app.update_log(Action::SetMode(Mode::Normal));
        app.begin_detail("uuid", "ada", "long body");
        app.update_log(Action::LogShortcut('R'));
        assert!(matches!(
            app.log_interaction(),
            Some(LogInteraction::Report { .. })
        ));
    }

    #[test]
    fn pastel_requires_explicit_confirmation_and_cancel_sends_nothing() {
        let mut app = App::default();
        app.begin_pastel("uuid", "ada");
        assert!(matches!(
            app.log_interaction(),
            Some(LogInteraction::PastelConfirm { .. })
        ));
        assert_eq!(app.update_log(Action::LogShortcut('p')), None);
        assert!(matches!(
            app.log_interaction(),
            Some(LogInteraction::PastelConfirm { .. })
        ));
        assert_eq!(
            app.update_log(Action::LogEnter),
            Some(LogIntent::Pastel {
                target: "uuid".to_owned()
            })
        );
        app.log_error("quota exhausted");
        assert!(
            matches!(app.log_interaction(), Some(LogInteraction::PastelConfirm { error: Some(error), .. }) if error == "quota exhausted")
        );
        app.update_log(Action::SetMode(Mode::Normal));
        assert!(app.log_interaction().is_none());
    }

    #[test]
    fn report_requires_reason_then_separate_confirmation_before_post() {
        let mut app = App::default();
        app.begin_report("uuid", "ada");
        assert_eq!(app.update_log(Action::LogEnter), None);
        app.update_log(Action::NavigationKey('2'));
        assert_eq!(app.update_log(Action::LogEnter), None);
        assert!(matches!(
            app.log_interaction(),
            Some(LogInteraction::Report {
                reason: Some(ReportReason::Harassment),
                confirming: true,
                ..
            })
        ));
        app.update_log(Action::NavigationKey('3'));
        assert!(matches!(
            app.log_interaction(),
            Some(LogInteraction::Report {
                confirming: false,
                ..
            })
        ));
        assert_eq!(app.update_log(Action::LogEnter), None);
        assert_eq!(
            app.update_log(Action::LogEnter),
            Some(LogIntent::Report {
                target: "uuid".to_owned(),
                reason: ReportReason::Unsafe,
            })
        );
        app.finish_log_interaction();
        assert!(app.log_interaction().is_none());
    }

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
    fn log_shortcut_q_quits_only_when_not_consumed_by_main() {
        let mut app = App::default();
        app.update(Action::LogShortcut('s'));
        app.update(Action::LogWheel {
            column: 4,
            row: 2,
            up: true,
        });
        assert!(!app.should_quit());
        app.update(Action::LogShortcut('q'));
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
    fn mouse_step_selection_sets_active_step_and_enters_visual_mode() {
        let mut app = App::default();

        app.update(Action::SelectStep(Step::Finish));

        assert_eq!(app.active_step(), Step::Finish);
        assert_eq!(app.mode(), Mode::Visual);
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
