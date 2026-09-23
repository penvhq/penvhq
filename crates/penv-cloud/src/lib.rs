//! penv.cloud client: credential kinds, encrypted cache, keychain.

pub mod api;
pub mod b64;
pub mod cache;
pub mod clock;
pub mod credential;
pub mod error;
pub mod fetch;
pub mod keychain;
pub mod provider;
pub mod signature;
pub mod tls;

pub use api::{
    AGENT_HEADER, Address, Api, Approval, Bearer, Challenge, CloudKey, DEFAULT_BASE_URL,
    DevicePoll, DeviceStart, Enrolment, EnvBody, Fetched, Freshness, Grant, KeypairGrant, Org,
    Project, PutResult, Requested, Revealed, SESSION_HEADER, SetResult, SignedRequest, Stamp,
    URL_VAR, UnsetResult, User, encode_segment, host_name, without_nulls,
};
pub use cache::{Cache, Entry, Resolved, Source, cache_dir, fetch, ttl_for};
pub use clock::{Clock, Fixed, SystemClock, epoch_from_rfc3339};
pub use credential::{
    AwsIam, BoundKeypair, Enrolled, Obtain, Oidc, TOKEN_VAR, Token, present, resolve,
};
pub use error::{ApiError, CloudError, Result};
pub use fetch::sha256_hex;
pub use keychain::{Keychain, Keyring, MemoryKeychain, NoKeychain};

/// Bytes from the operating system's generator, for values penv generates
/// (`random()` in a schema). The same source the credential and cache use.
pub fn random_bytes(len: usize) -> std::result::Result<Vec<u8>, String> {
    let mut bytes = vec![0u8; len];
    getrandom::fill(&mut bytes).map_err(|e| e.to_string())?;
    Ok(bytes)
}
