use std::collections::BTreeMap;
use std::fmt;
use std::time::Duration;

use serde::de::DeserializeOwned;
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use ureq::http::{Response, StatusCode};
use ureq::{Body, RequestBuilder};

use crate::clock::epoch_from_rfc3339;
use crate::error::{ApiError, CloudError, Result};

/// Where the CLI talks to when nothing says otherwise.
pub const DEFAULT_BASE_URL: &str = "https://penv.cloud";
/// The variable that points the CLI somewhere else.
pub const URL_VAR: &str = "PENV_URL";

/// The two headers the audit row is stamped from.
pub const AGENT_HEADER: &str = "X-Penv-Agent";
pub const SESSION_HEADER: &str = "X-Penv-Session";

const TIMEOUT: Duration = Duration::from_secs(30);
/// A server error is tried once more, this long after the first one.
const RETRY_PAUSE: Duration = Duration::from_secs(1);

/// One path segment, percent-encoded to RFC 3986's unreserved set. Slugs and
/// environment names are free-form, so nothing in one may reach the router.
pub fn encode_segment(segment: &str) -> String {
    let mut out = String::with_capacity(segment.len());
    for byte in segment.as_bytes() {
        match byte {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'.' | b'_' | b'~' => {
                out.push(*byte as char)
            }
            other => out.push_str(&format!("%{other:02X}")),
        }
    }
    out
}

/// The same JSON without its null members: the server reads a null as a value,
/// and the contract asks for absent fields to be absent.
pub fn without_nulls(value: Value) -> Value {
    match value {
        Value::Object(map) => Value::Object(
            map.into_iter()
                .filter(|(_, v)| !v.is_null())
                .map(|(k, v)| (k, without_nulls(v)))
                .collect(),
        ),
        other => other,
    }
}

/// The machine's name, for the approval page. No crate reads a hostname, so the
/// platform's own variable answers, and an unnamed host is just the CLI.
pub fn host_name() -> String {
    for var in ["COMPUTERNAME", "HOSTNAME", "HOST"] {
        if let Some(name) = std::env::var_os(var)
            && !name.is_empty()
        {
            return name.to_string_lossy().into_owned();
        }
    }
    std::fs::read_to_string("/etc/hostname")
        .ok()
        .map(|name| name.trim().to_string())
        .filter(|name| !name.is_empty())
        .unwrap_or_else(|| "penv CLI".to_string())
}

/// A bearer credential. Never printed: `Debug` says only that it exists.
#[derive(Clone, PartialEq, Eq)]
pub struct Bearer {
    pub token: String,
    pub expires_at: Option<u64>,
}

impl Bearer {
    pub fn new(token: impl Into<String>) -> Bearer {
        Bearer {
            token: token.into(),
            expires_at: None,
        }
    }

    pub fn until(token: impl Into<String>, expires_at: u64) -> Bearer {
        Bearer {
            token: token.into(),
            expires_at: Some(expires_at),
        }
    }

    /// The prefix says which principal it is; the rest never leaves this struct.
    pub fn principal(&self) -> &'static str {
        if self.token.starts_with("pcu_") {
            "user"
        } else {
            "machine"
        }
    }
}

impl fmt::Debug for Bearer {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Bearer")
            .field("principal", &self.principal())
            .field("expires_at", &self.expires_at)
            .finish()
    }
}

/// `{org}/{project}/{environment}`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Address {
    pub org: String,
    pub project: String,
    pub environment: String,
}

impl Address {
    pub fn new(org: &str, project: &str, environment: &str) -> Address {
        Address {
            org: org.to_string(),
            project: project.to_string(),
            environment: environment.to_string(),
        }
    }

    /// The three segments as they go into a URL. [`Display`] stays the address a
    /// person reads.
    pub fn path(&self) -> String {
        format!(
            "{}/{}/{}",
            encode_segment(&self.org),
            encode_segment(&self.project),
            encode_segment(&self.environment)
        )
    }
}

impl fmt::Display for Address {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}/{}/{}", self.org, self.project, self.environment)
    }
}

/// A time the server sent: the ISO-8601 string the contract promises, or a
/// number of seconds where one is sent instead.
#[derive(Debug, Clone, PartialEq, Deserialize)]
#[serde(untagged)]
pub enum Stamp {
    Epoch(u64),
    Text(String),
}

impl Stamp {
    pub fn epoch(&self) -> Option<u64> {
        match self {
            Stamp::Epoch(n) => Some(*n),
            Stamp::Text(text) => epoch_from_rfc3339(text),
        }
    }
}

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct DeviceStart {
    pub device_code: String,
    pub user_code: String,
    pub verification_uri: String,
    pub expires_in: u64,
    pub interval: u64,
}

