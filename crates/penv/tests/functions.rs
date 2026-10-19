//! Every example in docs/Functions.md, end to end through the real binary:
//! `check` passes, `ls --json` reports the documented sensitivity, `reveal`
//! prints the documented result, and no other command prints a fake secret.

use std::path::PathBuf;
use std::process::Command;
use std::sync::atomic::{AtomicU32, Ordering};

use serde_json::Value;

const PASSWORD: &str = "fakepass";
const API_KEY: &str = "fake_api_key_123";
const WEBHOOK: &str = "fake-webhook-0000";
const ROOT: &str = "fake-root-0000";
const DB_SECRET: &str = r#"{"username":"app","db":{"hosts":["db1.internal.test","db2.internal.test"],"port":5432},"tls":true}"#;

const SCHEMA: &str = "# @schema=1

# @type=url
DATABASE_URL=

# @type=string
API_KEY=

# @type=string
WEBHOOK_SECRET=

# @type=string
ROOT=

# @type=string
DB_SECRET=

# @type=string @sensitive=false
APP_ENV=

# @type=string @sensitive=false
DB_HOST=url($DATABASE_URL, host)

# @type=url
REPLICA_URL=url($DATABASE_URL, host=replica.internal.test, port=5433)

# @type=string
URL_SECRET=

# @type=string
URL_HOST=${URL_SECRET | json(database_url) | url(host)}

# @type=string
DB_FIRST=json($DB_SECRET, db.hosts[0])

# @type=string
DB_HOSTS=json($DB_SECRET, db.hosts)

# @type=string
DB_TLS=json($DB_SECRET, tls)

# @type=string @sensitive=false
FEATURES=json(beta=eq($APP_ENV, staging), maxUploadMb=25, region=eu-1)

# @type=string
API_KEY_FP=hash($API_KEY, algo=sha256, length=8)

# @type=string
TEST_SIG=hash('{\"id\":1}', algo=hmac-sha256, key=$WEBHOOK_SECRET)

# @type=string
SESSION_KEY=hash(session, algo=hmac-sha256, key=$ROOT)

# @type=string
SESSION_SECRET=random(length=32)

# @type=string
COOKIE_KEY=random(kind=hex, length=32)

# @type=string
INSTANCE_UUID=random(kind=uuid)

# @type=string
DECODED=${ENCODED | b64d}

# @type=string @sensitive=false
ENCODED=ZmFrZS12YWx1ZQ==
";

static COUNTER: AtomicU32 = AtomicU32::new(0);

fn workspace() -> PathBuf {
    let n = COUNTER.fetch_add(1, Ordering::SeqCst);
    let dir = std::env::temp_dir().join(format!("penv-functions-{}-{n}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(dir.join(".penv")).unwrap();
    std::fs::write(dir.join(".env.schema"), SCHEMA).unwrap();
    std::fs::write(
        dir.join(".env"),
        format!(
            "DATABASE_URL=postgres://app:{PASSWORD}@db.internal.test:5432/orders?sslmode=require\nAPI_KEY={API_KEY}\nWEBHOOK_SECRET={WEBHOOK}\nROOT={ROOT}\nDB_SECRET='{DB_SECRET}'\nAPP_ENV=staging\nURL_SECRET='{{\"database_url\":\"postgres://app:{PASSWORD}@db.internal.test/orders\"}}'\n"
        ),
    )
    .unwrap();
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
    if args.first() != Some(&"reveal") {
        for secret in [PASSWORD, API_KEY, WEBHOOK, ROOT] {
            assert!(!text.contains(secret), "a value was printed by {args:?}");
        }
    }
    (out.status.code().unwrap_or(-1), text)
}

/// Exits 0 when the child sees KEY equal to `want`; prints nothing.
fn holds(dir: &PathBuf, key: &str, want: &str) -> bool {
    let out = Command::new(env!("CARGO_BIN_EXE_penv"))
        .env(
            "PENV_LOCAL_KEY",
            "0000000000000000000000000000000000000000000000000000000000000001",
        )
        .env("WANT", want)
        .current_dir(dir)
        .args(["run", "--", "sh", "-c"])
        .arg(format!("[ \"${key}\" = \"$WANT\" ]"))
        .output()
        .unwrap();
    out.status.success()
}

fn assert_holds(dir: &PathBuf, key: &str, want: &str) {
    assert!(
        holds(dir, key, want),
        "{key} differs from the documented result"
    );
}

#[test]
fn the_documented_examples_pass_check_and_list_with_the_documented_sensitivity() {
    let dir = workspace();
    let (code, text) = penv(&dir, &["check"]);
    assert_eq!(code, 0, "{text}");

    let (code, text) = penv(&dir, &["--format", "json", "ls"]);
    assert_eq!(code, 0, "{text}");
    let listed: Value = serde_json::from_str(&text).unwrap();
    let shown = |name: &str| {
        listed["keys"]
            .as_array()
            .unwrap()
            .iter()
            .find(|k| k["name"] == name)
            .unwrap_or_else(|| panic!("{name} not listed"))["sensitive"]
            .as_bool()
            .unwrap()
    };
    assert!(!shown("DB_HOST"));
    assert!(shown("REPLICA_URL"));
    assert!(shown("URL_HOST"));
    assert!(shown("DB_FIRST"));
    assert!(shown("API_KEY_FP"));
    assert!(shown("TEST_SIG"));
    assert!(shown("SESSION_KEY"));
    assert!(shown("SESSION_SECRET"));
}

#[test]
fn the_documented_results_reach_a_child_process_as_the_doc_comments_say() {
    let dir = workspace();
    assert_holds(&dir, "DB_HOST", "db.internal.test");
    assert_holds(&dir, "URL_HOST", "db.internal.test");
    assert_holds(&dir, "DB_FIRST", "db1.internal.test");
    assert_holds(
        &dir,
        "DB_HOSTS",
        r#"["db1.internal.test","db2.internal.test"]"#,
    );
    assert_holds(&dir, "DB_TLS", "true");
    assert_holds(
        &dir,
        "FEATURES",
        r#"{"beta":true,"maxUploadMb":25,"region":"eu-1"}"#,
    );
    assert_holds(&dir, "API_KEY_FP", "604d6582");
    assert_holds(&dir, "DECODED", "fake-value");
    assert!(holds_len(&dir, "COOKIE_KEY", 32));
    assert!(holds_len(&dir, "INSTANCE_UUID", 36));
    assert!(holds_len(&dir, "SESSION_SECRET", 32));
}

fn holds_len(dir: &PathBuf, key: &str, len: usize) -> bool {
    let out = Command::new(env!("CARGO_BIN_EXE_penv"))
        .env(
            "PENV_LOCAL_KEY",
            "0000000000000000000000000000000000000000000000000000000000000001",
        )
        .current_dir(dir)
        .args(["run", "--", "sh", "-c"])
        .arg(format!("[ \"${{#{key}}}\" -eq {len} ]"))
        .output()
        .unwrap();
    out.status.success()
}

#[test]
fn random_with_a_bare_length_fails_check_and_says_to_name_it() {
    let dir = workspace();
    std::fs::write(
        dir.join(".env.schema"),
        "# @schema=1\n\n# @type=string\nSESSION_SECRET=random(32)\n",
    )
    .unwrap();
    let (code, text) = penv(&dir, &["check"]);
    assert_ne!(code, 0, "{text}");
    assert!(text.contains("length=32"), "{text}");
}
