#[allow(dead_code)] // API wire fields remain available for response parsing.
mod api;
mod app;
mod auth;
mod clipboard;
mod config;
mod device;
mod event;
#[allow(dead_code)] // Legacy navigation helpers remain for existing tests.
mod navigation;
mod protocol;
mod terminal;
#[allow(dead_code, clippy::needless_range_loop)] // Legacy views/tests coexist with API cards.
mod ui;

use std::{
    io::{self, IsTerminal},
    path::{Path, PathBuf},
    process::{Command, Stdio},
    sync::mpsc::{self, Receiver, Sender},
    time::{Duration, SystemTime, UNIX_EPOCH},
};

use anyhow::{bail, Result};
use app::{Action, App, ComposeKind, EnterAction, LogInteraction, Mode, Step};
use auth::AuthFlow;
use config::Config;
use navigation::{Keymap, LogTab, Navigation, Page};
use ratatui::{
    layout::Rect,
    style::{Color, Style},
    widgets::Paragraph,
};
use url::Url;

struct FeedRequest {
    epoch: u64,
    tab: LogTab,
    cursor: Option<String>,
}

struct FeedResponse {
    epoch: u64,
    tab: LogTab,
    result: std::result::Result<api::LogPage, String>,
}

struct FeedState {
    epoch: u64,
    tab: LogTab,
    commits: Vec<api::LogCommit>,
    next_cursor: Option<String>,
    loaded: bool,
    pending: bool,
    error: Option<String>,
}

impl FeedState {
    fn new(tab: LogTab) -> Self {
        Self {
            epoch: 0,
            tab,
            commits: Vec::new(),
            next_cursor: None,
            loaded: false,
            pending: false,
            error: None,
        }
    }

    fn switch(&mut self, tab: LogTab) -> bool {
        if self.tab == tab {
            return false;
        }
        self.tab = tab;
        self.refresh();
        true
    }

    fn refresh(&mut self) {
        self.epoch = self.epoch.wrapping_add(1);
        *self = Self {
            epoch: self.epoch,
            ..Self::new(self.tab)
        };
    }

    fn request(&mut self, sender: &Sender<FeedRequest>) {
        if self.pending || (self.loaded && self.next_cursor.is_none()) {
            return;
        }
        self.error = None;
        match sender.send(FeedRequest {
            epoch: self.epoch,
            tab: self.tab,
            cursor: self.next_cursor.clone(),
        }) {
            Ok(()) => self.pending = true,
            Err(_) => self.error = Some("Feed worker stopped.".to_owned()),
        }
    }

    fn accept(&mut self, response: FeedResponse) -> bool {
        if !self.pending || response.epoch != self.epoch || response.tab != self.tab {
            return false;
        }
        self.pending = false;
        match response.result {
            Ok(page) if page.log == log_kind(self.tab) => {
                self.loaded = true;
                self.next_cursor = page.meta.next_cursor;
                self.commits.extend(page.data);
                self.error = None;
            }
            Ok(_) => self.error = Some("API returned the wrong Log tab.".to_owned()),
            Err(error) => self.error = Some(error),
        }
        true
    }

    fn status(&self) -> Option<String> {
        if let Some(error) = &self.error {
            Some(format!(
                "Could not load {}: {error} · j or scroll to retry",
                self.tab.label()
            ))
        } else if self.pending {
            Some(format!("Loading {}…", self.tab.label()))
        } else if self.loaded && self.commits.is_empty() {
            Some(format!("No commits in {}.", self.tab.label()))
        } else {
            None
        }
    }
}

fn refresh_log(
    navigation: &mut Navigation,
    app: &App,
    feed: &mut FeedState,
    sender: Option<&Sender<FeedRequest>>,
) -> bool {
    if navigation.current_page() != Page::Log || app.log_interaction().is_some() {
        return false;
    }
    navigation.clear_pending_prefix();
    navigation.sync_log_entries(0);
    feed.refresh();
    if let Some(sender) = sender {
        feed.request(sender);
    }
    true
}

fn log_kind(tab: LogTab) -> api::LogKind {
    match tab {
        LogTab::Following => api::LogKind::Following,
        LogTab::Latest => api::LogKind::Latest,
        LogTab::Trending => api::LogKind::Trending,
    }
}

fn approved_client(config: Config) -> Result<api::ApiClient> {
    // load_or_create must never run without an existing device identity.
    if !AuthFlow::has_device_key(&config) {
        bail!("approved device key is missing");
    }
    let key = device::DeviceKey::load_or_create(&config.dir)?;
    api::ApiClient::new(config.api_origin, key)
}

fn start_feed_worker(config: Config) -> (Sender<FeedRequest>, Receiver<FeedResponse>) {
    let (requests, incoming) = mpsc::channel::<FeedRequest>();
    let (outgoing, responses) = mpsc::channel();
    std::thread::spawn(move || {
        let client = approved_client(config).map_err(|error| format!("{error:#}"));
        for request in incoming {
            let result = client.as_ref().map_err(Clone::clone).and_then(|client| {
                client
                    .log(log_kind(request.tab), request.cursor.as_deref())
                    .map_err(|error| format!("{error:#}"))
            });
            if outgoing
                .send(FeedResponse {
                    epoch: request.epoch,
                    tab: request.tab,
                    result,
                })
                .is_err()
            {
                break;
            }
        }
    });
    (requests, responses)
}

// Writes are only sent after a dialog returns a confirmed intent; shortcuts never enqueue them.
#[derive(Clone, Debug, PartialEq, Eq)]
enum SignedRequest {
    Detail {
        id: String,
    },
    Star {
        id: String,
    },
    Unstar {
        id: String,
    },
    Reply {
        id: String,
        body: String,
    },
    Quote {
        id: String,
        body: String,
    },
    Report {
        id: String,
        reason: api::ReportReason,
    },
    Pastel {
        id: String,
    },
}

#[derive(Debug)]
enum SignedResult {
    Detail(api::CommitDetail),
    Commit(api::LogCommit),
    Pasteis(api::PasteisResult),
    Reported,
}

#[derive(Debug)]
struct SignedResponse {
    request: SignedRequest,
    result: Result<SignedResult>,
}

fn execute_signed(client: &api::ApiClient, request: &SignedRequest) -> Result<SignedResult> {
    Ok(match request {
        SignedRequest::Detail { id } => SignedResult::Detail(client.commit(id)?),
        SignedRequest::Star { id } => SignedResult::Commit(client.star(id)?),
        SignedRequest::Unstar { id } => SignedResult::Commit(client.unstar(id)?),
        SignedRequest::Reply { id, body } => SignedResult::Commit(client.reply(id, body)?),
        SignedRequest::Quote { id, body } => SignedResult::Commit(client.quote(id, body)?),
        SignedRequest::Report { id, reason } => {
            client.report(id, *reason, None)?;
            SignedResult::Reported
        }
        SignedRequest::Pastel { id } => SignedResult::Pasteis(client.pasteis(id)?),
    })
}

fn route_signed_requests(
    incoming: Receiver<SignedRequest>,
    outgoing: Sender<SignedResponse>,
    mut execute: impl FnMut(&SignedRequest) -> Result<SignedResult>,
) {
    for request in incoming {
        let result = execute(&request);
        if outgoing.send(SignedResponse { request, result }).is_err() {
            break;
        }
    }
}

fn start_signed_worker(config: Config) -> (Sender<SignedRequest>, Receiver<SignedResponse>) {
    let (requests, incoming) = mpsc::channel();
    let (outgoing, responses) = mpsc::channel();
    std::thread::spawn(move || {
        let client = approved_client(config).map_err(|error| format!("{error:#}"));
        route_signed_requests(incoming, outgoing, |request| {
            let client = client
                .as_ref()
                .map_err(|error| anyhow::anyhow!("{error}"))?;
            execute_signed(client, request)
        });
    });
    (requests, responses)
}

