//! Every completion script over the real manifest, byte for byte, and through
//! the shell's own parser where that shell is installed.

use std::io::Write;
use std::path::PathBuf;
use std::process::{Command, Stdio};

use penv::completions::{SHELLS, script};
use penv::manifest::manifest;

fn snapshot_path(shell: &str) -> PathBuf {
    PathBuf::from("tests/snapshots").join(format!("completions.{shell}"))
}

/// Point at the first line that differs; the whole script is too long to read in
/// a test failure.
fn assert_same(shell: &str, expected: &str, actual: &str) {
    if expected == actual {
        return;
    }
    let path = snapshot_path(shell);
    let hint = expected
        .lines()
        .zip(actual.lines())
        .enumerate()
        .find(|(_, (want, got))| want != got)
        .map(|(line, (want, got))| format!("line {}:\n-{want}\n+{got}", line + 1))
        .unwrap_or_else(|| {
            format!(
                "same prefix, {} expected line(s) against {} generated",
                expected.lines().count(),
                actual.lines().count()
            )
        });
    panic!(
        "{} no longer matches the manifest.\n{hint}\nRe-run with PENV_BLESS=1 once the change is deliberate.",
        path.display()
    );
}

fn generated(shell: &str) -> String {
    script(shell, &manifest()).expect("a shell penv writes for")
}

#[test]
fn every_shell_renders_its_snapshot() {
    if std::env::var_os("PENV_BLESS").is_some() {
        for shell in SHELLS {
            let path = snapshot_path(shell);
            std::fs::create_dir_all(path.parent().unwrap()).unwrap();
            std::fs::write(path, generated(shell)).unwrap();
        }
        panic!("the snapshots were rewritten from PENV_BLESS; read the diff and run again");
    }
    for shell in SHELLS {
        let expected = std::fs::read_to_string(snapshot_path(shell)).unwrap_or_default();
        assert_same(shell, &expected, &generated(shell));
    }
}

/// `-n` parses without running, over stdin, so no path has to survive the trip
/// into a shell that may not spell this filesystem the same way.
fn parses(shell: &str, script: &str) {
    // Windows ships a `bash` that only launches WSL and fails without a distro.
    let answers = Command::new(shell)
        .arg("--version")
        .output()
        .is_ok_and(|o| o.status.success() && !o.stdout.is_empty());
    if !answers {
        return;
    }
    let spawned = Command::new(shell)
        .arg("-n")
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn();
    let Ok(mut child) = spawned else {
        return;
    };
    child
        .stdin
        .take()
        .expect("stdin was piped")
        .write_all(script.as_bytes())
        .unwrap();
    let output = child.wait_with_output().unwrap();
    assert!(
        output.status.success(),
        "{shell} -n refused its own script:\n{}",
        String::from_utf8_lossy(&output.stderr)
    );
}

/// Every install line redirects the script into a file, which leaves stdout a
/// pipe; the script has to survive that as plain text.
#[test]
fn a_piped_script_is_the_script_and_not_json() {
    let output = Command::new(env!("CARGO_BIN_EXE_penv"))
        .env(
            "PENV_LOCAL_KEY",
            "0000000000000000000000000000000000000000000000000000000000000001",
        )
        .args(["completions", "bash"])
        .stdout(Stdio::piped())
        .output()
        .expect("penv runs");

    assert!(output.status.success(), "{output:?}");
    let text = String::from_utf8_lossy(&output.stdout);
    assert_eq!(text.lines().next(), Some("# penv completions bash"));
    assert_eq!(text, generated("bash"));
}

#[test]
fn json_by_name_carries_the_script_as_a_member() {
    let output = Command::new(env!("CARGO_BIN_EXE_penv"))
        .env(
            "PENV_LOCAL_KEY",
            "0000000000000000000000000000000000000000000000000000000000000001",
        )
        .args(["--json", "completions", "zsh"])
        .output()
        .expect("penv runs");

    assert!(output.status.success(), "{output:?}");
    let json: serde_json::Value = serde_json::from_slice(&output.stdout).expect("one JSON object");
    assert_eq!(json["shell"], "zsh");
    assert_eq!(json["script"], generated("zsh"));
}

#[test]
fn the_bash_script_parses_where_bash_is_installed() {
    parses("bash", &generated("bash"));
}

#[test]
fn the_zsh_script_parses_where_zsh_is_installed() {
    parses("zsh", &generated("zsh"));
}
