//! A deploy bundle: one environment's values in `.penv/<env>.bundle`,
//! encrypted with a key the deploy platform holds as `PENV_BUNDLE_KEY`. With
//! that variable set, `penv run` reads the bundle where it would otherwise read
//! penv.cloud, so a container or server needs neither the cloud nor a plain
//! `.env` file.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use crate::env::Env;
use crate::error::{CliError, Exit};
use crate::files::show;

pub const KEY_VAR: &str = "PENV_BUNDLE_KEY";

/// The bundle file and the values it held.
pub type Opened = (PathBuf, BTreeMap<String, String>);
const HEADER: &str = "penv-bundle v1";

pub fn path(dir: &Path, environment: &str) -> PathBuf {
    dir.join(".penv").join(format!("{environment}.bundle"))
}

fn aad(environment: &str) -> String {
    format!("penv-bundle:{environment}")
}

pub fn parse_key(hex: &str) -> Option<[u8; 32]> {
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

/// The bundle text for `values`.
pub fn seal(
    key: &[u8; 32],
    environment: &str,
    values: &BTreeMap<String, String>,
) -> Result<String, CliError> {
    let payload = serde_json::json!({ "environment": environment, "values": values }).to_string();
    let sealed =
        penv_cloud::cache::seal(key, &aad(environment), payload.as_bytes()).map_err(|e| {
            CliError::new(
                "encrypt_failed",
                format!("the bundle could not be encrypted: {e}."),
                "Try again.",
            )
        })?;
    Ok(format!("{HEADER}\n{}\n", penv_cloud::b64::encode(&sealed)))
}

pub fn open(
    key: &[u8; 32],
    environment: &str,
    text: &str,
    shown: &str,
) -> Result<BTreeMap<String, String>, CliError> {
    let fail = |why: &str| {
        CliError::new(
            "bundle_unreadable",
            format!("{shown} could not be read: {why}."),
            format!("Check that {KEY_VAR} is the key penv bundle printed for {environment}, or run penv bundle --env {environment} again."),
        )
        .with_exit(Exit::Validation)
    };
    let mut lines = text.lines();
    if lines.next().map(str::trim) != Some(HEADER) {
        return Err(fail("it is not a penv bundle"));
    }
    let body: String = lines.map(str::trim).collect();
    let sealed = penv_cloud::b64::decode(&body).ok_or_else(|| fail("it is damaged"))?;
    let plain = penv_cloud::cache::unseal(key, &aad(environment), &sealed)
        .map_err(|_| fail("the key does not open it, or it was made for another environment"))?;
    let json: serde_json::Value =
        serde_json::from_slice(&plain).map_err(|_| fail("it is damaged"))?;
    let values = json["values"]
        .as_object()
        .ok_or_else(|| fail("it holds no values"))?
        .iter()
        .filter_map(|(k, v)| v.as_str().map(|v| (k.clone(), v.to_string())))
        .collect();
    Ok(values)
}

/// The bundle for `environment`, when `PENV_BUNDLE_KEY` is set. Set with no
/// bundle beside it is an error: a deploy that meant to read one must not start
/// without its values.
pub fn read(dir: &Path, environment: &str, env: &Env) -> Result<Option<Opened>, CliError> {
    let Some(hex) = env.get(KEY_VAR).filter(|v| !v.is_empty()) else {
        return Ok(None);
    };
    let key = parse_key(hex).ok_or_else(|| {
        CliError::new(
            "invalid_bundle_key",
            format!("{KEY_VAR} is not 64 hexadecimal characters."),
            "Set it to the key penv bundle printed, or unset it.",
        )
        .with_exit(Exit::Validation)
    })?;
    let file = path(dir, environment);
    let shown = show(file.strip_prefix(dir).unwrap_or(&file));
    let Ok(text) = std::fs::read_to_string(&file) else {
        return Err(CliError::new(
            "no_bundle",
            format!("{KEY_VAR} is set, and there is no bundle for {environment} at {shown}."),
            format!("Run penv bundle --env {environment} and ship {shown} with the deploy, or unset {KEY_VAR}."),
        )
        .with_exit(Exit::Validation));
    };
    open(&key, environment, &text, &shown).map(|v| Some((file, v)))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_bundle_opens_with_its_key_and_its_environment_only() {
        let key = [3u8; 32];
        let values = BTreeMap::from([("STRIPE_SECRET_KEY".to_string(), "sk_live_x".to_string())]);
        let text = seal(&key, "production", &values).unwrap();
        assert!(text.starts_with("penv-bundle v1\n") && !text.contains("sk_live"));
        assert_eq!(open(&key, "production", &text, "b").unwrap(), values);
        assert!(
            open(&[4u8; 32], "production", &text, "b").is_err(),
            "another key"
        );
        assert!(
            open(&key, "staging", &text, "b").is_err(),
            "renamed to another environment"
        );
        assert!(open(&key, "production", "nonsense", "b").is_err());
    }
}
