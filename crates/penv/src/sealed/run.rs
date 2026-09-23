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
    for key in schema.keys.iter().filter(|k| !k.hosts.is_empty()) {
        let Some(value) = values.get(&key.name).filter(|v| !v.is_empty()) else {
            continue;
        };
        if key.ty.base == BaseType::Url {
            return Err(refuse(
                "sealed_database",
                format!(
                    "{} is a URL with @hosts. A database URL is sealed by penv's database proxy, which is not in this build.",
                    key.name
                ),
                format!(
                    "Remove @hosts from {} for now; its value reaches the command as it is.",
                    key.name
                ),
            ));
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
                    "{} has @hosts, and its type leaves no room for a placeholder: a matches rule, a type other than string, or a maxLength too short for 16 random characters.",
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
        });
    }
    if sealed.is_empty() {
        return Ok(None);
    }

    let roots = extra_roots(env, agent)?;
    let placeholders: Vec<(String, String)> = sealed
        .iter()
        .map(|s| (s.name.clone(), s.placeholder.clone()))
        .collect();
    let summary: Vec<String> = sealed
        .iter()
        .map(|s| format!("{} (only to {})", s.name, s.hosts.join(", ")))
        .collect();
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
    if let Some(system) = SYSTEM_BUNDLES.iter().find(|p| Path::new(p).is_file()) {
        let bundle = dir.join("bundle.pem");
        let mut text = std::fs::read_to_string(system).unwrap_or_default();
        if !text.ends_with('\n') {
            text.push('\n');
        }
        text.push_str(&ca_pem);
        write_private(&bundle, &text)?;
        for name in ["SSL_CERT_FILE", "REQUESTS_CA_BUNDLE", "CURL_CA_BUNDLE"] {
            vars.push((name.into(), show(&bundle)));
        }
    }
    crate::ui::warn(&format!(
        "sealed: {} reach the command as placeholders; penv puts the values into requests to their hosts.",
        summary.join(", ")
    ));
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
