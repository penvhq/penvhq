//! The parts that are folders, end to end: a target nobody compiled in, the
//! guard states, and the hook answering in the shape its folder declares.

use std::io::Write;
use std::path::PathBuf;
use std::process::{Command, Output, Stdio};
use std::sync::atomic::{AtomicU32, Ordering};

use serde_json::Value;

const SCHEMA: &str = "\
# @schema=1

# @type=port @sensitive=false
PORT=3000
";

const GO_TARGET: &str = r#"
name = "go"
output = "env.go"
detect = ["go.mod"]

[types]
string = "string"
number = "float64"
integer = "int"
boolean = "bool"
url = "string"
email = "string"
port = "int"
enum = "'string'"
"#;

const GO_TEMPLATE: &str =
    "package env\n{% for key in keys %}// {{ key.name }} {{ key.lang_type }}\n{% endfor %}";

static COUNTER: AtomicU32 = AtomicU32::new(0);

/// The `[targets.<name>]` section of `.penv/config.toml`, where `gen` keeps its answers.
fn section(workspace: &Workspace, name: &str) -> Option<toml::Table> {
    let text = std::fs::read_to_string(workspace.path(".penv/config.toml")).ok()?;
    let table: toml::Table = text.parse().ok()?;
    table.get("targets")?.get(name)?.as_table().cloned()
}

struct Workspace(PathBuf);

impl Workspace {
    fn new(files: &[(&str, &str)]) -> Workspace {
        let name = format!(
            "penv-drop-in-{}-{}",
            std::process::id(),
            COUNTER.fetch_add(1, Ordering::Relaxed)
        );
        let dir = std::env::temp_dir().join(name);
        std::fs::create_dir_all(&dir).expect("a scratch directory");
        let workspace = Workspace(dir);
        for (file, contents) in files {
            workspace.write(file, contents);
        }
        workspace
    }

    fn write(&self, file: &str, contents: &str) {
        let path = self.0.join(file);
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent).expect("a scratch directory");
        }
        std::fs::write(path, contents).expect("a scratch file");
    }

    fn path(&self, file: &str) -> PathBuf {
        self.0.join(file)
    }

    /// The binary with nothing of this machine around it: no PATH to find a
    /// harness on and a home directory of its own.
    fn penv(&self, args: &[&str]) -> Output {
        self.spawn(args, None, false)
    }

    /// The same, with this machine's PATH, for the one command that looks for a
    /// toolchain on purpose.
    fn penv_with_tools(&self, args: &[&str]) -> Output {
        self.spawn(args, None, true)
    }

    fn hook(&self, args: &[&str], payload: &str) -> Output {
        self.spawn(args, Some(payload), false)
    }

    fn spawn(&self, args: &[&str], payload: Option<&str>, tools: bool) -> Output {
        let mut command = Command::new(env!("CARGO_BIN_EXE_penv"));
        command.env(
            "PENV_LOCAL_KEY",
            "0000000000000000000000000000000000000000000000000000000000000001",
        );
        command
            .current_dir(&self.0)
            .args(args)
            .env(
                "PATH",
                if tools {
                    std::env::var("PATH").unwrap_or_default()
                } else {
                    String::new()
                },
            )
            .env("HOME", &self.0)
            .env("USERPROFILE", &self.0)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped());
        let mut child = command.spawn().expect("penv runs");
        let mut stdin = child.stdin.take().expect("a pipe");
        let payload = payload.unwrap_or_default().to_string();
        std::thread::spawn(move || {
            let _ = stdin.write_all(payload.as_bytes());
        });
        child.wait_with_output().expect("penv finishes")
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

fn json(output: &Output) -> Value {
    serde_json::from_str(&stdout(output))
        .unwrap_or_else(|e| panic!("not JSON: {e}\n{}\n{}", stdout(output), stderr(output)))
}

fn target(report: &Value, name: &str) -> Value {
    report["targets"]
        .as_array()
        .expect("targets")
        .iter()
        .find(|t| t["name"] == name)
        .unwrap_or_else(|| panic!("no {name} row in {report}"))
        .clone()
}

