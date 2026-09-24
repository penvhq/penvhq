//! Values encrypted at rest in `.env` files: `KEY=enc:v1:<base64>`. Off by
//! default; `[local] encrypt = true` (or `penv encrypt`) turns it on. penv
//! decrypts wherever it reads a value file, whatever the setting,
//! so `run`, `ls`, `check` and `push` see the value and nothing else changes.
//!
//! The key is 32 random bytes, one per machine user, kept in the OS keychain.
//! `PENV_LOCAL_KEY` (64 hex characters) supplies it instead, for a machine with
//! no keychain; where there is neither, a key file under the user's config
//! folder, mode 600, holds it. The key name is the associated data, so an
//! encrypted value moved to another key does not decrypt.

use std::path::PathBuf;

use crate::error::{CliError, Exit};

pub const PREFIX: &str = "enc:v1:";
const ITEM: &str = "local-key";
const KEYCHAIN_BASE: &str = "local";
pub const KEY_VAR: &str = "PENV_LOCAL_KEY";

/// What to write for `name` into a value file under `dir`: the value encrypted
/// when `[local] encrypt` is on and the key is sensitive, else the value as it is.
pub fn stored(
    dir: &std::path::Path,
    name: &str,
    value: &str,
    sensitive: bool,
) -> Result<String, CliError> {
    let on = crate::config::Config::load(dir)
        .map(|c| c.encrypt())
        .unwrap_or(false);
    stored_as(on, name, value, sensitive)
}

/// [`stored`], for a caller writing many values that read the setting once.
pub fn stored_as(on: bool, name: &str, value: &str, sensitive: bool) -> Result<String, CliError> {
    if !on || !sensitive || value.is_empty() {
        return Ok(value.to_string());
    }
    match key(true)? {
        Some(k) => encrypt(&k, name, value),
        None => Ok(value.to_string()),
    }
}

pub fn is_encrypted(text: &str) -> bool {
    text.starts_with(PREFIX)
}

pub fn encrypt(key: &[u8; 32], name: &str, value: &str) -> Result<String, CliError> {
    let sealed = penv_cloud::cache::seal(key, name, value.as_bytes()).map_err(|e| {
        CliError::new(
            "encrypt_failed",
            format!("{name} could not be encrypted: {e}."),
            "Try again.",
        )
    })?;
    Ok(format!("{PREFIX}{}", penv_cloud::b64::encode(&sealed)))
}

pub fn decrypt(key: &[u8; 32], name: &str, text: &str, file: &str) -> Result<String, CliError> {
    let fail = || {
        CliError::new(
            "decrypt_failed",
            format!("{name} in {file} is encrypted with a key this machine does not hold."),
            format!("It was written on another machine or by another user. Set it again with penv set {name}, or supply that key in {KEY_VAR}."),
        )
        .with_exit(Exit::Validation)
    };
    let body = text.strip_prefix(PREFIX).ok_or_else(fail)?;
    let sealed = penv_cloud::b64::decode(body.trim()).ok_or_else(fail)?;
    let plain = penv_cloud::cache::unseal(key, name, &sealed).map_err(|_| fail())?;
    String::from_utf8(plain).map_err(|_| fail())
}

/// The keychain's or the key file's answer, once found: a keychain can take a
/// noticeable while to ask, and every encrypted file needs the key.
static FOUND: std::sync::Mutex<Option<[u8; 32]>> = std::sync::Mutex::new(None);

/// This user's key. With `create`, one is made and stored the first time.
/// `Ok(None)` when none exists and `create` is false.
pub fn key(create: bool) -> Result<Option<[u8; 32]>, CliError> {
    if let Ok(hex) = std::env::var(KEY_VAR)
        && !hex.is_empty()
    {
        return parse(&hex).map(Some).ok_or_else(|| {
            CliError::new(
                "invalid_local_key",
                format!("{KEY_VAR} is not 64 hexadecimal characters."),
                                format!("Unset {KEY_VAR}, or set it to the 64-character key the values were written with."),
            )
            .with_exit(Exit::Validation)
        });
    }
    let mut found = FOUND.lock().unwrap_or_else(|e| e.into_inner());
    if found.is_none() {
        *found = kept(create)?;
    }
    Ok(*found)
}

