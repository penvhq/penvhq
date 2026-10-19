//! Computed `@hosts`: `check` holds it to the `[hosts]` pin, and only
//! `check --pin-hosts` writes the pin. No message carries a value.

use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::atomic::{AtomicU32, Ordering};

const PASSWORD: &str = "fakepass0000";
const STRIPE: &str = "sk_test_FAKEHOSTS000000000000000000000";
const SECRET_HOST: &str = "fake-secret-host-0000.test";

const SCHEMA: &str = "# @schema=1

# @type=url @hosts=url($DATABASE_URL, host)
DATABASE_URL=

# @type=string(startsWith=sk_test_) @hosts=api.stripe.com
STRIPE_SECRET_KEY=
";

static COUNTER: AtomicU32 = AtomicU32::new(0);

fn workspace(schema: &str, host: &str, config: Option<&str>) -> PathBuf {
    let n = COUNTER.fetch_add(1, Ordering::SeqCst);
    let dir = std::env::temp_dir().join(format!("penv-hosts-{}-{n}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(dir.join(".penv")).unwrap();
    std::fs::write(dir.join(".env.schema"), schema).unwrap();
    std::fs::write(
        dir.join(".env"),
        format!(
            "DATABASE_URL=postgres://fakeuser:{PASSWORD}@{host}:5432/app\nSTRIPE_SECRET_KEY={STRIPE}\nSECRET_HOST={SECRET_HOST}\n"
        ),
    )
    .unwrap();
    if let Some(config) = config {
        std::fs::write(dir.join(".penv/config.toml"), config).unwrap();
    }
    dir
}

fn penv_with(dir: &Path, args: &[&str], agent: bool) -> (i32, String) {
    let mut command = Command::new(env!("CARGO_BIN_EXE_penv"));
    command
        .env(
            "PENV_LOCAL_KEY",
            "0000000000000000000000000000000000000000000000000000000000000001",
        )
        .env_remove("PENV_ENV")
        .current_dir(dir)
        .args(["--format", "text"])
        .args(args);
    if agent {
        command.env("AI_AGENT", "1");
    }
    let out = command.output().unwrap();
    let text = format!(
        "{}{}",
        String::from_utf8_lossy(&out.stdout),
        String::from_utf8_lossy(&out.stderr)
    );
    for secret in [PASSWORD, STRIPE, SECRET_HOST] {
        assert!(!text.contains(secret), "a value was printed by {args:?}");
    }
    (out.status.code().unwrap_or(-1), text)
}

fn penv(dir: &Path, args: &[&str]) -> (i32, String) {
    penv_with(dir, args, false)
}

fn config(dir: &Path) -> String {
    std::fs::read_to_string(dir.join(".penv/config.toml")).unwrap()
}

#[test]
fn an_unpinned_computed_host_fails_check_until_pin_hosts_writes_it() {
    let dir = workspace(
        SCHEMA,
        "db.internal.test",
        Some("# keep this\n[url]\npublic = [\"host\"]\n\n[hosts]\n# pins go here\n"),
    );
    let (code, text) = penv(&dir, &["check"]);
    assert_eq!(code, 3, "{text}");
    assert!(
        text.contains("DATABASE_URL: @hosts computes db.internal.test and [hosts] pins nothing"),
        "{text}"
    );

    let (code, text) = penv(&dir, &["check", "--pin-hosts"]);
    assert_eq!(code, 0, "{text}");
    assert!(
        text.contains("pinned DATABASE_URL +db.internal.test"),
        "{text}"
    );
    assert!(!text.contains("STRIPE_SECRET_KEY"), "{text}");
    let written = config(&dir);
    assert!(
        written.starts_with("# keep this\n[url]\npublic = [\"host\"]\n"),
        "{written}"
    );
    assert!(
        written.contains("[hosts]\nDATABASE_URL = [\"db.internal.test\"]\n# pins go here"),
        "{written}"
    );

    let (code, text) = penv(&dir, &["check", "--pin-hosts"]);
    assert_eq!(code, 0, "{text}");
    assert_eq!(text.trim(), "", "a second pin printed something");
    assert_eq!(config(&dir), written);

    let (code, text) = penv(&dir, &["check"]);
    assert!(!text.contains("@hosts computes"), "{text}");
    assert_eq!(code, 0, "{text}");
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn a_changed_host_fails_check_naming_both_lists_and_pinning_prints_only_the_change() {
    let pinned = "[hosts]\nDATABASE_URL = [\"old.internal.test\"] # approved\n";
    let dir = workspace(SCHEMA, "new.internal.test", Some(pinned));
    let (code, text) = penv(&dir, &["check"]);
    assert_eq!(code, 3, "{text}");
    assert!(
        text.contains(
            "DATABASE_URL: @hosts computes new.internal.test and [hosts] pins old.internal.test"
        ),
        "{text}"
    );
    let (code, text) = penv(&dir, &["check", "--pin-hosts"]);
    assert_eq!(code, 0, "{text}");
    assert!(
        text.contains("pinned DATABASE_URL +new.internal.test -old.internal.test"),
        "{text}"
    );
    assert!(
        config(&dir).contains("DATABASE_URL = [\"new.internal.test\"] # approved"),
        "{}",
        config(&dir)
    );
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn a_host_computed_from_a_sensitive_value_is_refused_and_never_shown() {
    let schema =
        "# @type=string @hosts=$SECRET_HOST\nSTRIPE_SECRET_KEY=\n\n# @type=string\nSECRET_HOST=\n";
    let dir = workspace(schema, "db.internal.test", None);
    let (code, text) = penv(&dir, &["check"]);
    assert_eq!(code, 3, "{text}");
    assert!(
        text.contains("STRIPE_SECRET_KEY: @hosts is computed from a sensitive value"),
        "{text}"
    );
    let (code, text) = penv(&dir, &["check", "--pin-hosts"]);
    assert_eq!(code, 3, "{text}");
    assert!(
        !dir.join(".penv/config.toml").exists(),
        "a refused pin was written"
    );
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn an_agent_cannot_pin_hosts() {
    let dir = workspace(SCHEMA, "db.internal.test", None);
    let (code, text) = penv_with(&dir, &["check", "--pin-hosts"], true);
    assert_eq!(code, 2, "{text}");
    assert!(
        text.contains("--pin-hosts approves where secrets may be sent"),
        "{text}"
    );
    assert!(!dir.join(".penv/config.toml").exists());
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn a_literal_host_behaves_as_before_and_is_never_pinned() {
    let schema = "# @type=string(startsWith=sk_test_) @hosts=api.stripe.com\nSTRIPE_SECRET_KEY=\n";
    let parsed = penv_schema::parse(schema).unwrap();
    let key = parsed.get("STRIPE_SECRET_KEY").unwrap();
    assert_eq!(key.hosts, ["api.stripe.com"]);
    assert_eq!(key.hosts_expr, None);
    let dir = workspace(schema, "db.internal.test", None);
    let (code, text) = penv(&dir, &["check"]);
    assert_eq!(code, 0, "{text}");
    let (code, text) = penv(&dir, &["check", "--pin-hosts"]);
    assert_eq!(code, 0, "{text}");
    assert_eq!(text.trim(), "");
    assert!(!dir.join(".penv/config.toml").exists());
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn a_computed_host_is_kept_as_written_through_a_render() {
    let parsed = penv_schema::parse(SCHEMA).unwrap();
    let key = parsed.get("DATABASE_URL").unwrap();
    assert!(key.hosts.is_empty());
    assert_eq!(key.hosts_expr.as_deref(), Some("url($DATABASE_URL, host)"));
    let again = penv_schema::parse(&penv_schema::render(&parsed)).unwrap();
    assert_eq!(
        again.get("DATABASE_URL").unwrap().hosts_expr,
        key.hosts_expr
    );
}
