use std::{
    env,
    fs::{self, OpenOptions},
    io::Write,
    path::{Path, PathBuf},
};

use anyhow::{bail, Context, Result};
use serde::Deserialize;

use crate::navigation::{Keymap, Page};
use url::Url;

const DEFAULT_ORIGIN: &str = "https://dev-null.sh";

#[derive(Clone, Debug)]
pub struct Config {
    pub api_origin: Url,
    pub dir: PathBuf,
    pub keymap: Keymap,
}

#[derive(Deserialize)]
struct FileConfig {
    api_origin: Option<String>,
    keymap: Option<std::collections::HashMap<String, String>>,
}

impl Config {
    pub fn load() -> Result<Self> {
        Self::load_at_dir(config_dir()?)
    }

    pub fn load_at_dir(dir: PathBuf) -> Result<Self> {
        fs::create_dir_all(&dir).with_context(|| format!("creating {}", dir.display()))?;
        set_dir_permissions(&dir)?;

        let file_path = dir.join("config.toml");
        let env_origin = env::var("DEVNULL_API_ORIGIN").ok();
        let file_config = if file_path.exists() {
            let contents = fs::read_to_string(&file_path)
                .with_context(|| format!("reading {}", file_path.display()))?;
            Some(
                toml::from_str::<FileConfig>(&contents)
                    .with_context(|| format!("parsing {}", file_path.display()))?,
            )
        } else {
            None
        };
        let file_origin = env_origin
            .is_none()
            .then(|| {
                file_config
                    .as_ref()
                    .and_then(|config| config.api_origin.as_deref())
            })
            .flatten();
        let api_origin = resolve_api_origin(env_origin.as_deref(), file_origin)?;
        let keymap = resolve_keymap(file_config.and_then(|config| config.keymap))?;

        Ok(Self {
            api_origin,
            dir,
            keymap,
        })
    }

    pub fn write_private(&self, path: &Path, contents: &[u8]) -> Result<()> {
        let mut options = OpenOptions::new();
        options.create(true).truncate(true).write(true);
        #[cfg(unix)]
        {
            use std::os::unix::fs::OpenOptionsExt;
            options.mode(0o600);
        }

        let mut file = options
            .open(path)
            .with_context(|| format!("opening {}", path.display()))?;
        set_file_permissions(path)?;
        file.write_all(contents)?;
        file.sync_all()?;
        Ok(())
    }
}

fn resolve_keymap(overrides: Option<std::collections::HashMap<String, String>>) -> Result<Keymap> {
    let mut bindings = [
        (Page::Log, "g l".to_owned()),
        (Page::Notifications, "g n".to_owned()),
        (Page::Profile, "g p".to_owned()),
        (Page::Starred, "g S".to_owned()),
        (Page::Remotes, "g r".to_owned()),
        (Page::Settings, "g s".to_owned()),
        (Page::Config, "g ,".to_owned()),
    ];
    if let Some(overrides) = overrides {
        for (name, sequence) in overrides {
            let page = match name.as_str() {
                "log" => Page::Log,
                "notifications" => Page::Notifications,
                "profile" => Page::Profile,
                "starred" => Page::Starred,
                "remotes" => Page::Remotes,
                "settings" => Page::Settings,
                "config" => Page::Config,
                _ => bail!("unknown keymap page: {name}"),
            };
            bindings
                .iter_mut()
                .find(|(candidate, _)| *candidate == page)
                .unwrap()
                .1 = sequence;
        }
    }
    Keymap::from_bindings(
        bindings
            .iter()
            .map(|(page, sequence)| (*page, sequence.as_str())),
    )
}