/// What the device token route said this time round. A 429 carries the seconds
/// the server asked for, and is never an error: polling only slows down.
#[derive(Debug, Clone)]
pub enum DevicePoll {
    Pending,
    SlowDown(Option<u64>),
    Expired,
    Denied,
    Granted(Grant),
}

#[derive(Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Grant {
    pub credential: String,
    #[serde(default)]
    pub expires_at: Option<Stamp>,
    #[serde(default)]
    pub user: Option<User>,
    #[serde(default)]
    pub orgs: Vec<Org>,
}

impl fmt::Debug for Grant {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Grant")
            .field("user", &self.user)
            .field("orgs", &self.orgs)
            .finish_non_exhaustive()
    }
}

/// A person's account. The email may be null, so a login says so rather than
/// failing to read the answer.
#[derive(Debug, Clone, Deserialize, PartialEq, Eq)]
pub struct User {
    #[serde(default)]
    pub email: Option<String>,
}

#[derive(Debug, Clone, Deserialize, PartialEq, Eq)]
pub struct Org {
    pub slug: String,
    pub name: String,
}

#[derive(Debug, Clone, Deserialize, PartialEq, Eq)]
pub struct Project {
    pub slug: String,
    #[serde(default)]
    pub name: String,
    #[serde(default)]
    pub environments: Vec<String>,
}

/// What a delete erased, so the report can say how much is gone.
#[derive(Debug, Clone, Deserialize, PartialEq, Eq)]
pub struct Deleted {
    pub name: String,
    #[serde(default)]
    pub environments: u64,
    #[serde(default)]
    pub parameters: u64,
}

#[derive(Debug, Clone, Deserialize, PartialEq, Eq)]
pub struct CreatedEnvironment {
    pub name: String,
    #[serde(default)]
    pub copied: u64,
}

/// One parameter as the cloud holds it. `kind` and `version` are the server's
/// own, so a write sends only what [`CloudKey::to_write`] carries.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct CloudKey {
    #[serde(default)]
    pub path: String,
    pub name: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub kind: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub version: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub schema: Option<Value>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub value: Option<String>,
    /// When the value was last written. `@rotate` counts from it.
    #[serde(default, rename = "updatedAt", skip_serializing_if = "Option::is_none")]
    pub updated_at: Option<String>,
}

impl CloudKey {
    /// What a write sends: the address, the schema and the value, never the
    /// version or the kind the server owns. An absent schema field is absent,
    /// not null.
    pub fn to_write(&self) -> Value {
        let mut out = serde_json::Map::new();
        out.insert("path".into(), Value::String(self.path.clone()));
        out.insert("name".into(), Value::String(self.name.clone()));
        if let Some(schema) = &self.schema {
            out.insert("schema".into(), without_nulls(schema.clone()));
        }
        if let Some(value) = &self.value {
            out.insert("value".into(), Value::String(value.clone()));
        }
        Value::Object(out)
    }

    /// `path/name`, the address the server skips and prunes by.
    pub fn address(&self) -> String {
        if self.path.is_empty() {
            self.name.clone()
        } else {
            format!("{}/{}", self.path, self.name)
        }
    }

    /// The same address as URL segments, each one encoded on its own.
    pub fn key_path(&self) -> String {
        let mut segments: Vec<String> = self
            .path
            .split('/')
            .filter(|segment| !segment.is_empty())
            .map(encode_segment)
            .collect();
        segments.push(encode_segment(&self.name));
        segments.join("/")
    }
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct EnvBody {
    #[serde(default)]
    pub keys: Vec<CloudKey>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub skipped: Vec<String>,
}

/// What a conditional GET came back with.
#[derive(Debug, Clone)]
pub enum Fetched {
    NotModified,
    Body { etag: Option<String>, body: EnvBody },
}

/// What a conditional HEAD came back with.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Freshness {
    Unchanged,
    Changed(Option<String>),
}

#[derive(Debug, Clone, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct PutResult {
    #[serde(default)]
    pub written: u64,
    #[serde(default)]
    pub unchanged: u64,
    #[serde(default)]
    pub pruned: u64,
    #[serde(default)]
    pub etag: String,
}

#[derive(Debug, Clone, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct SetResult {
    #[serde(default)]
    pub version: u64,
    #[serde(default)]
    pub etag: String,
}

#[derive(Debug, Clone, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct UnsetResult {
    #[serde(default)]
    pub etag: String,
}

#[derive(Debug, Clone, Deserialize, PartialEq, Eq)]
pub struct Challenge {
    pub nonce: String,
}

/// One console approval for one `reveal`. `expiresAt` is passed back to the
/// caller as the server sent it.
#[derive(Debug, Clone, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct Approval {
    pub id: String,
    #[serde(default)]
    pub url: String,
    #[serde(default)]
    pub status: Option<String>,
    #[serde(default)]
    pub key: Option<String>,
    #[serde(default)]
    pub expires_at: Option<Value>,
}

