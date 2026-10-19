//! The `public` lists of `.penv/config.toml`: what they declassify, and what
//! `check` refuses in them. No message carries a value.

use std::path::PathBuf;
use std::process::Command;
use std::sync::atomic::{AtomicU32, Ordering};

use serde_json::Value;

const PASSWORD: &str = "fakepass0000";
const PRIVATE_KEY: &str = "fake-private-key-0000";
const API_KEY: &str = "sk_test_FAKEPOLICY0000";

const SCHEMA: &str = "# @schema=1

# @type=url
DATABASE_URL=

# @type=string
API_KEY=

# @type=string
GCP_SA_JSON=

# @type=string @sensitive=false
DB_HOST=url($DATABASE_URL, host)

# @type=string @sensitive=false
DB_USER=url($DATABASE_URL, user)

# @type=string @sensitive=false
API_KEY_FP=hash($API_KEY, algo=sha256)

# @type=string
API_KEY_HASH=hash($API_KEY, algo=sha256)

# @type=string @sensitive=false
NOT_A_HASH=url($DATABASE_URL, scheme)

# @type=string @sensitive=false
SA_EMAIL=json($GCP_SA_JSON, client_email)

# @type=string @sensitive=false
SA_KEY=json($GCP_SA_JSON, private_key)
";

static COUNTER: AtomicU32 = AtomicU32::new(0);

fn workspace(config: Option<&str>) -> PathBuf {
    let n = COUNTER.fetch_add(1, Ordering::SeqCst);
    let dir = std::env::temp_dir().join(format!("penv-policy-{}-{n}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(dir.join(".penv")).unwrap();
    std::fs::write(dir.join(".env.schema"), SCHEMA).unwrap();
    std::fs::write(
        dir.join(".env"),
        format!(
            "DATABASE_URL=postgres://fakeuser:{PASSWORD}@db.internal.test:5432/app\nAPI_KEY={API_KEY}\nGCP_SA_JSON='{{\"client_email\":\"bot@fake.test\",\"private_key\":\"{PRIVATE_KEY}\"}}'\n"
        ),
    )
    .unwrap();
    if let Some(config) = config {
        std::fs::write(dir.join(".penv/config.toml"), config).unwrap();
    }
    dir
}

fn penv(dir: &PathBuf, args: &[&str]) -> (i32, String) {
    let out = Command::new(env!("CARGO_BIN_EXE_penv"))
        .env(
            "PENV_LOCAL_KEY",
            "0000000000000000000000000000000000000000000000000000000000000001",
        )
        .current_dir(dir)
        .args(args)
        .output()
        .unwrap();
    let text = format!(
        "{}{}",
        String::from_utf8_lossy(&out.stdout),
        String::from_utf8_lossy(&out.stderr)
    );
    for secret in [PASSWORD, PRIVATE_KEY, API_KEY] {
        assert!(!text.contains(secret), "a value was printed by {args:?}");
    }
    (out.status.code().unwrap_or(-1), text)
}

/// Each key `ls` lists, with whether it is shown as sensitive.
fn sensitivity(dir: &PathBuf) -> std::collections::BTreeMap<String, bool> {
    let (code, text) = penv(dir, &["--format", "json", "ls"]);
    assert_eq!(code, 0, "{text}");
    let listed: Value = serde_json::from_str(&text).unwrap();
    listed["keys"]
        .as_array()
        .unwrap()
        .iter()
        .map(|k| {
            (
                k["name"].as_str().unwrap().to_string(),
                k["sensitive"].as_bool().unwrap(),
            )
        })
        .collect()
}

#[test]
fn the_default_policy_shows_a_host_and_masks_the_user_the_key_and_every_read() {
    let dir = workspace(None);
    let shown = sensitivity(&dir);
    assert!(!shown["DB_HOST"]);
    assert!(shown["DB_USER"]);
    assert!(shown["API_KEY_FP"]);
    assert!(shown["SA_EMAIL"]);
    assert!(shown["SA_KEY"]);
}

#[test]
fn a_url_list_with_user_declassifies_it_and_one_with_password_fails_check() {
    let dir = workspace(Some(
        "[url]\npublic = [\"scheme\", \"host\", \"port\", \"origin\", \"path\", \"user\"]\n",
    ));
    assert!(!sensitivity(&dir)["DB_USER"]);
    let (code, text) = penv(&dir, &["check"]);
    assert!(!text.contains("[url] public"), "{text}");
    assert_eq!(code, 0, "{text}");

    std::fs::write(
        dir.join(".penv/config.toml"),
        "[url]\npublic = [\"host\", \"password\", \"hots\"]\n",
    )
    .unwrap();
    let (code, text) = penv(&dir, &["check"]);
    assert_ne!(code, 0, "{text}");
    assert!(text.contains("[url] public lists password"), "{text}");
    assert!(text.contains("[url] public lists entry 3"), "{text}");
}

#[test]
fn a_hash_list_declassifies_a_hash_key_and_fails_check_naming_any_other() {
    let dir = workspace(Some("[hash]\npublic = [\"API_KEY_FP\", \"NOT_A_HASH\"]\n"));
    let shown = sensitivity(&dir);
    assert!(!shown["API_KEY_FP"]);
    let (code, text) = penv(&dir, &["check"]);
    assert_ne!(code, 0, "{text}");
    assert!(text.contains("[hash] public lists NOT_A_HASH"), "{text}");
    assert!(text.contains("API_KEY_FP is in [hash] public"), "{text}");
    assert!(!text.contains("[hash] public lists API_KEY_FP"), "{text}");
}

#[test]
fn a_sensitive_hash_key_is_noted_as_kept_and_a_pasted_token_is_not_echoed() {
    let dir = workspace(Some(
        "[hash]\npublic = [\"API_KEY_HASH\", \"sk_live_FAKE0123456789abcdef\"]\n",
    ));
    assert!(sensitivity(&dir)["API_KEY_HASH"]);
    let (code, text) = penv(&dir, &["check"]);
    assert_ne!(code, 0, "{text}");
    assert!(
        text.contains("API_KEY_HASH is in [hash] public but stays sensitive"),
        "{text}"
    );
    assert!(text.contains("@sensitive=false"), "{text}");
    assert!(
        !text.contains("API_KEY_HASH is in [hash] public, so"),
        "{text}"
    );
    assert!(text.contains("[hash] public lists entry 2"), "{text}");
    assert!(!text.contains("sk_live"), "{text}");
}

#[test]
fn a_json_list_declassifies_only_that_read_and_must_name_a_schema_key() {
    let dir = workspace(Some("[json.GCP_SA_JSON]\npublic = [\"client_email\"]\n"));
    let shown = sensitivity(&dir);
    assert!(!shown["SA_EMAIL"]);
    assert!(shown["SA_KEY"]);

    std::fs::write(
        dir.join(".penv/config.toml"),
        "[json.NOT_DECLARED]\npublic = [\"client_email\"]\n",
    )
    .unwrap();
    let (code, text) = penv(&dir, &["check"]);
    assert_ne!(code, 0, "{text}");
    assert!(text.contains("[json.entry 1] names no key"), "{text}");
    assert!(!text.contains("NOT_DECLARED"), "{text}");
}

#[test]
fn check_notes_url_json_and_hash_as_penv_only() {
    let dir = workspace(None);
    let (_, text) = penv(&dir, &["--format", "text", "check"]);
    let line = text
        .lines()
        .find(|l| l.contains("penv-only features"))
        .unwrap_or_else(|| panic!("{text}"));
    for feature in ["url()", "json()", "hash()"] {
        assert!(line.contains(feature), "{line}");
    }
}
