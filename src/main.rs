mod api;
mod callback;

use api::{
    Action, Api, AuthFlow, Commit, Entry, Lane, Notice, Page, Pulse, RequestLog, Screen, User,
};
use crossterm::{
    event::{
        self, DisableBracketedPaste, DisableMouseCapture, EnableBracketedPaste, EnableMouseCapture,
        Event, KeyCode, KeyEvent, KeyEventKind, KeyModifiers, MouseButton, MouseEventKind,
    },
    execute,
    terminal::{disable_raw_mode, enable_raw_mode, EnterAlternateScreen, LeaveAlternateScreen},
};
use ratatui::{
    backend::CrosstermBackend,
    layout::{Constraint, Direction, Layout, Rect},
    style::{Color, Modifier, Style},
    text::{Line, Span, Text},
    widgets::{Block, Borders, Clear, List, ListItem, ListState, Paragraph, Wrap},
    Frame, Terminal,
};
use reqwest::Url;
use std::{
    cell::RefCell,
    collections::HashMap,
    io::{self, Read, Write},
    os::unix::net::UnixListener,
    path::PathBuf,
    process::Command,
    sync::{
        atomic::{AtomicBool, Ordering},
        mpsc::{self, Receiver, Sender},
    },
    thread,
    time::{Duration, Instant},
};
use time::{format_description::well_known::Rfc3339, OffsetDateTime};

// Terminal defaults + user-configured ANSI palette work on light, dark and transparent themes.
const MUTED: Color = Color::DarkGray;
const BG: Color = Color::Reset;
const PANEL: Color = Color::Reset;
static LIGHT_THEME: AtomicBool = AtomicBool::new(false);

fn is_light_theme() -> bool {
    LIGHT_THEME.load(Ordering::Relaxed)
}
fn web_color(dark: u32, light: u32) -> Color {
    let hex = if is_light_theme() { light } else { dark };
    Color::Rgb((hex >> 16) as u8, (hex >> 8) as u8, hex as u8)
}
fn primary_color() -> Color {
    web_color(0x7aa2f7, 0x3b6fe0)
}
fn brand_color() -> Color {
    web_color(0xff8a4c, 0xd9480f)
}
fn star_color() -> Color {
    web_color(0xfcc419, 0xd69e0b)
}
fn lane_color_index(index: u8) -> Color {
    match index % 6 {
        0 => web_color(0xff8a4c, 0xd9480f),
        1 => web_color(0x51cf66, 0x2f9e44),
        2 => web_color(0xda77f2, 0x9c36b5),
        3 => web_color(0x3bc9db, 0x0c8599),
        4 => web_color(0xf06595, 0xc2255c),
        _ => web_color(0xfcc419, 0xb7791f),
    }
}

fn parse_terminal_background(answer: &[u8]) -> Option<bool> {
    let text = String::from_utf8_lossy(answer);
    let start = text.find("]11;rgb:")? + 8;
    let end = text[start..]
        .find(|ch: char| !(ch.is_ascii_hexdigit() || ch == '/'))
        .map(|n| start + n)
        .unwrap_or(text.len());
    let channel = |value: &str| -> Option<f32> {
        let maximum = 16u32.checked_pow(value.len() as u32)?.saturating_sub(1) as f32;
        (maximum > 0.0).then_some(u32::from_str_radix(value, 16).ok()? as f32 / maximum)
    };
    let mut channels = text[start..end].split('/').map(channel);
    let (Some(Some(red)), Some(Some(green)), Some(Some(blue))) =
        (channels.next(), channels.next(), channels.next())
    else {
        return None;
    };
    Some(0.2126 * red + 0.7152 * green + 0.0722 * blue > 0.5)
}

fn detect_light_theme() -> bool {
    if let Some(background) = std::env::var("COLORFGBG")
        .ok()
        .and_then(|value| value.rsplit(';').next()?.parse::<u8>().ok())
    {
        return background >= 7;
    }
    #[cfg(unix)]
    {
        use std::os::fd::AsRawFd;
        let Ok(mut tty) = std::fs::OpenOptions::new()
            .read(true)
            .write(true)
            .open("/dev/tty")
        else {
            return false;
        };
        if tty.write_all(b"\x1b]11;?\x1b\\\x1b[c").is_err() || tty.flush().is_err() {
            return false;
        }
        let deadline = Instant::now() + Duration::from_millis(350);
        let mut answer = Vec::new();
        while !answer.windows(3).any(|window| window == b"\x1b[?") || !answer.contains(&b'c') {
            let left = deadline.saturating_duration_since(Instant::now());
            if left.is_zero() {
                break;
            }
            let mut descriptor = libc::pollfd {
                fd: tty.as_raw_fd(),
                events: libc::POLLIN,
                revents: 0,
            };
            if unsafe {
                libc::poll(
                    &mut descriptor,
                    1,
                    left.as_millis().min(i32::MAX as u128) as i32,
                )
            } <= 0
            {
                break;
            }
            let mut chunk = [0u8; 256];
            match tty.read(&mut chunk) {
                Ok(n) if n > 0 => answer.extend_from_slice(&chunk[..n]),
                _ => break,
            }
        }
        if let Some(light) = parse_terminal_background(&answer) {
            return light;
        }
    }
    false
}

const COMMANDS: [(&str, &str); 21] = [
    ("Latest log", "latest"),
    ("Trending log", "trending"),
    ("Following log", "following"),
    ("Notifications", "notifications"),
    ("Starred commits", "starred"),
    ("Star commit", "star"),
    ("Unstar commit", "unstar"),
    ("My profile", "profile"),
    ("Settings", "settings"),
    ("Search developers and commits", "search_slash"),
    ("Command center", "commands"),
    ("Selected author profile", "go_author"),
    ("Write commit", "compose"),
    ("Find user profile", "find_user"),
    ("Sign in", "login"),
    ("Refresh", "refresh"),
    ("Muted users", "muted"),
    ("Blocked users", "blocked"),
    ("Keyboard help", "help"),
    ("View API request log", "request_log"),
    ("Quit", "quit"),
];

fn help_text(app: &App, width: usize) -> String {
    let key = |name| app.keymap.label(name);
    let key_width = 20.min(width.saturating_sub(2));
    let command_width = width.saturating_sub(key_width + 2);
    let mut rows = Vec::new();
    let mut row = |command: &str, hotkey: String| {
        rows.push(format!("  {command:<command_width$}{hotkey:>key_width$}"));
    };
    row("Move down", key("move_down"));
    row("Move up", key("move_up"));
    row("Move focus up", "↑".into());
    row("Move focus down", "↓".into());
    row("Switch panes left", key("pane_left"));
    row("Switch panes right", key("pane_right"));
    row("Cycle panes forward", key("focus_next"));
    row("Cycle panes backward", key("focus_previous"));
    row("Move half a page down", key("half_page_down"));
    row("Move half a page up", key("half_page_up"));
    row("Latest Log (Log only)", "1".into());
    row("Trending Log (Log only)", "2".into());
    row("Following Log (Log only)", "3".into());
    row("Open selected item", key("open"));
    row("Open item on web", key("open_web"));
    row("Search developers / commits", key("search_ctrl"));
    row("Search on the web", key("search_slash"));
    row("Command center", key("commands"));
    row("Keyboard help", key("help"));
    row("Jump to Latest Log", key("go_latest"));
    row("Open Starred", key("go_starred"));
    row("Open Starred", key("starred"));
    row("Open my profile", key("go_profile"));
    row("Open own profile", key("profile"));
    row("Selected author profile", key("go_author"));
    row("Settings", key("settings"));
    row("Notifications", key("notifications"));
    row("Find user", key("find_user"));
    row("Sign in", key("login"));
    row("Write commit", key("compose"));
    row("Reply", key("reply"));
    row("Quote", key("quote"));
    row("Expand / collapse replies", key("expand_replies"));
    row("Load more replies", key("more_replies"));
    row("Star commit", key("star"));
    row("Unstar commit", key("unstar"));
    row("Copy link", key("copy_link"));
    row("Follow user", key("follow"));
    row("Mute user", key("mute"));
    row("Block user", key("block"));
    row("Refresh", key("refresh"));
    row("Submit commit", key("submit"));
    row("Preview commit", key("preview"));
    row("Insert newline", "Alt+Enter".into());
    row("Back / cancel", "Esc".into());
    row("Quit", key("quit"));
    row("Quit immediately", "Ctrl+C".into());
    row("First item", key("first_item"));
    row("First item via chord", key("first_item_chord"));
    row("Last item", key("last_item"));
    row("Last item", key("last_item_end"));
    row("Next page", key("next_page"));
    row("Page down", key("page_down"));
    row("Page up", key("page_up"));
    row("Profile commits tab", "a".into());
    row("Profile replies tab", "e".into());
    row("Followers", key("followers"));
    row("Following", key("following_list"));
    if app.request_log.is_some() {
        row("API request log popup", "Ctrl+Shift+L".into());
        row("Request log fallback", "Ctrl+L".into());
    }
    rows.join("\n")
}

fn mark(selected: bool) -> &'static str {
    if selected {
        "▶"
    } else {
        " "
    }
}

struct Keymap {
    overrides: std::collections::HashMap<String, KeyEvent>,
    chords: std::collections::HashMap<String, (KeyEvent, KeyEvent)>,
}

impl Keymap {
    fn defaults() -> Vec<(&'static str, KeyEvent)> {
        use KeyCode::{
            BackTab, Char, Down, End, Enter, Esc, Home, Left, PageDown, PageUp, Right, Tab, Up,
        };
        const NONE: KeyModifiers = KeyModifiers::NONE;
        const CONTROL: KeyModifiers = KeyModifiers::CONTROL;
        const SUPER: KeyModifiers = KeyModifiers::SUPER;
        [
            ("move_down", KeyEvent::new(Char('j'), NONE)),
            ("move_up", KeyEvent::new(Char('k'), NONE)),
            ("pane_left", KeyEvent::new(Char('h'), NONE)),
            ("pane_right", KeyEvent::new(Char('l'), NONE)),
            ("focus_next", KeyEvent::new(Tab, NONE)),
            (
                "focus_previous",
                KeyEvent::new(BackTab, KeyModifiers::SHIFT),
            ),
            ("half_page_down", KeyEvent::new(Char('d'), CONTROL)),
            ("half_page_up", KeyEvent::new(Char('u'), CONTROL)),
            ("first_item", KeyEvent::new(Home, NONE)),
            ("last_item", KeyEvent::new(Char('G'), NONE)),
            ("latest", KeyEvent::new(Char('1'), NONE)),
            ("trending", KeyEvent::new(Char('2'), NONE)),
            ("following", KeyEvent::new(Char('3'), NONE)),
            ("notifications", KeyEvent::new(Char('N'), NONE)),
            ("starred", KeyEvent::new(Char('b'), NONE)),
            ("profile", KeyEvent::new(Char('p'), NONE)),
            ("muted", KeyEvent::new(Char('M'), NONE)),
            ("blocked", KeyEvent::new(Char('X'), NONE)),
            ("settings", KeyEvent::new(Char('s'), KeyModifiers::ALT)),
            ("find_user", KeyEvent::new(Char('u'), KeyModifiers::ALT)),
            ("search_ctrl", KeyEvent::new(Char('k'), CONTROL)),
            ("search_slash", KeyEvent::new(Char('/'), NONE)),
            ("login", KeyEvent::new(Char('L'), NONE)),
            ("refresh", KeyEvent::new(Char('R'), NONE)),
            ("next_page", KeyEvent::new(Char('n'), NONE)),
            ("open", KeyEvent::new(Enter, NONE)),
            ("expand_replies", KeyEvent::new(Char(' '), NONE)),
            ("more_replies", KeyEvent::new(Char(']'), NONE)),
            ("copy_link", KeyEvent::new(Char('y'), NONE)),
            ("quit", KeyEvent::new(Char('q'), NONE)),
            ("help", KeyEvent::new(Char('?'), NONE)),
            ("commands", KeyEvent::new(Char(':'), NONE)),
            ("compose", KeyEvent::new(Char('c'), NONE)),
            ("reply", KeyEvent::new(Char('r'), NONE)),
            ("quote", KeyEvent::new(Char('Q'), NONE)),
            ("star", KeyEvent::new(Char('s'), NONE)),
            ("unstar", KeyEvent::new(Char('S'), NONE)),
            ("follow", KeyEvent::new(Char('f'), NONE)),
            ("mute", KeyEvent::new(Char('m'), NONE)),
            ("block", KeyEvent::new(Char('B'), NONE)),
            ("pastel", KeyEvent::new(Char('t'), NONE)),
            ("report", KeyEvent::new(Char('!'), NONE)),
            ("followers", KeyEvent::new(Char('u'), NONE)),
            ("following_list", KeyEvent::new(Char('i'), NONE)),
            ("open_web", KeyEvent::new(Char('o'), NONE)),
            (
                "preview",
                KeyEvent::new(Char('p'), KeyModifiers::SUPER | KeyModifiers::SHIFT),
            ),
            ("submit", KeyEvent::new(Char('k'), SUPER)),
            ("back", KeyEvent::new(Esc, NONE)),
            ("page_down", KeyEvent::new(PageDown, NONE)),
            ("page_up", KeyEvent::new(PageUp, NONE)),
            ("left_arrow", KeyEvent::new(Left, NONE)),
            ("right_arrow", KeyEvent::new(Right, NONE)),
            ("up_arrow", KeyEvent::new(Up, NONE)),
            ("down_arrow", KeyEvent::new(Down, NONE)),
            ("last_item_end", KeyEvent::new(End, NONE)),
        ]
        .into()
    }

    fn chord_defaults() -> Vec<(&'static str, (KeyEvent, KeyEvent))> {
        let key = |ch| KeyEvent::new(KeyCode::Char(ch), KeyModifiers::NONE);
        [
            ("go_latest", (key('g'), key('l'))),
            ("go_starred", (key('g'), key('s'))),
            ("go_profile", (key('g'), key('p'))),
            ("go_author", (key('g'), key('P'))),
            ("first_item_chord", (key('g'), key('g'))),
        ]
        .into()
    }

    fn load(path: &std::path::Path) -> (Self, Option<String>) {
        let defaults = Self::defaults();
        let chord_defaults = Self::chord_defaults();
        let fallback = || Self {
            overrides: Default::default(),
            chords: Default::default(),
        };
        let raw = match std::fs::read_to_string(path) {
            Ok(raw) => raw,
            Err(error) if error.kind() == io::ErrorKind::NotFound => return (fallback(), None),
            Err(error) => return (fallback(), Some(format!("Keymap: {error}"))),
        };
        let config: toml::Value = match toml::from_str(&raw) {
            Ok(config) => config,
            Err(error) => return (fallback(), Some(format!("Keymap: {error}"))),
        };
        let Some(table) = config.get("keymap").and_then(toml::Value::as_table) else {
            return (fallback(), Some("Keymap: missing [keymap] table".into()));
        };
        let mut overrides = std::collections::HashMap::new();
        let mut chords = std::collections::HashMap::new();
        for (name, value) in table {
            let Some(value) = value.as_str() else {
                return (
                    fallback(),
                    Some(format!("Keymap: `{name}` must be a quoted key")),
                );
            };
            if defaults.iter().any(|(key, _)| *key == name) {
                let Some(key) = parse_key(value) else {
                    return (fallback(), Some(format!("Keymap: invalid key `{value}`")));
                };
                overrides.insert(name.clone(), key);
            } else if chord_defaults.iter().any(|(key, _)| *key == name) {
                let Some(chord) = parse_chord(value) else {
                    return (
                        fallback(),
                        Some(format!("Keymap: `{value}` must be a two-key chord")),
                    );
                };
                chords.insert(name.clone(), chord);
            } else {
                return (fallback(), Some(format!("Keymap: unknown action `{name}`")));
            }
        }
        let keymap = Self { overrides, chords };
        let effective: Vec<_> = defaults
            .iter()
            .map(|(name, default)| (name, keymap.key(name, *default)))
            .collect();
        for (index, (name, key)) in effective.iter().enumerate() {
            if effective[index + 1..]
                .iter()
                .any(|(_, other)| same_key(*key, *other))
            {
                return (
                    fallback(),
                    Some(format!("Keymap: `{name}` conflicts with another binding")),
                );
            }
        }
        let effective_chords: Vec<_> = chord_defaults
            .iter()
            .map(|(name, default)| (name, keymap.chords.get(*name).copied().unwrap_or(*default)))
            .collect();
        for (index, (name, (first, second))) in effective_chords.iter().enumerate() {
            if effective_chords[index + 1..]
                .iter()
                .any(|(_, other)| same_key(*first, other.0) && same_key(*second, other.1))
                || defaults
                    .iter()
                    .any(|(binding, default)| same_key(keymap.key(binding, *default), *first))
            {
                return (
                    fallback(),
                    Some(format!(
                        "Keymap: chord `{name}` conflicts with another binding"
                    )),
                );
            }
        }
        (keymap, None)
    }

    fn key(&self, name: &str, default: KeyEvent) -> KeyEvent {
        self.overrides.get(name).copied().unwrap_or(default)
    }

    fn normalize(&self, pressed: KeyEvent) -> Option<KeyEvent> {
        let defaults = Self::defaults();
        if let Some((_, default)) = defaults
            .iter()
            .find(|(name, default)| same_key(self.key(name, *default), pressed))
        {
            return Some(*default);
        }
        if defaults
            .iter()
            .any(|(_, default)| same_key(*default, pressed))
        {
            None
        } else {
            Some(pressed)
        }
    }

    fn matches(&self, name: &str, pressed: KeyEvent) -> bool {
        self.event(name).is_some_and(|binding| same_key(binding, pressed)
            || (binding.modifiers.contains(KeyModifiers::SHIFT)
                && matches!((binding.code, pressed.code), (KeyCode::Char(a), KeyCode::Char(b)) if a.eq_ignore_ascii_case(&b))
                && same_modifiers(binding, pressed)))
    }

    fn event(&self, name: &str) -> Option<KeyEvent> {
        Self::defaults()
            .into_iter()
            .find(|(key, _)| *key == name)
            .map(|(name, key)| self.key(name, key))
    }

    fn chord(&self, name: &str) -> Option<(KeyEvent, KeyEvent)> {
        Self::chord_defaults()
            .into_iter()
            .find(|(key, _)| *key == name)
            .map(|(name, default)| self.chords.get(name).copied().unwrap_or(default))
    }

    fn chord_action(&self, first: KeyEvent, second: KeyEvent) -> Option<&'static str> {
        Self::chord_defaults()
            .into_iter()
            .find(|(name, default)| {
                let chord = self.chords.get(*name).copied().unwrap_or(*default);
                same_key(chord.0, first) && same_key(chord.1, second)
            })
            .map(|(name, _)| name)
    }

    fn starts_chord(&self, pressed: KeyEvent) -> bool {
        Self::chord_defaults().iter().any(|(name, default)| {
            let chord = self.chords.get(*name).copied().unwrap_or(*default);
            same_key(chord.0, pressed)
        })
    }

    fn label(&self, name: &str) -> String {
        let defaults = Self::defaults();
        if let Some((name, key)) = defaults.iter().find(|(key, _)| *key == name) {
            return key_label(self.key(name, *key));
        }
        self.chord(name)
            .map(|(first, second)| format!("{} {}", key_label(first), key_label(second)))
            .unwrap_or_else(|| "?".into())
    }
}

fn same_key(a: KeyEvent, b: KeyEvent) -> bool {
    a.code == b.code && same_modifiers(a, b)
}

fn same_modifiers(a: KeyEvent, b: KeyEvent) -> bool {
    a.modifiers
        .intersection(KeyModifiers::CONTROL | KeyModifiers::ALT | KeyModifiers::SUPER)
        == b.modifiers
            .intersection(KeyModifiers::CONTROL | KeyModifiers::ALT | KeyModifiers::SUPER)
}

fn is_request_log_key(key: KeyEvent) -> bool {
    matches!(key.code, KeyCode::Char('l' | 'L'))
        && key.modifiers.contains(KeyModifiers::CONTROL)
        && !key
            .modifiers
            .intersects(KeyModifiers::ALT | KeyModifiers::SUPER)
}

fn key_label(key: KeyEvent) -> String {
    match key.code {
        KeyCode::Char(' ') => "Space".into(),
        KeyCode::Char(ch) if key.modifiers.contains(KeyModifiers::SUPER) => {
            let shift = if key.modifiers.contains(KeyModifiers::SHIFT) {
                "⇧"
            } else {
                ""
            };
            if cfg!(target_os = "macos") {
                format!("⌘{shift}{}", ch.to_ascii_uppercase())
            } else {
                format!("Super+{shift}{}", ch.to_ascii_uppercase())
            }
        }
        KeyCode::Char(ch) if key.modifiers.contains(KeyModifiers::CONTROL) => {
            let shift = if key.modifiers.contains(KeyModifiers::SHIFT) {
                "Shift+"
            } else {
                ""
            };
            format!("Ctrl+{shift}{}", ch.to_ascii_uppercase())
        }
        KeyCode::Char(ch) if key.modifiers.contains(KeyModifiers::ALT) => {
            let shift = if key.modifiers.contains(KeyModifiers::SHIFT) {
                "Shift+"
            } else {
                ""
            };
            format!("Alt+{shift}{}", ch.to_ascii_uppercase())
        }
        KeyCode::Enter if key.modifiers.contains(KeyModifiers::SUPER) => "⌘Enter".into(),
        KeyCode::Enter => "Enter".into(),
        KeyCode::Esc => "Esc".into(),
        KeyCode::Tab => "Tab".into(),
        KeyCode::BackTab => "Shift+Tab".into(),
        KeyCode::Up => "↑".into(),
        KeyCode::Down => "↓".into(),
        KeyCode::Left => "←".into(),
        KeyCode::Right => "→".into(),
        KeyCode::PageUp => "PgUp".into(),
        KeyCode::PageDown => "PgDn".into(),
        KeyCode::Home => "Home".into(),
        KeyCode::End => "End".into(),
        KeyCode::Char(ch) => ch.to_string(),
        _ => "?".into(),
    }
}

fn parse_key(value: &str) -> Option<KeyEvent> {
    let value = value.trim();
    if value.contains('+') {
        let mut modifiers = KeyModifiers::NONE;
        let mut parts: Vec<_> = value.split('+').collect();
        let key = parts.pop()?;
        for modifier in parts {
            modifiers |= match modifier.to_ascii_lowercase().as_str() {
                "ctrl" | "control" => KeyModifiers::CONTROL,
                "alt" => KeyModifiers::ALT,
                "cmd" | "command" | "super" => KeyModifiers::SUPER,
                "shift" => KeyModifiers::SHIFT,
                _ => return None,
            };
        }
        if key.eq_ignore_ascii_case("tab") && modifiers.contains(KeyModifiers::SHIFT) {
            return Some(KeyEvent::new(KeyCode::BackTab, modifiers));
        }
        let code = match key.to_ascii_lowercase().as_str() {
            "enter" | "return" => KeyCode::Enter,
            "space" => KeyCode::Char(' '),
            _ if key.chars().count() == 1 => {
                KeyCode::Char(key.chars().next()?.to_ascii_lowercase())
            }
            _ => return None,
        };
        return Some(KeyEvent::new(code, modifiers));
    }
    let code = match value.to_ascii_lowercase().as_str() {
        "space" => KeyCode::Char(' '),
        "enter" | "return" => KeyCode::Enter,
        "esc" | "escape" => KeyCode::Esc,
        "tab" => KeyCode::Tab,
        "backtab" => KeyCode::BackTab,
        "up" => KeyCode::Up,
        "down" => KeyCode::Down,
        "left" => KeyCode::Left,
        "right" => KeyCode::Right,
        "home" => KeyCode::Home,
        "end" => KeyCode::End,
        "pageup" | "pgup" => KeyCode::PageUp,
        "pagedown" | "pgdn" => KeyCode::PageDown,
        _ if value.chars().count() == 1 => KeyCode::Char(value.chars().next()?),
        _ => return None,
    };
    Some(KeyEvent::new(code, KeyModifiers::NONE))
}