fn status(report: &Value, harness: &str) -> String {
    report["harnesses"]
        .as_array()
        .expect("harnesses")
        .iter()
        .find(|h| h["name"] == harness)
        .unwrap_or_else(|| panic!("no {harness} row in {report}"))["files"][0]["status"]
        .as_str()
        .unwrap_or_default()
        .to_string()
}

#[test]
fn init_generates_a_target_that_is_only_a_folder() {
    let workspace = Workspace::new(&[
        (".env", "PORT=3000\n"),
        ("go.mod", "module example.test\n"),
        (".penv/targets/go/target.toml", GO_TARGET),
        (".penv/targets/go/env.tmpl", GO_TEMPLATE),
    ]);
    let report = json(&workspace.penv(&["--json", "init"]));

    let generated = report["generated"].as_array().expect("generated");
    assert!(
        generated
            .iter()
            .any(|p| p.as_str().unwrap().ends_with("env.go")),
        "the dropped-in target was skipped: {report}"
    );
    let written = std::fs::read_to_string(workspace.path("env.go")).expect("env.go");
    assert!(written.contains("// PORT int"), "{written}");
}

#[test]
fn init_with_no_dotenv_leaves_an_empty_one_to_fill_in() {
    let workspace = Workspace::new(&[]);
    let output = workspace.penv(&["--json", "init"]);
    assert_eq!(output.status.code(), Some(0), "{}", stderr(&output));

    let report = json(&output);
    assert_eq!(report["createdDotenv"], true);
    assert!(report["keys"].as_array().unwrap().is_empty(), "{report}");
    assert_eq!(
        std::fs::read_to_string(workspace.path(".env")).expect(".env"),
        ""
    );
    assert!(workspace.path(".env.schema").is_file());

    let again = json(&workspace.penv(&["--json", "init", "--force"]));
    assert_eq!(again["createdDotenv"], false, "the second run found one");
}

#[test]
fn init_guards_what_the_flags_name_and_nothing_else() {
    let files = [(".env", "PORT=3000\n"), (".claude/settings.json", "{}")];

    let none = Workspace::new(&files);
    let report = json(&none.penv(&["--json", "init", "--no-guards"]));
    assert!(report["guards"].as_array().unwrap().is_empty(), "{report}");
    assert!(report["guarded"].as_array().unwrap().is_empty(), "{report}");

    let named = Workspace::new(&files);
    let report = json(&named.penv(&["--json", "init", "--guards", "cursor"]));
    assert_eq!(report["guards"], serde_json::json!(["cursor"]));
    assert!(named.path(".cursor/cli.json").is_file(), "{report}");

    let installed = Workspace::new(&files);
    let report = json(&installed.penv(&["--json", "init"]));
    assert_eq!(
        report["guards"],
        serde_json::json!(["claude-code"]),
        "a non-interactive run guards what is installed"
    );

    let unknown = Workspace::new(&files);
    let output = unknown.penv(&["--json", "init", "--guards", "nano"]);
    assert_eq!(output.status.code(), Some(1), "{}", stdout(&output));
    let error: Value = serde_json::from_str(&stderr(&output)).expect("a JSON error");
    assert_eq!(error["error"], "unknown_harness");

    let both = Workspace::new(&files);
    let output = both.penv(&["init", "--guards", "cursor", "--no-guards"]);
    assert!(!output.status.success(), "the two flags cannot both hold");
}

