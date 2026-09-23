//! Which certificate authorities penv trusts. By default the Mozilla roots
//! compiled into the binary, so a host with no CA bundle (a `FROM scratch`
//! image) still reaches penv.cloud. `SSL_CERT_FILE` replaces them with the
//! bundle it names, the way OpenSSL, curl and Python read it: that is how a
//! network that inspects TLS with its own authority is trusted.
//!
//! An agent session could point `SSL_CERT_FILE` at a bundle of its own and read
//! penv's traffic, credential and values included. So in an agent session the
//! bundle is used only when this user cannot write it, which rules out any file
//! the agent made.

use std::collections::BTreeMap;
use std::sync::{Arc, OnceLock};

use ureq::tls::{Certificate, PemItem, RootCerts, TlsConfig, parse_pem};

pub const CERT_FILE_VAR: &str = "SSL_CERT_FILE";

static ROOTS: OnceLock<RootCerts> = OnceLock::new();

/// Settle the trust store for this process. The first call wins; a later one
/// changes nothing. Returns what an error message should say when the named
/// bundle cannot be used.
pub fn configure(env: &BTreeMap<String, String>, agent: bool) -> Result<(), String> {
    let roots = match env.get(CERT_FILE_VAR).filter(|v| !v.is_empty()) {
        None => RootCerts::WebPki,
        Some(path) => load(path, agent)?,
    };
    let _ = ROOTS.set(roots);
    Ok(())
}

/// The places distributions keep their trust store.
const SYSTEM_BUNDLES: [&str; 4] = [
    "/etc/ssl/certs/ca-certificates.crt",
    "/etc/pki/tls/certs/ca-bundle.crt",
    "/etc/ssl/cert.pem",
    "/etc/ssl/ca-bundle.pem",
];

/// A distribution's own trust store, owned by root. An agent running as root
/// could still change it, but that is the machine's trust, not a file planted
/// for penv, and refusing it would leave a root agent behind a proxy with no way
/// to reach penv.cloud.
fn system_bundle(path: &str) -> bool {
    if !SYSTEM_BUNDLES.contains(&path) {
        return false;
    }
    #[cfg(unix)]
    {
        use std::os::unix::fs::MetadataExt;
        std::fs::metadata(path).is_ok_and(|m| m.uid() == 0)
    }
    #[cfg(not(unix))]
    {
        false
    }
}

fn load(path: &str, agent: bool) -> Result<RootCerts, String> {
    if agent && writable(path) && !system_bundle(path) {
        return Err(format!(
            "{CERT_FILE_VAR} names {path}, which this user can write, and an agent is running penv. A bundle an agent could have written is not trusted."
        ));
    }
    let bytes = std::fs::read(path)
        .map_err(|e| format!("{CERT_FILE_VAR} names {path}, which could not be read: {e}"))?;
    let certs: Vec<Certificate<'static>> = parse_pem(&bytes)
        .filter_map(|item| match item {
            Ok(PemItem::Certificate(cert)) => Some(cert),
            _ => None,
        })
        .collect();
    if certs.is_empty() {
        return Err(format!(
            "{CERT_FILE_VAR} names {path}, which holds no PEM certificate."
        ));
    }
    Ok(RootCerts::Specific(Arc::new(certs)))
}

fn writable(path: &str) -> bool {
    std::fs::OpenOptions::new().append(true).open(path).is_ok()
}

/// The TLS settings every request penv makes uses.
pub fn config() -> TlsConfig {
    TlsConfig::builder()
        .root_certs(ROOTS.get().cloned().unwrap_or(RootCerts::WebPki))
        .build()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn env(path: &str) -> BTreeMap<String, String> {
        [(CERT_FILE_VAR.to_string(), path.to_string())].into()
    }

    #[test]
    fn a_bundle_this_user_can_write_is_refused_under_an_agent_and_read_otherwise() {
        let dir = std::env::temp_dir().join(format!("penv-tls-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let bundle = dir.join("ca.pem");
        std::fs::write(&bundle, "not a certificate\n").unwrap();
        let path = bundle.to_string_lossy().to_string();
        let err = load(&path, true).unwrap_err();
        assert!(err.contains("an agent is running penv"), "{err}");
        let err = load(&path, false).unwrap_err();
        assert!(err.contains("holds no PEM certificate"), "{err}");
        assert!(
            load("/no/such/file.pem", false)
                .unwrap_err()
                .contains("could not be read")
        );
        assert!(configure(&BTreeMap::new(), true).is_ok());
    }

    #[test]
    fn a_root_owned_distribution_bundle_is_trusted_under_an_agent() {
        for candidate in SYSTEM_BUNDLES {
            if std::path::Path::new(candidate).is_file() {
                assert!(load(candidate, true).is_ok(), "{candidate}");
                return;
            }
        }
    }

    #[test]
    fn the_system_bundle_loads_when_there_is_one() {
        for candidate in [
            "/etc/ssl/certs/ca-certificates.crt",
            "/etc/pki/tls/certs/ca-bundle.crt",
            "/etc/ssl/cert.pem",
        ] {
            if std::path::Path::new(candidate).is_file() {
                assert!(matches!(load(candidate, false), Ok(RootCerts::Specific(_))));
                let _ = env(candidate);
                return;
            }
        }
    }
}
