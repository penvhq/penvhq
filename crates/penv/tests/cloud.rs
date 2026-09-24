//! Cloud mode end to end: the binary talks to an in-process server on a
//! loopback port, so the tests reach no network and no keychain.

// One mock server, kept where the client that speaks to it lives.
#[path = "../../penv-cloud/tests/common/mod.rs"]
mod server;

use std::path::{Path, PathBuf};
use std::process::{Command, Output, Stdio};
use std::sync::atomic::{AtomicU32, Ordering};

use serde_json::{Value, json};
use server::Mock;

const SECRET: &str = "sk_test_FAKE0000";
const TOKEN: &str = "pck_FAKE";
const PROJECT: &str = "api-gateway";
const ENVS: &str = "/api/v1/envs/acme/api-gateway/development";
/// The slug the server derives for this directory. It is not the directory name.
const SLUG: &str = "api-gateway-2";
const CREATED_ENVS: &str = "/api/v1/envs/acme/api-gateway-2/development";
const APPROVALS: &str = "/api/v1/approvals";
const APPROVAL: &str = "apr_1";
const APPROVAL_URL: &str = "https://penv.cloud/approvals/apr_1";
const EXPIRES: &str = "2026-09-09T12:10:00Z";

const KEYS: &str = "\
# @type=string(startsWith=sk_)
STRIPE_SECRET_KEY=

# @type=port @sensitive=false
PORT=3000
";

fn local_schema() -> String {
    format!("# @schema=1\n\n{KEYS}")
}

fn cloud_schema() -> String {
    format!("# @penv=acme/{PROJECT} @schema=1\n\n{KEYS}")
}

#[cfg(windows)]
const SHELL: [&str; 2] = ["cmd", "/C"];
#[cfg(windows)]
const ECHO_VALUES: &str = "echo %STRIPE_SECRET_KEY% %PORT%";

#[cfg(not(windows))]
const SHELL: [&str; 2] = ["sh", "-c"];
#[cfg(not(windows))]
const ECHO_VALUES: &str = "echo $STRIPE_SECRET_KEY $PORT";

/// Everything penv-agent looks at, so a test says who is driving.
const AGENT_MARKERS: [&str; 14] = [
    "AGENT",
    "AI_AGENT",
    "AMP_CURRENT_THREAD_ID",
    "CLAUDECODE",
    "CLAUDE_CODE_CHILD_SESSION",
    "CLAUDE_CODE_SESSION_ID",
    "CLINE_ACTIVE",
    "CODEX_SESSION_ID",
    "CODEX_THREAD_ID",
    "COPILOT_CLI",
    "CURSOR_AGENT",
    "CURSOR_SANDBOX",
    "GEMINI_CLI",
    "ROO_ACTIVE",
];

/// Credentials the machine running the suite may carry, so a test decides for
/// itself which identity penv finds.
const AMBIENT_CREDENTIALS: [&str; 10] = [
    "ACTIONS_ID_TOKEN_REQUEST_TOKEN",
    "ACTIONS_ID_TOKEN_REQUEST_URL",
    "AWS_ACCESS_KEY_ID",
    "AWS_CONTAINER_CREDENTIALS_FULL_URI",
    "AWS_CONTAINER_CREDENTIALS_RELATIVE_URI",
    "AWS_SECRET_ACCESS_KEY",
    "AWS_SESSION_TOKEN",
    "AWS_WEB_IDENTITY_TOKEN_FILE",
    "ID_TOKEN",
    "PENV_OIDC_TOKEN",
];

static COUNTER: AtomicU32 = AtomicU32::new(0);

/// A scratch project directory. Its name is the project name `push` offers.
struct Workspace {
    root: PathBuf,
    dir: PathBuf,
}

impl Workspace {
    fn new(files: &[(&str, &str)]) -> Workspace {
        let root = std::env::temp_dir().join(format!(
            "penv-cloud-cli-{}-{}",
            std::process::id(),
            COUNTER.fetch_add(1, Ordering::Relaxed)
        ));
        let dir = root.join(PROJECT);
        std::fs::create_dir_all(&dir).expect("a scratch directory");
        for (file, contents) in files {
            if let Some(parent) = dir.join(file).parent() {
                std::fs::create_dir_all(parent).expect("a scratch directory");
            }
            std::fs::write(dir.join(file), contents).expect("a scratch file");
        }
        Workspace { root, dir }
    }

    fn path(&self) -> &Path {
        &self.dir
    }

    fn read(&self, file: &str) -> String {
        std::fs::read_to_string(self.dir.join(file)).unwrap_or_default()
    }

    /// The binary, pointed at the mock, with a credential in the environment and
    /// no cache directory, so nothing on this host is read or written.
    fn command(&self, mock: &Mock) -> Command {
        let mut command = Command::new(env!("CARGO_BIN_EXE_penv"));
        command.env(
            "PENV_LOCAL_KEY",
            "0000000000000000000000000000000000000000000000000000000000000001",
        );
        command
            .current_dir(&self.dir)
            .env("PENV_URL", mock.url())
            .env("PENV_TOKEN", TOKEN)
            .env_remove("PENV_ENV")
            .env_remove("LOCALAPPDATA")
            .env_remove("XDG_CACHE_HOME")
            .env_remove("SSL_CERT_FILE")
            .env_remove("HOME");
        // Ambient AWS keys would be a credential the test never chose.
        for name in [
            "AWS_ACCESS_KEY_ID",
            "AWS_SECRET_ACCESS_KEY",
            "AWS_SESSION_TOKEN",
        ] {
            command.env_remove(name);
        }
        // The suite itself may be running under an agent, and these tests decide
        // for themselves which sessions are one.
        for marker in AGENT_MARKERS.iter().chain(&AMBIENT_CREDENTIALS) {
            command.env_remove(marker);
        }
        command
    }

    fn run(&self, mock: &Mock, args: &[&str]) -> Output {
        self.command(mock).args(args).output().expect("penv runs")
    }

    fn pipe(&self, mock: &Mock, args: &[&str], stdin: &str) -> Output {
        use std::io::Write;

        let mut child = self
            .command(mock)
            .args(args)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .expect("penv runs");
        child
            .stdin
            .take()
            .expect("a pipe")
            .write_all(stdin.as_bytes())
            .expect("the value goes in");
        child.wait_with_output().expect("penv finishes")
    }
}

impl Drop for Workspace {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.root);
    }
}

fn stdout(output: &Output) -> String {
    String::from_utf8_lossy(&output.stdout).into_owned()
}

fn stderr(output: &Output) -> String {
    String::from_utf8_lossy(&output.stderr).into_owned()
}

fn json_of(text: &str) -> Value {
    serde_json::from_str(text).unwrap_or_else(|e| panic!("not JSON ({e}): {text}"))
}

fn approval_body() -> String {
    json!({ "id": APPROVAL, "url": APPROVAL_URL, "expiresAt": EXPIRES }).to_string()
}

fn status_path() -> String {
    format!("{APPROVALS}/{APPROVAL}")
}

fn redeem_path() -> String {
    format!("{APPROVALS}/{APPROVAL}/redeem")
}

/// What the status route answers for an approval waiting on a person.
fn status_body(key: &str, status: &str) -> String {
    json!({
        "id": APPROVAL, "status": status, "key": key,
        "url": APPROVAL_URL, "expiresAt": EXPIRES,
    })
    .to_string()
}

fn values_body() -> String {
    json!({
        "keys": [
            { "path": "", "name": "STRIPE_SECRET_KEY", "kind": "static", "version": 2, "value": SECRET },
            { "path": "", "name": "PORT", "kind": "static", "version": 1, "value": "3000" },
        ]
    })
    .to_string()
}

// --- push -------------------------------------------------------------------

#[test]
fn push_creates_the_project_from_the_directory_and_writes_the_header() {
    let mock = Mock::new();
    mock.on(
        "GET",
        "/api/v1/orgs",
        200,
        &json!({ "orgs": [{ "slug": "acme", "name": "Acme" }] }).to_string(),
    );
    mock.on(
        "POST",
        "/api/v1/orgs/acme/projects",
        201,
        &json!({ "slug": SLUG, "name": PROJECT, "environments": ["development"] }).to_string(),
    );
    mock.on(
        "PUT",
        CREATED_ENVS,
        200,
        &json!({ "written": 2, "unchanged": 0, "pruned": 0, "etag": "\"abc\"" }).to_string(),
    );

    let workspace = Workspace::new(&[
        (".env.schema", &local_schema()),
        (".env", &format!("STRIPE_SECRET_KEY={SECRET}\n")),
    ]);
    let output = workspace.run(&mock, &["--json", "push"]);
    assert_eq!(output.status.code(), Some(0), "{}", stderr(&output));

    let report = json_of(&stdout(&output));
    assert_eq!(
        report["created"],
        format!("acme/{SLUG}"),
        "the slug the server derived, not the directory name"
    );
    assert_eq!(report["written"], 2);
    assert_eq!(report["etag"], "\"abc\"");

    let created = mock.last("POST", "/api/v1/orgs/acme/projects").json();
    assert_eq!(created["name"], PROJECT);
    assert_eq!(created["environments"][0], "development");

    let put = mock.last("PUT", CREATED_ENVS).json();
    assert_eq!(put["prune"], false);
    let sent = put["keys"].as_array().unwrap();
    let secret = sent
        .iter()
        .find(|k| k["name"] == "STRIPE_SECRET_KEY")
        .unwrap();
    assert_eq!(secret["value"], SECRET);
    assert_eq!(secret["schema"]["type"]["name"], "string");
    assert_eq!(
        secret["schema"].get("name"),
        None,
        "the route carries the name"
    );
    for (field, value) in secret["schema"].as_object().unwrap() {
        assert!(!value.is_null(), "{field} was sent as null");
    }

    assert!(
        workspace
            .read(".env.schema")
            .contains(&format!("@penv=acme/{SLUG}")),
        "the header was not written: {}",
        workspace.read(".env.schema")
    );
    assert!(
        !workspace.path().join(".env").exists(),
        ".env was left behind"
    );
}

#[test]
fn push_says_which_organisations_it_could_not_choose_between() {
    let mock = Mock::new();
    mock.on(
        "GET",
        "/api/v1/orgs",
        200,
        &json!({ "orgs": [{ "slug": "acme", "name": "Acme" }, { "slug": "other", "name": "Other" }] })
            .to_string(),
    );
    let workspace = Workspace::new(&[(".env.schema", &local_schema()), (".env", "PORT=3000\n")]);
    let output = workspace.run(&mock, &["--json", "push"]);

    assert_eq!(output.status.code(), Some(1));
    let error = json_of(&stderr(&output));
    assert_eq!(error["error"], "org_required");
    assert!(
        error["message"].as_str().unwrap().contains("acme"),
        "{error}"
    );
    assert!(error["fix"].as_str().unwrap().contains("--org"), "{error}");
}

