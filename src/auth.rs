use std::fs;

use anyhow::{bail, Context, Result};
use base64::{engine::general_purpose::URL_SAFE_NO_PAD, Engine};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use url::Url;

use crate::{api::ApiClient, config::Config, device::DeviceKey};

const CALLBACK_URI: &str = "devnull://auth/callback";

#[derive(Serialize, Deserialize)]
struct PendingAuthorization {
    state: String,
    verifier: String,
}

struct Callback {
    code: Option<String>,
    state: String,
    error: Option<String>,
}

pub struct AuthFlow {
    config: Config,
    api: ApiClient,
    pending: PendingAuthorization,
    authorization_url: String,
}

impl AuthFlow {
    pub fn new(config: Config) -> Result<Self> {
        let key = DeviceKey::load_or_create(&config.dir)?;
        let api = ApiClient::new(config.api_origin.clone(), key)?;
        let pending_path = pending_path(&config);
        let pending = if pending_path.exists() {
            serde_json::from_slice(&fs::read(&pending_path)?)
                .with_context(|| format!("parsing {}", pending_path.display()))?
        } else {
            new_pending(&config)?
        };
        validate_pending(&pending)?;
        let authorization_url = build_authorization_url(&config, &api, &pending)?;

        Ok(Self {
            config,
            api,
            pending,
            authorization_url,
        })
    }

    pub fn has_device_key(config: &Config) -> bool {
        config.dir.join("device_ed25519").exists()
    }

    pub fn current_user(&self) -> Result<Option<String>> {
        self.api.current_user()
    }

    pub fn authorization_url(&self) -> &str {
        &self.authorization_url
    }

    pub fn fingerprint(&self) -> &str {
        self.api.fingerprint()
    }

    pub fn restart(&mut self) -> Result<()> {
        let pending = make_pending()?;
        let authorization_url = build_authorization_url(&self.config, &self.api, &pending)?;
        write_pending(&self.config, &pending)?;
        let _ = fs::remove_file(callback_path(&self.config));
        self.pending = pending;
        self.authorization_url = authorization_url;
        Ok(())
    }

    pub fn complete_callback(&self, value: &str) -> Result<String> {
        let callback = parse_callback(value)?;
        validate_callback(&callback, &self.pending)?;
        let code = callback
            .code
            .context("callback has no authorization code")?;
        let username = self.api.redeem_device(&code, &self.pending.verifier)?;

        let _ = fs::remove_file(pending_path(&self.config));
        let _ = fs::remove_file(callback_path(&self.config));
        Ok(username)
    }

    pub fn take_received_callback(&self) -> Result<Option<String>> {
        let path = callback_path(&self.config);
        if !path.exists() {
            return Ok(None);
        }

        let callback =
            fs::read_to_string(&path).with_context(|| format!("reading {}", path.display()))?;
        fs::remove_file(path)?;
        Ok(Some(callback))
    }
}

pub fn receive_callback_for_running_tui(config: &Config, value: &str) -> Result<()> {
    let pending_file = pending_path(config);
    let pending: PendingAuthorization = serde_json::from_slice(
        &fs::read(&pending_file).context("no stdout sign-in is waiting for a callback")?,
    )?;
    validate_pending(&pending)?;
    let callback = parse_callback(value)?;
    validate_state(&callback, &pending)?;
    config.write_private(&callback_path(config), value.trim().as_bytes())
}

fn new_pending(config: &Config) -> Result<PendingAuthorization> {
    let pending = make_pending()?;
    write_pending(config, &pending)?;
    Ok(pending)
}

fn make_pending() -> Result<PendingAuthorization> {
    let mut verifier = [0_u8; 32];
    let mut state = [0_u8; 32];
    getrandom::fill(&mut verifier)
        .map_err(|error| anyhow::anyhow!("generating PKCE verifier: {error}"))?;
    getrandom::fill(&mut state)
        .map_err(|error| anyhow::anyhow!("generating authorization state: {error}"))?;
    Ok(PendingAuthorization {
        state: URL_SAFE_NO_PAD.encode(state),
        verifier: URL_SAFE_NO_PAD.encode(verifier),
    })
}

fn validate_pending(pending: &PendingAuthorization) -> Result<()> {
    if pending.state.len() != 43 || !is_base64url(&pending.state) {
        bail!("pending sign-in state is invalid; press r to restart");
    }
    if !(43..=128).contains(&pending.verifier.len()) || !is_base64url(&pending.verifier) {
        bail!("pending PKCE verifier is invalid; press r to restart");
    }
    Ok(())
}

fn is_base64url(value: &str) -> bool {
    value
        .bytes()
        .all(|byte| byte.is_ascii_alphanumeric() || byte == b'_' || byte == b'-')
}

fn challenge_for(verifier: &str) -> String {
    URL_SAFE_NO_PAD.encode(Sha256::digest(verifier.as_bytes()))
}

fn build_authorization_url(
    config: &Config,
    api: &ApiClient,
    pending: &PendingAuthorization,
) -> Result<String> {
    let challenge = challenge_for(&pending.verifier);
    let device_name = device_name();
    let mut url = config.api_origin.join("mobile/authorize")?;
    url.query_pairs_mut()
        .append_pair("fingerprint", api.fingerprint())
        .append_pair("device_name", &device_name)
        .append_pair("code_challenge", &challenge)
        .append_pair("code_challenge_method", "S256")
        .append_pair("state", &pending.state)
        .append_pair("redirect_uri", CALLBACK_URI);
    Ok(url.into())
}