fn parse_chord(value: &str) -> Option<(KeyEvent, KeyEvent)> {
    let mut keys = value.split_whitespace();
    let first = parse_key(keys.next()?)?;
    let second = parse_key(keys.next()?)?;
    keys.next().is_none().then_some((first, second))
}

fn commands(query: &str, app: &App) -> Vec<(&'static str, &'static str)> {
    let query = query.to_lowercase();
    COMMANDS
        .iter()
        .copied()
        .filter(|(name, action)| {
            name.to_lowercase().contains(&query)
                && (app.me.is_some()
                    || !matches!(
                        *action,
                        "following"
                            | "notifications"
                            | "starred"
                            | "profile"
                            | "settings"
                            | "compose"
                            | "muted"
                            | "blocked"
                    ))
                && (*action != "login" || app.me.is_none())
                && (*action != "request_log" || app.request_log.is_some())
                && (*action != "go_author" || app.selected_author().is_some())
                && (*action != "star"
                    || app
                        .commit()
                        .is_some_and(|commit| commit.starred != Some(true)))
                && (*action != "unstar"
                    || app
                        .commit()
                        .is_some_and(|commit| commit.starred == Some(true)))
        })
        .collect()
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Pane {
    Nav,
    Feed,
}

fn entry_commit(entry: &Entry) -> Option<&Commit> {
    match entry {
        Entry::Commit(commit) => Some(commit.as_ref()),
        Entry::Notice(notice) => notice.commit.as_ref(),
        Entry::User(_) => None,
    }
}

fn pane(title: impl Into<String>, focused: bool, accent: Color) -> Block<'static> {
    let block = panel(title);
    if focused {
        block.border_style(Style::default().fg(accent).add_modifier(Modifier::BOLD))
    } else {
        block
    }
}

fn panel(title: impl Into<String>) -> Block<'static> {
    Block::default()
        .title(format!("  {}  ", title.into()))
        .borders(Borders::ALL)
        .border_style(Style::default().fg(MUTED))
        .style(Style::default().bg(PANEL).fg(Color::Reset))
}

enum Job {
    Bootstrap,
    Load(Screen, Option<String>),
    SelfProfile(String),
    Pulse(String, Option<(&'static str, String)>),
    Replies(String, Option<String>, u64),
    Act(Screen, Action),
    Redeem(String, String),
    InstallHandler,
}

enum Reply {
    Boot(Result<Option<User>, String>),
    Loaded(Screen, bool, Result<Page, String>),
    SelfProfile(Result<User, String>),
    Pulse(
        String,
        Option<(&'static str, String)>,
        Result<Pulse, String>,
    ),
    Replies(String, u64, Result<Page, String>),
    Acted(Screen, Result<(String, Option<User>), String>),
    Redeemed(Result<User, String>),
    HandlerInstalled(Result<(), String>),
}

enum Mode {
    Normal,
    Search(String),
    WebSearch(String),
    Compose {
        text: String,
        parent: Option<String>,
        quote: Option<String>,
    },
    ComposePreview {
        text: String,
        parent: Option<String>,
        quote: Option<String>,
    },
    LoginSetup {
        flow: AuthFlow,
    },
    Login {
        flow: AuthFlow,
        input: String,
    },
    Palette {
        query: String,
        selected: usize,
    },
    RequestLog {
        offset: usize,
        return_to: Box<Mode>,
    },
    Report(String),
    Block {
        username: String,
        remove: bool,
    },
    Settings {
        selected: usize,
    },
    CustomColor(String),
    Help {
        offset: usize,
    },
}

fn user_avatar(user: Option<&User>) -> Vec<Span<'static>> {
    let Some(user) = user else {
        return vec![Span::raw("  ")];
    };
    let name = user
        .name
        .as_deref()
        .filter(|name| !name.trim().is_empty())
        .unwrap_or(&user.username);
    let initials = name.chars().take(2).collect::<String>().to_uppercase();
    vec![Span::styled(
        format!("{initials:<2}"),
        Style::default()
            .fg(lane_color(Some(user)))
            .add_modifier(Modifier::BOLD),
    )]
}

struct LinkRegion {
    area: Rect,
    url: String,
}

struct Expanded {
    count: usize,
    next: Option<String>,
    loading: bool,
    error: bool,
    generation: u64,
}

struct App {
    sender: Sender<Job>,
    receiver: Receiver<Reply>,
    key: PathBuf,
    keymap: Keymap,
    request_log: Option<RequestLog>,
    callback: Option<UnixListener>,
    screen: Screen,
    history: Vec<Screen>,
    entries: Vec<Entry>,
    expanded: HashMap<String, Expanded>,
    reply_generation: u64,
    own_profile: Option<User>,
    link_regions: RefCell<Vec<LinkRegion>>,
    poll_at: Instant,
    poll_pending: bool,
    unread: Option<u64>,
    new_commits: u64,
    toast: Option<(String, Instant)>,
    next: Option<String>,
    profile: Option<User>,
    me: Option<User>,
    selected: usize,
    sidebar_selected: usize,
    focus: Pane,
    width: u16,
    height: u16,
    pending_chord: Option<KeyEvent>,
    status: String,
    busy: bool,
    mode: Mode,
    quit: bool,
}

impl App {
    fn new(sender: Sender<Job>, receiver: Receiver<Reply>, key: PathBuf) -> Self {
        let config_path = key
            .parent()
            .unwrap_or(std::path::Path::new("."))
            .join("config.toml");
        let (keymap, warning) = Keymap::load(&config_path);
        Self {
            sender,
            receiver,
            key,
            keymap,
            request_log: None,
            callback: None,
            screen: Screen::Feed("latest"),
            history: vec![],
            entries: vec![],
            expanded: HashMap::new(),
            reply_generation: 0,
            own_profile: None,
            link_regions: RefCell::new(Vec::new()),
            poll_at: Instant::now(),
            poll_pending: false,
            unread: None,
            new_commits: 0,
            toast: None,
            next: None,
            profile: None,
            me: None,
            selected: 0,
            sidebar_selected: 0,
            focus: Pane::Feed,
            width: 120,
            height: 30,
            pending_chord: None,
            status: warning
                .unwrap_or_else(|| "Loading Latest log · L sign in · / find user".into()),
            busy: false,
            mode: Mode::Normal,
            quit: false,
        }
    }

    fn accent(&self) -> Color {
        self.me
            .as_ref()
            .map(|user| lane_color(Some(user)))
            .unwrap_or_else(primary_color)
    }

    fn load(&mut self, next: Option<String>) {
        if next.is_none() {
            self.new_commits = 0;
            self.expanded.clear();
            self.entries.clear();
            self.profile = None;
            self.next = None;
            self.selected = 0;
        }
        if self.screen.private() && self.me.is_none() {
            self.busy = false;
            self.status = "Sign in with L to use this view".into();
            return;
        }
        self.busy = true;
        if self
            .sender
            .send(Job::Load(self.screen.clone(), next))
            .is_err()
        {
            self.busy = false;
            self.status = "Network worker stopped".into();
        }
    }

    fn go(&mut self, screen: Screen) {
        if screen != self.screen {
            self.history.push(self.screen.clone());
            self.screen = screen;
        }
        self.focus = Pane::Feed;
        self.new_commits = 0;
        self.sidebar_selected = match self.screen {
            Screen::Feed(_) => 0,
            Screen::Notifications => 1,
            Screen::Profile(_) | Screen::ProfileReplies(_) => 2,
            Screen::Starred => 3,
            _ => self.sidebar_selected,
        };
        self.load(None);
    }

    fn thread_parent<'a>(&self, commit: &'a Commit) -> Option<&'a str> {
        commit.parent_id.as_deref()
    }

    fn entry_index(&self, id: &str) -> Option<usize> {
        self.entries
            .iter()
            .position(|entry| entry_commit(entry).is_some_and(|commit| commit.id == id))
    }

    fn is_descendant(&self, index: usize, ancestor: &str) -> bool {
        let Some(mut parent) = self
            .entries
            .get(index)
            .and_then(entry_commit)
            .and_then(|commit| self.thread_parent(commit))
        else {
            return false;
        };
        for _ in 0..self.entries.len() {
            if parent == ancestor {
                return true;
            }
            let Some(parent_index) = self.entry_index(parent) else {
                return false;
            };
            let Some(commit) = self.entries.get(parent_index).and_then(entry_commit) else {
                return false;
            };
            let Some(next) = self.thread_parent(commit) else {
                return false;
            };
            parent = next;
        }
        false
    }

    fn subtree_end(&self, parent_index: usize) -> usize {
        let Some(id) = self
            .entries
            .get(parent_index)
            .and_then(entry_commit)
            .map(|c| c.id.as_str())
        else {
            return parent_index + 1;
        };
        let mut end = parent_index + 1;
        while end < self.entries.len() && self.is_descendant(end, id) {
            end += 1;
        }
        end
    }

    fn collapse_branch(&mut self, id: &str) {
        self.expanded.remove(id);
        let Some(index) = self.entry_index(id) else {
            return;
        };
        let end = self.subtree_end(index);
        let removed: Vec<String> = self.entries[index + 1..end]
            .iter()
            .filter_map(entry_commit)
            .map(|c| c.id.clone())
            .collect();
        let count = end - index - 1;
        self.entries.drain(index + 1..end);
        self.expanded.retain(|branch, _| !removed.contains(branch));
        if self.selected >= end {
            self.selected -= count;
        } else if self.selected > index {
            self.selected = index;
        }
    }

    fn toggle_replies(&mut self) {
        let Some(id) = self.commit().map(|commit| commit.id.clone()) else {
            return;
        };
        if self.expanded.contains_key(&id) {
            self.collapse_branch(&id);
            self.status = "Replies collapsed".into();
            return;
        }
        if self.me.is_none() {
            self.status = "Sign in with L to view replies".into();
            return;
        }
        self.reply_generation = self.reply_generation.wrapping_add(1);
        let generation = self.reply_generation;
        self.expanded.insert(
            id.clone(),
            Expanded {
                count: 0,
                next: None,
                loading: true,
                error: false,
                generation,
            },
        );
        self.status = "Loading replies…".into();
        if self
            .sender
            .send(Job::Replies(id.clone(), None, generation))
            .is_err()
        {
            self.expanded.remove(&id);
            self.status = "Network worker stopped".into();
        }
    }

    fn more_replies(&mut self) {
        let Some(id) = self.commit().map(|commit| commit.id.clone()) else {
            return;
        };
        let Some(expanded) = self.expanded.get_mut(&id) else {
            return;
        };
        if expanded.loading {
            return;
        }
        let Some(next) = expanded.next.clone() else {
            return;
        };
        expanded.loading = true;
        self.status = "Loading more replies…".into();
        if self
            .sender
            .send(Job::Replies(id, Some(next), expanded.generation))
            .is_err()
        {
            expanded.loading = false;
            self.status = "Network worker stopped".into();
        }
    }

    fn move_feed_selection(&mut self, amount: isize) {
        let focus = self.focus;
        self.focus = Pane::Feed;
        self.move_cursor(amount);
        self.focus = focus;
    }

    fn move_cursor(&mut self, amount: isize) {
        match self.focus {
            Pane::Nav => {
                self.sidebar_selected = self.sidebar_selected.saturating_add_signed(amount).min(4)
            }
            Pane::Feed => {
                self.selected = self
                    .selected
                    .saturating_add_signed(amount)
                    .min(self.entries.len().saturating_sub(1))
            }
        }
    }

    fn edge(&mut self, bottom: bool) {
        match self.focus {
            Pane::Nav => self.sidebar_selected = if bottom { 4 } else { 0 },
            Pane::Feed => {
                self.selected = if bottom {
                    self.entries.len().saturating_sub(1)
                } else {
                    0
                }
            }
        }
    }

    fn focus_step(&mut self, step: isize) {
        let panes: &[Pane] = if self.width >= 76 {
            &[Pane::Nav, Pane::Feed]
        } else {
            &[Pane::Feed]
        };
        let current = panes
            .iter()
            .position(|pane| *pane == self.focus)
            .unwrap_or(0);
        self.focus = panes[(current as isize + step).rem_euclid(panes.len() as isize) as usize];
    }

    fn open_selected(&mut self) {
        if self.focus != Pane::Nav {
            match self.current() {
                Some(Entry::Commit(commit)) => {
                    self.go(Screen::Thread(commit.id.clone()));
                    return;
                }
                Some(Entry::User(user)) => {
                    self.go(Screen::Profile(user.username.clone()));
                    return;
                }
                Some(Entry::Notice(notice)) => {
                    if let Some(commit) = &notice.commit {
                        self.go(Screen::Thread(commit.id.clone()));
                        return;
                    }
                    if let Some(user) = &notice.actor {
                        self.go(Screen::Profile(user.username.clone()));
                        return;
                    }
                }
                _ => {}
            }
        }
        if self.focus == Pane::Nav {
            let actions = ["latest", "notifications", "profile", "starred", "settings"];
            let Some(action) = actions.get(self.sidebar_selected) else {
                return;
            };
            if *action == "settings" {
                if self.me.is_some() {
                    self.mode = Mode::Settings { selected: 0 };
                } else {
                    self.status = "Sign in with L to change settings".into();
                }
            } else if let Some(key) = self.keymap.event(action) {
                self.normal(key);
            }
        }
    }

    fn back(&mut self) {
        if let Some(previous) = self.history.pop() {
            self.screen = previous;
            self.focus = Pane::Feed;
            self.load(None);
        }
    }

    fn collect(&mut self) {
        if let Some(listener) = &self.callback {
            match callback::poll(listener) {
                Ok(Some(url)) => self.submit_callback(&url),
                Ok(None) => {}
                Err(error) => self.status = error,
            }
        }
        while let Ok(reply) = self.receiver.try_recv() {
            match reply {
                Reply::Boot(Ok(Some(user))) => {
                    self.status = format!("Signed in as @{}", user.username);
                    let _ = self.sender.send(Job::SelfProfile(user.username.clone()));
                    self.me = Some(user);
                    self.unread = None;
                    self.poll_at = Instant::now() - Duration::from_secs(60);
                    self.go(Screen::Feed("latest"));
                }
                Reply::Boot(Ok(None)) => {
                    self.me = None;
                    self.own_profile = None;
                    self.status = "Guest mode · L sign in · / find user".into()
                }
                Reply::Boot(Err(error)) => self.status = error,
                Reply::Redeemed(Ok(user)) => {
                    self.close_callback();
                    self.status = format!("Signed in as @{}", user.username);
                    let _ = self.sender.send(Job::SelfProfile(user.username.clone()));
                    self.me = Some(user);
                    self.unread = None;
                    self.poll_at = Instant::now() - Duration::from_secs(60);
                    self.mode = Mode::Normal;
                    self.go(Screen::Feed("latest"));
                }
                Reply::Redeemed(Err(error)) => {
                    self.status = error;
                    self.busy = false;
                }
                Reply::HandlerInstalled(Ok(())) => {
                    if let Mode::Login { flow, .. } = &self.mode {
                        match open_browser(&flow.url) {
                            Ok(()) => {
                                self.status =
                                    "Handler ready · approve in your browser; callback returns automatically"
                                        .into()
                            }
                            Err(error) => {
                                self.status = format!("Handler ready; open URL manually: {error}")
                            }
                        }
                    }
                    self.busy = false;
                }
                Reply::HandlerInstalled(Err(error)) => {
                    self.busy = false;
                    self.status = error;
                }
                Reply::Loaded(screen, append, result) if screen == self.screen => {
                    self.busy = false;
                    match result {
                        Ok(page) => {
                            if append {
                                self.entries.extend(page.entries);
                            } else {
                                self.entries = page.entries;
                                self.profile = page.profile;
                            }
                            self.next = page.next;
                            if let Screen::Thread(id) = &self.screen {
                                if !append {
                                    self.selected = self.entries.iter().position(|item| matches!(item, Entry::Commit(commit) if &commit.id == id)).unwrap_or(0);
                                }
                            }
                            if screen == Screen::Notifications && !append {
                                self.unread = Some(0);
                            }
                            self.status = format!(
                                "{} items{} · R refresh",
                                self.entries.len(),
                                if self.next.is_some() {
                                    " · n more"
                                } else {
                                    ""
                                }
                            );
                        }
                        Err(error) => {
                            if error.contains("(device_not_approved)") {
                                self.me = None;
                                self.own_profile = None;
                                self.entries.clear();
                            }
                            self.status = error;
                        }
                    }
                }
                Reply::Replies(id, generation, result) => {
                    if !self
                        .expanded
                        .get(&id)
                        .is_some_and(|branch| branch.generation == generation)
                    {
                        continue;
                    }
                    match result {
                        Ok(page) => {
                            let Some(parent_index) = self.entry_index(&id) else {
                                self.expanded.remove(&id);
                                continue;
                            };
                            let at = self.subtree_end(parent_index);
                            let added = page.entries.len();
                            self.entries.splice(at..at, page.entries);
                            if self.selected >= at {
                                self.selected += added;
                            }
                            if let Some(branch) = self.expanded.get_mut(&id) {
                                branch.loading = false;
                                branch.error = false;
                                branch.count += added;
                                branch.next = page.next;
                                self.status = format!(
                                    "{} replies · Space collapse{}",
                                    branch.count,
                                    if branch.next.is_some() {
                                        " · ] more"
                                    } else {
                                        ""
                                    }
                                );
                            }
                        }
                        Err(error) => {
                            if let Some(branch) = self.expanded.get_mut(&id) {
                                branch.loading = false;
                                branch.error = true;
                            }
                            if error.contains("(device_not_approved)") {
                                self.me = None;
                                self.own_profile = None;
                                self.expanded.clear();
                                self.entries.clear();
                            }
                            self.status = error;
                        }
                    }
                }
                Reply::SelfProfile(Ok(user)) => {
                    if self
                        .me
                        .as_ref()
                        .is_some_and(|me| me.username == user.username)
                    {
                        self.own_profile = Some(user);
                    }
                }
                Reply::SelfProfile(Err(error)) => self.status = format!("Your profile: {error}"),
                Reply::Pulse(username, anchor, result) => {
                    self.poll_pending = false;
                    if self
                        .me
                        .as_ref()
                        .is_some_and(|user| user.username == username)
                    {
                        match result {
                            Ok(pulse) => self.apply_pulse(anchor, pulse),
                            Err(error) if error.contains("(device_not_approved)") => {
                                self.me = None;
                                self.own_profile = None;
                                self.unread = None;
                                self.status = error;
                            }
                            Err(_) => {} // Transient polling failures never interrupt browsing.
                        }
                    }
                }
                Reply::Acted(screen, result) => {
                    self.busy = false;
                    match result {
                        Ok((message, updated_user)) => {
                            if let Some(user) = updated_user {
                                self.me = Some(user);
                            }
                            if let Some(name) = self.me.as_ref().map(|user| user.username.clone()) {
                                let _ = self.sender.send(Job::SelfProfile(name));
                            }
                            if screen == self.screen {
                                self.load(None);
                            }
                            self.status = message;
                        }
                        Err(error) => {
                            if error.contains("(device_not_approved)") {
                                self.me = None;
                                self.own_profile = None;
                                self.entries.clear();
                            }
                            self.status = error;
                        }
                    }
                }
                Reply::Loaded(_, _, _) => {}
            }
        }
        self.poll_updates();
    }

    fn poll_updates(&mut self) {
        if self.poll_pending || self.poll_at.elapsed() < Duration::from_secs(60) {
            return;
        }
        let Some(username) = self.me.as_ref().map(|user| user.username.clone()) else {
            return;
        };
        let anchor = match self.screen {
            Screen::Feed(log @ ("following" | "latest")) => self
                .entries
                .iter()
                .find_map(entry_commit)
                .map(|commit| (log, commit.id.clone())),
            _ => None,
        };
        self.poll_at = Instant::now();
        self.poll_pending = self.sender.send(Job::Pulse(username, anchor)).is_ok();
    }

