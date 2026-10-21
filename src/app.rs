const MAX_CALLBACK_INPUT: usize = 2048;

#[derive(Default)]
pub struct App {
    should_quit: bool,
    callback_input: String,
    message: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Action {
    Quit,
    OpenBrowser,
    RestartAuthorization,
    SubmitCallback,
    Backspace,
    Insert(char),
    Paste(String),
}

impl App {
    pub fn update(&mut self, action: Action) {
        match action {
            Action::Quit => self.should_quit = true,
            Action::Backspace => {
                self.callback_input.pop();
                self.message = None;
            }
            Action::Insert(character) => {
                if self.callback_input.chars().count() < MAX_CALLBACK_INPUT {
                    self.callback_input.push(character);
                }
                self.message = None;
            }
            Action::Paste(value) => {
                let available =
                    MAX_CALLBACK_INPUT.saturating_sub(self.callback_input.chars().count());
                self.callback_input.extend(value.chars().take(available));
                self.message = None;
            }
            Action::OpenBrowser | Action::RestartAuthorization | Action::SubmitCallback => {}
        }
    }

    pub fn should_quit(&self) -> bool {
        self.should_quit
    }

    pub fn callback_input(&self) -> &str {
        &self.callback_input
    }

    pub fn input_is_empty(&self) -> bool {
        self.callback_input.is_empty()
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
}

#[cfg(test)]
mod tests {
    use super::{Action, App};

    #[test]
    fn quit_action_stops_the_app() {
        let mut app = App::default();
        app.update(Action::Quit);
        assert!(app.should_quit());
    }

    #[test]
    fn paste_and_backspace_edit_the_callback_input() {
        let mut app = App::default();
        app.update(Action::Paste("devnull://auth/callback".to_owned()));
        app.update(Action::Backspace);
        assert_eq!(app.callback_input(), "devnull://auth/callbac");
    }
}