/// What asking for an approval answered: a new request, or the one already open
/// for this key and session.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Requested {
    Created(Approval),
    Pending(Approval),
}

/// The one value a redeemed approval carries. `Debug` names the key only.
#[derive(Clone, Deserialize, PartialEq, Eq)]
pub struct Revealed {
    pub key: String,
    pub value: String,
}

impl fmt::Debug for Revealed {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Revealed")
            .field("key", &self.key)
            .finish_non_exhaustive()
    }
}

/// A keypair exchange: the credential, and the counter that has to be on disk
/// before the credential is used.
#[derive(Debug, Clone)]
pub struct KeypairGrant {
    pub bearer: Bearer,
    pub generation: u64,
}

#[derive(Debug, Clone, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct Enrolment {
    pub credential_id: String,
    #[serde(default = "one")]
    pub generation: u64,
}

fn one() -> u64 {
    1
}

/// An STS request already signed, handed to the server to replay.
#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct SignedRequest {
    pub method: String,
    pub url: String,
    pub body: String,
    pub headers: BTreeMap<String, String>,
}

/// The HTTP surface of penv.cloud. One method per route in docs/Cloud-API.md.
pub struct Api {
    http: ureq::Agent,
    base_url: String,
    agent_name: Option<String>,
    session_id: Option<String>,
}

impl Api {
    pub fn new(base_url: &str) -> Result<Api> {
        let config = ureq::Agent::config_builder()
            .max_redirects(0)
            .max_redirects_will_error(true)
            .http_status_as_error(false)
            .user_agent(format!("penv/{}", env!("CARGO_PKG_VERSION")))
            .timeout_global(Some(TIMEOUT))
            .build();
        Ok(Api {
            http: ureq::Agent::new_with_config(config),
            base_url: checked_base_url(base_url)?,
            agent_name: None,
            session_id: None,
        })
    }

    /// `PENV_URL`, else penv.cloud.
    pub fn from_env(env: &BTreeMap<String, String>) -> Result<Api> {
        let raw = env
            .get(URL_VAR)
            .map(String::as_str)
            .filter(|v| !v.is_empty())
            .unwrap_or(DEFAULT_BASE_URL);
        Api::new(raw)
    }

    /// The agent name and session id every request carries for the audit row.
    pub fn stamped(mut self, agent_name: Option<&str>, session_id: Option<&str>) -> Api {
        self.agent_name = agent_name.filter(|v| !v.is_empty()).map(str::to_string);
        self.session_id = session_id.filter(|v| !v.is_empty()).map(str::to_string);
        self
    }

    pub fn base_url(&self) -> &str {
        &self.base_url
    }

    fn url(&self, path: &str) -> String {
        format!("{}/api/v1{path}", self.base_url)
    }

    fn stamp<A>(&self, mut request: RequestBuilder<A>) -> RequestBuilder<A> {
        if let Some(name) = &self.agent_name {
            request = request.header(AGENT_HEADER, name);
        }
        if let Some(id) = &self.session_id {
            request = request.header(SESSION_HEADER, id);
        }
        request
    }

    fn authed<A>(&self, request: RequestBuilder<A>, bearer: &Bearer) -> RequestBuilder<A> {
        self.stamp(request)
            .header("Authorization", format!("Bearer {}", bearer.token))
    }

    /// One request, and one more a second later when the server itself failed.
    fn attempt(
        &self,
        url: &str,
        send_once: impl Fn() -> std::result::Result<Response<Body>, ureq::Error>,
    ) -> Result<Response<Body>> {
        let response = send(url, send_once())?;
        if response.status().as_u16() < 500 {
            return Ok(response);
        }
        std::thread::sleep(RETRY_PAUSE);
        send(url, send_once())
    }

    // --- device-code login ---------------------------------------------------

    /// `device` is what the approval page shows the person: this machine's name.
    pub fn device_start(&self, device: &str) -> Result<DeviceStart> {
        let url = self.url("/auth/device");
        let mut response = self.attempt(&url, || {
            self.stamp(self.http.post(&url))
                .send_json(json!({ "device": device }))
        })?;
        expect(&mut response, &[StatusCode::CREATED, StatusCode::OK])?;
        read_json(&url, &mut response)
    }

