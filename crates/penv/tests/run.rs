//! `run` end to end: the binary spawns a real child, injects the values into it
//! and scrubs what comes back. Everything else about `run` is a unit test.

use std::path::{Path, PathBuf};
use std::process::{Command, Output};
use std::sync::atomic::{AtomicU32, Ordering};

const SECRET: &str = "sk_test_FAKE0000";

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

trait WithoutCredentials {
    fn without_credentials(&mut self) -> &mut Self;
}

impl WithoutCredentials for Command {
    /// No ambient identity, and penv's own roots rather than the machine's bundle.
    fn without_credentials(&mut self) -> &mut Self {
        for name in AMBIENT_CREDENTIALS {
            self.env_remove(name);
        }
        self.env_remove("SSL_CERT_FILE")
    }
}

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
    format!("# @penv=acme/api-gateway @schema=1\n\n{KEYS}")
}

#[cfg(windows)]
const SHELL: &str = "cmd";
#[cfg(windows)]
const SHELL_FLAG: &str = "/C";
#[cfg(windows)]
const ECHO_VALUES: &str = "echo %STRIPE_SECRET_KEY% %PORT%";

#[cfg(not(windows))]
const SHELL: &str = "sh";
#[cfg(not(windows))]
const SHELL_FLAG: &str = "-c";
#[cfg(not(windows))]
const ECHO_VALUES: &str = "echo $STRIPE_SECRET_KEY $PORT";

#[cfg(windows)]
const ECHO_EXTRA: &str = "echo %LEGACY_API_KEY%";
#[cfg(not(windows))]
const ECHO_EXTRA: &str = "echo $LEGACY_API_KEY";

const EXIT_SEVEN: &str = "exit 7";

static COUNTER: AtomicU32 = AtomicU32::new(0);

struct Workspace(PathBuf);

impl Workspace {
    fn new(files: &[(&str, &str)]) -> Workspace {
        let name = format!(
            "penv-run-{}-{}",
            std::process::id(),
            COUNTER.fetch_add(1, Ordering::Relaxed)
        );
        let dir = std::env::temp_dir().join(name);
        std::fs::create_dir_all(&dir).expect("a scratch directory");
        for (file, contents) in files {
            let path = dir.join(file);
            if let Some(parent) = path.parent() {
                std::fs::create_dir_all(parent).expect("a scratch folder");
            }
            std::fs::write(path, contents).expect("a scratch file");
        }
        Workspace(dir)
    }

    fn path(&self) -> &Path {
        &self.0
    }

    /// `penv <args>`, with no agent flag.
    fn penv(&self, args: &[&str]) -> Output {
        Command::new(env!("CARGO_BIN_EXE_penv"))
            .env(
                "PENV_LOCAL_KEY",
                "0000000000000000000000000000000000000000000000000000000000000001",
            )
            .current_dir(&self.0)
            .env_remove("SSL_CERT_FILE")
            .args(args)
            .output()
            .expect("penv runs")
    }

    /// `penv --agent run [args] -- <shell> <flag> <script>`.
    fn run(&self, args: &[&str], script: &str) -> Output {
        let mut command = Command::new(env!("CARGO_BIN_EXE_penv"));
        command.env(
            "PENV_LOCAL_KEY",
            "0000000000000000000000000000000000000000000000000000000000000001",
        );
        command
            .current_dir(&self.0)
            .env_remove("SSL_CERT_FILE")
            .arg("--agent")
            .arg("run");
        command.args(args);
        command.arg("--").arg(SHELL).arg(SHELL_FLAG).arg(script);
        command.output().expect("penv runs")
    }
}

