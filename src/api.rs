use std::time::{SystemTime, UNIX_EPOCH};

use anyhow::{Context, Result};
use reqwest::{
    blocking::{Client, Response},
    Method, StatusCode,
};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use url::Url;

use crate::device::DeviceKey;

const SIGNATURE_NAMESPACE: &str = "devnull-api";

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct CurrentUser {
    pub username: String,
    pub email: Option<String>,
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq)]
#[serde(rename_all = "lowercase")]
pub enum LogKind {
    Following,
    Latest,
    Trending,
}

impl LogKind {
    fn path(self) -> &'static str {
        match self {
            Self::Following => "following",
            Self::Latest => "latest",
            Self::Trending => "trending",
        }
    }
}

#[derive(Debug, Deserialize)]
pub struct LogPage {
    pub data: Vec<LogCommit>,
    pub log: LogKind,
    pub is_ranked: bool,
    pub suggestions: Vec<LogCommit>,
    pub new_commits_count: Option<u64>,
    pub links: LogLinks,
    pub meta: LogMeta,
}

#[derive(Debug, Deserialize)]
pub struct CommitDetail {
    pub data: LogCommit,
    pub ancestors: Vec<RelatedCommit>,
}

#[derive(Debug, Deserialize)]
pub struct PasteisResult {
    pub data: LogCommit,
    pub pasteis_remaining: u64,
    pub pasteis_resets_at: String,
    pub author_pasteis_received: Option<u64>,
}

#[derive(Clone, Copy, Debug, Serialize, Eq, PartialEq)]
#[serde(rename_all = "lowercase")]
pub enum ReportReason {
    Spam,
    Harassment,
    Unsafe,
    Other,
}

