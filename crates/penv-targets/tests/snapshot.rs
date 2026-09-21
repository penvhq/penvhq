//! One fixture schema through every built-in target, byte for byte.

use penv_targets::{Roots, Source, Target, Tree, load, render};

const FIXTURE: &str = include_str!("fixture.env.schema");
const TS: &str = include_str!("snapshots/ts.env.ts");
const PY: &str = include_str!("snapshots/py.penv_env.py");

/// Pinned so a version bump is not a snapshot change.
const VERSION: &str = "1.0.0-test";

struct Empty;

impl Tree for Empty {
    fn read(&self, _path: &str) -> Option<String> {
        None
    }
    fn dirs(&self, _path: &str) -> Vec<String> {
        Vec::new()
    }
}

fn built_in(name: &str) -> Target {
    let target = load(&Empty, &Roots::new("/repo", None), name).unwrap();
    assert_eq!(target.source, Source::BuiltIn);
    target
}

fn rendered(name: &str) -> String {
    let schema = penv_schema::parse(FIXTURE).expect("the fixture parses");
    render(&built_in(name), &schema.to_json(), VERSION).expect("the fixture renders")
}

/// Point at the first line that differs; the whole file is too long to read in a
/// test failure.
fn assert_same(name: &str, expected: &str, actual: &str) {
    if expected == actual {
        return;
    }
    if std::env::var_os("PENV_BLESS").is_some() {
        std::fs::write(format!("tests/snapshots/{name}"), actual).unwrap();
        panic!("{name} was rewritten from PENV_BLESS; read the diff and run again");
    }
    let mut hint = String::new();
    for (line, (want, got)) in expected.lines().zip(actual.lines()).enumerate() {
        if want != got {
            hint = format!("line {}:\n-{want}\n+{got}", line + 1);
            break;
        }
    }
    if hint.is_empty() {
        hint = format!(
            "same prefix, {} expected line(s) against {} rendered",
            expected.lines().count(),
            actual.lines().count()
        );
    }
    panic!(
        "tests/snapshots/{name} no longer matches the fixture render.\n{hint}\nRe-run with PENV_BLESS=1 once the change is deliberate."
    );
}

#[test]
fn the_ts_target_renders_its_snapshot() {
    assert_same("ts.env.ts", TS, &rendered("ts"));
}

#[test]
fn the_py_target_renders_its_snapshot() {
    assert_same("py.penv_env.py", PY, &rendered("py"));
}

/// `key_case` is the folder's own option, so a repo-local ts target can flip it.
#[test]
fn the_ts_target_renames_its_properties_when_key_case_is_camel() {
    let mut target = built_in("ts");
    target
        .options
        .insert("key_case".into(), toml::Value::String("camel".into()));
    let schema = penv_schema::parse(FIXTURE).expect("the fixture parses");
    let out = render(&target, &schema.to_json(), VERSION).expect("the fixture renders");
    assert!(out.contains(
        "  nextPublicAppUrl: (process.env.NEXT_PUBLIC_APP_URL ?? \"http://localhost:3000\") as string,"
    ));
    assert!(
        out.contains("seen[\"DATABASE_URL\"]"),
        "the schema reads the environment, whatever the properties are called"
    );
    assert!(out.contains("path: [\"DATABASE_URL\"]"));
    assert!(out.contains("message: \"DATABASE_URL is required\""));
}

/// `runtime` is the folder's own option too, and the accessor is the only line
/// that changes with it.
#[test]
fn the_ts_target_reads_through_the_runtime_the_options_name() {
    let reads = |runtime: &str| {
        let mut target = built_in("ts");
        target
            .options
            .insert("runtime".into(), toml::Value::String(runtime.into()));
        let schema = penv_schema::parse(FIXTURE).expect("the fixture parses");
        render(&target, &schema.to_json(), VERSION).expect("the fixture renders")
    };
    // A bundler only inlines a key it can see spelled out.
    assert!(reads("vite").contains("import.meta.env.DATABASE_URL"));
    assert!(reads("deno").contains("Deno.env.get(\"DATABASE_URL\")"));
    assert!(reads("node").contains("process.env.DATABASE_URL"));
    for runtime in ["vite", "deno"] {
        assert!(
            !reads(runtime).contains("process.env"),
            "{runtime} still read process.env"
        );
    }
}

/// Stdlib only unless the folder asks for pydantic, so an import nobody has
/// never lands in a generated file.
#[test]
fn the_py_target_takes_pydantic_types_only_when_its_options_ask_for_them() {
    let mut target = built_in("py");
    target
        .options
        .insert("pydantic".into(), toml::Value::Boolean(true));
    let schema = penv_schema::parse(FIXTURE).expect("the fixture parses");
    let out = render(&target, &schema.to_json(), VERSION).expect("the fixture renders");
    assert!(out.contains("from pydantic import HttpUrl, SecretStr"));
    assert!(out.contains("self.STRIPE_SECRET_KEY: SecretStr = SecretStr("));
    assert!(
        !PY.contains("pydantic"),
        "the default is the standard library"
    );
    assert!(
        PY.contains("self.STRIPE_SECRET_KEY: str = _require("),
        "{PY}"
    );
}

#[test]
fn a_constraint_never_reaches_the_generated_file() {
    for snapshot in [TS, PY] {
        assert!(
            !snapshot.contains("sk_"),
            "startsWith=sk_ leaked into the output"
        );
    }
}