fn device_name() -> String {
    let host = std::env::var("HOSTNAME")
        .or_else(|_| std::env::var("COMPUTERNAME"))
        .unwrap_or_else(|_| "computer".to_owned());
    format!("stdout on {}", host.chars().take(80).collect::<String>())
}

fn parse_callback(value: &str) -> Result<Callback> {
    let url = Url::parse(value.trim()).context("paste the full devnull:// callback URI")?;
    if url.scheme() != "devnull"
        || url.host_str() != Some("auth")
        || url.path() != "/callback"
        || url.fragment().is_some()
        || !url.username().is_empty()
        || url.password().is_some()
    {
        bail!("callback URI must use devnull://auth/callback");
    }

    let mut code = None;
    let mut state = None;
    let mut error = None;
    for (name, value) in url.query_pairs() {
        let slot = match name.as_ref() {
            "code" => Some(&mut code),
            "state" => Some(&mut state),
            "error" => Some(&mut error),
            _ => None,
        };
        if let Some(slot) = slot {
            if slot.replace(value.into_owned()).is_some() {
                bail!("callback URI contains duplicate {name} values");
            }
        }
    }

    let state = state.context("callback URI has no state")?;
    if state.len() > 255 {
        bail!("callback state is too long");
    }
    if code
        .as_ref()
        .is_some_and(|code| code.is_empty() || code.len() > 255)
    {
        bail!("authorization code is empty or too long");
    }

    Ok(Callback { code, state, error })
}

fn validate_state(callback: &Callback, pending: &PendingAuthorization) -> Result<()> {
    if callback.state != pending.state {
        bail!("callback state does not match this sign-in; restart authorization");
    }
    Ok(())
}

fn validate_callback(callback: &Callback, pending: &PendingAuthorization) -> Result<()> {
    validate_state(callback, pending)?;
    if callback.error.is_some() {
        bail!("sign-in was cancelled or denied; press r to restart");
    }
    if callback.code.is_none() {
        bail!("callback URI has no authorization code");
    }
    Ok(())
}

fn write_pending(config: &Config, pending: &PendingAuthorization) -> Result<()> {
    let bytes = serde_json::to_vec(pending)?;
    config.write_private(&pending_path(config), &bytes)
}

fn pending_path(config: &Config) -> std::path::PathBuf {
    config.dir.join("pending-auth.json")
}

fn callback_path(config: &Config) -> std::path::PathBuf {
    config.dir.join("auth-callback-uri")
}

#[cfg(test)]
mod tests {
    use std::{fs, path::PathBuf};

    use url::Url;

    use super::{
        make_pending, parse_callback, receive_callback_for_running_tui, validate_callback,
        write_pending,
    };
    use crate::config::Config;

    #[test]
    fn pkce_verifier_is_base64url_and_challenge_uses_s256() {
        use super::challenge_for;

        let pending = make_pending().unwrap();
        assert_eq!(pending.verifier.len(), 43);
        assert_eq!(challenge_for(&pending.verifier).len(), 43);
        assert!(pending
            .state
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || byte == b'_' || byte == b'-'));
        assert_eq!(
            challenge_for("dBjftJeZ4CVP-mB92K27uhbUJU1p1r_wW1gFWFOEjXk"),
            "E9Melhoa2OwvFrEMTJguCHaoeK1t8URWbuGJSstw-cM"
        );
    }

    #[test]
    fn callback_requires_expected_scheme_and_state() {
        let callback = parse_callback("devnull://auth/callback?code=abc&state=expected").unwrap();
        let pending = super::PendingAuthorization {
            state: "expected".to_owned(),
            verifier: "verifier".to_owned(),
        };
        assert!(validate_callback(&callback, &pending).is_ok());
        assert!(parse_callback("https://example.test/callback?code=abc&state=expected").is_err());

        let mismatch = parse_callback("devnull://auth/callback?code=abc&state=other").unwrap();
        assert!(validate_callback(&mismatch, &pending).is_err());
    }

    #[test]
    fn callback_requires_single_code_and_state() {
        assert!(parse_callback("devnull://auth/callback?code=a&code=b&state=s").is_err());
        assert!(parse_callback("devnull://auth/callback?code=a").is_err());
    }

    #[test]
    fn protocol_callback_is_persisted_only_for_the_pending_state() {
        use super::PendingAuthorization;

        let dir = test_dir();
        fs::create_dir_all(&dir).unwrap();
        let config = Config {
            api_origin: Url::parse("http://localhost:8000/").unwrap(),
            dir: dir.clone(),
        };
        let pending = PendingAuthorization {
            state: "s".repeat(43),
            verifier: "v".repeat(43),
        };
        write_pending(&config, &pending).unwrap();

        let callback = format!(
            "devnull://auth/callback?code=one-time&state={}",
            pending.state
        );
        assert!(receive_callback_for_running_tui(&config, &callback).is_ok());
        assert_eq!(
            fs::read_to_string(dir.join("auth-callback-uri")).unwrap(),
            callback
        );
        assert!(receive_callback_for_running_tui(
            &config,
            "devnull://auth/callback?code=wrong&state=other"
        )
        .is_err());

        fs::remove_dir_all(dir).unwrap();
    }

    fn test_dir() -> PathBuf {
        std::env::temp_dir().join(format!("stdout-auth-test-{}", std::process::id()))
    }
}
