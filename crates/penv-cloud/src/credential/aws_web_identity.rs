//! EKS IAM Roles for Service Accounts and any other web-identity federation:
//! the pod's token file and role are exchanged with STS
//! (`AssumeRoleWithWebIdentity`, an unsigned call) for temporary keys, which
//! are then proved the same way as [`AwsIam`].

use std::collections::BTreeMap;
use std::fmt;
use std::time::Duration;

use crate::api::{Api, Bearer, checked_url, url_host};
use crate::credential::{AwsIam, Obtain};
use crate::error::{CloudError, Result};

pub const TOKEN_FILE_VAR: &str = "AWS_WEB_IDENTITY_TOKEN_FILE";
pub const ROLE_ARN_VAR: &str = "AWS_ROLE_ARN";
pub const SESSION_NAME_VAR: &str = "AWS_ROLE_SESSION_NAME";
/// The AWS SDKs' own override for the STS endpoint, and the only one read.
pub const ENDPOINT_VAR: &str = "AWS_ENDPOINT_URL_STS";

#[derive(Clone, PartialEq, Eq)]
pub struct AwsWebIdentity {
    token_file: String,
    role_arn: String,
    session_name: String,
    endpoint: String,
    region: String,
    org: Option<String>,
}

impl fmt::Debug for AwsWebIdentity {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("AwsWebIdentity")
            .field("role_arn", &self.role_arn)
            .finish_non_exhaustive()
    }
}

impl AwsWebIdentity {
    /// The workspace the signed login names; see [`AwsIam::for_org`].
    pub fn for_org(mut self, org: Option<&str>) -> AwsWebIdentity {
        self.org = org.map(str::to_string);
        self
    }

    pub fn from_env(env: &BTreeMap<String, String>) -> Option<AwsWebIdentity> {
        let at = |key: &str| env.get(key).filter(|v| !v.is_empty()).cloned();
        let region = super::aws::region(env);
        let endpoint =
            at(ENDPOINT_VAR).unwrap_or_else(|| format!("https://sts.{region}.amazonaws.com"));
        Some(AwsWebIdentity {
            token_file: at(TOKEN_FILE_VAR)?,
            role_arn: at(ROLE_ARN_VAR)?,
            session_name: at(SESSION_NAME_VAR).unwrap_or_else(|| "penv".to_string()),
            endpoint: endpoint.trim_end_matches('/').to_string(),
            region,
            org: None,
        })
    }

    fn assume(&self) -> Result<AwsIam> {
        let failed = |why: &str| CloudError::Credential(format!("STS web identity {why}"));
        // The token goes over https, or to loopback, and to no one in front of a host.
        let url = checked_url(&format!("{}/", self.endpoint))?;
        let token = std::fs::read_to_string(&self.token_file)
            .map_err(|_| failed("token file could not be read"))?;
        let body = format!(
            "Action=AssumeRoleWithWebIdentity&Version=2011-06-15&RoleArn={}&RoleSessionName={}&WebIdentityToken={}",
            form(&self.role_arn),
            form(&self.session_name),
            form(token.trim())
        );
        let mut response = agent(&url)
            .post(&url)
            .header(
                "Content-Type",
                "application/x-www-form-urlencoded; charset=utf-8",
            )
            .send(body.as_bytes())
            .map_err(|_| failed("could not be reached"))?;
        let status = response.status().as_u16();
        let text = response.body_mut().read_to_string().unwrap_or_default();
        if !(200..300).contains(&status) {
            let code = tag(&text, "Code").unwrap_or_else(|| status.to_string());
            return Err(failed(&format!("was refused: {code}")));
        }
        match (tag(&text, "AccessKeyId"), tag(&text, "SecretAccessKey")) {
            (Some(key), Some(secret)) => {
                Ok(
                    AwsIam::new(key, secret, tag(&text, "SessionToken"), self.region.clone())
                        .for_org(self.org.as_deref()),
                )
            }
            _ => Err(failed("answered without an access key")),
        }
    }
}

impl Obtain for AwsWebIdentity {
    fn obtain(&self, api: &Api, now: u64) -> Result<Bearer> {
        self.assume()?.obtain(api, now)
    }

    /// The role and the token the platform wrote, read from the file and never
    /// sent anywhere for this.
    fn identity(&self) -> Option<String> {
        let token = std::fs::read_to_string(&self.token_file).ok()?;
        Some(format!(
            "aws-web-identity:{}:{}",
            self.role_arn,
            token.trim()
        ))
    }
}

