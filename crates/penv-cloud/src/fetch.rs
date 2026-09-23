//! Unauthenticated GETs, for the release `upgrade` reads. Redirects are followed
//! here and nowhere else, because nothing on this path carries a credential.

use std::time::Duration;

use sha2::{Digest, Sha256};

use crate::api::checked_url;
use crate::error::{CloudError, Result};

const TIMEOUT: Duration = Duration::from_secs(120);
/// A release binary is tens of megabytes, well past ureq's 10MB default.
const LIMIT: u64 = 128 * 1024 * 1024;

/// What one GET answered. The status and the headers come back with the bytes,
/// so the caller can explain a refusal without a second request.
pub struct Response {
    pub status: u16,
    pub headers: Vec<(String, String)>,
    pub bytes: Vec<u8>,
}

impl Response {
    pub fn ok(&self) -> bool {
        (200..300).contains(&self.status)
    }

    /// One header by name, matched without regard to case.
    pub fn header(&self, name: &str) -> Option<&str> {
        self.headers
            .iter()
            .find(|(key, _)| key.eq_ignore_ascii_case(name))
            .map(|(_, value)| value.as_str())
    }
}

/// One GET, redirects followed over TLS only, the body as bytes. A status other
/// than 2xx comes back whole rather than as an error: the caller reads the
/// headers to say why.
pub fn get(url: &str, accept: &str) -> Result<Response> {
    let url = checked_url(url)?;
    let config = ureq::Agent::config_builder()
        .tls_config(crate::tls::config())
        .http_status_as_error(false)
        .https_only(true)
        .user_agent(format!("penv/{}", env!("CARGO_PKG_VERSION")))
        .timeout_global(Some(TIMEOUT))
        .build();
    let agent = ureq::Agent::new_with_config(config);

    let mut response = agent
        .get(&url)
        .header("Accept", accept)
        .call()
        .map_err(|e| CloudError::Offline {
            url: url.clone(),
            reason: e.to_string(),
        })?;

    let status = response.status().as_u16();
    let headers = response
        .headers()
        .iter()
        .filter_map(|(name, value)| {
            Some((name.as_str().to_string(), value.to_str().ok()?.to_string()))
        })
        .collect();
    let bytes = response
        .body_mut()
        .with_config()
        .limit(LIMIT)
        .read_to_vec()
        .map_err(|e| CloudError::Body {
            url,
            reason: e.to_string(),
        })?;

    Ok(Response {
        status,
        headers,
        bytes,
    })
}

pub fn sha256_hex(bytes: &[u8]) -> String {
    use std::fmt::Write;

    Sha256::digest(bytes)
        .iter()
        .fold(String::with_capacity(64), |mut out, byte| {
            let _ = write!(out, "{byte:02x}");
            out
        })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn answered(status: u16, headers: &[(&str, &str)]) -> Response {
        Response {
            status,
            headers: headers
                .iter()
                .map(|(k, v)| (k.to_string(), v.to_string()))
                .collect(),
            bytes: Vec::new(),
        }
    }

    #[test]
    fn a_digest_is_the_lowercase_hex_the_checksum_files_carry() {
        assert_eq!(
            sha256_hex(b"penv"),
            "5d4a791835646b10990fbeff94cccc456a5c5c455da3cee2391499b400dd7e7b"
        );
    }

    #[test]
    fn a_header_is_found_however_the_server_spelled_it() {
        let response = answered(429, &[("X-RateLimit-Reset", "1700000000")]);
        assert!(!response.ok());
        assert_eq!(response.header("x-ratelimit-reset"), Some("1700000000"));
        assert_eq!(response.header("retry-after"), None);
        assert!(answered(204, &[]).ok());
    }

    #[test]
    fn a_body_penv_cannot_read_is_not_the_json_message() {
        let body = CloudError::Body {
            url: "https://example.test/penv".into(),
            reason: "too big".into(),
        };
        let json = CloudError::Unreadable {
            url: "https://example.test/penv".into(),
            reason: "too big".into(),
        };
        assert_ne!(body.to_string(), json.to_string());
        assert!(!body.to_string().contains("JSON"), "{body}");
    }
}
