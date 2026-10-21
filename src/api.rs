use std::time::{SystemTime, UNIX_EPOCH};

use anyhow::{Context, Result};
use reqwest::{
    blocking::{Client, Response},
    Method, StatusCode,
};
use serde_json::{json, Value};
use url::Url;

use crate::device::DeviceKey;

const SIGNATURE_NAMESPACE: &str = "devnull-api";

pub struct ApiClient {
    origin: Url,
    client: Client,
    key: DeviceKey,
}

impl ApiClient {
    pub fn new(origin: Url, key: DeviceKey) -> Result<Self> {
        let client = Client::builder()
            .timeout(std::time::Duration::from_secs(15))
            .build()
            .context("creating API client")?;
        Ok(Self {
            origin,
            client,
            key,
        })
    }

    pub fn fingerprint(&self) -> &str {
        self.key.fingerprint()
    }

    pub fn current_user(&self) -> Result<Option<String>> {
        let url = self.origin.join("api/v1/me")?;
        let response = self.signed_request(Method::GET, url, &[])?;
        let (status, body) = response_body(response)?;
        let payload: Value = serde_json::from_slice(&body).unwrap_or(Value::Null);

        if status.is_success() {
            return payload["data"]["username"]
                .as_str()
                .map(|name| Some(name.to_owned()))
                .context("API response did not include the current username");
        }

        if payload["code"] == "device_not_approved" {
            return Ok(None);
        }

        Err(api_error(status, &payload))
    }

    pub fn redeem_device(&self, code: &str, verifier: &str) -> Result<String> {
        let url = self.origin.join("api/v1/devices")?;
        let body = serde_json::to_vec(&json!({
            "code": code,
            "code_verifier": verifier,
        }))?;
        let response = self.signed_request(Method::POST, url, &body)?;
        let (status, body) = response_body(response)?;
        let payload: Value = serde_json::from_slice(&body).unwrap_or(Value::Null);

        if !status.is_success() {
            return Err(api_error(status, &payload));
        }

        payload["data"]["username"]
            .as_str()
            .map(str::to_owned)
            .context("API response did not include the authenticated username")
    }

    fn signed_request(&self, method: Method, url: Url, body: &[u8]) -> Result<Response> {
        let timestamp = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .context("system clock is before Unix epoch")?
            .as_secs()
            .to_string();
        let nonce = random_nonce()?;
        let message = canonical_message(&timestamp, &nonce, &method, &url, body);
        let signature = self.key.sign(&message, SIGNATURE_NAMESPACE)?;

        let mut request = self
            .client
            .request(method, url)
            .header("X-Devnull-Timestamp", timestamp)
            .header("X-Devnull-Nonce", nonce)
            .header("X-SSH-Signature", signature);
        if !body.is_empty() {
            request = request.header("Content-Type", "application/json");
        }
        request
            .body(body.to_vec())
            .send()
            .context("requesting dev-null.sh API")
    }
}

fn canonical_message(
    timestamp: &str,
    nonce: &str,
    method: &Method,
    url: &Url,
    body: &[u8],
) -> Vec<u8> {
    let path_and_query = match url.query() {
        Some(query) => format!("{}?{query}", url.path()),
        None => url.path().to_owned(),
    };
    let mut message = format!("{timestamp}\n{nonce}\n{method}\n{path_and_query}\n").into_bytes();
    message.extend_from_slice(body);
    message
}

fn random_nonce() -> Result<String> {
    let mut bytes = [0_u8; 16];
    getrandom::fill(&mut bytes)
        .map_err(|error| anyhow::anyhow!("getting secure random bytes: {error}"))?;
    Ok(bytes.iter().map(|byte| format!("{byte:02x}")).collect())
}

fn response_body(response: Response) -> Result<(StatusCode, Vec<u8>)> {
    let status = response.status();
    let body = response.bytes().context("reading API response")?.to_vec();
    Ok((status, body))
}

fn api_error(status: StatusCode, payload: &Value) -> anyhow::Error {
    let code = payload["code"].as_str().unwrap_or("unknown");
    let message = payload["message"].as_str().unwrap_or("Request failed");
    anyhow::anyhow!("dev-null.sh API {status} ({code}): {message}")
}

#[cfg(test)]
mod tests {
    use reqwest::Method;
    use url::Url;

    use super::canonical_message;

    #[test]
    fn signature_message_matches_server_contract() {
        let url = Url::parse("https://dev-null.sh/api/v1/devices?x=1").unwrap();
        let message = canonical_message("123", "0123", &Method::POST, &url, b"{}");
        assert_eq!(message, b"123\n0123\nPOST\n/api/v1/devices?x=1\n{}");
    }
}