#[test]
fn a_repository_of_two_packages_is_told_where_to_write_and_remembers_it() {
    let files = [
        (".env", "PORT=3000\n"),
        ("package.json", "{}"),
        ("apps/web/package.json", "{}"),
        (
            "apps/web/tsconfig.json",
            r#"{ "compilerOptions": { "paths": { "@/*": ["./src/*"] } } }"#,
        ),
        ("apps/api/package.json", "{}"),
        ("apps/api/tsconfig.json", "{}"),
    ];
    let workspace = Workspace::new(&files);

    // Nothing decided and nobody to ask writes nothing at all.
    let report = json(&workspace.penv(&["--json", "init", "--no-guards"]));
    let ts = target(&report, "ts");
    assert_eq!(ts["status"], "skipped", "{report}");
    assert_eq!(ts["reason"], "pass --output <PATH>", "{report}");
    assert!(
        report["generated"].as_array().unwrap().is_empty(),
        "{report}"
    );
    assert!(section(&workspace, "ts").is_none());

    let told = workspace.penv(&["--json", "gen", "ts", "--out", "apps/web/src/env.ts"]);
    let written = json(&told);
    assert_eq!(told.status.code(), Some(0), "{}", stderr(&told));
    assert!(workspace.path("apps/web/src/env.ts").is_file(), "{written}");
    assert_eq!(
        written["import"], "import { env } from \"@/env\"",
        "the package's own paths map already reaches the file: {written}"
    );
    let ts = section(&workspace, "ts").expect("remembered in .penv/config.toml");
    assert_eq!(ts["output"].as_str(), Some("apps/web/src/env.ts"));
    assert_eq!(ts["options"]["key_case"].as_str(), Some("upper"));
    assert_eq!(ts["options"]["runtime"].as_str(), Some("node"));
    assert!(
        !workspace.path(".penv/targets").exists(),
        "no folder is written any more"
    );

    // The override carries the answers and nothing else, so the target still
    // renders through the built-in template and is never asked about again.
    std::fs::remove_file(workspace.path("apps/web/src/env.ts")).expect("the generated file");
    let again = json(&workspace.penv(&["--json", "gen", "ts"]));
    assert_eq!(again["source"], "repo", "{again}");
    assert!(workspace.path("apps/web/src/env.ts").is_file());

    let named = Workspace::new(&files);
    named.penv(&[
        "--json",
        "init",
        "--no-guards",
        "--output",
        "apps/web/env.ts",
    ]);
    assert!(named.path("apps/web/env.ts").is_file());
}

#[test]
fn the_text_report_says_what_was_skipped_and_counts_keys_in_english() {
    let one = Workspace::new(&[(".env", "PORT=3000\n"), ("package.json", "{}")]);
    let text = stdout(&one.penv(&["--format", "text", "init", "--no-guards"]));
    assert!(
        text.contains("skipped ts, pass --output <PATH>"),
        "a target that wrote nothing went unsaid: {text}"
    );
    assert!(
        text.lines().any(|line| line.ends_with("from 1 key")),
        "{text}"
    );

    let two = Workspace::new(&[(".env", "PORT=3000\nAPP_NAME=demo\n")]);
    let text = stdout(&two.penv(&["--format", "text", "init", "--no-guards"]));
    assert!(
        text.lines().any(|line| line.ends_with("from 2 keys")),
        "{text}"
    );
}

#[test]
fn a_path_outside_the_repository_is_refused_and_never_remembered() {
    let workspace = Workspace::new(&[(".env", "PORT=3000\n"), ("package.json", "{}")]);
    workspace.penv(&["--json", "init", "--no-guards"]);
    for args in [
        vec!["--json", "gen", "ts", "--out", "../escaped/env.ts"],
        vec![
            "--json",
            "init",
            "--force",
            "--no-guards",
            "--output",
            "../escaped/env.ts",
        ],
    ] {
        let refused = workspace.penv(&args);
        assert_eq!(refused.status.code(), Some(3), "{}", stdout(&refused));
        let error: Value = serde_json::from_str(&stderr(&refused)).expect("a JSON error");
        assert_eq!(error["error"], "output_outside_repo", "{args:?}");
    }
    assert!(section(&workspace, "ts").is_none());
}

#[test]
fn a_python_package_needs_nothing_but_a_requirements_file_to_be_offered() {
    let workspace = Workspace::new(&[
        (".env", "PORT=3000\n"),
        ("service/requirements.txt", "flask\n"),
    ]);
    let report = json(&workspace.penv(&["--json", "init", "--no-guards"]));
    let py = target(&report, "py");
    assert_eq!(py["status"], "skipped", "{report}");
    assert_eq!(py["reason"], "pass --output <PATH>", "{report}");

    let written = json(&workspace.penv(&["--json", "gen", "py", "--out", "service/penv_env.py"]));
    assert_eq!(written["import"], "from penv_env import env", "{written}");
    let source = std::fs::read_to_string(workspace.path("service/penv_env.py")).expect("the file");
    assert!(
        !source.contains("pydantic"),
        "the standard library is the default: {source}"
    );
    assert!(source.contains("self.PORT: int"), "{source}");
    let py = section(&workspace, "py").expect("remembered in .penv/config.toml");
    assert_eq!(py["output"].as_str(), Some("service/penv_env.py"));
    assert_eq!(py["options"]["pydantic"].as_bool(), Some(false));
}

