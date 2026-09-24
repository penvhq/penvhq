//! What `penv upgrade` works out before it touches the network or the disk:
//! which release is newer, which asset this host runs, and how a running binary
//! is swapped for the one that was downloaded.

use std::cmp::Ordering;
use std::path::{Path, PathBuf};

use serde_json::Value;

use crate::error::CliError;

/// The targets the release workflow builds, by the constants a running binary
/// reports for itself.
pub const TARGETS: [(&str, &str, &str); 6] = [
    ("x86_64", "linux", "x86_64-unknown-linux-musl"),
    ("aarch64", "linux", "aarch64-unknown-linux-musl"),
    ("x86_64", "macos", "x86_64-apple-darwin"),
    ("aarch64", "macos", "aarch64-apple-darwin"),
    ("x86_64", "windows", "x86_64-pc-windows-msvc"),
    ("aarch64", "windows", "aarch64-pc-windows-msvc"),
];

/// The Ed25519 public keys, base64, a release checksum file may be signed with.
/// Empty until the owner runs `cargo run -p penv-release -- keygen` and pastes
/// the public half here. A rotation adds the new key, ships a release signed by
/// the old one carrying both, and drops the old key the release after that.
pub const PUBLIC_KEYS: &[&str] = &["VRJ90W7uzjrwQKeD6KCGQj1dih6z6/4QVeat0M5qL/0="];

/// One release, narrowed to what this host would install.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Picked {
    pub tag: String,
    pub asset: String,
    pub asset_url: String,
    pub checksum: String,
    pub checksum_url: String,
    pub signature: String,
    /// None when the release lists no signature for its checksum file.
    pub signature_url: Option<String>,
}

pub fn triple(arch: &str, os: &str) -> Result<&'static str, CliError> {
    TARGETS
        .iter()
        .find(|(a, o, _)| *a == arch && *o == os)
        .map(|(_, _, triple)| *triple)
        .ok_or_else(|| {
            let known = TARGETS
                .iter()
                .map(|(_, _, triple)| *triple)
                .collect::<Vec<_>>()
                .join(", ");
            CliError::new(
                "unknown_target",
                format!("penv publishes no build for {arch} on {os}."),
                format!("The releases carry {known}. Build from source for anything else."),
            )
        })
}

/// `penv-v1.2.3-x86_64-pc-windows-msvc.exe`, the raw binary the upgrade replaces
/// itself with. The archives beside it need a decompressor this binary has not got.
pub fn asset_name(tag: &str, triple: &str) -> String {
    let suffix = if triple.contains("windows") {
        ".exe"
    } else {
        ""
    };
    format!("penv-{tag}-{triple}{suffix}")
}

/// Every asset for one target shares the checksum file.
pub fn checksum_name(tag: &str, triple: &str) -> String {
    format!("penv-{tag}-{triple}.sha256")
}

/// The signature over that checksum file, which is what a release key signs.
pub fn signature_name(tag: &str, triple: &str) -> String {
    format!("{}.sig", checksum_name(tag, triple))
}

/// A build with no key in [`PUBLIC_KEYS`] cannot tell a penv release from a file
/// somebody put in its place, so it does not upgrade at all.
pub fn checked_keys(keys: &[&str]) -> Result<(), CliError> {
    if keys.is_empty() {
        return Err(CliError::new(
            "unsigned_build",
            "this build carries no release key, so it cannot check who signed a download.",
            "Install from https://penv.cloud/install; that binary carries the key and upgrades from then on.",
        ));
    }
    Ok(())
}

/// The signature over the checksum file, against every key this build carries.
/// Nothing about the digest is read until this holds.
pub fn checked_signature(
    keys: &[&str],
    sums: &[u8],
    signature: Option<&str>,
) -> Result<(), CliError> {
    checked_keys(keys)?;
    let Some(signature) = signature.map(str::trim).filter(|s| !s.is_empty()) else {
        return Err(CliError::new(
            "signature_missing",
            "the release carries no signature for its checksum file.",
            "Nothing was replaced. Install from https://penv.cloud/install, and report a release that stays unsigned.",
        ));
    };
    if !penv_cloud::signature::verify_any(keys, sums, signature) {
        return Err(CliError::new(
            "signature_invalid",
            "the checksum file is not signed by a penv release key.",
            "Nothing was replaced. Install from https://penv.cloud/install, and report it.",
        ));
    }
    Ok(())
}

