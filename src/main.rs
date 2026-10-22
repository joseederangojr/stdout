mod api;
mod app;
mod auth;
mod clipboard;
mod config;
mod device;
mod event;
mod protocol;
mod terminal;
mod ui;

use std::{
    io::{self, IsTerminal},
    path::PathBuf,
    time::Duration,
};

use anyhow::{bail, Result};
use app::{Action, App, EnterAction, Step};
use auth::AuthFlow;
use config::Config;

fn main() {
    if let Err(error) = run() {
        eprintln!("stdout: {error:#}");
        std::process::exit(1);
    }
}

fn run() -> Result<()> {
    let mut arguments: Vec<String> = std::env::args().skip(1).collect();
    if arguments
        .iter()
        .any(|argument| matches!(argument.as_str(), "-h" | "--help"))
    {
        print_help();
        return Ok(());
    }
    let config_dir = take_config_dir(&mut arguments)?;

    if let [command, uri, flag, dir] = arguments.as_slice() {
        if command == "--callback" && flag == "--config-dir" {
            return receive_callback(uri, Config::load_at_dir(PathBuf::from(dir))?);
        }
    }

    match arguments.as_slice() {
        [] => run_tui(false, None, load_config(config_dir)?),
        [command] if command == "--install-protocol" => {
            protocol::install(&load_config(config_dir)?)
        }
        [group, action] if group == "auth" && action == "login" => {
            run_tui(true, None, load_config(config_dir)?)
        }
        [group, action, flag, name]
            if group == "auth" && action == "login" && flag == "--device-name" =>
        {
            run_tui(true, Some(name.clone()), load_config(config_dir)?)
        }
        [group, action] if group == "auth" && action == "logout" => {
            run_auth_logout(load_config(config_dir)?)
        }
        [uri] if uri.starts_with("devnull://") => receive_callback(uri, load_config(config_dir)?),
        [command, uri] if command == "--callback" => {
            receive_callback(uri, load_config(config_dir)?)
        }
        [command, ..] if command == "--callback" => {
            bail!("usage: stdout --callback <uri> [--config-dir <path>]")
        }
        _ => bail!("unknown command; run `stdout --help` for usage"),
    }
}

fn take_config_dir(arguments: &mut Vec<String>) -> Result<Option<PathBuf>> {
    let mut config_dir = None;
    let mut index = 0;
    while index < arguments.len() {
        if arguments[index] != "--config" {
            index += 1;
            continue;
        }
        if config_dir.is_some() {
            bail!("--config may only be specified once");
        }
        let Some(path) = arguments.get(index + 1) else {
            bail!("--config requires a directory path");
        };
        let path = PathBuf::from(path);
        let path = if path.is_absolute() {
            path
        } else {
            std::env::current_dir()?.join(path)
        };
        arguments.drain(index..=index + 1);
        config_dir = Some(path);
    }
    Ok(config_dir)
}

fn load_config(config_dir: Option<PathBuf>) -> Result<Config> {
    match config_dir {
        Some(dir) => Config::load_at_dir(dir),
        None => Config::load(),
    }
}

fn receive_callback(uri: &str, config: Config) -> Result<()> {
    auth::receive_callback_for_running_tui(&config, uri)
}