fn kept(create: bool) -> Result<Option<[u8; 32]>, CliError> {
    use penv_cloud::Keychain;
    if let Some(keychain) = penv_cloud::Keyring::open(KEYCHAIN_BASE)
        && let Ok(found) = keychain.get(ITEM)
    {
        if let Some(hex) = found {
            if let Some(key) = parse(&hex) {
                return Ok(Some(key));
            }
        } else if create {
            let key = fresh()?;
            if keychain.set(ITEM, &to_hex(&key)).is_ok() {
                return Ok(Some(key));
            }
        } else {
            return file_key(false);
        }
    }
    file_key(create)
}

/// The fallback: a file only this user can read. It keeps values out of a
/// commit and a casual read; a process running as the user can still read it.
fn file_key(create: bool) -> Result<Option<[u8; 32]>, CliError> {
    let Some(path) = key_file() else {
        return Ok(None);
    };
    if let Ok(text) = std::fs::read_to_string(&path) {
        return Ok(parse(text.trim()));
    }
    if !create {
        return Ok(None);
    }
    let key = fresh()?;
    if let Some(parent) = path.parent() {
        let _ = std::fs::create_dir_all(parent);
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let _ = std::fs::set_permissions(parent, std::fs::Permissions::from_mode(0o700));
        }
    }
    crate::files::write_private_file(&path, &format!("{}\n", to_hex(&key)))?;
    crate::ui::warn(&format!(
        "this machine has no keychain penv can use, so the key that encrypts .env values is in {} (only you can read it).",
        crate::files::show(&path)
    ));
    Ok(Some(key))
}

pub fn key_file() -> Option<PathBuf> {
    let var = |name: &str| {
        std::env::var_os(name)
            .filter(|v| !v.is_empty())
            .map(PathBuf::from)
    };
    let base = if cfg!(windows) {
        var("APPDATA")
    } else {
        var("XDG_CONFIG_HOME").or_else(|| var("HOME").map(|h| h.join(".config")))
    }?;
    Some(base.join("penv").join("local.key"))
}

fn fresh() -> Result<[u8; 32], CliError> {
    let bytes = penv_cloud::random_bytes(32).map_err(|e| {
        CliError::new(
            "random_unavailable",
            format!("the system random number generator failed: {e}."),
            "Try again.",
        )
    })?;
    let mut key = [0u8; 32];
    key.copy_from_slice(&bytes);
    Ok(key)
}

pub fn to_hex(key: &[u8; 32]) -> String {
    key.iter().map(|b| format!("{b:02x}")).collect()
}

fn parse(hex: &str) -> Option<[u8; 32]> {
    let hex = hex.trim();
    if hex.len() != 64 || !hex.is_ascii() {
        return None;
    }
    let mut key = [0u8; 32];
    for (i, byte) in key.iter_mut().enumerate() {
        *byte = u8::from_str_radix(&hex[i * 2..i * 2 + 2], 16).ok()?;
    }
    Some(key)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_value_round_trips_and_is_bound_to_its_key_name() {
        let key = [7u8; 32];
        let text = encrypt(&key, "STRIPE_SECRET_KEY", "sk_live_abc").unwrap();
        assert!(text.starts_with(PREFIX));
        assert!(!text.contains("sk_live"));
        assert_eq!(
            decrypt(&key, "STRIPE_SECRET_KEY", &text, ".env").unwrap(),
            "sk_live_abc"
        );
        assert!(
            decrypt(&key, "OTHER_KEY", &text, ".env").is_err(),
            "moved to another key"
        );
        assert!(
            decrypt(&[8u8; 32], "STRIPE_SECRET_KEY", &text, ".env").is_err(),
            "another machine's key"
        );
        assert_ne!(
            encrypt(&key, "K", "v").unwrap(),
            encrypt(&key, "K", "v").unwrap(),
            "a fresh nonce each time"
        );
    }

    #[test]
    fn a_key_found_once_is_not_looked_up_again() {
        if std::env::var_os(KEY_VAR).is_some_and(|v| !v.is_empty()) {
            return;
        }
        *FOUND.lock().unwrap() = Some([9u8; 32]);
        assert_eq!(key(false).unwrap(), Some([9u8; 32]));
    }

    #[test]
    fn hex_keys_parse_and_bad_ones_do_not() {
        let key = [0xabu8; 32];
        assert_eq!(parse(&to_hex(&key)), Some(key));
        assert!(parse("abc").is_none());
        assert!(parse(&"zz".repeat(32)).is_none());
    }
}
