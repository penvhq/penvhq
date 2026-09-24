use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use chacha20poly1305::aead::{Aead, KeyInit, Payload};
use chacha20poly1305::{ChaCha20Poly1305, Nonce};
use serde::{Deserialize, Serialize};

use crate::api::{Address, Api, EnvBody, Fetched, Freshness};
use crate::b64;
use crate::credential::Obtain;
use crate::error::{CloudError, Result};
use crate::fetch::sha256_hex;
use crate::keychain::{self, Keychain};

/// The environment a stale cache may still answer for.
pub const DEV_ENVIRONMENT: &str = "development";
/// How long a development answer stays fresh without asking the server.
pub const DEV_TTL_SECS: u64 = 60;
/// How often an offline development run says it is running on the cache.
pub const OFFLINE_WARNING_SECS: u64 = 86_400;

const MAGIC: &[u8; 6] = b"penvc1";
const NONCE_LEN: usize = 12;

/// dev is 60 seconds; every other environment revalidates every time.
pub fn ttl_for(environment: &str) -> u64 {
    if environment == DEV_ENVIRONMENT {
        DEV_TTL_SECS
    } else {
        0
    }
}

/// The platform cache directory penv keeps its files under.
pub fn cache_dir(env: &BTreeMap<String, String>) -> Option<PathBuf> {
    let at = |key: &str| env.get(key).filter(|v| !v.is_empty()).map(PathBuf::from);
    if cfg!(windows) {
        return Some(at("LOCALAPPDATA")?.join("penv").join("cache"));
    }
    if cfg!(target_os = "macos") {
        return Some(at("HOME")?.join("Library").join("Caches").join("penv"));
    }
    at("XDG_CACHE_HOME")
        .or_else(|| at("HOME").map(|home| home.join(".cache")))
        .map(|dir| dir.join("penv"))
}

/// One GET body, the ETag it came with and when it arrived.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Entry {
    #[serde(default)]
    pub etag: Option<String>,
    pub fetched_at: u64,
    pub body: EnvBody,
}

impl Entry {
    /// A stamp from the future is a clock that moved back, never fresh.
    pub fn fresh(&self, ttl: u64, now: u64) -> bool {
        ttl > 0 && now >= self.fetched_at && now - self.fetched_at < ttl
    }
}

/// Where the answer came from.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Source {
    Cache,
    Server,
}

/// The values `run` will inject, and how they were arrived at.
#[derive(Debug, Clone)]
pub struct Resolved {
    pub body: EnvBody,
    pub etag: Option<String>,
    pub fetched_at: u64,
    pub source: Source,
    /// The once-a-day line an offline development run prints.
    pub offline_warning: bool,
}

/// One environment's encrypted file. The key lives in the keychain, so a host
/// without one has no cache at all.
pub struct Cache {
    path: PathBuf,
    stamp: PathBuf,
    key: [u8; 32],
    address: Address,
    aad: String,
}

impl Cache {
    /// `None` where there is no keychain, or the credential has no identity to
    /// seal against without asking the network: the cache is off and every run
    /// is online.
    pub fn open(
        dir: &Path,
        base_url: &str,
        address: &Address,
        credential: &dyn Obtain,
        store: &dyn Keychain,
    ) -> Result<Option<Cache>> {
        let Some(identity) = credential.identity().filter(|_| store.usable()) else {
            return Ok(None);
        };
        let key = cache_key(store)?;
        let dir = dir_for(dir, base_url);
        let name = name_of(base_url, address);
        Ok(Some(Cache {
            path: dir.join(format!("{name}.bin")),
            stamp: dir.join(format!("{name}.warned")),
            key,
            address: address.clone(),
            aad: aad_of(base_url, address, &identity),
        }))
    }

    pub fn path(&self) -> &Path {
        &self.path
    }

    /// A file that will not open reads as no cache: a rotated key is not an error.
    pub fn read(&self) -> Option<Entry> {
        let sealed = std::fs::read(&self.path).ok()?;
        let plain = unseal(&self.key, &self.aad, &sealed).ok()?;
        serde_json::from_slice(&plain).ok()
    }