fn run_tui(start_login: bool, device_name: Option<String>, config: Config) -> Result<()> {
    if !io::stdin().is_terminal() || !io::stdout().is_terminal() {
        bail!("an interactive terminal is required");
    }

    let protocol_warning = protocol::ensure_installed(&config).err().map(|error| {
        format!("Could not register callback handler: {error}. Paste the URI manually.")
    });
    let had_device_key = AuthFlow::has_device_key(&config);
    let mut auth = AuthFlow::new(config)?;
    let mut app = App::default();
    app.set_device_name_suggestion(auth.device_name());
    if let Some(warning) = protocol_warning {
        app.set_message(Some(warning));
    }
    let mut identity = None;

    if had_device_key {
        match auth.current_user() {
            Ok(user) => identity = user,
            Err(error) => {
                app.set_message(Some(format!("Could not verify device with API: {error}")))
            }
        }
    }
    let mut username = identity.as_ref().map(|user| user.username.clone());

    if start_login {
        if let Some(user) = identity.as_ref() {
            println!(
                "Hi @{} ({}) [{}]",
                user.username,
                user.email.as_deref().unwrap_or("email unavailable"),
                user.device_name
            );
            return Ok(());
        }
    }

    apply_cli_device_name_prefill(&mut app, device_name);

    let mut terminal = terminal::TerminalSession::new()?;
    while !app.should_quit() {
        let view = match username.as_deref() {
            Some(_) => ui::View::SignedIn,
            None => ui::View::SignIn {
                active_step: app.active_step(),
                mode: app.mode(),
                name_device_complete: app.is_step_complete(app::Step::NameDevice),
                browser_approval_complete: app.is_step_complete(app::Step::BrowserApproval),
                device_name_input: app.device_name_input(),
                device_name_suggestion: app.device_name_suggestion(),
                authorization_url: auth.authorization_url(),
                fingerprint: auth.fingerprint(),
                callback_input: app.callback_input(),
                message: app.message(),
            },
        };
        terminal.draw(|frame| ui::render(frame, &view))?;

        if username.is_none() {
            if let Some(uri) = auth.take_received_callback()? {
                complete_callback(&auth, uri, &mut app, &mut username);
                continue;
            }
        }

        let action = event::read_action(Duration::from_millis(100), app.mode(), app.active_step())?;
        let Some(action) = action else {
            continue;
        };

        let contextual_actions = app.contextual_actions();

        match action {
            Action::OpenBrowser if username.is_none() && contextual_actions.open_browser => {
                match webbrowser::open(auth.authorization_url()) {
                    Ok(()) => app.set_message(Some(
                        "Approve stdout in the browser, then paste its callback URI here."
                            .to_owned(),
                    )),
                    Err(error) => app.set_message(Some(format!("Could not open browser: {error}"))),
                }
            }
            Action::CopyAuthorizationUrl
                if username.is_none() && contextual_actions.copy_authorization_url =>
            {
                match clipboard::copy(auth.authorization_url()) {
                    Ok(()) => {
                        app.set_message(Some("Authorization URL copied to clipboard.".to_owned()))
                    }
                    Err(error) => {
                        app.set_message(Some(format!("Could not copy URL to clipboard: {error}")))
                    }
                }
            }
            Action::RestartAuthorization
                if username.is_none() && contextual_actions.restart_authorization =>
            {
                match auth.restart() {
                    Ok(()) => app.set_message(Some("New sign-in link ready.".to_owned())),
                    Err(error) => {
                        app.set_message(Some(format!("Could not restart sign-in: {error}")))
                    }
                }
                app.take_callback_input();
            }
            Action::ContinueStep
                if username.is_none()
                    && app.active_step() == Step::NameDevice
                    && contextual_actions.enter == Some(EnterAction::Continue) =>
            {
                let device_name =
                    device_name_to_submit(app.device_name_input(), app.device_name_suggestion());
                match auth.set_device_name(device_name.clone()) {
                    Ok(()) => {
                        app.set_device_name_input(device_name);
                        app.update(Action::ContinueStep);
                        app.set_message(None);
                    }
                    Err(error) => {
                        app.set_message(Some(format!("Could not start sign-in: {error}")))
                    }
                }
            }
            Action::SubmitCallback
                if username.is_none()
                    && contextual_actions.enter == Some(EnterAction::SubmitCallback) =>
            {
                let uri = app.take_callback_input();
                if uri.trim().is_empty() {
                    app.set_message(Some(
                        "Paste the full devnull:// callback URI first.".to_owned(),
                    ));
                } else {
                    complete_callback(&auth, uri, &mut app, &mut username);
                }
            }
            other => app.update(other),
        }
    }

    Ok(())
}