    pub fn device_poll(&self, device_code: &str) -> Result<DevicePoll> {
        let url = self.url("/auth/device/token");
        let mut response = self.attempt(&url, || {
            self.stamp(self.http.post(&url))
                .send_json(json!({ "deviceCode": device_code }))
        })?;
        let status = response.status().as_u16();
        if (200..300).contains(&status) {
            return Ok(DevicePoll::Granted(read_json(&url, &mut response)?));
        }
        let error = refusal(status, &mut response);
        // Every 429 is the ceiling talking, whichever code it carries.
        if status == 429 {
            return Ok(DevicePoll::SlowDown(error.retry_after));
        }
        Ok(match error.code.as_str() {
            "authorization_pending" => DevicePoll::Pending,
            "expired" => DevicePoll::Expired,
            "denied" => DevicePoll::Denied,
            _ => return Err(error.into()),
        })
    }

    pub fn revoke(&self, bearer: &Bearer) -> Result<()> {
        let url = self.url("/auth/revoke");
        let mut response = self.attempt(&url, || {
            self.authed(self.http.post(&url), bearer).send_empty()
        })?;
        expect(&mut response, &[StatusCode::OK, StatusCode::NO_CONTENT])?;
        Ok(())
    }

    // --- environments --------------------------------------------------------

    pub fn env_head(&self, bearer: &Bearer, at: &Address, etag: Option<&str>) -> Result<Freshness> {
        let url = self.url(&format!("/envs/{}", at.path()));
        let mut response = self.attempt(&url, || {
            let mut request = self.authed(self.http.head(&url), bearer);
            if let Some(etag) = etag {
                request = request.header("If-None-Match", etag);
            }
            request.call()
        })?;
        match response.status().as_u16() {
            304 => Ok(Freshness::Unchanged),
            200 => Ok(Freshness::Changed(etag_of(&response))),
            status => Err(refusal(status, &mut response).into()),
        }
    }

    pub fn env_get(
        &self,
        bearer: &Bearer,
        at: &Address,
        etag: Option<&str>,
        values: bool,
    ) -> Result<Fetched> {
        let url = self.url(&format!("/envs/{}", at.path()));
        let mut response = self.attempt(&url, || {
            let mut request = self.authed(self.http.get(&url), bearer);
            if !values {
                request = request.query("values", "false");
            }
            if let Some(etag) = etag {
                request = request.header("If-None-Match", etag);
            }
            request.call()
        })?;
        match response.status().as_u16() {
            304 => Ok(Fetched::NotModified),
            200 => Ok(Fetched::Body {
                etag: etag_of(&response),
                body: read_json(&url, &mut response)?,
            }),
            status => Err(refusal(status, &mut response).into()),
        }
    }

    pub fn env_put(
        &self,
        bearer: &Bearer,
        at: &Address,
        keys: &[CloudKey],
        prune: bool,
    ) -> Result<PutResult> {
        let url = self.url(&format!("/envs/{}", at.path()));
        let body = json!({
            "keys": keys.iter().map(CloudKey::to_write).collect::<Vec<_>>(),
            "prune": prune,
        });
        let mut response = self.attempt(&url, || {
            self.authed(self.http.put(&url), bearer).send_json(&body)
        })?;
        expect(&mut response, &[StatusCode::OK])?;
        read_json(&url, &mut response)
    }

    pub fn key_set(&self, bearer: &Bearer, at: &Address, key: &CloudKey) -> Result<SetResult> {
        let url = self.url(&format!("/envs/{}/keys/{}", at.path(), key.key_path()));
        let mut body = key.to_write();
        if let Some(object) = body.as_object_mut() {
            object.remove("path");
            object.remove("name");
        }
        let mut response = self.attempt(&url, || {
            self.authed(self.http.patch(&url), bearer).send_json(&body)
        })?;
        expect(&mut response, &[StatusCode::OK])?;
        read_json(&url, &mut response)
    }

    /// The key's whole address, the same segments `key_set` writes to.
    pub fn key_unset(&self, bearer: &Bearer, at: &Address, key: &CloudKey) -> Result<UnsetResult> {
        let url = self.url(&format!("/envs/{}/keys/{}", at.path(), key.key_path()));
        let mut response =
            self.attempt(&url, || self.authed(self.http.delete(&url), bearer).call())?;
        expect(&mut response, &[StatusCode::OK])?;
        read_json(&url, &mut response)
    }

    // --- reveal approvals ----------------------------------------------------

    /// Ask a person to approve one reveal. `device` is what the console page
    /// shows; the harness and the session reach the audit row as the headers
    /// every request already carries.
    pub fn approval_create(
        &self,
        bearer: &Bearer,
        at: &Address,
        key: &str,
        device: &str,
    ) -> Result<Requested> {
        let url = self.url("/approvals");
        let body = json!({
            "org": at.org,
            "project": at.project,
            "environment": at.environment,
            "key": key,
            "device": device,
        });
        let mut response = self.attempt(&url, || {
            self.authed(self.http.post(&url), bearer).send_json(&body)
        })?;
        match response.status().as_u16() {
            200 | 201 => Ok(Requested::Created(read_json(&url, &mut response)?)),
            // A 409 carries the request already open for this key and session.
            409 => {
                let body: Value = read_json(&url, &mut response)?;
                let code = body
                    .get("error")
                    .and_then(Value::as_str)
                    .unwrap_or("conflict");
                if code != "approval_pending" {
                    return Err(ApiError::new(409, code).into());
                }
                serde_json::from_value(body)
                    .map(Requested::Pending)
                    .map_err(|_| CloudError::Unreadable {
                        url: url.clone(),
                        reason: "no approval id in the answer".into(),
                    })
            }
            status => Err(refusal(status, &mut response).into()),
        }
    }

