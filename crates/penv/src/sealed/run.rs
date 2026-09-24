//! Setting up a sealed `penv run`: placeholders for the keys with `@hosts`, the
//! proxy that puts the values back on the way out, and the variables that make
//! the child's HTTP clients use it and trust its authority.

use std::path::{Path, PathBuf};

use penv_schema::{BaseType, Schema, Values};
use rustls::pki_types::CertificateDer;

use super::proxy::{Proxy, Sealed};
use crate::env::Env;
use crate::error::{CliError, Exit};

pub struct Seal {
    /// Key → placeholder, for the child's environment.
    pub placeholders: Vec<(String, String)>,
    /// Proxy and trust variables for the child.
    pub env: Vec<(String, String)>,
}

/// The distribution trust stores, so a bundle for the child still trusts every
/// public site as well as this run's authority.
const SYSTEM_BUNDLES: [&str; 4] = [
    "/etc/ssl/certs/ca-certificates.crt",
    "/etc/pki/tls/certs/ca-bundle.crt",
    "/etc/ssl/cert.pem",
    "/etc/ssl/ca-bundle.pem",
];

pub fn prepare(
    schema: &Schema,
    values: &Values,
    env: &Env,
    agent: bool,
) -> Result<Option<Seal>, CliError> {
    let mut sealed = Vec::new();
    let mut databases = Vec::new();
    for key in schema.keys.iter().filter(|k| !k.hosts.is_empty()) {
        let Some(value) = values.get(&key.name).filter(|v| !v.is_empty()) else {
            continue;
        };
        if let Some(why) = super::signing_secret(&key.name) {
            return Err(refuse(
                "cannot_seal",
                format!(
                    "{} has @hosts, but {why}, so a placeholder cannot stand in for it.",
                    key.name
                ),
                format!("Remove @hosts from {}.", key.name),
            ));
        }
        if key.ty.base == BaseType::Url {
            databases.push((key, value.clone()));
            continue;
        }
        let mut failed = None;
        let placeholder = penv_schema::placeholder::placeholder(&key.ty, &mut |n| {
            penv_cloud::random_bytes(n).unwrap_or_else(|e| {
                failed = Some(e);
                vec![0; n]
            })
        });
        if let Some(e) = failed {
            return Err(refuse(
                "random_unavailable",
                format!("the system random number generator failed: {e}."),
                "Try again.",
            ));
        }
        let Some(placeholder) = placeholder else {
            return Err(refuse(
                "cannot_seal",
                format!(
                    "{} has @hosts, and its type leaves no room for a placeholder: a matches rule, a type other than string, or a maxLength too short for 24 random characters.",
                    key.name
                ),
                format!(
                    "Describe {} with startsWith, endsWith, minLength and maxLength instead.",
                    key.name
                ),
            ));
        };
        sealed.push(Sealed {
            name: key.name.clone(),
            placeholder,
            value: value.clone(),
            hosts: key.hosts.clone(),
            sigv4: super::is_aws_secret(&key.name),
        });
    }
    if sealed.is_empty() && databases.is_empty() {
        return Ok(None);
    }

    let roots = extra_roots(env, agent)?;
    let mut placeholders_db = Vec::new();
    let mut summary_db = Vec::new();
    for (key, value) in databases {
        let (child_url, host) = database(key, &value, &roots)?;
        summary_db.push(format!("{} (only to {host})", key.name));
        placeholders_db.push((key.name.clone(), child_url));
    }
    if sealed.is_empty() {
        crate::ui::warn(&format!(
            "sealed: {} reach the command through penv's database proxy, with a placeholder password.",
            summary_db.join(", ")
        ));
        return Ok(Some(Seal {
            placeholders: placeholders_db,
            env: Vec::new(),
        }));
    }
    let placeholders: Vec<(String, String)> = sealed
        .iter()
        .map(|s| (s.name.clone(), s.placeholder.clone()))
        .collect();
    let mut summary: Vec<String> = sealed
        .iter()
        .map(|s| format!("{} (only to {})", s.name, s.hosts.join(", ")))
        .collect();
    summary.extend(summary_db);
    let proxy = Proxy::new(sealed, roots).map_err(|e| {
        refuse(
            "proxy_failed",
            format!("the sealed proxy could not start: {e}."),
            "Try again.",
        )
    })?;
    let ca_pem = proxy.ca_pem().to_string();
    let port = proxy.start().map_err(|e| {
        refuse(
            "proxy_failed",
            format!("the sealed proxy could not listen: {e}."),
            "Try again.",
        )
    })?;

    let dir = files_dir()?;
    let ca = dir.join("ca.pem");
    write_private(&ca, &ca_pem)?;
    let url = format!("http://127.0.0.1:{port}");
    let mut vars: Vec<(String, String)> = vec![
        ("HTTPS_PROXY".into(), url.clone()),
        ("HTTP_PROXY".into(), url.clone()),
        ("https_proxy".into(), url.clone()),
        ("http_proxy".into(), url),
        // Every request goes through the proxy: a host left out would get the
        // placeholder, never the value, but would also fail.
        ("NO_PROXY".into(), String::new()),
        ("no_proxy".into(), String::new()),
        // Node 22.21+ and 24 route fetch and http through the variables above.
        ("NODE_USE_ENV_PROXY".into(), "1".into()),
        ("NODE_EXTRA_CA_CERTS".into(), show(&ca)),
        ("DENO_CERT".into(), show(&ca)),
    ];
    // A bundle of every public root plus this run's authority: the system's
    // own where it has one file, else the Mozilla roots penv carries (Windows,
    // where Python, curl and the AWS SDKs read a file, not the OS store).
    let bundle = dir.join("bundle.pem");
    let mut text = match SYSTEM_BUNDLES.iter().find(|p| Path::new(p).is_file()) {
        Some(system) => std::fs::read_to_string(system).unwrap_or_default(),
        None => mozilla_roots_pem(),
    };
    if !text.ends_with('\n') {
        text.push('\n');
    }
    text.push_str(&ca_pem);
    write_private(&bundle, &text)?;
    for name in [
        "SSL_CERT_FILE",
        "REQUESTS_CA_BUNDLE",
        "CURL_CA_BUNDLE",
        "AWS_CA_BUNDLE",
    ] {
        vars.push((name.into(), show(&bundle)));
    }
    crate::ui::warn(&format!(
        "sealed: {} reach the command as placeholders; penv puts the values into requests to their hosts.",
        summary.join(", ")
    ));
    if let Some(version) = node_version()
        && !node_follows_proxy_variables(version)
    {
        crate::ui::warn(&format!(
            "node {}.{} on PATH ignores HTTPS_PROXY, so its fetch sends placeholders straight to the host and fails. Node 22.21 or 24 and later follow it.",
            version.0, version.1
        ));
    }
    Ok(Some(Seal {
        placeholders,
        env: vars,
    }))
}