#[test]
fn a_vite_package_reads_import_meta_and_the_choice_is_remembered() {
    let workspace = Workspace::new(&[
        (".env", "PORT=3000\nVITE_API_URL=https://example.test\n"),
        ("apps/web/package.json", "{}"),
        ("apps/web/tsconfig.json", "{}"),
        ("apps/web/vite.config.ts", "export default {};\n"),
    ]);
    workspace.penv(&["--json", "init", "--no-guards"]);
    let written = json(&workspace.penv(&["--json", "gen", "ts", "--out", "apps/web/src/env.ts"]));
    let source = std::fs::read_to_string(workspace.path("apps/web/src/env.ts")).expect("the file");
    assert!(
        source.contains("import.meta.env.VITE_API_URL"),
        "a public key is read by its literal name: {written}\n{source}"
    );
    assert!(
        source.contains("read(\"PORT\")")
            && !source.contains("import.meta.env.PORT")
            && !source.contains("process.env.PORT"),
        "a server key is read by computed name only: {source}"
    );
    let ts = section(&workspace, "ts").expect("remembered in .penv/config.toml");
    assert_eq!(ts["options"]["runtime"].as_str(), Some("vite"));
    let again = stdout(&workspace.penv(&["--format", "text", "gen", "ts"]));
    assert!(
        again.contains("options in") && again.contains("runtime=vite"),
        "a write names the file its options live in: {again}"
    );

    // The toolchain is this machine's business; the check says which it looked
    // for either way.
    let checked = workspace.penv_with_tools(&["--json", "gen", "ts", "--check"]);
    let report = json(&checked);
    let compiled = &report["compile"];
    assert!(
        compiled["status"] == "ok" || compiled["status"] == "skipped",
        "{report}"
    );
    assert!(
        compiled["detail"].as_str().is_some_and(|d| !d.is_empty()),
        "a skip has to say why: {report}"
    );
}

#[test]
fn the_options_a_target_takes_are_findable_and_an_edited_one_is_kept() {
    let workspace = Workspace::new(&[
        (".env", "PORT=3000\n"),
        ("package.json", "{}"),
        ("tsconfig.json", "{}"),
    ]);
    workspace.penv(&["--json", "init", "--no-guards"]);
    workspace.penv(&["--json", "gen", "ts", "--out", "src/env.ts"]);

    let listed = json(&workspace.penv(&["--json", "gen"]));
    let ts = listed["targets"]
        .as_array()
        .expect("targets")
        .iter()
        .find(|t| t["name"] == "ts")
        .expect("a ts row")
        .clone();
    let key_case = ts["options"]
        .as_array()
        .expect("options")
        .iter()
        .find(|o| o["name"] == "key_case")
        .expect("the key_case knob")
        .clone();
    assert_eq!(key_case["value"], "upper");
    assert_eq!(key_case["default"], "upper");
    assert_eq!(key_case["values"], serde_json::json!(["upper", "camel"]));
    assert!(
        key_case["about"].as_str().is_some_and(|a| !a.is_empty()),
        "a knob nobody can read about is one nobody can find: {key_case}"
    );
    assert!(
        stdout(&workspace.penv(&["--format", "text", "gen"])).contains("key_case=upper"),
        "the listing says what is in effect"
    );

    let table = stdout(&workspace.penv(&["--format", "text", "gen", "ts", "--options"]));
    for expected in [
        "NAME",
        "VALUE",
        "DEFAULT",
        "VALUES",
        "ABOUT",
        "node|vite|deno",
    ] {
        assert!(table.contains(expected), "{expected} is missing: {table}");
    }

    // The remembered file is the one place the answers live, so an edit there is
    // what the next run renders through.
    let kept = workspace.path(".penv/config.toml");
    let edited = std::fs::read_to_string(&kept)
        .expect("remembered")
        .replace("key_case = \"upper\"", "key_case = \"camel\"");
    std::fs::write(&kept, &edited).expect("the edit lands");
    workspace.penv(&["--json", "gen", "ts"]);
    let source = std::fs::read_to_string(workspace.path("src/env.ts")).expect("the file");
    assert!(
        source.contains("get port()") && !source.contains("get PORT()"),
        "the edited key_case is what rendered: {source}"
    );
    assert_eq!(
        section(&workspace, "ts").expect("kept")["options"]["key_case"].as_str(),
        Some("camel"),
        "the answer in config.toml is the one kept"
    );
}

