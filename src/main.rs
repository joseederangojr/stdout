mod api;
mod app;
mod auth;
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

use anyhow::{bail, Context, Result};
use app::{Action, App};
use auth::AuthFlow;
use config::Config;

fn main() {
    if let Err(error) = run() {
        eprintln!("stdout: {error:#}");
        std::process::exit(1);
    }
}

fn run() -> Result<()> {
    if std::env::args().nth(1).as_deref() == Some("--install-protocol") {
        protocol::install()?;
        return Ok(());
    }

    if let Some((uri, config_dir)) = callback_argument()? {
        let config = match config_dir {
            Some(dir) => Config::load_at_dir(dir)?,
            None => Config::load()?,
        };
        auth::receive_callback_for_running_tui(&config, &uri)?;
        return Ok(());
    }

    if !io::stdin().is_terminal() || !io::stdout().is_terminal() {
        anyhow::bail!("an interactive terminal is required");
    }

    let config = Config::load()?;
    let had_device_key = AuthFlow::has_device_key(&config);
    let mut auth = AuthFlow::new(config)?;
    let mut app = App::default();
    let mut username = None;

    if had_device_key {
        match auth.current_user() {
            Ok(user) => username = user,
            Err(error) => app.set_message(Some(format!("Could not verify device: {error}"))),
        }
    }

    let mut terminal = terminal::TerminalSession::new()?;
    while !app.should_quit() {
        let view = match username.as_deref() {
            Some(_) => ui::View::SignedIn,
            None => ui::View::SignIn {
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

        let action = event::read_action(Duration::from_millis(100), app.input_is_empty())?;
        let Some(action) = action else {
            continue;
        };

        match action {
            Action::OpenBrowser if username.is_none() => {
                match webbrowser::open(auth.authorization_url()) {
                    Ok(()) => app.set_message(Some(
                        "Approve stdout in the browser, then paste its callback URI here."
                            .to_owned(),
                    )),
                    Err(error) => app.set_message(Some(format!("Could not open browser: {error}"))),
                }
            }
            Action::RestartAuthorization if username.is_none() => {
                match auth.restart() {
                    Ok(()) => app.set_message(Some("New sign-in link ready.".to_owned())),
                    Err(error) => {
                        app.set_message(Some(format!("Could not restart sign-in: {error}")))
                    }
                }
                app.take_callback_input();
            }
            Action::SubmitCallback if username.is_none() => {
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

fn complete_callback(auth: &AuthFlow, uri: String, app: &mut App, username: &mut Option<String>) {
    match auth.complete_callback(&uri) {
        Ok(name) => {
            *username = Some(name);
            app.set_message(None);
        }
        Err(error) => app.set_message(Some(format!("Sign-in failed: {error:#}"))),
    }
}

fn callback_argument() -> Result<Option<(String, Option<PathBuf>)>> {
    let mut arguments = std::env::args().skip(1);
    let Some(first) = arguments.next() else {
        return Ok(None);
    };

    if first == "--callback" {
        let uri = arguments.next().context("--callback requires a URI")?;
        let config_dir = match arguments.next() {
            None => None,
            Some(flag) if flag == "--config-dir" => Some(PathBuf::from(
                arguments.next().context("--config-dir requires a path")?,
            )),
            Some(argument) => bail!("unexpected callback argument: {argument}"),
        };
        if arguments.next().is_some() {
            bail!("unexpected argument after callback options");
        }
        return Ok(Some((uri, config_dir)));
    }

    Ok(first.starts_with("devnull://").then_some((first, None)))
}
