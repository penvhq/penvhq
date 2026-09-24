use std::collections::BTreeMap;
use std::fmt;

use hmac::{Hmac, KeyInit, Mac};
use sha2::{Digest, Sha256};

use crate::api::{Api, Bearer, SignedRequest};
use crate::credential::Obtain;
use crate::error::Result;

pub const ACCESS_KEY_VAR: &str = "AWS_ACCESS_KEY_ID";
pub const SECRET_KEY_VAR: &str = "AWS_SECRET_ACCESS_KEY";
pub const SESSION_TOKEN_VAR: &str = "AWS_SESSION_TOKEN";
pub const REGION_VARS: [&str; 2] = ["AWS_REGION", "AWS_DEFAULT_REGION"];

const DEFAULT_REGION: &str = "us-east-1";
const SERVICE: &str = "sts";
const ALGORITHM: &str = "AWS4-HMAC-SHA256";
const BODY: &str = "Action=GetCallerIdentity&Version=2011-06-15";
const CONTENT_TYPE: &str = "application/x-www-form-urlencoded; charset=utf-8";
/// The workspace the login is for. The server requires it signed, so a
/// `GetCallerIdentity` signed for another service cannot be replayed to it.
pub const ORG_HEADER: &str = "x-penv-cloud-org";

/// `AWS_REGION`, then `AWS_DEFAULT_REGION`, then us-east-1. A value that is not
/// shaped like a region (letters, digits and dashes) is ignored: it becomes part
/// of a host name, and `x.evil.test/#` must not.
pub fn region(env: &BTreeMap<String, String>) -> String {
    REGION_VARS
        .iter()
        .filter_map(|k| env.get(*k))
        .find(|v| {
            !v.is_empty()
                && v.len() <= 32
                && v.chars()
                    .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '-')
        })
        .cloned()
        .unwrap_or_else(|| DEFAULT_REGION.to_string())
}

/// The caller's own AWS identity, signed once and replayed by the server.
#[derive(Clone, PartialEq, Eq)]
pub struct AwsIam {
    access_key_id: String,
    secret_access_key: String,
    session_token: Option<String>,
    region: String,
    org: Option<String>,
}

impl AwsIam {
    pub fn new(
        access_key_id: impl Into<String>,
        secret_access_key: impl Into<String>,
        session_token: Option<String>,
        region: impl Into<String>,
    ) -> AwsIam {
        AwsIam {
            access_key_id: access_key_id.into(),
            secret_access_key: secret_access_key.into(),
            session_token,
            region: region.into(),
            org: None,
        }
    }

    /// The workspace, by slug or id, that the signed request names.
    pub fn for_org(mut self, org: Option<&str>) -> AwsIam {
        self.org = org.map(str::to_string);
        self
    }

    /// The variables every AWS runtime sets. A role gives all three.
    pub fn from_env(env: &BTreeMap<String, String>) -> Option<AwsIam> {
        let at = |key: &str| env.get(key).filter(|v| !v.is_empty()).cloned();
        Some(AwsIam {
            access_key_id: at(ACCESS_KEY_VAR)?,
            secret_access_key: at(SECRET_KEY_VAR)?,
            session_token: at(SESSION_TOKEN_VAR),
            region: region(env),
            org: None,
        })
    }

    pub fn host(&self) -> String {
        format!("sts.{}.amazonaws.com", self.region)
    }

