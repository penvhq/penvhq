use std::path::{Path, PathBuf};

use serde_json::json;

use crate::error::CliError;
use crate::files::show;
use crate::output::{Output, Report};
use crate::upgrade::{
    PUBLIC_KEYS, RELEASE_BASE, Swap, checked_keys, checked_signature, digest_in, is_newer,
    latest_url, manager, pick, replace, resets_in, tag_of, triple,
};
use penv_cloud::fetch;

const VERSION: &str = env!("CARGO_PKG_VERSION");
const INSTALL: &str = "Install from https://penv.cloud/install.";

/// The published release is the source of truth: the raw binary for this target,
/// its digest, and then the swap.
pub fn run(out: &Output, check: bool) -> Result<Report, CliError> {
    let style = out.style();
    // The host is answered before the network is, so --check refuses here too.
    let target = triple(std::env::consts::ARCH, std::env::consts::OS)?;

    let url = latest_url(RELEASE_BASE);
    let body = get(&url, "application/vnd.github+json")?;
    let release: serde_json::Value =
        serde_json::from_slice(&body).map_err(|e| unreadable(&url, &e.to_string()))?;
    let tag = tag_of(&release)?;
    let latest = tag.trim_start_matches('v');
    let behind = is_newer(tag, VERSION);

    if check {
        // Naming a release to install would be advice this build cannot take.
        if behind {
            checked_keys(PUBLIC_KEYS)?;
        }
        let text = if behind {
            format!(
                "penv {latest} is out\n{}",
                style.dim(&format!("you are on {VERSION}; run penv upgrade"))
            )
        } else {
            format!("penv {VERSION} is current")
        };
        return Ok(Report::new(
            json!({ "current": !behind, "latest": tag }),
            text,
        ));
    }

    if !behind {
        return Ok(Report::new(
            json!({ "current": true, "latest": tag }),
            format!("penv {VERSION} is current"),
        ));
    }

    // The manager owns this file whatever this build could verify, so its advice
    // comes before the key check that would otherwise send the user nowhere.
    let path = current_exe()?;
    if let Some((name, command)) = manager(&path) {
        return Err(CliError::new(
            "managed_install",
            format!("penv here is managed by {name}; run {command}."),
            format!("Run {command}, or install from https://penv.cloud/install elsewhere."),
        ));
    }
    checked_keys(PUBLIC_KEYS)?;

    let picked = pick(&release, target, RELEASE_BASE)?;
    let binary = get(&picked.asset_url, "application/octet-stream")?;
    let sums = get(&picked.checksum_url, "text/plain")?;

    // The signature stands in front of the digest: an unsigned checksum file says
    // nothing about the binary it lists.
    let signature = match &picked.signature_url {
        Some(url) => Some(String::from_utf8_lossy(&get(url, "text/plain")?).into_owned()),
        None => None,
    };
    checked_signature(PUBLIC_KEYS, &sums, signature.as_deref())?;

    let sums = String::from_utf8_lossy(&sums).into_owned();
    let expected = digest_in(&sums, &picked.asset).ok_or_else(|| {
        CliError::new(
            "no_checksum",
            format!(
                "{} lists no sha256 digest for {}.",
                picked.checksum, picked.asset
            ),
            "Install from https://penv.cloud/install until that release is fixed.",
        )
    })?;
    if !expected.eq_ignore_ascii_case(&penv_cloud::sha256_hex(&binary)) {
        return Err(CliError::new(
            "checksum_mismatch",
            format!(
                "{} is not the file {} names.",
                picked.asset, picked.checksum
            ),
            "Nothing was replaced. Try again, and report it if it happens twice.",
        ));
    }

    replace(&path, &binary).map_err(|e| swap_failed(&path, &e))?;

    Ok(Report::new(
        json!({
            "from": VERSION,
            "to": latest,
            "path": show(&path),
            "asset": picked.asset,
            "digest": "sha256",
            "signature": "ed25519",
        }),
        format!(
            "{} penv {VERSION} to {latest}\n{}",
            style.green("upgraded"),
            style.dim(&show(&path))
        ),
    ))
}

/// The real file behind this process: a symlink on the install path would leave
/// the swap writing to the link rather than the binary a manager owns.
fn current_exe() -> Result<PathBuf, CliError> {
    let path = std::env::current_exe().map_err(|e| {
        CliError::new(
            "no_current_exe",
            format!("this binary's own path could not be read: {e}."),
            INSTALL,
        )
    })?;
    Ok(std::fs::canonicalize(&path).unwrap_or(path))
}