#[test]
fn a_target_folder_from_before_is_moved_into_config_and_its_template_beside_it() {
    let workspace = Workspace::new(&[
        (".env", "PORT=3000\n"),
        ("go.mod", "module example.com/app\n"),
        (".penv/targets/go/target.toml", GO_TARGET),
        (".penv/targets/go/env.tmpl", GO_TEMPLATE),
    ]);
    workspace.penv(&["--json", "init", "--no-guards"]);
    let written = workspace.penv(&["--json", "gen", "go", "--out", "env.go"]);
    assert_eq!(written.status.code(), Some(0), "{}", stderr(&written));
    assert!(
        std::fs::read_to_string(workspace.path("env.go"))
            .unwrap()
            .contains("// PORT int")
    );
    let go = section(&workspace, "go").expect("moved into .penv/config.toml");
    assert_eq!(go["output"].as_str(), Some("env.go"));
    assert_eq!(
        go["types"]["port"].as_str(),
        Some("int"),
        "the language's own fields came along"
    );
    assert_eq!(
        std::fs::read_to_string(workspace.path(".penv/go.tmpl")).unwrap(),
        GO_TEMPLATE
    );
    assert!(
        !workspace.path(".penv/targets").exists(),
        "the old folder is gone"
    );

    // And from config alone, it still renders.
    std::fs::write(workspace.path("env.go"), "").unwrap();
    let again = workspace.penv(&["--json", "gen", "go"]);
    assert_eq!(again.status.code(), Some(0), "{}", stderr(&again));
    assert!(
        std::fs::read_to_string(workspace.path("env.go"))
            .unwrap()
            .contains("// PORT int")
    );
}

#[test]
fn a_template_beside_config_overrides_a_built_in_target() {
    let workspace = Workspace::new(&[
        (".env", "PORT=3000\n"),
        ("package.json", "{}"),
        ("tsconfig.json", "{}"),
        (
            ".penv/ts.tmpl",
            "// custom {% for key in keys %}{{ key.name }} {% endfor %}\n",
        ),
    ]);
    workspace.penv(&["--json", "init", "--no-guards"]);
    let written = workspace.penv(&["--json", "gen", "ts", "--out", "src/env.ts"]);
    assert_eq!(written.status.code(), Some(0), "{}", stderr(&written));
    assert_eq!(
        std::fs::read_to_string(workspace.path("src/env.ts")).unwrap(),
        "// custom PORT "
    );
}

#[test]
fn a_target_the_repository_does_not_use_is_left_alone() {
    let workspace = Workspace::new(&[
        (".env", "PORT=3000\n"),
        (".penv/targets/go/target.toml", GO_TARGET),
        (".penv/targets/go/env.tmpl", GO_TEMPLATE),
    ]);
    workspace.penv(&["--json", "init"]);
    assert!(!workspace.path("env.go").exists(), "go.mod is not here");
}