/// The bundle SSL_CERT_FILE names, under penv's own rule for it, trusted for
/// upstream hosts: a network that inspects TLS still works through the proxy.
fn extra_roots(env: &Env, agent: bool) -> Result<Vec<CertificateDer<'static>>, CliError> {
    let Some(path) = env.get("SSL_CERT_FILE").filter(|p| !p.is_empty()) else {
        return Ok(Vec::new());
    };
    crate::commands::cloud::trust(env, agent)?;
    let text = std::fs::read_to_string(path).unwrap_or_default();
    let mut out = Vec::new();
    let mut inside = None::<String>;
    for line in text.lines() {
        let line = line.trim();
        if line == "-----BEGIN CERTIFICATE-----" {
            inside = Some(String::new());
        } else if line == "-----END CERTIFICATE-----" {
            if let Some(body) = inside.take()
                && let Some(der) = penv_cloud::b64::decode(&body)
            {
                out.push(CertificateDer::from(der));
            }
        } else if let Some(body) = inside.as_mut() {
            body.push_str(line);
        }
    }
    Ok(out)
}

/// A folder only this user can read, for the certificate files the child trusts.
fn files_dir() -> Result<PathBuf, CliError> {
    let base = crate::preload::cache_dir().ok_or_else(|| {
        refuse(
            "no_home",
            "a sealed run needs a home folder for its certificate files.".into(),
            "Set HOME (LOCALAPPDATA on Windows).",
        )
    })?;
    let dir = base.join(format!("sealed-{}", std::process::id()));
    std::fs::create_dir_all(&dir).map_err(|e| {
        refuse(
            "write_failed",
            format!("{} could not be created: {e}.", show(&dir)),
            "Check the permissions of your cache folder.",
        )
    })?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let _ = std::fs::set_permissions(&dir, std::fs::Permissions::from_mode(0o700));
    }
    // Folders from earlier runs hold only a certificate; clear them anyway.
    if let Ok(entries) = std::fs::read_dir(&base) {
        for entry in entries.flatten() {
            let name = entry.file_name().to_string_lossy().to_string();
            if name.starts_with("sealed-") && entry.path() != dir {
                let _ = std::fs::remove_dir_all(entry.path());
            }
        }
    }
    Ok(dir)
}