#[test]
fn push_refuses_when_there_is_nothing_to_push() {
    let mock = Mock::new();
    let workspace = Workspace::new(&[(".env.schema", &local_schema())]);
    let output = workspace.run(&mock, &["--json", "push"]);

    assert_eq!(output.status.code(), Some(3), "{}", stderr(&output));
    let error = json_of(&stderr(&output));
    assert_eq!(error["error"], "nothing_to_push");
    assert!(
        mock.hits("GET", "/api/v1/orgs").is_empty(),
        "it asked the server before finding nothing to send"
    );

    let workspace = Workspace::new(&[(".env.schema", &local_schema()), (".env", "PORT=\n")]);
    let output = workspace.run(&mock, &["--json", "push"]);
    assert_eq!(json_of(&stderr(&output))["error"], "nothing_to_push");
}

#[test]
fn push_creates_the_project_with_the_environment_it_was_asked_for() {
    let mock = Mock::new();
    mock.on(
        "GET",
        "/api/v1/orgs",
        200,
        &json!({ "orgs": [{ "slug": "acme", "name": "Acme" }] }).to_string(),
    );
    mock.on(
        "POST",
        "/api/v1/orgs/acme/projects",
        201,
        &json!({ "slug": SLUG, "name": PROJECT, "environments": ["development", "production"] })
            .to_string(),
    );
    mock.on(
        "PUT",
        "/api/v1/envs/acme/api-gateway-2/production",
        200,
        &json!({ "written": 1, "unchanged": 0, "pruned": 0, "etag": "\"abc\"" }).to_string(),
    );
    let workspace = Workspace::new(&[
        (".env.schema", &local_schema()),
        (".env", &format!("STRIPE_SECRET_KEY={SECRET}\n")),
    ]);
    let output = workspace.run(&mock, &["--json", "push", "--env", "production"]);
    assert_eq!(output.status.code(), Some(0), "{}", stderr(&output));

    let created = mock.last("POST", "/api/v1/orgs/acme/projects").json();
    let environments = created["environments"].as_array().unwrap();
    assert!(environments.contains(&json!("development")));
    assert!(environments.contains(&json!("production")));
}

#[test]
fn push_refuses_an_environment_the_project_does_not_have() {
    let mock = Mock::new();
    mock.on(
        "GET",
        "/api/v1/orgs/acme/projects",
        200,
        &json!({ "projects": [{ "slug": PROJECT, "name": PROJECT, "environments": ["development", "staging"] }] })
            .to_string(),
    );
    let workspace = Workspace::new(&[
        (".env.schema", &cloud_schema()),
        (".env", &format!("STRIPE_SECRET_KEY={SECRET}\n")),
    ]);
    let output = workspace.run(&mock, &["--json", "push", "--env", "production"]);

    assert_eq!(output.status.code(), Some(3), "{}", stderr(&output));
    let error = json_of(&stderr(&output));
    assert_eq!(error["error"], "no_such_environment");
    assert!(
        error["message"].as_str().unwrap().contains("production"),
        "{error}"
    );
    assert!(
        error["fix"].as_str().unwrap().contains("staging"),
        "{error}"
    );
    assert!(
        mock.hits("PUT", "/api/v1/envs/acme/api-gateway/production")
            .is_empty(),
        "it pushed anyway"
    );
}

#[test]
fn push_refuses_an_org_that_contradicts_the_header() {
    let mock = Mock::new();
    let workspace = Workspace::new(&[
        (".env.schema", &cloud_schema()),
        (".env", &format!("STRIPE_SECRET_KEY={SECRET}\n")),
    ]);
    let output = workspace.run(&mock, &["--json", "push", "--org", "other"]);

    assert_eq!(output.status.code(), Some(3), "{}", stderr(&output));
    let error = json_of(&stderr(&output));
    assert_eq!(error["error"], "org_mismatch");
    assert!(
        error["message"].as_str().unwrap().contains("acme"),
        "{error}"
    );
    assert!(mock.hits("PUT", ENVS).is_empty(), "it pushed anyway");
}

// --- pull -------------------------------------------------------------------

#[test]
fn pull_is_refused_for_an_agent_and_written_for_a_person() {
    let mock = Mock::new();
    mock.on("GET", ENVS, 200, &values_body());
    let workspace = Workspace::new(&[(".env.schema", &cloud_schema())]);

    let refused = workspace.run(&mock, &["--agent", "pull"]);
    assert_eq!(refused.status.code(), Some(2));
    assert_eq!(json_of(&stderr(&refused))["error"], "agent_session");
    assert!(!workspace.path().join(".env").exists());
    assert!(mock.hits("GET", ENVS).is_empty(), "it never asked");

    let allowed = workspace.run(&mock, &["--json", "pull", "--i-am-human"]);
    assert_eq!(allowed.status.code(), Some(0), "{}", stderr(&allowed));
    let report = json_of(&stdout(&allowed));
    assert_eq!(report["keys"], 2);
    assert!(
        !stdout(&allowed).contains(SECRET),
        "pull prints the count, not the values"
    );
    assert_eq!(
        workspace.read(".env"),
        format!("STRIPE_SECRET_KEY={SECRET}\nPORT=3000\n")
    );
}

// --- set and unset ----------------------------------------------------------

#[test]
fn set_takes_a_piped_value_echoes_nothing_and_declares_the_new_key() {
    let mock = Mock::new();
    mock.on(
        "PATCH",
        &format!("{ENVS}/keys/NEW_TOKEN"),
        200,
        &json!({ "version": 1, "etag": "\"abc\"" }).to_string(),
    );
    let workspace = Workspace::new(&[(".env.schema", &cloud_schema())]);
    let output = workspace.pipe(&mock, &["--json", "set", "NEW_TOKEN"], "sk_live_FAKE1111\n");

    assert_eq!(output.status.code(), Some(0), "{}", stderr(&output));
    let report = json_of(&stdout(&output));
    assert_eq!(report["key"], "NEW_TOKEN");
    assert_eq!(report["version"], 1);
    assert_eq!(report["schemaAdded"], true);

    assert!(!stdout(&output).contains("sk_live_FAKE1111"), "it echoed");
    assert!(!stderr(&output).contains("sk_live_FAKE1111"), "it echoed");
    assert!(
        !stderr(&output).contains("NEW_TOKEN:"),
        "a piped value is never prompted for: {}",
        stderr(&output)
    );

    assert_eq!(
        mock.last("PATCH", &format!("{ENVS}/keys/NEW_TOKEN")).json()["value"],
        "sk_live_FAKE1111"
    );
    let schema = workspace.read(".env.schema");
    assert!(schema.contains("NEW_TOKEN="), "{schema}");
    assert!(
        !schema.contains("sk_live_FAKE1111"),
        "the value landed: {schema}"
    );
    assert!(
        schema.contains("PORT=3000"),
        "the file was rewritten: {schema}"
    );
}

#[test]
fn set_refuses_a_value_the_shell_would_remember() {
    let mock = Mock::new();
    let workspace = Workspace::new(&[(".env.schema", &cloud_schema())]);
    let output = workspace.run(&mock, &["--json", "set", "PORT", "--value", "3000"]);

    assert_eq!(output.status.code(), Some(1));
    let error = json_of(&stderr(&output));
    assert_eq!(error["error"], "value_on_the_command_line");
    assert!(mock.requests().is_empty(), "it never asked");
}

#[test]
fn unset_removes_the_value_and_keeps_the_block() {
    let mock = Mock::new();
    mock.on(
        "DELETE",
        &format!("{ENVS}/keys/PORT"),
        200,
        &json!({ "etag": "\"abc\"" }).to_string(),
    );
    let workspace = Workspace::new(&[(".env.schema", &cloud_schema())]);
    let output = workspace.run(&mock, &["--json", "unset", "PORT"]);

    assert_eq!(output.status.code(), Some(0), "{}", stderr(&output));
    assert_eq!(json_of(&stdout(&output))["key"], "PORT");
    assert!(workspace.read(".env.schema").contains("PORT=3000"));
}

// --- reveal -----------------------------------------------------------------

#[test]
fn reveal_under_an_agent_asks_for_an_approval_and_exits_four() {
    let mock = Mock::new();
    mock.on("GET", ENVS, 200, &values_body());
    mock.on("POST", APPROVALS, 201, &approval_body());
    let workspace = Workspace::new(&[(".env.schema", &cloud_schema())]);

    let output = workspace
        .command(&mock)
        .env("CLAUDECODE", "1")
        .env("CLAUDE_CODE_SESSION_ID", "sess-9")
        .args(["reveal", "STRIPE_SECRET_KEY"])
        .output()
        .expect("penv runs");

    assert_eq!(output.status.code(), Some(4), "{}", stderr(&output));
    let error = json_of(&stderr(&output));
    assert_eq!(error["error"], "approval_required");
    assert_eq!(error["approval"], APPROVAL);
    assert_eq!(error["url"], APPROVAL_URL);
    assert_eq!(error["expiresAt"], EXPIRES);
    assert!(
        error["fix"]
            .as_str()
            .unwrap()
            .contains("penv reveal STRIPE_SECRET_KEY --approval apr_1"),
        "{error}"
    );
    assert_eq!(stdout(&output), "", "an exit 4 prints nothing at all");
    assert!(
        mock.hits("GET", ENVS).is_empty(),
        "no value is read before a person approves"
    );

    let asked = mock.last("POST", APPROVALS);
    let body = asked.json();
    assert_eq!(body["org"], "acme");
    assert_eq!(body["project"], PROJECT);
    assert_eq!(body["environment"], "development");
    assert_eq!(body["key"], "STRIPE_SECRET_KEY");
    assert!(
        body.get("harness").is_none() && body.get("session").is_none(),
        "the harness and the session are headers, not body members: {body}"
    );
    assert!(
        body["device"].as_str().is_some_and(|name| !name.is_empty()),
        "the console page names no machine: {body}"
    );
    assert_eq!(asked.header("x-penv-agent"), Some("claude-code"));
    assert_eq!(asked.header("x-penv-session"), Some("sess-9"));
}