/// A loopback endpoint is called directly, never handed to a proxy with the
/// token. Public STS keeps the environment's proxy, which an egress-filtered
/// cluster needs to reach it at all, as the AWS SDKs do.
fn agent(url: &str) -> ureq::Agent {
    let loopback = url_host(url)
        .is_some_and(|host| matches!(host.as_str(), "127.0.0.1" | "localhost" | "[::1]"));
    let proxy = if loopback {
        None
    } else {
        ureq::Proxy::try_from_env()
    };
    ureq::Agent::config_builder()
        .tls_config(crate::tls::config())
        .proxy(proxy)
        .timeout_global(Some(Duration::from_secs(10)))
        .http_status_as_error(false)
        // The web identity token goes to STS and nowhere a redirect points.
        .max_redirects(0)
        .build()
        .into()
}

/// The text of the first `<name>` element.
fn tag(xml: &str, name: &str) -> Option<String> {
    let open = format!("<{name}>");
    let start = xml.find(&open)? + open.len();
    let end = xml[start..].find(&format!("</{name}>"))? + start;
    Some(xml[start..end].trim().to_string())
}

fn form(value: &str) -> String {
    value
        .bytes()
        .map(|b| match b {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'.' | b'_' | b'~' => {
                (b as char).to_string()
            }
            _ => format!("%{b:02X}"),
        })
        .collect()
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
    fn both_the_token_file_and_the_role_are_needed() {
        assert!(AwsWebIdentity::from_env(&env(&[(TOKEN_FILE_VAR, "/t")])).is_none());
        assert!(
            AwsWebIdentity::from_env(&env(&[(ROLE_ARN_VAR, "arn:aws:iam::1:role/r")])).is_none()
        );
        let got = AwsWebIdentity::from_env(&env(&[
            (TOKEN_FILE_VAR, "/t"),
            (ROLE_ARN_VAR, "arn:aws:iam::1:role/r"),
            ("AWS_REGION", "eu-west-1"),
        ]))
        .unwrap();
        assert_eq!(got.endpoint, "https://sts.eu-west-1.amazonaws.com");
        assert_eq!(got.session_name, "penv");
    }

    #[test]
    fn a_region_that_is_not_a_region_never_reaches_the_sts_host_name() {
        let got = AwsWebIdentity::from_env(&env(&[
            (TOKEN_FILE_VAR, "/t"),
            (ROLE_ARN_VAR, "r"),
            ("AWS_REGION", "x.evil.test/#"),
        ]))
        .unwrap();
        assert_eq!(got.endpoint, "https://sts.us-east-1.amazonaws.com");
    }

    #[test]
    fn the_sdks_endpoint_override_is_honoured() {
        let got = AwsWebIdentity::from_env(&env(&[
            (TOKEN_FILE_VAR, "/t"),
            (ROLE_ARN_VAR, "r"),
            ("AWS_ENDPOINT_URL_STS", "http://127.0.0.1:9/"),
        ]))
        .unwrap();
        assert_eq!(got.endpoint, "http://127.0.0.1:9");
    }

    #[test]
    fn only_the_sts_override_is_read() {
        let got = AwsWebIdentity::from_env(&env(&[
            (TOKEN_FILE_VAR, "/t"),
            (ROLE_ARN_VAR, "r"),
            ("AWS_ENDPOINT_URL", "https://elsewhere.example.test"),
        ]))
        .unwrap();
        assert_eq!(got.endpoint, "https://sts.us-east-1.amazonaws.com");
    }

    #[test]
    fn the_web_identity_token_never_travels_over_plain_http_or_to_user_info() {
        for endpoint in [
            "http://sts.evil.test",
            "https://u:secretFAKE@sts.evil.test",
            "ftp://sts.evil.test",
        ] {
            let got = AwsWebIdentity::from_env(&env(&[
                (TOKEN_FILE_VAR, "/no/such/token"),
                (ROLE_ARN_VAR, "r"),
                (ENDPOINT_VAR, endpoint),
            ]))
            .unwrap();
            let error = got.assume().unwrap_err();
            assert!(matches!(error, CloudError::Url(_)), "{endpoint}: {error:?}");
            assert!(!error.to_string().contains("secretFAKE"), "{error}");
        }
    }

    #[test]
    fn a_loopback_endpoint_is_called_directly_and_never_through_a_proxy() {
        for url in [
            "http://127.0.0.1:9/",
            "http://localhost/",
            "http://[::1]:9/",
        ] {
            assert!(agent(url).config().proxy().is_none(), "{url}");
        }
    }

    #[test]
    fn the_sts_answer_is_read_and_form_values_are_encoded() {
        let xml = "<AssumeRoleWithWebIdentityResponse><Credentials><AccessKeyId>ASIA1</AccessKeyId><SecretAccessKey>s/+=</SecretAccessKey><SessionToken>t</SessionToken></Credentials></AssumeRoleWithWebIdentityResponse>";
        assert_eq!(tag(xml, "AccessKeyId").as_deref(), Some("ASIA1"));
        assert_eq!(tag(xml, "Nope"), None);
        assert_eq!(
            form("arn:aws:iam::1:role/r a"),
            "arn%3Aaws%3Aiam%3A%3A1%3Arole%2Fr%20a"
        );
    }
}