fn write_private(path: &Path, body: &str) -> Result<(), CliError> {
    crate::files::write_private_file(path, body)
}

fn show(path: &Path) -> String {
    path.to_string_lossy().to_string()
}

fn refuse(code: &'static str, message: String, fix: impl Into<String>) -> CliError {
    CliError::new(code, message, fix.into()).with_exit(Exit::Validation)
}

/// A sealed database URL: start the proxy for its scheme and return the URL
/// the command gets, pointing at it with a placeholder password.
fn database(
    key: &penv_schema::Key,
    value: &str,
    roots: &[CertificateDer<'static>],
) -> Result<(String, String), CliError> {
    let Some(url) = super::url::parse(value) else {
        return Err(refuse(
            "cannot_seal",
            format!(
                "{} has @hosts, and its URL has no password to seal (or names several hosts).",
                key.name
            ),
            format!(
                "Put the password in the URL, or remove @hosts from {}.",
                key.name
            ),
        ));
    };
    if !penv_schema::placeholder::host_allowed(&key.hosts, &url.host) {
        return Err(refuse(
            "cannot_seal",
            format!(
                "{}'s URL points at {}, which its @hosts does not name.",
                key.name, url.host
            ),
            format!("Add {} to @hosts on {}.", url.host, key.name),
        ));
    }
    let placeholder = format!(
        "penvph{}",
        penv_cloud::b64::encode(&penv_cloud::random_bytes(24).map_err(|e| refuse(
            "random_unavailable",
            e,
            "Try again."
        ))?)
        .replace(['+', '/', '='], "")
    );
    let failed = |e: String| {
        refuse(
            "proxy_failed",
            format!("the database proxy for {} could not start: {e}.", key.name),
            "Try again.",
        )
    };
    match url.scheme.as_str() {
        "postgres" | "postgresql" => {
            let mode = super::url::query(&url.rest, "sslmode").unwrap_or_else(|| "prefer".into());
            let tls = match mode.as_str() {
                "disable" => super::postgres::Tls::Off,
                "allow" | "prefer" => {
                    super::postgres::Tls::Prefer(super::upstream::unverified().map_err(failed)?)
                }
                "require" => {
                    super::postgres::Tls::Require(super::upstream::unverified().map_err(failed)?)
                }
                "verify-ca" | "verify-full" => {
                    super::postgres::Tls::Verify(super::upstream::verified(roots).map_err(failed)?)
                }
                other => {
                    return Err(refuse(
                        "cannot_seal",
                        format!(
                            "{}'s URL has sslmode={other}, which penv does not know.",
                            key.name
                        ),
                        "Use disable, prefer, require, verify-ca or verify-full.",
                    ));
                }
            };
            let port = super::postgres::start(super::postgres::Target {
                host: url.host.clone(),
                port: url.port.unwrap_or(5432),
                password: url.password.clone(),
                placeholder: placeholder.clone(),
                tls,
            })
            .map_err(|e| failed(e.to_string()))?;
            // The loopback leg is plain and answered with a cleartext password
            // request, so options that forbid that on the command's side go.
            let mut rest = super::url::with_query(&url.rest, "sslmode", "disable");
            if super::url::query(&rest, "channel_binding").is_some() {
                rest = super::url::with_query(&rest, "channel_binding", "disable");
            }
            if super::url::query(&rest, "require_auth").is_some() {
                rest = super::url::with_query(&rest, "require_auth", "password");
            }
            let child = format!(
                "{}://{}:{}@127.0.0.1:{port}{rest}",
                url.scheme,
                super::url::encode(&url.user),
                placeholder
            );
            Ok((child, url.host))
        }
        "redis" | "rediss" => {
            let tls = if url.scheme == "rediss" {
                Some(super::upstream::verified(roots).map_err(failed)?)
            } else {
                None
            };
            let port = super::redis::start(super::redis::Target {
                host: url.host.clone(),
                port: url.port.unwrap_or(6379),
                password: url.password.clone(),
                placeholder: placeholder.clone(),
                tls,
            })
            .map_err(|e| failed(e.to_string()))?;
            // The command talks plain Redis to the loopback port.
            let child = format!(
                "redis://{}:{}@127.0.0.1:{port}{}",
                super::url::encode(&url.user),
                placeholder,
                url.rest
            );
            Ok((child, url.host))
        }
        other => Err(refuse(
            "cannot_seal",
            format!(
                "{} is a {other}:// URL; penv's database proxy speaks postgres:// and redis://.",
                key.name
            ),
            format!("Remove @hosts from {}.", key.name),
        )),
    }
}

/// The Mozilla roots compiled into penv, as PEM.
fn mozilla_roots_pem() -> String {
    let mut out = String::new();
    for cert in webpki_root_certs::TLS_SERVER_ROOT_CERTS {
        out.push_str("-----BEGIN CERTIFICATE-----\n");
        let b64 = penv_cloud::b64::encode(cert.as_ref());
        for line in b64.as_bytes().chunks(64) {
            out.push_str(&String::from_utf8_lossy(line));
            out.push('\n');
        }
        out.push_str("-----END CERTIFICATE-----\n");
    }
    out
}

/// `node --version` as (major, minor), when there is a node on PATH.
fn node_version() -> Option<(u32, u32)> {
    let out = std::process::Command::new("node")
        .arg("--version")
        .output()
        .ok()?;
    parse_node_version(&String::from_utf8_lossy(&out.stdout))
}

fn parse_node_version(text: &str) -> Option<(u32, u32)> {
    let mut parts = text.trim().trim_start_matches('v').split('.');
    Some((parts.next()?.parse().ok()?, parts.next()?.parse().ok()?))
}

/// `NODE_USE_ENV_PROXY` arrived in 24.0 and was backported to 22.21.
fn node_follows_proxy_variables((major, minor): (u32, u32)) -> bool {
    major >= 24 || (major == 22 && minor >= 21)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_carried_roots_make_a_bundle_python_can_load() {
        let pem = mozilla_roots_pem();
        assert!(pem.matches("-----BEGIN CERTIFICATE-----").count() > 100);
        let path = std::env::temp_dir().join(format!("penv-roots-{}.pem", std::process::id()));
        std::fs::write(&path, &pem).unwrap();
        // Where Python is present, its ssl module (OpenSSL) must read every cert.
        if let Ok(out) = std::process::Command::new("python3")
            .args(["-c", "import ssl,sys; c=ssl.create_default_context(cafile=sys.argv[1]); print(c.cert_store_stats()['x509_ca'])"])
            .arg(&path)
            .output()
            && out.status.success()
        {
            let loaded: usize = String::from_utf8_lossy(&out.stdout).trim().parse().unwrap_or(0);
            assert_eq!(loaded, webpki_root_certs::TLS_SERVER_ROOT_CERTS.len());
        }
        let _ = std::fs::remove_file(&path);
    }

    #[test]
    fn node_versions_that_follow_the_proxy_variables() {
        assert_eq!(parse_node_version("v22.22.2\n"), Some((22, 22)));
        for (v, ok) in [
            ((22, 21), true),
            ((22, 20), false),
            ((23, 11), false),
            ((24, 0), true),
            ((20, 19), false),
            ((26, 1), true),
        ] {
            assert_eq!(node_follows_proxy_variables(v), ok, "{v:?}");
        }
    }
}
