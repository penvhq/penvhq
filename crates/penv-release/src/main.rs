//! The release signing tool, and the one place a private key is printed: the
//! owner runs `keygen` once and keeps the halves, CI runs `sign` over every
//! checksum file. Nothing here ships in a release build.

use std::path::{Path, PathBuf};
use std::process::ExitCode;

use penv_cloud::signature;

/// The CI secret `sign` reads the private key from.
const KEY_VAR: &str = "PENV_SIGNING_KEY";

const USAGE: &str = "usage: penv-release keygen | penv-release sign <file>";

fn main() -> ExitCode {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let taken: Vec<&str> = args.iter().map(String::as_str).collect();
    let done = match taken.as_slice() {
        ["keygen"] => keygen(),
        ["sign", file] => sign(Path::new(file)),
        _ => Err(USAGE.to_string()),
    };
    match done {
        Ok(()) => ExitCode::SUCCESS,
        Err(reason) => {
            eprintln!("penv-release: {reason}");
            ExitCode::FAILURE
        }
    }
}

/// Two base64 lines on stdout, and which is which on stderr, so a redirect keeps
/// the pair alone.
fn keygen() -> Result<(), String> {
    let pair = signature::generate().map_err(|e| e.to_string())?;
    eprintln!(
        "penv-release: line 1 is the private key for the {KEY_VAR} secret; line 2 is the public key for PUBLIC_KEYS in crates/penv/src/upgrade.rs, public_keys in install.sh and $publicKeys in install.ps1"
    );
    println!("{}", pair.private);
    println!("{}", pair.public);
    Ok(())
}

fn sign(file: &Path) -> Result<(), String> {
    let key = std::env::var(KEY_VAR)
        .ok()
        .filter(|key| !key.trim().is_empty())
        .ok_or_else(|| {
            format!("{KEY_VAR} is unset, and it is the key a release is signed with.")
        })?;
    let bytes =
        std::fs::read(file).map_err(|e| format!("{} could not be read: {e}.", file.display()))?;
    let signature = signature::sign(&key, &bytes)
        .ok_or_else(|| format!("{KEY_VAR} is not a base64 Ed25519 private key; run keygen."))?;

    let mut name = file.as_os_str().to_owned();
    name.push(".sig");
    let out = PathBuf::from(name);
    std::fs::write(&out, format!("{signature}\n"))
        .map_err(|e| format!("{} could not be written: {e}.", out.display()))?;
    eprintln!(
        "penv-release: signed {} into {}",
        file.display(),
        out.display()
    );
    Ok(())
}