    fn apply_pulse(&mut self, anchor: Option<(&'static str, String)>, pulse: Pulse) {
        let mut messages = Vec::new();
        if let Some(old) = self.unread.filter(|old| pulse.unread > *old) {
            messages.push(format!(
                "🔔 {} new notifications · N open",
                pulse.unread - old
            ));
        }
        self.unread = Some(pulse.unread);
        if let Some(user) = pulse.profile {
            if let Some(previous) = self
                .own_profile
                .as_ref()
                .and_then(|profile| profile.achievements.as_ref())
            {
                if let Some(current) = &user.achievements {
                    let names: Vec<_> = current
                        .badges
                        .iter()
                        .filter(|badge| {
                            badge.unlocked
                                && !previous
                                    .badges
                                    .iter()
                                    .any(|old| old.id == badge.id && old.unlocked)
                        })
                        .map(|badge| badge.name.as_str())
                        .collect();
                    if !names.is_empty() {
                        messages.push(format!("Achievement unlocked: {}", names.join(", ")));
                    }
                }
            }
            self.own_profile = Some(user);
        }
        if let Some((log, id)) = anchor {
            if self.screen == Screen::Feed(log)
                && self
                    .entries
                    .iter()
                    .find_map(entry_commit)
                    .is_some_and(|entry| entry.id == id)
            {
                if let Some(count) = pulse.new_commits {
                    self.new_commits = count;
                }
            }
        }
        if !messages.is_empty() {
            self.toast = Some((messages.join("  ·  "), Instant::now()));
        }
    }

    fn current(&self) -> Option<&Entry> {
        self.entries.get(self.selected)
    }

    fn commit(&self) -> Option<&Commit> {
        match self.current()? {
            Entry::Commit(commit) => Some(commit.as_ref()),
            Entry::Notice(notice) => notice.commit.as_ref(),
            Entry::User(_) => None,
        }
    }

    fn selected_author(&self) -> Option<&str> {
        match self.current()? {
            Entry::Commit(commit) => commit.author.as_ref().map(|user| user.username.as_str()),
            Entry::Notice(notice) => notice.actor.as_ref().map(|user| user.username.as_str()),
            Entry::User(user) => Some(&user.username),
        }
    }

    fn person(&self) -> Option<&User> {
        match &self.screen {
            Screen::Profile(_) | Screen::ProfileReplies(_) => self.profile.as_ref(),
            Screen::Blocked | Screen::Muted => match self.current()? {
                Entry::User(user) => Some(user),
                _ => None,
            },
            _ => None,
        }
    }

    fn send_action(&mut self, action: Action) {
        if self.me.is_none() {
            self.status = "Sign in with L first".into();
            return;
        }
        if self.busy {
            self.status = "Wait for current request".into();
            return;
        }
        self.busy = true;
        self.status = "Sending…".into();
        let _ = self.sender.send(Job::Act(self.screen.clone(), action));
    }

    fn login(&mut self) {
        match AuthFlow::begin(&self.key) {
            Ok(flow) => match callback::listen(self.key.parent().expect("key directory")) {
                Ok(listener) => {
                    self.callback = Some(listener);
                    self.mode = Mode::LoginSetup { flow };
                    self.status =
                        "Enter to enable browser-independent login · m manual · Esc cancel".into();
                }
                Err(error) => self.status = error,
            },
            Err(error) => self.status = error,
        }
    }

    fn close_callback(&mut self) {
        if self.callback.take().is_some() {
            let _ = std::fs::remove_file(callback::socket_path(&self.key));
        }
    }

    fn submit_callback(&mut self, url: &str) {
        if self.busy {
            return;
        }
        if let Mode::Login { flow, .. } = &self.mode {
            match flow.code(url) {
                Ok(code) => {
                    self.busy = true;
                    self.status = "Approving device…".into();
                    let _ = self.sender.send(Job::Redeem(code, flow.verifier()));
                }
                Err(error) => self.status = error,
            }
        }
    }

    fn key(&mut self, key: KeyEvent) {
        if key.kind != KeyEventKind::Press {
            return;
        }
        if is_request_log_key(key) {
            if self.request_log.is_some() {
                let mode = std::mem::replace(&mut self.mode, Mode::Normal);
                self.mode = match mode {
                    Mode::RequestLog { return_to, .. } => *return_to,
                    mode => Mode::RequestLog {
                        offset: 0,
                        return_to: Box::new(mode),
                    },
                };
            }
            return;
        }
        let typing = matches!(
            self.mode,
            Mode::Search(_)
                | Mode::WebSearch(_)
                | Mode::Compose { .. }
                | Mode::ComposePreview { .. }
                | Mode::CustomColor(_)
                | Mode::Login { .. }
                | Mode::Palette { .. }
                | Mode::RequestLog { .. }
                | Mode::Help { .. }
        );
        if key.code == KeyCode::Char('c') && key.modifiers.contains(KeyModifiers::CONTROL) {
            if matches!(self.mode, Mode::Compose { .. }) {
                self.status = "Draft unsent; Enter to submit or Esc to discard".into();
            } else if !typing {
                self.quit = true;
            }
            return;
        }
        if !typing && (key.code == KeyCode::Char('q') || self.keymap.matches("quit", key)) {
            self.quit = true;
            return;
        }
        if !typing && self.keymap.matches("help", key) {
            self.mode = Mode::Help { offset: 0 };
            return;
        }
        if let Mode::Help { offset } = &self.mode {
            let offset = *offset;
            let max_offset = help_text(self, self.width.saturating_sub(6).min(82) as usize)
                .lines()
                .count()
                .saturating_sub(self.height.saturating_sub(6).min(22) as usize);
            let page = self.height.saturating_sub(8).max(1) as usize;
            if key.code == KeyCode::Esc || self.keymap.matches("help", key) {
                self.mode = Mode::Normal;
            } else {
                let offset = match key.code {
                    KeyCode::Up | KeyCode::Char('k') => offset.saturating_sub(1),
                    KeyCode::Down | KeyCode::Char('j') => (offset + 1).min(max_offset),
                    KeyCode::PageUp => offset.saturating_sub(page),
                    KeyCode::PageDown => (offset + page).min(max_offset),
                    KeyCode::Home => 0,
                    KeyCode::End => max_offset,
                    _ => offset,
                };
                self.mode = Mode::Help { offset };
            }
            return;
        }
        if matches!(self.mode, Mode::Settings { .. })
            && !matches!(
                key.code,
                KeyCode::Esc
                    | KeyCode::Enter
                    | KeyCode::Up
                    | KeyCode::Down
                    | KeyCode::Backspace
                    | KeyCode::Char('?')
                    | KeyCode::Char('C')
            )
            && self.keymap.normalize(key).is_some()
        {
            self.normal(key);
            return;
        }
        let mode = std::mem::replace(&mut self.mode, Mode::Normal);
        match mode {
            Mode::Normal => self.normal(key),
            Mode::Help { offset } => self.mode = Mode::Help { offset },
            Mode::RequestLog { offset, return_to } => {
                if key.code == KeyCode::Esc {
                    self.mode = *return_to;
                    return;
                }
                let count = self
                    .request_log
                    .as_ref()
                    .map(|log| log.entries().len())
                    .unwrap_or_default();
                let offset = match key.code {
                    KeyCode::Up | KeyCode::Char('k') => (offset + 1).min(count.saturating_sub(14)),
                    KeyCode::Down | KeyCode::Char('j') => offset.saturating_sub(1),
                    KeyCode::Home => count.saturating_sub(14),
                    KeyCode::End => 0,
                    _ => offset,
                };
                self.mode = Mode::RequestLog { offset, return_to };
            }
            Mode::Settings { selected } => match key.code {
                KeyCode::Esc => {}
                KeyCode::Up | KeyCode::Char('k') => {
                    self.mode = Mode::Settings {
                        selected: selected.saturating_sub(1),
                    }
                }
                KeyCode::Down | KeyCode::Char('j') => {
                    self.mode = Mode::Settings {
                        selected: (selected + 1).min(8),
                    }
                }
                KeyCode::Char('C') => self.mode = Mode::CustomColor(String::new()),
                KeyCode::Enter if selected <= 6 => {
                    self.send_action(Action::SetLane {
                        lane: (selected > 0).then_some((selected - 1) as u8),
                        color: None,
                    });
                    self.mode = Mode::Settings { selected };
                }
                KeyCode::Enter if selected == 7 => {
                    let enabled = !self
                        .me
                        .as_ref()
                        .and_then(|u| u.pasteis_mode)
                        .unwrap_or(false);
                    self.send_action(Action::SetPasteisMode { enabled });
                    self.mode = Mode::Settings { selected };
                }
                KeyCode::Enter => {
                    self.send_action(Action::SyncGithub);
                    self.mode = Mode::Settings { selected };
                }
                _ => self.mode = Mode::Settings { selected },
            },
            Mode::CustomColor(mut input) => match key.code {
                KeyCode::Esc => self.mode = Mode::Settings { selected: 0 },
                KeyCode::Backspace => {
                    input.pop();
                    self.mode = Mode::CustomColor(input);
                }
                KeyCode::Enter => {
                    let color = input.trim_start_matches('#').to_ascii_lowercase();
                    if color.len() == 6 && color.bytes().all(|byte| byte.is_ascii_hexdigit()) {
                        self.send_action(Action::SetLane {
                            lane: None,
                            color: Some(format!("#{color}")),
                        });
                        self.mode = Mode::Settings { selected: 0 };
                    } else {
                        self.status = "Enter six hex digits, e.g. #ff8800".into();
                        self.mode = Mode::CustomColor(input);
                    }
                }
                KeyCode::Char(ch) if ch.is_ascii_hexdigit() || ch == '#' => {
                    if input.len() < 7 {
                        input.push(ch);
                    }
                    self.mode = Mode::CustomColor(input);
                }
                _ => self.mode = Mode::CustomColor(input),
            },
            Mode::WebSearch(mut input) => match key.code {
                KeyCode::Esc => {}
                KeyCode::Backspace => {
                    input.pop();
                    self.mode = Mode::WebSearch(input);
                }
                KeyCode::Enter if !input.trim().is_empty() => {
                    match official_search_url(input.trim()).and_then(|url| open_browser(&url)) {
                        Ok(()) => {
                            self.status =
                                "Search opened in browser · no /api/v1 search endpoint".into()
                        }
                        Err(error) => {
                            self.status = error;
                            self.mode = Mode::WebSearch(input);
                        }
                    }
                }
                KeyCode::Char(ch)
                    if !key
                        .modifiers
                        .intersects(KeyModifiers::CONTROL | KeyModifiers::SUPER) =>
                {
                    input.push(ch);
                    self.mode = Mode::WebSearch(input);
                }
                _ => self.mode = Mode::WebSearch(input),
            },
            Mode::Search(mut input) => match key.code {
                KeyCode::Esc => {}
                KeyCode::Backspace => {
                    input.pop();
                    self.mode = Mode::Search(input);
                }
                KeyCode::Enter => match api::valid_username(&input) {
                    Some(name) => self.go(Screen::Profile(name)),
                    None => {
                        self.status = "Enter a GitHub username (letters, digits, -)".into();
                        self.mode = Mode::Search(input);
                    }
                },
                KeyCode::Char(ch)
                    if !key
                        .modifiers
                        .intersects(KeyModifiers::CONTROL | KeyModifiers::SUPER) =>
                {
                    input.push(ch);
                    self.mode = Mode::Search(input);
                }
                _ => self.mode = Mode::Search(input),
            },
            Mode::Compose {
                mut text,
                parent,
                quote,
            } => match key.code {
                _ if self.keymap.matches("submit", key) => self.submit_draft(text, parent, quote),
                _ if self.keymap.matches("preview", key) => {
                    self.mode = Mode::ComposePreview {
                        text,
                        parent,
                        quote,
                    };
                }
                KeyCode::Esc => {}
                KeyCode::Backspace => {
                    text.pop();
                    self.mode = Mode::Compose {
                        text,
                        parent,
                        quote,
                    };
                }
                KeyCode::Enter if key.modifiers.contains(KeyModifiers::ALT) => {
                    text.push('\n');
                    self.mode = Mode::Compose {
                        text,
                        parent,
                        quote,
                    };
                }
                KeyCode::Enter => self.submit_draft(text, parent, quote),
                KeyCode::Char(ch)
                    if !key
                        .modifiers
                        .intersects(KeyModifiers::CONTROL | KeyModifiers::SUPER) =>
                {
                    text.push(ch);
                    self.mode = Mode::Compose {
                        text,
                        parent,
                        quote,
                    };
                }
                _ => {
                    self.mode = Mode::Compose {
                        text,
                        parent,
                        quote,
                    }
                }
            },
            Mode::ComposePreview {
                text,
                parent,
                quote,
            } => match key.code {
                _ if self.keymap.matches("submit", key) => self.submit_draft(text, parent, quote),
                KeyCode::Esc => {
                    self.mode = Mode::Compose {
                        text,
                        parent,
                        quote,
                    };
                }
                _ if self.keymap.matches("preview", key) => {
                    self.mode = Mode::Compose {
                        text,
                        parent,
                        quote,
                    }
                }
                KeyCode::Enter => self.submit_draft(text, parent, quote),
                _ => {
                    self.mode = Mode::ComposePreview {
                        text,
                        parent,
                        quote,
                    }
                }
            },
            Mode::LoginSetup { flow } => match key.code {
                KeyCode::Esc => self.close_callback(),
                KeyCode::Enter | KeyCode::Char('y') => {
                    self.busy = true;
                    self.status = "Registering the devnull:// handler…".into();
                    self.mode = Mode::Login {
                        flow,
                        input: String::new(),
                    };
                    if self.sender.send(Job::InstallHandler).is_err() {
                        self.busy = false;
                        self.status = "Login worker stopped".into();
                    }
                }
                KeyCode::Char('m') => {
                    if let Err(error) = open_browser(&flow.url) {
                        self.status = error;
                    }
                    self.mode = Mode::Login {
                        flow,
                        input: String::new(),
                    };
                }
                _ => self.mode = Mode::LoginSetup { flow },
            },
            Mode::Login { flow, mut input } => match key.code {
                KeyCode::Esc => self.close_callback(),
                KeyCode::Char('H')
                    if cfg!(any(target_os = "macos", target_os = "linux")) && !self.busy =>
                {
                    self.busy = true;
                    self.status = "Registering the devnull:// handler…".into();
                    let _ = self.sender.send(Job::InstallHandler);
                    self.mode = Mode::Login { flow, input };
                }
                KeyCode::Char('b') => {
                    if let Err(error) = open_browser(&flow.url) {
                        self.status = error;
                    }
                    self.mode = Mode::Login { flow, input };
                }
                KeyCode::Backspace => {
                    input.pop();
                    self.mode = Mode::Login { flow, input };
                }
                KeyCode::Enter if self.busy => {
                    self.mode = Mode::Login { flow, input };
                }
                KeyCode::Enter => match flow.code(&input) {
                    Ok(code) => {
                        self.busy = true;
                        self.status = "Approving device…".into();
                        let _ = self.sender.send(Job::Redeem(code, flow.verifier()));
                        self.mode = Mode::Login { flow, input };
                    }
                    Err(error) => {
                        self.status = error;
                        self.mode = Mode::Login { flow, input };
                    }
                },
                KeyCode::Char(ch)
                    if !key
                        .modifiers
                        .intersects(KeyModifiers::CONTROL | KeyModifiers::SUPER) =>
                {
                    input.push(ch);
                    self.mode = Mode::Login { flow, input };
                }
                _ => self.mode = Mode::Login { flow, input },
            },
            Mode::Palette {
                mut query,
                mut selected,
            } => {
                match key.code {
                    KeyCode::Esc => return,
                    KeyCode::Backspace => {
                        query.pop();
                        selected = 0;
                    }
                    KeyCode::Char('j') if key.modifiers.contains(KeyModifiers::CONTROL) => {
                        selected =
                            (selected + 1).min(commands(&query, self).len().saturating_sub(1))
                    }
                    KeyCode::Up => selected = selected.saturating_sub(1),
                    KeyCode::Down => {
                        selected =
                            (selected + 1).min(commands(&query, self).len().saturating_sub(1))
                    }
                    KeyCode::Enter => {
                        if let Some((_, binding)) = commands(&query, self).get(selected) {
                            if *binding == "quit" {
                                self.quit = true;
                            } else if *binding == "request_log" {
                                self.mode = Mode::RequestLog {
                                    offset: 0,
                                    return_to: Box::new(Mode::Palette { query, selected }),
                                };
                                return;
                            } else if let Some(log) = match *binding {
                                "latest" => Some("latest"),
                                "trending" => Some("trending"),
                                "following" => Some("following"),
                                _ => None,
                            } {
                                self.go(Screen::Feed(log));
                            } else if *binding == "go_author" {
                                self.run_chord("go_author");
                            } else if let Some(key) = self.keymap.event(binding) {
                                self.normal(key);
                            }
                        }
                        return;
                    }
                    KeyCode::Char(ch)
                        if !key
                            .modifiers
                            .intersects(KeyModifiers::CONTROL | KeyModifiers::SUPER) =>
                    {
                        query.push(ch);
                        selected = 0;
                    }
                    _ => {}
                }
                self.mode = Mode::Palette { query, selected };
            }
            Mode::Block { username, remove } => {
                if key.code == KeyCode::Char('y') {
                    self.send_action(Action::Block { username, remove });
                } else if key.code != KeyCode::Esc {
                    self.mode = Mode::Block { username, remove };
                }
            }
            Mode::Report(id) => {
                let reason = match key.code {
                    KeyCode::Char('1') => Some("spam"),
                    KeyCode::Char('2') => Some("harassment"),
                    KeyCode::Char('3') => Some("unsafe"),
                    KeyCode::Char('4') => Some("other"),
                    _ => None,
                };
                if let Some(reason) = reason {
                    self.send_action(Action::Report { id, reason });
                } else if key.code != KeyCode::Esc {
                    self.mode = Mode::Report(id);
                }
            }
        }
    }

    fn paste(&mut self, text: String) {
        let text = api::display_text(&text);
        match &mut self.mode {
            Mode::Search(value)
            | Mode::WebSearch(value)
            | Mode::Login { input: value, .. }
            | Mode::Compose { text: value, .. } => value.push_str(&text),
            Mode::CustomColor(value) => value.push_str(&text),
            Mode::Palette { query, selected } => {
                query.push_str(&text);
                *selected = 0;
            }
            _ => {}
        }
    }

    fn run_chord(&mut self, action: &str) {
        match action {
            "go_latest" => self.go(Screen::Feed("latest")),
            "go_starred" => self.go(Screen::Starred),
            "go_profile" => {
                if let Some(name) = self.me.as_ref().map(|u| u.username.clone()) {
                    self.go(Screen::Profile(name));
                } else {
                    self.status = "Sign in with L to open your profile".into();
                }
            }
            "go_author" => {
                if let Some(name) = self.selected_author().map(str::to_owned) {
                    self.go(Screen::Profile(name));
                } else {
                    self.status = "Select a commit to open its author's profile".into();
                }
            }
            "first_item_chord" => self.edge(false),
            _ => {}
        }
    }

    fn submit_draft(&mut self, text: String, parent: Option<String>, quote: Option<String>) {
        if text.trim().is_empty() || text.chars().count() > 4000 {
            self.status = "Commit must be 1–4000 characters".into();
            self.mode = Mode::Compose {
                text,
                parent,
                quote,
            };
        } else {
            self.send_action(Action::Create {
                body: text,
                parent,
                quote,
            });
        }
    }

    fn open_link_at(&mut self, x: u16, y: u16) {
        let url = self
            .link_regions
            .borrow()
            .iter()
            .find(|region| {
                x >= region.area.x
                    && x < region.area.right()
                    && y >= region.area.y
                    && y < region.area.bottom()
            })
            .map(|region| region.url.clone());
        if let Some(url) = url {
            match open_browser(&url) {
                Ok(()) => self.status = "Opened link in browser".into(),
                Err(error) => self.status = error,
            }
        }
    }

    fn copy_link(&mut self) {
        let url = self
            .commit()
            .and_then(|commit| commit.html_url.as_deref())
            .map(str::to_owned);
        match url {
            Some(url) => match copy_to_clipboard(&url) {
                Ok(()) => self.status = "Commit link copied".into(),
                Err(error) => self.status = error,
            },
            None => self.status = "No commit link selected".into(),
        }
    }

    fn normal(&mut self, key: KeyEvent) {
        if let Some(prefix) = self.pending_chord.take() {
            if let Some(action) = self.keymap.chord_action(prefix, key) {
                self.run_chord(action);
                return;
            }
        }
        let pressed = key;
        let Some(key) = self.keymap.normalize(key) else {
            return;
        };
        if self.keymap.starts_chord(key) {
            self.pending_chord = Some(key);
            self.status = format!("{} …", key_label(key));
            return;
        }
        if self.keymap.matches("search_ctrl", pressed) {
            self.mode = Mode::WebSearch(String::new());
            return;
        }
        if key
            .modifiers
            .intersects(KeyModifiers::CONTROL | KeyModifiers::SUPER)
        {
            match key.code {
                KeyCode::Char('d') if key.modifiers.contains(KeyModifiers::CONTROL) => {
                    self.move_cursor(((self.height.saturating_sub(8) / 8).max(1)) as isize);
                    return;
                }
                KeyCode::Char('u') if key.modifiers.contains(KeyModifiers::CONTROL) => {
                    self.move_cursor(-(((self.height.saturating_sub(8) / 8).max(1)) as isize));
                    return;
                }
                _ => return,
            }
        }
        if matches!(self.screen, Screen::Feed(_)) {
            for (action, log) in [
                ("latest", "latest"),
                ("trending", "trending"),
                ("following", "following"),
            ] {
                if self.keymap.matches(action, key) {
                    self.go(Screen::Feed(log));
                    return;
                }
            }
        }
        match key.code {
            KeyCode::Char('?') => self.mode = Mode::Help { offset: 0 },
            KeyCode::Char(':') => {
                self.mode = Mode::Palette {
                    query: String::new(),
                    selected: 0,
                }
            }
            KeyCode::Char('j') => self.move_feed_selection(1),
            KeyCode::Char('k') => self.move_feed_selection(-1),
            KeyCode::Down => self.move_cursor(1),
            KeyCode::Up => self.move_cursor(-1),
            KeyCode::Char('G') | KeyCode::End => self.edge(true),
            KeyCode::Home => self.edge(false),
            KeyCode::PageDown => {
                self.move_cursor(((self.height.saturating_sub(8) / 4).max(1)) as isize)
            }
            KeyCode::PageUp => {
                self.move_cursor(-((self.height.saturating_sub(8) / 4).max(1) as isize))
            }
            KeyCode::Tab => self.focus_step(1),
            KeyCode::BackTab => self.focus_step(-1),
            KeyCode::Char('h') | KeyCode::Left => match self.focus {
                Pane::Feed if self.width >= 76 => self.focus = Pane::Nav,
                Pane::Feed => self.back(),
                Pane::Nav => {}
            },
            KeyCode::Char('l') | KeyCode::Right => match self.focus {
                Pane::Nav => self.focus = Pane::Feed,
                Pane::Feed => self.open_selected(),
            },
            KeyCode::Char('a') => {
                if let Screen::ProfileReplies(name) = &self.screen {
                    self.go(Screen::Profile(name.clone()));
                }
            }
            KeyCode::Char('e') => {
                if let Screen::Profile(name) | Screen::ProfileReplies(name) = &self.screen {
                    self.go(Screen::ProfileReplies(name.clone()));
                }
            }
            KeyCode::Char('N') => self.go(Screen::Notifications),
            KeyCode::Char('b') => self.go(Screen::Starred),
            KeyCode::Char('M') => self.go(Screen::Muted),
            KeyCode::Char('X') => self.go(Screen::Blocked),
            _ if self.keymap.matches("settings", key) => {
                if self.me.is_some() {
                    self.mode = Mode::Settings { selected: 0 };
                } else {
                    self.status = "Sign in with L to change settings".into();
                }
            }
            KeyCode::Char('p') => {
                if let Some(name) = self.me.as_ref().map(|u| u.username.clone()) {
                    self.go(Screen::Profile(name));
                } else {
                    self.status = "Sign in with L, or / to find a public profile".into();
                }
            }
            KeyCode::Char('/') => self.mode = Mode::WebSearch(String::new()),
            KeyCode::Char('u') if key.modifiers.contains(KeyModifiers::ALT) => {
                self.mode = Mode::Search(String::new())
            }
            KeyCode::Char('L') => self.login(),
            KeyCode::Char('R') => self.load(None),
            KeyCode::Char('n')
                if !key
                    .modifiers
                    .intersects(KeyModifiers::CONTROL | KeyModifiers::SUPER)
                    && self.next.is_some()
                    && !self.busy =>
            {
                self.load(self.next.clone())
            }
            KeyCode::Esc | KeyCode::Backspace => self.back(),
            KeyCode::Enter => self.open_selected(),
            KeyCode::Char(']') => self.more_replies(),
            KeyCode::Char('c') if self.me.is_some() => {
                self.mode = Mode::Compose {
                    text: String::new(),
                    parent: None,
                    quote: None,
                }
            }
            KeyCode::Char('c') => self.status = "Sign in with L before writing".into(),
            KeyCode::Char('r') if self.me.is_some() => {
                if let Some(id) = self.commit().map(|c| c.id.clone()) {
                    self.mode = Mode::Compose {
                        text: String::new(),
                        parent: Some(id),
                        quote: None,
                    };
                }
            }
            KeyCode::Char('q') => self.quit = true,
            KeyCode::Char('Q') if self.me.is_some() => {
                if let Some(id) = self.commit().map(|c| c.id.clone()) {
                    self.mode = Mode::Compose {
                        text: String::new(),
                        parent: None,
                        quote: Some(id),
                    };
                }
            }
            KeyCode::Char('Q') => self.status = "Sign in with L before quoting".into(),
            KeyCode::Char(' ') => self.toggle_replies(),
            KeyCode::Char('y') => self.copy_link(),
            KeyCode::Char('o') | KeyCode::Char('w') => {
                let url = match self.current() {
                    Some(Entry::Commit(commit)) => commit.html_url.as_deref(),
                    Some(Entry::User(user)) => user.html_url.as_deref(),
                    _ => self.profile.as_ref().and_then(|u| u.html_url.as_deref()),
                };
                if let Some(url) = url {
                    if let Err(error) = open_browser(url) {
                        self.status = error;
                    }
                }
            }
            KeyCode::Char('s') => {
                if let Some(commit) = self.commit().filter(|commit| commit.starred != Some(true)) {
                    self.send_action(Action::Star {
                        id: commit.id.clone(),
                        remove: false,
                    });
                }
            }
            KeyCode::Char('S') => {
                if let Some(commit) = self.commit().filter(|commit| commit.starred == Some(true)) {
                    self.send_action(Action::Star {
                        id: commit.id.clone(),
                        remove: true,
                    });
                }
            }
            KeyCode::Char('f') if !matches!(self.screen, Screen::Blocked | Screen::Muted) => {
                if let Some(user) = self.person().filter(|user| {
                    self.me
                        .as_ref()
                        .is_none_or(|me| me.username != user.username)
                }) {
                    self.send_action(Action::Follow {
                        username: user.username.clone(),
                        remove: user.is_followed.unwrap_or(false),
                    });
                }
            }
            KeyCode::Char('m') if !matches!(self.screen, Screen::Blocked) => {
                if let Some(user) = self.person().filter(|user| {
                    self.me
                        .as_ref()
                        .is_none_or(|me| me.username != user.username)
                }) {
                    self.send_action(Action::Mute {
                        username: user.username.clone(),
                        remove: matches!(self.screen, Screen::Muted)
                            || user.is_muting.unwrap_or(false),
                    });
                }
            }
            KeyCode::Char('B') => {
                if let Some(user) = self.person().filter(|user| {
                    self.me
                        .as_ref()
                        .is_none_or(|me| me.username != user.username)
                }) {
                    self.mode = Mode::Block {
                        username: user.username.clone(),
                        remove: matches!(self.screen, Screen::Blocked),
                    };
                }
            }
            KeyCode::Char('t') => {
                if let Some(commit) = self.commit() {
                    self.send_action(Action::Pastel {
                        id: commit.id.clone(),
                    });
                }
            }
            KeyCode::Char('!') => {
                if let Some(id) = self.commit().map(|c| c.id.clone()) {
                    self.mode = Mode::Report(id);
                }
            }
            KeyCode::Char('u') => {
                if let Screen::Profile(name) | Screen::ProfileReplies(name) = &self.screen {
                    self.go(Screen::Followers(name.clone()));
                }
            }
            KeyCode::Char('i') => {
                if let Screen::Profile(name) | Screen::ProfileReplies(name) = &self.screen {
                    self.go(Screen::Following(name.clone()));
                }
            }
            _ => {}
        }
    }
}

impl Drop for App {
    fn drop(&mut self) {
        self.close_callback();
    }
}

fn copy_to_clipboard(text: &str) -> Result<(), String> {
    #[cfg(target_os = "macos")]
    let commands: &[(&str, &[&str])] = &[("pbcopy", &[])];
    #[cfg(target_os = "linux")]
    let commands: &[(&str, &[&str])] = &[
        ("wl-copy", &[]),
        ("xclip", &["-selection", "clipboard"]),
        ("xsel", &["--clipboard", "--input"]),
    ];
    for &(program, args) in commands {
        let Ok(mut child) = Command::new(program)
            .args(args)
            .stdin(std::process::Stdio::piped())
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null())
            .spawn()
        else {
            continue;
        };
        if let Some(mut stdin) = child.stdin.take() {
            stdin
                .write_all(text.as_bytes())
                .map_err(|e| e.to_string())?;
        }
        if child.wait().map_err(|e| e.to_string())?.success() {
            return Ok(());
        }
    }
    Err("Clipboard unavailable (install wl-clipboard, xclip, or xsel)".into())
}

