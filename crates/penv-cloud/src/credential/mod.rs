//! Credential kinds. One file each, one arm of [`resolve`] each; adding a kind
//! adds a file and a line and touches nothing else.

mod aws;
mod aws_container;
mod aws_web_identity;
mod keypair;
mod oidc;
mod token;

use std::collections::BTreeMap;

pub use aws::AwsIam;
pub use aws_container::AwsContainer;
pub use aws_web_identity::AwsWebIdentity;
pub use keypair::{BoundKeypair, Enrolled, MESSAGE_PREFIX, enroll, public_key_der, sign_message};
pub use oidc::Oidc;
pub use token::{TOKEN_VAR, Token};

use crate::api::{Api, Bearer};
use crate::error::{CloudError, Result};
use crate::keychain::Keychain;

/// How one kind proves who this host is.
pub trait Obtain {
    fn obtain(&self, api: &Api, now: u64) -> Result<Bearer>;
}

/// The order the design fixes: the variable, the person's login, the enrolled
/// keypair, the platform's OIDC token, then AWS (keys, web identity, container). `org` is the OIDC audience.
pub fn resolve<'a>(
    env: &BTreeMap<String, String>,
    store: &'a dyn Keychain,
    org: Option<&str>,
) -> Result<Box<dyn Obtain + 'a>> {
    if let Some(token) = Token::from_env(env) {
        return Ok(Box::new(token));
    }
    if let Some(token) = Token::from_keychain(store)? {
        return Ok(Box::new(token));
    }
    if let Some(keypair) = BoundKeypair::from_keychain(store)? {
        return Ok(Box::new(keypair));
    }
    if let Some(oidc) = Oidc::from_env(env, org) {
        return Ok(Box::new(oidc));
    }
    // The AWS SDKs' own order: keys in the environment, web identity (EKS
    // IRSA), then the container endpoint (ECS task roles, EKS Pod Identity).
    if let Some(aws) = AwsIam::from_env(env) {
        return Ok(Box::new(aws));
    }
    if let Some(aws) = AwsWebIdentity::from_env(env) {
        return Ok(Box::new(aws));
    }
    if let Some(aws) = AwsContainer::from_env(env) {
        return Ok(Box::new(aws));
    }
    Err(CloudError::NoCredential)
}

/// True when this host can prove itself at all. Which kind is never said.
pub fn present(env: &BTreeMap<String, String>, store: &dyn Keychain) -> bool {
    resolve(env, store, None).is_ok()
}