    pub fn approval(&self, bearer: &Bearer, id: &str) -> Result<Approval> {
        let url = self.url(&format!("/approvals/{}", encode_segment(id)));
        let mut response =
            self.attempt(&url, || self.authed(self.http.get(&url), bearer).call())?;
        expect(&mut response, &[StatusCode::OK])?;
        read_json(&url, &mut response)
    }

    /// Redeem an approved request, once. A 409 answers with why not, and the
    /// caller turns that code into the exit code.
    pub fn approval_redeem(&self, bearer: &Bearer, id: &str) -> Result<Revealed> {
        let url = self.url(&format!("/approvals/{}/redeem", encode_segment(id)));
        let mut response = self.attempt(&url, || {
            self.authed(self.http.post(&url), bearer).send_empty()
        })?;
        expect(&mut response, &[StatusCode::OK])?;
        read_json(&url, &mut response)
    }

    // --- projects ------------------------------------------------------------

    pub fn orgs(&self, bearer: &Bearer) -> Result<Vec<Org>> {
        #[derive(Deserialize)]
        struct Body {
            #[serde(default)]
            orgs: Vec<Org>,
        }
        let url = self.url("/orgs");
        let mut response =
            self.attempt(&url, || self.authed(self.http.get(&url), bearer).call())?;
        expect(&mut response, &[StatusCode::OK])?;
        Ok(read_json::<Body>(&url, &mut response)?.orgs)
    }

    pub fn projects(&self, bearer: &Bearer, org: &str) -> Result<Vec<Project>> {
        #[derive(Deserialize)]
        struct Body {
            #[serde(default)]
            projects: Vec<Project>,
        }
        let url = self.url(&format!("/orgs/{}/projects", encode_segment(org)));
        let mut response =
            self.attempt(&url, || self.authed(self.http.get(&url), bearer).call())?;
        expect(&mut response, &[StatusCode::OK])?;
        Ok(read_json::<Body>(&url, &mut response)?.projects)
    }

    /// The slug the server derived is in the answer, and it is the only name the
    /// header may carry.
    pub fn create_project(
        &self,
        bearer: &Bearer,
        org: &str,
        name: &str,
        environments: &[String],
    ) -> Result<Project> {
        let url = self.url(&format!("/orgs/{}/projects", encode_segment(org)));
        let body = json!({ "name": name, "environments": environments });
        let mut response = self.attempt(&url, || {
            self.authed(self.http.post(&url), bearer).send_json(&body)
        })?;
        expect(&mut response, &[StatusCode::CREATED, StatusCode::OK])?;
        let answered: Value = read_json(&url, &mut response)?;
        let project = answered.get("project").unwrap_or(&answered);
        serde_json::from_value(project.clone()).map_err(|_| CloudError::Unreadable {
            url: url.clone(),
            reason: "no project slug in the answer".into(),
        })
    }

    fn project_url(&self, org: &str, project: &str) -> String {
        self.url(&format!(
            "/orgs/{}/projects/{}",
            encode_segment(org),
            encode_segment(project)
        ))
    }

    fn environment_url(&self, org: &str, project: &str, environment: &str) -> String {
        format!(
            "{}/environments/{}",
            self.project_url(org, project),
            encode_segment(environment)
        )
    }

    /// The answer carries the slug the new name became, which the header needs.
    pub fn rename_project(
        &self,
        bearer: &Bearer,
        org: &str,
        project: &str,
        name: &str,
    ) -> Result<Project> {
        let url = self.project_url(org, project);
        let body = json!({ "name": name });
        let mut response = self.attempt(&url, || {
            self.authed(self.http.patch(&url), bearer).send_json(&body)
        })?;
        expect(&mut response, &[StatusCode::OK])?;
        read_json(&url, &mut response)
    }

    pub fn delete_project(&self, bearer: &Bearer, org: &str, project: &str) -> Result<Deleted> {
        let url = self.project_url(org, project);
        let mut response =
            self.attempt(&url, || self.authed(self.http.delete(&url), bearer).call())?;
        expect(&mut response, &[StatusCode::OK])?;
        read_json(&url, &mut response)
    }