fn relative_time(created_at: &str, now: SystemTime) -> String {
    let Some(age) =
        time::OffsetDateTime::parse(created_at, &time::format_description::well_known::Rfc3339)
            .ok()
            .and_then(|created| {
                let now = now.duration_since(UNIX_EPOCH).ok()?.as_nanos() as i128;
                let age = now - created.unix_timestamp_nanos();
                (age >= 0).then_some(age / 1_000_000_000)
            })
    else {
        return created_at.to_owned();
    };

    match age {
        0..60 => "just now".to_owned(),
        60..3600 => format!("{}m ago", age / 60),
        3600..86400 => format!("{}h ago", age / 3600),
        _ => format!("{}d ago", age / 86400),
    }
}

fn present_commits<'a>(
    commits: &'a [api::LogCommit],
    times: &'a [String],
) -> Vec<ui::LogCommit<'a>> {
    commits
        .iter()
        .enumerate()
        .map(|(index, commit)| {
            let author = commit.author.as_ref();
            ui::LogCommit {
                author: author.map_or("Unavailable author", |author| author.name.as_str()),
                handle: author.map_or("", |author| author.username.as_str()),
                time: times
                    .get(index)
                    .map_or(commit.created_at.as_str(), String::as_str),
                pending: matches!(commit.status, api::CommitStatus::Pending),
                hash: &commit.short_hash,
                body: &commit.body,
                thread: (commit.replies_count > 0).then_some("Replies"),
            }
        })
        .collect()
}

#[derive(Debug, PartialEq, Eq)]
enum LogIntent {
    Star(String),
    Reply(String),
    Quote(String),
    Report(String),
    Pastel(String),
    CopyUrl(Url),
    OpenUrl(Url),
    Thread(String),
}

fn verified_commit_url(commit: &api::LogCommit, origin: &Url) -> Option<Url> {
    let url = Url::parse(&commit.html_url).ok()?;
    let segments: Vec<_> = url.path_segments()?.collect();
    (matches!(url.scheme(), "https" | "http")
        && url.origin() == origin.origin()
        && url.username().is_empty()
        && url.password().is_none()
        && url.query().is_none()
        && url.fragment().is_none()
        && matches!(segments.as_slice(), [author, "commits", id] if author.starts_with('@') && !author.contains('%') && *id == commit.id))
    .then_some(url)
}

// Default q is never a prefix; this reveals pending state even with a custom q binding.
fn navigation_prefix_pending(navigation: &Navigation) -> bool {
    !navigation.should_quit('q', &Keymap::default())
}

// Navigation prefixes (including configured first keys) win over commit shortcuts.
fn log_intent(
    key: char,
    navigation: &Navigation,
    keymap: &Keymap,
    commits: &[api::LogCommit],
    origin: &Url,
) -> Option<LogIntent> {
    if navigation.current_page() != Page::Log || keymap.is_prefix(key) {
        return None;
    }
    let mut probe = navigation.clone();
    if probe.input(key, keymap).is_some() || navigation_prefix_pending(navigation) {
        return None;
    }
    let commit = commits.get(navigation.log_selection()?)?;
    Some(match key {
        's' => LogIntent::Star(commit.id.clone()),
        'r' => LogIntent::Reply(commit.id.clone()),
        'q' => LogIntent::Quote(commit.id.clone()),
        'R' => LogIntent::Report(commit.id.clone()),
        'p' => LogIntent::Pastel(commit.id.clone()),
        'y' => LogIntent::CopyUrl(verified_commit_url(commit, origin)?),
        'o' => LogIntent::OpenUrl(verified_commit_url(commit, origin)?),
        _ => return None,
    })
}

fn log_enter_intent(navigation: &Navigation, commits: &[api::LogCommit]) -> Option<LogIntent> {
    if navigation.current_page() != Page::Log || navigation_prefix_pending(navigation) {
        return None;
    }
    commits
        .get(navigation.log_selection()?)
        .map(|commit| LogIntent::Thread(commit.id.clone()))
}

fn clicked_body_url<'a>(
    navigation: &Navigation,
    app: &App,
    geometry: Option<&ui::CardFeedLayout>,
    cards: &[ui::LogCommit<'a>],
    column: u16,
    row: u16,
) -> Option<&'a str> {
    if navigation.current_page() != Page::Log || app.log_interaction().is_some() {
        return None;
    }
    geometry
        .and_then(|layout| layout.link_at(column, row, navigation.log_scroll(), cards))
        .filter(|raw| ui::safe_body_url(raw))
}

fn wheel_feed(
    navigation: &mut Navigation,
    geometry: Option<&ui::CardFeedLayout>,
    column: u16,
    row: u16,
    up: bool,
) -> bool {
    let Some(layout) = geometry.filter(|layout| layout.contains(column, row)) else {
        return false;
    };
    if navigation.current_page() != Page::Log {
        return false;
    }
    navigation.scroll_log(if up { -3 } else { 3 }, layout.clamp_scroll(u16::MAX));
    !up && navigation.log_scroll() == layout.clamp_scroll(u16::MAX)
}

fn reconcile_log_scroll(
    navigation: &mut Navigation,
    layout: &ui::CardFeedLayout,
    reveal_focus: bool,
) {
    let target = if reveal_focus {
        navigation.log_selection().map_or_else(
            || layout.clamp_scroll(navigation.log_scroll()),
            |index| layout.scroll_to_show(navigation.log_scroll(), index),
        )
    } else {
        layout.clamp_scroll(navigation.log_scroll())
    };
    while navigation.log_scroll() != target {
        let previous = navigation.log_scroll();
        let delta = target as i32 - previous as i32;
        navigation.scroll_log(
            delta.clamp(i16::MIN as i32, i16::MAX as i32) as i16,
            layout.clamp_scroll(u16::MAX),
        );
        if navigation.log_scroll() == previous {
            break; // A navigation prefix suppresses scrolling until resolved.
        }
    }
}

#[allow(clippy::too_many_arguments)]
fn handle_navigation_key(
    key: char,
    navigation: &mut Navigation,
    keymap: &Keymap,
    app: &mut App,
    feed: &mut FeedState,
    feed_worker: &Option<(Sender<FeedRequest>, Receiver<FeedResponse>)>,
    remotes_url: &Url,
    config_path: &Path,
    reveal_focus: &mut bool,
) {
    if navigation.should_quit(key, keymap) {
        app.update(Action::Quit);
    } else if keymap.is_prefix(key) && !navigation_prefix_pending(navigation) {
        navigation.input(key, keymap);
    } else if navigation.handle_log_key_with_count(key, feed.commits.len()) {
        *reveal_focus = true;
        if feed.switch(navigation.log_tab())
            || key == 'j'
                && (feed.commits.is_empty()
                    || navigation
                        .log_selection()
                        .is_some_and(|index| index + 2 >= feed.commits.len()))
        {
            if let Some((requests, _)) = feed_worker {
                feed.request(requests);
            }
        }
    } else {
        match navigation.input(key, keymap) {
            Some(Page::Remotes) => {
                if let Err(error) = webbrowser::open(remotes_url.as_str()) {
                    app.set_message(Some(format!("Could not open Remotes: {error}")));
                }
            }
            Some(Page::Config) => {
                if let Err(error) = open_config_file(config_path) {
                    app.set_message(Some(format!("Could not open config: {error}")));
                }
            }
            _ => {}
        }
    }
}

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

