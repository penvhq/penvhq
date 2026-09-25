use std::collections::BTreeMap;
use std::fmt;

use crate::api::{Api, Bearer};
use crate::credential::{Find, Obtain, Place};
use crate::error::Result;

/// GitHub Actions mints one token per audience, from a URL it puts in the job.
pub const GITHUB_URL_VAR: &str = "ACTIONS_ID_TOKEN_REQUEST_URL";
pub const GITHUB_TOKEN_VAR: &str = "ACTIONS_ID_TOKEN_REQUEST_TOKEN";
/// GitLab writes the token straight into the job.
pub const GITLAB_VAR: &str = "ID_TOKEN";
/// Any other platform, or a token minted by hand.
pub const GENERIC_VAR: &str = "PENV_OIDC_TOKEN";

/// Where the platform token comes from. GitHub hands out a URL; everyone else
/// hands out the token.
#[derive(Clone, PartialEq, Eq)]
enum Platform {
    GithubActions {
        url: String,
        request_token: String,
        audience: Option<String>,
    },
    Held(String),
}

/// A platform JWT exchanged for a short-lived penv credential.
#[derive(Clone, PartialEq, Eq)]
pub struct Oidc(Platform);

impl Oidc {
    /// GitHub Actions, then GitLab, then the generic variable. `audience` is the
    /// org the schema names.
    pub fn from_env(env: &BTreeMap<String, String>, audience: Option<&str>) -> Option<Oidc> {
        let at = |key: &str| env.get(key).filter(|v| !v.is_empty());
        if let (Some(url), Some(request_token)) = (at(GITHUB_URL_VAR), at(GITHUB_TOKEN_VAR)) {
            return Some(Oidc(Platform::GithubActions {
                url: url.clone(),
                request_token: request_token.clone(),
                audience: audience.map(str::to_string),
            }));
        }
        [GITLAB_VAR, GENERIC_VAR]
            .into_iter()
            .find_map(at)
            .map(|token| Oidc(Platform::Held(token.clone())))
    }
}

impl fmt::Debug for Oidc {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let source = match &self.0 {
            Platform::GithubActions { .. } => "github-actions",
            Platform::Held(_) => "held",
        };
        write!(f, "Oidc({source})")
    }
}

impl Obtain for Oidc {
    fn obtain(&self, api: &Api, now: u64) -> Result<Bearer> {
        let token = match &self.0 {
            Platform::GithubActions {
                url,
                request_token,
                audience,
            } => api.platform_id_token(url, request_token, audience.as_deref())?,
            Platform::Held(token) => token.clone(),
        };
        api.exchange_oidc(&token, now)
    }

    /// A held token is the same for the whole job; GitHub's is fetched per run.
    fn identity(&self) -> Option<String> {
        match &self.0 {
            Platform::GithubActions { .. } => None,
            Platform::Held(token) => Some(format!("oidc:{token}")),
        }
    }
}

/// After what the host holds, before AWS. `org` is the audience.
pub(super) const PLACES: &[Place] = &[Place {
    rank: 40,
    find: Find::Env(|env, org| Some(Box::new(Oidc::from_env(env, org)?))),
}];

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
    fn github_wins_over_a_held_token_and_carries_the_org_as_the_audience() {
        let found = Oidc::from_env(
            &env(&[
                (
                    GITHUB_URL_VAR,
                    "https://token.actions.example/?api-version=1",
                ),
                (GITHUB_TOKEN_VAR, "request_FAKE"),
                (GENERIC_VAR, "held_FAKE"),
            ]),
            Some("acme"),
        )
        .expect("a source");
        assert!(matches!(
            found.0,
            Platform::GithubActions { ref audience, .. } if audience.as_deref() == Some("acme")
        ));
    }

    #[test]
    fn gitlab_and_the_generic_variable_are_both_held_tokens() {
        for var in ["ID_TOKEN", GENERIC_VAR] {
            let found = Oidc::from_env(&env(&[(var, "jwt_FAKE")]), None).expect(var);
            assert!(matches!(found.0, Platform::Held(_)), "{var}");
        }
    }

    #[test]
    fn only_the_variables_the_design_names_are_read() {
        assert!(Oidc::from_env(&env(&[("CI_JOB_JWT_V2", "jwt_FAKE")]), None).is_none());
    }

    #[test]
    fn a_job_with_no_token_source_offers_nothing() {
        assert!(Oidc::from_env(&env(&[("CI", "true")]), None).is_none());
        assert!(Oidc::from_env(&env(&[(GITHUB_URL_VAR, "https://x")]), None).is_none());
    }

    #[test]
    fn a_platform_token_never_prints_itself() {
        let found = Oidc::from_env(&env(&[(GENERIC_VAR, "jwt_FAKE_NEVER_PRINTED")]), None).unwrap();
        assert!(!format!("{found:?}").contains("FAKE"));
    }
}
