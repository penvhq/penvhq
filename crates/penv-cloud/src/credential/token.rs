use std::collections::BTreeMap;
use std::fmt;

use crate::api::{Api, Bearer};
use crate::credential::Obtain;
use crate::error::Result;
use crate::keychain::{self, Keychain};

/// How CI and servers pass a `pck_`.
pub const TOKEN_VAR: &str = "PENV_TOKEN";

/// A bearer that is already a bearer: nothing is exchanged.
#[derive(Clone, PartialEq, Eq)]
pub struct Token(String);

impl Token {
    pub fn new(token: impl Into<String>) -> Token {
        Token(token.into())
    }

    pub fn from_env(env: &BTreeMap<String, String>) -> Option<Token> {
        env.get(TOKEN_VAR).filter(|v| !v.is_empty()).map(Token::new)
    }

    /// The `pcu_` a person's login left behind.
    pub fn from_keychain(store: &dyn Keychain) -> Result<Option<Token>> {
        Ok(store
            .get(keychain::USER)?
            .filter(|v| !v.is_empty())
            .map(Token::new))
    }
}

impl fmt::Debug for Token {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("Token(<held>)")
    }
}

impl Obtain for Token {
    fn obtain(&self, _api: &Api, _now: u64) -> Result<Bearer> {
        Ok(Bearer::new(self.0.clone()))
    }

    fn identity(&self) -> Option<String> {
        Some(identity(&self.0))
    }
}

/// A bearer is its own identity, whether it came as a token or in hand.
pub(crate) fn identity(token: &str) -> String {
    format!("token:{token}")
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
    fn an_empty_variable_is_no_credential() {
        assert!(Token::from_env(&env(&[(TOKEN_VAR, "")])).is_none());
        assert!(Token::from_env(&env(&[(TOKEN_VAR, "pck_FAKE")])).is_some());
    }

    #[test]
    fn a_token_never_prints_itself() {
        let shown = format!("{:?}", Token::new("pck_FAKE_NEVER_PRINTED"));
        assert!(!shown.contains("FAKE"), "{shown}");
    }
}