/// The digest for one asset out of a `sha256sum` file. The name is matched whole,
/// so the archive's line is never read for the raw binary, and the digest is only
/// answered when it is the 64 hex characters sha256 spells.
pub fn digest_in<'a>(sums: &'a str, asset: &str) -> Option<&'a str> {
    sums.lines().find_map(|line| {
        let (digest, name) = line.split_once(char::is_whitespace)?;
        let digest = digest.trim();
        let named = name.trim().trim_start_matches('*') == asset;
        let hex = digest.len() == 64 && digest.bytes().all(|b| b.is_ascii_hexdigit());
        (named && hex).then_some(digest)
    })
}

/// The tag the release names. Without one there is nothing to compare against,
/// so the answer is never "you are current".
pub fn tag_of(release: &Value) -> Result<&str, CliError> {
    release["tag_name"]
        .as_str()
        .filter(|tag| !tag.trim().is_empty())
        .ok_or_else(|| {
            CliError::new(
                "no_release",
                "the latest release carries no tag_name.",
                "Try again later, or install from https://penv.cloud/install.",
            )
        })
}

/// The package managers that own the file they installed. Replacing a binary
/// behind one of them leaves the manager describing a version that is gone.
const MANAGERS: [(&str, &str, &str); 7] = [
    // npm leads: a global install lands under whichever prefix installed node, Homebrew's included.
    ("/node_modules/", "npm", "npm i -g @penvhq/cli"),
    ("/opt/homebrew/", "Homebrew", "brew upgrade penv"),
    ("/usr/local/cellar/", "Homebrew", "brew upgrade penv"),
    ("/home/linuxbrew/", "Homebrew", "brew upgrade penv"),
    ("/nix/store/", "Nix", "nix profile upgrade penv"),
    ("/microsoft/winget/", "winget", "winget upgrade penv"),
    ("/scoop/apps/", "Scoop", "scoop update penv"),
];

/// The manager that installed this path, if one did, with the command that
/// upgrades through it.
pub fn manager(path: &Path) -> Option<(&'static str, &'static str)> {
    let path = path.to_string_lossy().replace('\\', "/").to_lowercase();
    MANAGERS
        .iter()
        .find(|(marker, _, _)| path.contains(marker))
        .map(|(_, name, command)| (*name, *command))
}

/// How long until the rate limit lifts, from the epoch second a header carries.
/// None once that second is behind us, since there is nothing left to wait for.
pub fn resets_in(reset: u64, now: u64) -> Option<String> {
    let left = reset.checked_sub(now).filter(|left| *left > 0)?;
    Some(if left < 90 {
        format!("in {left}s")
    } else {
        format!("in {}m", left.div_ceil(60))
    })
}

/// The one address a build carries. Which repository stands behind it is Cargo.toml
/// metadata and a redirect penv.cloud owns, so moving the repository installs nothing new.
pub const RELEASE_BASE: &str = "https://penv.cloud";

/// The release JSON, behind a redirect to the API that serves it.
pub fn latest_url(base: &str) -> String {
    format!("{}/releases/latest", base.trim_end_matches('/'))
}

/// One asset, behind a redirect to the release that carries it.
pub fn download_url(base: &str, tag: &str, asset: &str) -> String {
    format!(
        "{}/releases/download/{tag}/{asset}",
        base.trim_end_matches('/')
    )
}

/// The tag and the two URLs this host needs. The release JSON says which assets
/// exist; where they are downloaded from is the one address, never a URL the JSON
/// carries.
pub fn pick(release: &Value, triple: &str, base: &str) -> Result<Picked, CliError> {
    let tag = tag_of(release)?;
    let asset = asset_name(tag, triple);
    let checksum = checksum_name(tag, triple);
    let signature = signature_name(tag, triple);
    let listed = |name: &str| {
        release["assets"]
            .as_array()
            .into_iter()
            .flatten()
            .any(|a| a["name"].as_str() == Some(name))
            .then(|| download_url(base, tag, name))
    };
    let url_of = |name: &str| {
        listed(name).ok_or_else(|| {
            CliError::new(
                "missing_asset",
                format!("release {tag} carries no {name}."),
                "Install from https://penv.cloud/install until that release is fixed.",
            )
        })
    };
    Ok(Picked {
        tag: tag.to_string(),
        asset_url: url_of(&asset)?,
        checksum_url: url_of(&checksum)?,
        // An unsigned release is refused where the signature is checked, so this
        // one is picked up when it is there and named when it is not.
        signature_url: listed(&signature),
        asset,
        checksum,
        signature,
    })
}