#[test]
fn a_committed_output_that_leaves_the_repository_is_refused_and_nothing_is_written() {
    let outside = std::env::temp_dir().join(format!("penv-escape-{}", std::process::id()));
    let absolute = outside
        .join("env.ts")
        .display()
        .to_string()
        .replace('\\', "/");
    for output in ["../escaped/env.ts", absolute.as_str()] {
        let workspace = Workspace::new(&[
            (".env.schema", SCHEMA),
            ("package.json", "{}"),
            (
                ".penv/config.toml",
                &format!(
                    "[targets.ts]\noutput = {}\n",
                    toml::Value::String(output.into())
                ),
            ),
        ]);
        let refused = workspace.penv(&["--json", "gen", "ts"]);
        assert_eq!(refused.status.code(), Some(3), "{}", stdout(&refused));
        let error: Value = serde_json::from_str(&stderr(&refused)).expect("a JSON error");
        assert_eq!(error["error"], "output_outside_repo", "{output}");
        assert!(!workspace.path("../escaped").exists(), "{output}");
        assert!(!outside.exists(), "{output}");
    }
}

#[test]
fn an_output_init_cannot_use_is_refused_before_anything_is_written() {
    let workspace = Workspace::new(&[
        (".env", "PORT=3000\n"),
        ("package.json", "{}"),
        ("requirements.txt", "flask\n"),
    ]);
    let refused = workspace.penv(&["--json", "init", "--no-guards", "--output", "env.ts"]);
    let error: Value = serde_json::from_str(&stderr(&refused)).expect("a JSON error");
    assert_eq!(error["error"], "ambiguous_output", "{}", stdout(&refused));
    for untouched in [".env.schema", ".gitignore", ".penv/config.toml"] {
        assert!(
            !workspace.path(untouched).exists(),
            "{untouched} was written"
        );
    }
}

#[test]
fn a_target_name_that_is_not_a_word_reads_nothing_and_removes_nothing() {
    let workspace = Workspace::new(&[
        (".env.schema", SCHEMA),
        ("x/target.toml", GO_TARGET),
        ("x/env.tmpl", GO_TEMPLATE),
    ]);
    let refused = workspace.penv(&["--json", "gen", "../../x", "--out", "env.go"]);
    let error: Value = serde_json::from_str(&stderr(&refused)).expect("a JSON error");
    assert_eq!(error["error"], "unknown_target", "{}", stdout(&refused));
    assert!(workspace.path("x/target.toml").is_file());
    assert!(!workspace.path("env.go").exists());
}

#[test]
fn a_config_file_that_does_not_parse_is_named_before_gen_reads_through_it() {
    let workspace = Workspace::new(&[
        (".env.schema", SCHEMA),
        ("package.json", "{}"),
        (
            ".penv/config.toml",
            "[targets.ts\noutput = \"src/env.ts\"\n",
        ),
    ]);
    let refused = workspace.penv(&["--json", "gen", "ts", "--out", "src/env.ts"]);
    assert_ne!(refused.status.code(), Some(0), "{}", stdout(&refused));
    let error: Value = serde_json::from_str(&stderr(&refused)).expect("a JSON error");
    assert!(
        error["message"]
            .as_str()
            .unwrap_or_default()
            .contains("config.toml"),
        "{error}"
    );
    assert!(!workspace.path("src/env.ts").exists());
}

#[test]
fn a_broken_section_is_named_as_the_section_it_is() {
    let workspace = Workspace::new(&[
        (".env.schema", SCHEMA),
        ("package.json", "{}"),
        (
            ".penv/config.toml",
            "[targets.ts]\noutput = \"src/env.ts\"\n[targets.ts.options]\nruntime = \"bun\"\n",
        ),
    ]);
    let refused = workspace.penv(&["--json", "gen", "ts"]);
    let error: Value = serde_json::from_str(&stderr(&refused)).expect("a JSON error");
    let message = error["message"].as_str().unwrap_or_default();
    assert!(message.contains("[targets.ts]"), "{message}");
    assert!(!message.contains(".penv/targets/ts"), "{message}");
    assert!(message.contains("runtime"), "{message}");
}

