//! Credential kinds. One file each, one arm of [`resolve`] each; adding a kind
//! adds a file and a line and touches nothing else.

mod aws;
mod aws_container;
mod aws_web_identity;
mod keypair;
mod oidc;
mod token;

use std::collections::BTreeMap;
use std::path::PathBuf;

pub use aws::AwsIam;
pub use aws_container::AwsContainer;
pub use aws_web_identity::AwsWebIdentity;
pub use keypair::{
    BoundKeypair, Enrolled, LOCK_FILE, MESSAGE_PREFIX, enroll, public_key_der, sign_message,
};
pub use oidc::Oidc;
pub use token::{TOKEN_VAR, Token};

use crate::api::{Api, Bearer};
use crate::error::{CloudError, Result};
use crate::keychain::Keychain;

/// How one kind proves who this host is.
pub trait Obtain {
    fn obtain(&self, api: &Api, now: u64) -> Result<Bearer>;

    /// Who this is, from what the host holds and without asking the network,
    /// stable from one run to the next. The cache is sealed against it, so it
    /// carries the proof itself (only ever hashed), never a name anyone could
    /// claim. `None` means there is no such thing and no cache.
    fn identity(&self) -> Option<String> {
        None
    }
}

/// A bearer already in hand proves itself.
impl Obtain for Bearer {
    fn obtain(&self, _api: &Api, _now: u64) -> Result<Bearer> {
        Ok(self.clone())
    }

    fn identity(&self) -> Option<String> {
        Some(token::identity(&self.token))
    }
}

/// The order the design fixes: the variable, the person's login, the enrolled
/// keypair, the platform's OIDC token, then AWS (keys, web identity, container).
/// `org` is the OIDC audience and the workspace an AWS login signs. A keychain that will not answer is passed over,
/// and named only when nothing else applies.
pub fn resolve<'a>(
    env: &BTreeMap<String, String>,
    store: &'a dyn Keychain,
    org: Option<&str>,
) -> Result<Box<dyn Obtain + 'a>> {
    if let Some(token) = Token::from_env(env) {
        return Ok(Box::new(token));
    }
    let (held, unreadable) = held(store, crate::cache::cache_dir(env));
    if let Some(held) = held {
        return Ok(held);
    }
    if let Some(oidc) = Oidc::from_env(env, org) {
        return Ok(Box::new(oidc));
    }
    // The AWS SDKs' own order: keys in the environment, web identity (EKS
    // IRSA), then the container endpoint (ECS task roles, EKS Pod Identity).
    if let Some(aws) = AwsIam::from_env(env) {
        return Ok(Box::new(aws.for_org(org)));
    }
    if let Some(aws) = AwsWebIdentity::from_env(env) {
        return Ok(Box::new(aws.for_org(org)));
    }
    if let Some(aws) = AwsContainer::from_env(env) {
        return Ok(Box::new(aws.for_org(org)));
    }
    Err(unreadable.unwrap_or(CloudError::NoCredential))
}

/// Only what this machine holds for the server: a person's login, then an
/// enrolled keypair. `lock_dir` is where parallel keypair exchanges queue.
pub fn resolve_held(
    store: &dyn Keychain,
    lock_dir: Option<PathBuf>,
) -> Result<Box<dyn Obtain + '_>> {
    match held(store, lock_dir) {
        (Some(held), _) => Ok(held),
        (None, unreadable) => Err(unreadable.unwrap_or(CloudError::NoCredential)),
    }
}

/// What the keychain holds, and the error it answered with if it would not.
fn held(
    store: &dyn Keychain,
    lock_dir: Option<PathBuf>,
) -> (Option<Box<dyn Obtain + '_>>, Option<CloudError>) {
    let mut unreadable = None;
    match Token::from_keychain(store) {
        Ok(Some(token)) => return (Some(Box::new(token)), None),
        Ok(None) => {}
        Err(e) => unreadable = Some(e),
    }
    match BoundKeypair::from_keychain(store) {
        Ok(Some(keypair)) => return (Some(Box::new(keypair.locked_in(lock_dir))), None),
        Ok(None) => {}
        Err(e) => unreadable = unreadable.or(Some(e)),
    }
    (None, unreadable)
}

