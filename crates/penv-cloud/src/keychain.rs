use std::collections::BTreeMap;
use std::sync::Mutex;

use crate::error::{CloudError, Result};

/// The one service name every penv item is filed under.
pub const SERVICE: &str = "penv";

/// The person's `pcu_` credential.
pub const USER: &str = "user";
/// The 32 bytes the cache is encrypted with.
pub const CACHE_KEY: &str = "cache-key";
/// The enrolled Ed25519 key, its credential id and its generation counter.
pub const KEYPAIR: &str = "keypair";

/// The account one item is stored under. Two servers never share a credential.
pub fn account(base_url: &str, item: &str) -> String {
    format!("{base_url}/{item}")
}

/// One secret store, so the cache and every credential kind can be driven from
/// a map in a test.
pub trait Keychain {
    fn get(&self, item: &str) -> Result<Option<String>>;
    fn set(&self, item: &str, value: &str) -> Result<()>;
    fn delete(&self, item: &str) -> Result<()>;

    /// False where the host has no store. The cache is off there, and every run
    /// goes to the server.
    fn usable(&self) -> bool {
        true
    }
}

/// The OS keychain.
pub struct Keyring {
    base_url: String,
}

impl Keyring {
    /// `None` where the platform store will not open, which is normal in a
    /// container. The caller falls back to [`NoKeychain`].
    pub fn open(base_url: &str) -> Option<Keyring> {
        keyring::Entry::store_status().as_ref().ok()?;
        Some(Keyring {
            base_url: base_url.to_string(),
        })
    }

    fn entry(&self, item: &str) -> Result<keyring::Entry> {
        keyring::Entry::new(SERVICE, &account(&self.base_url, item))
            .map_err(|e| CloudError::Keychain(e.to_string()))
    }
}

impl Keychain for Keyring {
    fn get(&self, item: &str) -> Result<Option<String>> {
        match self.entry(item)?.get_password() {
            Ok(value) => Ok(Some(value)),
            Err(keyring::Error::NoEntry) => Ok(None),
            Err(e) => Err(CloudError::Keychain(e.to_string())),
        }
    }

    fn set(&self, item: &str, value: &str) -> Result<()> {
        self.entry(item)?
            .set_password(value)
            .map_err(|e| CloudError::Keychain(e.to_string()))
    }

    fn delete(&self, item: &str) -> Result<()> {
        match self.entry(item)?.delete_credential() {
            Ok(()) | Err(keyring::Error::NoEntry) => Ok(()),
            Err(e) => Err(CloudError::Keychain(e.to_string())),
        }
    }
}

/// A store that reads the cache key once per process, since every address a
/// command reads asks for it. Every other item is read through each time.
pub struct Remembering<K> {
    inner: K,
    cache_key: Mutex<Option<String>>,
}

impl<K: Keychain> Remembering<K> {
    pub fn new(inner: K) -> Remembering<K> {
        Remembering {
            inner,
            cache_key: Mutex::new(None),
        }
    }

    fn remembered(&self) -> std::sync::MutexGuard<'_, Option<String>> {
        self.cache_key
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
    }
}

impl<K: Keychain> Keychain for Remembering<K> {
    fn get(&self, item: &str) -> Result<Option<String>> {
        if item != CACHE_KEY {
            return self.inner.get(item);
        }
        let mut remembered = self.remembered();
        if remembered.is_none() {
            *remembered = self.inner.get(item)?;
        }
        Ok(remembered.clone())
    }

    fn set(&self, item: &str, value: &str) -> Result<()> {
        self.inner.set(item, value)?;
        if item == CACHE_KEY {
            *self.remembered() = Some(value.to_string());
        }
        Ok(())
    }

    fn delete(&self, item: &str) -> Result<()> {
        self.inner.delete(item)?;
        if item == CACHE_KEY {
            *self.remembered() = None;
        }
        Ok(())
    }

    fn usable(&self) -> bool {
        self.inner.usable()
    }
}

/// A store that holds nothing. Reads answer empty so credential resolution walks
/// on; writes say why they cannot happen.
pub struct NoKeychain;

impl Keychain for NoKeychain {
    fn get(&self, _item: &str) -> Result<Option<String>> {
        Ok(None)
    }

    fn set(&self, _item: &str, _value: &str) -> Result<()> {
        Err(CloudError::Keychain(
            "this host has no keychain to write to".into(),
        ))
    }

    fn delete(&self, _item: &str) -> Result<()> {
        Ok(())
    }

    fn usable(&self) -> bool {
        false
    }
}

/// A store in memory, for tests.
#[derive(Default)]
pub struct MemoryKeychain(Mutex<BTreeMap<String, String>>);

impl MemoryKeychain {
    pub fn new() -> MemoryKeychain {
        MemoryKeychain::default()
    }

    pub fn items(&self) -> BTreeMap<String, String> {
        self.0.lock().expect("the test keychain").clone()
    }
}

impl Keychain for MemoryKeychain {
    fn get(&self, item: &str) -> Result<Option<String>> {
        Ok(self.0.lock().expect("the test keychain").get(item).cloned())
    }

    fn set(&self, item: &str, value: &str) -> Result<()> {
        self.0
            .lock()
            .expect("the test keychain")
            .insert(item.to_string(), value.to_string());
        Ok(())
    }

    fn delete(&self, item: &str) -> Result<()> {
        self.0.lock().expect("the test keychain").remove(item);
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn an_item_is_filed_under_the_server_it_belongs_to() {
        assert_eq!(
            account("https://penv.cloud", USER),
            "https://penv.cloud/user"
        );
        assert_ne!(
            account("https://penv.cloud", USER),
            account("http://127.0.0.1:8080", USER)
        );
    }

    /// Counts the reads that reach the store.
    struct Counted(MemoryKeychain, Mutex<usize>);

    impl Keychain for Counted {
        fn get(&self, item: &str) -> Result<Option<String>> {
            *self.1.lock().unwrap() += 1;
            self.0.get(item)
        }
        fn set(&self, item: &str, value: &str) -> Result<()> {
            self.0.set(item, value)
        }
        fn delete(&self, item: &str) -> Result<()> {
            self.0.delete(item)
        }
    }

    #[test]
    fn the_cache_key_is_read_from_the_store_once_per_process() {
        let inner = MemoryKeychain::new();
        inner.set(CACHE_KEY, "key_FAKE").unwrap();
        inner.set(USER, "pcu_FAKE").unwrap();
        let store = Remembering::new(Counted(inner, Mutex::new(0)));
        for _ in 0..3 {
            assert_eq!(store.get(CACHE_KEY).unwrap().as_deref(), Some("key_FAKE"));
        }
        assert_eq!(*store.inner.1.lock().unwrap(), 1);
        for _ in 0..2 {
            store.get(USER).unwrap();
        }
        assert_eq!(
            *store.inner.1.lock().unwrap(),
            3,
            "other items read through"
        );

        store.delete(CACHE_KEY).unwrap();
        assert_eq!(store.get(CACHE_KEY).unwrap(), None, "a deleted key is gone");
        store.set(CACHE_KEY, "key_NEW_FAKE").unwrap();
        assert_eq!(
            store.get(CACHE_KEY).unwrap().as_deref(),
            Some("key_NEW_FAKE")
        );
    }

    #[test]
    fn a_host_with_no_keychain_reads_empty_and_refuses_to_write() {
        let store = NoKeychain;
        assert_eq!(store.get(USER).unwrap(), None);
        assert!(!store.usable());
        assert!(store.set(USER, "pcu_FAKE").is_err());
    }
}