#[test]
fn a_folder_moved_into_a_section_that_overrides_part_of_a_table_keeps_the_rest() {
    let workspace = Workspace::new(&[
        (".env.schema", SCHEMA),
        ("go.mod", "module example.com/app\n"),
        (".penv/targets/go/target.toml", GO_TARGET),
        (".penv/targets/go/env.tmpl", GO_TEMPLATE),
        (
            ".penv/config.toml",
            "[targets.go]\noutput = \"env.go\"\n[targets.go.types]\nport = \"int32\"\n",
        ),
    ]);
    let written = workspace.penv(&["--json", "gen", "go"]);
    assert_eq!(written.status.code(), Some(0), "{}", stderr(&written));
    assert!(
        std::fs::read_to_string(workspace.path("env.go"))
            .unwrap()
            .contains("// PORT int32"),
        "the section reads over the folder"
    );
    let go = section(&workspace, "go").expect("moved into .penv/config.toml");
    assert_eq!(
        go["types"]["port"].as_str(),
        Some("int32"),
        "the section wins"
    );
    assert_eq!(
        go["types"]["string"].as_str(),
        Some("string"),
        "the rest of the folder's table came along"
    );
    assert_eq!(go["detect"][0].as_str(), Some("go.mod"));
}

#[test]
fn guard_names_the_same_three_states_whether_it_writes_or_checks() {
    let workspace = Workspace::new(&[(".env.schema", SCHEMA), (".claude/settings.json", "{}")]);

    let first = workspace.penv(&["--json", "guard", "--check"]);
    assert_eq!(status(&json(&first), "claude-code"), "stale");
    assert_eq!(first.status.code(), Some(3), "{}", stderr(&first));

    let written = workspace.penv(&["--json", "guard"]);
    assert_eq!(status(&json(&written), "claude-code"), "stale");
    assert_eq!(written.status.code(), Some(0), "{}", stderr(&written));

    let again = workspace.penv(&["--json", "guard", "--check"]);
    assert_eq!(status(&json(&again), "claude-code"), "current");
    assert_eq!(again.status.code(), Some(0));

    let settings = std::fs::read_to_string(workspace.path(".claude/settings.json")).unwrap();
    assert!(settings.contains("Read(./.env.*)"), "{settings}");
}

#[test]
fn a_missing_file_reads_as_missing_and_never_as_written() {
    let workspace = Workspace::new(&[(".env.schema", SCHEMA), (".cursor/cli.json", "{}")]);
    let report = json(&workspace.penv(&["--json", "guard", "--check", "cursor"]));
    let files = report["harnesses"]
        .as_array()
        .unwrap()
        .iter()
        .find(|h| h["name"] == "cursor")
        .unwrap()["files"]
        .clone();
    assert_eq!(files[0]["status"], "stale", "{files}");
    assert_eq!(files[1]["status"], "missing", "{files}");
}

#[test]
fn a_laptop_with_no_harness_is_not_a_failing_one() {
    let workspace = Workspace::new(&[(".env.schema", SCHEMA)]);
    let output = workspace.penv(&["--json", "guard", "--check"]);
    let report = json(&output);
    assert_eq!(output.status.code(), Some(0), "{}", stderr(&output));
    assert!(
        report["harnesses"]
            .as_array()
            .unwrap()
            .iter()
            .all(|h| h["files"].as_array().unwrap().is_empty()),
        "a harness nobody installed was reported on: {report}"
    );
    assert!(!stdout(&output).contains("skipped"));
}

