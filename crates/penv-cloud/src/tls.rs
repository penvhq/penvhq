//! Which certificate authorities penv trusts. By default the Mozilla roots
//! compiled into the binary, so a host with no CA bundle (a `FROM scratch`
//! image) still reaches penv.cloud. `SSL_CERT_FILE` replaces them with the
//! bundle it names, the way OpenSSL, curl and Python read it: that is how a
//! network that inspects TLS with its own authority is trusted.
//!
//! An agent session could point `SSL_CERT_FILE` at a bundle of its own and read
//! penv's traffic, credential and values included. So in an agent session the
//! bundle is used only when this user cannot write it, which rules out any file
//! the agent made. The file is opened once, and what is checked is the file
//! that is then read, not whatever the path names a moment later.

use std::collections::BTreeMap;
use std::fs::File;
use std::io::Read;
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
#[cfg_attr(not(unix), allow(dead_code))]
const SYSTEM_BUNDLES: [&str; 4] = [
    "/etc/ssl/certs/ca-certificates.crt",
    "/etc/pki/tls/certs/ca-bundle.crt",
    "/etc/ssl/cert.pem",
    "/etc/ssl/ca-bundle.pem",
];

fn load(path: &str, agent: bool) -> Result<RootCerts, String> {
    let unreadable =
        |e: std::io::Error| format!("{CERT_FILE_VAR} names {path}, which could not be read: {e}");
    let mut file = File::open(path).map_err(unreadable)?;
    if agent && !out_of_reach(&file, path) {
        return Err(format!(
            "{CERT_FILE_VAR} names {path}, which this user owns or can write, and an agent is running penv. A bundle an agent could have written is not trusted."
        ));
    }
    let mut bytes = Vec::new();
    file.read_to_end(&mut bytes).map_err(unreadable)?;
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

/// Whether the opened bundle is beyond this user's reach, judged from the open
/// handle rather than the path.
#[cfg(unix)]
fn out_of_reach(file: &File, path: &str) -> bool {
    use std::os::unix::fs::MetadataExt;

    unsafe extern "C" {
        safe fn geteuid() -> u32;
    }
    file.metadata().is_ok_and(|meta| {
        beyond(
            meta.uid(),
            meta.mode(),
            geteuid(),
            SYSTEM_BUNDLES.contains(&path),
        )
    })
}

/// Windows reads no owner here: the bundle passes when this user cannot open
/// it for writing though it carries no read-only attribute, and no
/// distribution store is named.
#[cfg(not(unix))]
fn out_of_reach(file: &File, path: &str) -> bool {
    // The read-only attribute is the user's to clear, like a mode bit; only an
    // access list that denies this user the write counts.
    let flagged = file.metadata().is_ok_and(|m| m.permissions().readonly());
    !flagged && std::fs::OpenOptions::new().append(true).open(path).is_err()
}

/// A file its owner can always make writable again with chmod, so a mode alone
/// never clears one this user owns. Root can write anything, so for root only a
/// distribution's own root-owned store passes; an agent running as root could
/// change it, but that is the machine's trust, not a file planted for penv.
#[cfg_attr(not(unix), allow(dead_code))]
fn beyond(owner: u32, mode: u32, euid: u32, system: bool) -> bool {
    if mode & 0o022 != 0 {
        return false;
    }
    if system && owner == 0 {
        return true;
    }
    euid != 0 && owner != euid
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

    #[cfg(unix)]
    #[test]
    fn a_bundle_made_read_only_by_its_owner_is_still_refused_under_an_agent() {
        use std::os::unix::fs::PermissionsExt;

        let dir = std::env::temp_dir().join(format!("penv-tls-ro-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let bundle = dir.join("ca.pem");
        std::fs::write(&bundle, "not a certificate\n").unwrap();
        std::fs::set_permissions(&bundle, std::fs::Permissions::from_mode(0o444)).unwrap();
        let err = load(&bundle.to_string_lossy(), true).unwrap_err();
        assert!(err.contains("an agent is running penv"), "{err}");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[cfg(windows)]
    #[test]
    fn a_bundle_its_owner_flagged_read_only_is_still_refused_under_an_agent() {
        let dir = std::env::temp_dir().join(format!("penv-tls-ro-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let bundle = dir.join("ca.pem");
        std::fs::write(&bundle, "not a certificate\n").unwrap();
        let mut permissions = std::fs::metadata(&bundle).unwrap().permissions();
        permissions.set_readonly(true);
        std::fs::set_permissions(&bundle, permissions).unwrap();
        let err = load(&bundle.to_string_lossy(), true).unwrap_err();
        assert!(err.contains("an agent is running penv"), "{err}");
        let mut permissions = std::fs::metadata(&bundle).unwrap().permissions();
        #[allow(clippy::permissions_set_readonly_false)]
        permissions.set_readonly(false);
        let _ = std::fs::set_permissions(&bundle, permissions);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn only_a_file_this_user_neither_owns_nor_can_write_is_out_of_reach() {
        let (me, other, root) = (1000, 1001, 0);
        assert!(!beyond(me, 0o444, me, false), "the owner can chmod it back");
        assert!(!beyond(me, 0o644, me, false));
        assert!(beyond(other, 0o644, me, false));
        assert!(beyond(root, 0o444, me, false));
        assert!(!beyond(other, 0o664, me, false), "group-writable");
        assert!(!beyond(other, 0o646, me, false), "world-writable");
        assert!(!beyond(other, 0o644, root, false), "root writes anything");
        assert!(!beyond(root, 0o644, root, false));
        assert!(
            beyond(root, 0o644, root, true),
            "a distribution's own store"
        );
        assert!(!beyond(root, 0o666, root, true));
        assert!(!beyond(me, 0o644, me, true), "a store path this user owns");
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