fn official_search_url(query: &str) -> Result<String, String> {
    if query.chars().count() > 100 {
        return Err("Search query limited to 100 characters".into());
    }
    let mut url = reqwest::Url::parse("https://dev-null.sh/search").map_err(|e| e.to_string())?;
    url.query_pairs_mut().append_pair("q", query);
    Ok(url.to_string())
}

fn valid_click_url(value: &str) -> bool {
    Url::parse(value).ok().is_some_and(|url| {
        matches!(url.scheme(), "http" | "https")
            && url.host_str().is_some()
            && url.username().is_empty()
            && url.password().is_none()
    })
}

fn open_browser(url: &str) -> Result<(), String> {
    if !valid_click_url(url) {
        return Err("Refusing to open unsafe URL".into());
    }
    let command = if cfg!(target_os = "macos") {
        "open"
    } else {
        "xdg-open"
    };
    Command::new(command)
        .arg(url)
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .spawn()
        .map_err(|e| e.to_string())?;
    Ok(())
}

fn worker(mut api: Api, jobs: Receiver<Job>, replies: Sender<Reply>) {
    while let Ok(job) = jobs.recv() {
        let reply = match job {
            Job::InstallHandler => {
                #[cfg(any(target_os = "macos", target_os = "linux"))]
                let result = callback::install(api.key());
                #[cfg(not(any(target_os = "macos", target_os = "linux")))]
                let result =
                    Err("Automatic callback requires macOS or Linux; use the manual flow".into());
                Reply::HandlerInstalled(result)
            }
            Job::Bootstrap => Reply::Boot(api.bootstrap()),
            Job::Load(screen, next) => {
                let append = next.is_some();
                let result = api.load(&screen, next.as_deref());
                Reply::Loaded(screen, append, result)
            }
            Job::SelfProfile(username) => Reply::SelfProfile(api.user(&username)),
            Job::Pulse(username, anchor) => {
                let result = api.pulse(
                    &username,
                    anchor.as_ref().map(|(log, _)| *log),
                    anchor.as_ref().map(|(_, id)| id.as_str()),
                );
                Reply::Pulse(username, anchor, result)
            }
            Job::Replies(id, next, generation) => {
                let result = api.replies(&id, next.as_deref());
                Reply::Replies(id, generation, result)
            }
            Job::Act(screen, action) => Reply::Acted(screen, api.act(action)),
            Job::Redeem(code, verifier) => Reply::Redeemed(api.redeem(code, verifier)),
        };
        if matches!(&reply, Reply::Loaded(_, _, Err(error)) | Reply::Acted(_, Err(error)) | Reply::Replies(_, _, Err(error)) | Reply::Pulse(_, _, Err(error)) if error.contains("(device_not_approved)"))
        {
            api.forget_approval();
        }
        if replies.send(reply).is_err() {
            break;
        }
    }
}

fn wide_columns(area: Rect) -> [Rect; 3] {
    let columns = Layout::default()
        .direction(Direction::Horizontal)
        .constraints([
            Constraint::Percentage(20),
            Constraint::Percentage(60),
            Constraint::Percentage(20),
        ])
        .split(area);
    [columns[0], columns[1], columns[2]]
}

fn draw(frame: &mut Frame, app: &App) {
    let size = frame.area();
    frame.render_widget(
        Block::default().style(Style::default().bg(BG).fg(Color::Reset)),
        size,
    );
    if size.width < 52 || size.height < 10 {
        frame.render_widget(
            Paragraph::new("Resize terminal to at least 52×10")
                .style(Style::default().fg(app.accent())),
            size,
        );
        return;
    }
    let layout = Layout::default()
        .direction(Direction::Vertical)
        .constraints([Constraint::Min(4), Constraint::Length(3)])
        .split(size);
    if size.width >= 100 {
        let cols = wide_columns(layout[0]);
        sidebar(frame, app, cols[0]);
        feed(frame, app, cols[1]);
    } else if size.width >= 76 {
        let cols = Layout::default()
            .direction(Direction::Horizontal)
            .constraints([Constraint::Length(26), Constraint::Min(30)])
            .split(layout[0]);
        sidebar(frame, app, cols[0]);
        feed(frame, app, cols[1]);
    } else {
        feed(frame, app, layout[0]);
    }
    let mode = match app.focus {
        Pane::Nav => "NAV",
        Pane::Feed => "LOG",
    };
    let mode_label = match app.mode {
        Mode::Normal => "NORMAL",
        Mode::Settings { .. } | Mode::CustomColor(_) => "SETTINGS",
        Mode::Palette { .. } => "COMMANDS",
        Mode::RequestLog { .. } => "HTTP LOG",
        Mode::Compose { .. } => "WRITE",
        Mode::LoginSetup { .. } | Mode::Login { .. } => "LOGIN",
        Mode::Search(_) | Mode::WebSearch(_) => "SEARCH",
        Mode::ComposePreview { .. } => "PREVIEW",
        Mode::Help { .. } => "HELP",
        Mode::Report(_) | Mode::Block { .. } => "CONFIRM",
    };
    let status = Line::from(vec![
        Span::styled(
            format!(" {mode_label} "),
            Style::default()
                .fg(app.accent())
                .add_modifier(Modifier::BOLD),
        ),
        Span::styled(format!("  {mode}  "), Style::default().fg(app.accent())),
        Span::styled(
            if app.busy { "◌ " } else { "● " },
            Style::default().fg(app.accent()),
        ),
        Span::styled(api::display_text(&app.status), Style::default().fg(MUTED)),
    ]);
    let log_hint = if app.request_log.is_some() {
        "   Ctrl+Shift+L / Ctrl+L request log"
    } else {
        ""
    };
    let keys = match app.focus {
        Pane::Nav => format!(
            " ↑/↓ nav   {} feed   {} commands   {} help   q quit{}",
            app.keymap.label("pane_right"),
            app.keymap.label("commands"),
            app.keymap.label("help"),
            log_hint
        ),
        Pane::Feed => format!(
            " {} next   {} prev   {} copy   {} open   {} web   {} reply   {} quote   {} replies   s star · S unstar   q quit   {} search{}",
            app.keymap.label("move_down"),
            app.keymap.label("move_up"),
            app.keymap.label("copy_link"),
            app.keymap.label("open"),
            app.keymap.label("open_web"),
            app.keymap.label("reply"),
            app.keymap.label("quote"),
            app.keymap.label("expand_replies"),
            app.keymap.label("search_ctrl"),
            log_hint
        ),
    };
    let keys = if size.width < 100 {
        format!(
            "{} next {} prev {} open {} web {} reply {} quote {} replies s star S unstar q quit{}",
            app.keymap.label("move_down"),
            app.keymap.label("move_up"),
            app.keymap.label("open"),
            app.keymap.label("open_web"),
            app.keymap.label("reply"),
            app.keymap.label("quote"),
            app.keymap.label("expand_replies"),
            log_hint
        )
    } else {
        keys
    };
    frame.render_widget(
        Paragraph::new(Text::from(vec![
            status,
            Line::styled(keys, Style::default().fg(Color::Reset)),
        ])),
        layout[1],
    );
    draw_events(frame, app, size);
    overlay(frame, app, size);
    let regions = find_link_regions(app, frame.buffer_mut(), size);
    *app.link_regions.borrow_mut() = regions;
}

fn draw_events(frame: &mut Frame, app: &App, size: Rect) {
    if let Some((message, _)) = app
        .toast
        .as_ref()
        .filter(|(_, at)| at.elapsed() < Duration::from_secs(5))
    {
        let width = size.width.saturating_sub(4).min(62);
        let area = Rect::new(size.width.saturating_sub(width + 2), 1, width, 4);
        frame.render_widget(Clear, area);
        frame.render_widget(
            Paragraph::new(format!("  {message}"))
                .wrap(Wrap { trim: true })
                .block(
                    Block::default()
                        .borders(Borders::ALL)
                        .border_style(Style::default().fg(app.accent())),
                ),
            area,
        );
    }
}

fn sidebar(frame: &mut Frame, app: &App, area: Rect) {
    let active = match &app.screen {
        Screen::Feed(_) => Some(0),
        Screen::Notifications => Some(1),
        Screen::Profile(name) | Screen::ProfileReplies(name)
            if app.me.as_ref().is_some_and(|me| &me.username == name) =>
        {
            Some(2)
        }
        Screen::Starred => Some(3),
        _ if matches!(app.mode, Mode::Settings { .. } | Mode::CustomColor(_)) => Some(4),
        _ => None,
    };
    let logo = Line::from(vec![
        Span::raw("  "),
        Span::styled("/", Style::default().fg(Color::White)),
        Span::styled("dev", Style::default().fg(Color::White)),
        Span::styled("/", Style::default().fg(brand_color())),
        Span::styled("null", Style::default().fg(Color::White)),
    ]);
    let mut account = app
        .me
        .as_ref()
        .map(|user| user_avatar(Some(user)))
        .unwrap_or_default();
    account.push(Span::styled(
        app.me
            .as_ref()
            .map(|user| format!(" @{}", user.username))
            .unwrap_or_else(|| " @guest · L sign in".into()),
        Style::default().fg(app.accent()),
    ));
    let mut lines = vec![
        Line::raw(""),
        logo,
        Line::raw(""),
        Line::raw(""),
        Line::from(account),
    ];
    lines.extend([Line::raw(""), Line::raw("")]);
    let menu = ["Log", "Notification", "Profile", "Starred", "Settings"];
    let gap = usize::from(area.height >= 18);
    for (index, label) in menu.iter().enumerate() {
        let selected = app.focus == Pane::Nav && app.sidebar_selected == index;
        let style = if selected {
            Style::default()
                .fg(app.accent())
                .add_modifier(Modifier::BOLD | Modifier::UNDERLINED)
        } else if active == Some(index) {
            Style::default()
                .fg(app.accent())
                .add_modifier(Modifier::BOLD)
        } else {
            Style::default().fg(Color::Reset)
        };
        let badge = if index == 1 {
            app.unread
                .filter(|count| *count > 0)
                .map(|count| format!("  {count}"))
                .unwrap_or_default()
        } else {
            String::new()
        };
        lines.push(Line::styled(
            format!(
                "  {} {label}{badge}",
                if selected {
                    "▏"
                } else if active == Some(index) {
                    "●"
                } else {
                    " "
                }
            ),
            style,
        ));
        for _ in 0..gap {
            lines.push(Line::raw(""));
        }
    }
    frame.render_widget(Paragraph::new(Text::from(lines)), area);
}

struct TreePosition {
    parents: Vec<(bool, Color)>,
    depth: usize,
    visual_parent_id: Option<String>,
}

fn commit_parent_id(commit: &Commit) -> Option<&str> {
    commit.parent_id.as_deref()
}

fn tree_positions(app: &App) -> Vec<TreePosition> {
    let visual_parent_ids: Vec<Option<String>> = app
        .entries
        .iter()
        .enumerate()
        .map(|(index, entry)| {
            let parent_id = commit_parent_id(entry_commit(entry)?)?;
            let first_sibling = app.entries[..index]
                .iter()
                .filter_map(entry_commit)
                .find(|sibling| commit_parent_id(sibling) == Some(parent_id));
            Some(
                first_sibling
                    .map(|sibling| sibling.id.clone())
                    .unwrap_or_else(|| parent_id.to_owned()),
            )
        })
        .collect();

    app.entries
        .iter()
        .enumerate()
        .map(|(index, entry)| {
            let Some(commit) = entry_commit(entry) else {
                return TreePosition {
                    parents: vec![],
                    depth: 0,
                    visual_parent_id: None,
                };
            };
            let mut depth = 0;
            let mut actual_parent = commit_parent_id(commit);
            while let Some(id) = actual_parent {
                let Some(parent_index) = app.entries[..index].iter().rposition(|item| {
                    entry_commit(item).is_some_and(|candidate| candidate.id == id)
                }) else {
                    break;
                };
                depth += 1;
                actual_parent = app
                    .entries
                    .get(parent_index)
                    .and_then(entry_commit)
                    .and_then(commit_parent_id);
            }

            let visual_parent_id = visual_parent_ids[index].clone();
            let mut ancestors = Vec::new();
            let mut visual_parent = visual_parent_id.as_deref();
            while let Some(id) = visual_parent {
                let Some(parent_index) = app.entries[..index].iter().rposition(|item| {
                    entry_commit(item).is_some_and(|candidate| candidate.id == id)
                }) else {
                    break;
                };
                ancestors.push(parent_index);
                visual_parent = visual_parent_ids[parent_index].as_deref();
            }
            ancestors.reverse();
            let parents = ancestors
                .iter()
                .enumerate()
                .map(|(level, ancestor_index)| {
                    let ancestor =
                        entry_commit(&app.entries[*ancestor_index]).expect("ancestor is a commit");
                    let child_index = ancestors.get(level + 1).copied().unwrap_or(index);
                    let has_later_sibling = visual_parent_ids
                        .iter()
                        .skip(child_index + 1)
                        .any(|parent| parent.as_deref() == Some(ancestor.id.as_str()));
                    (has_later_sibling, lane_color(ancestor.author.as_ref()))
                })
                .collect();
            TreePosition {
                parents,
                depth,
                visual_parent_id,
            }
        })
        .collect()
}

fn feed(frame: &mut Frame, app: &App, area: Rect) {
    let mut inner = Rect::new(
        area.x.saturating_add(3),
        area.y.saturating_add(2),
        area.width.saturating_sub(5),
        area.height.saturating_sub(3),
    );
    if inner.height == 0 {
        return;
    }
    if matches!(app.screen, Screen::Feed(_)) {
        let mut tabs = Vec::new();
        for (log, key) in [
            ("latest", "latest"),
            ("trending", "trending"),
            ("following", "following"),
        ] {
            let active = app.screen == Screen::Feed(log);
            tabs.push(Span::styled(
                format!("{} {}    ", app.keymap.label(key), log.to_uppercase()),
                if active {
                    Style::default()
                        .fg(app.accent())
                        .add_modifier(Modifier::BOLD | Modifier::UNDERLINED)
                } else {
                    Style::default().fg(MUTED)
                },
            ));
        }
        frame.render_widget(
            Paragraph::new(Line::from(tabs)),
            Rect { height: 1, ..inner },
        );
        if app.new_commits > 0 {
            frame.render_widget(
                Paragraph::new(format!("↑ {} new commits · R refresh", app.new_commits))
                    .style(Style::default().fg(app.accent())),
                Rect {
                    y: inner.y.saturating_add(1),
                    height: 1,
                    ..inner
                },
            );
        }
        inner.y = inner.y.saturating_add(3);
        inner.height = inner.height.saturating_sub(3);
    }
    let positions = tree_positions(app);
    let reply_context = selected_reply_context(app, &positions)
        .map(|context| format!("   {context}"))
        .unwrap_or_default();
    frame.render_widget(
        Paragraph::new(format!(
            "{}   {:02}/{:02}{reply_context}",
            if matches!(app.screen, Screen::Feed(_)) {
                "LOG".into()
            } else {
                app.screen.title()
            },
            if app.entries.is_empty() {
                0
            } else {
                app.selected + 1
            },
            app.entries.len()
        ))
        .style(
            Style::default()
                .fg(app.accent())
                .add_modifier(Modifier::BOLD),
        ),
        Rect { height: 1, ..inner },
    );
    inner.y = inner.y.saturating_add(2);
    inner.height = inner.height.saturating_sub(2);
    if inner.height == 0 {
        return;
    }
    if let Some(profile) = &app.profile {
        let name = profile.name.as_deref().unwrap_or(&profile.username);
        let bio = profile.bio.as_deref().unwrap_or("No bio");
        let counts = format!(
            " {} commits  {} followers  {} following  · u / i",
            profile.commits_count.unwrap_or(0),
            profile.followers_count.unwrap_or(0),
            profile.following_count.unwrap_or(0)
        );
        let mut profile_line = user_avatar(Some(profile));
        profile_line.push(Span::styled(
            format!(" @{name}  /  @{}", profile.username),
            Style::default()
                .fg(lane_color(Some(profile)))
                .add_modifier(Modifier::BOLD),
        ));
        let mut head = vec![
            Line::from(profile_line),
            Line::styled(format!(" {bio}"), Style::default().fg(Color::Reset)),
            Line::styled(counts, Style::default().fg(MUTED)),
            Line::styled(
                format!(
                    " a commits{} · e replies{}",
                    if matches!(app.screen, Screen::Profile(_)) {
                        " ●"
                    } else {
                        ""
                    },
                    if matches!(app.screen, Screen::ProfileReplies(_)) {
                        " ●"
                    } else {
                        ""
                    }
                ),
                Style::default().fg(app.accent()),
            ),
        ];
        {
            for (label, value) in [
                ("Company", &profile.company),
                ("Location", &profile.location),
                ("GitHub", &profile.github_url),
                ("Website", &profile.website),
            ] {
                if let Some(value) = value {
                    head.push(Line::raw(format!(
                        " {label}: {}",
                        link_label(value).unwrap_or_else(|| value.clone())
                    )));
                }
            }
            if let Some(date) = &profile.created_at {
                head.push(Line::styled(
                    format!(" Joined {}", date.get(..10).unwrap_or(date)),
                    Style::default().fg(MUTED),
                ));
            }
            if let Some(streaks) = &profile.streaks {
                head.push(Line::raw(format!(
                    " Streak: {} current · {} best",
                    streaks.current, streaks.longest
                )));
            }
            if let Some(activity) = &profile.activity {
                head.push(Line::styled(
                    format!(
                        " Activity: {} · {} days",
                        activity
                            .buckets
                            .iter()
                            .map(|bucket| bucket.count)
                            .sum::<u64>(),
                        activity.buckets.len()
                    ),
                    Style::default().fg(MUTED),
                ));
            }
            if let Some(achievements) = &profile.achievements {
                head.push(Line::styled(
                    format!(
                        " Level {} · {} · {} XP",
                        achievements.level, achievements.rank, achievements.xp
                    ),
                    Style::default().fg(app.accent()),
                ));
                head.push(Line::styled(
                    format!(
                        " {}% to next level{}",
                        achievements.level_progress,
                        achievements
                            .next_level_xp
                            .map(|xp| format!(" · {xp} XP"))
                            .unwrap_or_default()
                    ),
                    Style::default().fg(MUTED),
                ));
                for badge in achievements.badges.iter().take(4) {
                    head.push(Line::raw(format!(
                        " {} {}",
                        if badge.unlocked { "◆" } else { "◇" },
                        badge.name
                    )));
                    if !badge.unlocked {
                        head.push(Line::styled(
                            format!(
                                "    {} · {}/{}",
                                badge.description, badge.progress, badge.target
                            ),
                            Style::default().fg(MUTED),
                        ));
                    }
                }
            }
        }
        if app
            .me
            .as_ref()
            .is_some_and(|me| me.username != profile.username)
        {
            head.push(Line::styled(
                format!(
                    " f {} · m {} · B block",
                    if profile.is_followed == Some(true) {
                        "unfollow"
                    } else {
                        "follow"
                    },
                    if profile.is_muting == Some(true) {
                        "unmute"
                    } else {
                        "mute"
                    }
                ),
                Style::default().fg(app.accent()),
            ));
        }
        let head_area = Rect {
            height: inner.height.min(head.len() as u16 + 1),
            ..inner
        };
        let head = Text::from(head);
        frame.render_widget(Paragraph::new(head), head_area);
        inner.y += head_area.height;
        inner.height -= head_area.height;
    }
    if app.entries.is_empty() {
        let message = if app.busy {
            "  Fetching commits…"
        } else if app.screen.private() && app.me.is_none() {
            "  Sign in with L to see this view.\n  Public profiles: / username"
        } else if app.screen == Screen::Welcome {
            "  Welcome to /dev/null. Press L to sign in, / to browse."
        } else {
            "  Nothing here yet. R refresh · Esc back"
        };
        frame.render_widget(
            Paragraph::new(message).style(Style::default().fg(MUTED)),
            inner,
        );
        return;
    }
    let items: Vec<ListItem> = app
        .entries
        .iter()
        .enumerate()
        .map(|(index, entry)| match entry {
            Entry::Commit(commit) => commit_item(
                commit,
                inner
                    .width
                    .saturating_sub((positions[index].parents.len() * 3 + 6) as u16)
                    as usize,
                &positions[index],
                app.expanded.get(&commit.id),
                app.accent(),
                inner.height.saturating_sub(1) as usize,
                positions
                    .get(index + 1)
                    .and_then(|next| next.visual_parent_id.as_deref())
                    == Some(commit.id.as_str()),
            ),
            Entry::User(user) => {
                let mut author = user_avatar(Some(user));
                author.push(Span::styled(
                    format!(" @{}", user.username),
                    Style::default()
                        .fg(lane_color(Some(user)))
                        .add_modifier(Modifier::BOLD),
                ));
                if let Some(name) = user.name.as_deref().filter(|name| !name.is_empty()) {
                    author.push(Span::raw(format!(" · {name}")));
                }
                ListItem::new(vec![
                    Line::from(author),
                    Line::raw(user.bio.clone().unwrap_or_default()),
                    Line::styled(
                        format!("{} followers", user.followers_count.unwrap_or(0)),
                        Style::default().fg(MUTED),
                    ),
                    Line::raw(" "),
                ])
            }
            Entry::Notice(notice) => notice_item(notice),
        })
        .collect();
    let item_heights: Vec<_> = items.iter().map(ListItem::height).collect();
    let list = List::new(items)
        .highlight_symbol(if app.focus == Pane::Feed {
            "▎ "
        } else {
            "· "
        })
        .highlight_style(Style::default());
    let mut state = ListState::default().with_selected(Some(app.selected));
    frame.render_stateful_widget(list, inner, &mut state);

    let bottom = inner.y.saturating_add(inner.height);
    let mut item_y = inner.y;
    for (index, height) in item_heights.iter().enumerate().skip(state.offset()) {
        if item_y >= bottom {
            break;
        }
        if index == app.selected && app.focus == Pane::Feed {
            frame.render_widget(
                Paragraph::new(Line::from(Span::styled(
                    "▏",
                    Style::default().fg(app.accent()),
                ))),
                Rect::new(inner.x, item_y, 1, 1),
            );
        }
        item_y = item_y.saturating_add(*height as u16);
    }
}