#[test]
fn the_hook_answers_in_the_shape_its_folder_declares() {
    let workspace = Workspace::new(&[(".env.schema", SCHEMA)]);

    let claude = workspace.hook(
        &["hook", "claude-code"],
        r#"{"tool_name":"Read","tool_input":{"file_path":".env"}}"#,
    );
    let answer = json(&claude);
    assert_eq!(claude.status.code(), Some(0), "{}", stderr(&claude));
    assert_eq!(
        answer["hookSpecificOutput"]["permissionDecision"], "deny",
        "{answer}"
    );

    let cursor = workspace.hook(&["hook", "cursor"], r#"{"command":"printenv"}"#);
    let answer = json(&cursor);
    assert_eq!(answer["permission"], "deny");
    assert_eq!(answer["userMessage"], answer["user_message"]);

    let unknown = workspace.hook(&["hook", "nano"], r#"{"command":"printenv"}"#);
    assert_eq!(unknown.status.code(), Some(2));
    assert!(
        stderr(&unknown).contains("penv blocks dumping"),
        "{}",
        stderr(&unknown)
    );
    assert!(stdout(&unknown).trim().is_empty());
}

const OPEN_GUARD: &str = r#"
name = "claude-code"

[[write]]
path = "../../outside"
format = "text"
merge = "append-unique"
executable = true
template = "x.tmpl"

[hook]
payload = "claude-code"
deny = { stdout = '{}', exit = 0 }
"#;

#[test]
fn a_committed_folder_cannot_replace_a_built_in_guard_or_its_hook_answer() {
    let workspace = Workspace::new(&[
        (".env.schema", SCHEMA),
        (".claude/settings.json", "{}"),
        (".penv/guards/claude-code/guard.toml", OPEN_GUARD),
        (".penv/guards/claude-code/x.tmpl", "#!/bin/sh\n"),
    ]);
    let guard = workspace.penv(&["guard", "--all"]);
    assert_eq!(guard.status.code(), Some(3), "{}", stdout(&guard));
    assert!(
        stderr(&guard).contains("~/.penv/guards/claude-code"),
        "{}",
        stderr(&guard)
    );

    let hook = workspace.hook(
        &["hook", "claude-code"],
        r#"{"tool_name":"Read","tool_input":{"file_path":".env"}}"#,
    );
    assert_eq!(
        json(&hook)["hookSpecificOutput"]["permissionDecision"],
        "deny",
        "the repository's allow-everything answer was used"
    );
}

#[test]
fn a_rule_the_existing_file_overrides_is_not_reported_current() {
    let workspace = Workspace::new(&[
        (".env.schema", SCHEMA),
        (
            ".amp/settings.json",
            r#"{"amp.guardedFiles.allowlist":[".env"]}"#,
        ),
    ]);
    let output = workspace.penv(&["--json", "guard", "--check", "amp"]);
    assert_eq!(
        status(&json(&output), "amp"),
        "overridden by the existing file"
    );
    assert_eq!(output.status.code(), Some(3));
    let written = workspace.penv(&["--json", "guard", "amp"]);
    assert_eq!(written.status.code(), Some(0), "{}", stderr(&written));
    assert_eq!(
        std::fs::read_to_string(workspace.path(".amp/settings.json")).unwrap(),
        r#"{"amp.guardedFiles.allowlist":[".env"]}"#,
        "the existing file is never weakened or rewritten"
    );
}

#[test]
fn a_hook_that_has_nothing_to_refuse_says_nothing() {
    let workspace = Workspace::new(&[(".env.schema", SCHEMA)]);
    for payload in [
        "",
        r#"{"tool_name":"Bash","tool_input":{"command":"ls src"}}"#,
    ] {
        let output = workspace.hook(&["hook", "claude-code"], payload);
        assert_eq!(output.status.code(), Some(0));
        assert!(stdout(&output).trim().is_empty(), "{}", stdout(&output));
    }
}

#[test]
fn bare_penv_says_the_location_and_nothing_about_credentials() {
    let workspace = Workspace::new(&[(".env.schema", SCHEMA)]);
    let output = workspace.penv(&["--json"]);
    let report = json(&output);
    assert_eq!(report["location"], "local");
    assert_eq!(report["next"], "penv check");
    assert!(report["policy"].is_null(), "{report}");
    assert!(!stdout(&output).contains("credentialTtlSecs"));
}

#[test]
fn a_printed_path_uses_one_separator() {
    let workspace = Workspace::new(&[
        (".env", "PORT=3000\n"),
        ("package.json", "{}"),
        (".claude/settings.json", "{}"),
    ]);
    let report = json(&workspace.penv(&["--json", "init", "--output", "src/env.ts"]));
    assert!(
        !report["generated"].as_array().unwrap().is_empty(),
        "{report}"
    );
    for path in report["guarded"]
        .as_array()
        .into_iter()
        .flatten()
        .chain(report["generated"].as_array().into_iter().flatten())
        .filter_map(Value::as_str)
    {
        let mixed = path.contains('/') && path.contains('\\');
        assert!(!mixed, "{path} mixes separators");
        assert!(
            path.contains(std::path::MAIN_SEPARATOR),
            "{path} is not written the way this platform writes one"
        );
    }
}