/// True when this host can prove itself at all. Which kind is never said.
pub fn present(env: &BTreeMap<String, String>, store: &dyn Keychain) -> bool {
    resolve(env, store, None).is_ok()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::keychain::{self, MemoryKeychain};

    /// A keychain that refuses every read, as a locked one does.
    struct Locked;

    impl Keychain for Locked {
        fn get(&self, _item: &str) -> Result<Option<String>> {
            Err(CloudError::Keychain("the store is locked".into()))
        }
        fn set(&self, _item: &str, _value: &str) -> Result<()> {
            Err(CloudError::Keychain("the store is locked".into()))
        }
        fn delete(&self, _item: &str) -> Result<()> {
            Ok(())
        }
    }

    fn env(pairs: &[(&str, &str)]) -> BTreeMap<String, String> {
        pairs
            .iter()
            .map(|(k, v)| (k.to_string(), v.to_string()))
            .collect()
    }

    #[test]
    fn a_keychain_that_will_not_answer_leaves_the_rest_of_the_order_to_try() {
        let oidc = env(&[("PENV_OIDC_TOKEN", "jwt_FAKE")]);
        assert!(resolve(&oidc, &Locked, None).is_ok(), "OIDC is still tried");
        let aws = env(&[
            ("AWS_ACCESS_KEY_ID", "AKIAFAKE"),
            ("AWS_SECRET_ACCESS_KEY", "secretFAKE"),
        ]);
        assert!(resolve(&aws, &Locked, None).is_ok(), "AWS is still tried");
    }

    #[test]
    fn with_nothing_else_the_keychain_s_own_error_is_the_answer() {
        assert!(matches!(
            resolve(&env(&[]), &Locked, None),
            Err(CloudError::Keychain(_))
        ));
        assert!(matches!(
            resolve_held(&Locked, None),
            Err(CloudError::Keychain(_))
        ));
        assert!(matches!(
            resolve_held(&MemoryKeychain::new(), None),
            Err(CloudError::NoCredential)
        ));
    }

    #[test]
    fn an_unreadable_keypair_does_not_hide_the_kinds_after_it() {
        let store = MemoryKeychain::new();
        store.set(keychain::KEYPAIR, "not json").unwrap();
        let oidc = env(&[("PENV_OIDC_TOKEN", "jwt_FAKE")]);
        assert!(resolve(&oidc, &store, None).is_ok());
    }

    #[test]
    fn an_identity_is_stable_where_the_host_holds_the_proof_and_absent_otherwise() {
        let token = Token::new("pck_FAKE");
        assert_eq!(token.identity(), Token::new("pck_FAKE").identity());
        assert_eq!(token.identity(), Bearer::new("pck_FAKE").identity());
        assert_ne!(token.identity(), Token::new("pck_OTHER").identity());

        let aws = |secret: &str| AwsIam::new("AKIAFAKE", secret, None, "us-east-1").identity();
        assert_eq!(aws("secretFAKE"), aws("secretFAKE"));
        assert_ne!(
            aws("secretFAKE"),
            aws("secretOTHER"),
            "the key id alone is not proof"
        );
        assert_ne!(aws("secretFAKE"), token.identity());

        let held = Oidc::from_env(&env(&[("PENV_OIDC_TOKEN", "jwt_FAKE")]), None).unwrap();
        assert!(held.identity().is_some());
        let github = Oidc::from_env(
            &env(&[
                ("ACTIONS_ID_TOKEN_REQUEST_URL", "https://token.example/"),
                ("ACTIONS_ID_TOKEN_REQUEST_TOKEN", "request_FAKE"),
            ]),
            None,
        )
        .unwrap();
        assert_eq!(github.identity(), None, "its token is only a request away");
    }
}