enum CommitTarget<'a> {
    Parent(&'a str),
    Quote(&'a str),
}

#[derive(Debug, Deserialize)]
pub struct LogLinks {
    pub first: Option<String>,
    pub last: Option<String>,
    pub prev: Option<String>,
    pub next: Option<String>,
}

#[derive(Debug, Deserialize)]
pub struct LogMeta {
    pub path: String,
    pub per_page: u32,
    pub next_cursor: Option<String>,
    pub prev_cursor: Option<String>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum CommitStatus {
    Pending,
    Approved,
    Rejected,
}

#[derive(Debug, Deserialize)]
#[serde(untagged)]
pub enum LogLane {
    Number(u64),
    Name(String),
}

#[derive(Debug, Deserialize)]
pub struct LogAuthor {
    pub id: String,
    pub username: String,
    pub name: String,
    pub avatar_url: Option<String>,
    pub bio: Option<String>,
    pub company: Option<String>,
    pub location: Option<String>,
    pub website: Option<String>,
    pub github_url: String,
    pub html_url: String,
    pub feed_url: String,
    pub created_at: String,
    pub lane: LogLane,
}

#[derive(Debug, Deserialize)]
#[serde(tag = "type", rename_all = "lowercase")]
pub enum BodySegment {
    Text { text: String },
    Code { code: String },
    Block { language: String, html: String },
}

#[derive(Debug, Deserialize)]
pub struct Mention {
    pub username: String,
    pub offset: u64,
    pub length: u64,
}

#[derive(Debug, Deserialize)]
pub struct LinkPreview {
    pub url: String,
    pub title: Option<String>,
    pub description: Option<String>,
    pub image_url: Option<String>,
    pub site_name: Option<String>,
    pub is_pending: bool,
    pub embed: Option<Value>,
}

#[derive(Debug, Deserialize)]
pub struct LogImage {
    pub id: String,
    pub url: String,
    pub width: u32,
    pub height: u32,
    pub description: Option<String>,
}

#[derive(Debug, Deserialize)]
pub struct LogPoll {
    pub id: String,
    pub closes_at: Option<String>,
    pub is_closed: bool,
    pub has_voted: bool,
    pub total_votes: Option<u64>,
    pub options: Vec<LogPollOption>,
}

#[derive(Debug, Deserialize)]
pub struct LogPollOption {
    pub id: String,
    pub label: String,
    pub votes_count: Option<u64>,
    pub voted: bool,
    pub percentage: Option<f64>,
}

#[derive(Debug, Deserialize)]
pub struct LogCommit {
    pub id: String,
    pub short_hash: String,
    pub body: String,
    pub parent_id: Option<String>,
    pub html_url: String,
    pub created_at: String,
    pub status: CommitStatus,
    pub deleted_at: Option<String>,
    pub author: Option<LogAuthor>,
    pub stars_count: u64,
    pub replies_count: u64,
    pub pasteis_count: u64,
    pub starred: bool,
    pub is_pinned: bool,
    pub body_segments: Vec<BodySegment>,
    pub preview_segments: Option<Vec<BodySegment>>,
    pub mentions: Vec<Mention>,
    pub link_preview: Option<LinkPreview>,
    pub images: Vec<LogImage>,
    pub quoted_commit_id: Option<String>,
    pub quoted_commit: Option<Box<RelatedCommit>>,
    pub poll: Option<LogPoll>,
    // Detail and star responses omit reply_preview; log cards include it.
    pub reply_preview: Option<ReplyPreview>,
}

#[derive(Debug, Deserialize)]
pub struct ReplyPreview {
    pub data: Vec<RelatedCommit>,
    pub next_cursor: Option<String>,
}

/// Nested commits are load-dependent and may be tombstones without a body or author.
#[derive(Debug, Deserialize)]
pub struct RelatedCommit {
    pub id: String,
    pub parent_id: Option<String>,
    pub deleted_at: Option<String>,
    pub body: Option<String>,
    pub author: Option<LogAuthor>,
    pub replies_count: Option<u64>,
}

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

    pub fn current_user(&self) -> Result<Option<CurrentUser>> {
        let url = self.origin.join("api/v1/me")?;
        let response = self.signed_request(Method::GET, url, &[])?;
        let (status, body) = response_body(response)?;
        let payload: Value = serde_json::from_slice(&body).unwrap_or(Value::Null);

        if status.is_success() {
            return parse_current_user(&payload).map(Some);
        }

        if payload["code"] == "device_not_approved" {
            return Ok(None);
        }

        Err(api_error(status, &payload))
    }

    pub fn log(&self, log: LogKind, cursor: Option<&str>) -> Result<LogPage> {
        let url = log_url(&self.origin, log, cursor)?;
        let response = self.signed_request(Method::GET, url, &[])?;
        let (status, body) = response_body(response)?;
        parse_log_response(status, &body)
    }

    /// Signed detail includes visible ancestors; own pending/rejected commits remain readable.
    pub fn commit(&self, id: &str) -> Result<CommitDetail> {
        let url = commit_url(&self.origin, id, false)?;
        let response = self.signed_request(Method::GET, url, &[])?;
        let (status, body) = response_body(response)?;
        parse_commit_response(status, &body)
    }

    /// Star only another author's visible commit; the server rejects own commits (403).
    pub fn star(&self, id: &str) -> Result<LogCommit> {
        self.set_star(id, Method::POST)
    }

    /// Send a pastéis to another author's visible commit. Mode, quota and throttling are server-enforced.
    pub fn pasteis(&self, id: &str) -> Result<PasteisResult> {
        let url = pasteis_url(&self.origin, id)?;
        let response = self.signed_request(Method::POST, url, &[])?;
        let (status, body) = response_body(response)?;
        parse_pasteis_response(status, &body)
    }

    /// Unstar requires a visible commit; both operations return refreshed counts/state.
    pub fn unstar(&self, id: &str) -> Result<LogCommit> {
        self.set_star(id, Method::DELETE)
    }

    /// Reply to a visible commit. Posting may be rejected for rules, timeout or rate limits.
    pub fn reply(&self, parent_id: &str, body: &str) -> Result<LogCommit> {
        self.create_commit(body, CommitTarget::Parent(parent_id))
    }

    /// Quote a visible commit without adding it as a reply parent.
    pub fn quote(&self, quoted_commit_id: &str, body: &str) -> Result<LogCommit> {
        self.create_commit(body, CommitTarget::Quote(quoted_commit_id))
    }

    fn create_commit(&self, body: &str, target: CommitTarget<'_>) -> Result<LogCommit> {
        let body = create_commit_body(body, target)?;
        let url = self.origin.join("api/v1/commits")?;
        let response = self.signed_request(Method::POST, url, &body)?;
        let (status, body) = response_body(response)?;
        parse_created_commit(status, &body)
    }

    /// Only other authors' approved, visible commits can be reported. The server
    /// enforces ownership and visibility (including pending/rejected commits).
    pub fn report(&self, id: &str, reason: ReportReason, details: Option<&str>) -> Result<()> {
        let url = report_url(&self.origin, id)?;
        let body = serde_json::to_vec(&json!({"reason": reason, "details": details}))?;
        let response = self.signed_request(Method::POST, url, &body)?;
        let (status, body) = response_body(response)?;
        parse_report_response(status, &body)
    }

    fn set_star(&self, id: &str, method: Method) -> Result<LogCommit> {
        let url = commit_url(&self.origin, id, true)?;
        let response = self.signed_request(method, url, &[])?;
        let (status, body) = response_body(response)?;
        parse_commit_data(status, &body)
    }

    pub fn redeem_device(&self, code: &str, verifier: &str) -> Result<CurrentUser> {
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

        parse_current_user(&payload)
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

fn log_url(origin: &Url, log: LogKind, cursor: Option<&str>) -> Result<Url> {
    let mut url = origin.join(&format!("api/v1/log/{}", log.path()))?;
    if let Some(cursor) = cursor {
        url.query_pairs_mut().append_pair("cursor", cursor);
    }
    Ok(url)
}

fn validate_commit_id(id: &str) -> Result<()> {
    anyhow::ensure!(
        id.len() == 36
            && id.bytes().enumerate().all(|(index, byte)| {
                if [8, 13, 18, 23].contains(&index) {
                    byte == b'-'
                } else {
                    byte.is_ascii_hexdigit()
                }
            }),
        "commit id must be a UUID"
    );
    Ok(())
}

fn create_commit_body(body: &str, target: CommitTarget<'_>) -> Result<Vec<u8>> {
    let payload = match target {
        CommitTarget::Parent(id) => {
            validate_commit_id(id)?;
            json!({"body": body, "parent_id": id})
        }
        CommitTarget::Quote(id) => {
            validate_commit_id(id)?;
            json!({"body": body, "quoted_commit_id": id})
        }
    };
    Ok(serde_json::to_vec(&payload)?)
}

fn commit_url(origin: &Url, id: &str, star: bool) -> Result<Url> {
    validate_commit_id(id)?;
    let mut url = origin.join("api/v1/commits/")?;
    {
        let mut path = url
            .path_segments_mut()
            .map_err(|_| anyhow::anyhow!("invalid API origin"))?;
        path.pop_if_empty().push(id);
        if star {
            path.push("star");
        }
    }
    Ok(url)
}

fn report_url(origin: &Url, id: &str) -> Result<Url> {
    let mut url = commit_url(origin, id, false)?;
    url.path_segments_mut()
        .map_err(|_| anyhow::anyhow!("invalid API origin"))?
        .push("reports");
    Ok(url)
}

fn pasteis_url(origin: &Url, id: &str) -> Result<Url> {
    let mut url = commit_url(origin, id, false)?;
    url.path_segments_mut()
        .map_err(|_| anyhow::anyhow!("invalid API origin"))?
        .push("pasteis");
    Ok(url)
}

fn parse_commit_response(status: StatusCode, body: &[u8]) -> Result<CommitDetail> {
    parse_api_response(status, body, "invalid dev-null.sh commit response")
}

fn parse_commit_data(status: StatusCode, body: &[u8]) -> Result<LogCommit> {
    #[derive(Deserialize)]
    struct Envelope {
        data: LogCommit,
    }
    Ok(parse_api_response::<Envelope>(status, body, "invalid dev-null.sh commit response")?.data)
}

fn parse_pasteis_response(status: StatusCode, body: &[u8]) -> Result<PasteisResult> {
    if !status.is_success() {
        return Err(api_error(
            status,
            &serde_json::from_slice(body).unwrap_or(Value::Null),
        ));
    }
    anyhow::ensure!(
        status == StatusCode::OK,
        "expected 200 for pastéis, got {status}"
    );
    parse_api_response(status, body, "invalid dev-null.sh pastéis response")
}

fn parse_created_commit(status: StatusCode, body: &[u8]) -> Result<LogCommit> {
    if !status.is_success() {
        return Err(api_error(
            status,
            &serde_json::from_slice(body).unwrap_or(Value::Null),
        ));
    }
    anyhow::ensure!(
        status == StatusCode::CREATED,
        "expected 201 for created commit, got {status}"
    );
    parse_commit_data(status, body)
}

fn parse_report_response(status: StatusCode, body: &[u8]) -> Result<()> {
    if !status.is_success() {
        return Err(api_error(
            status,
            &serde_json::from_slice(body).unwrap_or(Value::Null),
        ));
    }
    anyhow::ensure!(
        status == StatusCode::NO_CONTENT && body.is_empty(),
        "expected empty 204 for report, got {status}"
    );
    Ok(())
}

fn parse_api_response<T: serde::de::DeserializeOwned>(
    status: StatusCode,
    body: &[u8],
    context: &'static str,
) -> Result<T> {
    if !status.is_success() {
        let payload = serde_json::from_slice(body).unwrap_or(Value::Null);
        return Err(api_error(status, &payload));
    }
    serde_json::from_slice(body).context(context)
}

fn parse_log_response(status: StatusCode, body: &[u8]) -> Result<LogPage> {
    let page: LogPage = parse_api_response(status, body, "invalid dev-null.sh log response")?;
    anyhow::ensure!(
        page.data
            .iter()
            .chain(&page.suggestions)
            .all(|card| card.reply_preview.is_some()),
        "invalid dev-null.sh log response: missing reply_preview"
    );
    Ok(page)
}

fn parse_current_user(payload: &serde_json::Value) -> Result<CurrentUser> {
    Ok(CurrentUser {
        username: payload["data"]["username"]
            .as_str()
            .context("API response did not include the current username")?
            .to_owned(),
        email: payload["data"]["email"].as_str().map(str::to_owned),
    })
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
    use reqwest::{Method, StatusCode};
    use url::Url;

    use serde_json::json;

    use super::{
        canonical_message, commit_url, create_commit_body, log_url, parse_commit_data,
        parse_commit_response, parse_created_commit, parse_current_user, parse_log_response,
        parse_pasteis_response, parse_report_response, pasteis_url, report_url, CommitStatus,
        CommitTarget, LogKind, ReportReason,
    };

    fn log_card() -> serde_json::Value {
        json!({
            "id": "d647b518-55f2-4a26-9283-19b8c5f36d84",
            "short_hash": "5f36d84",
            "body": "Hello world",
            "parent_id": null,
            "html_url": "https://dev-null.sh/@alice/commits/d647b518-55f2-4a26-9283-19b8c5f36d84",
            "created_at": "2026-02-17T12:00:00.000000Z",
            "status": "approved",
            "deleted_at": null,
            "author": {
                "id": "user-id", "username": "alice", "name": "Alice", "avatar_url": null,
                "bio": null, "company": null, "location": null, "website": null,
                "github_url": "https://github.com/alice", "html_url": "https://dev-null.sh/@alice",
                "feed_url": "https://dev-null.sh/@alice/feed", "created_at": "2026-01-01T00:00:00Z",
                "lane": 1
            },
            "stars_count": 3, "replies_count": 2, "pasteis_count": 1,
            "starred": true, "is_pinned": false,
            "body_segments": [{"type": "text", "text": "Hello world"}],
            "preview_segments": null,
            "mentions": [], "link_preview": null, "images": [],
            "quoted_commit_id": null, "quoted_commit": null, "poll": null,
            "reply_preview": {
                "data": [{"id": "deleted-reply", "parent_id": "d647b518-55f2-4a26-9283-19b8c5f36d84", "deleted_at": "2026-02-18T00:00:00Z"}],
                "next_cursor": null
            }
        })
    }

    fn log_page() -> serde_json::Value {
        json!({
            "data": [log_card()], "log": "latest", "is_ranked": false,
            "suggestions": [],
            "links": {"first": "https://dev-null.sh/api/v1/log/latest", "last": null,
                      "prev": null, "next": "https://dev-null.sh/api/v1/log/latest?cursor=opaque"},
            "meta": {"path": "https://dev-null.sh/api/v1/log/latest", "per_page": 20,
                     "next_cursor": "opaque", "prev_cursor": null}
        })
    }

    #[test]
    fn parses_upstream_log_envelope_and_nested_tombstone() {
        let mut fixture = log_page();
        fixture["new_commits_count"] = json!(4);
        let page =
            parse_log_response(StatusCode::OK, &serde_json::to_vec(&fixture).unwrap()).unwrap();
        assert_eq!(page.log, LogKind::Latest);
        assert!(!page.is_ranked);
        assert_eq!(page.new_commits_count, Some(4));
        assert_eq!(page.meta.next_cursor.as_deref(), Some("opaque"));
        assert_eq!(page.data[0].author.as_ref().unwrap().username, "alice");
        assert!(matches!(page.data[0].status, CommitStatus::Approved));
        assert_eq!(
            page.data[0].reply_preview.as_ref().unwrap().data[0].body,
            None
        );
        assert_eq!(
            page.data[0].html_url,
            "https://dev-null.sh/@alice/commits/d647b518-55f2-4a26-9283-19b8c5f36d84"
        );

        fixture["log"] = json!("following");
        fixture["data"] = json!([]);
        fixture["suggestions"] = json!([log_card()]);
        fixture["meta"]["next_cursor"] = serde_json::Value::Null;
        assert_eq!(
            parse_log_response(StatusCode::OK, &serde_json::to_vec(&fixture).unwrap())
                .unwrap()
                .suggestions
                .len(),
            1
        );

        fixture["log"] = json!("trending");
        fixture["is_ranked"] = json!(true);
        assert_eq!(
            parse_log_response(StatusCode::OK, &serde_json::to_vec(&fixture).unwrap())
                .unwrap()
                .log,
            LogKind::Trending
        );
    }

    #[test]
    fn rejects_malformed_log_and_surfaces_api_errors() {
        let mut fixture = log_page();
        fixture["data"][0].as_object_mut().unwrap().remove("body");
        assert!(
            parse_log_response(StatusCode::OK, &serde_json::to_vec(&fixture).unwrap()).is_err()
        );
        assert!(parse_log_response(StatusCode::OK, b"not json").is_err());
        assert!(parse_log_response(
            StatusCode::UNAUTHORIZED,
            br#"{"code":"device_not_approved","message":"Not approved"}"#
        )
        .unwrap_err()
        .to_string()
        .contains("device_not_approved"));
        assert!(parse_log_response(
            StatusCode::UNPROCESSABLE_ENTITY,
            br#"{"message":"The cursor is invalid.","errors":{"cursor":["Invalid"]}}"#
        )
        .unwrap_err()
        .to_string()
        .contains("The cursor is invalid."));
    }

    #[test]
    fn signs_encoded_cursor_in_request_path() {
        let origin = Url::parse("https://dev-null.sh/").unwrap();
        let url = log_url(&origin, LogKind::Following, Some("a+b/=?")).unwrap();
        assert_eq!(
            url.query_pairs()
                .find(|(key, _)| key == "cursor")
                .unwrap()
                .1,
            "a+b/=?"
        );
        let message = canonical_message("123", "nonce", &Method::GET, &url, b"");
        assert_eq!(
            message,
            b"123\nnonce\nGET\n/api/v1/log/following?cursor=a%2Bb%2F%3D%3F\n"
        );
    }

    #[test]
    fn signed_commit_detail_decodes_ancestors_and_own_pending_state() {
        // CommitControllerTest: signed detail has data + ancestors, without reply_preview.
        let mut card = log_card();
        card.as_object_mut().unwrap().remove("reply_preview");
        card["status"] = json!("pending");
        card["starred"] = json!(false);
        card["stars_count"] = json!(0);
        let parent = "019a2b3c-4d5e-7f60-8a9b-0c1d2e3f4a5b";
        let fixture = json!({
            "data": card,
            "ancestors": [
                {"id": parent, "parent_id": null, "deleted_at": null,
                 "body": "Parent", "replies_count": 1},
                {"id": "019a2b3c-4d5e-7f60-8a9b-0c1d2e3f4a5c", "parent_id": parent,
                 "deleted_at": "2026-02-18T00:00:00Z"}
            ]
        });
        let detail =
            parse_commit_response(StatusCode::OK, &serde_json::to_vec(&fixture).unwrap()).unwrap();
        assert!(matches!(detail.data.status, CommitStatus::Pending));
        assert!(detail.data.reply_preview.is_none());
        assert_eq!(detail.ancestors[0].body.as_deref(), Some("Parent"));
        assert_eq!(detail.ancestors[1].body, None);
        assert_eq!(
            detail.ancestors[1].deleted_at.as_deref(),
            Some("2026-02-18T00:00:00Z")
        );

        let mut pending_poll = fixture;
        pending_poll["data"]["poll"] = json!({
            "id": "poll-id", "closes_at": null, "is_closed": false, "has_voted": false,
            "total_votes": null, "options": [{"id": "option-id", "label": "Yes",
                "votes_count": null, "voted": false, "percentage": null}]
        });
        assert!(
            parse_commit_response(StatusCode::OK, &serde_json::to_vec(&pending_poll).unwrap())
                .unwrap()
                .data
                .poll
                .unwrap()
                .closes_at
                .is_none()
        );
    }

    #[test]
    fn star_and_unstar_decode_refreshed_state_without_preview() {
        // CommitStarControllerTest: both verbs return the commit page's data, no ancestors.
        let mut card = log_card();
        card.as_object_mut().unwrap().remove("reply_preview");
        for (starred, count) in [(true, 2), (false, 1)] {
            card["starred"] = json!(starred);
            card["stars_count"] = json!(count);
            let fixture = json!({"data": card});
            let updated =
                parse_commit_data(StatusCode::OK, &serde_json::to_vec(&fixture).unwrap()).unwrap();
            assert_eq!(updated.starred, starred);
            assert_eq!(updated.stars_count, count);
            assert!(updated.reply_preview.is_none());
        }
    }

    #[test]
    fn commit_paths_and_empty_write_signatures_match_upstream() {
        let origin = Url::parse("https://dev-null.sh/").unwrap();
        let id = "d647b518-55f2-4a26-9283-19b8c5f36d84";
        let detail = commit_url(&origin, id, false).unwrap();
        assert_eq!(detail.path(), format!("/api/v1/commits/{id}"));
        assert_eq!(
            canonical_message("123", "nonce", &Method::GET, &detail, b""),
            format!("123\nnonce\nGET\n/api/v1/commits/{id}\n").as_bytes()
        );
        assert!(commit_url(&origin, "../log/latest", false).is_err());
        let star = commit_url(&origin, id, true).unwrap();
        for method in [Method::POST, Method::DELETE] {
            assert_eq!(
                canonical_message("123", "nonce", &method, &star, b""),
                format!("123\nnonce\n{method}\n/api/v1/commits/{id}/star\n").as_bytes()
            );
        }
    }

    #[test]
    fn pasteis_path_and_empty_post_signature_match_upstream() {
        let origin = Url::parse("https://dev-null.sh/").unwrap();
        let id = "d647b518-55f2-4a26-9283-19b8c5f36d84";
        let url = pasteis_url(&origin, id).unwrap();
        assert_eq!(url.path(), format!("/api/v1/commits/{id}/pasteis"));
        assert_eq!(
            canonical_message("123", "nonce", &Method::POST, &url, b""),
            format!("123\nnonce\nPOST\n/api/v1/commits/{id}/pasteis\n").as_bytes()
        );
        assert!(pasteis_url(&origin, "../pasteis").is_err());
        assert!(pasteis_url(&origin, "not-a-uuid").is_err());
    }

    #[test]
    fn pasteis_decodes_updated_commit_and_quota_response() {
        let mut card = log_card();
        card.as_object_mut().unwrap().remove("reply_preview");
        card["pasteis_count"] = json!(2);
        let fixture = json!({
            "data": card,
            "pasteis_remaining": 9,
            "pasteis_resets_at": "2026-02-18T00:00:00Z",
            "author_pasteis_received": 7
        });
        let result =
            parse_pasteis_response(StatusCode::OK, &serde_json::to_vec(&fixture).unwrap()).unwrap();
        assert_eq!(result.data.pasteis_count, 2);
        assert_eq!(result.pasteis_remaining, 9);
        assert_eq!(result.pasteis_resets_at, "2026-02-18T00:00:00Z");
        assert_eq!(result.author_pasteis_received, Some(7));

        let mut no_received_count = fixture;
        no_received_count["author_pasteis_received"] = serde_json::Value::Null;
        assert_eq!(
            parse_pasteis_response(
                StatusCode::OK,
                &serde_json::to_vec(&no_received_count).unwrap()
            )
            .unwrap()
            .author_pasteis_received,
            None
        );

        assert!(parse_pasteis_response(StatusCode::OK, b"").is_err());
        assert!(parse_pasteis_response(StatusCode::OK, br#"{"data":null}"#).is_err());
        assert!(parse_pasteis_response(StatusCode::CREATED, b"").is_err());
        for (status, body, error) in [
            (
                StatusCode::FORBIDDEN,
                br#"{"code":"own_commit","message":"You cannot send pasteis to your own commit."}"#
                    .as_slice(),
                "own_commit",
            ),
            (
                StatusCode::UNPROCESSABLE_ENTITY,
                br#"{"code":"pasteis_exhausted","message":"You have no pasteis remaining today."}"#
                    .as_slice(),
                "pasteis_exhausted",
            ),
            (
                StatusCode::TOO_MANY_REQUESTS,
                br#"{"code":"rate_limited","message":"Slow down"}"#.as_slice(),
                "rate_limited",
            ),
        ] {
            let error_message = parse_pasteis_response(status, body)
                .unwrap_err()
                .to_string();
            assert!(error_message.contains(error), "{error_message}");
        }
    }

    #[test]
    fn detail_and_star_fail_closed_on_missing_data_and_authorization() {
        assert!(parse_commit_response(StatusCode::OK, br#"{"data":{}}"#).is_err());
        assert!(parse_commit_response(StatusCode::OK, br#"{"data":null,"ancestors":[]}"#).is_err());
        assert!(parse_commit_data(StatusCode::OK, br#"{"data":{}}"#).is_err());
        assert!(parse_commit_data(StatusCode::OK, br#"{"data":null}"#).is_err());
        let mut card = log_card();
        card.as_object_mut().unwrap().remove("reply_preview");
        let page = json!({"data": [card], "log": "latest", "is_ranked": false,
            "suggestions": [], "links": {"first": null, "last": null, "prev": null,
            "next": null}, "meta": {"path": "/api/v1/log/latest", "per_page": 20,
            "next_cursor": null, "prev_cursor": null}});
        assert!(parse_log_response(StatusCode::OK, &serde_json::to_vec(&page).unwrap()).is_err());
        assert!(parse_commit_response(
            StatusCode::NOT_FOUND,
            br#"{"message":"Not Found","code":"commit_unavailable"}"#,
        )
        .unwrap_err()
        .to_string()
        .contains("commit_unavailable"));
        assert!(parse_commit_data(
            StatusCode::FORBIDDEN,
            br#"{"message":"This action is unauthorized."}"#,
        )
        .unwrap_err()
        .to_string()
        .contains("403"));
    }

    #[test]
    fn reply_and_quote_sign_distinct_json_payloads() {
        let origin = Url::parse("https://dev-null.sh/").unwrap();
        let target = "d647b518-55f2-4a26-9283-19b8c5f36d84";
        let url = origin.join("api/v1/commits").unwrap();
        let reply = create_commit_body("reply", CommitTarget::Parent(target)).unwrap();
        let quote = create_commit_body("quote", CommitTarget::Quote(target)).unwrap();
        assert_eq!(
            serde_json::from_slice::<serde_json::Value>(&reply).unwrap(),
            json!({"body": "reply", "parent_id": target})
        );
        assert_eq!(
            serde_json::from_slice::<serde_json::Value>(&quote).unwrap(),
            json!({"body": "quote", "quoted_commit_id": target})
        );
        for payload in [&reply, &quote] {
            let signature_input = canonical_message("123", "nonce", &Method::POST, &url, payload);
            assert!(signature_input.starts_with(b"123\nnonce\nPOST\n/api/v1/commits\n"));
            assert!(signature_input.ends_with(payload));
        }
        assert!(create_commit_body("reply", CommitTarget::Parent("../reports")).is_err());
        assert!(create_commit_body("quote", CommitTarget::Quote("not-a-uuid")).is_err());
    }

    #[test]
    fn created_commit_requires_201_and_complete_data() {
        // CommitControllerTest: posting creates a pending commit, not an empty success.
        let mut card = log_card();
        card.as_object_mut().unwrap().remove("reply_preview");
        card["status"] = json!("pending");
        card["parent_id"] = json!("019a2b3c-4d5e-7f60-8a9b-0c1d2e3f4a5b");
        let response = serde_json::to_vec(&json!({"data": card})).unwrap();
        let created = parse_created_commit(StatusCode::CREATED, &response).unwrap();
        assert!(matches!(created.status, CommitStatus::Pending));
        assert!(created.reply_preview.is_none());
        let mut quoted = log_card();
        quoted.as_object_mut().unwrap().remove("reply_preview");
        quoted["status"] = json!("pending");
        quoted["quoted_commit_id"] = json!("019a2b3c-4d5e-7f60-8a9b-0c1d2e3f4a5b");
        quoted["quoted_commit"] = json!({"id": "019a2b3c-4d5e-7f60-8a9b-0c1d2e3f4a5b",
            "parent_id": null, "deleted_at": null, "body": "Required reading."});
        let quote = parse_created_commit(
            StatusCode::CREATED,
            &serde_json::to_vec(&json!({"data": quoted})).unwrap(),
        )
        .unwrap();
        assert!(quote.parent_id.is_none());
        assert_eq!(
            quote.quoted_commit.unwrap().body.as_deref(),
            Some("Required reading.")
        );
        assert!(parse_created_commit(StatusCode::OK, &response).is_err());
        assert!(parse_created_commit(StatusCode::NO_CONTENT, b"").is_err());
        assert!(parse_created_commit(StatusCode::CREATED, br#"{"data":null}"#).is_err());
        assert!(parse_created_commit(StatusCode::CREATED, b"not json").is_err());
        for (status, body, error) in [
            (
                StatusCode::FORBIDDEN,
                br#"{"code":"commit_rules_required","message":"Accept rules"}"#.as_slice(),
                "commit_rules_required",
            ),
            (
                StatusCode::UNPROCESSABLE_ENTITY,
                br#"{"message":"The body field is required.","errors":{"body":["required"]}}"#
                    .as_slice(),
                "The body field is required.",
            ),
            (
                StatusCode::UNPROCESSABLE_ENTITY,
                br#"{"message":"That commit is no longer available to reply to.","errors":{"parent_id":["That commit is no longer available to reply to."]}}"#.as_slice(),
                "no longer available to reply",
            ),
            (
                StatusCode::UNPROCESSABLE_ENTITY,
                br#"{"message":"That commit is no longer available to quote.","errors":{"quoted_commit_id":["That commit is no longer available to quote."]}}"#.as_slice(),
                "no longer available to quote",
            ),
            (
                StatusCode::TOO_MANY_REQUESTS,
                br#"{"code":"rate_limited","message":"Slow down"}"#.as_slice(),
                "rate_limited",
            ),
        ] {
            assert!(parse_created_commit(status, body)
                .unwrap_err()
                .to_string()
                .contains(error));
        }
    }

    #[test]
    fn report_requires_supported_reason_and_empty_204() {
        let origin = Url::parse("https://dev-null.sh/").unwrap();
        let id = "d647b518-55f2-4a26-9283-19b8c5f36d84";
        let url = report_url(&origin, id).unwrap();
        assert_eq!(url.path(), format!("/api/v1/commits/{id}/reports"));
        assert!(report_url(&origin, "../reports").is_err());
        for (reason, label) in [
            (ReportReason::Spam, "spam"),
            (ReportReason::Harassment, "harassment"),
            (ReportReason::Unsafe, "unsafe"),
            (ReportReason::Other, "other"),
        ] {
            let body = serde_json::to_vec(&json!({"reason": reason, "details": null})).unwrap();
            assert_eq!(
                serde_json::from_slice::<serde_json::Value>(&body).unwrap(),
                json!({"reason": label, "details": null})
            );
            assert!(canonical_message("123", "nonce", &Method::POST, &url, &body).ends_with(&body));
        }
        assert_eq!(
            json!({"reason": ReportReason::Other, "details": "context"})["details"],
            "context"
        );
        assert!(parse_report_response(StatusCode::NO_CONTENT, b"").is_ok());
        assert!(parse_report_response(StatusCode::NO_CONTENT, b"unexpected").is_err());
        assert!(parse_report_response(StatusCode::OK, b"").is_err());
        assert!(parse_report_response(StatusCode::CREATED, b"").is_err());
        for (status, body, error) in [
            (
                StatusCode::FORBIDDEN,
                br#"{"code":"own_commit","message":"You cannot report your own commit."}"#
                    .as_slice(),
                "own_commit",
            ),
            (
                StatusCode::NOT_FOUND,
                br#"{"code":"commit_unavailable","message":"This commit isn't available."}"#
                    .as_slice(),
                "commit_unavailable",
            ),
            (
                StatusCode::UNPROCESSABLE_ENTITY,
                br#"{"message":"The selected reason is invalid.","errors":{"reason":["invalid"]}}"#
                    .as_slice(),
                "The selected reason is invalid.",
            ),
            (
                StatusCode::TOO_MANY_REQUESTS,
                br#"{"code":"rate_limited","message":"Slow down"}"#.as_slice(),
                "rate_limited",
            ),
        ] {
            assert!(parse_report_response(status, body)
                .unwrap_err()
                .to_string()
                .contains(error));
        }
    }

    #[test]
    fn parses_current_user_username_and_email() {
        let user = parse_current_user(&json!({
            "data": { "username": "octocat", "email": "octo@example.test" }
        }))
        .unwrap();
        assert_eq!(user.username, "octocat");
        assert_eq!(user.email.as_deref(), Some("octo@example.test"));
    }

    #[test]
    fn parses_current_user_without_optional_email() {
        let user = parse_current_user(&json!({"data": {"username": "octocat"}})).unwrap();
        assert_eq!(user.email, None);
    }

    #[test]
    fn signature_message_matches_server_contract() {
        let url = Url::parse("https://dev-null.sh/api/v1/devices?x=1").unwrap();
        let message = canonical_message("123", "0123", &Method::POST, &url, b"{}");
        assert_eq!(message, b"123\n0123\nPOST\n/api/v1/devices?x=1\n{}");
    }
}