fn relative_time(timestamp: Option<&str>) -> String {
    let Some(timestamp) = timestamp else {
        return "time unknown".into();
    };
    let Ok(date) = OffsetDateTime::parse(timestamp, &Rfc3339) else {
        return "time unknown".into();
    };
    let seconds = (OffsetDateTime::now_utc() - date).whole_seconds().max(0) as u64;
    match seconds {
        0..60 => "now".into(),
        60..3600 => format!("{}m", seconds / 60),
        3600..86400 => format!("{}h", seconds / 3600),
        86400..604800 => format!("{}d", seconds / 86400),
        _ => timestamp.get(..10).unwrap_or(timestamp).to_string(),
    }
}

fn short_id(commit: &Commit) -> &str {
    commit.short_hash.as_deref().unwrap_or_else(|| {
        commit
            .id
            .get(commit.id.len().saturating_sub(7)..)
            .unwrap_or(&commit.id)
    })
}

fn link_label(url: &str) -> Option<String> {
    let parsed = Url::parse(url).ok()?;
    if !matches!(parsed.scheme(), "http" | "https") {
        return None;
    }
    let parts: Vec<_> = parsed
        .path_segments()?
        .filter(|part| !part.is_empty())
        .collect();
    match parsed.host_str()?.to_ascii_lowercase().as_str() {
        "dev-null.sh" | "www.dev-null.sh" => {
            let name = parts.first()?.strip_prefix('@')?;
            api::valid_username(name)?;
            let id = if parts.get(1) == Some(&"commits") {
                parts.get(2).map(|id| format!("/{id}"))
            } else {
                None
            };
            Some(format!("//{name}{}", id.unwrap_or_default()))
        }
        "github.com" | "www.github.com" => {
            let owner = parts.first()?;
            let repo = parts.get(1).copied();
            if !owner
                .bytes()
                .all(|byte| byte.is_ascii_alphanumeric() || byte == b'-')
                || repo.is_some_and(|repo| {
                    !repo.bytes().all(|byte| {
                        byte.is_ascii_alphanumeric() || byte == b'-' || byte == b'_' || byte == b'.'
                    })
                })
            {
                return None;
            }
            Some(format!(
                "🐙 {owner}{}",
                repo.map(|repo| format!("/{repo}")).unwrap_or_default()
            ))
        }
        _ => None,
    }
}

fn add_text_links(text: &str, links: &mut Vec<(String, String)>) {
    for word in text.split_whitespace() {
        let url = word.trim_end_matches(['.', ',', ')', ';']);
        let Ok(parsed) = Url::parse(url) else {
            continue;
        };
        if !matches!(parsed.scheme(), "http" | "https")
            || parsed.host_str().is_none()
            || !parsed.username().is_empty()
        {
            continue;
        }
        let label = link_label(url).unwrap_or_else(|| url.to_owned());
        let needle = label.strip_prefix("🐙 ").unwrap_or(&label).to_owned();
        if !needle.is_empty() && !links.iter().any(|(existing, _)| existing == &needle) {
            links.push((needle, url.to_owned()));
        }
    }
}

fn add_commit_links(commit: &Commit, links: &mut Vec<(String, String)>) {
    if let Some(body) = &commit.body {
        add_text_links(body, links);
    }
    if let Some(body) = commit
        .quoted_commit
        .as_deref()
        .and_then(|quote| quote.body.as_deref())
    {
        add_text_links(body, links);
    }
    if let Some(preview) = &commit.link_preview {
        add_text_links(&preview.url, links);
        if let Some(title) = &preview.title {
            add_text_links(title, links);
        }
        if let Some(description) = &preview.description {
            add_text_links(description, links);
        }
    }
    if let Some(images) = &commit.images {
        for image in images {
            add_text_links(&image.url, links);
        }
    }
}

fn link_targets(app: &App) -> Vec<(String, String)> {
    let mut links = Vec::new();
    for entry in &app.entries {
        match entry {
            Entry::Commit(commit) => add_commit_links(commit, &mut links),
            Entry::Notice(notice) => {
                if let Some(commit) = &notice.commit {
                    add_commit_links(commit, &mut links);
                }
            }
            Entry::User(user) => {
                for url in [
                    user.html_url.as_deref(),
                    user.github_url.as_deref(),
                    user.website.as_deref(),
                ]
                .into_iter()
                .flatten()
                {
                    add_text_links(url, &mut links);
                }
            }
        }
    }
    if let Some(profile) = &app.profile {
        for url in [
            profile.html_url.as_deref(),
            profile.github_url.as_deref(),
            profile.website.as_deref(),
        ]
        .into_iter()
        .flatten()
        {
            add_text_links(url, &mut links);
        }
    }
    links
}

fn find_link_regions(app: &App, buffer: &ratatui::buffer::Buffer, area: Rect) -> Vec<LinkRegion> {
    let mut regions = Vec::new();
    for (needle, url) in link_targets(app) {
        for y in area.y..area.bottom() {
            for x in area.x..area.right() {
                let mut found = String::new();
                for end in x..area.right() {
                    let symbol = buffer[(end, y)].symbol();
                    if symbol.is_empty() || !needle.starts_with(&found) {
                        break;
                    }
                    found.push_str(symbol);
                    if found == needle {
                        regions.push(LinkRegion {
                            area: Rect::new(x, y, end - x + 1, 1),
                            url: url.clone(),
                        });
                        break;
                    }
                    if !needle.starts_with(&found) {
                        break;
                    }
                }
            }
        }
    }
    regions
}

fn display_links(text: &str) -> String {
    text.split_whitespace()
        .map(|word| {
            let url = word.trim_end_matches(['.', ',', ')', ';']);
            link_label(url)
                .map(|label| format!("{label}{}", &word[url.len()..]))
                .unwrap_or_else(|| {
                    if word == "/dev/null" {
                        "// /dev/null".into()
                    } else {
                        word.into()
                    }
                })
        })
        .collect::<Vec<_>>()
        .join(" ")
}

fn body_lines(body: &str, width: usize) -> Vec<Line<'static>> {
    let mut lines = Vec::new();
    let mut in_code = false;
    for raw in body.lines() {
        if raw.trim_start().starts_with("```") {
            in_code = !in_code;
            continue;
        }
        if in_code {
            lines.push(Line::raw(
                raw.chars().take(width.max(1)).collect::<String>(),
            ));
        } else {
            lines.extend(
                preview_lines(&display_links(raw), width)
                    .into_iter()
                    .map(Line::raw),
            );
        }
        if lines.len() >= 8 {
            lines.push(Line::raw("…"));
            break;
        }
    }
    if lines.is_empty() {
        lines.push(Line::raw("[deleted]"));
    }
    lines
}

fn preview_lines(body: &str, width: usize) -> Vec<String> {
    let body = body.split_whitespace().collect::<Vec<_>>().join(" ");
    let mut lines = vec![String::new()];
    let mut words = body.split_whitespace().peekable();
    while let Some(word) = words.next() {
        let line = lines.last_mut().expect("at least one line");
        if !line.is_empty() && line.chars().count() + word.chars().count() + 1 > width.max(8) {
            if lines.len() == 2 {
                lines.last_mut().expect("second line").push('…');
                break;
            }
            lines.push(String::new());
        }
        let last = lines.len() == 2;
        let line = lines.last_mut().expect("at least one line");
        if !line.is_empty() {
            line.push(' ');
        }
        line.push_str(word);
        if last && words.peek().is_some() && line.chars().count() >= width.max(8) {
            line.push('…');
            break;
        }
    }
    lines
}

fn lane_color(user: Option<&User>) -> Color {
    match user.and_then(|user| user.lane.as_ref()) {
        Some(Lane::Index(index)) => lane_color_index(*index),
        Some(Lane::Custom(hex)) => hex
            .strip_prefix('#')
            .filter(|hex| hex.len() == 6)
            .and_then(|hex| u32::from_str_radix(hex, 16).ok())
            .map(|rgb| Color::Rgb((rgb >> 16) as u8, (rgb >> 8) as u8, rgb as u8))
            .unwrap_or_else(primary_color),
        None => primary_color(), // Public user lanes are omitted; don't guess a lane.
    }
}

fn tree_prefix(tree: &TreePosition, own: Color) -> Vec<Span<'static>> {
    let mut prefix = tree
        .parents
        .iter()
        .map(|(continues, color)| {
            Span::styled(
                if *continues { "│  " } else { "   " },
                Style::default().fg(*color),
            )
        })
        .collect::<Vec<_>>();
    prefix.push(Span::styled("│  ", Style::default().fg(own)));
    prefix
}

fn tree_gap(tree: &TreePosition, own: Color, next_is_child: bool) -> Line<'static> {
    let mut gap = tree_prefix(tree, own);
    if !next_is_child {
        gap.pop();
        gap.push(Span::styled("   ", Style::default().fg(own)));
    }
    Line::from(gap)
}

fn commit_item(
    commit: &Commit,
    width: usize,
    tree: &TreePosition,
    expanded: Option<&Expanded>,
    accent: Color,
    max_lines: usize,
    next_is_child: bool,
) -> ListItem<'static> {
    let author = commit
        .author
        .as_ref()
        .map(|u| u.username.as_str())
        .unwrap_or("deleted");
    let date = relative_time(commit.created_at.as_deref());
    let own_color = lane_color(commit.author.as_ref());
    let body = body_lines(commit.body.as_deref().unwrap_or("[deleted]"), width);
    let mut header = Vec::new();
    if tree.parents.is_empty() {
        header.push(Span::styled("│ ", Style::default().fg(own_color)));
    } else {
        for (index, (continues, color)) in tree.parents.iter().enumerate() {
            let glyph = if index + 1 == tree.parents.len() {
                if *continues {
                    "├─ "
                } else {
                    "└─ "
                }
            } else if *continues {
                "│  "
            } else {
                "   "
            };
            header.push(Span::styled(glyph, Style::default().fg(*color)));
        }
    }
    header.extend(user_avatar(commit.author.as_ref()));
    header.push(Span::raw(" "));
    header.extend([
        Span::styled(
            format!("@{author}"),
            Style::default().fg(own_color).add_modifier(Modifier::BOLD),
        ),
        Span::styled(
            format!("  · {date}  #{:7}", short_id(commit)),
            Style::default().fg(MUTED),
        ),
    ]);
    if commit.parent_id.is_some() {
        header.push(Span::styled(
            "  ↳ REPLY",
            Style::default().fg(primary_color()),
        ));
    }
    if commit.status.as_deref() == Some("pending") {
        header.push(Span::styled(
            "  IN REVIEW",
            Style::default().fg(primary_color()),
        ));
    }
    let mut lines = vec![Line::from(header)];
    for line in body {
        let mut row = tree_prefix(tree, own_color);
        row.extend(line.spans);
        lines.push(Line::from(row));
    }
    for info in post_info(commit, width) {
        let mut row = tree_prefix(tree, own_color);
        row.extend(info.spans);
        lines.push(Line::from(row));
    }
    let mut stats = tree_prefix(tree, own_color);
    stats.push(Span::styled(
        format!(
            "{} {}    ",
            if expanded.is_some() { "▾" } else { "▸" },
            commit.replies_count.unwrap_or(0)
        ),
        Style::default().fg(accent),
    ));
    stats.push(Span::styled("★", Style::default().fg(star_color())));
    stats.push(Span::styled(
        format!(
            " {}{}",
            commit.stars_count.unwrap_or(0),
            if commit.starred == Some(true) {
                " ✓"
            } else {
                ""
            }
        ),
        Style::default().fg(MUTED),
    ));
    lines.push(Line::from(stats));
    if let Some(count) = commit.pasteis_count.filter(|count| *count > 0) {
        let mut row = tree_prefix(tree, own_color);
        row.push(Span::styled(
            format!("♡ {count} pastéis"),
            Style::default().fg(star_color()),
        ));
        lines.push(Line::from(row));
    }
    if let Some(expanded) = expanded {
        let hint = if expanded.loading {
            "↳ loading replies…"
        } else if expanded.error && expanded.next.is_some() {
            "↳ failed to load · ] retry"
        } else if expanded.error {
            "↳ failed · Space close, then reopen"
        } else if expanded.count == 0 {
            "↳ no replies"
        } else if expanded.next.is_some() {
            "↳ ] more replies"
        } else {
            "↳ Space collapse"
        };
        let mut row = tree_prefix(tree, own_color);
        row.push(Span::styled(hint, Style::default().fg(MUTED)));
        lines.push(Line::from(row));
    }
    lines.push(tree_gap(tree, own_color, next_is_child));
    if lines.len() > max_lines.max(1) {
        lines.truncate(max_lines.max(1));
        if let Some(last) = lines.last_mut() {
            *last = Line::styled("   … more · o open web", Style::default().fg(MUTED));
        }
    }
    ListItem::new(lines)
}

fn post_info(commit: &Commit, width: usize) -> Vec<Line<'static>> {
    let mut info = Vec::new();
    if let Some(parent_id) = commit.parent_id.as_deref() {
        let parent = commit.parent.as_deref();
        let mut line = vec![Span::raw("↰ reply to ")];
        if let Some(author) = parent.and_then(|parent| parent.author.as_ref()) {
            line.push(Span::styled(
                format!("@{}", author.username),
                Style::default().fg(lane_color(Some(author))),
            ));
        } else {
            line.push(Span::raw(format!("#{}", short_related_id(parent_id))));
        }
        if let Some(detail) = parent
            .filter(|parent| parent.deleted_at.is_some())
            .map(|_| "[deleted]".to_owned())
            .or_else(|| {
                parent
                    .and_then(|parent| parent.body.as_deref())
                    .map(|body| preview_lines(body, 50).first().cloned().unwrap_or_default())
            })
            .or_else(|| {
                parent.map(|parent| {
                    format!(
                        "#{}",
                        short_related_id(parent.short_hash.as_deref().unwrap_or(&parent.id))
                    )
                })
            })
        {
            line.push(Span::raw(format!(" · {detail}")));
        }
        info.push(Line::from(line));
    }
    if let Some(id) = &commit.quoted_commit_id {
        let quote = commit.quoted_commit.as_deref();
        let author = quote
            .and_then(|commit| commit.author.as_ref())
            .map(|user| user.username.clone())
            .unwrap_or_else(|| "author".into());
        let hash = quote
            .map(|commit| short_related_id(commit.short_hash.as_deref().unwrap_or(id)))
            .unwrap_or_else(|| short_related_id(id));
        let color = lane_color(quote.and_then(|commit| commit.author.as_ref()));
        let mut header = vec![
            Span::styled("╭─ ", Style::default().fg(color)),
            Span::styled(
                format!("@{author}"),
                Style::default().fg(color).add_modifier(Modifier::BOLD),
            ),
            Span::styled(format!("  #{hash}"), Style::default().fg(MUTED)),
        ];
        if quote.is_none() {
            header.push(Span::styled(" · quoted commit", Style::default().fg(MUTED)));
        }
        info.push(Line::from(header));
        let body = quote
            .filter(|commit| commit.deleted_at.is_some())
            .map(|_| "[deleted]".to_owned())
            .or_else(|| quote.and_then(|commit| commit.body.clone()))
            .unwrap_or_default();
        for text in preview_lines(&display_links(&body), width.saturating_sub(4)) {
            info.push(Line::from(vec![
                Span::styled("│ ", Style::default().fg(color)),
                Span::raw(text),
            ]));
        }
        info.push(Line::styled("╰─", Style::default().fg(color)));
    }
    if let Some(link) = &commit.link_preview {
        let provider = link
            .embed
            .as_ref()
            .map(|embed| embed.provider.as_str())
            .or(link.site_name.as_deref())
            .unwrap_or("Link");
        let label = link_label(&link.url).unwrap_or_else(|| link.url.clone());
        info.push(Line::raw(format!(
            "Link · {provider}: {}",
            link.title.as_deref().unwrap_or(&link.url)
        )));
        if let Some(description) = &link.description {
            info.extend(
                preview_lines(description, width)
                    .into_iter()
                    .map(|line| Line::raw(format!("  {line}"))),
            );
        }
        info.push(Line::raw(format!(
            "  {label}{}",
            if link.is_pending == Some(true) {
                " · preview pending"
            } else {
                ""
            }
        )));
    }
    if let Some(images) = commit.images.as_ref().filter(|images| !images.is_empty()) {
        for image in images {
            info.push(Line::raw(format!(
                "Image · {}{} · {}",
                image.description.as_deref().unwrap_or("Image"),
                image
                    .width
                    .zip(image.height)
                    .map(|(width, height)| format!(" ({width}×{height})"))
                    .unwrap_or_default(),
                link_label(&image.url).unwrap_or_else(|| image.url.clone())
            )));
        }
    }
    if let Some(poll) = &commit.poll {
        info.push(Line::raw(format!(
            "Poll · {} · {}",
            poll.total_votes
                .map(|count| format!("{count} votes"))
                .unwrap_or_else(|| "results hidden".into()),
            if poll.is_closed {
                "closed"
            } else if poll.has_voted {
                "voted · open"
            } else {
                "open"
            }
        )));
        for option in poll.options.iter().take(4) {
            let results = option
                .percentage
                .map(|percentage| format!("{percentage}%"))
                .or_else(|| option.votes_count.map(|count| format!("{count} votes")))
                .unwrap_or_else(|| "votes hidden".into());
            info.push(Line::raw(format!(
                "  {}{} · {results}",
                if option.voted { "✓ " } else { "" },
                option.label
            )));
        }
    }
    info
}

fn short_related_id(id: &str) -> &str {
    id.get(id.len().saturating_sub(7)..).unwrap_or(id)
}

fn selected_reply_context(app: &App, positions: &[TreePosition]) -> Option<String> {
    let commit = entry_commit(app.entries.get(app.selected)?)?;
    let parent_id = commit.parent_id.as_deref()?;
    let author = commit
        .parent
        .as_deref()
        .and_then(|parent| parent.author.as_ref())
        .map(|author| author.username.as_str())
        .or_else(|| {
            app.entries
                .iter()
                .filter_map(entry_commit)
                .find(|parent| parent.id == parent_id)
                .and_then(|parent| parent.author.as_ref())
                .map(|author| author.username.as_str())
        });
    let parent = author
        .map(|author| format!("@{author}"))
        .unwrap_or_else(|| format!("#{}", short_related_id(parent_id)));
    let depth = positions
        .get(app.selected)
        .map_or(1, |position| position.depth.max(1));
    Some(format!("↳ {parent} · depth {depth}"))
}

fn notice_item(notice: &Notice) -> ListItem<'static> {
    let mut header = user_avatar(notice.actor.as_ref());
    header.push(Span::raw(format!(" {} · ", notice.kind)));
    header.push(Span::styled(
        format!(
            "@{}",
            notice
                .actor
                .as_ref()
                .map(|user| user.username.as_str())
                .unwrap_or("system")
        ),
        Style::default()
            .fg(lane_color(notice.actor.as_ref()))
            .add_modifier(Modifier::BOLD),
    ));
    let mut lines = vec![Line::from(header)];
    if let Some(body) = notice
        .commit
        .as_ref()
        .and_then(|commit| commit.body.as_deref())
    {
        lines.extend(body_lines(body, 72));
    }
    lines.extend([
        Line::styled(
            format!(
                "{} events · {} unread",
                notice.event_count.unwrap_or(1),
                notice.unread_count.unwrap_or(0)
            ),
            Style::default().fg(MUTED),
        ),
        Line::raw(" "),
    ]);
    ListItem::new(lines)
}

fn palette(frame: &mut Frame, app: &App, size: Rect, query: &str, selected: usize) {
    let width = size.width.saturating_sub(4).min(72);
    let height = size.height.saturating_sub(4).min(17);
    let area = Rect::new(
        (size.width - width) / 2,
        (size.height - height) / 2,
        width,
        height,
    );
    frame.render_widget(Clear, area);
    let block = pane(
        format!("COMMAND CENTER · {}", app.keymap.label("commands")),
        true,
        app.accent(),
    );
    let inner = block.inner(area);
    frame.render_widget(block, area);
    let rows = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Length(2),
            Constraint::Min(1),
            Constraint::Length(1),
        ])
        .split(inner);
    frame.render_widget(
        Paragraph::new(format!("  : {query}█")).block(
            Block::default()
                .borders(Borders::BOTTOM)
                .border_style(Style::default().fg(MUTED)),
        ),
        rows[0],
    );
    let results = commands(query, app);
    let items: Vec<ListItem> = results
        .iter()
        .map(|(name, key)| {
            let binding = if *key == "request_log" {
                "Ctrl+Shift+L / Ctrl+L".into()
            } else {
                app.keymap.label(key)
            };
            ListItem::new(Line::from(vec![
                Span::styled(format!("  {name:<28}"), Style::default().fg(Color::Reset)),
                Span::styled(binding, Style::default().fg(app.accent())),
            ]))
        })
        .collect();
    if items.is_empty() {
        frame.render_widget(
            Paragraph::new("  No matching commands").style(Style::default().fg(MUTED)),
            rows[1],
        );
    } else {
        let mut state = ListState::default().with_selected(Some(selected));
        frame.render_stateful_widget(
            List::new(items).highlight_symbol("▏ ").highlight_style(
                Style::default()
                    .fg(app.accent())
                    .add_modifier(Modifier::UNDERLINED | Modifier::BOLD),
            ),
            rows[1],
            &mut state,
        );
    }
    frame.render_widget(
        Paragraph::new("  ↑↓ select · Enter run · Esc close").style(Style::default().fg(MUTED)),
        rows[2],
    );
}