    /// `from` copies another environment's keys and settings, never its values.
    pub fn create_environment(
        &self,
        bearer: &Bearer,
        org: &str,
        project: &str,
        name: &str,
        from: Option<&str>,
    ) -> Result<CreatedEnvironment> {
        let url = format!("{}/environments", self.project_url(org, project));
        let body = without_nulls(json!({ "name": name, "from": from }));
        let mut response = self.attempt(&url, || {
            self.authed(self.http.post(&url), bearer).send_json(&body)
        })?;
        expect(&mut response, &[StatusCode::CREATED, StatusCode::OK])?;
        read_json(&url, &mut response)
    }

    pub fn rename_environment(
        &self,
        bearer: &Bearer,
        org: &str,
        project: &str,
        environment: &str,
        name: &str,
    ) -> Result<()> {
        let url = self.environment_url(org, project, environment);
        let body = json!({ "name": name });
        let mut response = self.attempt(&url, || {
            self.authed(self.http.patch(&url), bearer).send_json(&body)
        })?;
        expect(&mut response, &[StatusCode::OK])
    }

    pub fn delete_environment(
        &self,
        bearer: &Bearer,
        org: &str,
        project: &str,
        environment: &str,
    ) -> Result<Deleted> {
        let url = self.environment_url(org, project, environment);
        let mut response =
            self.attempt(&url, || self.authed(self.http.delete(&url), bearer).call())?;
        expect(&mut response, &[StatusCode::OK])?;
        read_json(&url, &mut response)
    }

    // --- credential exchanges ------------------------------------------------

    pub fn exchange_oidc(&self, token: &str, now: u64) -> Result<Bearer> {
        let url = self.url("/auth/oidc");
        let mut response = self.attempt(&url, || {
            self.stamp(self.http.post(&url))
                .send_json(json!({ "token": token }))
        })?;
        expect(&mut response, &[StatusCode::OK, StatusCode::CREATED])?;
        bearer_from(&url, &mut response, now)
    }

    pub fn exchange_aws(&self, signed: &SignedRequest, now: u64) -> Result<Bearer> {
        let url = self.url("/auth/aws");
        let mut response =
            self.attempt(&url, || self.stamp(self.http.post(&url)).send_json(signed))?;
        expect(&mut response, &[StatusCode::OK, StatusCode::CREATED])?;
        bearer_from(&url, &mut response, now)
    }

    pub fn keypair_challenge(&self, credential_id: &str) -> Result<Challenge> {
        let url = self.url("/auth/keypair/challenge");
        let mut response = self.attempt(&url, || {
            self.stamp(self.http.post(&url))
                .send_json(json!({ "credentialId": credential_id }))
        })?;
        expect(&mut response, &[StatusCode::OK, StatusCode::CREATED])?;
        read_json(&url, &mut response)
    }

    pub fn exchange_keypair(
        &self,
        credential_id: &str,
        nonce: &str,
        generation: u64,
        signature: &str,
        now: u64,
    ) -> Result<KeypairGrant> {
        let url = self.url("/auth/keypair");
        let body = json!({
            "credentialId": credential_id,
            "nonce": nonce,
            "generation": generation,
            "signature": signature,
        });
        let mut response =
            self.attempt(&url, || self.stamp(self.http.post(&url)).send_json(&body))?;
        expect(&mut response, &[StatusCode::OK, StatusCode::CREATED])?;
        let body: Value = read_json(&url, &mut response)?;
        Ok(KeypairGrant {
            generation: body
                .get("generation")
                .and_then(Value::as_u64)
                .unwrap_or(generation + 1),
            bearer: bearer_of(&url, &body, now)?,
        })
    }

    pub fn keypair_enroll(&self, secret: &str, public_key: &str) -> Result<Enrolment> {
        let url = self.url("/auth/keypair/enroll");
        let mut response = self.attempt(&url, || {
            self.stamp(self.http.post(&url))
                .send_json(json!({ "secret": secret, "publicKey": public_key }))
        })?;
        expect(&mut response, &[StatusCode::OK, StatusCode::CREATED])?;
        read_json(&url, &mut response)
    }

    /// The platform's own OIDC endpoint, which is not penv.cloud. GitHub Actions
    /// mints one token per audience and hands it over from this URL.
    pub fn platform_id_token(
        &self,
        url: &str,
        request_token: &str,
        audience: Option<&str>,
    ) -> Result<String> {
        #[derive(Deserialize)]
        struct Body {
            value: String,
        }
        let url = &checked_url(url)?;
        let mut response = self.attempt(url, || {
            let mut request = self
                .http
                .get(url)
                .header("Authorization", format!("Bearer {request_token}"))
                .header("Accept", "application/json");
            if let Some(audience) = audience {
                request = request.query("audience", audience);
            }
            request.call()
        })?;
        expect(&mut response, &[StatusCode::OK])?;
        Ok(read_json::<Body>(url, &mut response)?.value)
    }
}