fn apply_signed_response(response: SignedResponse, app: &mut App, feed: &mut FeedState) {
    match (response.request, response.result) {
        (SignedRequest::Detail { id }, Ok(SignedResult::Detail(detail))) => {
            if let Some(LogInteraction::Detail { target, handle, .. }) = app.log_interaction() {
                if *target == id && detail.data.id == id {
                    let handle = handle.clone();
                    app.begin_detail(&id, &handle, &detail.data.body);
                }
            }
        }
        (
            SignedRequest::Star { id } | SignedRequest::Unstar { id },
            Ok(SignedResult::Commit(commit)),
        ) => {
            if commit.id == id {
                if let Some(existing) = feed.commits.iter_mut().find(|item| item.id == id) {
                    *existing = commit;
                }
                app.set_message(Some("Star updated.".to_owned()));
            } else {
                app.set_message(Some("API returned a different commit.".to_owned()));
            }
        }
        (
            SignedRequest::Reply { id, body } | SignedRequest::Quote { id, body },
            Ok(SignedResult::Commit(_)),
        ) => {
            if matches!(app.log_interaction(), Some(LogInteraction::Compose { target, body: draft, .. }) if *target == id && *draft == body)
            {
                app.finish_log_interaction();
            }
            app.set_message(Some(
                "Commit submitted. Refresh the feed to see API changes.".to_owned(),
            ));
        }
        (SignedRequest::Pastel { id }, Ok(SignedResult::Pasteis(result))) => {
            if result.data.id == id {
                if let Some(existing) = feed.commits.iter_mut().find(|item| item.id == id) {
                    *existing = result.data;
                }
                if matches!(app.log_interaction(), Some(LogInteraction::PastelConfirm { target, .. }) if *target == id)
                {
                    app.finish_log_interaction();
                }
                app.set_message(Some(format!(
                    "Pastel sent · {} remaining today.",
                    result.pasteis_remaining
                )));
            } else {
                app.log_error("API returned a different commit.");
                app.set_message(Some("API returned a different commit.".to_owned()));
            }
        }
        (SignedRequest::Report { id, reason }, Ok(SignedResult::Reported)) => {
            if matches!(app.log_interaction(), Some(LogInteraction::Report { target, reason: selected, .. }) if *target == id && *selected == Some(reason))
            {
                app.finish_log_interaction();
            }
            app.set_message(Some("Report submitted.".to_owned()));
        }
        (_, Err(error)) => {
            let message = format!("API request failed: {error:#}");
            app.log_error(&message);
            app.set_message(Some(message));
        }
        _ => app.set_message(Some("Unexpected API response.".to_owned())),
    }
}

fn send_signed(
    worker: &Option<(Sender<SignedRequest>, Receiver<SignedResponse>)>,
    pending: &mut Option<SignedRequest>,
    app: &mut App,
    request: SignedRequest,
) {
    let result = worker
        .as_ref()
        .ok_or("API worker unavailable")
        .and_then(|(sender, _)| {
            sender
                .send(request.clone())
                .map_err(|_| "API worker stopped")
        });
    match result {
        Ok(()) => {
            *pending = Some(request);
            app.set_message(None);
        }
        Err(error) => {
            app.log_error(error);
            app.set_message(Some(error.to_owned()));
        }
    }
}

