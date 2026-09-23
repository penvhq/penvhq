//! `penv bundle` and a deploy that reads it with `PENV_BUNDLE_KEY`.

use std::path::{Path, PathBuf};
use std::process::{Command, Output};

const SECRET: &str = "sk_live_BUNDLE_0123456789abcdef";
const SCHEMA: &str =
    "# @type=string\nSTRIPE_SECRET_KEY=\n\n# @type=port @sensitive=false\nPORT=3000\n";

fn dir(name: &str) -> PathBuf {
    let d = std::env::temp_dir().join(format!("penv-bundle-{}-{name}", std::process::id()));
    let _ = std::fs::remove_dir_all(&d);
    std::fs::create_dir_all(&d).unwrap();
    d
}

fn penv(at: &Path, vars: &[(&str, &str)], args: &[&str]) -> Output {
    let mut command = Command::new(env!("CARGO_BIN_EXE_penv"));
    command
        .current_dir(at)
        .env(
            "PENV_LOCAL_KEY",
            "0000000000000000000000000000000000000000000000000000000000000001",
        )
        .env_remove("PENV_BUNDLE_KEY")
        .args(args);
    for (k, v) in vars {
        command.env(k, v);
    }
    command.output().unwrap()
}

fn text(o: &Output) -> String {
    format!(
        "{}{}",
        String::from_utf8_lossy(&o.stdout),
        String::from_utf8_lossy(&o.stderr)
    )
}

#[test]
fn a_bundle_ships_the_values_encrypted_and_a_deploy_with_the_key_runs_on_them() {
    let src = dir("src");
    std::fs::write(src.join(".env.schema"), SCHEMA).unwrap();
    std::fs::write(
        src.join(".env.production"),
        format!("STRIPE_SECRET_KEY={SECRET}\n"),
    )
    .unwrap();

    let refused = penv(&src, &[], &["--agent", "bundle", "--env", "production"]);
    assert_ne!(refused.status.code(), Some(0));
    assert!(text(&refused).contains("agent_session"));

    let built = penv(&src, &[], &["bundle", "--env", "production"]);
    assert_eq!(built.status.code(), Some(0), "{}", text(&built));
    let report: serde_json::Value = serde_json::from_slice(&built.stdout).unwrap();
    let key = report["key"].as_str().unwrap().to_string();
    assert_eq!(key.len(), 64);
    let bundle = std::fs::read_to_string(src.join(".penv/production.bundle")).unwrap();
    assert!(bundle.starts_with("penv-bundle v1\n") && !bundle.contains(SECRET));

    // Rebuilt with the key set: same key, not printed again.
    let again = penv(
        &src,
        &[("PENV_BUNDLE_KEY", &key)],
        &["bundle", "--env", "production"],
    );
    let report: serde_json::Value = serde_json::from_slice(&again.stdout).unwrap();
    assert!(report["key"].is_null(), "{report}");

    // The deploy: the schema and the bundle, no value file.
    let deploy = dir("deploy");
    std::fs::write(deploy.join(".env.schema"), SCHEMA).unwrap();
    std::fs::create_dir_all(deploy.join(".penv")).unwrap();
    std::fs::copy(
        src.join(".penv/production.bundle"),
        deploy.join(".penv/production.bundle"),
    )
    .unwrap();
    let script = if cfg!(windows) {
        "echo %STRIPE_SECRET_KEY%,%PORT%,%PENV_BUNDLE_KEY%"
    } else {
        "printf '%s,%s,%s' \"$STRIPE_SECRET_KEY\" \"$PORT\" \"${PENV_BUNDLE_KEY:-none}\""
    };
    let shell = if cfg!(windows) {
        ["cmd", "/c"]
    } else {
        ["sh", "-c"]
    };
    let ran = penv(
        &deploy,
        &[("PENV_BUNDLE_KEY", &key), ("PORT", "8080")],
        &[
            "run",
            "--env",
            "production",
            "--",
            shell[0],
            shell[1],
            script,
        ],
    );
    assert_eq!(ran.status.code(), Some(0), "{}", text(&ran));
    let out = String::from_utf8_lossy(&ran.stdout).to_string();
    assert!(
        out.contains(",8080,"),
        "the platform's own variable wins: {out}"
    );
    assert!(
        !out.contains(&key),
        "the command never gets PENV_BUNDLE_KEY: {out}"
    );
    assert!(
        !out.contains(SECRET),
        "penv run masks the value in the output: {out}"
    );

    let wrong = penv(
        &deploy,
        &[("PENV_BUNDLE_KEY", &"ab".repeat(32))],
        &[
            "run",
            "--env",
            "production",
            "--",
            shell[0],
            shell[1],
            "echo started",
        ],
    );
    assert_eq!(wrong.status.code(), Some(3));
    assert!(text(&wrong).contains("bundle_unreadable") && !text(&wrong).contains("started"));

    let missing = penv(
        &deploy,
        &[("PENV_BUNDLE_KEY", &key)],
        &[
            "run",
            "--env",
            "staging",
            "--",
            shell[0],
            shell[1],
            "echo started",
        ],
    );
    assert_eq!(missing.status.code(), Some(3));
    assert!(text(&missing).contains("no_bundle"));

    let _ = std::fs::remove_dir_all(&src);
    let _ = std::fs::remove_dir_all(&deploy);
}

#[test]
fn a_committed_bundle_is_not_a_value_file_to_check() {
    let d = dir("git");
    std::fs::write(d.join(".env.schema"), SCHEMA).unwrap();
    std::fs::write(
        d.join(".env.production"),
        format!("STRIPE_SECRET_KEY={SECRET}\n"),
    )
    .unwrap();
    std::fs::write(d.join(".gitignore"), ".env*\n!.env.schema\n").unwrap();
    let built = penv(&d, &[], &["bundle", "--env", "production"]);
    let key = serde_json::from_slice::<serde_json::Value>(&built.stdout).unwrap()["key"]
        .as_str()
        .unwrap()
        .to_string();
    std::fs::remove_file(d.join(".env.production")).unwrap();
    let git = |args: &[&str]| {
        Command::new("git")
            .current_dir(&d)
            .args(args)
            .output()
            .map(|o| o.status.success())
            .unwrap_or(false)
    };
    if !git(&["init", "-q"]) {
        return;
    }
    git(&["add", ".penv/production.bundle", ".env.schema"]);
    let checked = penv(
        &d,
        &[("PENV_BUNDLE_KEY", &key)],
        &["check", "--env", "production"],
    );
    assert_eq!(checked.status.code(), Some(0), "{}", text(&checked));
    let _ = std::fs::remove_dir_all(&d);
}