pub fn resolve_api_origin(env_origin: Option<&str>, file_origin: Option<&str>) -> Result<Url> {
    let value = env_origin.or(file_origin).unwrap_or(DEFAULT_ORIGIN).trim();
    let mut origin = Url::parse(value).context("API origin must be an absolute URL")?;

    if !matches!(origin.scheme(), "https" | "http")
        || origin.query().is_some()
        || origin.fragment().is_some()
        || !origin.username().is_empty()
        || origin.password().is_some()
    {
        bail!("API origin must be an http(s) URL without credentials, query, or fragment");
    }

    if origin.scheme() == "http" && !is_loopback(origin.host_str().unwrap_or_default()) {
        bail!("plain HTTP API origins are allowed only for localhost development");
    }

    let path = format!("{}/", origin.path().trim_end_matches('/'));
    origin.set_path(&path);
    Ok(origin)
}

fn config_dir() -> Result<PathBuf> {
    if let Some(path) = env::var_os("XDG_CONFIG_HOME") {
        let path = PathBuf::from(path);
        if !path.is_absolute() {
            bail!("XDG_CONFIG_HOME must be an absolute path");
        }
        return Ok(path.join("stdout"));
    }

    let home = env::var_os("HOME")
        .or_else(|| env::var_os("USERPROFILE"))
        .context("cannot find a home directory for stdout configuration")?;
    Ok(PathBuf::from(home).join(".config/stdout"))
}

fn is_loopback(host: &str) -> bool {
    host.eq_ignore_ascii_case("localhost")
        || host.ends_with(".localhost")
        || host
            .parse::<std::net::IpAddr>()
            .is_ok_and(|address| address.is_loopback())
}

fn set_dir_permissions(path: &Path) -> Result<()> {
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        fs::set_permissions(path, fs::Permissions::from_mode(0o700))?;
    }
    Ok(())
}

fn set_file_permissions(path: &Path) -> Result<()> {
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        fs::set_permissions(path, fs::Permissions::from_mode(0o600))?;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::{resolve_api_origin, resolve_keymap, FileConfig};

    #[test]
    fn reads_api_origin_from_toml() {
        let config: FileConfig = toml::from_str(r#"api_origin = "http://localhost:8000""#).unwrap();
        assert_eq!(config.api_origin.as_deref(), Some("http://localhost:8000"));
        assert!(config.keymap.is_none());
    }

    #[test]
    fn environment_origin_overrides_file_origin() {
        let origin =
            resolve_api_origin(Some("http://localhost:8000"), Some("https://example.test"))
                .unwrap();
        assert_eq!(origin.as_str(), "http://localhost:8000/");
    }

    #[test]
    fn file_origin_overrides_default() {
        let origin = resolve_api_origin(None, Some("https://api.example.test/")).unwrap();
        assert_eq!(origin.as_str(), "https://api.example.test/");
    }

    #[test]
    fn default_origin_is_production() {
        let origin = resolve_api_origin(None, None).unwrap();
        assert_eq!(origin.as_str(), "https://dev-null.sh/");
    }

    #[test]
    fn non_local_http_is_rejected() {
        assert!(resolve_api_origin(Some("http://api.example.test"), None).is_err());
    }

    #[test]
    fn reads_keymap_overrides_from_toml() {
        let config: FileConfig = toml::from_str(
            r#"[keymap]
log = "x l"
remotes = "x r"
config = "x ,""#,
        )
        .unwrap();
        let keymap = resolve_keymap(config.keymap).unwrap();
        assert_eq!(keymap.binding(crate::navigation::Page::Log), ['x', 'l']);
        assert_eq!(keymap.binding(crate::navigation::Page::Remotes), ['x', 'r']);
        assert_eq!(keymap.binding(crate::navigation::Page::Config), ['x', ',']);
    }

    #[test]
    fn keymap_overrides_defaults_and_rejects_duplicates() {
        let keymap = resolve_keymap(Some(std::collections::HashMap::from([(
            "log".to_owned(),
            "x l".to_owned(),
        )])))
        .unwrap();
        assert_eq!(keymap.binding(crate::navigation::Page::Log), ['x', 'l']);
        assert!(resolve_keymap(Some(std::collections::HashMap::from([(
            "log".to_owned(),
            "g n".to_owned(),
        )])))
        .is_err());
    }
}
