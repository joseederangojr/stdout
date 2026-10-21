use base64::{engine::general_purpose::URL_SAFE_NO_PAD, Engine};
use rand::RngCore;
use reqwest::{blocking::Client, Method, Url};
use serde::Deserialize;
use serde_json::{json, Value};
use sha2::{Digest, Sha256};
use std::{
    collections::VecDeque,
    fs,
    io::Write,
    net::IpAddr,
    path::{Path, PathBuf},
    process::{Command, Stdio},
    sync::{Arc, Mutex},
    time::{Duration, Instant, SystemTime, UNIX_EPOCH},
};

const ORIGIN: &str = "https://dev-null.sh";

fn parse_api_origin(value: Option<&str>) -> Result<Url, String> {
    let origin = Url::parse(value.unwrap_or(ORIGIN)).map_err(|error| error.to_string())?;
    let host = origin.host_str().unwrap_or_default();
    let loopback = host == "localhost"
        || host
            .trim_start_matches('[')
            .trim_end_matches(']')
            .parse::<IpAddr>()
            .is_ok_and(|address| address.is_loopback());
    let production = origin.scheme() == "https"
        && host == "dev-null.sh"
        && origin.port_or_known_default() == Some(443);
    let local = origin.scheme() == "http" && loopback;
    if (!production && !local)
        || !origin.username().is_empty()
        || origin.password().is_some()
        || origin.path() != "/"
        || origin.query().is_some()
        || origin.fragment().is_some()
    {
        return Err("API origin must be https://dev-null.sh or an HTTP loopback URL".into());
    }
    Ok(origin)
}

fn configured_api_origin() -> Result<Url, String> {
    match std::env::var("STDOUT_API_ORIGIN") {
        Ok(value) => parse_api_origin(Some(&value)),
        Err(std::env::VarError::NotPresent) => parse_api_origin(None),
        Err(std::env::VarError::NotUnicode(_)) => {
            Err("STDOUT_API_ORIGIN must be valid Unicode".into())
        }
    }
}

fn device_authorization_url(
    origin: &Url,
    fingerprint: &str,
    challenge: &str,
    state: &str,
) -> Result<Url, String> {
    let mut url = origin
        .join("/mobile/authorize")
        .map_err(|error| error.to_string())?;
    url.query_pairs_mut()
        .append_pair("fingerprint", fingerprint)
        .append_pair("device_name", "stdout")
        .append_pair("code_challenge", challenge)
        .append_pair("code_challenge_method", "S256")
        .append_pair("state", state)
        .append_pair("redirect_uri", "devnull://auth/callback");
    Ok(url)
}

fn config_directory(home: &Path) -> Result<PathBuf, String> {
    let root = home.join(".config");
    let current = root.join("stdout");
    let legacy = root.join("dev-null-tui");
    fs::create_dir_all(&current).map_err(|e| e.to_string())?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        fs::set_permissions(&current, fs::Permissions::from_mode(0o700))
            .map_err(|e| e.to_string())?;
        for name in ["device_ed25519", "config.toml"] {
            let destination = current.join(name);
            if !destination.exists() && legacy.join(name).exists() {
                fs::rename(legacy.join(name), &destination)
                    .map_err(|e| format!("migrate {name}: {e}"))?;
            }
            if destination.exists() {
                fs::set_permissions(&destination, fs::Permissions::from_mode(0o600))
                    .map_err(|e| e.to_string())?;
            }
        }
    }
    #[cfg(not(unix))]
    for name in ["device_ed25519", "config.toml"] {
        let destination = current.join(name);
        if !destination.exists() && legacy.join(name).exists() {
            fs::rename(legacy.join(name), &destination)
                .map_err(|e| format!("migrate {name}: {e}"))?;
        }
    }
    Ok(current)
}

#[derive(Clone, Debug, Deserialize)]
#[serde(untagged)]
pub enum Lane {
    Index(u8),
    Custom(String),
}

#[derive(Clone, Debug, Deserialize)]
pub struct Activity {
    pub buckets: Vec<ActivityBucket>,
}
#[derive(Clone, Debug, Deserialize)]
pub struct ActivityBucket {
    pub count: u64,
}
#[derive(Clone, Debug, Deserialize)]
pub struct Streaks {
    pub longest: u64,
    pub current: u64,
}
#[derive(Clone, Debug, Deserialize)]
pub struct Achievements {
    pub level: u64,
    pub rank: String,
    pub xp: u64,
    pub next_level_xp: Option<u64>,
    pub level_progress: u8,
    pub badges: Vec<AchievementBadge>,
}
#[derive(Clone, Debug, Deserialize)]
pub struct AchievementBadge {
    pub id: String,
    pub name: String,
    pub description: String,
    pub unlocked: bool,
    pub progress: u64,
    pub target: u64,
}