/// The base URL, with its trailing slash off.
pub fn checked_base_url(raw: &str) -> Result<String> {
    checked_url(raw.trim_end_matches('/'))
}

/// https everywhere but the loopback the tests and a local server run on. Every
/// URL penv sends a credential to goes through here.
pub fn checked_url(raw: &str) -> Result<String> {
    let (scheme, rest) = raw
        .split_once("://")
        .ok_or_else(|| CloudError::Url(format!("{raw} is not an http or https URL")))?;
    let authority = rest.split(['/', '?', '#']).next().unwrap_or_default();
    // A userinfo half hides the real host behind an @, so no authority may carry one.
    if authority.contains('@') {
        return Err(CloudError::Url(format!(
            "{raw} carries a user in front of its host, and penv sends a credential to hosts only"
        )));
    }
    let host = host_of(authority);
    if host.is_empty() {
        return Err(CloudError::Url(format!("{raw} names no host")));
    }
    match scheme {
        "https" => Ok(raw.to_string()),
        "http" if matches!(host, "127.0.0.1" | "localhost" | "[::1]") => Ok(raw.to_string()),
        "http" => Err(CloudError::Url(format!(
            "{raw} is plain http, and a credential only travels over https"
        ))),
        other => Err(CloudError::Url(format!(
            "{other} is not a scheme penv speaks"
        ))),
    }
}

/// The host out of an authority, brackets and all for an IPv6 literal.
fn host_of(authority: &str) -> &str {
    match authority.strip_prefix('[') {
        Some(rest) => match rest.find(']') {
            Some(end) => &authority[..end + 2],
            None => "",
        },
        None => authority.split(':').next().unwrap_or_default(),
    }
}

fn send(
    url: &str,
    sent: std::result::Result<Response<Body>, ureq::Error>,
) -> Result<Response<Body>> {
    sent.map_err(|e| match e {
        ureq::Error::TooManyRedirects | ureq::Error::RedirectFailed => CloudError::Redirected {
            url: url.to_string(),
        },
        other => CloudError::Offline {
            url: url.to_string(),
            reason: other.to_string(),
        },
    })
}

fn expect(response: &mut Response<Body>, ok: &[StatusCode]) -> Result<()> {
    let status = response.status();
    if ok.contains(&status) {
        return Ok(());
    }
    Err(refusal(status.as_u16(), response).into())
}

/// The body's `error` code, the status, and how long the server asked us to wait.
fn refusal(status: u16, response: &mut Response<Body>) -> ApiError {
    let retry_after = response
        .headers()
        .get("retry-after")
        .and_then(|v| v.to_str().ok())
        .and_then(|v| v.trim().parse::<u64>().ok());
    let body: Option<Value> = response.body_mut().read_json().ok();
    let code = body
        .as_ref()
        .and_then(|b| b.get("error"))
        .and_then(Value::as_str)
        .unwrap_or(match status {
            401 => "unauthorized",
            403 => "forbidden",
            404 => "not_found",
            409 => "conflict",
            429 => "rate_limited",
            _ => "error",
        })
        .to_string();
    let retry_after = retry_after.or_else(|| {
        body.as_ref()
            .and_then(|b| b.get("retryAfter"))
            .and_then(Value::as_u64)
    });
    ApiError::new(status, code).after(retry_after)
}

fn read_json<T: DeserializeOwned>(url: &str, response: &mut Response<Body>) -> Result<T> {
    response
        .body_mut()
        .read_json()
        .map_err(|e| CloudError::Unreadable {
            url: url.to_string(),
            reason: e.to_string(),
        })
}

fn etag_of(response: &Response<Body>) -> Option<String> {
    response
        .headers()
        .get("etag")
        .and_then(|v| v.to_str().ok())
        .map(str::to_string)
}

fn bearer_from(url: &str, response: &mut Response<Body>, now: u64) -> Result<Bearer> {
    let body: Value = read_json(url, response)?;
    bearer_of(url, &body, now)
}