    pub fn write(&self, entry: &Entry) -> Result<()> {
        if let Some(parent) = self.path.parent() {
            std::fs::create_dir_all(parent)
                .map_err(|e| CloudError::Cache(format!("written: {e}")))?;
        }
        let plain =
            serde_json::to_vec(entry).map_err(|e| CloudError::Cache(format!("written: {e}")))?;
        let sealed = seal(&self.key, &self.aad, &plain)?;
        write_private(&self.path, &sealed).map_err(|e| CloudError::Cache(format!("written: {e}")))
    }

    /// The entry when it is still inside the TTL, and nothing otherwise.
    pub fn fresh(&self, ttl: u64, now: u64) -> Option<Entry> {
        self.read().filter(|entry| entry.fresh(ttl, now))
    }

    pub fn age(&self, now: u64) -> Option<u64> {
        self.read().map(|e| now.saturating_sub(e.fetched_at))
    }

    /// Design section 5: fresh, else HEAD with the ETag, else GET; offline falls
    /// back to the cache in development only, and fails closed everywhere else.
    /// The credential is proved only once the server has to be asked, so a fresh
    /// or offline answer costs no exchange.
    pub fn revalidate(&self, api: &Api, credential: &dyn Obtain, now: u64) -> Result<Resolved> {
        let stored = self.read();
        if let Some(entry) = stored
            .as_ref()
            .filter(|entry| entry.fresh(ttl_for(&self.address.environment), now))
        {
            return Ok(answer(entry.clone(), Source::Cache, false));
        }

        match self.ask_server(api, credential, stored.as_ref(), now) {
            Ok(resolved) => Ok(resolved),
            Err(error) if error.is_offline() => match stored {
                Some(entry) if self.address.environment == DEV_ENVIRONMENT => {
                    Ok(answer(entry, Source::Cache, self.warn_once_a_day(now)))
                }
                _ => Err(error),
            },
            Err(error) => Err(error),
        }
    }

    fn ask_server(
        &self,
        api: &Api,
        credential: &dyn Obtain,
        stored: Option<&Entry>,
        now: u64,
    ) -> Result<Resolved> {
        let bearer = &credential.obtain(api, now)?;
        let etag = stored.and_then(|entry| entry.etag.clone());
        if let (Some(etag), Some(stored)) = (&etag, stored)
            && api.env_head(bearer, &self.address, Some(etag))? == Freshness::Unchanged
        {
            let refreshed = Entry {
                fetched_at: now,
                ..stored.clone()
            };
            self.write(&refreshed)?;
            return Ok(answer(refreshed, Source::Server, false));
        }
        match api.env_get(bearer, &self.address, None, true)? {
            Fetched::Body { etag, body } => {
                let entry = Entry {
                    etag,
                    fetched_at: now,
                    body,
                };
                self.write(&entry)?;
                Ok(answer(entry, Source::Server, false))
            }
            Fetched::NotModified => match stored {
                Some(entry) => Ok(answer(entry.clone(), Source::Cache, false)),
                None => Err(CloudError::Unreadable {
                    url: self.address.to_string(),
                    reason: "304 for a request that carried no ETag".into(),
                }),
            },
        }
    }

    /// True the first time today, and stamps the day so the next run is quiet.
    fn warn_once_a_day(&self, now: u64) -> bool {
        let last = std::fs::read_to_string(&self.stamp)
            .ok()
            .and_then(|text| text.trim().parse::<u64>().ok());
        if last.is_some_and(|at| now.saturating_sub(at) < OFFLINE_WARNING_SECS) {
            return false;
        }
        if let Some(parent) = self.stamp.parent() {
            let _ = std::fs::create_dir_all(parent);
        }
        let _ = std::fs::write(&self.stamp, now.to_string());
        true
    }
}

/// The values for an address, with or without a cache to answer from.
pub fn fetch(
    api: &Api,
    credential: &dyn Obtain,
    address: &Address,
    cache: Option<&Cache>,
    now: u64,
) -> Result<Resolved> {
    match cache {
        Some(cache) => cache.revalidate(api, credential, now),
        None => match api.env_get(&credential.obtain(api, now)?, address, None, true)? {
            Fetched::Body { etag, body } => Ok(Resolved {
                body,
                etag,
                fetched_at: now,
                source: Source::Server,
                offline_warning: false,
            }),
            Fetched::NotModified => Err(CloudError::Unreadable {
                url: address.to_string(),
                reason: "304 for a request that carried no ETag".into(),
            }),
        },
    }
}