/// True when the release is ahead of what is running.
pub fn is_newer(latest: &str, running: &str) -> bool {
    compare(latest, running) == Ordering::Greater
}

/// Semantic versions, as far as penv tags go: a numeric triple, and a prerelease
/// suffix that sorts below the release it leads to.
pub fn compare(a: &str, b: &str) -> Ordering {
    let (core_a, pre_a) = split(a);
    let (core_b, pre_b) = split(b);
    match core_a.cmp(&core_b) {
        Ordering::Equal => match (pre_a, pre_b) {
            (None, None) => Ordering::Equal,
            (None, Some(_)) => Ordering::Greater,
            (Some(_), None) => Ordering::Less,
            (Some(x), Some(y)) => prerelease(x, y),
        },
        other => other,
    }
}

fn split(version: &str) -> ([u64; 3], Option<&str>) {
    let version = version.trim().trim_start_matches('v');
    let version = version.split('+').next().unwrap_or_default();
    let (core, pre) = match version.split_once('-') {
        Some((core, pre)) => (core, Some(pre)),
        None => (version, None),
    };
    let mut parts = core.split('.');
    let mut number = || {
        parts
            .next()
            .and_then(|p| p.parse::<u64>().ok())
            .unwrap_or(0)
    };
    ([number(), number(), number()], pre)
}

/// Dot-separated identifiers, numeric ones by value, so `alpha.10` is above
/// `alpha.2`. Fewer identifiers sort first.
fn prerelease(a: &str, b: &str) -> Ordering {
    let mut left = a.split('.');
    let mut right = b.split('.');
    loop {
        match (left.next(), right.next()) {
            (None, None) => return Ordering::Equal,
            (None, Some(_)) => return Ordering::Less,
            (Some(_), None) => return Ordering::Greater,
            (Some(x), Some(y)) => {
                let order = match (x.parse::<u64>(), y.parse::<u64>()) {
                    (Ok(x), Ok(y)) => x.cmp(&y),
                    (Ok(_), Err(_)) => Ordering::Less,
                    (Err(_), Ok(_)) => Ordering::Greater,
                    (Err(_), Err(_)) => x.cmp(y),
                };
                if order != Ordering::Equal {
                    return order;
                }
            }
        }
    }
}

/// Where the download lands before it takes the running binary's place.
pub fn staged(current: &Path) -> PathBuf {
    beside(current, "new")
}

/// Where Windows moves the running binary while the new one lands, since it
/// cannot rename over a file it is executing.
pub fn retired(current: &Path) -> PathBuf {
    beside(current, "old")
}

fn beside(current: &Path, extension: &str) -> PathBuf {
    let stem = current
        .file_stem()
        .map(|s| s.to_string_lossy().into_owned())
        .unwrap_or_else(|| "penv".to_string());
    current.with_file_name(format!("{stem}.{extension}"))
}

/// What went wrong swapping the binary. A lost restore is its own case, because
/// the file the caller ran is no longer there.
#[derive(Debug)]
pub enum Swap {
    /// Nothing moved, or the running binary was put back.
    Kept(std::io::Error),
    /// The staged binary did not land and the running one could not be restored.
    Lost(std::io::Error),
}

impl std::fmt::Display for Swap {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Swap::Kept(e) | Swap::Lost(e) => write!(f, "{e}"),
        }
    }
}

/// Write the downloaded binary where the running one is. Unix renames it over
/// the running file in one step, so there is never a moment with no penv.
/// Windows cannot rename over a file it is executing, so the running binary is
/// retired first and left for the next invocation to sweep.
pub fn replace(current: &Path, bytes: &[u8]) -> Result<(), Swap> {
    std::fs::metadata(current).map_err(Swap::Kept)?;
    let staged = staged(current);
    std::fs::write(&staged, bytes).map_err(Swap::Kept)?;
    swap(current, &staged)
}

#[cfg(unix)]
fn swap(current: &Path, staged: &Path) -> Result<(), Swap> {
    use std::os::unix::fs::PermissionsExt;
    let landed = std::fs::set_permissions(staged, std::fs::Permissions::from_mode(0o755))
        .and_then(|()| std::fs::rename(staged, current));
    if let Err(e) = landed {
        let _ = std::fs::remove_file(staged);
        return Err(Swap::Kept(e));
    }
    // A copy an older penv retired.
    let _ = std::fs::remove_file(retired(current));
    Ok(())
}

