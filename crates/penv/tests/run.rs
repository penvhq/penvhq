//! `run` end to end: the binary spawns a real child, injects the values into it
//! and scrubs what comes back. Everything else about `run` is a unit test.

use std::path::{Path, PathBuf};
use std::process::{Command, Output};
use std::sync::atomic::{AtomicU32, Ordering};

const SECRET: &str = "sk_test_FAKE0000";

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
            std::fs::write(dir.join(file), contents).expect("a scratch file");
        }
        Workspace(dir)
    }

    fn path(&self) -> &Path {
        &self.0
    }

    /// `penv --agent run [args] -- <shell> <flag> <script>`.
    fn run(&self, args: &[&str], script: &str) -> Output {
        let mut command = Command::new(env!("CARGO_BIN_EXE_penv"));
        command.current_dir(&self.0).arg("--agent").arg("run");
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
fn another_environment_is_refused_with_exit_six() {
    let workspace = Workspace::new(&[
        (".env.schema", &local_schema()),
        (".env", &format!("STRIPE_SECRET_KEY={SECRET}\n")),
    ]);
    let output = workspace.run(&["--env", "production"], ECHO_VALUES);

    assert_eq!(output.status.code(), Some(6));
    assert!(stderr(&output).contains("cloud"), "{}", stderr(&output));
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
        .current_dir(workspace.path())
        .env("PENV_URL", "http://127.0.0.1:1")
        .env_remove("PENV_TOKEN")
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
        command
            .current_dir(workspace.path())
            .env("PENV_URL", "http://127.0.0.1:1")
            .env_remove("PENV_TOKEN")
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