    /// SigV4 over `GetCallerIdentity`, which proves the identity without ever
    /// handing over the secret key.
    pub fn sign(&self, now: u64) -> SignedRequest {
        let (timestamp, day) = amz_time(now);
        let host = self.host();

        let mut headers: BTreeMap<String, String> = BTreeMap::new();
        headers.insert("content-type".into(), CONTENT_TYPE.into());
        headers.insert("host".into(), host.clone());
        headers.insert("x-amz-date".into(), timestamp.clone());
        if let Some(token) = &self.session_token {
            headers.insert("x-amz-security-token".into(), token.clone());
        }
        if let Some(org) = &self.org {
            headers.insert(ORG_HEADER.into(), org.clone());
        }

        let signed_headers = headers.keys().cloned().collect::<Vec<_>>().join(";");
        let canonical_headers: String = headers
            .iter()
            .map(|(name, value)| format!("{name}:{}\n", value.trim()))
            .collect();
        let canonical_request = format!(
            "POST\n/\n\n{canonical_headers}\n{signed_headers}\n{}",
            hex(&Sha256::digest(BODY.as_bytes()))
        );

        let scope = format!("{day}/{}/{SERVICE}/aws4_request", self.region);
        let to_sign = format!(
            "{ALGORITHM}\n{timestamp}\n{scope}\n{}",
            hex(&Sha256::digest(canonical_request.as_bytes()))
        );

        let mut key = mac(
            format!("AWS4{}", self.secret_access_key).as_bytes(),
            day.as_bytes(),
        );
        for part in [self.region.as_bytes(), SERVICE.as_bytes(), b"aws4_request"] {
            key = mac(&key, part);
        }
        let signature = hex(&mac(&key, to_sign.as_bytes()));

        headers.insert(
            "authorization".into(),
            format!(
                "{ALGORITHM} Credential={}/{scope}, SignedHeaders={signed_headers}, Signature={signature}",
                self.access_key_id
            ),
        );

        SignedRequest {
            method: "POST".into(),
            url: format!("https://{host}/"),
            body: BODY.into(),
            headers,
        }
    }
}

impl fmt::Debug for AwsIam {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("AwsIam")
            .field("region", &self.region)
            .finish_non_exhaustive()
    }
}

impl Obtain for AwsIam {
    fn obtain(&self, api: &Api, now: u64) -> Result<Bearer> {
        api.exchange_aws(&self.sign(now), now)
    }

    /// The keys themselves: a session's keys change with the session, and so
    /// does the cache that opens for them.
    fn identity(&self) -> Option<String> {
        Some(format!(
            "aws:{}:{}:{}",
            self.access_key_id,
            self.secret_access_key,
            self.session_token.as_deref().unwrap_or_default()
        ))
    }
}

fn mac(key: &[u8], data: &[u8]) -> Vec<u8> {
    let mut hmac =
        <Hmac<Sha256> as KeyInit>::new_from_slice(key).expect("hmac takes any key length");
    hmac.update(data);
    hmac.finalize().into_bytes().to_vec()
}

fn hex(bytes: &[u8]) -> String {
    bytes.iter().map(|b| format!("{b:02x}")).collect()
}

/// `20260908T131415Z` and `20260908`, from seconds since the epoch.
pub fn amz_time(now: u64) -> (String, String) {
    let days = (now / 86_400) as i64;
    let seconds = now % 86_400;
    let (year, month, day) = civil(days);
    (
        format!(
            "{year:04}{month:02}{day:02}T{:02}{:02}{:02}Z",
            seconds / 3600,
            (seconds % 3600) / 60,
            seconds % 60
        ),
        format!("{year:04}{month:02}{day:02}"),
    )
}