#[cfg(not(unix))]
fn swap(current: &Path, staged: &Path) -> Result<(), Swap> {
    let retired = retired(current);
    let _ = std::fs::remove_file(&retired);
    if let Err(e) = std::fs::rename(current, &retired) {
        let _ = std::fs::remove_file(staged);
        return Err(Swap::Kept(e));
    }
    if let Err(e) = std::fs::rename(staged, current) {
        let _ = std::fs::remove_file(staged);
        return match std::fs::rename(&retired, current) {
            Ok(()) => Err(Swap::Kept(e)),
            Err(restore) => Err(Swap::Lost(restore)),
        };
    }
    Ok(())
}

/// The copy Windows had to keep last time, gone as soon as it is not running.
/// One stat per invocation, and nothing at all anywhere else.
pub fn sweep_retired() {
    if !cfg!(windows) {
        return;
    }
    let Ok(current) = std::env::current_exe() else {
        return;
    };
    let retired = retired(&current);
    if retired.exists() {
        let _ = std::fs::remove_file(retired);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn a_release_is_newer_only_when_its_numbers_are() {
        assert!(is_newer("v1.2.3", "1.2.2"));
        assert!(is_newer("1.10.0", "1.9.9"));
        assert!(is_newer("2.0.0", "1.99.99"));
        assert!(!is_newer("1.2.3", "1.2.3"));
        assert!(!is_newer("v1.2.3", "1.2.3"));
        assert!(!is_newer("1.2.2", "1.2.3"));
    }

    #[test]
    fn a_prerelease_sorts_below_the_release_it_leads_to() {
        assert!(is_newer("1.0.0", "1.0.0-alpha.1"));
        assert!(!is_newer("1.0.0-alpha.1", "1.0.0"));
        assert!(is_newer("1.0.0-alpha.2", "1.0.0-alpha.1"));
        assert!(is_newer("1.0.0-alpha.10", "1.0.0-alpha.2"));
        assert!(is_newer("1.0.0-beta.1", "1.0.0-alpha.9"));
        assert_eq!(compare("1.0.0-alpha.1", "1.0.0-alpha.1"), Ordering::Equal);
        assert_eq!(compare("1.0.0+build.5", "1.0.0"), Ordering::Equal);
    }

    #[test]
    fn every_target_the_workflow_builds_is_one_a_running_binary_can_ask_for() {
        assert_eq!(
            triple("x86_64", "linux").unwrap(),
            "x86_64-unknown-linux-musl"
        );
        assert_eq!(triple("aarch64", "macos").unwrap(), "aarch64-apple-darwin");
        assert_eq!(
            triple("x86_64", "windows").unwrap(),
            "x86_64-pc-windows-msvc"
        );
        assert_eq!(
            triple("aarch64", "windows").unwrap(),
            "aarch64-pc-windows-msvc"
        );
        let refused = triple("riscv64", "linux").unwrap_err();
        assert_eq!(refused.code, "unknown_target");
        assert!(
            refused.fix.contains("x86_64-unknown-linux-musl"),
            "{refused:?}"
        );
    }

    #[test]
    fn only_a_windows_asset_carries_an_exe_suffix() {
        assert_eq!(
            asset_name("v1.2.3", "x86_64-unknown-linux-musl"),
            "penv-v1.2.3-x86_64-unknown-linux-musl"
        );
        assert_eq!(
            asset_name("v1.2.3", "x86_64-pc-windows-msvc"),
            "penv-v1.2.3-x86_64-pc-windows-msvc.exe"
        );
        assert_eq!(
            checksum_name("v1.2.3", "x86_64-pc-windows-msvc"),
            "penv-v1.2.3-x86_64-pc-windows-msvc.sha256"
        );
        assert_eq!(
            signature_name("v1.2.3", "x86_64-pc-windows-msvc"),
            "penv-v1.2.3-x86_64-pc-windows-msvc.sha256.sig"
        );
    }

    /// Obviously fake, and still the 64 hex characters a real digest is.
    const ARCHIVE: &str = "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa";
    const STAR: &str = "bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb";

    #[test]
    fn the_archive_line_is_never_read_for_the_raw_binary() {
        let sums = format!(
            "{ARCHIVE}  penv-v1.2.3-x86_64-unknown-linux-musl.tar.gz\n\
             {STAR}  penv-v1.2.3-x86_64-unknown-linux-musl\n"
        );
        let sums = sums.as_str();
        assert_eq!(
            digest_in(sums, "penv-v1.2.3-x86_64-unknown-linux-musl"),
            Some(STAR)
        );
        assert_eq!(digest_in(sums, "penv-v1.2.3-aarch64-apple-darwin"), None);
        assert_eq!(
            digest_in(&format!("{STAR} *penv-v1.2.3\n"), "penv-v1.2.3"),
            Some(STAR)
        );
    }

    /// A checksum file carrying another release's lines answers for that release
    /// only, so an old digest cannot be replayed against the tag being installed.
    #[test]
    fn a_line_for_another_tag_never_answers_for_this_one() {
        let sums = format!(
            "{ARCHIVE}  penv-v1.2.4-x86_64-unknown-linux-musl\n\
             {STAR}  penv-v1.2.3-x86_64-unknown-linux-musl\n"
        );
        assert_eq!(
            digest_in(&sums, "penv-v1.2.3-x86_64-unknown-linux-musl"),
            Some(STAR)
        );
        assert_eq!(
            digest_in(
                &format!("{ARCHIVE}  penv-v1.2.4-x86_64-unknown-linux-musl\n"),
                "penv-v1.2.3-x86_64-unknown-linux-musl"
            ),
            None
        );
        // A prefix of the asked-for name is not the name either, in either direction.
        assert_eq!(
            digest_in(
                &format!("{ARCHIVE}  penv-v1.2.3-x86_64-unknown-linux-musl.tar.gz\n"),
                "penv-v1.2.3-x86_64-unknown-linux-musl"
            ),
            None
        );
        assert_eq!(
            digest_in(
                &format!("{ARCHIVE}  penv-v1.2.3-x86_64-unknown-linux-musl\n"),
                "penv-v1.2.3-x86_64-unknown-linux-musl.exe"
            ),
            None
        );
    }

    #[test]
    fn a_line_that_is_not_a_sha256_names_no_digest() {
        assert_eq!(digest_in("cccc  penv-v1.2.3\n", "penv-v1.2.3"), None);
        assert_eq!(
            digest_in(&format!("{}  penv-v1.2.3\n", "z".repeat(64)), "penv-v1.2.3"),
            None
        );
        assert_eq!(
            digest_in(&format!("{STAR}0  penv-v1.2.3\n"), "penv-v1.2.3"),
            None
        );
    }

    fn release() -> Value {
        let asset = |name: &str| json!({ "name": name });
        json!({
            "tag_name": "v1.2.3",
            "assets": [
                asset("penv-v1.2.3-x86_64-unknown-linux-musl.tar.gz"),
                asset("penv-v1.2.3-x86_64-unknown-linux-musl"),
                asset("penv-v1.2.3-x86_64-unknown-linux-musl.sha256"),
                asset("penv-v1.2.3-x86_64-unknown-linux-musl.sha256.sig"),
                asset("penv-v1.2.3-aarch64-apple-darwin"),
            ],
        })
    }

    #[test]
    fn the_release_answers_with_the_asset_this_host_runs() {
        let picked = pick(&release(), "x86_64-unknown-linux-musl", RELEASE_BASE).unwrap();
        assert_eq!(picked.tag, "v1.2.3");
        assert_eq!(picked.asset, "penv-v1.2.3-x86_64-unknown-linux-musl");
        assert_eq!(
            picked.signature_url.as_deref(),
            Some(
                "https://penv.cloud/releases/download/v1.2.3/penv-v1.2.3-x86_64-unknown-linux-musl.sha256.sig"
            )
        );
        assert_eq!(
            picked.asset_url,
            "https://penv.cloud/releases/download/v1.2.3/penv-v1.2.3-x86_64-unknown-linux-musl"
        );
        assert_eq!(
            picked.checksum_url,
            "https://penv.cloud/releases/download/v1.2.3/penv-v1.2.3-x86_64-unknown-linux-musl.sha256"
        );
    }

    /// The installer verifies against its own copy of the list, so a key pasted
    /// into one file and not the other is caught here rather than in a release.
    #[test]
    fn the_installer_lists_the_same_keys_as_this_build() {
        const INSTALLER: &str = include_str!("../../../install.sh");
        let declared = INSTALLER
            .lines()
            .find_map(|line| line.strip_prefix("public_keys="))
            .expect("install.sh declares public_keys");
        let listed: Vec<&str> = declared
            .trim()
            .trim_matches('"')
            .split_whitespace()
            .collect();
        assert_eq!(
            listed.as_slice(),
            PUBLIC_KEYS,
            "install.sh and PUBLIC_KEYS carry different release keys"
        );
    }

    #[test]
    fn every_key_this_build_ships_is_a_key() {
        for key in PUBLIC_KEYS {
            assert!(
                penv_cloud::signature::is_public_key(key),
                "{key} is not a base64 Ed25519 public key"
            );
        }
    }

    #[test]
    fn a_build_carrying_no_key_upgrades_to_nothing() {
        let refused = checked_keys(&[]).unwrap_err();
        assert_eq!(refused.code, "unsigned_build");
        assert!(refused.message.contains("no release key"), "{refused:?}");
        assert_eq!(
            checked_signature(&[], b"anything", Some("whatever"))
                .unwrap_err()
                .code,
            "unsigned_build"
        );
    }

    #[test]
    fn only_a_release_key_signature_over_these_bytes_passes() {
        let sums = b"0000000000000000000000000000000000000000000000000000000000000000  penv-v1.2.3-x86_64-apple-darwin\n";
        let ours = penv_cloud::signature::generate().unwrap();
        let retired = penv_cloud::signature::generate().unwrap();
        let theirs = penv_cloud::signature::generate().unwrap();
        let signature = penv_cloud::signature::sign(&ours.private, sums).unwrap();

        assert!(checked_signature(&[&ours.public], sums, Some(&signature)).is_ok());
        // A rotation lists both keys, and the release signed by either one lands.
        assert!(
            checked_signature(&[&retired.public, &ours.public], sums, Some(&signature)).is_ok()
        );

        let missing = checked_signature(&[&ours.public], sums, None).unwrap_err();
        assert_eq!(missing.code, "signature_missing");
        assert!(missing.message.contains("no signature"), "{missing:?}");

        assert_eq!(
            checked_signature(&[&ours.public], sums, Some("   "))
                .unwrap_err()
                .code,
            "signature_missing"
        );
        assert_eq!(
            checked_signature(&[&theirs.public], sums, Some(&signature))
                .unwrap_err()
                .code,
            "signature_invalid"
        );
        assert_eq!(
            checked_signature(
                &[&ours.public],
                b"a checksum file nobody signed",
                Some(&signature)
            )
            .unwrap_err()
            .code,
            "signature_invalid"
        );
    }

    #[test]
    fn a_release_missing_this_hosts_asset_is_named_rather_than_guessed() {
        let refused = pick(&release(), "aarch64-apple-darwin", RELEASE_BASE).unwrap_err();
        assert_eq!(refused.code, "missing_asset");
        assert!(refused.message.contains(".sha256"), "{refused:?}");
    }

    fn workspace(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("penv-upgrade-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    #[test]
    fn the_new_binary_takes_the_running_ones_place() {
        let dir = workspace("swap");
        let current = dir.join(if cfg!(windows) { "penv.exe" } else { "penv" });
        std::fs::write(&current, b"old binary").unwrap();
        std::fs::write(retired(&current), b"a leftover from last time").unwrap();

        replace(&current, b"new binary").unwrap();

        assert_eq!(std::fs::read(&current).unwrap(), b"new binary");
        assert!(
            !staged(&current).exists(),
            "the staged file was left behind"
        );
        if cfg!(windows) {
            assert_eq!(std::fs::read(retired(&current)).unwrap(), b"old binary");
        } else {
            assert!(
                !retired(&current).exists(),
                "Unix has nothing to keep the retired binary for"
            );
        }
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[cfg(unix)]
    #[test]
    fn unix_swaps_in_one_rename_and_never_moves_the_running_binary_aside() {
        let dir = workspace("atomic");
        let current = dir.join("penv");
        std::fs::write(&current, b"old binary").unwrap();
        // A retired path nothing can be renamed onto: a two-step swap fails here.
        std::fs::create_dir_all(retired(&current).join("busy")).unwrap();

        replace(&current, b"new binary").unwrap();

        assert_eq!(std::fs::read(&current).unwrap(), b"new binary");
        assert!(!staged(&current).exists());
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn a_swap_that_never_starts_leaves_no_staged_file() {
        let dir = workspace("nostart");
        let current = dir.join("penv");

        let refused = replace(&current, b"new binary").unwrap_err();

        assert!(matches!(refused, Swap::Kept(_)), "{refused:?}");
        assert!(!staged(&current).exists(), "penv.new was left behind");
        assert!(!current.exists());
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn a_managed_install_is_named_by_the_manager_that_owns_it() {
        assert_eq!(
            manager(Path::new("/opt/homebrew/bin/penv")),
            Some(("Homebrew", "brew upgrade penv"))
        );
        assert_eq!(
            manager(Path::new("/usr/local/Cellar/penv/1.0.0/bin/penv")),
            Some(("Homebrew", "brew upgrade penv"))
        );
        assert_eq!(
            manager(Path::new("/home/linuxbrew/.linuxbrew/bin/penv")),
            Some(("Homebrew", "brew upgrade penv"))
        );
        assert_eq!(
            manager(Path::new("/nix/store/abc-penv-1.0.0/bin/penv")).map(|(name, _)| name),
            Some("Nix")
        );
        assert_eq!(
            manager(Path::new(
                r"C:\Users\dev\AppData\Local\Microsoft\WinGet\Packages\penv\penv.exe"
            ))
            .map(|(_, command)| command),
            Some("winget upgrade penv")
        );
        assert_eq!(
            manager(Path::new(r"C:\Users\dev\scoop\apps\penv\current\penv.exe"))
                .map(|(_, command)| command),
            Some("scoop update penv")
        );
        assert_eq!(
            manager(Path::new(
                "/usr/local/lib/node_modules/@penvhq/cli-linux-x64/bin/penv"
            )),
            Some(("npm", "npm i -g @penvhq/cli"))
        );
        assert_eq!(
            manager(Path::new(
                "/opt/homebrew/lib/node_modules/@penvhq/cli-darwin-arm64/bin/penv"
            ))
            .map(|(name, _)| name),
            Some("npm")
        );
        assert_eq!(
            manager(Path::new(
                r"C:\Users\dev\AppData\Roaming\npm\node_modules\@penvhq\cli-win32-x64\bin\penv.exe"
            ))
            .map(|(name, _)| name),
            Some("npm")
        );
        assert_eq!(manager(Path::new("/usr/local/bin/penv")), None);
        assert_eq!(manager(Path::new(r"C:\tools\penv.exe")), None);
    }

    #[test]
    fn a_release_with_no_tag_is_no_release_rather_than_the_current_one() {
        let refused = tag_of(&json!({ "assets": [] })).unwrap_err();
        assert_eq!(refused.code, "no_release");
        assert_eq!(
            tag_of(&json!({ "tag_name": "" })).unwrap_err().code,
            "no_release"
        );
        assert_eq!(tag_of(&json!({ "tag_name": "v1.2.3" })).unwrap(), "v1.2.3");
    }

    #[test]
    fn a_rate_limit_reset_reads_as_the_wait_it_leaves() {
        assert_eq!(resets_in(1_000_030, 1_000_000), Some("in 30s".to_string()));
        assert_eq!(resets_in(1_000_600, 1_000_000), Some("in 10m".to_string()));
        assert_eq!(resets_in(1_000_601, 1_000_000), Some("in 11m".to_string()));
        assert_eq!(resets_in(1_000_000, 1_000_000), None);
        assert_eq!(resets_in(999_000, 1_000_000), None);
    }

    #[test]
    fn every_url_the_upgrade_reads_hangs_off_the_one_address() {
        assert_eq!(
            latest_url(RELEASE_BASE),
            "https://penv.cloud/releases/latest"
        );
        assert_eq!(
            download_url(
                "https://penv.test/",
                "v1.2.3",
                "penv-v1.2.3-x86_64-apple-darwin"
            ),
            "https://penv.test/releases/download/v1.2.3/penv-v1.2.3-x86_64-apple-darwin"
        );
    }

    #[test]
    fn the_staged_and_retired_names_sit_beside_the_binary() {
        let current = Path::new("/usr/local/bin/penv");
        assert_eq!(staged(current), Path::new("/usr/local/bin/penv.new"));
        assert_eq!(retired(current), Path::new("/usr/local/bin/penv.old"));
        let windows = Path::new("C:/tools/penv.exe");
        assert_eq!(staged(windows), Path::new("C:/tools/penv.new"));
    }
}