#[test]
fn a_second_ask_for_the_same_key_reuses_the_request_already_open() {
    let mock = Mock::new();
    mock.on(
        "POST",
        APPROVALS,
        409,
        &json!({ "error": "approval_pending", "id": APPROVAL, "url": APPROVAL_URL }).to_string(),
    );
    let workspace = Workspace::new(&[(".env.schema", &cloud_schema())]);
    let output = workspace.run(&mock, &["--agent", "reveal", "STRIPE_SECRET_KEY"]);

    assert_eq!(output.status.code(), Some(4), "{}", stderr(&output));
    let error = json_of(&stderr(&output));
    assert_eq!(error["error"], "approval_required");
    assert_eq!(error["approval"], APPROVAL);
    assert_eq!(error["url"], APPROVAL_URL);
    // The reuse answer carries no expiry, so nothing invents one.
    assert!(error.get("expiresAt").is_none(), "{error}");
    assert!(
        error["message"]
            .as_str()
            .unwrap()
            .contains("already has an open approval"),
        "a reused request reads as one: {error}"
    );
    assert!(
        error["fix"]
            .as_str()
            .unwrap()
            .contains("penv reveal STRIPE_SECRET_KEY --approval apr_1"),
        "{error}"
    );
    assert_eq!(stdout(&output), "", "an exit 4 prints nothing at all");
}

#[test]
fn an_approved_request_prints_the_value_once() {
    let mock = Mock::new();
    mock.on(
        "GET",
        &status_path(),
        200,
        &status_body("STRIPE_SECRET_KEY", "approved"),
    );
    mock.on(
        "POST",
        &redeem_path(),
        200,
        &json!({ "key": "STRIPE_SECRET_KEY", "value": SECRET }).to_string(),
    );
    let workspace = Workspace::new(&[(".env.schema", &cloud_schema())]);
    let output = workspace
        .command(&mock)
        .env("CLAUDECODE", "1")
        .env("CLAUDE_CODE_SESSION_ID", "sess-9")
        .args(["reveal", "STRIPE_SECRET_KEY", "--approval", APPROVAL])
        .output()
        .expect("penv runs");

    assert_eq!(output.status.code(), Some(0), "{}", stderr(&output));
    assert_eq!(
        json_of(&stdout(&output)),
        json!({ "key": "STRIPE_SECRET_KEY", "value": SECRET })
    );
    assert!(
        mock.hits("GET", ENVS).is_empty(),
        "the value comes from the redemption, not from the environment"
    );

    // Both routes are audit rows, so both carry who asked.
    for asked in [
        mock.last("GET", &status_path()),
        mock.last("POST", &redeem_path()),
    ] {
        assert_eq!(asked.header("x-penv-agent"), Some("claude-code"));
        assert_eq!(asked.header("x-penv-session"), Some("sess-9"));
    }
}

/// A person may hold an approval too, and redeeming one is the same command.
#[test]
fn a_person_at_a_terminal_redeems_an_approval_they_were_handed() {
    let mock = Mock::new();
    mock.on(
        "GET",
        &status_path(),
        200,
        &status_body("STRIPE_SECRET_KEY", "approved"),
    );
    mock.on(
        "POST",
        &redeem_path(),
        200,
        &json!({ "key": "STRIPE_SECRET_KEY", "value": SECRET }).to_string(),
    );
    let workspace = Workspace::new(&[(".env.schema", &cloud_schema())]);
    let output = workspace.run(
        &mock,
        &[
            "--format",
            "text",
            "reveal",
            "STRIPE_SECRET_KEY",
            "--approval",
            APPROVAL,
        ],
    );

    assert_eq!(output.status.code(), Some(0), "{}", stderr(&output));
    assert_eq!(stdout(&output).trim_end(), SECRET);
    let asked = mock.last("GET", &status_path());
    assert_eq!(asked.header("x-penv-agent"), None, "nobody is driving");
    assert_eq!(asked.header("x-penv-session"), None);
}

/// An approval names one key. Redeeming it for another would print a value
/// nobody released, so it is refused before the id is spent.
#[test]
fn an_approval_for_another_key_is_refused_rather_than_spent() {
    let mock = Mock::new();
    mock.on(
        "GET",
        &status_path(),
        200,
        &status_body("DATABASE_URL", "approved"),
    );
    mock.on(
        "POST",
        &redeem_path(),
        200,
        &json!({ "key": "DATABASE_URL", "value": SECRET }).to_string(),
    );
    let workspace = Workspace::new(&[(".env.schema", &cloud_schema())]);
    let output = workspace.run(
        &mock,
        &[
            "--agent",
            "reveal",
            "STRIPE_SECRET_KEY",
            "--approval",
            APPROVAL,
        ],
    );

    assert_eq!(output.status.code(), Some(4), "{}", stderr(&output));
    let error = json_of(&stderr(&output));
    assert_eq!(error["error"], "approval_mismatch");
    assert_eq!(error["approval"], APPROVAL);
    assert!(
        error["message"]
            .as_str()
            .unwrap()
            .contains("is for DATABASE_URL, not STRIPE_SECRET_KEY"),
        "{error}"
    );
    assert_eq!(
        error["fix"], "Run penv reveal STRIPE_SECRET_KEY for its own approval.",
        "{error}"
    );
    assert_eq!(stdout(&output), "", "an exit 4 prints nothing at all");
    assert!(
        mock.hits("POST", &redeem_path()).is_empty(),
        "the approval was spent on the wrong key"
    );
}

#[test]
fn a_request_nobody_has_answered_yet_stays_exit_four() {
    let mock = Mock::new();
    mock.on(
        "POST",
        &redeem_path(),
        409,
        &json!({ "error": "approval_pending" }).to_string(),
    );
    mock.on(
        "GET",
        &status_path(),
        200,
        &status_body("STRIPE_SECRET_KEY", "pending"),
    );
    let workspace = Workspace::new(&[(".env.schema", &cloud_schema())]);
    let output = workspace.run(
        &mock,
        &[
            "--agent",
            "reveal",
            "STRIPE_SECRET_KEY",
            "--approval",
            APPROVAL,
        ],
    );

    assert_eq!(output.status.code(), Some(4), "{}", stderr(&output));
    let error = json_of(&stderr(&output));
    assert_eq!(error["error"], "approval_required");
    assert_eq!(error["approval"], APPROVAL);
    assert_eq!(error["url"], APPROVAL_URL);
    assert!(
        error["message"]
            .as_str()
            .unwrap()
            .contains("not yet approved"),
        "{error}"
    );
    assert_eq!(stdout(&output), "", "an exit 4 prints nothing at all");
}

#[test]
fn a_denied_request_is_exit_two_and_an_expired_one_is_asked_again() {
    let workspace = Workspace::new(&[(".env.schema", &cloud_schema())]);

    let denied = Mock::new();
    denied.on(
        "GET",
        &status_path(),
        200,
        &status_body("STRIPE_SECRET_KEY", "denied"),
    );
    denied.on(
        "POST",
        &redeem_path(),
        409,
        &json!({ "error": "approval_denied" }).to_string(),
    );
    let output = workspace.run(
        &denied,
        &[
            "--agent",
            "reveal",
            "STRIPE_SECRET_KEY",
            "--approval",
            APPROVAL,
        ],
    );
    assert_eq!(output.status.code(), Some(2), "{}", stderr(&output));
    assert_eq!(json_of(&stderr(&output))["error"], "approval_denied");
    assert_eq!(stdout(&output), "", "a refusal prints nothing at all");

    let expired = Mock::new();
    expired.on(
        "GET",
        &status_path(),
        200,
        &status_body("STRIPE_SECRET_KEY", "expired"),
    );
    expired.on(
        "POST",
        &redeem_path(),
        409,
        &json!({ "error": "approval_expired" }).to_string(),
    );
    let output = workspace.run(
        &expired,
        &[
            "--agent",
            "reveal",
            "STRIPE_SECRET_KEY",
            "--approval",
            APPROVAL,
        ],
    );
    assert_eq!(output.status.code(), Some(4), "{}", stderr(&output));
    let error = json_of(&stderr(&output));
    assert_eq!(error["error"], "approval_expired");
    assert!(
        error["fix"]
            .as_str()
            .unwrap()
            .contains("penv reveal STRIPE_SECRET_KEY"),
        "{error}"
    );
    assert_eq!(stdout(&output), "", "an exit 4 prints nothing at all");
}

/// The key is read before the id is spent. When that read fails, or names no
/// key, nothing says the approval is for this key, so it is not redeemed.
#[test]
fn an_approval_whose_key_cannot_be_read_is_left_unspent() {
    let workspace = Workspace::new(&[(".env.schema", &cloud_schema())]);
    for (status, answer, code) in [
        (
            503,
            json!({ "error": "unavailable" }).to_string(),
            "server_error",
        ),
        (
            404,
            json!({ "error": "not_found" }).to_string(),
            "no_approval",
        ),
        (
            200,
            json!({ "id": APPROVAL, "status": "approved", "url": APPROVAL_URL }).to_string(),
            "approval_unreadable",
        ),
    ] {
        let mock = Mock::new();
        mock.on("GET", &status_path(), status, &answer);
        mock.on(
            "POST",
            &redeem_path(),
            200,
            &json!({ "key": "STRIPE_SECRET_KEY", "value": SECRET }).to_string(),
        );
        let output = workspace.run(
            &mock,
            &[
                "--agent",
                "reveal",
                "STRIPE_SECRET_KEY",
                "--approval",
                APPROVAL,
            ],
        );
        assert_ne!(output.status.code(), Some(0), "{code}");
        assert_eq!(json_of(&stderr(&output))["error"], code);
        assert_eq!(stdout(&output), "", "{code}: nothing is printed");
        assert!(
            mock.hits("POST", &redeem_path()).is_empty(),
            "{code}: the approval was spent anyway"
        );
    }
}

#[test]
fn reveal_prints_the_one_value_and_nothing_else() {
    let mock = Mock::new();
    mock.on("GET", ENVS, 200, &values_body());
    let workspace = Workspace::new(&[(".env.schema", &cloud_schema())]);

    let output = workspace.run(&mock, &["--json", "reveal", "PORT"]);
    assert_eq!(output.status.code(), Some(0), "{}", stderr(&output));
    assert_eq!(
        json_of(&stdout(&output)),
        json!({ "key": "PORT", "value": "3000" })
    );

    let plain = workspace.run(&mock, &["--format", "text", "reveal", "PORT"]);
    assert_eq!(stdout(&plain).trim_end(), "3000");
}

// --- run --------------------------------------------------------------------

