//! Credential-scoped HTTPS transport. Never forwards the key to asset CDNs.
use crate::Result;
use reqwest::{
    Url,
    blocking::{Client, Response, multipart},
    header::HeaderValue,
};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::{fs, io::Read, path::Path, thread, time::Duration};

#[derive(Clone, Deserialize, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct Policy {
    pub request_timeout_seconds: u64,
    pub wait_timeout_seconds: u64,
    pub poll_interval_millis: u64,
    pub read_attempts: u32,
    pub max_response_bytes: u64,
    pub max_upload_bytes: u64,
}

pub struct Cloud {
    client: Client,
    base: Url,
    key: HeaderValue,
    secret: String,
    pub policy: Policy,
}

#[derive(Debug)]
pub struct HttpFailure {
    pub status: Option<u16>,
    message: String,
}

impl std::fmt::Display for HttpFailure {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.message)
    }
}
impl std::error::Error for HttpFailure {}

impl Cloud {
    pub fn new(base: &str, key_file: &Path, policy: &Policy) -> Result<Self> {
        let base = Url::parse(base)?;
        let local = matches!(base.host_str(), Some("127.0.0.1" | "[::1]"));
        if !(base.as_str() == "https://apis.roblox.com/"
            || (local && base.scheme() == "http" && base.path() == "/"))
            || !base.username().is_empty()
            || base.password().is_some()
            || base.query().is_some()
            || base.fragment().is_some()
        {
            return Err("API origin must be https://apis.roblox.com/ (or an explicit loopback HTTP test server)".into());
        }
        if policy.request_timeout_seconds == 0
            || policy.wait_timeout_seconds == 0
            || policy.poll_interval_millis == 0
            || policy.read_attempts == 0
            || policy.max_upload_bytes == 0
            || policy.max_response_bytes == 0
            || policy.max_response_bytes == u64::MAX
        {
            return Err("deployment timing, retry and response policies must be positive".into());
        }
        let metadata = fs::metadata(key_file)?;
        if !metadata.is_file() {
            return Err("API key must be a regular file".into());
        }
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            if metadata.permissions().mode() & 0o077 != 0 {
                return Err("API key file must be owner-only (chmod 600)".into());
            }
        }
        let secret = fs::read_to_string(key_file)?.trim().to_owned();
        if secret.is_empty() {
            return Err("empty API key file".into());
        }
        let mut key = HeaderValue::from_str(&secret).map_err(|_| "invalid API key header")?;
        key.set_sensitive(true);
        let client = Client::builder()
            .redirect(reqwest::redirect::Policy::none())
            .timeout(Duration::from_secs(policy.request_timeout_seconds))
            .build()?;
        Ok(Self {
            client,
            base,
            key,
            secret,
            policy: policy.clone(),
        })
    }

    fn url(&self, path: &str) -> Result<Url> {
        if !path.starts_with('/') || path.starts_with("//") {
            return Err("invalid API path".into());
        }
        let url = self.base.join(path)?;
        if url.origin() != self.base.origin() {
            return Err("API path changed credential origin".into());
        }
        Ok(url)
    }

    fn response(&self, response: std::result::Result<Response, reqwest::Error>) -> Result<Vec<u8>> {
        let mut response = response.map_err(|error| {
            let error = error.without_url();
            let mut detail = error.to_string();
            let mut cause = std::error::Error::source(&error);
            while let Some(source) = cause {
                detail.push_str(&format!(": {source}"));
                cause = source.source();
            }
            HttpFailure {
                status: None,
                message: format!(
                    "HTTP transport failed: {}; write outcome may be unknown; receipt retained for reconciliation",
                    detail.replace(&self.secret,"[REDACTED]")
                ),
            }
        })?;
        let status = response.status();
        let mut bytes = Vec::new();
        response
            .by_ref()
            .take(self.policy.max_response_bytes + 1)
            .read_to_end(&mut bytes)?;
        if bytes.len() as u64 > self.policy.max_response_bytes {
            return Err("HTTP response exceeds configured maxResponseBytes".into());
        }
        if !status.is_success() {
            let body = String::from_utf8_lossy(&bytes).replace(&self.secret, "[REDACTED]");
            return Err(HttpFailure {
                status: Some(status.as_u16()),
                message: format!("Roblox HTTP {status}: {body}"),
            }
            .into());
        }
        Ok(bytes)
    }

    pub fn get(&self, path: &str) -> Result<Value> {
        let url = self.url(path)?;
        for attempt in 0..self.policy.read_attempts {
            match self.response(
                self.client
                    .get(url.clone())
                    .header("x-api-key", self.key.clone())
                    .send(),
            ) {
                Ok(bytes) => return Ok(serde_json::from_slice(&bytes)?),
                Err(error) => {
                    let retry = error.downcast_ref::<HttpFailure>().is_some_and(|e| {
                        e.status.is_none()
                            || e.status == Some(429)
                            || e.status.is_some_and(|s| s >= 500)
                    });
                    if !retry || attempt + 1 == self.policy.read_attempts {
                        return Err(error);
                    }
                    self.back_off(attempt);
                }
            }
        }
        unreachable!("validated read attempts")
    }

    pub fn upload(&self, request: &Value, name: &str, mime: &str, bytes: Vec<u8>) -> Result<Value> {
        let part = multipart::Part::bytes(bytes)
            .file_name(name.to_owned())
            .mime_str(mime)?;
        let body = multipart::Form::new()
            .text("request", serde_json::to_string(request)?)
            .part("fileContent", part);
        let bytes = self.response(
            self.client
                .post(self.url("/assets/v1/assets")?)
                .header("x-api-key", self.key.clone())
                .multipart(body)
                .send(),
        )?;
        Ok(serde_json::from_slice(&bytes)?)
    }

    pub fn publish(&self, universe: &str, place: &str, bytes: Vec<u8>) -> Result<Value> {
        let path =
            format!("/universes/v1/{universe}/places/{place}/versions?versionType=Published");
        let bytes = self.response(
            self.client
                .post(self.url(&path)?)
                .header("x-api-key", self.key.clone())
                .header("Content-Type", "application/octet-stream")
                .body(bytes)
                .send(),
        )?;
        Ok(serde_json::from_slice(&bytes)?)
    }

    pub fn place_bytes(&self, place: &str, version: u64) -> Result<Vec<u8>> {
        let delivery = self.get(&format!(
            "/asset-delivery-api/v1/assetId/{place}/version/{version}"
        ))?;
        let location = delivery["location"]
            .as_str()
            .ok_or_else(|| format!("asset delivery has no location: {delivery}"))?;
        let url = Url::parse(location)?;
        let production =
            url.scheme() == "https" && url.host_str().is_some_and(|h| h.ends_with(".rbxcdn.com"));
        let test = self.base.scheme() == "http" && url.origin() == self.base.origin();
        if (!production && !test) || !url.username().is_empty() || url.password().is_some() {
            return Err("asset delivery returned an untrusted CDN origin".into());
        }
        // Deliberately no API key: CDN authorization is carried by the returned URL.
        self.response(self.client.get(url).send())
    }

    pub fn pause(&self) {
        thread::sleep(Duration::from_millis(self.policy.poll_interval_millis));
    }

    /// Waits longer after each failed attempt: the poll interval doubled per
    /// attempt, at most a minute.
    pub fn back_off(&self, attempt: u32) {
        let millis = self
            .policy
            .poll_interval_millis
            .saturating_mul(1 << attempt.min(16))
            .min(60_000);
        thread::sleep(Duration::from_millis(millis));
    }
}