#[derive(Clone, Debug, Default, Deserialize)]
pub struct User {
    pub username: String,
    pub name: Option<String>,
    pub bio: Option<String>,
    pub company: Option<String>,
    pub location: Option<String>,
    pub website: Option<String>,
    pub github_url: Option<String>,
    pub created_at: Option<String>,
    pub lane: Option<Lane>,
    pub pasteis_mode: Option<bool>,
    pub pasteis_remaining: Option<u64>,
    pub pasteis_received: Option<u64>,
    pub preferred_lane: Option<Lane>,
    pub automatic_lane: Option<u8>,
    pub github_synced_at: Option<String>,
    pub activity: Option<Activity>,
    pub streaks: Option<Streaks>,
    pub achievements: Option<Achievements>,
    pub commits_count: Option<u64>,
    pub followers_count: Option<u64>,
    pub following_count: Option<u64>,
    pub is_followed: Option<bool>,
    pub follows_you: Option<bool>,
    pub is_muting: Option<bool>,
    pub html_url: Option<String>,
    pub sponsors_url: Option<String>,
}

#[derive(Clone, Debug, Deserialize)]
pub struct Commit {
    pub id: String,
    pub short_hash: Option<String>,
    pub body: Option<String>,
    pub status: Option<String>,
    pub parent_id: Option<String>,
    pub parent: Option<Box<RelatedCommit>>,
    pub quoted_commit_id: Option<String>,
    pub quoted_commit: Option<Box<RelatedCommit>>,
    pub link_preview: Option<LinkPreview>,
    pub images: Option<Vec<CommitImage>>,
    pub poll: Option<Poll>,
    pub pasteis_count: Option<u64>,
    pub author: Option<User>,
    pub created_at: Option<String>,
    pub stars_count: Option<u64>,
    pub replies_count: Option<u64>,
    pub starred: Option<bool>,
    pub html_url: Option<String>,
}

#[derive(Clone, Debug, Deserialize)]
pub struct RelatedCommit {
    pub id: String,
    pub short_hash: Option<String>,
    pub body: Option<String>,
    pub author: Option<User>,
    pub deleted_at: Option<String>,
}

#[derive(Clone, Debug, Deserialize)]
pub struct LinkPreview {
    pub url: String,
    pub embed: Option<Embed>,
    pub title: Option<String>,
    pub description: Option<String>,
    pub site_name: Option<String>,
    pub is_pending: Option<bool>,
}

#[derive(Clone, Debug, Deserialize)]
pub struct Embed {
    pub provider: String,
}

#[derive(Clone, Debug, Deserialize)]
pub struct CommitImage {
    pub url: String,
    pub description: Option<String>,
    pub width: Option<u32>,
    pub height: Option<u32>,
}

#[derive(Clone, Debug, Deserialize)]
pub struct Poll {
    pub is_closed: bool,
    pub has_voted: bool,
    pub total_votes: Option<u64>,
    pub options: Vec<PollOption>,
}

#[derive(Clone, Debug, Deserialize)]
pub struct PollOption {
    pub label: String,
    pub votes_count: Option<u64>,
    pub percentage: Option<u8>,
    pub voted: bool,
}

#[derive(Clone, Debug, Deserialize)]
pub struct Notice {
    #[serde(rename = "type")]
    pub kind: String,
    pub actor: Option<User>,
    pub commit: Option<Commit>,
    pub event_count: Option<u64>,
    pub unread_count: Option<u64>,
}