#[test]
fn run_injects_the_cloud_values_and_stamps_the_session_on_the_request() {
    let mock = Mock::new();
    mock.on("GET", ENVS, 200, &values_body());
    let workspace = Workspace::new(&[(".env.schema", &cloud_schema())]);

    let output = workspace
        .command(&mock)
        .env("CLAUDECODE", "1")
        .env("CLAUDE_CODE_SESSION_ID", "sess-9")
        .args(["run", "--"])
        .args(SHELL)
        .arg(ECHO_VALUES)
        .output()
        .expect("penv runs");

    assert_eq!(output.status.code(), Some(0), "{}", stderr(&output));
    let text = stdout(&output);
    assert!(
        text.contains("3000"),
        "the public value never arrived: {text}"
    );
    assert!(!text.contains(SECRET), "the value came back: {text}");

    let request = mock.last("GET", ENVS);
    assert_eq!(request.header("x-penv-agent"), Some("claude-code"));
    assert_eq!(request.header("x-penv-session"), Some("sess-9"));
    assert_eq!(request.header("authorization"), Some("Bearer pck_FAKE"));
}

#[test]
fn run_maps_a_refused_environment_to_exit_six_and_names_it() {
    let mock = Mock::new();
    mock.on(
        "GET",
        "/api/v1/envs/acme/api-gateway/production",
        403,
        &json!({ "error": "forbidden" }).to_string(),
    );
    let workspace = Workspace::new(&[(".env.schema", &cloud_schema())]);

    let output = workspace
        .command(&mock)
        .args(["--agent", "run", "--env", "production", "--"])
        .args(SHELL)
        .arg(ECHO_VALUES)
        .output()
        .expect("penv runs");

    assert_eq!(output.status.code(), Some(6), "{}", stderr(&output));
    let error = json_of(&stderr(&output));
    assert_eq!(error["error"], "environment_refused");
    assert!(
        error["message"]
            .as_str()
            .unwrap()
            .contains("acme/api-gateway/production"),
        "{error}"
    );
}

#[test]
fn a_host_with_no_credential_says_so_with_exit_five() {
    let mock = Mock::new();
    let workspace = Workspace::new(&[(".env.schema", &cloud_schema())]);

    let output = workspace
        .command(&mock)
        .env_remove("PENV_TOKEN")
        .args(["--agent", "run", "--"])
        .args(SHELL)
        .arg(ECHO_VALUES)
        .output()
        .expect("penv runs");

    assert_eq!(output.status.code(), Some(5), "{}", stderr(&output));
    let error = json_of(&stderr(&output));
    assert_eq!(error["error"], "no_credential");
    assert!(
        error["fix"].as_str().unwrap().contains("penv login"),
        "{error}"
    );
    assert!(mock.requests().is_empty(), "it never asked");
}

// --- state ------------------------------------------------------------------

#[test]
fn bare_penv_reports_cloud_without_saying_which_credential() {
    let mock = Mock::new();
    let workspace = Workspace::new(&[(".env.schema", &cloud_schema())]);
    let output = workspace.run(&mock, &["--json"]);

    let report = json_of(&stdout(&output));
    assert_eq!(report["location"], "cloud");
    assert_eq!(report["project"], format!("acme/{PROJECT}"));
    assert_eq!(report["credential"], true);
    assert_eq!(report["next"], "penv run");
    assert!(report["cacheAge"].is_null(), "no cache directory, no cache");
    assert!(!stdout(&output).contains(TOKEN), "it named the credential");
}

#[test]
fn login_is_refused_in_an_agent_session() {
    let mock = Mock::new();
    let workspace = Workspace::new(&[(".env.schema", &cloud_schema())]);
    let output = workspace.run(&mock, &["--agent", "login"]);

    assert_eq!(output.status.code(), Some(2));
    assert_eq!(json_of(&stderr(&output))["error"], "agent_session");
    assert!(mock.requests().is_empty(), "it never asked");
}

/// A sign-in on a host with no keychain would mint a 30-day credential with
/// nowhere to keep it, so the device flow never starts there.
#[cfg(target_os = "linux")]
#[test]
fn login_on_a_host_with_no_keychain_never_starts_the_device_flow() {
    let mock = Mock::new();
    let workspace = Workspace::new(&[(".env.schema", &cloud_schema())]);
    let output = workspace
        .command(&mock)
        .env(
            "DBUS_SESSION_BUS_ADDRESS",
            "unix:path=/nonexistent/penv-test-bus",
        )
        .arg("login")
        .output()
        .expect("penv runs");

    assert_ne!(output.status.code(), Some(0));
    assert_eq!(json_of(&stderr(&output))["error"], "no_keychain");
    assert!(mock.requests().is_empty(), "it never asked");
}

// --- machine ----------------------------------------------------------------

#[test]
fn machine_enroll_needs_a_secret() {
    let mock = Mock::new();
    let workspace = Workspace::new(&[(".env.schema", &cloud_schema())]);
    let output = workspace.run(&mock, &["--json", "machine", "enroll", ""]);

    assert_eq!(output.status.code(), Some(1));
    assert_eq!(json_of(&stderr(&output))["error"], "no_secret");
    assert!(mock.requests().is_empty(), "it never asked");
}

#[test]
fn push_takes_the_org_slug_from_the_listing_not_from_the_flag() {
    let mock = Mock::new();
    mock.on(
        "GET",
        "/api/v1/orgs",
        200,
        &json!({ "orgs": [{ "slug": "acme", "name": "Acme Corp" }] }).to_string(),
    );
    mock.on(
        "POST",
        "/api/v1/orgs/acme/projects",
        201,
        &json!({ "slug": SLUG, "name": PROJECT }).to_string(),
    );
    mock.on(
        "PUT",
        CREATED_ENVS,
        200,
        &json!({ "written": 1, "unchanged": 0, "pruned": 0, "etag": "\"abc\"" }).to_string(),
    );

    let workspace = Workspace::new(&[
        (".env.schema", &local_schema()),
        (
            ".env",
            "PORT=3000
",
        ),
    ]);
    let output = workspace.run(&mock, &["--json", "push", "--org", "Acme Corp"]);

    assert_eq!(output.status.code(), Some(0), "{}", stderr(&output));
    assert_eq!(json_of(&stdout(&output))["created"], format!("acme/{SLUG}"));
    assert!(
        workspace
            .read(".env.schema")
            .contains(&format!("@penv=acme/{SLUG}")),
        "{}",
        workspace.read(".env.schema")
    );
}

#[test]
fn a_key_an_engine_mints_says_where_it_is_edited() {
    let mock = Mock::new();
    mock.on(
        "PATCH",
        &format!("{ENVS}/keys/PORT"),
        409,
        &json!({ "error": "dynamic" }).to_string(),
    );
    let workspace = Workspace::new(&[(".env.schema", &cloud_schema())]);
    let output = workspace.pipe(
        &mock,
        &["--json", "set", "PORT"],
        "4000
",
    );

    assert_eq!(output.status.code(), Some(3), "{}", stderr(&output));
    let error = json_of(&stderr(&output));
    assert_eq!(error["error"], "dynamic");
    assert!(
        error["fix"].as_str().unwrap().contains("console"),
        "{error}"
    );
}

#[test]
fn an_expired_login_says_to_sign_in_again() {
    let mock = Mock::new();
    mock.on("GET", ENVS, 401, &json!({ "error": "expired" }).to_string());
    let workspace = Workspace::new(&[(".env.schema", &cloud_schema())]);
    let output = workspace.run(&mock, &["--json", "pull", "--i-am-human"]);

    assert_eq!(output.status.code(), Some(2), "{}", stderr(&output));
    let error = json_of(&stderr(&output));
    assert_eq!(error["error"], "expired");
    assert!(
        error["fix"].as_str().unwrap().contains("penv login"),
        "{error}"
    );
}

#[test]
fn a_project_over_the_plan_limit_says_what_the_limit_is() {
    let mock = Mock::new();
    mock.on(
        "GET",
        "/api/v1/orgs",
        200,
        &json!({ "orgs": [{ "slug": "acme", "name": "Acme" }] }).to_string(),
    );
    mock.on(
        "POST",
        "/api/v1/orgs/acme/projects",
        409,
        &json!({ "error": "quota_exceeded" }).to_string(),
    );
    let workspace = Workspace::new(&[
        (".env.schema", &local_schema()),
        (
            ".env",
            "PORT=3000
",
        ),
    ]);
    let output = workspace.run(&mock, &["--json", "push"]);

    assert_eq!(output.status.code(), Some(1), "{}", stderr(&output));
    // The note about what push would create comes first; the refusal is the last line.
    let error = json_of(stderr(&output).lines().next_back().unwrap_or_default());
    assert_eq!(error["error"], "quota_exceeded");
    assert!(
        error["message"].as_str().unwrap().contains("plan"),
        "{error}"
    );
    assert!(
        !workspace.read(".env.schema").contains("@penv="),
        "no header for a project that was never created"
    );
}

fn orgs_with_projects(mock: &Mock, slugs: &[&str]) {
    mock.on(
        "GET",
        "/api/v1/orgs",
        200,
        &json!({ "orgs": [{ "slug": "acme", "name": "Acme" }] }).to_string(),
    );
    let projects: Vec<Value> = slugs.iter().map(|slug| json!({ "slug": slug })).collect();
    mock.on(
        "GET",
        "/api/v1/orgs/acme/projects",
        200,
        &json!({ "projects": projects }).to_string(),
    );
}

#[test]
fn pull_links_a_folder_with_no_header_to_the_one_project_the_account_has() {
    let mock = Mock::new();
    orgs_with_projects(&mock, &[PROJECT]);
    mock.on("GET", ENVS, 200, &values_body());
    let workspace = Workspace::new(&[(".env.schema", &local_schema())]);
    let output = workspace.run(&mock, &["--json", "pull", "--i-am-human"]);

    assert_eq!(output.status.code(), Some(0), "{}", stderr(&output));
    assert!(
        workspace
            .read(".env.schema")
            .starts_with(&format!("# @penv=acme/{PROJECT}")),
        "{}",
        workspace.read(".env.schema")
    );
}

#[test]
fn pull_with_no_header_and_several_projects_lists_them_when_it_cannot_ask() {
    let mock = Mock::new();
    orgs_with_projects(&mock, &[PROJECT, "billing"]);
    let workspace = Workspace::new(&[(".env.schema", &local_schema())]);
    let output = workspace.run(&mock, &["--json", "pull", "--i-am-human"]);

    let error = json_of(&stderr(&output));
    assert_eq!(error["error"], "project_required", "{error}");
    assert!(
        error["message"].as_str().unwrap().contains("acme/billing"),
        "{error}"
    );
    assert_eq!(workspace.read(".env.schema"), local_schema());
}

