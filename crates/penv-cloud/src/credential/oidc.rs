use std::collections::BTreeMap;
use std::fmt;

use crate::api::{Api, Bearer};
use crate::credential::Obtain;
use crate::error::Result;
use crate::workspace::WorkspaceId;

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
        audience: Option<WorkspaceId>,
    },
    Held(String),
}

/// A platform JWT exchanged for a short-lived penv credential.
#[derive(Clone, PartialEq, Eq)]
pub struct Oidc(Platform);

impl Oidc {
    /// GitHub Actions, then GitLab, then the generic variable.
    pub fn from_env(env: &BTreeMap<String, String>) -> Option<Oidc> {
        let at = |key: &str| env.get(key).filter(|v| !v.is_empty());
        if let (Some(url), Some(request_token)) = (at(GITHUB_URL_VAR), at(GITHUB_TOKEN_VAR)) {
            return Some(Oidc(Platform::GithubActions {
                url: url.clone(),
                request_token: request_token.clone(),
                audience: None,
            }));
        }
        [GITLAB_VAR, GENERIC_VAR]
            .into_iter()
            .find_map(at)
            .map(|token| Oidc(Platform::Held(token.clone())))
    }

    /// True when penv asks the platform for the token, and so names its audience.
    /// A held token's audience was fixed where it was minted.
    pub fn requests_audience(&self) -> bool {
        matches!(self.0, Platform::GithubActions { .. })
    }

    /// The workspace the requested token is for: its id is the audience.
    pub fn for_workspace(mut self, workspace: Option<WorkspaceId>) -> Oidc {
        if let Platform::GithubActions { audience, .. } = &mut self.0 {
            *audience = workspace;
        }
        self
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
            } => api.platform_id_token(
                url,
                request_token,
                audience.as_ref().map(WorkspaceId::as_str),
            )?,
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

#[cfg(test)]
mod tests {
    use super::*;

    fn env(pairs: &[(&str, &str)]) -> BTreeMap<String, String> {
        pairs
            .iter()
            .map(|(k, v)| (k.to_string(), v.to_string()))
            .collect()
    }

    const WORKSPACE: &str = "3f2504e0-4f89-11d3-9a0c-0305e82c3301";

    #[test]
    fn github_wins_over_a_held_token_and_carries_the_workspace_id_as_the_audience() {
        let found = Oidc::from_env(&env(&[
            (
                GITHUB_URL_VAR,
                "https://token.actions.example/?api-version=1",
            ),
            (GITHUB_TOKEN_VAR, "request_FAKE"),
            (GENERIC_VAR, "held_FAKE"),
        ]))
        .expect("a source");
        assert!(found.requests_audience());
        let found = found.for_workspace(Some(WorkspaceId::parse(WORKSPACE).unwrap()));
        assert!(matches!(
            found.0,
            Platform::GithubActions { ref audience, .. }
                if audience.as_ref().map(WorkspaceId::as_str) == Some(WORKSPACE)
        ));
    }

    #[test]
    fn gitlab_and_the_generic_variable_are_both_held_tokens() {
        for var in ["ID_TOKEN", GENERIC_VAR] {
            let found = Oidc::from_env(&env(&[(var, "jwt_FAKE")])).expect(var);
            assert!(matches!(found.0, Platform::Held(_)), "{var}");
            assert!(!found.requests_audience(), "{var}");
        }
    }

    #[test]
    fn only_the_variables_the_design_names_are_read() {
        assert!(Oidc::from_env(&env(&[("CI_JOB_JWT_V2", "jwt_FAKE")])).is_none());
    }

    #[test]
    fn a_job_with_no_token_source_offers_nothing() {
        assert!(Oidc::from_env(&env(&[("CI", "true")])).is_none());
        assert!(Oidc::from_env(&env(&[(GITHUB_URL_VAR, "https://x")])).is_none());
    }

    #[test]
    fn a_platform_token_never_prints_itself() {
        let found = Oidc::from_env(&env(&[(GENERIC_VAR, "jwt_FAKE_NEVER_PRINTED")])).unwrap();
        assert!(!format!("{found:?}").contains("FAKE"));
    }
}