/// The exchanges answer with the same credential shape the device route uses.
fn bearer_of(url: &str, body: &Value, now: u64) -> Result<Bearer> {
    let token = body
        .get("credential")
        .or_else(|| body.get("token"))
        .and_then(Value::as_str)
        .ok_or_else(|| CloudError::Unreadable {
            url: url.to_string(),
            reason: "no credential in the answer".into(),
        })?;
    // The contract sends an ISO-8601 expiresAt; expiresIn is only a fallback.
    let expires_at = body
        .get("expiresAt")
        .and_then(|at| serde_json::from_value::<Stamp>(at.clone()).ok())
        .and_then(|stamp| stamp.epoch())
        .or_else(|| {
            body.get("expiresIn")
                .and_then(Value::as_u64)
                .map(|seconds| now + seconds)
        });
    Ok(Bearer {
        token: token.to_string(),
        expires_at,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn https_is_required_everywhere_but_the_loopback() {
        assert_eq!(
            checked_base_url("https://penv.cloud/").unwrap(),
            "https://penv.cloud"
        );
        assert_eq!(
            checked_base_url("http://127.0.0.1:8787").unwrap(),
            "http://127.0.0.1:8787"
        );
        assert!(checked_base_url("http://localhost:1234").is_ok());
        assert!(checked_base_url("http://[::1]:8787").is_ok());
        assert!(checked_base_url("http://penv.cloud").is_err());
        assert!(checked_base_url("ftp://penv.cloud").is_err());
        assert!(checked_base_url("penv.cloud").is_err());
    }

    #[test]
    fn a_host_that_only_looks_like_the_loopback_is_refused() {
        for raw in [
            "http://localhost@evil.example",
            "http://127.0.0.1@evil.example",
            "https://penv.cloud@evil.example",
            "http://localhost.evil.example",
            "http://127.0.0.1.evil.example",
            "http://evil.example#localhost",
            "http://evil.example/localhost",
            "https://",
        ] {
            assert!(checked_url(raw).is_err(), "{raw} was let through");
        }
    }

    #[test]
    fn every_segment_of_an_address_is_encoded() {
        let at = Address::new("acme corp", "api/gateway", "feature/new ui");
        assert_eq!(at.path(), "acme%20corp/api%2Fgateway/feature%2Fnew%20ui");
        assert_eq!(
            at.to_string(),
            "acme corp/api/gateway/feature/new ui",
            "what a person reads is not encoded"
        );

        let key = CloudKey {
            path: "services/web api".into(),
            name: "DB URL".into(),
            ..CloudKey::default()
        };
        assert_eq!(key.key_path(), "services/web%20api/DB%20URL");
        assert_eq!(encode_segment("a-b_c.d~e"), "a-b_c.d~e");
    }

    #[test]
    fn an_absent_schema_field_is_absent_and_never_null() {
        let key = CloudKey {
            name: "PORT".into(),
            schema: Some(json!({
                "type": { "name": "port" },
                "required": true,
                "default": Value::Null,
                "docs": Value::Null,
            })),
            ..CloudKey::default()
        };
        let schema = &key.to_write()["schema"];
        assert_eq!(schema.get("default"), None);
        assert_eq!(schema.get("docs"), None);
        assert_eq!(schema["required"], true);
    }

    #[test]
    fn an_iso_expiry_is_the_one_the_credential_carries() {
        let body = json!({ "credential": "pck_FAKE", "expiresAt": "2026-09-08T12:34:56Z" });
        let bearer = bearer_of("https://penv.cloud", &body, 1_000).unwrap();
        assert_eq!(bearer.expires_at, Some(1_788_870_896));

        let fallback = json!({ "credential": "pck_FAKE", "expiresIn": 900 });
        assert_eq!(
            bearer_of("https://penv.cloud", &fallback, 1_000)
                .unwrap()
                .expires_at,
            Some(1_900)
        );
    }

    #[test]
    fn a_bearer_never_prints_itself() {
        let shown = format!("{:?}", Bearer::new("pcu_FAKE_NEVER_PRINTED"));
        assert!(!shown.contains("FAKE"), "{shown}");
        assert!(shown.contains("user"), "{shown}");
    }

    #[test]
    fn a_write_never_sends_back_what_only_the_server_owns() {
        let key = CloudKey {
            path: String::new(),
            name: "PORT".into(),
            kind: Some("static".into()),
            version: Some(3),
            schema: Some(json!({ "type": "port" })),
            value: Some("3000".into()),
            updated_at: Some("2026-09-22T10:00:00Z".into()),
        };
        let sent = key.to_write();
        assert_eq!(sent.get("updatedAt"), None);
        assert_eq!(sent.get("kind"), None);
        assert_eq!(sent.get("version"), None);
        assert_eq!(sent["name"], "PORT");
        assert_eq!(sent["value"], "3000");
    }

    #[test]
    fn a_redeemed_approval_never_prints_the_value_it_carries() {
        let shown = format!(
            "{:?}",
            Revealed {
                key: "STRIPE_SECRET_KEY".into(),
                value: "sk_test_FAKE0000".into(),
            }
        );
        assert!(!shown.contains("FAKE"), "{shown}");
        assert!(shown.contains("STRIPE_SECRET_KEY"), "{shown}");
    }

    #[test]
    fn an_address_is_the_path_the_routes_use() {
        let at = Address::new("acme", "api-gateway", "development");
        assert_eq!(at.to_string(), "acme/api-gateway/development");
    }
}