#[test]
fn i_am_human_from_an_agent_or_through_a_pipe_does_not_speak_for_a_person() {
    let mock = Mock::new();
    mock.on("GET", ENVS, 200, &values_body());
    let workspace = Workspace::new(&[(".env.schema", &cloud_schema())]);

    let flagged = workspace.run(&mock, &["--agent", "pull", "--i-am-human"]);
    assert_eq!(flagged.status.code(), Some(2), "{}", stderr(&flagged));
    assert_eq!(json_of(&stderr(&flagged))["error"], "agent_session");

    let detected = workspace
        .command(&mock)
        .env("CLAUDECODE", "1")
        .args(["--json", "pull", "--i-am-human"])
        .output()
        .expect("penv runs");
    assert_eq!(detected.status.code(), Some(2), "{}", stderr(&detected));
    assert_eq!(json_of(&stderr(&detected))["error"], "agent_session");
    assert!(!workspace.path().join(".env").exists());
    assert!(mock.hits("GET", ENVS).is_empty(), "it never asked");
}

#[test]
fn pull_with_no_header_and_no_login_says_to_sign_in() {
    let mock = Mock::new();
    let workspace = Workspace::new(&[(".env.schema", &local_schema())]);
    let output = workspace
        .command(&mock)
        .env_remove("PENV_TOKEN")
        .args(["--json", "pull", "--i-am-human"])
        .output()
        .expect("penv runs");

    let error = json_of(&stderr(&output));
    assert_eq!(error["error"], "no_credential", "{error}");
    assert!(error["fix"].as_str().unwrap().contains("penv login"));
}

// --- project and env --------------------------------------------------------

const PROJECT_ROUTE: &str = "/api/v1/orgs/acme/projects/api-gateway";

#[test]
fn project_ls_lists_every_project_with_its_environments() {
    let mock = Mock::new();
    orgs_with_projects(&mock, &[PROJECT, "billing"]);
    let workspace = Workspace::new(&[]);
    let output = workspace.run(&mock, &["--json", "project", "ls"]);

    assert_eq!(output.status.code(), Some(0), "{}", stderr(&output));
    let report = json_of(&stdout(&output));
    assert_eq!(report["projects"][1]["project"], "billing");
    assert_eq!(report["projects"][0]["org"], "acme");
}

#[test]
fn renaming_the_project_this_folder_is_linked_to_rewrites_the_header() {
    let mock = Mock::new();
    orgs_with_projects(&mock, &[PROJECT]);
    mock.on(
        "PATCH",
        PROJECT_ROUTE,
        200,
        &json!({ "slug": "gateway", "name": "Gateway" }).to_string(),
    );
    let workspace = Workspace::new(&[(".env.schema", &cloud_schema())]);
    let output = workspace.run(&mock, &["--json", "project", "rename", PROJECT, "Gateway"]);

    assert_eq!(output.status.code(), Some(0), "{}", stderr(&output));
    assert_eq!(mock.last("PATCH", PROJECT_ROUTE).json()["name"], "Gateway");
    assert_eq!(json_of(&stdout(&output))["relinked"], true);
    assert!(
        workspace
            .read(".env.schema")
            .starts_with("# @penv=acme/gateway"),
        "{}",
        workspace.read(".env.schema")
    );
}

#[test]
fn a_delete_without_a_person_at_a_terminal_is_exit_four_with_the_command_to_run() {
    let mock = Mock::new();
    orgs_with_projects(&mock, &[PROJECT]);
    let workspace = Workspace::new(&[]);
    let output = workspace.run(&mock, &["--json", "project", "rm", PROJECT]);

    assert_eq!(output.status.code(), Some(4), "{}", stderr(&output));
    let error = json_of(&stderr(&output));
    assert_eq!(error["error"], "confirmation_required");
    assert_eq!(error["replay"], format!("penv project rm {PROJECT}"));

    let environment = workspace.run(
        &mock,
        &["--json", "env", "rm", "staging", "-p", "acme/billing"],
    );
    assert_eq!(
        environment.status.code(),
        Some(4),
        "{}",
        stderr(&environment)
    );
    assert_eq!(
        json_of(&stderr(&environment))["replay"],
        "penv env rm staging -p acme/billing"
    );
}

#[test]
fn env_copy_names_the_source_and_env_new_does_not() {
    let mock = Mock::new();
    let route = format!("{PROJECT_ROUTE}/environments");
    mock.on(
        "POST",
        &route,
        201,
        &json!({ "name": "staging", "copied": 2 }).to_string(),
    );
    let workspace = Workspace::new(&[(".env.schema", &cloud_schema())]);

    let copied = workspace.run(&mock, &["--json", "env", "copy", "development", "staging"]);
    assert_eq!(copied.status.code(), Some(0), "{}", stderr(&copied));
    assert_eq!(mock.last("POST", &route).json()["from"], "development");
    assert_eq!(json_of(&stdout(&copied))["copied"], 2);

    let made = workspace.run(&mock, &["--json", "env", "new", "staging"]);
    assert_eq!(made.status.code(), Some(0), "{}", stderr(&made));
    assert_eq!(mock.last("POST", &route).json().get("from"), None);
}

#[test]
fn env_commands_in_a_folder_with_no_link_ask_for_the_project() {
    let mock = Mock::new();
    let workspace = Workspace::new(&[(".env.schema", &local_schema())]);
    let output = workspace.run(&mock, &["--json", "env", "ls"]);

    let error = json_of(&stderr(&output));
    assert_eq!(error["error"], "project_required", "{error}");
    assert!(error["fix"].as_str().unwrap().contains("-p <project>"));
}

#[test]
fn ls_in_a_linked_folder_reads_the_environment_it_is_asked_for() {
    let mock = Mock::new();
    let staging = "/api/v1/envs/acme/api-gateway/staging";
    mock.on("GET", staging, 200, &values_body());
    let workspace = Workspace::new(&[(".env.schema", &cloud_schema())]);
    let output = workspace.run(&mock, &["--json", "ls", "--env", "staging"]);

    assert_eq!(output.status.code(), Some(0), "{}", stderr(&output));
    let report = json_of(&stdout(&output));
    assert_eq!(report["env"], "acme/api-gateway/staging");
    assert_eq!(report["keys"][0]["value"], "present");
    assert!(!stdout(&output).contains(SECRET));
}

#[test]
fn a_value_the_file_cannot_hold_is_left_out_and_the_rest_is_written() {
    let mock = Mock::new();
    let body = json!({
        "keys": [
            { "path": "", "name": "PORT", "kind": "static", "version": 1, "value": "3000" },
            { "path": "", "name": "XSS_KEY", "kind": "static", "version": 1, "value": "a\"b'c`d" },
        ]
    });
    mock.on("GET", ENVS, 200, &body.to_string());
    let workspace = Workspace::new(&[(".env.schema", &cloud_schema())]);
    let output = workspace.run(&mock, &["--json", "pull", "--i-am-human"]);

    assert_eq!(output.status.code(), Some(0), "{}", stderr(&output));
    let report = json_of(&stdout(&output));
    assert_eq!(report["keys"], 1);
    assert!(
        report["left_out"][0]
            .as_str()
            .unwrap()
            .starts_with("XSS_KEY "),
        "{report}"
    );
    let written = std::fs::read_to_string(workspace.path().join(".env")).unwrap();
    assert_eq!(written, "PORT=3000\n");
}

#[cfg(unix)]
#[test]
fn a_pulled_env_file_is_readable_only_by_this_account() {
    use std::os::unix::fs::PermissionsExt;

    let mock = Mock::new();
    mock.on("GET", ENVS, 200, &values_body());
    let workspace = Workspace::new(&[(".env.schema", &cloud_schema())]);
    let output = workspace.run(&mock, &["--json", "pull", "--i-am-human"]);
    assert_eq!(output.status.code(), Some(0), "{}", stderr(&output));

    let mode = std::fs::metadata(workspace.path().join(".env"))
        .unwrap()
        .permissions()
        .mode();
    assert_eq!(mode & 0o777, 0o600, "{mode:o}");
}

// --- local files over the cloud ----------------------------------------------

#[test]
fn a_local_layer_wins_over_the_cloud_and_run_names_the_key_it_replaced() {
    let mock = Mock::new();
    mock.on("GET", ENVS, 200, &values_body());
    let workspace = Workspace::new(&[
        (".env.schema", &cloud_schema()),
        (".env.local", "PORT=5151\n"),
    ]);
    let output = workspace
        .command(&mock)
        .args(["--agent", "run", "--"])
        .args(SHELL)
        .arg(ECHO_VALUES)
        .output()
        .expect("penv runs");
    assert_eq!(output.status.code(), Some(0), "{}", stderr(&output));
    assert!(stdout(&output).contains("5151"), "{}", stdout(&output));
    let warned = stderr(&output);
    assert!(
        warned.contains("PORT (") && warned.contains(".env.local"),
        "{warned}"
    );
    assert!(!warned.contains(SECRET) && !stdout(&output).contains(SECRET));
}

#[test]
fn a_penv_reference_reads_another_environment_once_from_the_cloud() {
    let mock = Mock::new();
    mock.on("GET", ENVS, 200, &values_body());
    let production = "/api/v1/envs/acme/api-gateway/production";
    mock.on(
        "GET",
        production,
        200,
        &json!({ "keys": [ { "path": "", "name": "PORT", "kind": "static", "version": 4, "value": "8443" } ] })
            .to_string(),
    );
    let schema = format!(
        "# @penv=acme/{PROJECT} @schema=1\n\n# @type=string(startsWith=sk_)\nSTRIPE_SECRET_KEY=\n\n# @type=port @sensitive=false\nPORT=penv(production/PORT)\n"
    );
    let workspace = Workspace::new(&[(".env.schema", &schema)]);
    let output = workspace
        .command(&mock)
        .args(["--agent", "run", "--"])
        .args(SHELL)
        .arg(if cfg!(windows) {
            "echo %PORT%"
        } else {
            "echo $PORT"
        })
        .output()
        .expect("penv runs");
    // The development body sets PORT itself, which wins over the default.
    assert_eq!(output.status.code(), Some(0), "{}", stderr(&output));
    assert!(stdout(&output).contains("3000"), "{}", stdout(&output));
    assert!(
        mock.hits("GET", production).is_empty(),
        "a default that never applies is not fetched"
    );
}