fn overlay(frame: &mut Frame, app: &App, size: Rect) {
    if let Mode::Palette { query, selected } = &app.mode {
        palette(frame, app, size, query, *selected);
        return;
    }
    let (title, body) = match &app.mode {
        Mode::Normal => return,
        Mode::Help { .. } => (
            "KEYBOARD SHORTCUTS · ↑/↓ · Esc",
            help_text(app, size.width.saturating_sub(6).min(82) as usize),
        ),
        Mode::RequestLog { offset, .. } => {
            let entries = app
                .request_log
                .as_ref()
                .map(RequestLog::entries)
                .unwrap_or_default();
            let mut body = format!(
                "{} retained · newest first · request/response bodies and auth headers omitted\n\n",
                entries.len()
            );
            for entry in entries.iter().rev().skip(*offset).take(14) {
                let status = entry
                    .status
                    .map(|status| status.to_string())
                    .unwrap_or_else(|| "ERR".into());
                let outcome = entry
                    .outcome
                    .as_ref()
                    .map(|outcome| format!(" · {outcome}"))
                    .unwrap_or_default();
                body.push_str(&format!(
                    "{:>3}s ago {} {status} {}ms ↑{}B ↓{}B {}{outcome}\n",
                    entry.at.elapsed().as_secs(),
                    entry.method,
                    entry.duration_ms,
                    entry.request_bytes,
                    entry.response_bytes,
                    entry.path
                ));
            }
            body.push_str("\n↑/↓ or j/k scroll · Home oldest · End newest · Esc / Ctrl+Shift+L / Ctrl+L close");
            ("API REQUEST / RESPONSE", body)
        }
        Mode::Settings { selected } => {
            let user = app.me.as_ref();
            let preference = user.and_then(|u| u.preferred_lane.as_ref());
            let current = match preference { Some(Lane::Index(n)) => *n as usize + 1, Some(Lane::Custom(_)) => usize::MAX, None => 0 };
            let automatic = user.and_then(|u| u.automatic_lane).map(|n| format!(" (auto #{})", n + 1)).unwrap_or_default();
            let names = ["Automatic", "Lane 1 · red", "Lane 2 · green", "Lane 3 · magenta", "Lane 4 · cyan", "Lane 5 · light red", "Lane 6 · yellow"];
            let mut body = String::from("↑/↓ choose · Enter save/toggle · C custom hex color\n");
            for (index, name) in names.iter().enumerate() {
                let name = if index == 0 { format!("{name}{automatic}") } else { (*name).to_string() };
                body.push_str(&format!("{} {}{}\n", mark(*selected == index), name, if current == index { "  ✓" } else { "" }));
            }
            if let Some(Lane::Custom(color)) = preference { body.push_str(&format!("Current custom accent: {color}\n")); }
            let mode = if user.and_then(|u| u.pasteis_mode).unwrap_or(false) { "on" } else { "off" };
            let remaining = user.and_then(|u| u.pasteis_remaining).map(|n| format!(" · {n} remaining")).unwrap_or_default();
            let received = user.and_then(|u| u.pasteis_received).map(|n| format!(" · {n} received")).unwrap_or_default();
            let synced = user.and_then(|u| u.github_synced_at.as_deref()).map(|date| relative_time(Some(date))).unwrap_or_else(|| "never".into());
            body.push_str(&format!("{} Pastéis mode: {mode}{remaining}{received}\n{} Sync GitHub profile · last {synced}.", mark(*selected == 7), mark(*selected == 8)));
            ("ACCOUNT SETTINGS", body)
        }
        Mode::CustomColor(input) => ("SET CUSTOM ACCENT", format!("Hex color: {input}█\n\nEnter #RRGGBB · Enter save · Esc cancel")),
        Mode::Search(input) => ("FIND USER", format!("GitHub username:\n\n  {input}█\n\nEnter open · Esc cancel")),
        Mode::WebSearch(input) => ("SEARCH DEVELOPERS + COMMITS", format!("Official search opens in your browser; no /api/v1 search endpoint.\n\n  {input}█\n\nEnter search · Esc cancel")),
        Mode::ComposePreview { text, .. } => ("COMMIT PREVIEW", format!("{text}\n\n⌘⇧P edit · ⌘K submit · Esc edit")),
        Mode::Compose { text, parent, quote } => (if parent.is_some() { "REPLY" } else if quote.is_some() { "QUOTE" } else { "NEW COMMIT" },
            format!("{} / 4000 characters\n\n{text}█\n\n[Image · unavailable]  [Poll · unavailable]  [Schedule · unavailable]\nLocal preview: {} · {} submit · Alt+Enter newline · Esc cancel", text.chars().count(), app.keymap.label("preview"), app.keymap.label("submit"))),
        Mode::LoginSetup { .. } => ("CONNECT TO /DEV/NULL", "Upstream redirects to devnull:// — not localhost.\n\nFor any browser, register a system URL handler that forwards\nthe approval code to this TUI. This changes the default\ndevnull:// handler for your user and persists after exit.\nAn existing handler will never be replaced.\n\nEnter  register handler + open browser\nm      manual callback (browser Network → 303 Location)\nEsc    cancel".into()),
        Mode::Login { flow, input } => ("AWAITING APPROVAL", format!("Status: {}\nApprove in your default browser; allow the callback app if asked.\n\nAuthorization URL (b to reopen):\n{}\n\nThe callback returns here automatically.\nManual: paste the full devnull:// URL:\n{input}█\nEnter redeem · H retry handler · Esc cancel · code expires in 5 minutes.", api::display_text(&app.status), flow.url)),
        Mode::Palette { .. } => return,
        Mode::Report(_) => ("REPORT COMMIT", "Choose reason; report goes privately to moderators.\n\n1 spam      2 harassment\n3 unsafe    4 other\n\nEsc cancel".into()),
        Mode::Block { username, remove } => ("CONFIRM SAFETY CHANGE", format!("{} @{}?\n\nBlocking ends follows both ways; unblocking does not restore them.\n\ny confirm · Esc cancel", if *remove { "Unblock" } else { "Block" }, username)),
    };
    let width = size
        .width
        .saturating_sub(4)
        .min(if matches!(app.mode, Mode::RequestLog { .. }) {
            110
        } else {
            84
        });
    let height = size.height.saturating_sub(4).min(
        if matches!(app.mode, Mode::Settings { .. } | Mode::Login { .. }) {
            18
        } else if matches!(app.mode, Mode::ComposePreview { .. }) {
            12
        } else if matches!(app.mode, Mode::WebSearch(_) | Mode::CustomColor(_)) {
            8
        } else if matches!(app.mode, Mode::LoginSetup { .. }) {
            14
        } else if matches!(app.mode, Mode::RequestLog { .. } | Mode::Help { .. }) {
            24
        } else {
            12
        },
    );
    let area = Rect::new(
        (size.width - width) / 2,
        (size.height - height) / 2,
        width,
        height,
    );
    frame.render_widget(Clear, area);
    let scroll = match &app.mode {
        Mode::Help { offset } => (*offset as u16, 0),
        _ => (0, 0),
    };
    frame.render_widget(
        Paragraph::new(body)
            .wrap(Wrap { trim: false })
            .scroll(scroll)
            .block(panel(title).border_style(Style::default().fg(app.accent()))),
        area,
    );
}

struct TerminalGuard;
impl TerminalGuard {
    fn enter() -> io::Result<Self> {
        enable_raw_mode()?;
        if let Err(error) = execute!(
            io::stdout(),
            EnterAlternateScreen,
            EnableBracketedPaste,
            EnableMouseCapture
        ) {
            let _ = disable_raw_mode();
            return Err(error);
        }
        Ok(Self)
    }
}
impl Drop for TerminalGuard {
    fn drop(&mut self) {
        let _ = disable_raw_mode();
        let _ = execute!(
            io::stdout(),
            DisableMouseCapture,
            DisableBracketedPaste,
            LeaveAlternateScreen
        );
    }
}

fn run(log_enabled: bool) -> Result<(), String> {
    let mut api = Api::new()?;
    let request_log = log_enabled.then(RequestLog::default);
    if let Some(log) = &request_log {
        api.enable_request_log(log.clone());
    }
    let key = api.key().clone();
    let (job_tx, job_rx) = mpsc::channel();
    let (reply_tx, reply_rx) = mpsc::channel();
    thread::spawn(move || worker(api, job_rx, reply_tx));
    let mut app = App::new(job_tx, reply_rx, key);
    app.request_log = request_log;
    let _ = app.sender.send(Job::Bootstrap);
    app.load(None);
    let _guard = TerminalGuard::enter().map_err(|e| e.to_string())?;
    LIGHT_THEME.store(detect_light_theme(), Ordering::Relaxed);
    let mut terminal =
        Terminal::new(CrosstermBackend::new(io::stdout())).map_err(|e| e.to_string())?;
    let size = terminal.size().map_err(|e| e.to_string())?;
    app.width = size.width;
    app.height = size.height;
    while !app.quit {
        app.collect();
        terminal
            .draw(|frame| draw(frame, &app))
            .map_err(|e| e.to_string())?;
        if event::poll(Duration::from_millis(100)).map_err(|e| e.to_string())? {
            match event::read().map_err(|e| e.to_string())? {
                Event::Key(key) => app.key(key),
                Event::Paste(text) => app.paste(text),
                Event::Mouse(mouse) if mouse.kind == MouseEventKind::Down(MouseButton::Left) => {
                    app.open_link_at(mouse.column, mouse.row)
                }
                Event::Resize(width, height) => {
                    app.width = width;
                    app.height = height;
                    if width < 76 && app.focus == Pane::Nav {
                        app.focus = Pane::Feed;
                    }
                }
                _ => {}
            }
        }
    }
    Ok(())
}