fn answer(entry: Entry, source: Source, offline_warning: bool) -> Resolved {
    Resolved {
        body: entry.body,
        etag: entry.etag,
        fetched_at: entry.fetched_at,
        source,
        offline_warning,
    }
}

/// The 32 bytes every cache file on this host is sealed with, made once.
fn cache_key(store: &dyn Keychain) -> Result<[u8; 32]> {
    if let Some(stored) = store.get(keychain::CACHE_KEY)?
        && let Some(bytes) = b64::decode(&stored)
        && let Ok(key) = <[u8; 32]>::try_from(bytes.as_slice())
    {
        return Ok(key);
    }
    let mut key = [0u8; 32];
    getrandom::fill(&mut key).map_err(|e| CloudError::Cache(format!("keyed: {e}")))?;
    store.set(keychain::CACHE_KEY, &b64::encode(&key))?;
    Ok(key)
}

/// One server's files, so signing out can take that server's cache with it.
pub fn dir_for(dir: &Path, base_url: &str) -> PathBuf {
    dir.join(&sha256_hex(base_url.as_bytes())[..32])
}

/// Everything cached for one server. Signing out leaves nothing behind.
pub fn forget(dir: &Path, base_url: &str) {
    let _ = std::fs::remove_dir_all(dir_for(dir, base_url));
}

/// The file name: one hash of the server and the address, so no path on disk
/// names a project. The address is the encoded one, where no `/` inside a
/// segment can pass for the line between two.
fn name_of(base_url: &str, address: &Address) -> String {
    sha256_hex(format!("{base_url}|{}", address.path()).as_bytes())
}

/// The server, the address and the credential's identity, bound into the
/// ciphertext: a file cannot be moved between environments, and another
/// credential on this host cannot open it.
pub fn aad_of(base_url: &str, address: &Address, identity: &str) -> String {
    format!(
        "{base_url}|{}|{}",
        address.path(),
        sha256_hex(identity.as_bytes())
    )
}

/// A file only this account can read, swapped in whole: a reader sees the old
/// file or the new one, never half of one. Windows keeps the directory's own
/// ACL, which is the user's profile and no one else.
fn write_private(path: &Path, bytes: &[u8]) -> std::io::Result<()> {
    use std::io::Write;

    let mut suffix = [0u8; 8];
    getrandom::fill(&mut suffix).map_err(std::io::Error::other)?;
    let mut name = path.as_os_str().to_owned();
    name.push(format!(".{}.tmp", &sha256_hex(&suffix)[..16]));
    let temp = PathBuf::from(name);

    let mut options = std::fs::OpenOptions::new();
    options.write(true).create_new(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.mode(0o600);
    }
    let written = options
        .open(&temp)
        .and_then(|mut file| file.write_all(bytes).and_then(|()| file.sync_all()))
        .and_then(|()| std::fs::rename(&temp, path));
    if written.is_err() {
        let _ = std::fs::remove_file(&temp);
    }
    written
}

pub fn seal(key: &[u8; 32], aad: &str, plaintext: &[u8]) -> Result<Vec<u8>> {
    let cipher = ChaCha20Poly1305::new_from_slice(key)
        .map_err(|e| CloudError::Cache(format!("sealed: {e}")))?;
    let mut nonce = [0u8; NONCE_LEN];
    getrandom::fill(&mut nonce).map_err(|e| CloudError::Cache(format!("sealed: {e}")))?;
    let ciphertext = cipher
        .encrypt(
            &Nonce::from(nonce),
            Payload {
                msg: plaintext,
                aad: aad.as_bytes(),
            },
        )
        .map_err(|e| CloudError::Cache(format!("sealed: {e}")))?;
    let mut out = Vec::with_capacity(MAGIC.len() + NONCE_LEN + ciphertext.len());
    out.extend_from_slice(MAGIC);
    out.extend_from_slice(&nonce);
    out.extend_from_slice(&ciphertext);
    Ok(out)
}

