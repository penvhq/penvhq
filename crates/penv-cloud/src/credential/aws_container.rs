//! AWS credentials a container is handed through an endpoint instead of the
//! environment: ECS task roles (`AWS_CONTAINER_CREDENTIALS_RELATIVE_URI`) and EKS
//! Pod Identity (`AWS_CONTAINER_CREDENTIALS_FULL_URI` with an authorization token
//! file). They are fetched, then proved the same way as [`AwsIam`].

use std::collections::BTreeMap;
use std::fmt;
use std::time::Duration;

use crate::api::{Api, Bearer};
use crate::credential::{AwsIam, Obtain};
use crate::error::{CloudError, Result};

pub const RELATIVE_URI_VAR: &str = "AWS_CONTAINER_CREDENTIALS_RELATIVE_URI";
pub const FULL_URI_VAR: &str = "AWS_CONTAINER_CREDENTIALS_FULL_URI";
pub const AUTH_TOKEN_VAR: &str = "AWS_CONTAINER_AUTHORIZATION_TOKEN";
pub const AUTH_TOKEN_FILE_VAR: &str = "AWS_CONTAINER_AUTHORIZATION_TOKEN_FILE";

/// ECS's fixed credential host.
const ECS_HOST: &str = "http://169.254.170.2";

#[derive(Clone, PartialEq, Eq)]
pub struct AwsContainer {
    url: String,
    token: Option<String>,
    token_file: Option<String>,
    region: String,
}

impl fmt::Debug for AwsContainer {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("AwsContainer")
            .field("url", &self.url)
            .finish_non_exhaustive()
    }
}

impl AwsContainer {
    /// The relative form wins, as in the AWS SDKs. A full URI is honoured only
    /// where the SDKs honour it: https, loopback, or the ECS and EKS link-local
    /// hosts, so a planted variable cannot send the authorization token anywhere.
    pub fn from_env(env: &BTreeMap<String, String>) -> Option<AwsContainer> {
        let at = |key: &str| env.get(key).filter(|v| !v.is_empty()).cloned();
        let url = match (at(RELATIVE_URI_VAR), at(FULL_URI_VAR)) {
            (Some(relative), _) if relative.starts_with('/') => format!("{ECS_HOST}{relative}"),
            (Some(_), _) => return None,
            (None, Some(full)) if allowed(&full) => full,
            _ => return None,
        };
        Some(AwsContainer {
            url,
            token: at(AUTH_TOKEN_VAR),
            token_file: at(AUTH_TOKEN_FILE_VAR),
            region: super::aws::region(env),
        })
    }

    fn fetch(&self) -> Result<AwsIam> {
        let failed =
            |why: &str| CloudError::Credential(format!("the container credential endpoint {why}"));
        let token = match (&self.token_file, &self.token) {
            (Some(file), _) => Some(
                std::fs::read_to_string(file)
                    .map_err(|_| failed("token file could not be read"))?
                    .trim()
                    .to_string(),
            ),
            (None, token) => token.clone(),
        };
        let agent: ureq::Agent = ureq::Agent::config_builder()
            .timeout_global(Some(Duration::from_secs(5)))
            .http_status_as_error(false)
            // A redirect could carry the authorization token to another host.
            .max_redirects(0)
            .build()
            .into();
        let mut request = agent.get(&self.url);
        if let Some(token) = &token {
            request = request.header("Authorization", token);
        }
        let mut response = request.call().map_err(|_| failed("could not be reached"))?;
        if !response.status().is_success() {
            return Err(failed(&format!("answered {}", response.status().as_u16())));
        }
        let body: serde_json::Value = response
            .body_mut()
            .read_json()
            .map_err(|_| failed("answered something that is not credentials"))?;
        let field = |name: &str| body.get(name).and_then(|v| v.as_str()).map(str::to_string);
        match (field("AccessKeyId"), field("SecretAccessKey")) {
            (Some(key), Some(secret)) => Ok(AwsIam::new(
                key,
                secret,
                field("Token"),
                self.region.clone(),
            )),
            _ => Err(failed("answered without an access key")),
        }
    }
}

/// https anywhere, http only to loopback and the ECS and EKS credential hosts.
pub fn allowed(url: &str) -> bool {
    if url.starts_with("https://") {
        return true;
    }
    let Some(rest) = url.strip_prefix("http://") else {
        return false;
    };
    let authority = rest.split(['/', '?', '#']).next().unwrap_or_default();
    // `http://[::1]@evil.test/` names evil.test: user info or a backslash in
    // the authority is refused outright.
    if authority.contains(['@', '\\']) {
        return false;
    }
    let host = authority;
    let host = if let Some(v6) = host.strip_prefix('[') {
        v6.split(']').next().unwrap_or_default()
    } else {
        host.rsplit_once(':').map_or(host, |(h, _)| h)
    };
    host == "localhost"
        || host == "169.254.170.2"
        || host == "169.254.170.23"
        || host == "::1"
        || host == "fd00:ec2::23"
        || host.split('.').next() == Some("127") && host.split('.').count() == 4
}

impl Obtain for AwsContainer {
    fn obtain(&self, api: &Api, now: u64) -> Result<Bearer> {
        self.fetch()?.obtain(api, now)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn env(pairs: &[(&str, &str)]) -> BTreeMap<String, String> {
        pairs
            .iter()
            .map(|(k, v)| (k.to_string(), v.to_string()))
            .collect()
    }

    #[test]
    fn ecs_relative_uri_goes_to_the_fixed_host() {
        let c = AwsContainer::from_env(&env(&[(RELATIVE_URI_VAR, "/v2/credentials/abc")])).unwrap();
        assert_eq!(c.url, "http://169.254.170.2/v2/credentials/abc");
        assert!(AwsContainer::from_env(&env(&[(RELATIVE_URI_VAR, "evil.test/x")])).is_none());
    }

    #[test]
    fn a_full_uri_is_honoured_only_where_the_sdks_honour_it() {
        for ok in [
            "http://169.254.170.23/v1/credentials",
            "http://127.0.0.1:8080/creds",
            "http://localhost/creds",
            "http://[::1]:9/creds",
            "http://[fd00:ec2::23]/v1",
            "https://creds.example.test/x",
        ] {
            assert!(allowed(ok), "{ok}");
        }
        for bad in [
            "http://evil.test/creds",
            "http://127.0.0.1.evil.test/",
            "http://169.254.169.254/latest",
            "ftp://127.0.0.1/",
            "http://10.0.0.1/",
            "http://[::1]@evil.test/creds",
            "http://127.0.0.1@evil.test/",
            "http://127.0.0.1\\@evil.test/",
            "HTTP://127.0.0.1/",
        ] {
            assert!(!allowed(bad), "{bad}");
        }
    }

    #[test]
    fn nothing_set_means_this_kind_does_not_apply() {
        assert!(AwsContainer::from_env(&env(&[])).is_none());
    }
}