#[derive(Clone, Debug)]
pub enum Entry {
    Commit(Box<Commit>),
    User(Box<User>),
    Notice(Box<Notice>),
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Screen {
    Welcome,
    Feed(&'static str),
    Starred,
    Notifications,
    Muted,
    Blocked,
    Profile(String),
    ProfileReplies(String),
    Followers(String),
    Following(String),
    Thread(String),
}

impl Screen {
    pub fn title(&self) -> String {
        match self {
            Self::Welcome => "WELCOME".into(),
            Self::Feed(log) => log.to_uppercase(),
            Self::Starred => "STARRED".into(),
            Self::Notifications => "NOTIFICATIONS".into(),
            Self::Muted => "MUTED".into(),
            Self::Blocked => "BLOCKED".into(),
            Self::Profile(name) => format!("@{name}"),
            Self::ProfileReplies(name) => format!("@{name} / REPLIES"),
            Self::Followers(name) => format!("@{name} / FOLLOWERS"),
            Self::Following(name) => format!("@{name} / FOLLOWING"),
            Self::Thread(_) => "THREAD".into(),
        }
    }

    pub fn private(&self) -> bool {
        matches!(
            self,
            Self::Feed(_) | Self::Starred | Self::Notifications | Self::Muted | Self::Blocked
        )
    }
}

pub struct Pulse {
    pub unread: u64,
    pub profile: Option<User>,
    pub new_commits: Option<u64>,
}

pub struct Page {
    pub entries: Vec<Entry>,
    pub next: Option<String>,
    pub profile: Option<User>,
}

pub enum Action {
    Create {
        body: String,
        parent: Option<String>,
        quote: Option<String>,
    },
    Star {
        id: String,
        remove: bool,
    },
    Follow {
        username: String,
        remove: bool,
    },
    Mute {
        username: String,
        remove: bool,
    },
    Block {
        username: String,
        remove: bool,
    },
    Pastel {
        id: String,
    },
    SetLane {
        lane: Option<u8>,
        color: Option<String>,
    },
    SetPasteisMode {
        enabled: bool,
    },
    SyncGithub,
    Report {
        id: String,
        reason: &'static str,
    },
}

pub struct AuthFlow {
    pub url: String,
    verifier: String,
    state: String,
}

impl AuthFlow {
    pub fn begin(key: &PathBuf) -> Result<Self, String> {
        if let Some(dir) = key.parent() {
            fs::create_dir_all(dir).map_err(|e| e.to_string())?;
            #[cfg(unix)]
            {
                use std::os::unix::fs::PermissionsExt;
                fs::set_permissions(dir, fs::Permissions::from_mode(0o700))
                    .map_err(|e| e.to_string())?;
            }
        }
        if !key.exists() {
            let output = Command::new("ssh-keygen")
                .args(["-q", "-t", "ed25519", "-N", "", "-C", "stdout", "-f"])
                .arg(key)
                .output()
                .map_err(|e| format!("ssh-keygen: {e}"))?;
            if !output.status.success() {
                return Err(format!(
                    "ssh-keygen: {}",
                    String::from_utf8_lossy(&output.stderr)
                ));
            }
        }
        let output = Command::new("ssh-keygen")
            .args(["-l", "-f"])
            .arg(key.with_extension("pub"))
            .args(["-E", "sha256"])
            .output()
            .map_err(|e| format!("ssh-keygen: {e}"))?;
        if !output.status.success() {
            return Err(format!(
                "SSH public key: {}",
                String::from_utf8_lossy(&output.stderr)
            ));
        }
        let fingerprint = String::from_utf8_lossy(&output.stdout)
            .split_whitespace()
            .find(|word| word.starts_with("SHA256:"))
            .ok_or("Could not read SSH key fingerprint")?
            .to_string();
        let mut verifier_bytes = [0; 32];
        let mut state_bytes = [0; 16];
        rand::thread_rng().fill_bytes(&mut verifier_bytes);
        rand::thread_rng().fill_bytes(&mut state_bytes);
        let verifier = URL_SAFE_NO_PAD.encode(verifier_bytes);
        let state = URL_SAFE_NO_PAD.encode(state_bytes);
        let challenge = URL_SAFE_NO_PAD.encode(Sha256::digest(verifier.as_bytes()));
        let origin = configured_api_origin()?;
        let url = device_authorization_url(&origin, &fingerprint, &challenge, &state)?;
        Ok(Self {
            url: url.into(),
            verifier,
            state,
        })
    }

    pub fn code(&self, callback: &str) -> Result<String, String> {
        let url =
            Url::parse(callback.trim()).map_err(|_| "Paste the full devnull:// callback URL")?;
        if url.scheme() != "devnull" || url.host_str() != Some("auth") || url.path() != "/callback"
        {
            return Err("Unexpected callback URL".into());
        }
        let values: Vec<_> = url.query_pairs().collect();
        let get = |key: &str| -> Result<String, String> {
            let found: Vec<_> = values.iter().filter(|(k, _)| k == key).collect();
            if found.len() != 1 {
                return Err(format!("Missing or duplicate {key}"));
            }
            Ok(found[0].1.to_string())
        };
        if get("state")? != self.state {
            return Err("Login state mismatch; start again".into());
        }
        if let Ok(error) = get("error") {
            return Err(format!("Authorization declined: {error}"));
        }
        get("code")
    }

    pub fn verifier(&self) -> String {
        self.verifier.clone()
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct RequestLogEntry {
    pub at: Instant,
    pub method: String,
    pub path: String,
    pub status: Option<u16>,
    pub duration_ms: u128,
    pub request_bytes: usize,
    pub response_bytes: usize,
    pub outcome: Option<String>,
}

#[derive(Clone, Default)]
pub struct RequestLog(Arc<Mutex<VecDeque<RequestLogEntry>>>);

impl RequestLog {
    const CAPACITY: usize = 100;

    pub fn entries(&self) -> Vec<RequestLogEntry> {
        self.0
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .iter()
            .cloned()
            .collect()
    }

    fn push(&self, entry: RequestLogEntry) {
        let mut entries = self
            .0
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        if entries.len() == Self::CAPACITY {
            entries.pop_front();
        }
        entries.push_back(entry);
    }
}

struct RequestTrace<'a> {
    method: &'a Method,
    path: &'a str,
    started: Instant,
    request_bytes: usize,
}

pub struct Api {
    client: Client,
    origin: Url,
    key: PathBuf,
    approved: bool,
    request_log: Option<RequestLog>,
}

impl Api {
    pub fn new() -> Result<Self, String> {
        let origin = configured_api_origin()?;
        let home = std::env::var_os("HOME").ok_or("HOME is not set")?;
        let config = config_directory(&PathBuf::from(&home))?;
        let client = Client::builder()
            .timeout(Duration::from_secs(15))
            .redirect(reqwest::redirect::Policy::none())
            .user_agent(concat!("stdout/", env!("CARGO_PKG_VERSION")))
            .build()
            .map_err(|e| e.to_string())?;
        Ok(Self {
            client,
            origin,
            key: config.join("device_ed25519"),
            approved: false,
            request_log: None,
        })
    }

    pub fn enable_request_log(&mut self, request_log: RequestLog) {
        self.request_log = Some(request_log);
    }

    pub fn key(&self) -> &PathBuf {
        &self.key
    }

    pub fn forget_approval(&mut self) {
        self.approved = false;
    }

    pub fn bootstrap(&mut self) -> Result<Option<User>, String> {
        if !self.key.exists() {
            return Ok(None);
        }
        match self.request(Method::GET, "/api/v1/me", None, true) {
            Ok(value) => {
                let user = decode(value.get("data").cloned().ok_or("Missing user")?)?;
                self.approved = true;
                Ok(Some(user))
            }
            Err(e) if e.contains("device_not_approved") => Ok(None),
            Err(e) => Err(e),
        }
    }

    pub fn redeem(&mut self, code: String, verifier: String) -> Result<User, String> {
        let value = self.request(
            Method::POST,
            "/api/v1/devices",
            Some(json!({"code": code, "code_verifier": verifier})),
            true,
        )?;
        let user = decode(value.get("data").cloned().ok_or("Missing user")?)?;
        self.approved = true;
        Ok(user)
    }

    pub fn load(&self, screen: &Screen, next: Option<&str>) -> Result<Page, String> {
        if screen.private() && !self.approved {
            return Err("Sign in with L to use this view".into());
        }
        if *screen == Screen::Welcome {
            return Ok(Page {
                entries: vec![],
                next: None,
                profile: None,
            });
        }
        let mut profile = None;
        let mut entries = vec![];
        let path = if let Some(next) = next {
            next.to_owned()
        } else {
            match screen {
                Screen::Feed(log) => format!("/api/v1/log/{log}"),
                Screen::Notifications => "/api/v1/notifications".into(),
                Screen::Starred => "/api/v1/starred".into(),
                Screen::Muted => "/api/v1/me/muted".into(),
                Screen::Blocked => "/api/v1/me/blocked".into(),
                Screen::Profile(name) | Screen::ProfileReplies(name) => {
                    let value = self.get(&format!("/api/v1/users/{name}"))?;
                    let mut user: User = decode(value["data"].clone())?;
                    user.is_followed = value["is_following"].as_bool().or(user.is_followed);
                    user.follows_you = value["follows_you"].as_bool().or(user.follows_you);
                    user.is_muting = value["is_muting"].as_bool();
                    user.pasteis_received =
                        value["pasteis_received"].as_u64().or(user.pasteis_received);
                    user.activity = decode(value["activity"].clone()).ok();
                    user.streaks = decode(value["streaks"].clone()).ok();
                    user.achievements = decode(value["achievements"].clone()).ok();
                    user.sponsors_url = value["sponsors_url"].as_str().map(str::to_owned);
                    profile = Some(user);
                    if matches!(screen, Screen::Profile(_)) && value["pinned_commit"].is_object() {
                        entries.push(Entry::Commit(Box::new(decode(
                            value["pinned_commit"].clone(),
                        )?)));
                    }
                    format!(
                        "/api/v1/users/{name}/commits{}",
                        if matches!(screen, Screen::ProfileReplies(_)) {
                            "?replies=1"
                        } else {
                            ""
                        }
                    )
                }
                Screen::Followers(name) => format!("/api/v1/users/{name}/followers"),
                Screen::Following(name) => format!("/api/v1/users/{name}/following"),
                Screen::Thread(id) => {
                    let value = self.get(&format!("/api/v1/commits/{id}"))?;
                    if let Some(ancestors) = value["ancestors"].as_array() {
                        for item in ancestors {
                            entries.push(Entry::Commit(Box::new(decode(item.clone())?)));
                        }
                    }
                    entries.push(Entry::Commit(Box::new(decode(value["data"].clone())?)));
                    if !self.approved {
                        return Ok(Page {
                            entries,
                            next: None,
                            profile: None,
                        });
                    }
                    format!("/api/v1/commits/{id}/replies?newest=1")
                }
                Screen::Welcome => unreachable!(),
            }
        };
        let value = self.get(&path)?;
        let list = value["data"]
            .as_array()
            .ok_or("Invalid API list response")?;
        for item in list {
            entries.push(match screen {
                Screen::Followers(_) | Screen::Following(_) | Screen::Muted | Screen::Blocked => {
                    Entry::User(Box::new(decode(item.clone())?))
                }
                Screen::Notifications => Entry::Notice(Box::new(decode(item.clone())?)),
                _ => Entry::Commit(Box::new(decode(item.clone())?)),
            });
        }
        let next = value["links"]["next"].as_str().map(str::to_owned);
        Ok(Page {
            entries,
            next,
            profile,
        })
    }

    pub fn pulse(
        &self,
        username: &str,
        log: Option<&str>,
        since: Option<&str>,
    ) -> Result<Pulse, String> {
        if !self.approved {
            return Err("Sign in first".into());
        }
        let me = self.get("/api/v1/me")?;
        let unread = me["unread_notifications_count"]
            .as_u64()
            .ok_or("Invalid unread count")?;
        let profile = self.user(username).ok();
        let new_commits = match (log, since) {
            (Some(log @ ("following" | "latest")), Some(id))
                if id
                    .bytes()
                    .all(|byte| byte.is_ascii_alphanumeric() || byte == b'-') =>
            {
                self.get(&format!("/api/v1/log/{log}?since={id}"))
                    .ok()
                    .and_then(|data| data["new_commits_count"].as_u64())
            }
            _ => None,
        };
        Ok(Pulse {
            unread,
            profile,
            new_commits,
        })
    }

    pub fn user(&self, username: &str) -> Result<User, String> {
        if valid_username(username).is_none() {
            return Err("Invalid username".into());
        }
        let value = self.get(&format!("/api/v1/users/{username}"))?;
        let mut user: User = decode(value["data"].clone())?;
        user.is_followed = value["is_following"].as_bool().or(user.is_followed);
        user.follows_you = value["follows_you"].as_bool().or(user.follows_you);
        user.is_muting = value["is_muting"].as_bool();
        user.pasteis_received = value["pasteis_received"].as_u64().or(user.pasteis_received);
        user.activity = decode(value["activity"].clone()).ok();
        user.streaks = decode(value["streaks"].clone()).ok();
        user.achievements = decode(value["achievements"].clone()).ok();
        user.sponsors_url = value["sponsors_url"].as_str().map(str::to_owned);
        Ok(user)
    }

    pub fn replies(&self, id: &str, next: Option<&str>) -> Result<Page, String> {
        if !self.approved {
            return Err("Sign in with L to view replies".into());
        }
        let path = next
            .map(str::to_owned)
            .unwrap_or_else(|| format!("/api/v1/commits/{id}/replies?newest=1"));
        let value = self.get(&path)?;
        let entries = value["data"]
            .as_array()
            .ok_or("Invalid replies response")?
            .iter()
            .map(|item| decode(item.clone()).map(Box::new).map(Entry::Commit))
            .collect::<Result<Vec<_>, _>>()?;
        Ok(Page {
            entries,
            next: value["links"]["next"].as_str().map(str::to_owned),
            profile: None,
        })
    }

    pub fn act(&self, action: Action) -> Result<(String, Option<User>), String> {
        if !self.approved {
            return Err("Sign in with L first".into());
        }
        let settings = matches!(
            action,
            Action::SetLane { .. } | Action::SetPasteisMode { .. } | Action::SyncGithub
        );
        let (method, path, body, message) = match action {
            Action::Create {
                body,
                parent,
                quote,
            } => (
                Method::POST,
                "/api/v1/commits".into(),
                Some(json!({"body": body, "parent_id": parent, "quoted_commit_id": quote})),
                "Commit submitted for review",
            ),
            Action::Star { id, remove } => (
                if remove { Method::DELETE } else { Method::POST },
                format!("/api/v1/commits/{id}/star"),
                None,
                if remove { "Star removed" } else { "Starred" },
            ),
            Action::Follow { username, remove } => (
                if remove { Method::DELETE } else { Method::POST },
                format!("/api/v1/users/{username}/follow"),
                None,
                if remove { "Unfollowed" } else { "Following" },
            ),
            Action::Mute { username, remove } => (
                if remove { Method::DELETE } else { Method::POST },
                format!("/api/v1/users/{username}/mute"),
                None,
                if remove { "Unmuted" } else { "Muted" },
            ),
            Action::Block { username, remove } => (
                if remove { Method::DELETE } else { Method::POST },
                format!("/api/v1/users/{username}/block"),
                None,
                if remove {
                    "Unblocked (follows not restored)"
                } else {
                    "Blocked (follows ended)"
                },
            ),
            Action::Pastel { id } => (
                Method::POST,
                format!("/api/v1/commits/{id}/pasteis"),
                None,
                "Pastel sent",
            ),
            Action::SetLane { lane, color } => (
                Method::PUT,
                "/api/v1/me/lane".into(),
                Some(match color {
                    Some(color) => json!({"color": color}),
                    None => json!({"lane": lane}),
                }),
                "Color preference saved",
            ),
            Action::SetPasteisMode { enabled } => (
                Method::PUT,
                "/api/v1/me/pasteis-mode".into(),
                Some(json!({"enabled": enabled})),
                if enabled {
                    "Pastéis mode enabled"
                } else {
                    "Pastéis mode disabled"
                },
            ),
            Action::SyncGithub => (
                Method::PUT,
                "/api/v1/me/github".into(),
                None,
                "GitHub profile synced",
            ),
            Action::Report { id, reason } => (
                Method::POST,
                format!("/api/v1/commits/{id}/reports"),
                Some(json!({"reason": reason})),
                "Report sent privately",
            ),
        };
        let value = self.request(method, &path, body, false)?;
        let user = if settings {
            Some(decode(value.get("data").cloned().ok_or("Missing user")?)?)
        } else {
            None
        };
        Ok((message.into(), user))
    }

    fn get(&self, path: &str) -> Result<Value, String> {
        self.request(Method::GET, path, None, false)
    }

    fn request(
        &self,
        method: Method,
        path: &str,
        body: Option<Value>,
        force_sign: bool,
    ) -> Result<Value, String> {
        let url = api_url(&self.origin, path)?;
        let bytes = body
            .map(|b| serde_json::to_vec(&b).map_err(|e| e.to_string()))
            .transpose()?;
        let trace = RequestTrace {
            method: &method,
            path: url.path(),
            started: Instant::now(),
            request_bytes: bytes.as_ref().map_or(0, Vec::len),
        };
        let mut request = self
            .client
            .request(method.clone(), url.clone())
            .header("Accept", "application/json");
        if force_sign || self.approved {
            let timestamp = SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .map_err(|e| e.to_string())?
                .as_secs()
                .to_string();
            let mut nonce_bytes = [0u8; 16];
            rand::thread_rng().fill_bytes(&mut nonce_bytes);
            let nonce: String = nonce_bytes.iter().map(|b| format!("{b:02x}")).collect();
            let path_and_query = match url.query() {
                Some(query) => format!("{}?{query}", url.path()),
                None => url.path().to_string(),
            };
            let signed = signed_payload(
                &timestamp,
                &nonce,
                method.as_str(),
                &path_and_query,
                bytes.as_deref().unwrap_or(b""),
            );
            let signature = sign(&self.key, &signed)?;
            request = request
                .header("X-Devnull-Timestamp", timestamp)
                .header("X-Devnull-Nonce", nonce)
                .header("X-SSH-Signature", signature);
        }
        if let Some(bytes) = bytes {
            request = request
                .header("Content-Type", "application/json")
                .body(bytes);
        }
        let response = match request.send() {
            Ok(response) => response,
            Err(error) => {
                self.log_request(&trace, None, 0, Some("network error"));
                return Err(format!("Network: {error}"));
            }
        };
        let status = response.status();
        let retry_after = response
            .headers()
            .get(reqwest::header::RETRY_AFTER)
            .and_then(|header| header.to_str().ok())
            .map(str::to_owned);
        if status == reqwest::StatusCode::NO_CONTENT {
            self.log_request(&trace, Some(status), 0, None);
            return Ok(Value::Null);
        }
        let response_bytes = match response.bytes() {
            Ok(bytes) => bytes,
            Err(error) => {
                self.log_request(&trace, Some(status), 0, Some("response read error"));
                return Err(error.to_string());
            }
        };
        let response_len = response_bytes.len();
        let value: Value = match serde_json::from_slice(&response_bytes) {
            Ok(value) => value,
            Err(_) => {
                self.log_request(
                    &trace,
                    Some(status),
                    response_len,
                    Some("invalid JSON response"),
                );
                return Err(format!("HTTP {status}: unexpected response"));
            }
        };
        if !status.is_success() {
            let message = value["message"].as_str().unwrap_or("Request failed");
            let code = value["code"]
                .as_str()
                .filter(|code| {
                    !code.is_empty()
                        && code
                            .chars()
                            .all(|ch| ch.is_ascii_alphanumeric() || "_-".contains(ch))
                })
                .unwrap_or("api_error");
            self.log_request(
                &trace,
                Some(status),
                response_len,
                Some(&format!("API error {code}")),
            );
            let retry = if status == reqwest::StatusCode::TOO_MANY_REQUESTS {
                retry_after
                    .map(|value| format!(" · retry after {value}s"))
                    .unwrap_or_default()
            } else {
                String::new()
            };
            return Err(format!("HTTP {status} ({code}): {message}{retry}"));
        }
        self.log_request(&trace, Some(status), response_len, None);
        Ok(value)
    }

    fn log_request(
        &self,
        trace: &RequestTrace<'_>,
        status: Option<reqwest::StatusCode>,
        response_bytes: usize,
        outcome: Option<&str>,
    ) {
        if let Some(log) = &self.request_log {
            log.push(RequestLogEntry {
                at: trace.started,
                method: trace.method.to_string(),
                path: trace.path.to_string(),
                status: status.map(|status| status.as_u16()),
                duration_ms: trace.started.elapsed().as_millis(),
                request_bytes: trace.request_bytes,
                response_bytes,
                outcome: outcome.map(str::to_owned),
            });
        }
    }
}

fn api_url(origin: &Url, path: &str) -> Result<Url, String> {
    let url = if path.starts_with("/api/v1/") {
        origin.join(path)
    } else {
        Url::parse(path)
    }
    .map_err(|e| e.to_string())?;
    let same_origin = url.scheme() == origin.scheme()
        && url.host_str() == origin.host_str()
        && url.port_or_known_default() == origin.port_or_known_default();
    if !same_origin
        || !url.username().is_empty()
        || url.password().is_some()
        || !url.path().starts_with("/api/v1/")
        || url.fragment().is_some()
    {
        return Err("Refusing API URL outside configured origin".into());
    }
    Ok(url)
}

fn signed_payload(timestamp: &str, nonce: &str, method: &str, path: &str, body: &[u8]) -> Vec<u8> {
    let mut payload = format!("{timestamp}\n{nonce}\n{method}\n{path}\n").into_bytes();
    payload.extend_from_slice(body);
    payload
}

fn sign(key: &PathBuf, payload: &[u8]) -> Result<String, String> {
    let mut process = Command::new("ssh-keygen")
        .args(["-Y", "sign", "-n", "devnull-api", "-f"])
        .arg(key)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .map_err(|e| format!("ssh-keygen: {e}"))?;
    process
        .stdin
        .take()
        .ok_or("ssh-keygen stdin unavailable")?
        .write_all(payload)
        .map_err(|e| e.to_string())?;
    let output = process.wait_with_output().map_err(|e| e.to_string())?;
    if !output.status.success() {
        return Err(format!(
            "Signing failed: {}",
            String::from_utf8_lossy(&output.stderr)
        ));
    }
    let armored = String::from_utf8(output.stdout).map_err(|e| e.to_string())?;
    let signature = armored
        .lines()
        .filter(|s| !s.starts_with("-----"))
        .collect::<String>();
    if signature.is_empty() {
        return Err("ssh-keygen returned no signature".into());
    }
    Ok(signature)
}

pub fn display_text(text: &str) -> String {
    text.chars()
        .filter(|ch| !ch.is_control() || *ch == '\n')
        .collect()
}

fn decode<T: for<'a> Deserialize<'a>>(mut value: Value) -> Result<T, String> {
    fn scrub(value: &mut Value) {
        match value {
            Value::String(text) => *text = display_text(text),
            Value::Array(items) => items.iter_mut().for_each(scrub),
            Value::Object(fields) => fields.values_mut().for_each(scrub),
            _ => {}
        }
    }
    scrub(&mut value);
    serde_json::from_value(value).map_err(|e| format!("Invalid API data: {e}"))
}

pub fn valid_username(name: &str) -> Option<String> {
    let name = name.trim().trim_start_matches('@');
    if (1..=39).contains(&name.len())
        && name.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'-')
    {
        Some(name.into())
    } else {
        None
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn device_authorization_uses_the_configured_local_origin() {
        let origin = parse_api_origin(Some("http://127.0.0.1:8000")).unwrap();
        let url = device_authorization_url(&origin, "fingerprint", "challenge", "state").unwrap();
        assert_eq!(url.host_str(), Some("127.0.0.1"));
        assert_eq!(url.port(), Some(8000));
        assert_eq!(url.path(), "/mobile/authorize");
    }

    #[test]
    fn api_origin_override_is_restricted_to_localhost_or_production() {
        assert_eq!(
            parse_api_origin(None).unwrap().as_str(),
            "https://dev-null.sh/"
        );
        for value in [
            "http://localhost:8000",
            "http://127.0.0.1:8000",
            "http://[::1]:8000",
        ] {
            assert!(parse_api_origin(Some(value)).is_ok(), "{value}");
        }
        for value in [
            "http://example.com",
            "https://example.com",
            "http://dev-null.sh",
            "https://dev-null.sh:8443",
            "http://localhost:8000/prefix",
            "http://user@localhost:8000",
        ] {
            assert!(parse_api_origin(Some(value)).is_err(), "{value}");
        }
    }

    #[test]
    fn pagination_stays_within_configured_api_origin() {
        let local = parse_api_origin(Some("http://localhost:8000")).unwrap();
        assert_eq!(
            api_url(&local, "/api/v1/me").unwrap().as_str(),
            "http://localhost:8000/api/v1/me"
        );
        assert!(api_url(&local, "http://evil.test/api/v1/me").is_err());
        assert!(api_url(&local, "https://dev-null.sh/api/v1/me").is_err());
        assert!(api_url(&local, "http://localhost:8000/outside").is_err());
    }

    #[test]
    fn request_log_keeps_a_bounded_metadata_only_history() {
        let log = RequestLog::default();
        for index in 0..=RequestLog::CAPACITY {
            log.push(RequestLogEntry {
                at: Instant::now(),
                method: "GET".into(),
                path: format!("/api/v1/log/{index}"),
                status: Some(200),
                duration_ms: 12,
                request_bytes: 0,
                response_bytes: 64,
                outcome: None,
            });
        }
        let entries = log.entries();
        assert_eq!(entries.len(), RequestLog::CAPACITY);
        assert_eq!(entries.first().unwrap().path, "/api/v1/log/1");
        assert_eq!(entries.last().unwrap().path, "/api/v1/log/100");
    }

    #[test]
    fn config_migration_moves_key_and_bindings_without_overwriting_new_files() {
        let home = std::env::temp_dir().join(format!("stdout-config-{}", rand::random::<u64>()));
        let old = home.join(".config/dev-null-tui");
        fs::create_dir_all(&old).unwrap();
        fs::write(old.join("device_ed25519"), "old-key").unwrap();
        fs::write(old.join("config.toml"), "[keymap]\nquit = \"q\"\n").unwrap();
        let current = config_directory(&home).unwrap();
        assert_eq!(current, home.join(".config/stdout"));
        assert_eq!(
            fs::read_to_string(current.join("device_ed25519")).unwrap(),
            "old-key"
        );
        assert!(!old.join("device_ed25519").exists());
        fs::write(current.join("config.toml"), "new config").unwrap();
        fs::write(old.join("config.toml"), "legacy config").unwrap();
        config_directory(&home).unwrap();
        assert_eq!(
            fs::read_to_string(current.join("config.toml")).unwrap(),
            "new config"
        );
        fs::remove_dir_all(home).unwrap();
    }

    #[test]
    fn payload_is_exact_and_has_no_trailing_newline() {
        assert_eq!(
            signed_payload(
                "123",
                "ab",
                "POST",
                "/api/v1/commits?x=1",
                br#"{"body":"hi"}"#
            ),
            b"123\nab\nPOST\n/api/v1/commits?x=1\n{\"body\":\"hi\"}"
        );
    }

    #[test]
    fn authenticated_user_settings_accept_lane_and_custom_hex_payloads() {
        let lane: User = decode(json!({"username":"alice","lane":2,"preferred_lane":2,"automatic_lane":4,"pasteis_mode":true,"pasteis_remaining":3,"pasteis_received":9,"github_synced_at":"2026-10-20T12:00:00Z"})).unwrap();
        assert!(matches!(lane.lane, Some(Lane::Index(2))));
        assert!(matches!(lane.preferred_lane, Some(Lane::Index(2))));
        assert_eq!(lane.automatic_lane, Some(4));
        assert_eq!(lane.pasteis_mode, Some(true));
        let custom: User =
            decode(json!({"username":"alice","lane":"#12abef","preferred_lane":"#12abef"}))
                .unwrap();
        assert!(matches!(custom.lane, Some(Lane::Custom(ref hex)) if hex == "#12abef"));
    }

    #[test]
    fn pagination_cannot_redirect_to_another_host() {
        let origin = parse_api_origin(None).unwrap();
        assert!(api_url(&origin, "https://evil.test/api/v1/saved").is_err());
        assert!(api_url(&origin, "https://dev-null.sh.evil.test/api/v1/saved").is_err());
        assert!(api_url(&origin, "http://dev-null.sh/api/v1/saved").is_err());
        assert!(api_url(&origin, "https://dev-null.sh/api/v1/saved").is_ok());
    }

    #[test]
    fn authorization_checks_scheme_state_and_code() {
        let flow = AuthFlow {
            url: String::new(),
            verifier: "verifier".into(),
            state: "secret".into(),
        };
        assert_eq!(
            flow.code("devnull://auth/callback?state=secret&code=hello")
                .unwrap(),
            "hello"
        );
        assert!(flow
            .code("devnull://auth/callback?state=other&code=hello")
            .is_err());
        assert!(flow
            .code("https://evil.test/?state=secret&code=hello")
            .is_err());
        assert!(flow
            .code("devnull://auth/callback?state=secret&state=secret&code=hello")
            .is_err());
    }

    #[test]
    fn api_text_cannot_inject_terminal_escape_sequences() {
        let user: User = decode(json!({"username": "alice", "name": "a\u{1b}[31mred"})).unwrap();
        assert_eq!(user.name.as_deref(), Some("a[31mred"));
        assert_eq!(display_text("a\nb\t"), "a\nb");
    }

    #[test]
    fn username_cannot_escape_api_path() {
        assert_eq!(valid_username("@dev-null"), Some("dev-null".into()));
        assert!(valid_username("../me").is_none());
        assert!(valid_username("x?cursor=bad").is_none());
    }

    #[test]
    fn openssh_signature_verifies_for_exact_payload() {
        let dir = std::env::temp_dir().join(format!("dev-null-tui-{}", rand::random::<u64>()));
        let key = dir.join("device_ed25519");
        let flow = AuthFlow::begin(&key).unwrap();
        assert!(flow.url.contains("code_challenge_method=S256"));
        let payload = signed_payload("123", "aabb", "GET", "/api/v1/me", b"");
        let signature = sign(&key, &payload).unwrap();
        let public = fs::read_to_string(key.with_extension("pub")).unwrap();
        let allowed = dir.join("allowed_signers");
        fs::write(&allowed, format!("tui {public}")).unwrap();
        let signature_file = dir.join("request.sig");
        let wrapped = signature
            .as_bytes()
            .chunks(70)
            .map(|line| std::str::from_utf8(line).unwrap())
            .collect::<Vec<_>>()
            .join("\n");
        fs::write(
            &signature_file,
            format!("-----BEGIN SSH SIGNATURE-----\n{wrapped}\n-----END SSH SIGNATURE-----\n"),
        )
        .unwrap();
        let mut verifier = Command::new("ssh-keygen")
            .args(["-Y", "verify", "-f"])
            .arg(&allowed)
            .args(["-I", "tui", "-n", "devnull-api", "-s"])
            .arg(&signature_file)
            .stdin(Stdio::piped())
            .stdout(Stdio::null())
            .stderr(Stdio::piped())
            .spawn()
            .unwrap();
        verifier.stdin.take().unwrap().write_all(&payload).unwrap();
        let output = verifier.wait_with_output().unwrap();
        assert!(
            output.status.success(),
            "{}",
            String::from_utf8_lossy(&output.stderr)
        );
        fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    #[ignore = "Makes a signed request to the live dev-null.sh API"]
    fn signed_request_reaches_live_server() {
        let dir = std::env::temp_dir().join(format!("dev-null-live-{}", rand::random::<u64>()));
        let key = dir.join("device_ed25519");
        AuthFlow::begin(&key).unwrap();
        let api = Api {
            client: Client::builder()
                .timeout(Duration::from_secs(15))
                .build()
                .unwrap(),
            origin: parse_api_origin(None).unwrap(),
            key,
            approved: false,
            request_log: None,
        };
        let error = api
            .request(Method::GET, "/api/v1/me", None, true)
            .unwrap_err();
        assert!(error.contains("device_not_approved"), "{error}");
        fs::remove_dir_all(dir).unwrap();
    }
}