fn run_tui(start_login: bool, device_name: Option<String>, config: Config) -> Result<()> {
    if !io::stdin().is_terminal() || !io::stdout().is_terminal() {
        bail!("an interactive terminal is required");
    }

    let protocol_warning = protocol::ensure_installed(&config).err().map(|error| {
        format!("Could not register callback handler: {error}. Paste the URI manually.")
    });
    let had_device_key = AuthFlow::has_device_key(&config);
    let keymap = config.keymap.clone();
    let remotes_url = config.api_origin.join("remotes")?;
    let config_path = config.dir.join("config.toml");
    let feed_config = config.clone();
    let api_origin = config.api_origin.clone();
    let mut auth = AuthFlow::new(config)?;
    let mut app = App::default();
    let mut navigation = Navigation::default();
    let mut feed = FeedState::new(navigation.log_tab());
    let mut feed_worker = None;
    let mut signed_worker = None;
    let mut pending_signed: Option<SignedRequest> = None;
    let mut reveal_focus = true;
    let mut last_size = None;
    let mut redraw = true;
    let mut shown_minute = None;
    let mut times = Vec::new();
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
        if username.is_some() {
            let (_, signed_responses) =
                signed_worker.get_or_insert_with(|| start_signed_worker(feed_config.clone()));
            while let Ok(response) = signed_responses.try_recv() {
                if pending_signed.as_ref() == Some(&response.request) {
                    pending_signed = None;
                    apply_signed_response(response, &mut app, &mut feed);
                    redraw = true;
                }
            }
            let (requests, responses) =
                feed_worker.get_or_insert_with(|| start_feed_worker(feed_config.clone()));
            if !feed.loaded && !feed.pending && feed.error.is_none() {
                feed.request(requests);
                redraw = true;
            }
            while let Ok(response) = responses.try_recv() {
                if feed.accept(response) {
                    redraw = true;
                    let had_focus = navigation.log_selection().is_some();
                    navigation.sync_log_entries(feed.commits.len());
                    reveal_focus |= !had_focus && navigation.log_selection().is_some();
                }
            }
        }
        let now = SystemTime::now();
        let minute = now
            .duration_since(UNIX_EPOCH)
            .ok()
            .map(|d| d.as_secs() / 60);
        let (width, height) = crossterm::terminal::size()?;
        if last_size.replace((width, height)) != Some((width, height)) {
            reveal_focus = true;
            redraw = true;
        }
        if shown_minute != minute
            && username.is_some()
            && navigation.current_page() == Page::Log
            && !feed.commits.is_empty()
        {
            redraw = true;
        }
        if redraw {
            times = feed
                .commits
                .iter()
                .map(|commit| relative_time(&commit.created_at, now))
                .collect();
        }
        let cards = present_commits(&feed.commits, &times);
        let geometry = ui::card_feed_layout(width, height, &cards, navigation.log_selection());
        if let Some(layout) = &geometry {
            reconcile_log_scroll(&mut navigation, layout, reveal_focus);
        }
        reveal_focus = false;
        if redraw {
            let view = match username.as_deref() {
                Some(_) => match app.log_interaction() {
                    Some(interaction) if navigation.current_page() == Page::Log => {
                        ui::View::SignedInInteraction {
                            current_page: navigation.current_page(),
                            keymap: &keymap,
                            log_tab: navigation.log_tab(),
                            log_scroll: navigation.log_scroll(),
                            commits: &cards,
                            focused_commit: navigation.log_selection(),
                            interaction,
                        }
                    }
                    _ => ui::View::SignedInCards {
                        current_page: navigation.current_page(),
                        keymap: &keymap,
                        log_tab: navigation.log_tab(),
                        log_scroll: navigation.log_scroll(),
                        commits: &cards,
                        focused_commit: navigation.log_selection(),
                    },
                },
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
            terminal.draw(|frame| {
                ui::render(frame, &view);
                if username.is_some() && navigation.current_page() == Page::Log {
                    let status = if let Some(message) = app
                        .message()
                        .filter(|message| message.starts_with("Could not open link:"))
                    {
                        Some(message.to_owned())
                    } else if pending_signed.is_some() {
                        Some("Waiting for API…".to_owned())
                    } else {
                        feed.status().or_else(|| app.message().map(str::to_owned))
                    };
                    if let (Some(layout), Some(status)) = (&geometry, status) {
                        if layout.viewport.width > 0 && layout.viewport.height > 0 {
                            let row = layout.status_row(feed.commits.is_empty());
                            frame.render_widget(
                                Paragraph::new(status).style(Style::default().fg(Color::Yellow)),
                                Rect::new(layout.viewport.x, row, layout.viewport.width, 1),
                            );
                        }
                    }
                }
            })?;
            shown_minute = minute;
            redraw = false;
        }

        if username.is_none() {
            if let Some(uri) = auth.take_received_callback()? {
                complete_callback(&auth, uri, &mut app, &mut username);
                redraw = true;
                continue;
            }
        }

        let action = event::read_action(
            Duration::from_millis(100),
            app.mode(),
            app.active_step(),
            username.is_some(),
        )?;
        let Some(action) = action else {
            continue;
        };
        redraw = true;

        let contextual_actions = app.contextual_actions();

        if username.is_some() && app.log_interaction().is_some() && !matches!(action, Action::Quit)
        {
            // Freeze a submitted draft until its response; otherwise edits after POST
            // could leave an already-sent draft open and allow a duplicate retry.
            if pending_signed.is_some() && !matches!(action, Action::SetMode(Mode::Normal)) {
                continue;
            }
            if let Some(intent) = app.update_log(action) {
                if pending_signed.is_none() {
                    let request = match intent {
                        app::LogIntent::Commit {
                            kind: ComposeKind::Reply,
                            target,
                            body,
                        } => SignedRequest::Reply { id: target, body },
                        app::LogIntent::Commit {
                            kind: ComposeKind::Quote,
                            target,
                            body,
                        } => SignedRequest::Quote { id: target, body },
                        app::LogIntent::Report { target, reason } => {
                            SignedRequest::Report { id: target, reason }
                        }
                        app::LogIntent::Pastel { target } => SignedRequest::Pastel { id: target },
                    };
                    send_signed(&signed_worker, &mut pending_signed, &mut app, request);
                }
            }
            continue;
        }

        match action {
            Action::RefreshLog if username.is_some() => {
                if refresh_log(
                    &mut navigation,
                    &app,
                    &mut feed,
                    feed_worker.as_ref().map(|(requests, _)| requests),
                ) {
                    reveal_focus = false;
                }
            }
            Action::LogShortcut(key) if username.is_some() => {
                if let Some(intent) =
                    log_intent(key, &navigation, &keymap, &feed.commits, &api_origin)
                {
                    let commit = feed
                        .commits
                        .get(navigation.log_selection().unwrap_or(usize::MAX));
                    match intent {
                        LogIntent::Star(id) if pending_signed.is_none() => {
                            let request = if commit.is_some_and(|commit| commit.starred) {
                                SignedRequest::Unstar { id }
                            } else {
                                SignedRequest::Star { id }
                            };
                            send_signed(&signed_worker, &mut pending_signed, &mut app, request);
                        }
                        LogIntent::Reply(_)
                        | LogIntent::Quote(_)
                        | LogIntent::Report(_)
                        | LogIntent::Pastel(_)
                            if pending_signed.is_none() =>
                        {
                            if let Some(commit) = commit {
                                let handle = commit
                                    .author
                                    .as_ref()
                                    .map_or("", |author| author.username.as_str());
                                match intent {
                                    LogIntent::Reply(_) => {
                                        app.begin_compose(ComposeKind::Reply, &commit.id, handle)
                                    }
                                    LogIntent::Quote(_) => {
                                        app.begin_compose(ComposeKind::Quote, &commit.id, handle)
                                    }
                                    LogIntent::Report(_) => app.begin_report(&commit.id, handle),
                                    LogIntent::Pastel(_) => app.begin_pastel(&commit.id, handle),
                                    _ => unreachable!(),
                                }
                                app.set_message(None);
                            }
                        }
                        LogIntent::CopyUrl(url) => match clipboard::copy(url.as_str()) {
                            Ok(()) => {
                                app.set_message(Some("Commit URL copied to clipboard.".to_owned()))
                            }
                            Err(error) => {
                                app.set_message(Some(format!("Could not copy URL: {error}")))
                            }
                        },
                        LogIntent::OpenUrl(url) => {
                            if let Err(error) = webbrowser::open(url.as_str()) {
                                app.set_message(Some(format!("Could not open commit: {error}")));
                            }
                        }
                        _ => {}
                    }
                } else {
                    if matches!(key, 'y' | 'o')
                        && navigation.current_page() == Page::Log
                        && navigation.log_selection().is_some()
                        && !navigation_prefix_pending(&navigation)
                        && !keymap.is_prefix(key)
                    {
                        app.set_message(Some(
                            "API commit URL is not safe to open or copy.".to_owned(),
                        ));
                    }
                    handle_navigation_key(
                        key,
                        &mut navigation,
                        &keymap,
                        &mut app,
                        &mut feed,
                        &feed_worker,
                        &remotes_url,
                        &config_path,
                        &mut reveal_focus,
                    );
                }
            }
            Action::LogEnter if username.is_some() => {
                if let Some(LogIntent::Thread(id)) = log_enter_intent(&navigation, &feed.commits) {
                    if pending_signed.is_none() {
                        if let Some(commit) = feed
                            .commits
                            .get(navigation.log_selection().unwrap_or(usize::MAX))
                        {
                            let handle = commit
                                .author
                                .as_ref()
                                .map_or("", |author| author.username.as_str());
                            app.begin_detail(&id, handle, &commit.body);
                            app.set_message(None);
                            send_signed(
                                &signed_worker,
                                &mut pending_signed,
                                &mut app,
                                SignedRequest::Detail { id },
                            );
                        }
                    }
                } else {
                    navigation.clear_pending_prefix();
                }
            }
            Action::NavigationKey(key) if username.is_some() => {
                handle_navigation_key(
                    key,
                    &mut navigation,
                    &keymap,
                    &mut app,
                    &mut feed,
                    &feed_worker,
                    &remotes_url,
                    &config_path,
                    &mut reveal_focus,
                );
            }
            Action::LogClick { column, row } if username.is_some() => {
                if let Some(url) =
                    clicked_body_url(&navigation, &app, geometry.as_ref(), &cards, column, row)
                {
                    match webbrowser::open(url) {
                        Ok(()) => app.set_message(None),
                        Err(error) => {
                            app.set_message(Some(format!("Could not open link: {error}")))
                        }
                    }
                }
            }
            Action::LogWheel { column, row, up } if username.is_some() => {
                if wheel_feed(&mut navigation, geometry.as_ref(), column, row, up) {
                    if let Some((requests, _)) = &feed_worker {
                        feed.request(requests);
                    }
                }
            }
            Action::OpenConfigEditor if username.is_some() => {
                drop(terminal);
                let result = open_config_in_editor(&config_path);
                terminal = terminal::TerminalSession::new()?;
                match result {
                    Ok(()) => app.set_message(Some("Returned from $EDITOR.".to_owned())),
                    Err(error) => app.set_message(Some(format!("Could not open $EDITOR: {error}"))),
                }
            }
            Action::CancelNavigationPrefix if username.is_some() => {
                navigation.clear_pending_prefix()
            }
            Action::SetMode(Mode::Normal) if username.is_some() => {
                navigation.clear_pending_prefix();
                app.update(Action::SetMode(Mode::Normal));
            }
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

fn prepare_config_file(path: &Path) -> io::Result<()> {
    let mut options = std::fs::OpenOptions::new();
    options.write(true).create_new(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.mode(0o600);
    }
    match options.open(path) {
        Ok(mut file) => {
            use io::Write;
            file.write_all(b"# stdout configuration\n# api_origin = \"https://dev-null.sh\"\n\n[keymap]\n# log = \"g l\"\n# notifications = \"g n\"\n# remotes = \"g r\"\n# profile = \"g p\"\n# starred = \"g S\"\n# settings = \"g s\"\n# config = \"g ,\"\n")
        }
        Err(error) if error.kind() == io::ErrorKind::AlreadyExists => Ok(()),
        Err(error) => Err(error),
    }
}

fn editor_invocation(
    editor: &str,
    path: &Path,
) -> Result<(std::ffi::OsString, Vec<std::ffi::OsString>)> {
    let mut parts = editor.split_whitespace();
    let program = parts
        .next()
        .filter(|program| !program.is_empty())
        .ok_or_else(|| anyhow::anyhow!("$EDITOR is empty"))?;
    let mut args: Vec<_> = parts.map(std::ffi::OsString::from).collect();
    args.push(path.as_os_str().to_owned());
    Ok((program.into(), args))
}

fn open_config_in_editor(path: &Path) -> Result<()> {
    prepare_config_file(path)?;
    let editor = std::env::var("EDITOR").map_err(|_| anyhow::anyhow!("$EDITOR is not set"))?;
    let (program, args) = editor_invocation(&editor, path)?;
    let status = Command::new(program).args(args).status()?;
    if !status.success() {
        bail!("editor exited with {status}");
    }
    Ok(())
}

fn open_config_file(path: &Path) -> Result<()> {
    prepare_config_file(path)?;
    #[cfg(target_os = "macos")]
    let launcher = "open";
    #[cfg(target_os = "linux")]
    let launcher = "xdg-open";
    #[cfg(target_os = "windows")]
    let launcher = "explorer";
    #[cfg(not(any(target_os = "macos", target_os = "linux", target_os = "windows")))]
    bail!("opening config with the default application is unsupported on this platform");

    #[cfg(any(target_os = "macos", target_os = "linux", target_os = "windows"))]
    Command::new(launcher)
        .arg(path)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use std::{
        env,
        sync::mpsc,
        time::{Duration, UNIX_EPOCH},
    };

    use super::{
        apply_cli_device_name_prefill, apply_signed_response, clicked_body_url,
        device_name_to_submit, editor_invocation, handle_navigation_key, log_enter_intent,
        log_intent, log_kind, prepare_config_file, present_commits, reconcile_log_scroll,
        refresh_log, relative_time, route_signed_requests, take_config_dir, wheel_feed, App,
        FeedResponse, FeedState, LogIntent, SignedRequest, SignedResponse, SignedResult,
    };
    use crate::{
        api::{LogCommit, LogKind, LogPage, ReportReason},
        app::{Action, ComposeKind, LogInteraction, Mode},
        navigation::{Keymap, LogTab, Navigation, Page},
        ui,
    };

    fn commit() -> LogCommit {
        serde_json::from_value(serde_json::json!({
            "id": "commit-id", "short_hash": "abc", "body": "hello",
            "parent_id": null, "html_url": "https://dev-null.sh/@alice/commits/commit-id",
            "created_at": "now", "status": "approved", "deleted_at": null,
            "author": null, "stars_count": 0, "replies_count": 0,
            "pasteis_count": 0, "starred": false, "is_pinned": false,
            "body_segments": [], "preview_segments": null, "mentions": [],
            "link_preview": null, "images": [], "quoted_commit_id": null,
            "quoted_commit": null, "poll": null, "reply_preview": null
        }))
        .unwrap()
    }

    #[test]
    fn log_cards_show_relative_api_time_at_fixed_clock() {
        let now = time::OffsetDateTime::parse(
            "2026-01-02T12:00:00Z",
            &time::format_description::well_known::Rfc3339,
        )
        .unwrap();
        let clock = UNIX_EPOCH + Duration::from_secs(now.unix_timestamp() as u64);
        for (stamp, expected) in [
            ("2026-01-02T12:00:00Z", "just now"),
            ("2026-01-02T11:59:01Z", "just now"),
            ("2026-01-02T11:59:00Z", "1m ago"),
            ("2026-01-02T10:00:01Z", "1h ago"),
            ("2026-01-02T10:00:00Z", "2h ago"),
            ("2026-01-01T12:00:00Z", "1d ago"),
        ] {
            assert_eq!(relative_time(stamp, clock), expected);
        }
        assert_eq!(
            relative_time("2026-01-02T11:59:00Z", clock + Duration::from_secs(60)),
            "2m ago"
        );
        assert_eq!(
            relative_time("2026-01-02T12:00:00.123+01:00", clock),
            "59m ago"
        );
        for raw in [
            "",
            "not a timestamp",
            "2026-02-30T12:00:00Z",
            "2026-01-02T12:00:01Z",
            "2026-01-02T12:00:00.001Z",
        ] {
            assert_eq!(relative_time(raw, clock), raw);
        }
        let mut commit = commit();
        commit.created_at = "2026-01-02T11:59:00Z".into();
        let times = [relative_time(&commit.created_at, clock)];
        assert_eq!(
            present_commits(std::slice::from_ref(&commit), &times)[0].time,
            "1m ago"
        );
        assert_eq!(
            present_commits(&[commit], &[])[0].time,
            "2026-01-02T11:59:00Z"
        );
    }

    #[test]
    fn log_shortcuts_require_log_and_focused_commit_and_prefix_wins() {
        let keymap = Keymap::default();
        let origin = url::Url::parse("https://dev-null.sh/").unwrap();
        let commits = [commit()];
        let mut nav = Navigation::default();
        for key in ['s', 'p', 'r', 'q', 'y', 'R', 'o'] {
            assert_eq!(log_intent(key, &nav, &keymap, &commits, &origin), None);
        }
        assert_eq!(log_enter_intent(&nav, &commits), None);
        nav.sync_log_entries(1);
        for (key, expected) in [
            ('s', LogIntent::Star("commit-id".into())),
            ('p', LogIntent::Pastel("commit-id".into())),
            ('r', LogIntent::Reply("commit-id".into())),
            ('q', LogIntent::Quote("commit-id".into())),
            ('R', LogIntent::Report("commit-id".into())),
        ] {
            assert_eq!(
                log_intent(key, &nav, &keymap, &commits, &origin),
                Some(expected)
            );
        }
        assert!(matches!(
            log_intent('y', &nav, &keymap, &commits, &origin),
            Some(LogIntent::CopyUrl(_))
        ));
        assert!(matches!(
            log_intent('o', &nav, &keymap, &commits, &origin),
            Some(LogIntent::OpenUrl(_))
        ));
        assert_eq!(
            log_enter_intent(&nav, &commits),
            Some(LogIntent::Thread("commit-id".into()))
        );
        assert_eq!(log_intent('q', &nav, &keymap, &[], &origin), None);
        assert!(nav.should_quit('q', &keymap)); // no focus: q falls through to quit
        nav.input('g', &keymap);
        assert_eq!(log_intent('r', &nav, &keymap, &commits, &origin), None);
        assert_eq!(log_intent('s', &nav, &keymap, &commits, &origin), None);
        assert_eq!(log_intent('q', &nav, &keymap, &commits, &origin), None);
        assert_eq!(log_enter_intent(&nav, &commits), None);
        assert!(!nav.should_quit('q', &keymap));
        assert_eq!(nav.input('r', &keymap), Some(Page::Remotes));
        nav.input('g', &keymap);
        assert_eq!(nav.input('s', &keymap), Some(Page::Settings));
        assert_eq!(log_intent('s', &nav, &keymap, &commits, &origin), None);
        assert_eq!(log_intent('q', &nav, &keymap, &commits, &origin), None);
        assert!(nav.should_quit('q', &keymap)); // other page: quit
        assert_eq!(log_enter_intent(&nav, &commits), None);
    }

    #[test]
    fn configured_j_prefix_navigates_before_log_focus() {
        let keymap = Keymap::from_bindings([
            (Page::Log, "g l"),
            (Page::Notifications, "j n"),
            (Page::Profile, "g p"),
            (Page::Starred, "g S"),
            (Page::Remotes, "g r"),
            (Page::Settings, "g s"),
            (Page::Config, "g ,"),
        ])
        .unwrap();
        let mut nav = Navigation::default();
        nav.sync_log_entries(2);
        let mut app = App::default();
        let mut feed = FeedState::new(LogTab::Following);
        let worker = None;
        let origin = url::Url::parse("https://dev-null.sh/remotes").unwrap();
        let mut reveal = false;
        for key in ['j', 'n'] {
            handle_navigation_key(
                key,
                &mut nav,
                &keymap,
                &mut app,
                &mut feed,
                &worker,
                &origin,
                std::path::Path::new("config.toml"),
                &mut reveal,
            );
        }
        assert_eq!(nav.current_page(), Page::Notifications);
        assert_eq!(nav.log_selection(), Some(0));
        assert!(!reveal);
    }

    #[test]
    fn configured_q_prefix_suppresses_commit_actions() {
        let keymap = Keymap::from_bindings([
            (Page::Log, "q l"),
            (Page::Notifications, "g n"),
            (Page::Profile, "g p"),
            (Page::Starred, "g S"),
            (Page::Remotes, "g r"),
            (Page::Settings, "g s"),
            (Page::Config, "g ,"),
        ])
        .unwrap();
        let origin = url::Url::parse("https://dev-null.sh/").unwrap();
        let commits = [commit()];
        let mut nav = Navigation::default();
        nav.sync_log_entries(1);
        assert_eq!(log_intent('q', &nav, &keymap, &commits, &origin), None);
        nav.input('q', &keymap);
        assert_eq!(log_intent('s', &nav, &keymap, &commits, &origin), None);
        assert_eq!(log_enter_intent(&nav, &commits), None);
    }

    #[test]
    fn copy_and_open_reject_unverified_api_urls() {
        let keymap = Keymap::default();
        let origin = url::Url::parse("https://dev-null.sh/").unwrap();
        let mut nav = Navigation::default();
        nav.sync_log_entries(1);
        let mut commit = commit();
        for url in [
            "javascript:alert(1)",
            "https://evil.test/commit-id",
            "https://dev-null.sh/@alice/commits/other",
            "https://dev-null.sh/evil/commits/commit-id",
            "https://dev-null.sh/@alice/commits/commit-id?redirect=evil",
            "https://dev-null.sh/@alice/commits/commit-id#fragment",
        ] {
            commit.html_url = url.into();
            for key in ['y', 'o'] {
                assert_eq!(
                    log_intent(key, &nav, &keymap, std::slice::from_ref(&commit), &origin),
                    None
                );
            }
        }
    }

    #[test]
    fn body_links_allow_only_http_urls_without_credentials_or_controls() {
        for raw in [
            "https://example.org/a?q=1",
            "http://example.org/",
            "http://[::1]:8080/a?x=@y",
            "https://example.org/@handle",
        ] {
            assert!(ui::safe_body_url(raw), "rejected {raw:?}");
        }
        for raw in [
            "javascript:alert(1)",
            "file:///etc/passwd",
            "https://@example.org/",
            "https://@example.org",
            "http://@example.org/",
            "https://user@example.org/",
            "https://:password@example.org/",
            "https://user:password@example.org/",
            "https://example.org/\nmalicious",
            "https://example.org/\rmalicious",
            "https://example.org/\tmalicious",
            "https://example.org/\u{7f}malicious",
        ] {
            assert!(!ui::safe_body_url(raw), "accepted {raw:?}");
        }
    }

    #[test]
    fn click_opens_only_visible_log_body_links_without_a_dialog() {
        let cards = [ui::LogCommit {
            author: "Ada",
            handle: "ada",
            time: "now",
            pending: false,
            hash: "abc",
            body: "see https://example.org/docs",
            thread: None,
        }];
        let layout = ui::card_feed_layout(100, 24, &cards, None).unwrap();
        let mut navigation = Navigation::default();
        let mut app = App::default();
        let find_link = |layout: &ui::CardFeedLayout| {
            (layout.viewport.y..layout.viewport.bottom())
                .flat_map(|row| {
                    (layout.viewport.x..layout.viewport.right()).map(move |column| (column, row))
                })
                .find(|&(column, row)| {
                    layout.link_at(column, row, 0, &cards) == Some("https://example.org/docs")
                })
                .unwrap()
        };
        let (x, y) = find_link(&layout);
        assert_eq!(
            clicked_body_url(&navigation, &app, Some(&layout), &cards, x, y).unwrap(),
            "https://example.org/docs"
        );
        for (column, row) in [
            (x, y - 1),                    // card header
            (x, layout.viewport.y - 1),    // tabs
            (layout.viewport.x - 1, y),    // sidebar
            (layout.viewport.right(), y),  // right column
            (layout.viewport.x + 2, y),    // plain body text
            (x, y + 1),                    // blank gap
            (x, layout.viewport.bottom()), // footer
        ] {
            assert_eq!(
                clicked_body_url(&navigation, &app, Some(&layout), &cards, column, row),
                None
            );
        }
        let compact = ui::card_feed_layout(80, 24, &cards, None).unwrap();
        let (wrapped_x, wrapped_y) = find_link(&compact);
        assert_eq!(
            clicked_body_url(
                &navigation,
                &app,
                Some(&compact),
                &cards,
                wrapped_x,
                wrapped_y
            )
            .unwrap(),
            "https://example.org/docs"
        );
        assert_eq!(
            clicked_body_url(&navigation, &app, None, &cards, x, y),
            None
        );
        app.begin_detail("abc", "ada", "see https://example.org/docs");
        assert_eq!(
            clicked_body_url(&navigation, &app, Some(&layout), &cards, x, y),
            None
        );
        navigation.input('g', &Keymap::default());
        navigation.input('n', &Keymap::default());
        app.finish_log_interaction();
        assert_eq!(
            clicked_body_url(&navigation, &app, Some(&layout), &cards, x, y),
            None
        );
    }

    #[test]
    fn wheel_stays_within_feed_and_does_not_change_focus_or_snap_back() {
        let cards: Vec<_> = (0..12)
            .map(|_| ui::LogCommit {
                author: "Ada",
                handle: "ada",
                time: "now",
                pending: false,
                hash: "abc",
                body: "hello",
                thread: None,
            })
            .collect();
        let layout = ui::card_feed_layout(80, 22, &cards, Some(0)).unwrap();
        let mut nav = Navigation::default();
        nav.sync_log_entries(cards.len());
        let (x, y) = (layout.viewport.x, layout.viewport.y);
        assert!(!wheel_feed(&mut nav, Some(&layout), x - 1, y, false));
        assert!(!wheel_feed(&mut nav, Some(&layout), x, y - 1, false));
        assert!(!wheel_feed(
            &mut nav,
            Some(&layout),
            x,
            layout.viewport.bottom(),
            false
        ));
        assert_eq!(nav.log_scroll(), 0);
        for _ in 0..100 {
            wheel_feed(&mut nav, Some(&layout), x, y, false);
        }
        assert_eq!(nav.log_scroll(), layout.clamp_scroll(u16::MAX));
        assert_eq!(nav.log_selection(), Some(0));
        reconcile_log_scroll(&mut nav, &layout, false);
        assert_eq!(nav.log_scroll(), layout.clamp_scroll(u16::MAX));
        for _ in 0..100 {
            wheel_feed(&mut nav, Some(&layout), x, y, true);
        }
        assert_eq!(nav.log_scroll(), 0);
        nav.handle_log_key_with_count('j', cards.len());
        reconcile_log_scroll(&mut nav, &layout, true);
        assert_eq!(nav.log_scroll(), layout.scroll_to_show(0, 1));
        assert_eq!(nav.log_selection(), Some(1));
        for _ in 0..4 {
            nav.handle_log_key_with_count('j', cards.len());
        }
        reconcile_log_scroll(&mut nav, &layout, true);
        assert_eq!(nav.log_scroll(), layout.scroll_to_show(0, 5));
        nav.input('g', &Keymap::default());
        wheel_feed(&mut nav, Some(&layout), x, y, false);
        assert_eq!(nav.log_scroll(), layout.scroll_to_show(0, 5));
        nav.clear_pending_prefix();
        nav.input('g', &Keymap::default());
        nav.input('n', &Keymap::default());
        assert!(!wheel_feed(&mut nav, Some(&layout), x, y, false));
        assert_eq!(nav.log_scroll(), layout.scroll_to_show(0, 5));

        let empty = ui::card_feed_layout(80, 22, &[], None).unwrap();
        let mut empty_nav = Navigation::default();
        assert!(wheel_feed(
            &mut empty_nav,
            Some(&empty),
            empty.viewport.x,
            empty.viewport.y,
            false
        ));
        assert_eq!(empty_nav.log_scroll(), 0);
        assert_eq!(empty_nav.log_selection(), None);
    }

    #[test]
    fn log_tabs_route_to_matching_signed_api_kinds() {
        assert_eq!(log_kind(LogTab::Following), LogKind::Following);
        assert_eq!(log_kind(LogTab::Latest), LogKind::Latest);
        assert_eq!(log_kind(LogTab::Trending), LogKind::Trending);
    }

    #[test]
    fn switched_tab_discards_old_response_and_uses_opaque_cursor() {
        let (requests, incoming) = mpsc::channel();
        let mut feed = FeedState::new(LogTab::Following);
        feed.request(&requests);
        let old = incoming.try_recv().unwrap();
        assert_eq!(old.cursor, None);
        assert!(feed.switch(LogTab::Latest));
        feed.request(&requests);
        let current = incoming.try_recv().unwrap();
        assert_eq!(current.tab, LogTab::Latest);
        assert!(!feed.accept(FeedResponse {
            epoch: old.epoch,
            tab: old.tab,
            result: Err("stale failure".to_owned()),
        }));
        assert!(feed.pending);
        assert!(feed.error.is_none());

        // Synthetic empty page tests cursor state only; runtime never inserts fixture commits.
        let page: LogPage = serde_json::from_value(serde_json::json!({
            "data": [], "log": "latest", "is_ranked": false, "suggestions": [],
            "links": { "first": null, "last": null, "prev": null, "next": null },
            "meta": { "path": "/api/v1/log/latest", "per_page": 20,
                      "next_cursor": "opaque+/=", "prev_cursor": null }
        }))
        .unwrap();
        assert!(feed.accept(FeedResponse {
            epoch: current.epoch,
            tab: current.tab,
            result: Ok(page),
        }));
        feed.request(&requests);
        assert_eq!(
            incoming.try_recv().unwrap().cursor.as_deref(),
            Some("opaque+/=")
        );
    }

    #[test]
    fn refresh_restarts_selected_tab_and_ignores_in_flight_response() {
        let (requests, incoming) = mpsc::channel();
        let mut nav = Navigation::default();
        nav.sync_log_entries(3);
        nav.handle_log_key_with_count('j', 3);
        nav.scroll_log(4, 10);
        let mut app = App::default();
        app.set_device_name_input("saved onboarding text");
        let mut feed = FeedState::new(LogTab::Following);
        feed.commits.push(commit());
        feed.next_cursor = Some("old cursor".into());
        feed.loaded = true;
        feed.request(&requests);
        let old = incoming.try_recv().unwrap();
        assert_eq!(old.cursor.as_deref(), Some("old cursor"));

        assert!(refresh_log(&mut nav, &app, &mut feed, Some(&requests)));
        let fresh = incoming.try_recv().unwrap();
        assert_eq!(fresh.tab, LogTab::Following);
        assert_eq!(fresh.cursor, None);
        assert_ne!(fresh.epoch, old.epoch);
        assert!(feed.commits.is_empty());
        assert_eq!(feed.next_cursor, None);
        assert_eq!(nav.log_selection(), None);
        assert_eq!(nav.log_scroll(), 0);
        assert_eq!(app.device_name_input(), "saved onboarding text");
        assert!(!feed.accept(FeedResponse {
            epoch: old.epoch,
            tab: old.tab,
            result: Err("stale failure".into()),
        }));
        assert!(feed.pending);
        assert!(feed.error.is_none());
        let stale_page: LogPage = serde_json::from_value(serde_json::json!({
            "data": [], "log": "following", "is_ranked": false, "suggestions": [],
            "links": { "first": null, "last": null, "prev": null, "next": null },
            "meta": { "path": "/api/v1/log/following", "per_page": 20,
                      "next_cursor": "stale cursor", "prev_cursor": null }
        }))
        .unwrap();
        assert!(!feed.accept(FeedResponse {
            epoch: old.epoch,
            tab: old.tab,
            result: Ok(stale_page),
        }));
        assert!(feed.commits.is_empty());
        assert_eq!(feed.next_cursor, None);
        assert!(feed.accept(FeedResponse {
            epoch: fresh.epoch,
            tab: fresh.tab,
            result: Err("current failure".into()),
        }));
        assert_eq!(feed.error.as_deref(), Some("current failure"));
    }

    #[test]
    fn refresh_retries_failed_page_from_first_cursor() {
        let (requests, incoming) = mpsc::channel();
        let mut nav = Navigation::default();
        let app = App::default();
        let mut feed = FeedState::new(LogTab::Following);
        feed.loaded = true;
        feed.next_cursor = Some("failed page cursor".into());
        feed.request(&requests);
        let failed = incoming.try_recv().unwrap();
        assert!(feed.accept(FeedResponse {
            epoch: failed.epoch,
            tab: failed.tab,
            result: Err("page failed".into()),
        }));
        assert_eq!(feed.next_cursor.as_deref(), Some("failed page cursor"));
        assert!(refresh_log(&mut nav, &app, &mut feed, Some(&requests)));
        let retry = incoming.try_recv().unwrap();
        assert_eq!(retry.cursor, None);
        assert_ne!(retry.epoch, failed.epoch);
        assert!(feed.error.is_none());
        assert!(feed.pending);
    }

    #[test]
    fn refresh_ignores_dialogs_and_non_log_pages() {
        let (requests, incoming) = mpsc::channel();
        let mut nav = Navigation::default();
        let mut app = App::default();
        let mut feed = FeedState::new(LogTab::Following);
        app.begin_compose(ComposeKind::Reply, "id", "ada");
        assert!(!refresh_log(&mut nav, &app, &mut feed, Some(&requests)));
        assert!(matches!(
            app.log_interaction(),
            Some(LogInteraction::Compose { .. })
        ));
        app.begin_report("id", "ada");
        assert!(!refresh_log(&mut nav, &app, &mut feed, Some(&requests)));
        assert!(matches!(
            app.log_interaction(),
            Some(LogInteraction::Report { .. })
        ));
        assert_eq!(feed.epoch, 0);
        assert!(incoming.try_recv().is_err());
        app.finish_log_interaction();
        nav.input('g', &Keymap::default());
        nav.input('n', &Keymap::default());
        assert_eq!(nav.current_page(), Page::Notifications);
        assert!(!refresh_log(&mut nav, &app, &mut feed, Some(&requests)));
        assert_eq!(feed.epoch, 0);
        assert!(incoming.try_recv().is_err());
    }

    #[test]
    fn signed_worker_routes_each_request_and_typed_result_without_network() {
        let (requests, incoming) = mpsc::channel();
        let (outgoing, responses) = mpsc::channel();
        let worker = std::thread::spawn(move || {
            route_signed_requests(incoming, outgoing, |request| match request {
                SignedRequest::Report { .. } => Ok(SignedResult::Reported),
                SignedRequest::Detail { .. } => anyhow::bail!("detail failed"),
                SignedRequest::Star { .. } => anyhow::bail!("star failed"),
                SignedRequest::Unstar { .. } => anyhow::bail!("unstar failed"),
                SignedRequest::Reply { .. } => anyhow::bail!("reply failed"),
                SignedRequest::Quote { .. } => anyhow::bail!("quote failed"),
                SignedRequest::Pastel { .. } => anyhow::bail!("pastel failed"),
            });
        });
        let operations = [
            SignedRequest::Detail {
                id: "detail".into(),
            },
            SignedRequest::Star { id: "star".into() },
            SignedRequest::Unstar {
                id: "unstar".into(),
            },
            SignedRequest::Reply {
                id: "reply".into(),
                body: "text".into(),
            },
            SignedRequest::Quote {
                id: "quote".into(),
                body: "quote text".into(),
            },
            SignedRequest::Report {
                id: "report".into(),
                reason: ReportReason::Spam,
            },
            SignedRequest::Pastel {
                id: "pastel".into(),
            },
        ];
        for request in operations {
            requests.send(request).unwrap();
        }
        drop(requests);
        let mut results = Vec::new();
        for _ in 0..7 {
            results.push(
                responses
                    .recv_timeout(std::time::Duration::from_secs(1))
                    .unwrap(),
            );
        }
        worker.join().unwrap();
        assert!(matches!(&results[0].request, SignedRequest::Detail { id } if id == "detail"));
        for (index, id) in [(1, "star"), (2, "unstar"), (3, "reply")] {
            let response = &results[index];
            let actual = match &response.request {
                SignedRequest::Star { id }
                | SignedRequest::Unstar { id }
                | SignedRequest::Reply { id, .. } => id,
                _ => panic!("wrong request"),
            };
            assert_eq!(actual, id);
            assert_eq!(
                response.result.as_ref().unwrap_err().to_string(),
                format!("{id} failed")
            );
        }
        assert!(matches!(&results[3].request, SignedRequest::Reply { body, .. } if body == "text"));
        assert!(
            matches!(&results[4].request, SignedRequest::Quote { id, body } if id == "quote" && body == "quote text")
        );
        assert_eq!(
            results[4].result.as_ref().unwrap_err().to_string(),
            "quote failed"
        );
        assert_eq!(
            results[0].result.as_ref().unwrap_err().to_string(),
            "detail failed"
        );
        assert!(
            matches!(&results[5].request, SignedRequest::Report { id, reason: ReportReason::Spam } if id == "report")
        );
        assert!(matches!(results[5].result, Ok(SignedResult::Reported)));
        assert!(matches!(&results[6].request, SignedRequest::Pastel { id } if id == "pastel"));
        assert_eq!(
            results[6].result.as_ref().unwrap_err().to_string(),
            "pastel failed"
        );
    }

    #[test]
    fn signed_star_response_replaces_matching_feed_card_only() {
        let mut feed = FeedState::new(LogTab::Latest);
        feed.commits.push(commit());
        let mut app = App::default();
        let mut updated = commit();
        updated.starred = true;
        updated.stars_count = 1;
        apply_signed_response(
            SignedResponse {
                request: SignedRequest::Star {
                    id: "commit-id".into(),
                },
                result: Ok(SignedResult::Commit(updated)),
            },
            &mut app,
            &mut feed,
        );
        assert!(feed.commits[0].starred);
        assert_eq!(feed.commits[0].stars_count, 1);
        apply_signed_response(
            SignedResponse {
                request: SignedRequest::Unstar {
                    id: "commit-id".into(),
                },
                result: Ok(SignedResult::Commit({
                    let mut other = commit();
                    other.id = "other".into();
                    other
                })),
            },
            &mut app,
            &mut feed,
        );
        assert!(feed.commits[0].starred);
        assert!(app.message().unwrap().contains("different commit"));
    }

    #[test]
    fn confirmed_drafts_survive_failure_and_only_matching_success_closes_them() {
        let mut app = App::default();
        let mut feed = FeedState::new(LogTab::Latest);
        app.begin_compose(ComposeKind::Reply, "commit-id", "alice");
        app.update_log(Action::Paste("draft".into()));
        let request = SignedRequest::Reply {
            id: "commit-id".into(),
            body: "draft".into(),
        };
        assert!(matches!(
            app.update_log(Action::LogEnter),
            Some(crate::app::LogIntent::Commit { .. })
        ));
        apply_signed_response(
            SignedResponse {
                request: request.clone(),
                result: Err(anyhow::anyhow!("offline")),
            },
            &mut app,
            &mut feed,
        );
        assert!(
            matches!(app.log_interaction(), Some(LogInteraction::Compose { body, error: Some(_), .. }) if body == "draft")
        );
        app.update_log(Action::Paste(" edited".into()));
        apply_signed_response(
            SignedResponse {
                request,
                result: Ok(SignedResult::Commit(commit())),
            },
            &mut app,
            &mut feed,
        );
        assert!(
            matches!(app.log_interaction(), Some(LogInteraction::Compose { body, .. }) if body == "draft edited")
        );
        assert!(!app.should_quit());
        app.update_log(Action::LogShortcut('q'));
        assert!(
            matches!(app.log_interaction(), Some(LogInteraction::Compose { body, .. }) if body.ends_with('q'))
        );
        app.update_log(Action::SetMode(Mode::Normal));
        assert!(app.log_interaction().is_none());
    }

    #[test]
    fn signed_worker_never_creates_missing_device_key() {
        let dir = env::temp_dir().join(format!(
            "stdout-no-signed-key-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        std::fs::create_dir_all(&dir).unwrap();
        let config = crate::config::Config {
            api_origin: url::Url::parse("https://example.test/").unwrap(),
            dir: dir.clone(),
            keymap: Keymap::default(),
        };
        let (requests, responses) = super::start_signed_worker(config);
        requests
            .send(SignedRequest::Detail { id: "id".into() })
            .unwrap();
        assert!(responses
            .recv_timeout(std::time::Duration::from_secs(1))
            .unwrap()
            .result
            .unwrap_err()
            .to_string()
            .contains("approved device key is missing"));
        assert!(!dir.join("device_ed25519").exists());
        std::fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn editor_invocation_keeps_config_path_as_one_argument_without_a_shell() {
        let path = std::path::Path::new("/tmp/config directory/config.toml");
        let (program, args) = editor_invocation("nvim --clean", path).unwrap();
        assert_eq!(program, "nvim");
        assert_eq!(args[0], "--clean");
        assert_eq!(args[1], path.as_os_str());
        assert!(editor_invocation("  ", path).is_err());
    }

    #[test]
    fn config_shortcut_creates_private_template_without_overwriting_existing_file() {
        let dir = env::temp_dir().join(format!(
            "stdout-config-shortcut-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        std::fs::create_dir(&dir).unwrap();
        let path = dir.join("config.toml");
        prepare_config_file(&path).unwrap();
        assert!(std::fs::read_to_string(&path)
            .unwrap()
            .contains("# config = \"g ,\""));
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            assert_eq!(
                std::fs::metadata(&path).unwrap().permissions().mode() & 0o777,
                0o600
            );
        }
        std::fs::write(&path, "api_origin = \"https://example.test\"\n").unwrap();
        prepare_config_file(&path).unwrap();
        assert_eq!(
            std::fs::read_to_string(&path).unwrap(),
            "api_origin = \"https://example.test\"\n"
        );
        std::fs::remove_dir_all(&dir).unwrap();
    }

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