#[test]
fn check_counts_rotation_from_the_cloud_write_time() {
    let mock = Mock::new();
    mock.on(
        "GET",
        ENVS,
        200,
        &json!({ "keys": [
            { "path": "", "name": "STRIPE_SECRET_KEY", "kind": "static", "version": 2, "value": SECRET, "updatedAt": "2020-01-01T00:00:00Z" },
            { "path": "", "name": "PORT", "kind": "static", "version": 1, "value": "3000" },
        ] })
        .to_string(),
    );
    let schema = format!(
        "# @penv=acme/{PROJECT} @schema=1\n\n# @type=string(startsWith=sk_) @rotate=90d\nSTRIPE_SECRET_KEY=\n\n# @type=port @sensitive=false\nPORT=3000\n"
    );
    let workspace = Workspace::new(&[(".env.schema", &schema)]);
    let output = workspace.run(&mock, &["--json", "check"]);
    assert_eq!(output.status.code(), Some(0), "{}", stderr(&output));
    let report = json_of(&stdout(&output));
    assert_eq!(
        report["rotation"][0]["due"], "2020-03-31T00:00:00Z",
        "{report}"
    );
    assert_eq!(report["rotation"][0]["overdue"], true);
    assert!(!stdout(&output).contains(SECRET));
}

#[test]
fn a_computed_default_fetches_its_address_once_for_every_key_that_names_it() {
    let mock = Mock::new();
    mock.on(
        "GET",
        ENVS,
        200,
        &json!({ "keys": [ { "path": "", "name": "STRIPE_SECRET_KEY", "kind": "static", "version": 2, "value": SECRET } ] })
            .to_string(),
    );
    let production = "/api/v1/envs/acme/api-gateway/production";
    mock.on(
        "GET",
        production,
        200,
        &json!({ "keys": [ { "path": "", "name": "PORT", "kind": "static", "version": 4, "value": "8443" } ] })
            .to_string(),
    );
    let schema = format!(
        "# @penv=acme/{PROJECT} @schema=1\n\n# @type=string(startsWith=sk_)\nSTRIPE_SECRET_KEY=\n\n# @type=port @sensitive=false\nPORT=penv(production/PORT)\n\n# @type=port @sensitive=false\nPORT_TOO=penv(production/PORT)\n"
    );
    let workspace = Workspace::new(&[(".env.schema", &schema)]);
    let output = workspace
        .command(&mock)
        .args(["--agent", "run", "--"])
        .args(SHELL)
        .arg(if cfg!(windows) {
            "echo %PORT% %PORT_TOO%"
        } else {
            "echo $PORT $PORT_TOO"
        })
        .output()
        .expect("penv runs");
    assert_eq!(output.status.code(), Some(0), "{}", stderr(&output));
    assert!(stdout(&output).contains("8443 8443"), "{}", stdout(&output));
    assert_eq!(
        mock.hits("GET", production).len(),
        1,
        "one read per address"
    );
}

// --- AWS credentials for containers -------------------------------------------

fn aws_ready(mock: &Mock) {
    mock.on("GET", ENVS, 200, &values_body());
    mock.on(
        "POST",
        "/api/v1/auth/aws",
        200,
        &json!({ "credential": "pcm_FAKE", "expiresIn": 900 }).to_string(),
    );
}

fn without_token(workspace: &Workspace, mock: &Mock) -> Command {
    let mut command = workspace.command(mock);
    command.env_remove("PENV_TOKEN");
    for name in [
        "AWS_ACCESS_KEY_ID",
        "AWS_SECRET_ACCESS_KEY",
        "AWS_SESSION_TOKEN",
        "AWS_WEB_IDENTITY_TOKEN_FILE",
        "AWS_ROLE_ARN",
        "AWS_CONTAINER_CREDENTIALS_RELATIVE_URI",
        "AWS_CONTAINER_CREDENTIALS_FULL_URI",
        "ACTIONS_ID_TOKEN_REQUEST_URL",
        "ACTIONS_ID_TOKEN_REQUEST_TOKEN",
        "CI_JOB_JWT_V2",
        "ID_TOKEN",
        "PENV_OIDC_TOKEN",
    ] {
        command.env_remove(name);
    }
    command
}

#[test]
fn an_ecs_or_eks_pod_identity_container_proves_itself_with_its_endpoint_credentials() {
    let mock = Mock::new();
    aws_ready(&mock);
    mock.on(
        "GET",
        "/creds",
        200,
        &json!({ "AccessKeyId": "ASIACONTAINER", "SecretAccessKey": "s3cr3t", "Token": "t0k" })
            .to_string(),
    );
    let workspace = Workspace::new(&[(".env.schema", &cloud_schema())]);
    std::fs::write(workspace.path().join("pod-token"), "pod-auth\n").unwrap();
    let output = without_token(&workspace, &mock)
        .env(
            "AWS_CONTAINER_CREDENTIALS_FULL_URI",
            format!("{}/creds", mock.url()),
        )
        .env(
            "AWS_CONTAINER_AUTHORIZATION_TOKEN_FILE",
            workspace.path().join("pod-token"),
        )
        .args(["--agent", "run", "--"])
        .args(SHELL)
        .arg(ECHO_VALUES)
        .output()
        .unwrap();
    assert_eq!(output.status.code(), Some(0), "{}", stderr(&output));
    assert_eq!(
        mock.last("GET", "/creds").header("authorization"),
        Some("pod-auth")
    );
    let proof = mock.last("POST", "/api/v1/auth/aws").body;
    assert!(proof.contains("ASIACONTAINER"), "{proof}");
    assert!(
        !proof.contains("s3cr3t"),
        "the secret key signs, it is never sent"
    );
}

#[test]
fn a_container_uri_to_an_arbitrary_host_is_never_called() {
    let mock = Mock::new();
    aws_ready(&mock);
    let workspace = Workspace::new(&[(".env.schema", &cloud_schema())]);
    let output = without_token(&workspace, &mock)
        .env(
            "AWS_CONTAINER_CREDENTIALS_FULL_URI",
            "http://evil.invalid/creds",
        )
        .env("AWS_CONTAINER_AUTHORIZATION_TOKEN", "pod-auth")
        .args(["--agent", "run", "--", "true"])
        .output()
        .unwrap();
    assert_eq!(
        output.status.code(),
        Some(5),
        "no credential: {}",
        stderr(&output)
    );
    assert!(mock.hits("POST", "/api/v1/auth/aws").is_empty());
}

#[test]
fn an_eks_irsa_pod_trades_its_token_file_for_keys_then_proves_itself() {
    let mock = Mock::new();
    aws_ready(&mock);
    mock.on(
        "POST",
        "/",
        200,
        "<AssumeRoleWithWebIdentityResponse><AssumeRoleWithWebIdentityResult><Credentials><AccessKeyId>ASIAWEBID</AccessKeyId><SecretAccessKey>w3bs3cr3t</SecretAccessKey><SessionToken>st</SessionToken></Credentials></AssumeRoleWithWebIdentityResult></AssumeRoleWithWebIdentityResponse>",
    );
    let workspace = Workspace::new(&[(".env.schema", &cloud_schema())]);
    std::fs::write(workspace.path().join("sa-token"), "eyJ.pod.jwt\n").unwrap();
    let output = without_token(&workspace, &mock)
        .env(
            "AWS_WEB_IDENTITY_TOKEN_FILE",
            workspace.path().join("sa-token"),
        )
        .env("AWS_ROLE_ARN", "arn:aws:iam::123456789012:role/app")
        .env("AWS_ENDPOINT_URL_STS", mock.url())
        .args(["--agent", "run", "--", "true"])
        .output()
        .unwrap();
    assert_eq!(output.status.code(), Some(0), "{}", stderr(&output));
    let sts = mock.last("POST", "/").body;
    assert!(sts.contains("Action=AssumeRoleWithWebIdentity"), "{sts}");
    assert!(
        sts.contains("RoleArn=arn%3Aaws%3Aiam%3A%3A123456789012%3Arole%2Fapp"),
        "{sts}"
    );
    assert!(sts.contains("WebIdentityToken=eyJ.pod.jwt"), "{sts}");
    let proof = mock.last("POST", "/api/v1/auth/aws").body;
    assert!(
        proof.contains("ASIAWEBID") && !proof.contains("w3bs3cr3t"),
        "{proof}"
    );
}

#[test]
fn a_refused_web_identity_says_why_without_echoing_the_token() {
    let mock = Mock::new();
    aws_ready(&mock);
    mock.on(
        "POST",
        "/",
        403,
        "<ErrorResponse><Error><Code>AccessDenied</Code></Error></ErrorResponse>",
    );
    let workspace = Workspace::new(&[(".env.schema", &cloud_schema())]);
    std::fs::write(workspace.path().join("sa-token"), "eyJ.secret.jwt").unwrap();
    let output = without_token(&workspace, &mock)
        .env(
            "AWS_WEB_IDENTITY_TOKEN_FILE",
            workspace.path().join("sa-token"),
        )
        .env("AWS_ROLE_ARN", "arn:aws:iam::1:role/app")
        .env("AWS_ENDPOINT_URL_STS", mock.url())
        .args(["--agent", "run", "--", "true"])
        .output()
        .unwrap();
    assert_ne!(output.status.code(), Some(0));
    let text = format!("{}{}", stdout(&output), stderr(&output));
    assert!(text.contains("AccessDenied"), "{text}");
    assert!(!text.contains("eyJ.secret.jwt"));
}

// --- which certificate authorities are trusted ---------------------------------

#[test]
fn a_ca_bundle_the_user_can_write_is_refused_for_an_agent_and_used_for_a_person() {
    let mock = Mock::new();
    mock.on("GET", ENVS, 200, &values_body());
    let workspace = Workspace::new(&[(".env.schema", &cloud_schema())]);
    let bundle = workspace.path().join("ca.pem");
    let system = [
        "/etc/ssl/certs/ca-certificates.crt",
        "/etc/pki/tls/certs/ca-bundle.crt",
        "/etc/ssl/cert.pem",
    ]
    .into_iter()
    .find(|p| std::path::Path::new(p).is_file());
    let Some(system) = system else {
        return;
    };
    std::fs::copy(system, &bundle).unwrap();

    let agent = workspace
        .command(&mock)
        .env("SSL_CERT_FILE", &bundle)
        .args(["--agent", "run", "--"])
        .args(SHELL)
        .arg(ECHO_VALUES)
        .output()
        .unwrap();
    assert_ne!(agent.status.code(), Some(0));
    let said = format!("{}{}", stdout(&agent), stderr(&agent));
    assert!(said.contains("untrusted_ca_bundle"), "{said}");
    assert!(
        mock.hits("GET", ENVS).is_empty(),
        "nothing is sent through a bundle an agent could have written"
    );

    let person = workspace
        .command(&mock)
        .env("SSL_CERT_FILE", &bundle)
        .args(["run", "--"])
        .args(SHELL)
        .arg(ECHO_VALUES)
        .output()
        .unwrap();
    assert_eq!(person.status.code(), Some(0), "{}", stderr(&person));

    std::fs::write(&bundle, "not a certificate\n").unwrap();
    let broken = workspace
        .command(&mock)
        .env("SSL_CERT_FILE", &bundle)
        .args(["run", "--"])
        .args(SHELL)
        .arg(ECHO_VALUES)
        .output()
        .unwrap();
    assert!(
        stderr(&broken).contains("holds no PEM certificate")
            || stdout(&broken).contains("holds no PEM certificate"),
        "{} {}",
        stdout(&broken),
        stderr(&broken)
    );
}