fn apply_cli_device_name_prefill(app: &mut App, device_name: Option<String>) {
    if let Some(name) = device_name {
        app.set_device_name_input(name);
    }
}

fn device_name_to_submit(input: &str, suggestion: &str) -> String {
    if input.trim().is_empty() {
        suggestion.to_owned()
    } else {
        input.to_owned()
    }
}

fn run_auth_logout(config: Config) -> Result<()> {
    let validation = if AuthFlow::has_device_key(&config) {
        Some(
            match AuthFlow::new(config.clone()).and_then(|auth| auth.current_user()) {
                Ok(Some(user)) => format!("API validated the local key for {}.", user.username),
                Ok(None) => "API reports that the local key is not approved.".to_owned(),
                Err(error) => {
                    format!("Could not validate the local key against the API: {error:#}")
                }
            },
        )
    } else {
        None
    };

    let removed_key = auth::logout_local(&config)?;
    if let Some(validation) = validation {
        println!("{validation}");
    }
    if removed_key {
        println!("Logged out locally. The server-side device remains approved; revoke it in dev-null.sh device settings.");
    } else {
        println!("No local device key. Any server-side device approval remains unchanged.");
    }
    Ok(())
}

fn print_help() {
    println!(
        "stdout — dev-null.sh terminal client\n\nUsage:\n  stdout [--config <dir>] [--help]\n  stdout [--config <dir>] auth login [--device-name <name>]\n  stdout [--config <dir>] auth logout\n\nCommands:\n  auth login   Show current user if authenticated; otherwise sign in\n               Opens sign-in TUI; --device-name pre-fills step 1\n  auth logout  Remove local credentials and cached identity (server approval remains)\n\nOptions:\n  --config <dir>       Use this config directory (config.toml and local credentials)\n  -h, --help           Show this help\n  --install-protocol   Re-register the devnull:// callback handler"
    );
}

fn complete_callback(auth: &AuthFlow, uri: String, app: &mut App, username: &mut Option<String>) {
    match auth.complete_callback(&uri) {
        Ok(identity) => {
            app.mark_browser_approval_complete();
            *username = Some(identity.username);
            app.set_message(None);
        }
        Err(error) => app.set_message(Some(format!("Sign-in failed: {error:#}"))),
    }
}

#[cfg(test)]
mod tests {
    use std::env;

    use super::{apply_cli_device_name_prefill, device_name_to_submit, take_config_dir, App};

    #[test]
    fn config_option_works_after_a_subcommand_and_uses_absolute_path() {
        let mut arguments = vec![
            "auth".to_owned(),
            "login".to_owned(),
            "--config".to_owned(),
            "custom-config".to_owned(),
        ];

        let config_dir = take_config_dir(&mut arguments).unwrap().unwrap();
        assert_eq!(arguments, ["auth", "login"]);
        assert_eq!(
            config_dir,
            env::current_dir().unwrap().join("custom-config")
        );
    }

    #[test]
    fn config_option_requires_one_directory() {
        assert!(take_config_dir(&mut vec!["--config".to_owned()]).is_err());
        assert!(take_config_dir(&mut vec![
            "--config".to_owned(),
            "one".to_owned(),
            "--config".to_owned(),
            "two".to_owned(),
        ])
        .is_err());
    }

    #[test]
    fn device_name_submission_prefers_user_input_and_falls_back_to_suggestion() {
        assert_eq!(
            device_name_to_submit(" Work Mac ", "stdout on host"),
            " Work Mac "
        );
        assert_eq!(
            device_name_to_submit("   ", "stdout on host"),
            "stdout on host"
        );
    }

    #[test]
    fn device_name_input_is_prefilled_only_from_cli_flag() {
        let mut app = App::default();

        apply_cli_device_name_prefill(&mut app, None);
        assert_eq!(app.device_name_input(), "");

        apply_cli_device_name_prefill(&mut app, Some("Work Mac".to_owned()));
        assert_eq!(app.device_name_input(), "Work Mac");
    }
}