fn swap_failed(path: &Path, e: &Swap) -> CliError {
    match e {
        Swap::Kept(_) => CliError::new(
            "unwritable_binary",
            format!("{} could not be replaced: {e}.", show(path)),
            "Nothing changed. Run it again where you may write that directory, or reinstall.",
        ),
        Swap::Lost(_) => CliError::new(
            "binary_lost",
            format!(
                "penv is now missing at {}; the new file did not land and the old one could not be put back: {e}.",
                show(path)
            ),
            "Reinstall from https://penv.cloud/install.",
        ),
    }
}

fn get(url: &str, accept: &str) -> Result<Vec<u8>, CliError> {
    // A release is checked against its signature, so a bundle cannot slip in a
    // binary; it only has to let the download through an inspecting proxy.
    let process: std::collections::BTreeMap<String, String> = std::env::vars().collect();
    penv_cloud::tls::configure(&process, false).map_err(|message| {
        CliError::new(
            "untrusted_ca_bundle",
            message,
            "Unset SSL_CERT_FILE to use the roots built into penv, or point it at a readable PEM bundle.",
        )
    })?;
    let response = fetch::get(url, accept).map_err(|e| {
        CliError::new(
            "release_unreachable",
            format!("{url} could not be read: {e}."),
            "Check the network and try again.",
        )
    })?;
    match response.status {
        status if (200..300).contains(&status) => Ok(response.bytes),
        404 => Err(CliError::new(
            "no_release",
            format!("{url} answered 404, so there is no release to read yet."),
            INSTALL,
        )),
        403 | 429 => Err(rate_limited(url, &response)),
        status => Err(CliError::new(
            "release_unreachable",
            format!("{url} answered {status}."),
            "Check the network and try again.",
        )),
    }
}

/// The release host caps unauthenticated requests by the hour, and says on the
/// response when the next hour starts.
fn rate_limited(url: &str, response: &fetch::Response) -> CliError {
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or_default();
    let when = response
        .header("x-ratelimit-reset")
        .and_then(|value| value.trim().parse::<u64>().ok())
        .and_then(|reset| resets_in(reset, now))
        .unwrap_or_else(|| "within the hour".to_string());
    CliError::new(
        "rate_limited",
        format!(
            "{url} refused: the release host allows a limited number of unauthenticated requests an hour, and this host has spent them."
        ),
        format!("Try again {when}, or install from https://penv.cloud/install."),
    )
}

fn unreadable(url: &str, reason: &str) -> CliError {
    CliError::new(
        "unreadable_release",
        format!("{url} answered something that is not a release: {reason}."),
        "Try again later, or install from https://penv.cloud/install.",
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    fn answered(headers: &[(&str, &str)]) -> fetch::Response {
        fetch::Response {
            status: 403,
            headers: headers
                .iter()
                .map(|(k, v)| (k.to_string(), v.to_string()))
                .collect(),
            bytes: Vec::new(),
        }
    }

    #[test]
    fn a_rate_limit_says_when_it_lifts_when_the_header_carries_it() {
        let soon = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_secs()
            + 600;
        let refused = rate_limited("https://releases.test/x", &answered(&[]));
        assert_eq!(refused.code, "rate_limited");
        assert!(
            refused.message.contains("unauthenticated requests an hour"),
            "{refused:?}"
        );
        assert!(!refused.message.contains("GitHub"), "{refused:?}");
        assert!(refused.fix.contains("within the hour"), "{refused:?}");

        let timed = rate_limited(
            "https://releases.test/x",
            &answered(&[("x-ratelimit-reset", &soon.to_string())]),
        );
        assert!(timed.fix.contains("in 10m"), "{timed:?}");
    }

    #[test]
    fn a_lost_binary_is_not_the_same_refusal_as_an_unwritable_one() {
        let path = Path::new("/usr/local/bin/penv");
        let missing = || std::io::Error::from(std::io::ErrorKind::PermissionDenied);
        let kept = swap_failed(path, &Swap::Kept(missing()));
        let lost = swap_failed(path, &Swap::Lost(missing()));
        assert_eq!(kept.code, "unwritable_binary");
        assert_eq!(lost.code, "binary_lost");
        assert!(lost.message.contains("penv is now missing at"), "{lost:?}");
        assert!(lost.fix.contains("Reinstall"), "{lost:?}");
    }
}