pub fn unseal(key: &[u8; 32], aad: &str, sealed: &[u8]) -> Result<Vec<u8>> {
    let head = MAGIC.len() + NONCE_LEN;
    if sealed.len() <= head || &sealed[..MAGIC.len()] != MAGIC {
        return Err(CloudError::Cache("read: not a penv cache file".into()));
    }
    let nonce = <[u8; NONCE_LEN]>::try_from(&sealed[MAGIC.len()..head])
        .map_err(|_| CloudError::Cache("read: the nonce is short".into()))?;
    let cipher = ChaCha20Poly1305::new_from_slice(key)
        .map_err(|e| CloudError::Cache(format!("read: {e}")))?;
    cipher
        .decrypt(
            &Nonce::from(nonce),
            Payload {
                msg: &sealed[head..],
                aad: aad.as_bytes(),
            },
        )
        .map_err(|_| CloudError::Cache("read: the file does not belong to this address".into()))
}

#[cfg(test)]
mod tests {
    use super::*;

    const KEY: [u8; 32] = [7u8; 32];

    fn bound(environment: &str, token: &str) -> String {
        aad_of(
            "https://penv.cloud",
            &Address::new("acme", "api", environment),
            token,
        )
    }

    #[test]
    fn a_sealed_payload_comes_back_only_under_its_own_address() {
        let aad = bound("development", "pcu_FAKE");
        let sealed = seal(&KEY, &aad, b"{\"keys\":[]}").unwrap();
        assert_eq!(unseal(&KEY, &aad, &sealed).unwrap(), b"{\"keys\":[]}");

        assert!(
            unseal(&KEY, &bound("production", "pcu_FAKE"), &sealed).is_err(),
            "the AAD must bind the address"
        );
        assert!(
            unseal(&[9u8; 32], &aad, &sealed).is_err(),
            "the key must bind"
        );
    }

    #[test]
    fn another_credential_on_this_host_cannot_open_the_cache() {
        let sealed = seal(&KEY, &bound("development", "pcu_MINE"), b"{\"keys\":[]}").unwrap();
        assert!(
            unseal(&KEY, &bound("development", "pcu_THEIRS"), &sealed).is_err(),
            "the AAD must bind the credential"
        );
    }

    #[test]
    fn a_nonce_is_new_on_every_write() {
        let aad = bound("development", "pcu_FAKE");
        let first = seal(&KEY, &aad, b"same").unwrap();
        let second = seal(&KEY, &aad, b"same").unwrap();
        assert_ne!(first, second);
    }

    #[test]
    fn only_development_holds_a_value_between_runs() {
        assert_eq!(ttl_for("development"), DEV_TTL_SECS);
        assert_eq!(ttl_for("staging"), 0);
        assert_eq!(ttl_for("production"), 0);
    }

    #[test]
    fn freshness_is_the_ttl_and_nothing_else() {
        let entry = Entry {
            etag: None,
            fetched_at: 1_000,
            body: EnvBody::default(),
        };
        assert!(entry.fresh(60, 1_059));
        assert!(!entry.fresh(60, 1_060));
        assert!(!entry.fresh(0, 1_000), "a zero TTL is never fresh");
        assert!(
            !entry.fresh(60, 999),
            "an entry stamped after now is not fresh"
        );
        assert!(!entry.fresh(60, 0));
    }

    #[test]
    fn a_slash_inside_a_segment_is_not_the_line_between_two() {
        let one = Address::new("a", "b/c", "d");
        let two = Address::new("a", "b", "c/d");
        assert_ne!(
            name_of("https://penv.cloud", &one),
            name_of("https://penv.cloud", &two)
        );
        assert_ne!(
            aad_of("https://penv.cloud", &one, "token:pck_FAKE"),
            aad_of("https://penv.cloud", &two, "token:pck_FAKE")
        );
    }

    #[test]
    fn a_cached_entry_never_prints_its_values() {
        let body: EnvBody = serde_json::from_str(
            r#"{"keys":[{"name":"STRIPE_SECRET_KEY","value":"sk_test_FAKE0000"}]}"#,
        )
        .unwrap();
        let entry = Entry {
            etag: None,
            fetched_at: 1_000,
            body,
        };
        let resolved = answer(entry.clone(), Source::Cache, false);
        for shown in [format!("{entry:?}"), format!("{resolved:?}")] {
            assert!(!shown.contains("FAKE"), "{shown}");
            assert!(shown.contains("STRIPE_SECRET_KEY"), "{shown}");
        }
    }

    #[test]
    fn the_file_name_names_no_project() {
        let name = name_of(
            "https://penv.cloud",
            &Address::new("acme", "api", "development"),
        );
        assert_eq!(name.len(), 64);
        assert!(!name.contains("acme"));
    }
}