/// Days since 1970-01-01 to a civil date, by Howard Hinnant's algorithm.
fn civil(days: i64) -> (i64, u32, u32) {
    let z = days + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z.rem_euclid(146_097);
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let year = yoe + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let day = (doy - (153 * mp + 2) / 5 + 1) as u32;
    let month = if mp < 10 { mp + 3 } else { mp - 9 } as u32;
    (if month <= 2 { year + 1 } else { year }, month, day)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::BTreeSet;

    fn env(pairs: &[(&str, &str)]) -> BTreeMap<String, String> {
        pairs
            .iter()
            .map(|(k, v)| (k.to_string(), v.to_string()))
            .collect()
    }

    #[test]
    fn a_host_with_no_aws_keys_offers_nothing() {
        assert!(AwsIam::from_env(&env(&[(ACCESS_KEY_VAR, "AKIAFAKE")])).is_none());
        assert!(
            AwsIam::from_env(&env(&[
                (ACCESS_KEY_VAR, "AKIAFAKE"),
                (SECRET_KEY_VAR, "secretFAKE"),
            ]))
            .is_some()
        );
    }

    #[test]
    fn the_region_falls_back_from_both_variables_to_us_east_one() {
        let base = [(ACCESS_KEY_VAR, "AKIAFAKE"), (SECRET_KEY_VAR, "secretFAKE")];
        let plain = AwsIam::from_env(&env(&base)).unwrap();
        assert_eq!(plain.region, DEFAULT_REGION);

        let mut pairs = base.to_vec();
        pairs.push(("AWS_DEFAULT_REGION", "eu-west-2"));
        assert_eq!(AwsIam::from_env(&env(&pairs)).unwrap().region, "eu-west-2");
        pairs.push(("AWS_REGION", "eu-central-1"));
        assert_eq!(
            AwsIam::from_env(&env(&pairs)).unwrap().region,
            "eu-central-1"
        );
    }

    #[test]
    fn the_epoch_becomes_the_stamp_sigv4_expects() {
        assert_eq!(
            amz_time(1_369_353_600),
            ("20130524T000000Z".to_string(), "20130524".to_string())
        );
        assert_eq!(amz_time(0).0, "19700101T000000Z");
        assert_eq!(amz_time(1_757_337_296).0, "20250908T131456Z");
    }

    /// The published example from the AWS SigV4 test suite: same key, same
    /// scope, same string to sign.
    #[test]
    fn the_signing_key_matches_the_published_derivation() {
        let mut key = mac(b"AWS4wJalrXUtnFEMI/K7MDENG+bPxRfiCYEXAMPLEKEY", b"20150830");
        for part in [b"us-east-1".as_slice(), b"iam", b"aws4_request"] {
            key = mac(&key, part);
        }
        assert_eq!(
            hex(&key),
            "c4afb1cc5771d871763a393e44b703571b55cc28424d1a5e86da6ed3c154a4b9"
        );
    }

    #[test]
    fn the_workspace_is_signed_so_the_login_cannot_be_replayed_elsewhere() {
        let aws = AwsIam::new("AKIAFAKE", "secretFAKE", None, "us-east-1").for_org(Some("acme"));
        let signed = aws.sign(1_369_353_600);
        assert_eq!(signed.headers[ORG_HEADER], "acme");
        assert!(
            signed.headers["authorization"]
                .contains("SignedHeaders=content-type;host;x-amz-date;x-penv-cloud-org,"),
            "{}",
            signed.headers["authorization"]
        );
        let plain = AwsIam::new("AKIAFAKE", "secretFAKE", None, "us-east-1").sign(1_369_353_600);
        assert!(!plain.headers.contains_key(ORG_HEADER));
    }

    #[test]
    fn the_signed_request_carries_every_header_it_signed() {
        let aws = AwsIam::new(
            "AKIAFAKE",
            "secretFAKE",
            Some("sessionFAKE".into()),
            "eu-west-1",
        );
        let signed = aws.sign(1_369_353_600);

        assert_eq!(signed.method, "POST");
        assert_eq!(signed.url, "https://sts.eu-west-1.amazonaws.com/");
        assert_eq!(signed.body, BODY);
        assert_eq!(signed.headers["host"], "sts.eu-west-1.amazonaws.com");
        assert_eq!(signed.headers["x-amz-date"], "20130524T000000Z");
        assert_eq!(signed.headers["x-amz-security-token"], "sessionFAKE");

        let authorization = &signed.headers["authorization"];
        assert!(authorization.starts_with(ALGORITHM), "{authorization}");
        assert!(
            authorization
                .contains("SignedHeaders=content-type;host;x-amz-date;x-amz-security-token"),
            "{authorization}"
        );
        assert!(
            !authorization.contains("secretFAKE"),
            "the secret key never travels"
        );
    }

    #[test]
    fn a_session_token_changes_the_headers_that_get_signed() {
        let with = AwsIam::new(
            "AKIAFAKE",
            "secretFAKE",
            Some("sessionFAKE".into()),
            "us-east-1",
        );
        let without = AwsIam::new("AKIAFAKE", "secretFAKE", None, "us-east-1");
        let names: BTreeSet<String> = without.sign(0).headers.keys().cloned().collect();
        assert!(!names.contains("x-amz-security-token"));
        assert_ne!(
            with.sign(0).headers["authorization"],
            without.sign(0).headers["authorization"]
        );
    }

    #[test]
    fn aws_keys_never_print_themselves() {
        let aws = AwsIam::new("AKIAFAKE", "secretFAKE", None, "us-east-1");
        assert!(!format!("{aws:?}").contains("FAKE"));
    }
}
