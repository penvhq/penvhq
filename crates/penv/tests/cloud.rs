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
        command
            .current_dir(&self.dir)
            .env("PENV_URL", mock.url())
            .env("PENV_TOKEN", TOKEN)
            .env_remove("PENV_ENV")
            .env_remove("LOCALAPPDATA")
            .env_remove("XDG_CACHE_HOME")
            .env_remove("HOME");
        // The suite itself may be running under an agent, and these tests decide
        // for themselves which sessions are one.
        for marker in AGENT_MARKERS {
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
        .arg("echo $PORT; echo $PORT")
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
        .arg("echo $PORT $PORT_TOO")
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
