//! EKS IAM Roles for Service Accounts and any other web-identity federation:
//! the pod's token file and role are exchanged with STS
//! (`AssumeRoleWithWebIdentity`, an unsigned call) for temporary keys, which
//! are then proved the same way as [`AwsIam`].

use std::collections::BTreeMap;
use std::fmt;
use std::time::Duration;

use crate::api::{Api, Bearer};
use crate::credential::{AwsIam, Obtain};
use crate::error::{CloudError, Result};

pub const TOKEN_FILE_VAR: &str = "AWS_WEB_IDENTITY_TOKEN_FILE";
pub const ROLE_ARN_VAR: &str = "AWS_ROLE_ARN";
pub const SESSION_NAME_VAR: &str = "AWS_ROLE_SESSION_NAME";
/// The AWS SDKs' own endpoint overrides, service-specific first.
pub const ENDPOINT_VARS: [&str; 2] = ["AWS_ENDPOINT_URL_STS", "AWS_ENDPOINT_URL"];

#[derive(Clone, PartialEq, Eq)]
pub struct AwsWebIdentity {
    token_file: String,
    role_arn: String,
    session_name: String,
    endpoint: String,
    region: String,
}

impl fmt::Debug for AwsWebIdentity {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("AwsWebIdentity")
            .field("role_arn", &self.role_arn)
            .finish_non_exhaustive()
    }
}

impl AwsWebIdentity {
    pub fn from_env(env: &BTreeMap<String, String>) -> Option<AwsWebIdentity> {
        let at = |key: &str| env.get(key).filter(|v| !v.is_empty()).cloned();
        let region = super::aws::region(env);
        let endpoint = ENDPOINT_VARS
            .iter()
            .find_map(|v| at(v))
            .unwrap_or_else(|| format!("https://sts.{region}.amazonaws.com"));
        Some(AwsWebIdentity {
            token_file: at(TOKEN_FILE_VAR)?,
            role_arn: at(ROLE_ARN_VAR)?,
            session_name: at(SESSION_NAME_VAR).unwrap_or_else(|| "penv".to_string()),
            endpoint: endpoint.trim_end_matches('/').to_string(),
            region,
        })
    }

    fn assume(&self) -> Result<AwsIam> {
        let failed = |why: &str| CloudError::Credential(format!("STS web identity {why}"));
        let token = std::fs::read_to_string(&self.token_file)
            .map_err(|_| failed("token file could not be read"))?;
        let body = format!(
            "Action=AssumeRoleWithWebIdentity&Version=2011-06-15&RoleArn={}&RoleSessionName={}&WebIdentityToken={}",
            form(&self.role_arn),
            form(&self.session_name),
            form(token.trim())
        );
        let agent: ureq::Agent = ureq::Agent::config_builder()
            .tls_config(crate::tls::config())
            .timeout_global(Some(Duration::from_secs(10)))
            .http_status_as_error(false)
            // The web identity token goes to STS and nowhere a redirect points.
            .max_redirects(0)
            .build()
            .into();
        let mut response = agent
            .post(&format!("{}/", self.endpoint))
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
            (Some(key), Some(secret)) => Ok(AwsIam::new(
                key,
                secret,
                tag(&text, "SessionToken"),
                self.region.clone(),
            )),
            _ => Err(failed("answered without an access key")),
        }
    }
}

impl Obtain for AwsWebIdentity {
    fn obtain(&self, api: &Api, now: u64) -> Result<Bearer> {
        self.assume()?.obtain(api, now)
    }
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