impl Drop for Workspace {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

fn stdout(output: &Output) -> String {
    String::from_utf8_lossy(&output.stdout).into_owned()
}

fn stderr(output: &Output) -> String {
    String::from_utf8_lossy(&output.stderr).into_owned()
}

#[test]
fn the_values_reach_the_child_and_the_secret_does_not_come_back() {
    let workspace = Workspace::new(&[
        (".env.schema", &local_schema()),
        (".env", &format!("STRIPE_SECRET_KEY={SECRET}\n")),
    ]);
    let output = workspace.run(&[], ECHO_VALUES);

    let text = stdout(&output);
    assert!(!text.contains(SECRET), "the value came back: {text}");
    assert!(text.contains("sk\u{2592}\u{2592}"), "not masked: {text}");
    assert!(text.contains("3000"), "the default never arrived: {text}");
    assert_eq!(output.status.code(), Some(0), "{}", stderr(&output));
}

#[test]
fn the_childs_exit_code_is_this_commands_exit_code() {
    let workspace = Workspace::new(&[
        (".env.schema", &local_schema()),
        (".env", &format!("STRIPE_SECRET_KEY={SECRET}\n")),
    ]);
    assert_eq!(workspace.run(&[], EXIT_SEVEN).status.code(), Some(7));
}

#[test]
fn no_mask_is_ignored_when_an_agent_is_driving() {
    let workspace = Workspace::new(&[
        (".env.schema", &local_schema()),
        (".env", &format!("STRIPE_SECRET_KEY={SECRET}\n")),
    ]);
    let output = workspace.run(&["--no-mask"], ECHO_VALUES);

    assert!(!stdout(&output).contains(SECRET));
    assert!(
        stderr(&output).contains("--no-mask"),
        "no note was printed: {}",
        stderr(&output)
    );
}

#[test]
fn a_missing_value_stops_the_child_with_exit_three() {
    let workspace = Workspace::new(&[(".env.schema", &local_schema()), (".env", "PORT=3000\n")]);
    let output = workspace.run(&[], ECHO_VALUES);

    assert_eq!(output.status.code(), Some(3));
    let text = stdout(&output);
    assert!(text.contains("STRIPE_SECRET_KEY"), "{text}");
    assert!(text.contains("\"violations\""), "{text}");
}

#[test]
fn another_environment_layers_its_own_file_over_env() {
    let workspace = Workspace::new(&[
        (".env.schema", &local_schema()),
        (".env", &format!("STRIPE_SECRET_KEY={SECRET}\nPORT=3000\n")),
        (".env.production", "PORT=8080\n"),
    ]);
    let output = workspace.run(&["--env", "production"], ECHO_VALUES);
    assert_eq!(output.status.code(), Some(0), "{}", stderr(&output));
    assert!(stdout(&output).contains("8080"), "{}", stdout(&output));
    assert!(!stdout(&output).contains(SECRET));
}

#[test]
fn an_environment_with_no_file_runs_and_says_which_files_it_used() {
    let workspace = Workspace::new(&[
        (".env.schema", &local_schema()),
        (".env", &format!("STRIPE_SECRET_KEY={SECRET}\n")),
    ]);
    let output = workspace.run(&["--env", "staging"], ECHO_VALUES);
    assert_eq!(output.status.code(), Some(0), "{}", stderr(&output));
    assert!(
        stderr(&output).contains("no .env.staging"),
        "{}",
        stderr(&output)
    );
}

#[test]
fn current_env_names_the_environment_from_a_key() {
    let schema = format!(
        "# @schema=1 @currentEnv=$APP_ENV\n\n# @type=string @sensitive=false\nAPP_ENV=development\n\n{KEYS}"
    );
    let workspace = Workspace::new(&[
        (".env.schema", &schema),
        (
            ".env",
            &format!("STRIPE_SECRET_KEY={SECRET}\nAPP_ENV=production\n"),
        ),
        (".env.production", "PORT=9090\n"),
    ]);
    let output = workspace.run(&[], ECHO_VALUES);
    assert_eq!(output.status.code(), Some(0), "{}", stderr(&output));
    assert!(stdout(&output).contains("9090"), "{}", stdout(&output));
}

#[test]
fn expansion_and_penv_references_resolve_from_the_local_files() {
    let workspace = Workspace::new(&[
        (".env.schema", &local_schema()),
        (
            ".env",
            &format!("STRIPE_SECRET_KEY={SECRET}\nPORT=penv(staging/PORT)\n"),
        ),
        (".env.staging", "PORT=${STAGING_PORT:-7070}\n"),
    ]);
    let output = workspace.run(&[], ECHO_VALUES);
    assert_eq!(output.status.code(), Some(0), "{}", stderr(&output));
    assert!(stdout(&output).contains("7070"), "{}", stdout(&output));
}

#[test]
fn a_penv_reference_to_an_environment_with_no_file_creates_an_empty_one() {
    let workspace = Workspace::new(&[
        (".env.schema", &local_schema()),
        (
            ".env",
            "STRIPE_SECRET_KEY=penv(test/STRIPE_SECRET_KEY)
",
        ),
    ]);
    let output = workspace.run(&[], ECHO_VALUES);
    assert!(
        workspace.path().join(".env.test").is_file(),
        "{}",
        stderr(&output)
    );
    assert_eq!(
        output.status.code(),
        Some(3),
        "the empty value is still required"
    );
}

#[test]
fn check_reminds_about_an_overdue_rotation_and_still_exits_zero() {
    let schema = "# @schema=1\n\n# @type=string(startsWith=sk_) @rotate=1h\nSTRIPE_SECRET_KEY=\n";
    let workspace = Workspace::new(&[
        (".env.schema", schema),
        (".env", &format!("STRIPE_SECRET_KEY={SECRET}\n")),
    ]);
    std::fs::create_dir_all(workspace.path().join(".penv")).unwrap();
    std::fs::write(
        workspace.path().join(".penv/config.toml"),
        "[rotation]\nSTRIPE_SECRET_KEY = \"2020-01-01T00:00:00Z\"\n",
    )
    .unwrap();
    let output = workspace.penv(&["check"]);
    assert_eq!(output.status.code(), Some(0), "{}", stdout(&output));
    let text = stdout(&output);
    assert!(text.contains("\"due\": \"2020-01-01T01:00:00Z\""), "{text}");
    assert!(text.contains("\"overdue\": true"), "{text}");
}

#[test]
fn scan_names_the_file_line_and_key_and_never_the_value() {
    let workspace = Workspace::new(&[
        (".env.schema", &local_schema()),
        (".env", &format!("STRIPE_SECRET_KEY={SECRET}\n")),
        ("app.js", &format!("// setup\nconst key = \"{SECRET}\";\n")),
    ]);
    let output = workspace.penv(&["scan", "app.js"]);
    assert_eq!(output.status.code(), Some(3), "{}", stderr(&output));
    let text = stdout(&output);
    assert!(
        text.contains("\"line\": 2") && text.contains("STRIPE_SECRET_KEY"),
        "{text}"
    );
    assert!(!text.contains(SECRET));
}

#[test]
fn a_dotenv_with_no_schema_gets_one_written_and_then_runs() {
    let workspace =
        Workspace::new(&[(".env", &format!("STRIPE_SECRET_KEY={SECRET}\nPORT=3000\n"))]);
    let output = workspace.run(&[], ECHO_VALUES);

    assert_eq!(output.status.code(), Some(0), "{}", stderr(&output));
    assert!(workspace.path().join(".env.schema").is_file());
    assert!(
        stderr(&output).contains("penv init"),
        "init said nothing: {}",
        stderr(&output)
    );
    assert!(!stdout(&output).contains(SECRET));
}

#[test]
fn a_cloud_schema_with_no_local_values_needs_a_credential() {
    let cloud = cloud_schema();
    let workspace = Workspace::new(&[(".env.schema", &cloud)]);
    let output = Command::new(env!("CARGO_BIN_EXE_penv"))
        .env(
            "PENV_LOCAL_KEY",
            "0000000000000000000000000000000000000000000000000000000000000001",
        )
        .current_dir(workspace.path())
        .env("PENV_URL", "http://127.0.0.1:1")
        .env_remove("PENV_TOKEN")
        .without_credentials()
        .args(["--agent", "run", "--"])
        .args([SHELL, SHELL_FLAG, ECHO_VALUES])
        .output()
        .expect("penv runs");

    assert_eq!(output.status.code(), Some(5), "{}", stderr(&output));
    let text = stderr(&output);
    assert!(text.contains("no_credential"), "{text}");
    assert!(text.contains("penv login"), "{text}");
}

#[test]
fn a_cloud_schema_falls_back_to_the_local_dotenv_only_when_offline() {
    let cloud = cloud_schema();
    let workspace = Workspace::new(&[
        (".env.schema", &cloud),
        (".env", &format!("STRIPE_SECRET_KEY={SECRET}\n")),
    ]);
    let run = |token: Option<&str>| {
        let mut command = Command::new(env!("CARGO_BIN_EXE_penv"));
        command.env(
            "PENV_LOCAL_KEY",
            "0000000000000000000000000000000000000000000000000000000000000001",
        );
        command
            .current_dir(workspace.path())
            .env("PENV_URL", "http://127.0.0.1:1")
            .env_remove("PENV_TOKEN")
            .without_credentials()
            .args(["--agent", "run", "--"])
            .args([SHELL, SHELL_FLAG, ECHO_VALUES]);
        if let Some(token) = token {
            command.env("PENV_TOKEN", token);
        }
        command.output().expect("penv runs")
    };

    let offline = run(Some("pck_FAKE"));
    assert_eq!(offline.status.code(), Some(0), "{}", stderr(&offline));
    assert!(
        stderr(&offline).contains("could not be reached"),
        "no warning: {}",
        stderr(&offline)
    );
    assert!(!stdout(&offline).contains(SECRET));

    let signed_out = run(None);
    assert_eq!(signed_out.status.code(), Some(5), "{}", stderr(&signed_out));
    assert!(stderr(&signed_out).contains("no_credential"));
}

#[test]
fn a_key_the_schema_does_not_declare_is_masked_and_reported_as_drift() {
    let workspace = Workspace::new(&[
        (".env.schema", &local_schema()),
        (
            ".env",
            &format!("STRIPE_SECRET_KEY={SECRET}\nLEGACY_API_KEY=left_over_FAKE_0000\n"),
        ),
    ]);
    let output = workspace.run(&[], ECHO_EXTRA);

    assert_eq!(output.status.code(), Some(0), "{}", stderr(&output));
    let text = stdout(&output);
    assert!(
        !text.contains("left_over_FAKE_0000"),
        "an undeclared key came back in the clear: {text}"
    );
    assert!(
        stderr(&output).contains("LEGACY_API_KEY"),
        "the drift was not reported: {}",
        stderr(&output)
    );
}

#[test]
fn check_reports_drift_without_failing() {
    let workspace = Workspace::new(&[
        (".env.schema", &local_schema()),
        (
            ".env",
            &format!("STRIPE_SECRET_KEY={SECRET}\nLEGACY_API_KEY=left_over_FAKE_0000\n"),
        ),
    ]);
    let output = Command::new(env!("CARGO_BIN_EXE_penv"))
        .env(
            "PENV_LOCAL_KEY",
            "0000000000000000000000000000000000000000000000000000000000000001",
        )
        .current_dir(workspace.path())
        .args(["--agent", "check"])
        .output()
        .expect("penv runs");

    assert_eq!(
        output.status.code(),
        Some(0),
        "drift alone is not a failure"
    );
    let text = stdout(&output);
    assert!(text.contains("\"drift\""), "{text}");
    assert!(text.contains("LEGACY_API_KEY"), "{text}");
    assert!(
        !text.contains("left_over_FAKE_0000"),
        "a value was printed: {text}"
    );
}

#[test]
fn no_mask_is_ignored_when_the_pipes_are_not_a_terminal() {
    let workspace = Workspace::new(&[
        (".env.schema", &local_schema()),
        (".env", &format!("STRIPE_SECRET_KEY={SECRET}\n")),
    ]);
    let output = Command::new(env!("CARGO_BIN_EXE_penv"))
        .env(
            "PENV_LOCAL_KEY",
            "0000000000000000000000000000000000000000000000000000000000000001",
        )
        .current_dir(workspace.path())
        .args(["run", "--no-mask", "--", SHELL, SHELL_FLAG, ECHO_VALUES])
        .output()
        .expect("penv runs");

    assert!(
        stderr(&output).contains("--no-mask"),
        "the flag was honoured off a terminal: {}",
        stderr(&output)
    );
}

#[test]
fn agent_and_format_text_is_a_parse_error() {
    let workspace = Workspace::new(&[(".env.schema", &local_schema())]);
    let output = Command::new(env!("CARGO_BIN_EXE_penv"))
        .env(
            "PENV_LOCAL_KEY",
            "0000000000000000000000000000000000000000000000000000000000000001",
        )
        .current_dir(workspace.path())
        .args(["--agent", "--format", "text", "ls"])
        .output()
        .expect("penv runs");

    assert_ne!(output.status.code(), Some(0));
    assert!(
        stderr(&output).contains("--format text"),
        "{}",
        stderr(&output)
    );
}

/// `npm`, `pnpm` and `yarn` are `.cmd` shims on Windows; a bare name has to
/// find them the way a shell would.
#[cfg(windows)]
#[test]
fn a_bare_name_finds_its_cmd_shim_on_windows() {
    let workspace = Workspace::new(&[
        (".env.schema", &local_schema()),
        (".env", &format!("STRIPE_SECRET_KEY={SECRET}\n")),
        ("shim.cmd", "@echo shim ran %PORT%\r\n"),
    ]);
    let path = format!(
        "{};{}",
        workspace.path().display(),
        std::env::var("PATH").unwrap_or_default()
    );
    let output = Command::new(env!("CARGO_BIN_EXE_penv"))
        .env(
            "PENV_LOCAL_KEY",
            "0000000000000000000000000000000000000000000000000000000000000001",
        )
        .current_dir(workspace.path())
        .env("PATH", path)
        .args(["--agent", "run", "--", "shim"])
        .output()
        .expect("penv runs");

    assert_eq!(output.status.code(), Some(0), "{}", stderr(&output));
    assert!(
        stdout(&output).contains("shim ran 3000"),
        "{}",
        stdout(&output)
    );
}

#[test]
fn a_folder_with_only_an_environment_file_still_gets_a_schema_and_runs() {
    let workspace = Workspace::new(&[(
        ".env.local",
        &format!("STRIPE_SECRET_KEY={SECRET}\nPORT=3000\n"),
    )]);
    let output = workspace.run(&[], ECHO_VALUES);

    assert_eq!(output.status.code(), Some(0), "{}", stderr(&output));
    let schema = std::fs::read_to_string(workspace.path().join(".env.schema")).unwrap();
    assert!(schema.contains("STRIPE_SECRET_KEY="), "{schema}");
    assert!(
        !workspace.path().join(".env").exists(),
        "no empty .env when another value file holds the values"
    );
    assert!(
        stderr(&output).contains(".env.local"),
        "{}",
        stderr(&output)
    );
    assert!(stdout(&output).contains("3000"));
}

#[test]
fn bare_penv_names_every_value_file_and_the_environment() {
    let schema = format!(
        "# @schema=1 @currentEnv=$APP_ENV\n\n# @type=string @sensitive=false\nAPP_ENV=staging\n\n{KEYS}"
    );
    let workspace = Workspace::new(&[
        (".env.schema", &schema),
        (".env", "PORT=3000\n"),
        (".env.staging", "PORT=4000\n"),
        (".env.example", "PORT=\n"),
    ]);
    let text = stdout(&workspace.penv(&["--json"]));
    assert!(text.contains("\"environment\": \"staging\""), "{text}");
    assert!(
        text.contains(&format!("\"version\": \"{}\"", env!("CARGO_PKG_VERSION"))),
        "{text}"
    );
    assert!(
        !workspace.path().join(".env.local").exists(),
        "bare penv writes nothing"
    );
    assert!(
        text.contains(".env.staging") && !text.contains(".env.example"),
        "{text}"
    );
}

#[test]
fn the_current_env_key_follows_the_environment_however_it_was_chosen() {
    let schema = format!(
        "# @schema=1 @currentEnv=$APP_ENV\n\n# @type=string @sensitive=false\nAPP_ENV=development\n\n# @type=string @sensitive=false\nAPI=if(eq($APP_ENV, production), prod.test, dev.test)\n\n{KEYS}"
    );
    let workspace = Workspace::new(&[
        (".env.schema", &schema),
        (".env", &format!("STRIPE_SECRET_KEY={SECRET}\n")),
    ]);
    let script = if cfg!(windows) {
        "echo %APP_ENV% %API%"
    } else {
        "echo $APP_ENV $API"
    };
    let output = workspace.run(&["--env", "production"], script);
    assert!(
        stdout(&output).contains("production prod.test"),
        "{}",
        stdout(&output)
    );
    let mut command = Command::new(env!("CARGO_BIN_EXE_penv"));
    command.env(
        "PENV_LOCAL_KEY",
        "0000000000000000000000000000000000000000000000000000000000000001",
    );
    let output = command
        .current_dir(workspace.path())
        .env("APP_ENV", "production")
        .args(["--agent", "run", "--", SHELL, SHELL_FLAG, script])
        .output()
        .unwrap();
    assert!(
        stdout(&output).contains("production prod.test"),
        "the process environment wins: {}",
        stdout(&output)
    );
}

// --- programmable values -------------------------------------------------------

#[test]
fn a_public_key_built_from_a_secret_is_masked() {
    let schema = "# @schema=1\n\n# @type=string\nDB_PASS=\n\n# @type=string @sensitive=false\nDATABASE_URL=postgres://app:${DB_PASS | urlencode}@db/app\n\n# @type=string @sensitive=false\nHOST=db\n";
    let workspace = Workspace::new(&[
        (".env.schema", schema),
        (".env", "DB_PASS=hunter2hunter2\n"),
    ]);
    let script = if cfg!(windows) {
        "echo %DATABASE_URL% %HOST%"
    } else {
        "echo $DATABASE_URL $HOST"
    };
    let output = workspace.run(&[], script);
    assert_eq!(output.status.code(), Some(0), "{}", stderr(&output));
    let text = stdout(&output);
    assert!(
        !text.contains("postgres://app:hunter2"),
        "the built URL leaked: {text}"
    );
    assert!(text.contains(" db"), "a public value stays visible: {text}");
    let check = stdout(&workspace.penv(&["check"]));
    assert!(check.contains("built from a sensitive value"), "{check}");
}

#[test]
fn match_and_filters_compute_before_the_child_starts() {
    let schema = "# @schema=1 @currentEnv=$APP_ENV\n\n# @type=string @sensitive=false\nAPP_ENV=development\n\n# @type=string @sensitive=false\nAPI=match($APP_ENV, production: api.acme.test, _: localhost:3000)\n\n# @type=string @sensitive=false\nSHOUT=${API | upper}\n";
    let workspace = Workspace::new(&[(".env.schema", schema)]);
    let script = if cfg!(windows) {
        "echo %API% %SHOUT%"
    } else {
        "echo $API $SHOUT"
    };
    let output = workspace.run(&[], script);
    assert!(
        stdout(&output).contains("localhost:3000 LOCALHOST:3000"),
        "{} {}",
        stdout(&output),
        stderr(&output)
    );
    let output = workspace.run(&["--env", "production"], script);
    assert!(
        stdout(&output).contains("api.acme.test API.ACME.TEST"),
        "{}",
        stdout(&output)
    );
}

#[test]
fn random_is_generated_once_kept_locally_and_never_pushed() {
    let schema =
        format!("# @schema=1\n\n# @type=string(minLength=32)\nSESSION_SECRET=random(32)\n\n{KEYS}");
    let workspace = Workspace::new(&[
        (".env.schema", &schema),
        (".env", &format!("STRIPE_SECRET_KEY={SECRET}\n")),
    ]);
    let check = stdout(&workspace.penv(&["check"]));
    assert!(check.contains("generated by the first penv run"), "{check}");
    assert!(
        !workspace.path().join(".env.local").exists(),
        "check never writes"
    );

    let first = workspace.run(&[], "exit 0");
    assert_eq!(first.status.code(), Some(0), "{}", stderr(&first));
    let kept = std::fs::read_to_string(workspace.path().join(".env.local")).unwrap();
    let value = kept
        .trim()
        .strip_prefix("SESSION_SECRET=")
        .unwrap()
        .to_string();
    assert_eq!(value.len(), 32);
    assert!(value.chars().all(|c| c.is_ascii_alphanumeric()));
    assert!(
        !stderr(&first).contains(&value),
        "the generated value is never printed"
    );

    workspace.run(&[], "exit 0");
    let again = std::fs::read_to_string(workspace.path().join(".env.local")).unwrap();
    assert_eq!(again, kept, "a second run reuses the value");
}

#[test]
fn a_false_assert_stops_run_and_fails_check_naming_only_the_message() {
    let schema = format!(
        "# @schema=1\n# @assert(not(eq($PORT, $ADMIN_PORT)), \"PORT and ADMIN_PORT collide\")\n\n# @type=port @sensitive=false\nADMIN_PORT=3000\n\n{KEYS}"
    );
    let workspace = Workspace::new(&[
        (".env.schema", &schema),
        (".env", &format!("STRIPE_SECRET_KEY={SECRET}\n")),
    ]);
    let output = workspace.run(&[], "echo ran");
    assert_eq!(output.status.code(), Some(3));
    assert!(!stdout(&output).contains("ran"));
    let check = stdout(&workspace.penv(&["check"]));
    assert!(check.contains("PORT and ADMIN_PORT collide"), "{check}");
    assert!(check.contains("penv-only features (@assert)"), "{check}");
    assert!(!check.contains(SECRET));
}

#[test]
fn a_sensitive_value_too_short_to_mask_is_named_not_silently_shown() {
    let workspace = Workspace::new(&[
        (".env.schema", "# @schema=1\n\n# @type=string\nPIN=\n"),
        (".env", "PIN=123\n"),
    ]);
    let output = workspace.run(&[], "exit 0");
    assert!(
        stderr(&output).contains("PIN is sensitive and shorter than"),
        "{}",
        stderr(&output)
    );
    assert!(!stderr(&output).contains("123"));
}

// --- masking by default, browser safety, gen ---------------------------------

#[test]
fn masking_is_on_without_an_agent_and_no_mask_is_ignored_through_a_pipe() {
    let workspace = Workspace::new(&[
        (".env.schema", &local_schema()),
        (".env", &format!("STRIPE_SECRET_KEY={SECRET}\n")),
    ]);
    let script = if cfg!(windows) {
        "echo %STRIPE_SECRET_KEY%"
    } else {
        "echo $STRIPE_SECRET_KEY"
    };
    let plain = workspace.penv(&["run", "--", SHELL, SHELL_FLAG, script]);
    assert!(
        !stdout(&plain).contains(SECRET),
        "a person's run masks too: {}",
        stdout(&plain)
    );
    let asked = workspace.penv(&["run", "--no-mask", "--", SHELL, SHELL_FLAG, script]);
    assert!(!stdout(&asked).contains(SECRET), "a pipe is not a person");
    assert!(
        stderr(&asked).contains("--no-mask was ignored"),
        "{}",
        stderr(&asked)
    );
}

#[test]
fn a_public_key_built_from_a_secret_stops_run_and_fails_check() {
    let schema = format!(
        "# @schema=1\n\n{KEYS}\n# @type=url\nNEXT_PUBLIC_CHECKOUT=https://pay.test/?k=${{STRIPE_SECRET_KEY}}\n"
    );
    let workspace = Workspace::new(&[
        (".env.schema", &schema),
        (".env", &format!("STRIPE_SECRET_KEY={SECRET}\n")),
    ]);
    let output = workspace.run(&[], "echo ran");
    assert_eq!(output.status.code(), Some(3));
    assert!(!stdout(&output).contains("ran"));
    let check = stdout(&workspace.penv(&["check"]));
    assert!(
        check.contains("NEXT_PUBLIC_ prefix sends it to the browser"),
        "{check}"
    );
    assert!(!check.contains(SECRET));
}

#[test]
fn a_custom_public_prefix_from_config_is_public_and_refuses_sensitive() {
    let workspace = Workspace::new(&[(".env.schema", "# @type=string\nAPP_PUBLIC_NAME=acme\n")]);
    std::fs::create_dir_all(workspace.path().join(".penv")).unwrap();
    std::fs::write(
        workspace.path().join(".penv/config.toml"),
        "[public]\nprefixes = [\"APP_PUBLIC_\"]\n",
    )
    .unwrap();
    let listed = stdout(&workspace.penv(&["ls"]));
    assert!(listed.contains("\"sensitive\": false"), "{listed}");
    std::fs::write(
        workspace.path().join(".env.schema"),
        "# @type=string @sensitive\nAPP_PUBLIC_NAME=acme\n",
    )
    .unwrap();
    let check = stdout(&workspace.penv(&["check"]));
    assert!(check.contains("ships to the browser"), "{check}");
}

#[test]
fn a_secret_written_into_browser_output_fails_the_run_that_built_it() {
    let workspace = Workspace::new(&[
        (".env.schema", &local_schema()),
        (".env", &format!("STRIPE_SECRET_KEY={SECRET}\n")),
    ]);
    std::fs::create_dir_all(workspace.path().join("dist")).unwrap();
    std::fs::write(
        workspace.path().join("dist/old.js"),
        format!("var k='{SECRET}';"),
    )
    .unwrap();
    let old = std::time::SystemTime::now() - std::time::Duration::from_secs(3600);
    std::fs::File::options()
        .write(true)
        .open(workspace.path().join("dist/old.js"))
        .unwrap()
        .set_modified(old)
        .unwrap();
    let clean = workspace.run(&[], "exit 0");
    assert_eq!(
        clean.status.code(),
        Some(0),
        "old output is not this build's: {}",
        stderr(&clean)
    );

    #[cfg(not(windows))]
    {
        let built = workspace.run(&[], "echo \"var k='$STRIPE_SECRET_KEY';\" > dist/app.js");
        assert_eq!(built.status.code(), Some(3), "{}", stderr(&built));
        assert!(
            stderr(&built).contains("dist/app.js:1 holds the value of STRIPE_SECRET_KEY"),
            "{}",
            stderr(&built)
        );
        assert!(!stderr(&built).contains(SECRET));
        let failed = workspace.run(&[], "exit 7");
        assert_eq!(
            failed.status.code(),
            Some(7),
            "a failed build keeps its own code"
        );
    }
}

#[test]
fn gen_never_writes_a_computed_default_as_a_literal_and_never_writes_a_secret_as_a_literal() {
    let schema = "# @type=string\nDB_PASS=\n\n# @type=url @sensitive=false\nDATABASE_URL=postgres://app:${DB_PASS}@db/app\n\n# @type=number(isInt=true) @sensitive=false\nWORKERS=if(forEnv(production), 16, 2)\n\n# @type=string\nSESSION=random(32)\n\n# @type=url\nNEXT_PUBLIC_API=https://api.test\n";
    let workspace = Workspace::new(&[
        (".env.schema", schema),
        ("package.json", "{}"),
        ("tsconfig.json", "{}"),
    ]);
    let output = workspace.penv(&["gen", "ts", "--out", "env.ts"]);
    assert_eq!(output.status.code(), Some(0), "{}", stderr(&output));
    let ts = std::fs::read_to_string(workspace.path().join("env.ts")).unwrap();
    for literal in ["if(forEnv", "random(32)", "${DB_PASS}"] {
        assert!(
            !ts.contains(literal),
            "{literal} leaked into generated code:\n{ts}"
        );
    }
    // One env: a public key read by its literal name, so a bundler inlines it;
    // a secret read by computed name, so no bundler ever can.
    assert!(!ts.contains("publicEnv"), "{ts}");
    assert!(ts.contains("process.env.NEXT_PUBLIC_API"), "{ts}");
    assert!(
        ts.contains("read(\"DB_PASS\")") && !ts.contains("process.env.DB_PASS"),
        "{ts}"
    );
}

#[test]
fn init_keeps_the_schema_version_in_config_and_a_newer_one_is_refused() {
    let workspace = Workspace::new(&[(".env", "API_KEY=abc12345\n")]);
    workspace.penv(&["init"]);
    let schema = std::fs::read_to_string(workspace.path().join(".env.schema")).unwrap();
    assert!(!schema.contains("@schema"), "{schema}");
    let config = std::fs::read_to_string(workspace.path().join(".penv/config.toml")).unwrap();
    assert!(
        config.contains("[schema]") && config.contains("version = 1"),
        "{config}"
    );
    std::fs::write(
        workspace.path().join(".penv/config.toml"),
        "[schema]\nversion = 9\n",
    )
    .unwrap();
    let check = stdout(&workspace.penv(&["check"]));
    assert!(check.contains("reads up to version 1"), "{check}");
}

// --- in-process masking, native bundles, bundler configs -----------------------

fn on_path(program: &str) -> bool {
    Command::new(program)
        .arg("--version")
        .output()
        .is_ok_and(|o| o.status.success())
}

const SERVE_NODE: &str = r#"
const http = require("http");
const s = http.createServer((q, r) => r.end("env=" + process.env.STRIPE_SECRET_KEY));
s.listen(0, async () => {
  const body = await (await fetch(`http://127.0.0.1:${s.address().port}/`)).text();
  const logged = [];
  const log = console.log; console.log = (...a) => logged.push(a.join(" "));
  // An in-process shipper reads what the app hands console, before the pipe.
  const shipper = console.error; console.error = (...a) => logged.push(a.join(" ")); 
  console.info("k=" + process.env.STRIPE_SECRET_KEY);
  console.log = log; console.error = shipper;
  process.stdout.write(JSON.stringify({ served: !body.includes(process.env.STRIPE_SECRET_KEY), length: body.length }) + "\n");
  s.close();
});
"#;

#[test]
fn node_serves_a_masked_body_of_the_same_length() {
    if !on_path("node") {
        return;
    }
    let workspace = Workspace::new(&[
        (".env.schema", &local_schema()),
        (".env", &format!("STRIPE_SECRET_KEY={SECRET}\n")),
        ("serve.js", SERVE_NODE),
    ]);
    let output = workspace.penv(&["run", "--", "node", "serve.js"]);
    let text = stdout(&output);
    assert!(
        text.contains("\"served\":true"),
        "{text} {}",
        stderr(&output)
    );
    assert!(
        text.contains(&format!("\"length\":{}", 4 + SECRET.len())),
        "a Content-Length stays right: {text}"
    );
}

#[test]
fn python_masks_served_bytes_and_log_records_and_keeps_the_projects_sitecustomize() {
    if !on_path("python3") {
        return;
    }
    let app = r#"
import builtins, logging, os, threading, urllib.request
from http.server import BaseHTTPRequestHandler, HTTPServer
K = os.environ["STRIPE_SECRET_KEY"]
seen = []
class Grab(logging.Handler):
    def emit(self, r): seen.append(r.getMessage())
logging.getLogger().addHandler(Grab()); logging.getLogger().setLevel(logging.INFO)
logging.info("k=%s", K)
class H(BaseHTTPRequestHandler):
    def do_GET(self):
        b = ("env=" + K).encode(); self.send_response(200); self.send_header("content-length", str(len(b))); self.end_headers(); self.wfile.write(b)
    def log_message(self, *a): pass
s = HTTPServer(("127.0.0.1", 0), H); threading.Thread(target=s.serve_forever, daemon=True).start()
body = urllib.request.urlopen(f"http://127.0.0.1:{s.server_port}/").read().decode()
print("served", K not in body, "logged", K not in seen[0], "project", getattr(builtins, "PROJECT_HOOK", False))
s.shutdown()
"#;
    let workspace = Workspace::new(&[
        (".env.schema", &local_schema()),
        (".env", &format!("STRIPE_SECRET_KEY={SECRET}\n")),
        ("app.py", app),
        (
            "hooks/sitecustomize.py",
            "import builtins\nbuiltins.PROJECT_HOOK = True\n",
        ),
    ]);
    let output = Command::new(env!("CARGO_BIN_EXE_penv"))
        .env(
            "PENV_LOCAL_KEY",
            "0000000000000000000000000000000000000000000000000000000000000001",
        )
        .current_dir(workspace.path())
        .env("PYTHONPATH", workspace.path().join("hooks"))
        .args(["run", "--", "python3", "app.py"])
        .output()
        .unwrap();
    assert!(
        stdout(&output).contains("served True logged True project True"),
        "{} {}",
        stdout(&output),
        stderr(&output)
    );
}

#[test]
fn config_can_turn_the_preload_off_for_a_person_but_never_for_an_agent() {
    if !on_path("node") {
        return;
    }
    let workspace = Workspace::new(&[
        (".env.schema", &local_schema()),
        (".env", &format!("STRIPE_SECRET_KEY={SECRET}\n")),
        ("serve.js", SERVE_NODE),
    ]);
    std::fs::create_dir_all(workspace.path().join(".penv")).unwrap();
    std::fs::write(
        workspace.path().join(".penv/config.toml"),
        "[run]\npreload = false\n",
    )
    .unwrap();
    let person = stdout(&workspace.penv(&["run", "--", "node", "serve.js"]));
    assert!(person.contains("\"served\":false"), "config off: {person}");
    let agent = stdout(&workspace.penv(&["--agent", "run", "--", "node", "serve.js"]));
    assert!(
        agent.contains("\"served\":true"),
        "an agent cannot turn it off: {agent}"
    );
    let flag = workspace.penv(&["run", "--no-preload", "--", "node", "serve.js"]);
    assert!(
        stderr(&flag).contains("--no-preload was ignored"),
        "{}",
        stderr(&flag)
    );
}

#[cfg(not(windows))]
#[test]
fn a_secret_in_a_react_native_bundle_fails_the_build_run_text_or_bytecode() {
    let workspace = Workspace::new(&[
        (".env.schema", &local_schema()),
        (".env", &format!("STRIPE_SECRET_KEY={SECRET}\n")),
    ]);
    let bundle = "android/app/build/generated/assets/react/release";
    std::fs::create_dir_all(workspace.path().join(bundle)).unwrap();
    let text = workspace.run(
        &[],
        &format!("echo \"var k='$STRIPE_SECRET_KEY'\" > {bundle}/index.android.bundle"),
    );
    assert_eq!(text.status.code(), Some(3), "{}", stderr(&text));
    assert!(stderr(&text).contains("index.android.bundle:1 holds the value of STRIPE_SECRET_KEY"));
    std::fs::create_dir_all(workspace.path().join("ios/build")).unwrap();
    let bytecode = workspace.run(
        &[],
        "printf 'HBC\\000\\001%s\\000' \"$STRIPE_SECRET_KEY\" > ios/build/main.jsbundle",
    );
    assert_eq!(bytecode.status.code(), Some(3), "{}", stderr(&bytecode));
    assert!(
        stderr(&bytecode).contains("main.jsbundle holds the value of STRIPE_SECRET_KEY"),
        "{}",
        stderr(&bytecode)
    );
    assert!(!stderr(&bytecode).contains(SECRET));
}

#[test]
fn bundler_setups_that_ship_every_key_fail_check_and_a_config_naming_one_is_noted() {
    let workspace = Workspace::new(&[
        (".env.schema", &local_schema()),
        (".env", &format!("STRIPE_SECRET_KEY={SECRET}\n")),
        (
            "package.json",
            "{\"dependencies\":{\"react-native-config\":\"1\"}}",
        ),
        (
            "next.config.js",
            "module.exports = { env: { KEY: process.env.STRIPE_SECRET_KEY } };\n",
        ),
    ]);
    let check = stdout(&workspace.penv(&["check"]));
    assert!(
        check.contains("react-native-config puts every key in .env"),
        "{check}"
    );
    assert!(
        check.contains("next.config.js names STRIPE_SECRET_KEY"),
        "{check}"
    );
    std::fs::write(
        workspace.path().join("package.json"),
        "{\"devDependencies\":{\"babel-plugin-transform-inline-environment-variables\":\"1\"}}",
    )
    .unwrap();
    let check = stdout(&workspace.penv(&["check"]));
    assert!(
        check.contains("inlines every environment variable"),
        "{check}"
    );
    assert!(!check.contains(SECRET));
}

// --- value files git would commit ------------------------------------------------

fn git(dir: &std::path::Path, args: &[&str]) -> bool {
    Command::new("git")
        .current_dir(dir)
        .args(args)
        .env("GIT_AUTHOR_NAME", "t")
        .env("GIT_AUTHOR_EMAIL", "t@t")
        .env("GIT_COMMITTER_NAME", "t")
        .env("GIT_COMMITTER_EMAIL", "t@t")
        .output()
        .is_ok_and(|o| o.status.success())
}

#[test]
fn a_secret_in_a_file_git_would_commit_fails_check_until_init_ignores_it() {
    let workspace = Workspace::new(&[
        (".env.schema", &local_schema()),
        (".env", &format!("STRIPE_SECRET_KEY={SECRET}\n")),
        (".env.production", "PORT=4000\n"),
    ]);
    if !git(workspace.path(), &["init", "-q"]) {
        return;
    }
    let check = workspace.penv(&["check"]);
    assert_eq!(check.status.code(), Some(3));
    let text = stdout(&check);
    assert!(
        text.contains("holds STRIPE_SECRET_KEY and is not in .gitignore"),
        "{text}"
    );
    assert!(
        !text.contains(".env.production"),
        "a file with no secret in it may be committed: {text}"
    );
    assert!(!text.contains(SECRET));

    let ran = workspace.run(&[], "exit 0");
    assert!(
        stderr(&ran).contains("is not in .gitignore"),
        "{}",
        stderr(&ran)
    );

    let init = workspace.penv(&["--format", "text", "init"]);
    assert_eq!(init.status.code(), Some(0), "{}", stderr(&init));
    assert!(stdout(&init).contains("kept"), "{}", stdout(&init));
    let schema = std::fs::read_to_string(workspace.path().join(".env.schema")).unwrap();
    assert_eq!(schema, local_schema(), "the schema written first is kept");
    assert_eq!(workspace.penv(&["check"]).status.code(), Some(0));

    assert!(git(workspace.path(), &["add", "-f", ".env"]));
    let tracked = stdout(&workspace.penv(&["check"]));
    assert!(
        tracked.contains("git tracks it") && tracked.contains("git rm --cached .env"),
        "{tracked}"
    );
}

#[test]
fn setting_a_secret_in_a_fresh_repository_ignores_the_value_files_and_starts_the_schema_cleanly() {
    let workspace = Workspace::new(&[(".env.schema", "")]);
    if !git(workspace.path(), &["init", "-q"]) {
        return;
    }
    let mut set = Command::new(env!("CARGO_BIN_EXE_penv"))
        .env(
            "PENV_LOCAL_KEY",
            "0000000000000000000000000000000000000000000000000000000000000001",
        )
        .current_dir(workspace.path())
        .env_remove("SSL_CERT_FILE")
        .args(["set", "STRIPE_SECRET_KEY"])
        .stdin(std::process::Stdio::piped())
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped())
        .spawn()
        .unwrap();
    use std::io::Write as _;
    set.stdin
        .take()
        .unwrap()
        .write_all(SECRET.as_bytes())
        .unwrap();
    let output = set.wait_with_output().unwrap();
    assert_eq!(output.status.code(), Some(0), "{}", stderr(&output));
    let schema = std::fs::read_to_string(workspace.path().join(".env.schema")).unwrap();
    assert!(
        schema.starts_with("# @type="),
        "no blank lines before the first block: {schema:?}"
    );
    let ignore = std::fs::read_to_string(workspace.path().join(".gitignore")).unwrap();
    assert!(
        ignore.contains(".env\n") && ignore.contains("!.env.schema"),
        "{ignore}"
    );
}