fn parse_log_option(args: &[String]) -> Result<bool, String> {
    match args {
        [] => Ok(false),
        [flag] if flag == "--log" || flag == "-l" => Ok(true),
        _ => Err("Usage: stdout [--log|-l]".into()),
    }
}

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    if args.first().map(String::as_str) == Some("--callback") {
        let result = Api::new().and_then(|api| {
            callback::receive(
                api.key(),
                args.get(1).map(String::as_str).unwrap_or_default(),
            )
        });
        if let Err(error) = result {
            eprintln!("stdout callback: {error}");
            std::process::exit(1);
        }
        return;
    }
    if matches!(args.as_slice(), [flag] if flag == "--help" || flag == "-h") {
        println!("Usage: stdout [--log|-l]\n\nEnable an in-memory viewer for dev-null.sh API request/response metadata. Use Ctrl+Shift+L, or Ctrl+L if your terminal drops Shift.");
        return;
    }
    let result = parse_log_option(&args).and_then(run);
    if let Err(error) = result {
        eprintln!("stdout: {error}");
        std::process::exit(1);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use ratatui::backend::TestBackend;

    fn app() -> App {
        let (jobs, _) = mpsc::channel();
        let (_, replies) = mpsc::channel();
        App::new(jobs, replies, PathBuf::new())
    }

    fn terminal_background(buffer: &ratatui::buffer::Buffer, width: u16) -> bool {
        buffer.content().iter().enumerate().all(|(index, cell)| {
            if cell.bg == Color::Reset {
                return true;
            }
            let x = index as u16 % width;
            let y = index as u16 / width;
            x < 14 && (1..9).contains(&y)
        })
    }

    #[test]
    fn login_requires_explicit_handler_choice_without_changing_os() {
        let dir = std::env::temp_dir().join(format!("dev-null-login-{}", rand::random::<u64>()));
        let key = dir.join("device_ed25519");
        let (sender, jobs) = mpsc::channel();
        let (_, receiver) = mpsc::channel();
        let mut app = App::new(sender, receiver, key.clone());
        app.busy = true; // Login must still be accessible while public content loads.
        app.login();
        assert!(matches!(app.mode, Mode::LoginSetup { .. }));
        assert!(callback::socket_path(&key).exists());
        assert!(!dir.join("Callback.app").exists());
        let mut terminal = Terminal::new(TestBackend::new(90, 24)).unwrap();
        terminal.draw(|frame| draw(frame, &app)).unwrap();
        let visible = terminal
            .backend()
            .buffer()
            .content()
            .iter()
            .map(|cell| cell.symbol())
            .collect::<String>();
        assert!(visible.contains("CONNECT TO /DEV/NULL"));
        app.key(KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE));
        assert!(matches!(app.mode, Mode::Login { .. }));
        assert!(matches!(jobs.try_recv(), Ok(Job::InstallHandler)));
        app.busy = false; // Simulate handler registration without changing OS settings.
        callback::receive(&key, "devnull://auth/callback?code=dummy&state=wrong").unwrap();
        app.collect();
        assert!(app.status.contains("state mismatch"));
        assert!(jobs.try_recv().is_err());
        let state = match &app.mode {
            Mode::Login { flow, .. } => reqwest::Url::parse(&flow.url)
                .unwrap()
                .query_pairs()
                .find(|(key, _)| key == "state")
                .unwrap()
                .1
                .into_owned(),
            _ => panic!("expected login mode"),
        };
        callback::receive(
            &key,
            &format!("devnull://auth/callback?code=dummy&state={state}"),
        )
        .unwrap();
        app.collect();
        assert!(
            matches!(jobs.try_recv(), Ok(Job::Redeem(code, verifier)) if code == "dummy" && verifier.len() == 43)
        );
        drop(app);
        assert!(!callback::socket_path(&key).exists());
        std::fs::remove_dir_all(dir).unwrap();
    }

    fn entry(id: &str) -> Entry {
        Entry::Commit(Box::new(
            serde_json::from_value(serde_json::json!({
                "id": id, "body": id, "replies_count": 2,
                "author": { "username": id, "lane": 1 }
            }))
            .unwrap(),
        ))
    }

    fn reply_entry(id: &str, parent_id: &str) -> Entry {
        let mut item = entry(id);
        if let Entry::Commit(commit) = &mut item {
            commit.parent_id = Some(parent_id.into());
        }
        item
    }

    #[test]
    fn reply_tree_keeps_sibling_depth_and_parent_lane_colors() {
        let make = |id: &str, parent: Option<&str>, lane: u8| {
            let mut item = entry(id);
            if let Entry::Commit(commit) = &mut item {
                commit.parent_id = parent.map(str::to_owned);
                commit.replies_count = Some(if matches!(id, "op" | "alice") { 2 } else { 0 });
                commit.author = Some(User {
                    username: id.into(),
                    lane: Some(Lane::Index(lane)),
                    ..Default::default()
                });
            }
            item
        };
        let mut app = app();
        app.entries = vec![
            make("op", None, 0),
            make("alice", Some("op"), 1),
            make("bob", Some("alice"), 2),
            make("kim", Some("alice"), 3),
            make("sam", Some("op"), 4),
        ];
        let positions = tree_positions(&app);
        assert_eq!(positions[2].parents.len(), 2);
        assert_eq!(positions[3].parents.len(), 3);
        assert_eq!(positions[4].parents.len(), 2);
        assert_eq!(
            positions[2].parents,
            vec![(false, lane_color_index(0)), (true, lane_color_index(1))]
        );
        assert_eq!(
            positions[3].parents,
            vec![
                (false, lane_color_index(0)),
                (true, lane_color_index(1)),
                (false, lane_color_index(2)),
            ]
        );
        assert_eq!(
            positions[4].parents,
            vec![(false, lane_color_index(0)), (false, lane_color_index(1))]
        );
        let rails = tree_prefix(&positions[2], lane_color_index(2));
        assert_eq!(
            rails
                .iter()
                .map(|span| span.content.as_ref())
                .collect::<String>(),
            "   │  │  "
        );
        assert_eq!(rails[0].style.fg, Some(lane_color_index(0)));
        assert_eq!(rails[1].style.fg, Some(lane_color_index(1)));
        assert_eq!(rails[2].style.fg, Some(lane_color_index(2)));
        app.selected = 2;
        assert_eq!(
            selected_reply_context(&app, &positions).as_deref(),
            Some("↳ @alice · depth 2")
        );
    }

    #[test]
    fn collapsed_original_commits_keep_independent_thread_lanes() {
        let mut app = app();
        let mut root = entry("root");
        let mut reply = reply_entry("reply", "root");
        let mut other = entry("other");
        for item in [&mut root, &mut reply, &mut other] {
            if let Entry::Commit(commit) = item {
                commit.replies_count = Some(0);
            }
        }
        app.entries = vec![root, reply, other];
        let positions = tree_positions(&app);
        assert!(positions[0].parents.is_empty());
        assert_eq!(positions[1].parents, vec![(false, lane_color_index(1))]);
        assert!(positions[2].parents.is_empty());
        for position in [&positions[0], &positions[2]] {
            assert_eq!(
                tree_prefix(position, lane_color_index(1))
                    .iter()
                    .map(|span| span.content.as_ref())
                    .collect::<String>(),
                "│  "
            );
        }
    }

    #[test]
    fn independent_commits_get_a_blank_separator_and_visible_rails() {
        let mut app = app();
        app.entries = vec![entry("first"), entry("second")];
        for item in &mut app.entries {
            if let Entry::Commit(commit) = item {
                commit.replies_count = Some(0);
            }
        }
        let mut terminal = Terminal::new(TestBackend::new(60, 30)).unwrap();
        terminal.draw(|frame| draw(frame, &app)).unwrap();
        let rows: Vec<String> = (0..30)
            .map(|y| {
                (0..60)
                    .map(|x| terminal.backend().buffer()[(x, y)].symbol())
                    .collect()
            })
            .collect();
        let first = rows.iter().position(|row| row.contains("@first")).unwrap();
        let second = rows.iter().position(|row| row.contains("@second")).unwrap();
        assert!(rows[first].contains("│"));
        assert!(second > first && rows[second - 1].trim().is_empty());
    }

    #[test]
    fn reply_siblings_continue_from_first_reply_rail() {
        let mut app = app();
        let make = |id: &str, parent: Option<&str>, lane: u8, replies| {
            let mut item = entry(id);
            if let Entry::Commit(commit) = &mut item {
                commit.parent_id = parent.map(str::to_owned);
                commit.replies_count = Some(replies);
                commit.author = Some(User {
                    username: id.into(),
                    lane: Some(Lane::Index(lane)),
                    ..Default::default()
                });
            }
            item
        };
        app.entries = vec![
            make("root", None, 0, 3),
            make("replyA", Some("root"), 1, 2),
            make("nestedA1", Some("replyA"), 2, 2),
            make("deepA1a", Some("nestedA1"), 3, 0),
            make("deepA1b", Some("nestedA1"), 4, 0),
            make("nestedA2", Some("replyA"), 5, 0),
            make("replyB", Some("root"), 1, 1),
            make("nestedB1", Some("replyB"), 2, 0),
            make("replyC", Some("root"), 1, 0),
            make("nextOp", None, 0, 0),
        ];
        let positions = tree_positions(&app);
        assert_eq!(positions[1].parents, vec![(false, lane_color_index(0))]);
        assert_eq!(
            tree_gap(&positions[0], lane_color_index(0), true)
                .spans
                .iter()
                .map(|span| span.content.as_ref())
                .collect::<String>(),
            "│  "
        );
        assert_eq!(
            tree_gap(&positions[1], lane_color_index(1), true)
                .spans
                .iter()
                .map(|span| span.content.as_ref())
                .collect::<String>(),
            "   │  "
        );
        assert_eq!(
            positions[2].parents,
            vec![(false, lane_color_index(0)), (true, lane_color_index(1))]
        );
        assert_eq!(
            positions[3].parents,
            vec![
                (false, lane_color_index(0)),
                (true, lane_color_index(1)),
                (true, lane_color_index(2)),
            ]
        );
        assert_eq!(
            tree_gap(&positions[2], lane_color_index(2), true)
                .spans
                .iter()
                .map(|span| span.content.as_ref())
                .collect::<String>(),
            "   │  │  "
        );
        assert_eq!(
            positions[6].parents,
            vec![(false, lane_color_index(0)), (true, lane_color_index(1))]
        );
        assert_eq!(positions[6].visual_parent_id.as_deref(), Some("replyA"));
        assert_eq!(positions[6].depth, 1);
        assert_eq!(
            positions[8].parents,
            vec![(false, lane_color_index(0)), (false, lane_color_index(1))]
        );
        assert!(positions[9].parents.is_empty());

        let mut terminal = Terminal::new(TestBackend::new(90, 70)).unwrap();
        terminal.draw(|frame| draw(frame, &app)).unwrap();
        let rows: Vec<String> = (0..70)
            .map(|y| {
                (0..90)
                    .map(|x| terminal.backend().buffer()[(x, y)].symbol())
                    .collect()
            })
            .collect();
        for row in [
            "└─ RE @replyA",
            "   ├─ NE @nestedA1",
            "   │  ├─ DE @deepA1a",
            "   │  │  └─ DE @deepA1b",
            "   │  └─ NE @nestedA2",
            "   ├─ RE @replyB",
            "   │  └─ NE @nestedB1",
            "   └─ RE @replyC",
            "│ NE @nextOp",
        ] {
            assert!(rows.iter().any(|line| line.contains(row)), "missing {row}");
        }
        let next_op = rows
            .iter()
            .position(|line| line.contains("@nextOp"))
            .unwrap();
        assert!(rows[next_op - 1].trim().is_empty());
    }

    #[test]
    fn pending_commits_show_an_in_review_indicator() {
        let make = |id: &str, status: Option<&str>| {
            let mut item = entry(id);
            if let Entry::Commit(commit) = &mut item {
                commit.status = status.map(str::to_owned);
            }
            item
        };
        let mut app = app();
        app.entries = vec![
            make("pending", Some("pending")),
            make("approved", Some("approved")),
            make("public", None),
        ];
        let mut terminal = Terminal::new(TestBackend::new(90, 30)).unwrap();
        terminal.draw(|frame| draw(frame, &app)).unwrap();
        let rows: Vec<String> = (0..30)
            .map(|y| {
                (0..90)
                    .map(|x| terminal.backend().buffer()[(x, y)].symbol())
                    .collect()
            })
            .collect();
        assert!(rows
            .iter()
            .any(|row| row.contains("@pending") && row.contains("IN REVIEW")));
        assert!(!rows
            .iter()
            .any(|row| row.contains("IN REVIEW") && row.contains("@approved")));
        assert!(!rows
            .iter()
            .any(|row| row.contains("IN REVIEW") && row.contains("@public")));
    }

    #[test]
    fn inline_replies_keep_feed_and_selection_then_collapse() {
        let (sender, jobs) = mpsc::channel();
        let (replies, receiver) = mpsc::channel();
        let mut app = App::new(sender, receiver, PathBuf::new());
        app.screen = Screen::Feed("latest");
        app.me = Some(User {
            username: "me".into(),
            ..Default::default()
        });
        app.entries = vec![entry("a"), entry("b"), entry("c")];
        app.normal(KeyEvent::new(KeyCode::Char(' '), KeyModifiers::NONE));
        let generation = match jobs.try_recv().unwrap() {
            Job::Replies(id, None, generation) if id == "a" => generation,
            _ => panic!("expected reply request"),
        };
        app.selected = 1; // Reading another post must survive insertion above it.
        replies
            .send(Reply::Replies(
                "a".into(),
                generation,
                Ok(Page {
                    entries: vec![reply_entry("r1", "a"), reply_entry("r2", "a")],
                    next: Some("/api/v1/commits/a/replies?page=2".into()),
                    profile: None,
                }),
            ))
            .unwrap();
        app.collect();
        assert_eq!(app.screen, Screen::Feed("latest"));
        assert_eq!(app.selected, 3);
        assert!(matches!(app.current(), Some(Entry::Commit(c)) if c.id == "b"));
        app.selected = 0;
        app.normal(KeyEvent::new(KeyCode::Char(']'), KeyModifiers::NONE));
        assert!(
            matches!(jobs.try_recv(), Ok(Job::Replies(id, Some(_), token)) if id == "a" && token == generation)
        );
        replies
            .send(Reply::Replies(
                "a".into(),
                generation,
                Ok(Page {
                    entries: vec![reply_entry("r3", "a")],
                    next: None,
                    profile: None,
                }),
            ))
            .unwrap();
        app.collect();
        assert_eq!(app.selected, 0);
        app.selected = 0;
        for width in [60, 95] {
            let mut terminal = Terminal::new(TestBackend::new(width, 50)).unwrap();
            terminal.draw(|frame| draw(frame, &app)).unwrap();
            let rows: Vec<String> = (0..50)
                .map(|y| {
                    (0..width)
                        .map(|x| terminal.backend().buffer()[(x, y)].symbol())
                        .collect()
                })
                .collect();
            assert!(rows.iter().any(|row| row.contains("└─ R1 @r1")));
            assert!(rows.iter().any(|row| row.contains("├─ R2 @r2")));
            assert!(rows.iter().any(|row| row.contains("└─ R3 @r3")));
            let middle = rows
                .iter()
                .position(|row| row.contains("└─ R1 @r1"))
                .unwrap();
            assert!(rows[middle + 1].contains('│'));
        }
        app.selected = 1; // Replies are selectable; actions target the reply, not its parent.
        app.normal(KeyEvent::new(KeyCode::Char('r'), KeyModifiers::NONE));
        assert!(
            matches!(app.mode, Mode::Compose { ref parent, .. } if parent.as_deref() == Some("r1"))
        );
        app.mode = Mode::Normal;
        app.normal(KeyEvent::new(KeyCode::Char(' '), KeyModifiers::NONE));
        assert!(matches!(jobs.try_recv(), Ok(Job::Replies(id, None, _)) if id == "r1"));
        assert_eq!(app.entries.len(), 6);
        app.selected = 0;
        app.normal(KeyEvent::new(KeyCode::Char(' '), KeyModifiers::NONE));
        assert_eq!(app.entries.len(), 3);
        assert_eq!(app.selected, 0);
        app.normal(KeyEvent::new(KeyCode::Char(' '), KeyModifiers::NONE));
        let new_generation = match jobs.try_recv().unwrap() {
            Job::Replies(_, None, generation) => generation,
            _ => panic!("expected new reply request"),
        };
        replies
            .send(Reply::Replies(
                "a".into(),
                generation,
                Ok(Page {
                    entries: vec![entry("stale")],
                    next: None,
                    profile: None,
                }),
            ))
            .unwrap();
        app.collect();
        assert_eq!(app.entries.len(), 3);
        assert_ne!(new_generation, generation);
    }

    #[test]
    fn super_k_is_not_search_and_super_p_does_not_open_palette() {
        let (sender, jobs) = mpsc::channel();
        let (_, receiver) = mpsc::channel();
        let mut app = App::new(sender, receiver, PathBuf::new());
        app.me = Some(User {
            username: "me".into(),
            ..Default::default()
        });
        let super_k = KeyEvent::new(KeyCode::Char('k'), KeyModifiers::SUPER);
        app.normal(super_k);
        assert!(matches!(app.mode, Mode::Normal));
        app.normal(KeyEvent::new(KeyCode::Char('k'), KeyModifiers::CONTROL));
        assert!(matches!(app.mode, Mode::WebSearch(_)));
        app.mode = Mode::Normal;
        app.key(KeyEvent::new(KeyCode::Char('P'), KeyModifiers::SUPER));
        assert!(matches!(app.mode, Mode::Normal));
        app.key(KeyEvent::new(KeyCode::Char(':'), KeyModifiers::NONE));
        assert!(matches!(app.mode, Mode::Palette { .. }));
        app.mode = Mode::Compose {
            text: "hello".into(),
            parent: None,
            quote: None,
        };
        app.key(super_k);
        assert!(
            matches!(jobs.try_recv(), Ok(Job::Act(_, Action::Create { body, .. })) if body == "hello")
        );
    }

    #[test]
    fn request_logging_flag_shortcut_and_command_are_opt_in() {
        assert!(!parse_log_option(&[]).unwrap());
        assert!(parse_log_option(&["--log".into()]).unwrap());
        assert!(parse_log_option(&["-l".into()]).unwrap());
        assert!(parse_log_option(&["--other".into()]).is_err());

        let log_key = KeyEvent::new(
            KeyCode::Char('l'),
            KeyModifiers::CONTROL | KeyModifiers::SHIFT,
        );
        let ctrl_l = KeyEvent::new(KeyCode::Char('l'), KeyModifiers::CONTROL);
        let mut app = app();
        assert!(!commands("", &app)
            .iter()
            .any(|(_, action)| *action == "request_log"));
        assert!(!help_text(&app, 82).contains("Ctrl+Shift+L"));
        assert!(!help_text(&app, 82).contains("Ctrl+P"));
        assert!(!help_text(&app, 82).contains("Ctrl+N"));
        assert!(!help_text(&app, 82).contains("Remotes"));
        assert!(help_text(&app, 82)
            .lines()
            .all(|line| line.chars().count() == 82));
        assert!(help_text(&app, 82)
            .lines()
            .any(|line| line.contains("Move down") && line.ends_with('j')));
        app.key(log_key);
        app.key(ctrl_l);
        assert!(matches!(app.mode, Mode::Normal));

        app.request_log = Some(RequestLog::default());
        assert!(commands("", &app)
            .iter()
            .any(|(_, action)| *action == "request_log"));
        assert!(help_text(&app, 82).contains("Ctrl+Shift+L"));
        app.key(ctrl_l);
        assert!(matches!(app.mode, Mode::RequestLog { offset: 0, .. }));
        let mut terminal = Terminal::new(TestBackend::new(120, 30)).unwrap();
        terminal.draw(|frame| draw(frame, &app)).unwrap();
        let popup = terminal
            .backend()
            .buffer()
            .content()
            .iter()
            .map(|cell| cell.symbol())
            .collect::<String>();
        assert!(popup.contains("API REQUEST / RESPONSE"));
        assert!(popup.contains("0 retained"));
        app.key(log_key);
        assert!(matches!(app.mode, Mode::Normal));

        app.mode = Mode::Compose {
            text: "draft".into(),
            parent: None,
            quote: None,
        };
        app.key(ctrl_l);
        assert!(matches!(app.mode, Mode::RequestLog { .. }));
        app.key(KeyEvent::new(KeyCode::Esc, KeyModifiers::NONE));
        assert!(matches!(app.mode, Mode::Compose { ref text, .. } if text == "draft"));

        app.mode = Mode::Palette {
            query: "request".into(),
            selected: 0,
        };
        app.key(KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE));
        assert!(matches!(app.mode, Mode::RequestLog { offset: 0, .. }));
        app.key(KeyEvent::new(KeyCode::Esc, KeyModifiers::NONE));
        assert!(matches!(app.mode, Mode::Palette { ref query, .. } if query == "request"));
    }

    #[test]
    fn nested_replies_insert_under_parent_and_collapse_only_subtree() {
        let (sender, jobs) = mpsc::channel();
        let (replies, receiver) = mpsc::channel();
        let mut app = App::new(sender, receiver, PathBuf::new());
        app.screen = Screen::Feed("latest");
        app.me = Some(User {
            username: "me".into(),
            ..Default::default()
        });
        app.entries = vec![entry("root"), entry("next")];
        app.toggle_replies();
        let token = match jobs.try_recv().unwrap() {
            Job::Replies(_, _, token) => token,
            _ => unreachable!(),
        };
        replies
            .send(Reply::Replies(
                "root".into(),
                token,
                Ok(Page {
                    entries: vec![reply_entry("child", "root"), reply_entry("sibling", "root")],
                    next: None,
                    profile: None,
                }),
            ))
            .unwrap();
        app.collect();
        app.selected = 1;
        app.toggle_replies();
        let token = match jobs.try_recv().unwrap() {
            Job::Replies(id, _, token) if id == "child" => token,
            _ => panic!("expected child branch"),
        };
        replies
            .send(Reply::Replies(
                "child".into(),
                token,
                Ok(Page {
                    entries: vec![reply_entry("grandchild", "child")],
                    next: None,
                    profile: None,
                }),
            ))
            .unwrap();
        app.collect();
        assert_eq!(
            app.entries
                .iter()
                .filter_map(entry_commit)
                .map(|c| c.id.as_str())
                .collect::<Vec<_>>(),
            ["root", "child", "grandchild", "sibling", "next"]
        );
        let trees = tree_positions(&app);
        assert_eq!(trees[2].parents.len(), 2);
        assert!(!trees[2].parents[0].0); // OP connects only to its first reply.
        assert!(trees[2].parents[1].0); // Its reply lane continues to the sibling.
        app.selected = 1;
        app.toggle_replies();
        assert_eq!(app.entries.len(), 4);
        assert!(app.expanded.contains_key("root"));
        assert!(!app.expanded.contains_key("child"));
        assert_eq!(
            app.entries
                .iter()
                .filter_map(entry_commit)
                .map(|c| c.id.as_str())
                .collect::<Vec<_>>(),
            ["root", "child", "sibling", "next"]
        );
    }

    #[test]
    fn own_profile_stays_put_until_g_shift_p_opens_selected_author() {
        let (sender, jobs) = mpsc::channel();
        let (replies, receiver) = mpsc::channel();
        let mut app = App::new(sender, receiver, PathBuf::new());
        app.screen = Screen::Feed("latest");
        app.me = Some(User {
            username: "me".into(),
            ..Default::default()
        });
        app.own_profile = Some(User {
            username: "me".into(),
            bio: Some("My bio".into()),
            ..Default::default()
        });
        app.entries = vec![entry("alice"), entry("bob")];
        app.collect();
        app.selected = 1;
        app.collect();
        assert!(jobs.try_recv().is_err());
        let mut terminal = Terminal::new(TestBackend::new(130, 30)).unwrap();
        terminal.draw(|frame| draw(frame, &app)).unwrap();
        let view = terminal
            .backend()
            .buffer()
            .content()
            .iter()
            .map(|cell| cell.symbol())
            .collect::<String>();
        assert!(view.contains("@me"));
        app.normal(KeyEvent::new(KeyCode::Char('g'), KeyModifiers::NONE));
        app.normal(KeyEvent::new(KeyCode::Char('P'), KeyModifiers::SHIFT));
        assert_eq!(app.screen, Screen::Profile("bob".into()));
        assert!(
            matches!(jobs.try_recv(), Ok(Job::Load(Screen::Profile(name), None)) if name == "bob")
        );
        replies
            .send(Reply::Loaded(
                Screen::Profile("bob".into()),
                false,
                Ok(Page {
                    entries: vec![],
                    next: None,
                    profile: Some(User {
                        username: "bob".into(),
                        is_followed: Some(true),
                        is_muting: Some(true),
                        ..Default::default()
                    }),
                }),
            ))
            .unwrap();
        app.collect();
        app.normal(KeyEvent::new(KeyCode::Char('f'), KeyModifiers::NONE));
        assert!(
            matches!(jobs.try_recv(), Ok(Job::Act(_, Action::Follow { username, remove: true })) if username == "bob")
        );
        app.busy = false;
        app.normal(KeyEvent::new(KeyCode::Char('m'), KeyModifiers::NONE));
        assert!(
            matches!(jobs.try_recv(), Ok(Job::Act(_, Action::Mute { username, remove: true })) if username == "bob")
        );
        app.normal(KeyEvent::new(KeyCode::Char('B'), KeyModifiers::NONE));
        assert!(
            matches!(app.mode, Mode::Block { ref username, remove: false } if username == "bob")
        );
    }

    #[test]
    fn post_lane_colors_and_guest_reply_guard() {
        assert_eq!(
            lane_color(Some(&User {
                lane: Some(Lane::Index(1)),
                ..Default::default()
            })),
            Color::Rgb(81, 207, 102)
        );
        assert_eq!(
            lane_color(Some(&User {
                lane: Some(Lane::Custom("#22aa77".into())),
                ..Default::default()
            })),
            Color::Rgb(34, 170, 119)
        );
        assert_eq!(lane_color(Some(&User::default())), primary_color());
        let user = User {
            username: "ada".into(),
            name: Some("Ada Lovelace".into()),
            ..Default::default()
        };
        assert_eq!(user_avatar(Some(&user))[0].content.as_ref(), "AD");
        let mut app = app();
        app.entries = vec![entry("a")];
        app.normal(KeyEvent::new(KeyCode::Char(' '), KeyModifiers::NONE));
        assert!(app.status.contains("Sign in"));
        assert!(app.expanded.is_empty());
        let mut terminal = Terminal::new(TestBackend::new(130, 24)).unwrap();
        terminal.draw(|frame| draw(frame, &app)).unwrap();
        assert!(terminal
            .backend()
            .buffer()
            .content()
            .iter()
            .any(|cell| cell.symbol() == "A" && cell.fg == Color::Rgb(81, 207, 102)));
    }

    #[test]
    fn handles_and_nested_quoted_commits_keep_their_own_lane_colors() {
        let mut app = app();
        app.screen = Screen::Feed("latest");
        app.entries = vec![serde_json::from_value(serde_json::json!({
            "id":"quoted", "author":{"username":"quoted","lane":1}
        }))
        .map(|commit| Entry::Commit(Box::new(commit)))
        .unwrap(), serde_json::from_value(serde_json::json!({
            "id":"reply", "body":"A reply", "quoted_commit_id":"quoted",
            "quoted_commit":{"id":"quoted","body":"Quoted text","author":{"username":"quoter","lane":4}},
            "author":{"username":"speaker","lane":2}
        }))
        .map(|commit| Entry::Commit(Box::new(commit)))
        .unwrap()];
        let positions = tree_positions(&app);
        assert!(positions[1].parents.is_empty());
        let quote = match &app.entries[1] {
            Entry::Commit(commit) => post_info(commit, 80),
            _ => unreachable!(),
        };
        assert!(quote.iter().any(|line| line.spans.iter().any(|span| {
            span.content.contains("@quoter") && span.style.fg == Some(lane_color_index(4))
        })));
        assert!(quote.iter().any(|line| line
            .spans
            .iter()
            .any(|span| span.content.contains("Quoted text"))));

        let mut terminal = Terminal::new(TestBackend::new(100, 24)).unwrap();
        terminal.draw(|frame| draw(frame, &app)).unwrap();
        assert!(terminal
            .backend()
            .buffer()
            .content()
            .iter()
            .any(|cell| cell.symbol() == "@" && cell.fg == lane_color_index(2)));
    }

    #[test]
    fn post_metadata_and_relative_time_use_api_fields() {
        let stamp =
            (OffsetDateTime::now_utc() - time::Duration::minutes(5) - time::Duration::seconds(15))
                .format(&Rfc3339)
                .unwrap();
        assert_eq!(relative_time(Some(&stamp)), "5m");
        assert_eq!(relative_time(Some("not-a-date")), "time unknown");
        assert_eq!(relative_time(Some("2999-01-01T00:00:00Z")), "now");
        let commit: Commit = serde_json::from_value(serde_json::json!({
            "id":"123456789abcdef", "short_hash":"abcdef", "created_at":stamp,
            "body":"Root body", "parent_id":"parent-1234567",
            "parent":{"id":"parent-1234567","short_hash":"1234567","body":"Parent context","author":{"username":"parent"}},
            "quoted_commit_id":"quote-1234567",
            "quoted_commit":{"id":"quote-1234567","short_hash":"1234567","body":"Quoted context","author":{"username":"quoter"}},
            "link_preview":{"url":"https://example.com/article","title":"Article title","description":"Summary","image_url":"https://example.com/card.jpg","site_name":"Example","is_pending":false},
            "images":[{"url":"https://example.com/photo.jpg","description":"A photo","width":640,"height":480}],
            "poll":{"is_closed":false,"has_voted":true,"total_votes":8,"options":[{"label":"Yes","votes_count":6,"percentage":75,"voted":true}]},
            "pasteis_count":3,"status":"approved","author":{"username":"alice"}
        })).unwrap();
        let rendered = post_info(&commit, 80)
            .iter()
            .map(|line| {
                line.spans
                    .iter()
                    .map(|span| span.content.as_ref())
                    .collect::<String>()
            })
            .collect::<Vec<_>>()
            .join("\n");
        for detail in [
            "@parent",
            "Parent context",
            "@quoter",
            "Quoted context",
            "Article title",
            "Summary",
            "A photo",
            "photo.jpg",
            "75%",
            "Yes",
        ] {
            assert!(rendered.contains(detail), "missing {detail}: {rendered}");
        }
        assert_eq!(short_id(&commit), "abcdef");
        assert_eq!(commit.pasteis_count, Some(3));
        assert!(!rendered.contains("Status:"));
        assert!(!rendered.contains("card.jpg"));
        let orphan: Commit =
            serde_json::from_value(serde_json::json!({"id":"reply", "parent_id":"parent-1234567"}))
                .unwrap();
        let orphan_info = post_info(&orphan, 80)
            .iter()
            .flat_map(|line| line.spans.iter().map(|span| span.content.as_ref()))
            .collect::<String>();
        assert!(orphan_info.contains("#1234567"));
        assert!(!orphan_info.contains("#unknown"));
        let mut reply_app = app();
        reply_app.entries = vec![Entry::Commit(Box::new(orphan))];
        let mut reply_terminal = Terminal::new(TestBackend::new(100, 24)).unwrap();
        reply_terminal
            .draw(|frame| draw(frame, &reply_app))
            .unwrap();
        let reply_view = reply_terminal
            .backend()
            .buffer()
            .content()
            .iter()
            .map(|cell| cell.symbol())
            .collect::<String>();
        assert!(
            reply_view.contains("REPLY")
                && reply_view.contains("#1234567")
                && reply_view.contains("depth 1")
        );
        let mut app = app();
        app.screen = Screen::Feed("latest");
        app.entries.push(Entry::Commit(Box::new(commit)));
        let mut terminal = Terminal::new(TestBackend::new(130, 28)).unwrap();
        terminal.draw(|frame| draw(frame, &app)).unwrap();
        let visible = terminal
            .backend()
            .buffer()
            .content()
            .iter()
            .map(|cell| cell.symbol())
            .collect::<String>();
        assert!(
            visible.contains("#abcdef") && visible.contains("5m"),
            "{visible}"
        );
        assert!(visible.contains("AL") && visible.contains("photo.jpg"));
        assert!(
            !visible.contains("▧ image ·")
                && !visible.contains("Status:")
                && !visible.contains('▀')
        );
    }

    #[test]
    fn toml_keymaps_override_defaults_and_reject_conflicts() {
        let dir = std::env::temp_dir().join(format!("dev-null-keymap-{}", rand::random::<u64>()));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("config.toml");
        std::fs::write(
            &path,
            r#"[keymap]
reply = "R"
refresh = "r"
expand_replies = "Space"
go_latest = "g f"
search_ctrl = "Alt+K"
"#,
        )
        .unwrap();
        let (map, error) = Keymap::load(&path);
        assert!(error.is_none());
        assert_eq!(
            map.normalize(KeyEvent::new(KeyCode::Char('R'), KeyModifiers::NONE))
                .unwrap()
                .code,
            KeyCode::Char('r')
        );
        assert_eq!(
            map.normalize(KeyEvent::new(KeyCode::Char('r'), KeyModifiers::NONE))
                .unwrap()
                .code,
            KeyCode::Char('R')
        );
        assert_eq!(map.label("expand_replies"), "Space");
        assert_eq!(
            Keymap::defaults()
                .iter()
                .find(|(name, _)| *name == "quote")
                .map(|(_, key)| key.code),
            Some(KeyCode::Char('Q'))
        );
        assert_eq!(
            Keymap::defaults()
                .iter()
                .find(|(name, _)| *name == "copy_link")
                .map(|(_, key)| key.code),
            Some(KeyCode::Char('y'))
        );
        assert_eq!(
            key_label(parse_key("Cmd+Shift+P").unwrap()),
            if cfg!(target_os = "macos") {
                "⌘⇧P"
            } else {
                "Super+⇧P"
            }
        );
        assert_eq!(map.label("go_latest"), "g f");
        assert_eq!(
            map.chord_action(
                KeyEvent::new(KeyCode::Char('g'), KeyModifiers::NONE),
                KeyEvent::new(KeyCode::Char('f'), KeyModifiers::NONE)
            ),
            Some("go_latest")
        );
        let (sender, _) = mpsc::channel();
        let (_, receiver) = mpsc::channel();
        let mut app = App::new(sender, receiver, PathBuf::new());
        app.keymap = map;
        app.me = Some(User {
            username: "me".into(),
            ..Default::default()
        });
        app.entries = vec![entry("parent")];
        app.normal(KeyEvent::new(KeyCode::Char('R'), KeyModifiers::NONE));
        assert!(
            matches!(app.mode, Mode::Compose { ref parent, .. } if parent.as_deref() == Some("parent"))
        );
        app.mode = Mode::Normal;
        app.normal(KeyEvent::new(KeyCode::Char('k'), KeyModifiers::ALT));
        assert!(matches!(app.mode, Mode::WebSearch(_)));
        std::fs::write(
            &path,
            r#"[keymap]
reply = "R"
"#,
        )
        .unwrap();
        let (fallback, error) = Keymap::load(&path);
        assert!(error.unwrap().contains("conflicts"));
        assert_eq!(fallback.label("reply"), "r");
        std::fs::write(
            &path,
            r#"[keymap]
reply = "not-a-key"
"#,
        )
        .unwrap();
        assert!(Keymap::load(&path).1.unwrap().contains("invalid key"));
        std::fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn o_opens_web_and_space_toggles_replies() {
        let (sender, jobs) = mpsc::channel();
        let (_, receiver) = mpsc::channel();
        let mut app = App::new(sender, receiver, PathBuf::new());
        app.me = Some(User {
            username: "me".into(),
            ..Default::default()
        });
        app.entries = vec![entry("post")];
        assert_eq!(app.keymap.label("open_web"), "o");
        assert_eq!(app.keymap.label("expand_replies"), "Space");
        app.key(KeyEvent::new(KeyCode::Char('o'), KeyModifiers::NONE));
        assert!(app.expanded.is_empty());
        assert!(jobs.try_recv().is_err());
        app.key(KeyEvent::new(KeyCode::Char(' '), KeyModifiers::NONE));
        assert!(matches!(jobs.try_recv(), Ok(Job::Replies(id, None, _)) if id == "post"));
    }

    #[test]
    fn official_shortcuts_are_global_and_skip_text_input() {
        let (sender, jobs) = mpsc::channel();
        let (_, receiver) = mpsc::channel();
        let mut app = App::new(sender, receiver, PathBuf::new());
        app.screen = Screen::Feed("latest");
        app.me = Some(User {
            username: "me".into(),
            ..Default::default()
        });
        app.entries = vec![entry("first"), entry("second")];
        app.key(KeyEvent::new(KeyCode::Char('1'), KeyModifiers::NONE));
        assert_eq!(app.screen, Screen::Feed("latest"));
        app.key(KeyEvent::new(KeyCode::Char('2'), KeyModifiers::NONE));
        assert_eq!(app.screen, Screen::Feed("trending"));
        app.key(KeyEvent::new(KeyCode::Char('3'), KeyModifiers::NONE));
        assert_eq!(app.screen, Screen::Feed("following"));
        while jobs.try_recv().is_ok() {}
        app.entries = vec![entry("first"), entry("second")];
        app.focus = Pane::Nav;
        app.key(KeyEvent::new(KeyCode::Char('j'), KeyModifiers::NONE));
        assert_eq!(app.selected, 1);
        assert_eq!(app.focus, Pane::Nav);
        app.focus = Pane::Feed;
        app.key(KeyEvent::new(KeyCode::Char('k'), KeyModifiers::NONE));
        assert_eq!(app.selected, 0);
        app.key(KeyEvent::new(KeyCode::Char('Q'), KeyModifiers::SHIFT));
        assert!(
            matches!(app.mode, Mode::Compose { ref quote, .. } if quote.as_deref() == Some("first"))
        );
        app.key(KeyEvent::new(KeyCode::Char('q'), KeyModifiers::NONE));
        assert!(!app.quit); // Draft text can contain q.
        app.key(KeyEvent::new(KeyCode::Esc, KeyModifiers::NONE));
        app.key(KeyEvent::new(KeyCode::Char(' '), KeyModifiers::NONE));
        assert!(matches!(jobs.try_recv(), Ok(Job::Replies(id, None, _)) if id == "first"));
        app.key(KeyEvent::new(KeyCode::Char(' '), KeyModifiers::NONE));
        assert!(app.expanded.is_empty());
        app.key(KeyEvent::new(KeyCode::Char('g'), KeyModifiers::NONE));
        app.key(KeyEvent::new(KeyCode::Char('l'), KeyModifiers::NONE));
        assert_eq!(app.screen, Screen::Feed("latest"));
        app.key(KeyEvent::new(KeyCode::Char('g'), KeyModifiers::NONE));
        app.key(KeyEvent::new(KeyCode::Char('e'), KeyModifiers::NONE));
        assert_eq!(app.screen, Screen::Feed("latest"));
        app.key(KeyEvent::new(KeyCode::Char('q'), KeyModifiers::NONE));
        assert!(app.quit);
        app.key(KeyEvent::new(KeyCode::Char('g'), KeyModifiers::NONE));
        app.key(KeyEvent::new(KeyCode::Char('p'), KeyModifiers::NONE));
        assert_eq!(app.screen, Screen::Profile("me".into()));
        app.key(KeyEvent::new(KeyCode::Char('g'), KeyModifiers::NONE));
        app.key(KeyEvent::new(KeyCode::Char('s'), KeyModifiers::NONE));
        assert_eq!(app.screen, Screen::Starred);
        app.mode = Mode::Normal;
        app.key(KeyEvent::new(KeyCode::Char('s'), KeyModifiers::ALT));
        assert!(matches!(app.mode, Mode::Settings { .. }));
        app.mode = Mode::Normal;
        app.key(KeyEvent::new(KeyCode::Char('k'), KeyModifiers::SUPER));
        assert!(matches!(app.mode, Mode::Normal));
        app.key(KeyEvent::new(KeyCode::Char('k'), KeyModifiers::CONTROL));
        assert!(matches!(app.mode, Mode::WebSearch(_)));
        assert!(official_search_url("dev null & alice")
            .unwrap()
            .contains("q=dev+null+%26+alice"));
        app.key(KeyEvent::new(KeyCode::Esc, KeyModifiers::NONE));
        app.key(KeyEvent::new(KeyCode::Char('/'), KeyModifiers::NONE));
        assert!(matches!(app.mode, Mode::WebSearch(_)));
        app.key(KeyEvent::new(KeyCode::Esc, KeyModifiers::NONE));
        app.mode = Mode::Compose {
            text: String::new(),
            parent: None,
            quote: None,
        };
        app.key(KeyEvent::new(KeyCode::Char('j'), KeyModifiers::NONE));
        assert!(matches!(app.mode, Mode::Compose { ref text, .. } if text == "j"));
        app.key(KeyEvent::new(
            KeyCode::Char('P'),
            KeyModifiers::SUPER | KeyModifiers::SHIFT,
        ));
        assert!(matches!(app.mode, Mode::ComposePreview { ref text, .. } if text == "j"));
        app.key(KeyEvent::new(KeyCode::Esc, KeyModifiers::NONE));
        assert!(matches!(app.mode, Mode::Compose { ref text, .. } if text == "j"));
        app.busy = false;
        app.key(KeyEvent::new(KeyCode::Enter, KeyModifiers::SUPER));
        assert!(jobs
            .try_iter()
            .any(|job| matches!(job, Job::Act(_, Action::Create { body, .. }) if body == "j")));
    }

    #[test]
    fn star_and_unstar_are_explicit_and_repeated_star_is_a_noop() {
        let (sender, jobs) = mpsc::channel();
        let (_, receiver) = mpsc::channel();
        let mut app = App::new(sender, receiver, PathBuf::new());
        app.me = Some(User {
            username: "me".into(),
            ..Default::default()
        });
        app.entries = vec![serde_json::from_value(serde_json::json!({
            "id":"commit", "body":"test", "starred":false,
            "author":{"username":"alice"}
        }))
        .map(|commit| Entry::Commit(Box::new(commit)))
        .unwrap()];
        app.normal(KeyEvent::new(KeyCode::Char('s'), KeyModifiers::NONE));
        assert!(
            matches!(jobs.try_recv(), Ok(Job::Act(_, Action::Star { ref id, remove: false })) if id == "commit")
        );
        app.busy = false;
        if let Entry::Commit(commit) = &mut app.entries[0] {
            commit.starred = Some(true);
        }
        app.normal(KeyEvent::new(KeyCode::Char('s'), KeyModifiers::NONE));
        assert!(jobs.try_recv().is_err());
        app.normal(KeyEvent::new(KeyCode::Char('S'), KeyModifiers::SHIFT));
        assert!(
            matches!(jobs.try_recv(), Ok(Job::Act(_, Action::Star { ref id, remove: true })) if id == "commit")
        );
        app.busy = false;
        if let Entry::Commit(commit) = &mut app.entries[0] {
            commit.starred = Some(false);
        }
        app.normal(KeyEvent::new(KeyCode::Char('S'), KeyModifiers::SHIFT));
        assert!(jobs.try_recv().is_err());
    }

    #[test]
    fn copy_shortcut_reports_missing_focused_commit() {
        let mut app = app();
        app.normal(KeyEvent::new(KeyCode::Char('y'), KeyModifiers::NONE));
        assert!(app.status.contains("No commit link selected"));
    }

    #[test]
    fn settings_use_authenticated_lane_and_send_supported_changes() {
        let (sender, jobs) = mpsc::channel();
        let (_, replies) = mpsc::channel();
        let mut app = App::new(sender, replies, PathBuf::new());
        app.me = Some(User {
            username: "me".into(),
            lane: Some(Lane::Custom("#123abc".into())),
            ..Default::default()
        });
        assert_eq!(app.accent(), Color::Rgb(18, 58, 188));
        app.normal(KeyEvent::new(KeyCode::Char('s'), KeyModifiers::ALT));
        assert!(matches!(app.mode, Mode::Settings { selected: 0 }));
        app.key(KeyEvent::new(KeyCode::Down, KeyModifiers::NONE));
        app.key(KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE));
        assert!(matches!(
            jobs.try_recv(),
            Ok(Job::Act(
                _,
                Action::SetLane {
                    lane: Some(0),
                    color: None
                }
            ))
        ));
        let mut terminal = Terminal::new(TestBackend::new(130, 28)).unwrap();
        terminal.draw(|frame| draw(frame, &app)).unwrap();
        assert!(terminal
            .backend()
            .buffer()
            .content()
            .iter()
            .any(|cell| cell.symbol() == "/" && cell.fg == Color::White));
        assert!(terminal
            .backend()
            .buffer()
            .content()
            .iter()
            .any(|cell| cell.fg == brand_color()));
        assert!(terminal_background(terminal.backend().buffer(), 130));
        app.busy = false;
        app.mode = Mode::Settings { selected: 7 };
        app.key(KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE));
        assert!(matches!(
            jobs.try_recv(),
            Ok(Job::Act(_, Action::SetPasteisMode { enabled: true }))
        ));
        app.busy = false;
        app.mode = Mode::Settings { selected: 8 };
        app.key(KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE));
        assert!(matches!(
            jobs.try_recv(),
            Ok(Job::Act(_, Action::SyncGithub))
        ));
        app.busy = false;
        app.mode = Mode::CustomColor(String::new());
        app.paste("#FF8800".into());
        app.key(KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE));
        assert!(
            matches!(jobs.try_recv(), Ok(Job::Act(_, Action::SetLane { lane: None, color: Some(hex) })) if hex == "#ff8800")
        );
    }

    #[test]
    fn control_c_preserves_draft_and_control_n_is_not_a_newline_binding() {
        let mut app = app();
        app.mode = Mode::Compose {
            text: "draft".into(),
            parent: None,
            quote: None,
        };
        app.key(KeyEvent::new(KeyCode::Char('c'), KeyModifiers::CONTROL));
        assert!(!app.quit);
        app.key(KeyEvent::new(KeyCode::Char('n'), KeyModifiers::CONTROL));
        assert!(matches!(app.mode, Mode::Compose { ref text, .. } if text == "draft"));
        app.key(KeyEvent::new(KeyCode::Enter, KeyModifiers::ALT));
        assert!(matches!(app.mode, Mode::Compose { ref text, .. } if text == "draft\n"));
    }

    #[test]
    fn vim_motions_respect_pane_and_list_bounds() {
        let mut app = app();
        app.entries = (0..15)
            .map(|n| {
                Entry::User(Box::new(User {
                    username: format!("user{n}"),
                    ..Default::default()
                }))
            })
            .collect();
        app.selected = 9;
        app.normal(KeyEvent::new(KeyCode::Char('g'), KeyModifiers::NONE));
        assert_eq!(app.selected, 9);
        app.normal(KeyEvent::new(KeyCode::Char('g'), KeyModifiers::NONE));
        assert_eq!(app.selected, 0);
        app.normal(KeyEvent::new(KeyCode::Char('G'), KeyModifiers::NONE));
        assert_eq!(app.selected, 14);
        app.normal(KeyEvent::new(KeyCode::Char('h'), KeyModifiers::NONE));
        assert_eq!(app.focus, Pane::Nav);
        app.normal(KeyEvent::new(KeyCode::Char('j'), KeyModifiers::NONE));
        assert_eq!(app.sidebar_selected, 0);
        assert_eq!(app.selected, 14);
        app.normal(KeyEvent::new(KeyCode::Tab, KeyModifiers::NONE));
        assert_eq!(app.focus, Pane::Feed);
        app.normal(KeyEvent::new(KeyCode::Tab, KeyModifiers::NONE));
        assert_eq!(app.focus, Pane::Nav);
        app.normal(KeyEvent::new(KeyCode::Char('l'), KeyModifiers::NONE));
        assert_eq!(app.focus, Pane::Feed);
    }

    #[test]
    fn narrow_view_uses_l_to_open_and_h_to_go_back() {
        let mut app = app();
        app.width = 60;
        app.entries = vec![Entry::User(Box::new(User {
            username: "alice".into(),
            ..Default::default()
        }))];
        app.normal(KeyEvent::new(KeyCode::Char('l'), KeyModifiers::NONE));
        assert_eq!(app.screen, Screen::Profile("alice".into()));
        app.normal(KeyEvent::new(KeyCode::Char('h'), KeyModifiers::NONE));
        assert_eq!(app.screen, Screen::Feed("latest"));
    }

    #[test]
    fn private_screen_clears_stale_public_content_for_guests() {
        let mut app = app();
        app.entries = vec![Entry::User(Box::new(User {
            username: "someone".into(),
            ..Default::default()
        }))];
        app.busy = true;
        app.screen = Screen::Notifications;
        app.load(None);
        assert!(app.entries.is_empty());
        assert!(!app.busy);
        assert!(app.status.contains("Sign in"));
    }

    #[test]
    fn palette_filters_and_dispatches() {
        let mut app = app();
        app.key(KeyEvent::new(KeyCode::Char(':'), KeyModifiers::NONE));
        app.paste("trending".into());
        assert!(matches!(&app.mode, Mode::Palette { query, .. } if query == "trending"));
        app.key(KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE));
        assert_eq!(app.screen, Screen::Feed("trending"));
    }

    #[test]
    fn inherits_terminal_background_at_all_sizes() {
        for width in [52, 75, 95, 100, 130, 160] {
            let mut app = app();
            app.width = width;
            app.height = 28;
            app.screen = Screen::Feed("latest");
            app.me = Some(User {
                username: "you".into(),
                ..Default::default()
            });
            let commit = Commit {
                id: "1".into(),
                short_hash: Some("a81c7e2".into()),
                body: Some(
                    "Shipping a tiny fix today: less noise, better threads and useful shortcuts."
                        .into(),
                ),
                status: None,
                parent_id: None,
                parent: None,
                quoted_commit_id: None,
                quoted_commit: None,
                link_preview: None,
                images: None,
                poll: None,
                pasteis_count: None,
                author: Some(User {
                    username: "nunomaduro".into(),
                    ..Default::default()
                }),
                created_at: Some("2026-10-20T13:00:00Z".into()),
                stars_count: Some(18),
                replies_count: Some(6),
                starred: Some(false),
                html_url: None,
            };
            app.entries = vec![
                Entry::Commit(Box::new(commit.clone())),
                Entry::Commit(Box::new(Commit {
                    id: "2".into(),
                    body: Some("The best tools disappear into the workflow.".into()),
                    author: Some(User {
                        username: "someone".into(),
                        ..Default::default()
                    }),
                    ..commit
                })),
            ];
            let mut terminal = Terminal::new(TestBackend::new(width, 28)).unwrap();
            terminal.draw(|frame| draw(frame, &app)).unwrap();
            let buffer = terminal.backend().buffer();
            assert!(terminal_background(buffer, width));
            let visible = buffer
                .content()
                .iter()
                .map(|cell| cell.symbol())
                .collect::<String>();
            if width >= 76 {
                assert!(visible.contains("/dev/null"));
            }
            assert!(visible.contains("NORMAL"));
            assert!(visible.contains("@nunomaduro"));
            if width == 130 && std::env::var_os("DEV_NULL_PREVIEW").is_some() {
                for row in buffer.content().chunks(width as usize) {
                    println!(
                        "{}",
                        row.iter()
                            .map(|cell| cell.symbol())
                            .collect::<String>()
                            .trim_end()
                    );
                }
            }
        }
    }

    #[test]
    fn pulse_uses_unread_deltas_and_new_badges_without_marking_anything_read() {
        let (sender, jobs) = mpsc::channel();
        let (_, receiver) = mpsc::channel();
        let mut app = App::new(sender, receiver, PathBuf::new());
        app.me = Some(User {
            username: "me".into(),
            ..Default::default()
        });
        app.screen = Screen::Feed("latest");
        app.entries = vec![entry("anchor")];
        let old: User = serde_json::from_value(serde_json::json!({"username":"me", "achievements": {
            "level":1,"rank":"New","xp":10,"level_progress":10,
            "badges":[{"id":"first","name":"First","description":"Do something","unlocked":false,"progress":0,"target":1}]
        }})).unwrap();
        let unlocked: User = serde_json::from_value(serde_json::json!({"username":"me", "achievements": {
            "level":2,"rank":"Builder","xp":100,"level_progress":20,
            "badges":[{"id":"first","name":"First","description":"Do something","unlocked":true,"progress":1,"target":1}]
        }})).unwrap();
        app.own_profile = Some(old);
        app.unread = Some(1);
        app.poll_at -= Duration::from_secs(61);
        app.collect();
        assert!(
            matches!(jobs.try_recv(), Ok(Job::Pulse(name, Some(("latest", id)))) if name == "me" && id == "anchor")
        );
        assert!(app.poll_pending);
        app.apply_pulse(
            Some(("latest", "anchor".into())),
            Pulse {
                unread: 3,
                profile: Some(unlocked.clone()),
                new_commits: Some(4),
            },
        );
        assert_eq!(app.unread, Some(3));
        assert_eq!(app.new_commits, 4);
        assert!(app.toast.as_ref().is_some_and(
            |(text, _)| text.contains("2 new notifications") && text.contains("First")
        ));
        let mut terminal = Terminal::new(TestBackend::new(130, 30)).unwrap();
        terminal.draw(|frame| draw(frame, &app)).unwrap();
        let screen = terminal
            .backend()
            .buffer()
            .content()
            .iter()
            .map(|cell| cell.symbol())
            .collect::<String>();
        assert!(screen.contains("2 new notifications"));
        let visible = terminal
            .backend()
            .buffer()
            .content()
            .iter()
            .map(|cell| cell.symbol())
            .collect::<String>();
        assert!(visible.contains("First"));
        app.toast = None;
        app.apply_pulse(
            Some(("latest", "old-anchor".into())),
            Pulse {
                unread: 3,
                profile: Some(unlocked),
                new_commits: Some(99),
            },
        );
        assert!(app.toast.is_none());
        assert_eq!(app.new_commits, 4);
    }

    #[test]
    fn colon_searches_available_commands_while_question_mark_only_shows_keys() {
        let mut app = app();
        app.normal(KeyEvent::new(KeyCode::Char(':'), KeyModifiers::SHIFT));
        assert!(matches!(app.mode, Mode::Palette { .. }));
        assert!(commands("notifications", &app).is_empty());
        app.key(KeyEvent::new(KeyCode::Esc, KeyModifiers::NONE));
        app.normal(KeyEvent::new(KeyCode::Char('?'), KeyModifiers::SHIFT));
        assert!(matches!(app.mode, Mode::Help { offset: 0 }));
        assert!(help_text(&app, 82).contains("Selected author"));
        assert!(!help_text(&app, 82).contains("CONFIG"));
        app.me = Some(User {
            username: "me".into(),
            ..Default::default()
        });
        assert_eq!(commands("notifications", &app).len(), 1);
    }

    #[test]
    fn action_palette_uses_terminal_theme_and_shows_shortcuts() {
        let mut app = app();
        app.me = Some(User {
            username: "me".into(),
            ..Default::default()
        });
        app.mode = Mode::Palette {
            query: "notifications".into(),
            selected: 0,
        };
        let mut terminal = Terminal::new(TestBackend::new(90, 24)).unwrap();
        terminal.draw(|frame| draw(frame, &app)).unwrap();
        let buffer = terminal.backend().buffer();
        let visible = buffer
            .content()
            .iter()
            .map(|cell| cell.symbol())
            .collect::<String>();
        assert!(visible.contains("Notifications"));
        assert!(visible.contains("Enter run"));
        assert!(terminal_background(buffer, 90));
    }

    #[test]
    fn g_p_opens_selected_author_and_g_p_opens_my_profile() {
        let (sender, jobs) = mpsc::channel();
        let (_, receiver) = mpsc::channel();
        let mut app = App::new(sender, receiver, PathBuf::new());
        app.me = Some(User {
            username: "me".into(),
            ..Default::default()
        });
        app.entries = vec![entry("alice")];
        app.normal(KeyEvent::new(KeyCode::Char('g'), KeyModifiers::NONE));
        app.normal(KeyEvent::new(KeyCode::Char('P'), KeyModifiers::SHIFT));
        assert_eq!(app.screen, Screen::Profile("alice".into()));
        assert!(
            matches!(jobs.try_recv(), Ok(Job::Load(Screen::Profile(name), None)) if name == "alice")
        );
        app.go(Screen::Feed("latest"));
        let _ = jobs.try_recv();
        app.normal(KeyEvent::new(KeyCode::Char('g'), KeyModifiers::NONE));
        app.normal(KeyEvent::new(KeyCode::Char('p'), KeyModifiers::NONE));
        assert_eq!(app.screen, Screen::Profile("me".into()));
    }

    #[test]
    fn own_profile_panel_and_profile_tabs_are_explicit() {
        let mut app = app();
        app.me = Some(User {
            username: "me".into(),
            ..Default::default()
        });
        app.own_profile = Some(User {
            username: "me".into(),
            bio: Some("Builds tools".into()),
            ..Default::default()
        });
        app.screen = Screen::Feed("latest");
        app.entries = vec![entry("alice")];
        let show = |app: &App| {
            let mut terminal = Terminal::new(TestBackend::new(app.width, 36)).unwrap();
            terminal.draw(|frame| draw(frame, app)).unwrap();
            terminal
                .backend()
                .buffer()
                .content()
                .iter()
                .map(|cell| cell.symbol())
                .collect::<String>()
        };
        assert!(show(&app).contains("/dev/null"));
        assert!(show(&app).contains("@me"));
        app.focus = Pane::Nav;
        app.sidebar_selected = 2;
        app.normal(KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE));
        assert_eq!(app.screen, Screen::Profile("me".into()));
        app.profile = Some(
            serde_json::from_value(serde_json::json!({
                "username":"me", "bio":"Builds tools", "company":"Acme", "location":"Earth",
                "streaks":{"current":2,"longest":9},
                "achievements":{"level":3,"rank":"Builder","xp":200,"level_progress":50,"badges":[]}
            }))
            .unwrap(),
        );
        assert!(show(&app).contains("Builds tools"));
        app.width = 90;
        assert!(show(&app).contains("Streak: 2 current"));
        assert!(show(&app).contains("Level 3"));
        app.normal(KeyEvent::new(KeyCode::Char('e'), KeyModifiers::NONE));
        assert_eq!(app.screen, Screen::ProfileReplies("me".into()));
        app.normal(KeyEvent::new(KeyCode::Char('a'), KeyModifiers::NONE));
        assert_eq!(app.screen, Screen::Profile("me".into()));
    }

    #[test]
    fn blocked_list_allows_unblock_without_loading_hidden_profile() {
        let (sender, jobs) = mpsc::channel();
        let (_, receiver) = mpsc::channel();
        let mut app = App::new(sender, receiver, PathBuf::new());
        app.screen = Screen::Blocked;
        app.me = Some(User {
            username: "me".into(),
            ..Default::default()
        });
        app.entries = vec![Entry::User(Box::new(User {
            username: "blocked".into(),
            ..Default::default()
        }))];
        app.collect();
        app.collect();
        assert!(jobs.try_recv().is_err());
        app.normal(KeyEvent::new(KeyCode::Char('B'), KeyModifiers::NONE));
        assert!(
            matches!(app.mode, Mode::Block { ref username, remove: true } if username == "blocked")
        );
    }

    #[test]
    fn web_palette_reads_light_and_dark_osc_backgrounds() {
        assert_eq!(
            parse_terminal_background(b"\x1b]11;rgb:ffff/ffff/ffff\x1b\\"),
            Some(true)
        );
        assert_eq!(
            parse_terminal_background(b"\x1b]11;rgb:1111/2222/3333\x07"),
            Some(false)
        );
        assert_eq!(parse_terminal_background(b"\x1b[?1;2c"), None);
        assert_eq!(
            lane_color(Some(&User {
                lane: Some(Lane::Index(0)),
                ..Default::default()
            })),
            web_color(0xff8a4c, 0xd9480f)
        );
    }

    #[test]
    fn log_numbers_are_scoped_and_g_chords_navigate() {
        let mut app = app();
        app.screen = Screen::Profile("alice".into());
        app.key(KeyEvent::new(KeyCode::Char('2'), KeyModifiers::NONE));
        assert_eq!(app.screen, Screen::Profile("alice".into()));
        app.screen = Screen::Feed("latest");
        app.key(KeyEvent::new(KeyCode::Char('2'), KeyModifiers::NONE));
        assert_eq!(app.screen, Screen::Feed("trending"));
        app.key(KeyEvent::new(KeyCode::Char('3'), KeyModifiers::NONE));
        assert_eq!(app.screen, Screen::Feed("following"));
        app.key(KeyEvent::new(KeyCode::Char('g'), KeyModifiers::NONE));
        app.key(KeyEvent::new(KeyCode::Char('l'), KeyModifiers::NONE));
        assert_eq!(app.screen, Screen::Feed("latest"));
        app.key(KeyEvent::new(KeyCode::Char('g'), KeyModifiers::NONE));
        app.key(KeyEvent::new(KeyCode::Char('s'), KeyModifiers::NONE));
        assert_eq!(app.screen, Screen::Starred);
    }

    #[test]
    fn wide_feed_uses_20_60_20_layout() {
        let columns = wide_columns(Rect::new(0, 0, 200, 40));
        assert_eq!(
            (columns[0].width, columns[1].width, columns[2].width),
            (40, 120, 40)
        );
        let columns = wide_columns(Rect::new(0, 0, 130, 40));
        assert_eq!(
            (columns[0].width, columns[1].width, columns[2].width),
            (26, 78, 26)
        );
    }

    #[test]
    fn visible_https_links_get_mouse_click_regions() {
        let mut app = app();
        app.entries = vec![serde_json::from_value(serde_json::json!({
            "id":"link","body":"See https://github.com/acme/widget for details",
            "images":[{"url":"https://example.com/photo.jpg","description":"A photo"}],
            "author":{"username":"alice"}
        }))
        .map(|commit| Entry::Commit(Box::new(commit)))
        .unwrap()];
        app.width = 100;
        let mut terminal = Terminal::new(TestBackend::new(100, 24)).unwrap();
        terminal.draw(|frame| draw(frame, &app)).unwrap();
        assert!(app
            .link_regions
            .borrow()
            .iter()
            .any(|region| region.url == "https://github.com/acme/widget" && region.area.width > 0));
        assert!(app
            .link_regions
            .borrow()
            .iter()
            .any(|region| region.url == "https://example.com/photo.jpg" && region.area.width > 0));
        assert!(valid_click_url("https://github.com/acme/widget"));
        assert!(!valid_click_url("file:///etc/passwd"));
        assert!(!valid_click_url("https://user:pass@example.com/"));
    }

    #[test]
    fn code_fences_and_site_links_render_inline_without_scraping() {
        let body = "Try /dev/null https://dev-null.sh/@alice/commits/abc123 and https://github.com/acme/widget.\n```rust\n  fn main() {\n    println!(\"hello\");\n  }\n```";
        let body_lines = body_lines(body, 90);
        let lines = body_lines
            .iter()
            .map(|line| {
                line.spans
                    .iter()
                    .map(|span| span.content.as_ref())
                    .collect::<String>()
            })
            .collect::<Vec<_>>();
        assert!(lines
            .iter()
            .any(|line| line.contains("//alice/abc123") && line.contains("🐙 acme/widget")));
        assert!(lines.iter().any(|line| line.contains("// /dev/null")));
        assert!(!lines.iter().any(|line| line.contains("CODE · rust")));
        assert!(lines.iter().any(|line| line.contains("  fn main() {")));
        assert!(body_lines
            .iter()
            .all(|line| line.spans.iter().all(|span| span.style.fg.is_none())));
        assert_eq!(link_label("https://dev-null.sh.evil.test/@alice"), None);
        let commit: Commit = serde_json::from_value(serde_json::json!({
            "id":"a", "body":"hello", "link_preview": {
                "url":"https://www.youtube.com/watch?v=123", "title":"Demo",
                "embed": { "provider":"YouTube", "kind":"video", "poster":"https://img.example/poster.jpg" }
            }
        })).unwrap();
        let link = post_info(&commit, 80)
            .iter()
            .map(|line| {
                line.spans
                    .iter()
                    .map(|span| span.content.as_ref())
                    .collect::<String>()
            })
            .collect::<Vec<_>>()
            .join("\n");
        assert!(link.contains("Link · YouTube: Demo"));
        assert!(link.contains("www.youtube.com/watch?v=123"));
        assert!(!link.contains("poster.jpg"));
    }

    #[test]
    fn preview_is_two_lines_at_most() {
        let lines = preview_lines("a quick brown fox jumps over the lazy dog", 15);
        assert_eq!(lines.len(), 2);
        assert!(lines[1].ends_with('…'));
    }
}