// --- providers ----------------------------------------------------------------------

fn provider_schema(prefix: &str) -> String {
    cloud_schema().replacen("@penv=acme/", &format!("@penv={prefix}acme/"), 1)
}

fn run_values(workspace: &Workspace, mock: &Mock, extra: &[&str]) -> std::process::Output {
    workspace
        .command(mock)
        .args(extra)
        .args(["run", "--"])
        .args(SHELL)
        .arg(ECHO_VALUES)
        .output()
        .unwrap()
}

#[test]
fn penv_named_as_the_provider_reads_like_no_prefix() {
    let mock = Mock::new();
    mock.on("GET", ENVS, 200, &values_body());
    let workspace = Workspace::new(&[(".env.schema", &provider_schema("penv:"))]);
    let out = run_values(&workspace, &mock, &[]);
    assert_eq!(out.status.code(), Some(0), "{}", stderr(&out));
    assert_eq!(mock.hits("GET", ENVS).len(), 1);
}

#[test]
fn a_provider_penv_does_not_have_is_refused_by_name_before_any_request() {
    let mock = Mock::new();
    mock.on("GET", ENVS, 200, &values_body());
    let workspace = Workspace::new(&[(".env.schema", &provider_schema("doppler:"))]);
    let out = run_values(&workspace, &mock, &[]);
    assert_ne!(out.status.code(), Some(0));
    let said = format!("{}{}", stdout(&out), stderr(&out));
    assert!(
        said.contains("unknown_provider") && said.contains("doppler") && said.contains("penv"),
        "{said}"
    );
    assert!(
        mock.hits("GET", ENVS).is_empty(),
        "nothing is sent to a provider penv does not know"
    );

    // --provider picks one for this run, whatever the header says.
    let out = run_values(&workspace, &mock, &["--provider", "penv"]);
    assert_eq!(out.status.code(), Some(0), "{}", stderr(&out));
    assert_eq!(mock.hits("GET", ENVS).len(), 1);
}

#[test]
fn a_root_named_only_in_config_never_receives_a_token_from_the_environment() {
    let mock = Mock::new();
    mock.on("GET", ENVS, 200, &values_body());
    let config = format!("[providers.penv]\nurl = \"{}\"\n", mock.url());
    let workspace = Workspace::new(&[
        (".env.schema", &cloud_schema()),
        (".penv/config.toml", &config),
    ]);
    // PENV_TOKEN is set by the harness; only config.toml names the root.
    let out = workspace
        .command(&mock)
        .env_remove("PENV_URL")
        .args(["run", "--"])
        .args(SHELL)
        .arg(ECHO_VALUES)
        .output()
        .unwrap();
    assert_eq!(out.status.code(), Some(5), "{}", stderr(&out));
    let said = format!("{}{}", stdout(&out), stderr(&out));
    assert!(
        said.contains("credential_withheld") && said.contains("PENV_URL="),
        "{said}"
    );
    assert!(!said.contains(TOKEN), "the token is never printed");
    assert!(
        mock.requests().is_empty(),
        "nothing at all reached the config-named root"
    );

    // Naming the same root in PENV_URL is a choice made on purpose: the token goes.
    let out = run_values(&workspace, &mock, &[]);
    assert_eq!(out.status.code(), Some(0), "{}", stderr(&out));
    assert_eq!(mock.hits("GET", ENVS).len(), 1);
}

#[test]
fn penv_url_beats_the_root_config_names() {
    let mock = Mock::new();
    mock.on("GET", ENVS, 200, &values_body());
    let workspace = Workspace::new(&[
        (".env.schema", &cloud_schema()),
        (
            ".penv/config.toml",
            "[providers.penv]\nurl = \"http://127.0.0.1:9\"\n",
        ),
    ]);
    let out = run_values(&workspace, &mock, &[]);
    assert_eq!(out.status.code(), Some(0), "{}", stderr(&out));
    assert_eq!(mock.hits("GET", ENVS).len(), 1);
}

#[test]
fn ci_and_aws_credentials_are_withheld_from_a_config_named_root_too() {
    let mock = Mock::new();
    let config = format!("[providers.penv]\nurl = \"{}\"\n", mock.url());
    let workspace = Workspace::new(&[
        (".env.schema", &cloud_schema()),
        (".penv/config.toml", &config),
    ]);
    let kinds: [&[(&str, &str)]; 2] = [
        &[("PENV_OIDC_TOKEN", "eyJ.FAKE.jwt")],
        &[
            ("AWS_ACCESS_KEY_ID", "AKIAFAKE"),
            ("AWS_SECRET_ACCESS_KEY", "FAKEsecret"),
        ],
    ];
    for vars in kinds {
        let mut command = workspace.command(&mock);
        command.env_remove("PENV_URL").env_remove("PENV_TOKEN");
        for (k, v) in vars {
            command.env(k, v);
        }
        let out = command
            .args(["run", "--"])
            .args(SHELL)
            .arg(ECHO_VALUES)
            .output()
            .unwrap();
        assert_eq!(out.status.code(), Some(5), "{vars:?}: {}", stderr(&out));
        assert!(
            stderr(&out).contains("credential_withheld")
                || stdout(&out).contains("credential_withheld")
        );
    }
    assert!(
        mock.requests().is_empty(),
        "no proof of any kind reached it"
    );
}

fn hosts_workspace() -> Workspace {
    let schema = cloud_schema().replacen(
        "STRIPE_SECRET_KEY=",
        "# @hosts=api.stripe.com\nSTRIPE_SECRET_KEY=",
        1,
    );
    Workspace::new(&[
        (".env.schema", &schema),
        (".env", &format!("STRIPE_SECRET_KEY={SECRET}\n")),
    ])
}

fn projects(mock: &Mock) {
    mock.on(
        "GET",
        "/api/v1/orgs",
        200,
        &json!({ "orgs": [{ "slug": "acme", "name": "Acme" }] }).to_string(),
    );
    mock.on(
        "GET",
        "/api/v1/orgs/acme/projects",
        200,
        &json!({ "projects": [{ "slug": SLUG, "name": PROJECT, "environments": ["development"] }] }).to_string(),
    );
}

const PUT_OK: &str = r#"{ "written": 1, "unchanged": 0, "pruned": 0, "etag": "\"abc\"" }"#;

#[test]
fn push_sends_hosts_in_the_key_schema() {
    let mock = Mock::new();
    projects(&mock);
    mock.on("PUT", ENVS, 200, PUT_OK);
    let workspace = hosts_workspace();
    let output = workspace.run(&mock, &["--json", "push"]);
    assert_eq!(output.status.code(), Some(0), "{}", stderr(&output));
    let puts = mock.hits("PUT", ENVS);
    assert_eq!(puts.len(), 1);
    let body = puts[0].json();
    let key = body["keys"]
        .as_array()
        .unwrap()
        .iter()
        .find(|k| k["name"] == "STRIPE_SECRET_KEY")
        .unwrap()
        .clone();
    assert_eq!(key["schema"]["hosts"], json!(["api.stripe.com"]), "{body}");
    assert!(!stderr(&output).contains("does not store @hosts"));
}

