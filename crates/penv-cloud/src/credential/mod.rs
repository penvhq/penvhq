//! Credential kinds, one file each. A kind names the places it is found in its
//! `PLACES` and `build.rs` declares every file here, so adding a kind adds a
//! file and touches nothing else.

include!(concat!(env!("OUT_DIR"), "/kinds.rs"));

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

/// A kind read from the environment. `org` is the OIDC audience and the
/// workspace an AWS login signs.
type FromEnv = fn(&BTreeMap<String, String>, Option<&str>) -> Option<Box<dyn Obtain>>;

/// A kind this machine holds for the server. `lock_dir` is where parallel
/// keypair exchanges queue; an error is a keychain that would not answer.
type FromKeychain =
    for<'a> fn(&'a dyn Keychain, Option<PathBuf>) -> Result<Option<Box<dyn Obtain + 'a>>>;

/// Where one kind is looked for.
enum Find {
    Env(FromEnv),
    Held(FromKeychain),
}

/// One place a kind is found. Every lookup tries the places lowest rank first.
struct Place {
    rank: u32,
    find: Find,
}

/// Every kind's places by rank. The order the design fixes: the variable, the
/// person's login, the enrolled keypair, the platform's OIDC token, then AWS
/// (keys, web identity, container).
fn order() -> Vec<&'static Place> {
    let mut places: Vec<&'static Place> = KINDS.iter().flat_map(|kind| kind.iter()).collect();
    places.sort_by_key(|place| place.rank);
    places
}

/// The first place in the order that holds a credential. A keychain that will
/// not answer is passed over, and named only when nothing else applies.
pub fn resolve<'a>(
    env: &BTreeMap<String, String>,
    store: &'a dyn Keychain,
    org: Option<&str>,
) -> Result<Box<dyn Obtain + 'a>> {
    first(order(), env, store, org, crate::cache::cache_dir(env))
}

/// Only the places this machine holds for the server, in the same order.
/// `lock_dir` is where parallel keypair exchanges queue.
pub fn resolve_held(
    store: &dyn Keychain,
    lock_dir: Option<PathBuf>,
) -> Result<Box<dyn Obtain + '_>> {
    let held = order()
        .into_iter()
        .filter(|place| matches!(place.find, Find::Held(_)));
    first(held, &BTreeMap::new(), store, None, lock_dir)
}

fn first<'a>(
    places: impl IntoIterator<Item = &'static Place>,
    env: &BTreeMap<String, String>,
    store: &'a dyn Keychain,
    org: Option<&str>,
    lock_dir: Option<PathBuf>,
) -> Result<Box<dyn Obtain + 'a>> {
    let mut unreadable = None;
    for place in places {
        let found = match place.find {
            Find::Env(find) => find(env, org),
            Find::Held(find) => find(store, lock_dir.clone()).unwrap_or_else(|e| {
                unreadable = unreadable.take().or(Some(e));
                None
            }),
        };
        if let Some(found) = found {
            return Ok(found);
        }
    }
    Err(unreadable.unwrap_or(CloudError::NoCredential))
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
    fn the_places_keep_the_order_the_design_fixes_and_no_two_share_a_rank() {
        let fixed = [
            token::PLACES[0].rank,
            token::PLACES[1].rank,
            keypair::PLACES[0].rank,
            oidc::PLACES[0].rank,
            aws::PLACES[0].rank,
            aws_web_identity::PLACES[0].rank,
            aws_container::PLACES[0].rank,
        ];
        assert!(fixed.is_sorted(), "{fixed:?}");
        let mut ranks: Vec<u32> = order().iter().map(|place| place.rank).collect();
        ranks.dedup();
        assert_eq!(ranks.len(), order().len(), "two places share a rank");
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