#[test]
fn a_server_that_does_not_store_hosts_yet_gets_the_push_without_it() {
    let mock = Mock::new();
    projects(&mock);
    mock.on("PUT", ENVS, 400, r#"{ "error": "schema_invalid" }"#);
    mock.on("PUT", ENVS, 200, PUT_OK);
    let workspace = hosts_workspace();
    let output = workspace.run(&mock, &["--json", "push"]);
    assert_eq!(output.status.code(), Some(0), "{}", stderr(&output));
    let puts = mock.hits("PUT", ENVS);
    assert_eq!(puts.len(), 2, "one retry, no more");
    assert!(puts[0].body.contains("\"hosts\""));
    assert!(!puts[1].body.contains("\"hosts\""), "{}", puts[1].body);
    assert!(
        stderr(&output).contains("does not store @hosts yet"),
        "{}",
        stderr(&output)
    );
    let schema = std::fs::read_to_string(workspace.path().join(".env.schema")).unwrap();
    assert!(
        schema.contains("@hosts=api.stripe.com"),
        "the file keeps it"
    );
}

#[test]
fn a_schema_error_with_no_hosts_to_drop_is_not_retried() {
    let mock = Mock::new();
    projects(&mock);
    mock.on("PUT", ENVS, 400, r#"{ "error": "schema_invalid" }"#);
    let workspace = Workspace::new(&[
        (".env.schema", &cloud_schema()),
        (".env", &format!("STRIPE_SECRET_KEY={SECRET}\n")),
    ]);
    let output = workspace.run(&mock, &["--json", "push"]);
    assert_ne!(output.status.code(), Some(0));
    assert_eq!(mock.hits("PUT", ENVS).len(), 1);
}

#[test]
fn push_sends_an_encrypted_value_decrypted() {
    let mock = Mock::new();
    projects(&mock);
    mock.on("PUT", ENVS, 200, PUT_OK);
    let workspace = hosts_workspace();
    let encrypted = workspace.run(&mock, &["encrypt"]);
    assert_eq!(encrypted.status.code(), Some(0), "{}", stderr(&encrypted));
    assert!(workspace.read(".env").contains("enc:v1:"));
    let output = workspace.run(&mock, &["--json", "push"]);
    assert_eq!(output.status.code(), Some(0), "{}", stderr(&output));
    let body = mock.last("PUT", ENVS).json();
    let key = body["keys"]
        .as_array()
        .unwrap()
        .iter()
        .find(|k| k["name"] == "STRIPE_SECRET_KEY")
        .unwrap()
        .clone();
    assert_eq!(
        key["value"], SECRET,
        "the cloud gets the value, never the local encryption"
    );
}

// --- write-only environments ------------------------------------------------

const PRODUCTION: &str = "/api/v1/envs/acme/api-gateway/production";
const DB_PASSWORD: &str = "db_FAKE_local_0000";

fn write_only_keys() -> String {
    format!("{KEYS}\n# @type=string\nDB_PASSWORD=\n")
}

fn write_only_schema() -> String {
    format!("# @penv=acme/{PROJECT} @schema=1\n\n{}", write_only_keys())
}

/// A write-only environment as a person's login reads it: DB_PASSWORD, and
/// each of `withheld`, present and redacted.
fn write_only_body(withheld: &[&str]) -> String {
    let mut keys = vec![
        json!({ "path": "", "name": "PORT", "kind": "static", "version": 1, "value": "3000" }),
        json!({ "path": "", "name": "DB_PASSWORD", "kind": "static", "version": 4, "redacted": true }),
    ];
    if withheld.contains(&"STRIPE_SECRET_KEY") {
        keys.push(json!({ "path": "", "name": "STRIPE_SECRET_KEY", "kind": "static", "version": 2, "redacted": true }));
    } else {
        keys.push(json!({ "path": "", "name": "STRIPE_SECRET_KEY", "kind": "static", "version": 2, "value": SECRET }));
    }
    json!({ "keys": keys, "writeOnly": true }).to_string()
}

fn run_in(workspace: &Workspace, mock: &Mock, environment: &str) -> Output {
    workspace
        .command(mock)
        .args(["--agent", "run", "--env", environment, "--"])
        .args(SHELL)
        .arg(ECHO_VALUES)
        .output()
        .expect("penv runs")
}

#[test]
fn pull_writes_a_redacted_marker_and_no_value_line_for_a_write_only_key() {
    let mock = Mock::new();
    mock.on("GET", PRODUCTION, 200, &write_only_body(&[]));
    let workspace = Workspace::new(&[(".env.schema", &write_only_schema())]);

    let output = workspace.run(
        &mock,
        &["--json", "pull", "--env", "production", "--i-am-human"],
    );
    assert_eq!(output.status.code(), Some(0), "{}", stderr(&output));
    let report = json_of(&stdout(&output));
    assert_eq!(report["redacted"], json!(["DB_PASSWORD"]));
    assert_eq!(report["keys"], 2);
    assert_eq!(report["skipped"], json!([]));

    let written = workspace.read(".env.production");
    assert_eq!(
        written,
        format!("PORT=3000\nSTRIPE_SECRET_KEY={SECRET}\n# penv:redacted DB_PASSWORD\n")
    );
    assert!(!written.contains("DB_PASSWORD="), "{written}");
    assert!(!stderr(&output).contains("penv set"), "{}", stderr(&output));

    let text = workspace.run(
        &mock,
        &[
            "--format",
            "text",
            "pull",
            "--env",
            "production",
            "--i-am-human",
        ],
    );
    assert!(
        stdout(&text)
            .contains("Write-only in penv-cloud, written as a redacted marker: DB_PASSWORD"),
        "{}",
        stdout(&text)
    );
}

#[test]
fn run_refuses_a_write_only_key_naming_every_key_and_the_environment() {
    let mock = Mock::new();
    mock.on(
        "GET",
        PRODUCTION,
        200,
        &write_only_body(&["STRIPE_SECRET_KEY"]),
    );
    let workspace = Workspace::new(&[(".env.schema", &write_only_schema())]);

    let output = run_in(&workspace, &mock, "production");
    assert_eq!(output.status.code(), Some(6), "{}", stderr(&output));
    let error = json_of(&stderr(&output));
    assert_eq!(error["error"], "redacted");
    assert_eq!(
        error["message"],
        "DB_PASSWORD, STRIPE_SECRET_KEY in production are write-only in penv-cloud."
    );
    assert_eq!(
        error["fix"],
        "Run it where a workload identity (OIDC, AWS IAM or bound keypair) reads the environment, or set them in .env.production.local."
    );
    assert!(!stdout(&output).contains("3000"), "the command ran");
}

#[test]
fn a_local_overlay_supplies_a_write_only_key_and_run_goes_ahead() {
    let mock = Mock::new();
    mock.on("GET", PRODUCTION, 200, &write_only_body(&[]));
    let workspace = Workspace::new(&[
        (".env.schema", &write_only_schema()),
        (
            ".env.production.local",
            &format!("DB_PASSWORD={DB_PASSWORD}\n"),
        ),
    ]);

    let output = run_in(&workspace, &mock, "production");
    assert_eq!(output.status.code(), Some(0), "{}", stderr(&output));
    assert!(stdout(&output).contains("3000"), "{}", stdout(&output));
    assert!(!stdout(&output).contains(DB_PASSWORD));
    assert!(!stderr(&output).contains("redacted"), "{}", stderr(&output));
}

#[test]
fn check_ls_and_why_treat_a_write_only_key_as_present_and_withheld() {
    let mock = Mock::new();
    mock.on("GET", PRODUCTION, 200, &write_only_body(&[]));
    let workspace = Workspace::new(&[(".env.schema", &write_only_schema())]);

    let check = workspace.run(&mock, &["--json", "check", "--env", "production"]);
    let report = json_of(&stdout(&check));
    assert_eq!(check.status.code(), Some(0), "{report}");
    assert_eq!(report["redacted"], json!(["DB_PASSWORD"]));
    assert!(
        !report["violations"].to_string().contains("DB_PASSWORD"),
        "{report}"
    );
    assert!(
        report["notes"]
            .to_string()
            .contains("DB_PASSWORD in production is write-only in penv-cloud"),
        "{report}"
    );

    let ls = workspace.run(&mock, &["--json", "ls", "--env", "production"]);
    assert_eq!(ls.status.code(), Some(0), "{}", stderr(&ls));
    let listed = json_of(&stdout(&ls));
    let row = listed["keys"]
        .as_array()
        .unwrap()
        .iter()
        .find(|k| k["name"] == "DB_PASSWORD")
        .unwrap();
    assert_eq!(row["value"], "redacted");

    let why = workspace.run(
        &mock,
        &["--json", "why", "DB_PASSWORD", "--env", "production"],
    );
    assert_eq!(why.status.code(), Some(0), "{}", stderr(&why));
    let said = json_of(&stdout(&why));
    assert_eq!(said["state"], "redacted");
    let from = said["from"].as_str().unwrap();
    assert!(
        from.contains("acme/api-gateway/production") && from.contains("withheld"),
        "{from}"
    );
    assert!(!from.contains("nowhere"), "{from}");
}

#[test]
fn with_the_header_removed_a_pulled_marker_still_refuses_rather_than_passing_nothing() {
    let mock = Mock::new();
    // The file pull writes for a write-only environment, as the pull test checks.
    let workspace = Workspace::new(&[
        (".env.schema", &write_only_schema()),
        (
            ".env.production",
            &format!("PORT=3000\nSTRIPE_SECRET_KEY={SECRET}\n# penv:redacted DB_PASSWORD\n"),
        ),
    ]);

    // Local mode: no header, and a lower layer's value does not stand in for
    // the production one the marker names.
    std::fs::write(
        workspace.path().join(".env.schema"),
        format!("# @schema=1\n\n{}", write_only_keys()),
    )
    .unwrap();
    std::fs::write(workspace.path().join(".env"), "DB_PASSWORD=dev_FAKE_0000\n").unwrap();
    let requests = mock.requests().len();

    let refused = run_in(&workspace, &mock, "production");
    assert_eq!(refused.status.code(), Some(6), "{}", stderr(&refused));
    let error = json_of(&stderr(&refused));
    assert_eq!(error["error"], "redacted");
    assert_eq!(
        error["message"],
        "DB_PASSWORD in production is write-only in penv-cloud."
    );
    assert_eq!(
        error["fix"],
        "Set DB_PASSWORD in .env.production.local. penv-cloud keeps the production value write-only."
    );
    assert_eq!(
        mock.requests().len(),
        requests,
        "local mode asked the cloud"
    );

    let ls = workspace.run(&mock, &["--json", "ls", "--env", "production"]);
    assert!(stdout(&ls).contains("\"redacted\""), "{}", stdout(&ls));

    std::fs::write(
        workspace.path().join(".env.production.local"),
        format!("DB_PASSWORD={DB_PASSWORD}\n"),
    )
    .unwrap();
    let allowed = run_in(&workspace, &mock, "production");
    assert_eq!(allowed.status.code(), Some(0), "{}", stderr(&allowed));
    assert!(stdout(&allowed).contains("3000"), "{}", stdout(&allowed));
}

#[test]
fn an_approval_for_a_write_only_value_is_refused_as_redacted_with_exit_six() {
    let mock = Mock::new();
    mock.on(
        "POST",
        APPROVALS,
        409,
        &json!({ "error": "redacted" }).to_string(),
    );
    let workspace = Workspace::new(&[(".env.schema", &write_only_schema())]);

    let output = workspace
        .command(&mock)
        .env("CLAUDECODE", "1")
        .args(["reveal", "DB_PASSWORD", "--env", "production"])
        .output()
        .expect("penv runs");
    assert_eq!(output.status.code(), Some(6), "{}", stderr(&output));
    let error = json_of(&stderr(&output));
    assert_eq!(error["error"], "redacted");
    assert!(
        error["fix"]
            .as_str()
            .unwrap()
            .contains("Reveal is not available"),
        "{error}"
    );
}

#[test]
fn a_person_revealing_a_write_only_value_is_told_so_and_never_told_to_set_it() {
    let mock = Mock::new();
    mock.on("GET", PRODUCTION, 200, &write_only_body(&[]));
    let workspace = Workspace::new(&[(".env.schema", &write_only_schema())]);

    let output = workspace.run(
        &mock,
        &["--json", "reveal", "DB_PASSWORD", "--env", "production"],
    );
    assert_eq!(output.status.code(), Some(6), "{}", stderr(&output));
    let error = json_of(&stderr(&output));
    assert_eq!(error["error"], "redacted");
    assert!(
        !error["fix"].as_str().unwrap().contains("penv set"),
        "{error}"
    );
}

#[test]
fn bundle_refuses_a_write_only_key_rather_than_leaving_it_out() {
    let mock = Mock::new();
    mock.on("GET", PRODUCTION, 200, &write_only_body(&[]));
    let workspace = Workspace::new(&[(".env.schema", &write_only_schema())]);

    let output = workspace.run(&mock, &["--json", "bundle", "--env", "production"]);
    assert_eq!(output.status.code(), Some(6), "{}", stderr(&output));
    assert_eq!(json_of(&stderr(&output))["error"], "redacted");
    assert!(!workspace.path().join(".penv/production.bundle").exists());
}
